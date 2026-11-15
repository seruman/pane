//! End-to-end tests enter through real Luau tasks and Rune's completion scheduler.

use super::*;
use rune_luau::{Data, Script, ScriptOptions, Sources};
use std::time::{Duration, Instant};

fn app(root: PathBuf, body: &str, capabilities: Capabilities) -> rune::Runtime<Script> {
    let source = format!(
        r#"
local ui = require('@rune/ui')
local report = {{ done = false }}
local function must(value, err)
    assert(value ~= nil, if err then err.message else 'unexpected nil')
    return value
end
task.spawn(function()
    local ok, err = pcall(function()
        {body}
    end)
    report = {{ done = true, ok = ok, error = if ok then nil else tostring(err) }}
end)
return {{
    result = function() return report end,
    render = function() return ui.text(if report.done then 'done' else 'waiting') end,
}}
"#
    );
    let script = Script::with_options(
        "host-test",
        Sources::single(source),
        ScriptOptions::new().setup(move |host| install(host, root.clone(), capabilities)),
    );
    assert!(script.error().is_none(), "{:?}", script.error());
    let mut rt = rune::Runtime::new(
        script,
        |s, cx| s.view(cx),
        (200.0, 100.0),
        rune::Env::default(),
    );
    rt.poll = Some(|s, now, tx| s.poll(now, tx));
    rt.frame(0.0);
    rt
}

