//! Bounded Luau execution and owned, validated values at the VM boundary.

use std::collections::HashSet;
use std::ffi::c_void;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use mlua::{ChunkMode, Error, Function, Lua, LuaOptions, Result, StdLib, Table, Value, VmState};

const MAX_DEPTH: usize = 32;
const MAX_ENTRIES: usize = 4096;
const MAX_NODES: usize = 512;
const MAX_TEXT: usize = 64 * 1024;
const MEMORY_LIMIT: usize = 16 * 1024 * 1024;
const INTERRUPT_LIMIT: usize = 10_000;
const CALL_DEADLINE: Duration = Duration::from_millis(100);

/// A snapshot contains no VM references and cannot be mutated by Lua.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Data(OwnedValue);

#[derive(Clone, Debug, PartialEq)]
enum OwnedValue {
    Nil,
    Boolean(bool),
    Integer(mlua::Integer),
    Number(f64),
    String(String),
    Table(Vec<(Key, OwnedValue)>),
}

#[derive(Clone, Debug, PartialEq)]
enum Key {
    Integer(mlua::Integer),
    String(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Node {
    Text(String),
    Button { label: String, message: String },
    Row(Vec<Node>),
    Column(Vec<Node>),
}

pub(crate) struct Script {
    lua: Lua,
    update: Function,
    view: Function,
}

impl Script {
    pub(crate) fn load(
        name: &str,
        source: &str,
        previous: Option<&Data>,
    ) -> Result<(Self, Data, Node)> {
        let lua = new_vm()?;
        let module: Value = bounded(&lua, "top level", || {
            lua.load(source)
                .set_name(name)
                .set_mode(ChunkMode::Text)
                .eval()
        })?;
        let module = plain_table(module, "script must return a plain table")?;
        let init = required_function(&module, "init")?;
        let update = required_function(&module, "update")?;
        let view = required_function(&module, "view")?;
        let previous = previous.map_or(Ok(Value::Nil), |data| data.restore(&lua))?;
        let initial = bounded(&lua, "init", || init.call::<Value>(previous))?;
        let data = Data::capture(initial)?;
        let script = Self { lua, update, view };
        let node = script.render(&data)?;
        Ok((script, data, node))
    }

    pub(crate) fn update(&self, state: &Data, msg: &str) -> Result<(Data, Node)> {
        let input = state.restore(&self.lua)?;
        let output = bounded(&self.lua, "update", || {
            self.update.call::<Value>((input, msg))
        })?;
        let data = Data::capture(output)?;
        let node = self.render(&data)?;
        Ok((data, node))
    }

    fn render(&self, data: &Data) -> Result<Node> {
        // View gets its own copy; its mutations never become committed state.
        let input = data.restore(&self.lua)?;
        let output = bounded(&self.lua, "view", || self.view.call::<Value>(input))?;
        TreeReader::default().node(output, 1)
    }
}

fn required_function(module: &Table, name: &str) -> Result<Function> {
    match module.raw_get::<Value>(name)? {
        Value::Function(function) => Ok(function),
        _ => Err(Error::runtime(format!("script.{name} must be a function"))),
    }
}

fn new_vm() -> Result<Lua> {
    let libraries = StdLib::TABLE | StdLib::STRING | StdLib::MATH | StdLib::UTF8 | StdLib::BIT;
    let lua = Lua::new_with(libraries, LuaOptions::default())?;
    lua.set_memory_limit(MEMORY_LIMIT)?;
    let globals = lua.globals();
    for name in [
        "require",
        "load",
        "loadstring",
        "loadfile",
        "dofile",
        "getfenv",
        "setfenv",
        "collectgarbage",
        "newproxy",
        "print",
        "warn",
        "debug",
        "io",
        "os",
        "package",
    ] {
        globals.raw_set(name, Value::Nil)?;
    }
    let ui = lua.create_table()?;
    ui.raw_set(
        "text",
        lua.create_function(|lua, text: Value| {
            require_string(&text, "ui.text expects a string")?;
            let node = lua.create_table()?;
            node.raw_set("kind", "text")?;
            node.raw_set("text", text)?;
            Ok(node)
        })?,
    )?;
    ui.raw_set(
        "button",
        lua.create_function(|lua, (label, message): (Value, Value)| {
            require_string(&label, "ui.button label must be a string")?;
            require_string(&message, "ui.button message must be a string")?;
            let node = lua.create_table()?;
            node.raw_set("kind", "button")?;
            node.raw_set("label", label)?;
            node.raw_set("message", message)?;
            Ok(node)
        })?,
    )?;
    for kind in ["row", "column"] {
        ui.raw_set(
            kind,
            lua.create_function(move |lua, children: Value| {
                let children = plain_table(children, "UI children must be a plain array table")?;
                let node = lua.create_table()?;
                node.raw_set("kind", kind)?;
                node.raw_set("children", children)?;
                Ok(node)
            })?,
        )?;
    }
    ui.set_readonly(true);
    globals.raw_set("ui", ui)?;
    lua.sandbox(true)?;
    Ok(lua)
}

fn bounded<T>(lua: &Lua, stage: &'static str, call: impl FnOnce() -> Result<T>) -> Result<T> {
    let start = Instant::now();
    let interrupts = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&interrupts);
    lua.set_interrupt(move |_| {
        if counter.fetch_add(1, Ordering::Relaxed) >= INTERRUPT_LIMIT
            || start.elapsed() >= CALL_DEADLINE
        {
            return Err(Error::runtime(format!(
                "{stage}: execution budget exceeded"
            )));
        }
        Ok(VmState::Continue)
    });
    let result = call();
    lua.remove_interrupt();
    // Protected calls may catch an interrupt error; exhaustion still rejects the result.
    if interrupts.load(Ordering::Relaxed) > INTERRUPT_LIMIT {
        return Err(Error::runtime(format!(
            "{stage}: execution budget exceeded"
        )));
    }
    if start.elapsed() >= CALL_DEADLINE {
        return Err(Error::runtime(format!(
            "{stage}: execution deadline exceeded"
        )));
    }
    result.map_err(|error| Error::runtime(format!("{stage}: {error}")))
}

fn plain_table(value: Value, message: &str) -> Result<Table> {
    match value {
        Value::Table(table) if table.metatable().is_none() => Ok(table),
        _ => Err(Error::runtime(message)),
    }
}

fn require_string<'a>(value: &'a Value, message: &str) -> Result<&'a mlua::String> {
    match value {
        Value::String(text) => Ok(text),
        _ => Err(Error::runtime(message)),
    }
}

