use super::*;
use crate::{host, open, poll, view};

struct Harness {
    _dir: tempfile::TempDir,
    rt: Runtime<State>,
    bridge: Arc<Bridge>,
    jobs: mpsc::Receiver<Job>,
    clock: f64,
}

impl Harness {
    fn new(terminal: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("init.luau"),
            include_str!("../../tests/fixtures/counter.luau"),
        )
        .unwrap();
        let mut rt = Runtime::new(
            open(dir.path(), host::Capabilities::default()),
            view,
            (640.0, 480.0),
            rune::Env::default(),
        );
        rt.poll = Some(poll);
        rt.set_tab_visits_controls(true);
        let (sender, jobs) = mpsc::channel::<Job>();
        let bridge = Arc::new(Bridge::new(
            Arc::new(move |job| {
                sender.send(job).unwrap();
            }),
            terminal,
        ));
        rt.update(|_, tx| tx.record_rewind(Some(&bridge.recorder())));
        rt.frame(0.0);
        Self {
            _dir: dir,
            rt,
            bridge,
            jobs,
            clock: 0.0,
        }
    }

    fn turn(&mut self) {
        while let Ok(job) = self.jobs.try_recv() {
            self.rt.update_if(job);
        }
        self.clock += 0.01;
        self.rt.frame(self.clock);
        if !self.bridge.terminal {
            self.bridge.drawn(&self.rt);
        }
    }

    fn request(&mut self, request: Value) -> Reply {
        let bridge = self.bridge.clone();
        let worker = std::thread::spawn(move || bridge.request(request));
        while !worker.is_finished() {
            self.turn();
            std::thread::sleep(Duration::from_millis(2));
        }
        self.turn();
        worker.join().unwrap()
    }

    fn shows(&self, text: &str) -> bool {
        self.rt
            .elements()
            .iter()
            .any(|e| e.text.as_deref().is_some_and(|t| t.contains(text)))
    }
}

#[test]
fn terminal_geometry_and_activation_use_the_same_running_script() {
    let mut h = Harness::new(true);
    let elements = h.request(json!({"op":"elements"})).unwrap();
    assert_eq!(elements["kind"], "partial-keyed-geometry");
    assert!(
        elements["elements"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["name"] == "increment"),
        "{elements}"
    );
    let info = h.request(json!({"op":"info"})).unwrap();
    assert_eq!(info["tree"], true);
    assert_eq!(info["screenshot"], true);
    assert!(h.shows("Count: 0"));
    for n in 1..=2 {
        let answer = h
            .request(json!({"op":"activate", "key":"increment"}))
            .unwrap();
        assert_eq!(answer["dispatched"], true);
        assert!(h.shows(&format!("Count: {n}")));
    }
    assert!(
        h.request(json!({"op":"activate", "key":"missing"}))
            .is_err()
    );
    assert!(h.shows("Count: 2"));
    h.request(json!({"op":"activate", "key":"reset"})).unwrap();
    assert!(h.shows("Count: 0"));
    assert!(!h.bridge.recorder.is_paused());
}

#[test]
fn native_tree_includes_text_hierarchy_and_accessibility() {
    let mut h = Harness::new(false);
    h.request(json!({"op":"activate", "key":"increment"}))
        .unwrap();
    assert!(
        h.request(json!({"op":"screenshot"}))
            .unwrap_err()
            .contains("terminal host only")
    );
    let tree = h.request(json!({"op":"tree"})).unwrap();
    assert_eq!(tree["kind"], "native-frame-tree");
    assert!(
        tree["elements"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["text"].as_str().is_some_and(|t| t.contains("Count: 1")))
    );
    assert!(!tree["accessibility"].is_null());
}

#[test]
fn picking_and_keyboard_input_reach_live_runtime() {
    let mut h = Harness::new(true);
    let (x, y) = h.rt.find("increment").unwrap();
    let picked = h.request(json!({"op":"pick", "x":x, "y":y})).unwrap();
    assert!(picked["picked"]["path"].is_string(), "{picked}");
    h.request(json!({"op":"focus", "key":"increment"})).unwrap();
    h.request(json!({"op":"key", "key":"Space"})).unwrap();
    assert!(h.shows("Count: 1"));
}

#[test]
fn expired_ui_requests_do_not_run_later_or_accumulate() {
    let mut h = Harness::new(true);
    let result = h.bridge.request(json!({"op":"key", "key":"Tab"}));
    assert!(result.unwrap_err().contains("timed out"));
    assert!(
        h.bridge
            .request(json!({"op":"key", "key":"Space"}))
            .unwrap_err()
            .contains("still pending")
    );
    h.turn();
    assert!(!h.rt.elements().iter().any(|e| e.focused));
    h.request(json!({"op":"activate", "key":"increment"}))
        .unwrap();
    assert!(h.shows("Count: 1"));
}

#[test]
fn bridge_survives_reload_and_keeps_script_state() {
    let mut h = Harness::new(true);
    h.request(json!({"op":"activate", "key":"increment"}))
        .unwrap();
    std::fs::write(
        h._dir.path().join("init.luau"),
        include_str!("../../tests/fixtures/counter.luau")
            .replace("A scriptable Pane", "Reloaded live"),
    )
    .unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    while !h.shows("Reloaded live") && Instant::now() < until {
        h.turn();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(h.shows("Reloaded live") && h.shows("Count: 1"));
    h.request(json!({"op":"activate", "key":"increment"}))
        .unwrap();
    assert!(h.shows("Count: 2"));
}