fn finish(rt: &mut rune::Runtime<Script>) {
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut clock = 0.01;
    loop {
        rt.frame(clock);
        clock += 0.01;
        let result = rt.state.query("result", []).unwrap();
        if result.get("done") == &Data::Bool(true) {
            assert_eq!(result.get("ok"), &Data::Bool(true), "{result:?}");
            break;
        }
        assert!(
            Instant::now() < deadline,
            "host task stuck: {:?}",
            rt.state.log()
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn all() -> Capabilities {
    Capabilities {
        http: true,
        fs: true,
        process: true,
    }
}

#[test]
fn capabilities_are_opt_in_but_json_is_always_available() {
    let root = tempfile::tempdir().unwrap();
    let mut rt = app(
        root.path().into(),
        r#"
local http = require('@pane/http')
local fs = require('@pane/fs')
local process = require('@pane/process')
local json = require('@pane/json')
local value, err = http.request('http://127.0.0.1:1')
assert(value == nil and err.kind == 'permission_denied')
value, err = fs.open('missing')
assert(value == nil and err.kind == 'permission_denied')
value, err = process.spawn('missing', {})
assert(value == nil and err.kind == 'permission_denied')
assert(must(json.encode(must(json.decode('{"a":[]}')))) == '{"a":[]}')
"#,
        Capabilities::default(),
    );
    finish(&mut rt);
}

#[test]
fn filesystem_round_trip_preserves_binary_and_uses_script_root() {
    let root = tempfile::tempdir().unwrap();
    let mut rt = app(
        root.path().into(),
        r#"
local fs = require('@pane/fs')
local f = must(fs.open('binary', { mode = 'write' }))
local bytes = string.char(0, 255, 128, 65)
must(f:write(bytes))
must(f:close())
f = must(fs.open('binary'))
assert(must(f:read(1)) == string.char(0))
assert(must(f:readAll()) == string.char(255, 128, 65))
local value, err = f:read()
assert(value == nil and err == nil)
f:close()
assert(must(fs.stat('binary')).size == 4)
assert(must(fs.readDir('.'))[1].name == 'binary')
must(fs.rename('binary', 'renamed'))
must(fs.remove('renamed'))
"#,
        all(),
    );
    finish(&mut rt);
    assert!(!root.path().join("renamed").exists());
}

#[test]
#[cfg(unix)]
fn processes_expose_independent_streams_and_collect_nonzero_status() {
    let root = tempfile::tempdir().unwrap();
    let mut rt = app(
        root.path().into(),
        r#"
local process = require('@pane/process')
local p = must(process.spawn('/bin/sh', { '-c', 'printf out; printf err >&2; exit 7' }))
assert(p.stdin == nil)
local result = must(p:collect())
assert(result.stdout == 'out' and result.stderr == 'err')
assert(result.exitCode == 7 and result.signal == nil)
assert(must(p:wait()).exitCode == 7)
p = must(process.spawn('/bin/cat', {}, { stdin = 'pipe' }))
local out, errors
local data = string.char(0, 255, 65)
task.spawn(function() out = must(p.stdout:readAll()) end)
task.spawn(function() errors = must(p.stderr:readAll()) end)
must(p.stdin:write(data))
must(p.stdin:close())
assert(must(p:wait()).exitCode == 0)
while out == nil or errors == nil do task.wait(0) end
assert(out == data and errors == '')
"#,
        all(),
    );
    finish(&mut rt);
}

#[test]
fn http_facade_returns_headers_then_a_binary_reader() {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut request = [0; 4096];
        assert!(stream.read(&mut request).unwrap() > 0);
        stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 4\r\nX-Test: a\r\nX-Test: b\r\nConnection: close\r\n\r\n\0\xffok").unwrap();
    });
    let root = tempfile::tempdir().unwrap();
    let mut rt = app(
        root.path().into(),
        &format!(
            r#"
local http = require('@pane/http')
local r = must(http.request('{url}'))
assert(r.status == 404 and r.headers['x-test'][1] == 'a' and r.headers['x-test'][2] == 'b')
assert(must(r.body:readAll()) == string.char(0, 255) .. 'ok')
r.body:close()
"#
        ),
        all(),
    );
    finish(&mut rt);
    server.join().unwrap();
}

#[test]
#[cfg(unix)]
fn cancelling_an_awaiting_task_stops_its_process() {
    let root = tempfile::tempdir().unwrap();
    let mut rt = app(
        root.path().into(),
        r#"
local process = require('@pane/process')
local p
local worker = task.spawn(function()
    p = must(process.spawn('/bin/sleep', { '30' }))
    p:wait()
    error('cancelled task resumed')
end)
while p == nil do task.wait(0) end
task.cancel(worker)
local status, err = p:wait()
assert((status and status.signal ~= nil) or (err and err.kind == 'cancelled'))
"#,
        all(),
    );
    finish(&mut rt);
    assert!(
        !rt.state
            .log()
            .iter()
            .any(|line| line.message.contains("cancelled task resumed"))
    );
}

#[test]
fn failed_reload_keeps_old_scope_successful_reload_cancels_it() {
    let root = tempfile::tempdir().unwrap();
    let script = Script::with_options(
        "reload",
        Sources::single("return {}"),
        ScriptOptions::new().setup({
            let root = root.path().to_owned();
            move |host| install(host, root.clone(), all())
        }),
    );
    let old = script
        .with_lua(|lua| lua.app_data_ref::<VmGuard>().unwrap().0.clone())
        .unwrap();
    assert!(script.reload(Sources::single("invalid ! syntax")).is_err());
    assert!(!old.cancel.is_cancelled());
    script.reload(Sources::single("return {} ")).unwrap();
    assert!(old.cancel.is_cancelled());
    let current = script
        .with_lua(|lua| lua.app_data_ref::<VmGuard>().unwrap().0.clone())
        .unwrap();
    drop(script);
    assert!(current.cancel.is_cancelled());
}

#[test]
fn closed_file_handles_release_the_resource_budget() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("input"), b"abc").unwrap();
    let mut rt = app(
        root.path().into(),
        r#"
local fs = require('@pane/fs')
local retained = {}
for i = 1, 100 do
    local f = must(fs.open('input'))
    retained[i] = f
    if i % 2 == 0 then must(f:readAll()) else f:close() end
end
for i = 1, 100 do
    local f = must(fs.open('output', { mode = 'append' }))
    retained[i] = f
    must(f:write('x'))
    must(f:close())
end
"#,
        all(),
    );
    finish(&mut rt);
    assert_eq!(
        std::fs::read(root.path().join("output")).unwrap().len(),
        100
    );
}