fn read_string(value: &Value, bytes: &mut usize, context: &str) -> Result<String> {
    let text = require_string(value, &format!("{context}: expected a string"))?;
    *bytes += text.as_bytes().len();
    if *bytes > MAX_TEXT {
        return Err(Error::runtime(format!(
            "{context}: total strings exceed 64 KiB"
        )));
    }
    text.to_str()
        .map(|text| text.to_owned())
        .map_err(|_| Error::runtime(format!("{context}: strings must be valid UTF-8")))
}

impl Data {
    fn capture(value: Value) -> Result<Self> {
        StateReader::default().value(value, 1).map(Self)
    }

    fn restore(&self, lua: &Lua) -> Result<Value> {
        self.0.restore(lua)
    }
}

impl OwnedValue {
    fn restore(&self, lua: &Lua) -> Result<Value> {
        Ok(match self {
            Self::Nil => Value::Nil,
            Self::Boolean(value) => Value::Boolean(*value),
            Self::Integer(value) => Value::Integer(*value),
            Self::Number(value) => Value::Number(*value),
            Self::String(value) => Value::String(lua.create_string(value)?),
            Self::Table(entries) => {
                let table = lua.create_table()?;
                for (key, value) in entries {
                    let key = match key {
                        Key::Integer(value) => Value::Integer(*value),
                        Key::String(value) => Value::String(lua.create_string(value)?),
                    };
                    table.raw_set(key, value.restore(lua)?)?;
                }
                Value::Table(table)
            }
        })
    }
}

#[derive(Default)]
struct StateReader {
    active: HashSet<*const c_void>,
    entries: usize,
    bytes: usize,
}

impl StateReader {
    fn value(&mut self, value: Value, depth: usize) -> Result<OwnedValue> {
        if depth > MAX_DEPTH {
            return Err(Error::runtime("state: maximum depth 32 exceeded"));
        }
        match value {
            Value::Nil => Ok(OwnedValue::Nil),
            Value::Boolean(value) => Ok(OwnedValue::Boolean(value)),
            Value::Integer(value) => Ok(OwnedValue::Integer(value)),
            Value::Number(value) if value.is_finite() => Ok(OwnedValue::Number(value)),
            Value::String(_) => {
                read_string(&value, &mut self.bytes, "state").map(OwnedValue::String)
            }
            Value::Table(table) => self.table(table, depth),
            _ => Err(Error::runtime(
                "state: expected nil, boolean, finite number, string, or plain table",
            )),
        }
    }