#[test]
#[cfg(unix)]
fn reload_kills_a_process_waiting_in_the_old_vm() {
    let root = tempfile::tempdir().unwrap();
    let mut rt = app(
        root.path().into(),
        r#"
local process = require('@pane/process')
local p = must(process.spawn('/bin/sh', { '-c', 'echo $$ > child.pid; exec sleep 30' }))
task.spawn(function() p:wait() end)
"#,
        all(),
    );
    finish(&mut rt);
    let deadline = Instant::now() + Duration::from_secs(3);
    let pid = loop {
        if let Ok(text) = std::fs::read_to_string(root.path().join("child.pid"))
            && let Ok(pid) = text.trim().parse::<i32>()
        {
            break nix::unistd::Pid::from_raw(pid);
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    };
    rt.state.reload(Sources::single("local ui = require('@rune/ui'); return { render = function() return ui.text('fresh') end }")).unwrap();
    rt.frame(10.0);
    while nix::sys::signal::kill(pid, None).is_ok() {
        assert!(Instant::now() < deadline, "old VM child survived reload");
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
#[cfg(unix)]
fn io_playground_files_and_separate_pipes_work_through_ui_handlers() {
    let root = tempfile::tempdir().unwrap();
    let source = include_str!("../../examples/io/init.luau");
    std::fs::write(root.path().join("init.luau"), source).unwrap();
    let script = Script::with_options(
        "demo",
        Sources::single(source),
        ScriptOptions::new()
            .palette(crate::palette::current())
            .setup({
                let root = root.path().to_owned();
                move |host| install(host, root.clone(), all())
            }),
    );
    assert!(script.error().is_none(), "{:?}", script.error());
    let mut rt = rune::Runtime::new(
        script,
        |s, cx| s.view(cx),
        (640.0, 480.0),
        rune::Env::default(),
    );
    rt.poll = Some(|s, now, tx| s.poll(now, tx));
    rt.frame(0.0);
    let mut clock = 0.01;
    for (button, expected) in [
        ("Files + JSON", "First 240 bytes"),
        ("Stream both pipes", "exit: 0"),
    ] {
        let (x, y) = rt
            .find(button)
            .unwrap_or_else(|| panic!("missing button {button}"));
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            rt.frame(clock);
            clock += 0.01;
            assert!(rt.state.error().is_none(), "{:?}", rt.state.error());
            let texts: Vec<_> = rt.elements().into_iter().filter_map(|e| e.text).collect();
            if texts.iter().any(|text| text.contains(expected))
                && texts.iter().any(|text| text == "Ready")
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "demo did not finish: {texts:?}; logs {:?}",
                rt.state.log()
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    let output = rt
        .elements()
        .into_iter()
        .filter_map(|e| e.text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        output.contains("[stdout]")
            && output.contains("[stderr]")
            && output.contains("output 5")
            && output.contains("error 5"),
        "{output}"
    );
}

#[test]
fn flags_and_limits_are_validated() {
    let (path, caps) = crate::arguments(&[
        "--allow-fs".into(),
        "scripts/demo".into(),
        "--allow-http".into(),
    ])
    .unwrap();
    assert_eq!(path, PathBuf::from("scripts/demo"));
    assert!(caps.fs && caps.http && !caps.process);
    assert!(crate::arguments(&["--allow-everything".into(), "demo".into()]).is_err());
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("input"), b"12345").unwrap();
    let mut rt = app(
        root.path().into(),
        r#"
local fs = require('@pane/fs')
local f = must(fs.open('input'))
local value, err = f:read(-1)
assert(value == nil and err.kind == 'invalid_argument')
value, err = f:readAll({ maxBytes = 4 })
assert(value == nil and err.kind == 'limit')
local process = require('@pane/process')
value, err = process.spawn('must-not-execute', { [1] = 'one', [3] = 'three' })
assert(value == nil and err.kind == 'invalid_argument')
value, err = process.spawn('must-not-execute', { name = 'not an array' })
assert(value == nil and err.kind == 'invalid_argument')
local http = require('@pane/http')
value, err = http.request('http://127.0.0.1:1', { headers = { ['X-Test'] = table.create(129, 'x') } })
assert(value == nil and err.kind == 'limit')
value, err = http.request('http://127.0.0.1:1', { headers = { ['X-Test'] = string.rep('x', 65537) } })
assert(value == nil and err.kind == 'limit')
"#,
        all(),
    );
    finish(&mut rt);
}