    fn table(&mut self, table: Table, depth: usize) -> Result<OwnedValue> {
        if table.metatable().is_some() {
            return Err(Error::runtime("state: metatables are not allowed"));
        }
        let pointer = table.to_pointer();
        if !self.active.insert(pointer) {
            return Err(Error::runtime("state: cyclic table"));
        }
        let mut entries = Vec::new();
        // pairs() uses raw next, never __pairs or __index.
        for pair in table.pairs::<Value, Value>() {
            self.entries += 1;
            if self.entries > MAX_ENTRIES {
                return Err(Error::runtime("state: maximum 4096 entries exceeded"));
            }
            let (key, value) = pair?;
            let key = match key {
                Value::Integer(value) => Key::Integer(value),
                Value::String(_) => Key::String(read_string(&key, &mut self.bytes, "state")?),
                _ => {
                    return Err(Error::runtime(
                        "state: table keys must be strings or integers",
                    ));
                }
            };
            entries.push((key, self.value(value, depth + 1)?));
        }
        self.active.remove(&pointer);
        Ok(OwnedValue::Table(entries))
    }
}

#[derive(Default)]
struct TreeReader {
    active: HashSet<*const c_void>,
    nodes: usize,
    bytes: usize,
}

impl TreeReader {
    fn node(&mut self, value: Value, depth: usize) -> Result<Node> {
        if depth > MAX_DEPTH {
            return Err(Error::runtime("view tree: maximum depth 32 exceeded"));
        }
        self.nodes += 1;
        if self.nodes > MAX_NODES {
            return Err(Error::runtime("view tree: maximum 512 nodes exceeded"));
        }
        let table = plain_table(
            value,
            "view tree: nodes must be plain tables without metatables",
        )?;
        let pointer = table.to_pointer();
        if !self.active.insert(pointer) {
            return Err(Error::runtime("view tree: cyclic table"));
        }
        let kind = table.raw_get::<Value>("kind")?;
        let kind = require_string(&kind, "view tree: node kind must be a string")?;
        let kind = kind.to_str()?;
        let node = match kind.as_ref() {
            "text" => {
                check_fields(&table, &["kind", "text"])?;
                Node::Text(self.text(&table, "text")?)
            }
            "button" => {
                check_fields(&table, &["kind", "label", "message"])?;
                Node::Button {
                    label: self.text(&table, "label")?,
                    message: self.text(&table, "message")?,
                }
            }
            "row" | "column" => {
                check_fields(&table, &["kind", "children"])?;
                let children = self.children(table.raw_get("children")?, depth + 1)?;
                if kind == "row" {
                    Node::Row(children)
                } else {
                    Node::Column(children)
                }
            }
            _ => return Err(Error::runtime("view tree: unknown node kind")),
        };
        self.active.remove(&pointer);
        Ok(node)
    }

    fn text(&mut self, table: &Table, field: &str) -> Result<String> {
        read_string(&table.raw_get(field)?, &mut self.bytes, "view tree")
    }

    fn children(&mut self, value: Value, depth: usize) -> Result<Vec<Node>> {
        let table = plain_table(value, "view tree: children must be a plain array table")?;
        let pointer = table.to_pointer();
        if !self.active.insert(pointer) {
            return Err(Error::runtime("view tree: cyclic children table"));
        }
        let len = table.raw_len();
        if len > MAX_NODES {
            return Err(Error::runtime("view tree: maximum 512 nodes exceeded"));
        }
        let mut count = 0;
        for pair in table.pairs::<Value, Value>() {
            count += 1;
            let (key, _) = pair?;
            let valid = match key {
                Value::Integer(index) => {
                    usize::try_from(index).is_ok_and(|index| index > 0 && index <= len)
                }
                _ => false,
            };
            if !valid || count > len {
                return Err(Error::runtime(
                    "view tree: children must be a dense array without extra keys",
                ));
            }
        }
        if count != len {
            return Err(Error::runtime(
                "view tree: children must be a dense array without holes",
            ));
        }
        let mut nodes = Vec::with_capacity(len);
        for index in 1..=len {
            nodes.push(self.node(table.raw_get(index)?, depth)?);
        }
        self.active.remove(&pointer);
        Ok(nodes)
    }
}

fn check_fields(table: &Table, allowed: &[&str]) -> Result<()> {
    let mut count = 0;
    for pair in table.pairs::<Value, Value>() {
        count += 1;
        let (key, _) = pair?;
        let valid = match key {
            Value::String(key) => allowed
                .iter()
                .any(|name| key.as_bytes().as_ref() == name.as_bytes()),
            _ => false,
        };
        if !valid || count > allowed.len() {
            return Err(Error::runtime("view tree: unexpected node field"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn program(init: &str, update: &str, view: &str) -> String {
        format!(
            "return {{ init = function(previous) {init} end, update = function(state, msg) {update} end, view = function(state) {view} end }}"
        )
    }

    fn load_error(source: &str) -> String {
        match Script::load("test", source, None) {
            Ok(_) => panic!("expected script rejection"),
            Err(error) => error.to_string(),
        }
    }

    #[test]
    fn round_trip_and_view_mutation_isolation() {
        let source = program(
            "return previous or {count = 1}",
            "state.count += 1; return state",
            "local count = state.count; state.count = 999; return ui.column({ui.text(tostring(count)), ui.row({ui.button('Add', 'add')})})",
        );
        let (script, initial, tree) = Script::load("counter", &source, None).unwrap();
        assert!(matches!(tree, Node::Column(_)));
        let (updated, _) = script.update(&initial, "add").unwrap();
        let (_, _, tree) = Script::load("reloaded", &source, Some(&updated)).unwrap();
        assert!(matches!(tree, Node::Column(children) if children[0] == Node::Text("2".into())));
        let (_, _, tree) = Script::load("original", &source, Some(&initial)).unwrap();
        assert!(matches!(tree, Node::Column(children) if children[0] == Node::Text("1".into())));
    }

    #[test]
    fn budgets_cover_every_execution_stage() {
        assert!(load_error("while true do end").contains("top level"));
        for (init, view, stage) in [
            ("while true do end", "return ui.text('ok')", "init"),
            ("return nil", "while true do end", "view"),
        ] {
            let error = load_error(&program(init, "return state", view));
            assert!(
                error.contains(stage) && (error.contains("budget") || error.contains("deadline")),
                "{error}"
            );
        }
        let source = program("return {}", "while true do end", "return ui.text('ok')");
        let (script, state, _) = Script::load("test", &source, None).unwrap();
        let error = script.update(&state, "go").unwrap_err().to_string();
        assert!(
            error.contains("update") && (error.contains("budget") || error.contains("deadline")),
            "{error}"
        );
    }

    #[test]
    fn protected_calls_cannot_swallow_execution_budget() {
        for source in [
            "while true do pcall(function() while true do end end) end",
            "pcall(function() while true do end end); return {init=function() end, update=function() end, view=function() return ui.text('ok') end}",
        ] {
            let error = load_error(source);
            assert!(
                error.contains("budget") || error.contains("deadline"),
                "{error}"
            );
        }
    }

    #[test]
    fn rejects_malformed_and_cyclic_trees() {
        for view in [
            "return nil",
            "return {kind='unknown'}",
            "return {kind='text', text=1}",
            "return {kind='text', text='ok', extra=true}",
            "return setmetatable({kind='text', text='ok'}, {})",
            "return ui.row({[2]=ui.text('gap')})",
            "return ui.row({ui.text('ok'), extra=ui.text('bad')})",
            "local n = ui.row({}); n.children[1] = n; return n",
            "local n = ui.row({}); n.children = n; return n",
            "return ui.row(setmetatable({}, {}))",
            "return ui.button('ok', 2)",
            "return ui.text(42)",
        ] {
            assert!(
                !load_error(&program("return nil", "return state", view)).is_empty(),
                "{view}"
            );
        }
    }

    #[test]
    fn rejects_invalid_state() {
        for init in [
            "return function() end",
            "return 0/0",
            "return math.huge",
            "return {[true] = 1}",
            "return {[1.5] = 1}",
            "return setmetatable({}, {})",
            "local t = {}; t.self = t; return t",
            "return string.char(255)",
        ] {
            assert!(
                load_error(&program(init, "return state", "return ui.text('ok')"))
                    .contains("state"),
                "{init}"
            );
        }
    }

    #[test]
    fn enforces_snapshot_and_tree_size_limits() {
        for (init, expected) in [
            (
                "local t = {}; for i=1,4097 do t[i]=i end; return t",
                "entries",
            ),
            ("return string.rep('x', 65537)", "strings"),
            ("return {[string.rep('x', 65537)]=1}", "strings"),
            ("local t = {}; for i=1,34 do t={t} end; return t", "depth"),
        ] {
            let error = load_error(&program(init, "return state", "return ui.text('ok')"));
            assert!(error.contains(expected), "{error}");
        }
        for (view, expected) in [
            (
                "local t={}; for i=1,512 do t[i]=ui.text('') end; return ui.row(t)",
                "nodes",
            ),
            ("return ui.text(string.rep('x', 65537))", "strings"),
            (
                "local t=ui.text(''); for i=1,34 do t=ui.row({t}) end; return t",
                "depth",
            ),
        ] {
            let error = load_error(&program("return nil", "return state", view));
            assert!(error.contains(expected), "{error}");
        }
    }

    #[test]
    fn accepts_bounded_plain_data_and_shared_acyclic_tables() {
        let source = program(
            "local shared = {true, false, 1.25, 'hello'}; return previous or {[-4]=shared, nested=shared, big=9007199254740991}",
            "assert(state[-4][1] == true and state.nested[2] == false); assert(state.nested[3] == 1.25 and state.big == 9007199254740991); state[-4][4] = msg; return state",
            "return ui.text(state[-4][4])",
        );
        let (script, state, _) = Script::load("test", &source, None).unwrap();
        let (state, tree) = script.update(&state, "updated").unwrap();
        assert_eq!(tree, Node::Text("updated".into()));
        let (_, _, tree) = Script::load("reload", &source, Some(&state)).unwrap();
        assert_eq!(tree, Node::Text("updated".into()));
        for init in ["return nil", "return true", "return 1.25", "return 'hello'"] {
            assert!(
                Script::load(
                    "primitive",
                    &program(init, "return state", "return ui.text('ok')"),
                    None
                )
                .is_ok()
            );
        }
    }

    #[test]
    fn execution_budget_resets_after_failure() {
        let source = program(
            "return 1",
            "if msg == 'loop' then while true do end end; return state + 1",
            "return ui.text(tostring(state))",
        );
        let (script, state, _) = Script::load("test", &source, None).unwrap();
        assert!(script.update(&state, "loop").is_err());
        let (_, tree) = script.update(&state, "ok").unwrap();
        assert_eq!(tree, Node::Text("2".into()));
    }

    #[test]
    fn failed_update_or_view_does_not_change_snapshot() {
        let source = program(
            "return {value='before'}",
            "state.value='after'; if msg=='error' then error('fail') end; return state",
            "if state.value=='after' then return {} end; return ui.text(state.value)",
        );
        let (script, state, _) = Script::load("test", &source, None).unwrap();
        assert!(script.update(&state, "error").is_err());
        assert!(script.update(&state, "invalid view").is_err());
        assert_eq!(script.render(&state).unwrap(), Node::Text("before".into()));
    }

    #[test]
    fn limits_memory() {
        let source = program(
            "return string.rep('x', 32 * 1024 * 1024)",
            "return state",
            "return ui.text('ok')",
        );
        let error = load_error(&source);
        assert!(error.contains("memory"), "{error}");
    }

    #[test]
    fn sandbox_hides_capabilities_and_freezes_libraries() {
        let source = program(
            "for _, key in ipairs({'require','load','loadstring','loadfile','dofile','getfenv','setfenv','collectgarbage','newproxy','print','warn','debug','io','os','package','coroutine'}) do assert(_G[key] == nil, key) end; return nil",
            "return state",
            "return ui.text('safe')",
        );
        assert!(Script::load("sandbox", &source, None).is_ok());
        for source in ["math.abs = nil", "ui.text = nil", "_G.injected = true"] {
            assert!(load_error(source).contains("readonly"));
        }
        let bytecode = mlua::Compiler::new().compile("return {}").unwrap();
        let lua = new_vm().unwrap();
        assert!(lua.load(bytecode).set_mode(ChunkMode::Text).exec().is_err());
    }
}
