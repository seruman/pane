use super::*;
use std::time::Duration;

fn pending(capture: &Capture) -> (CaptureId, mpsc::Receiver<Reply>) {
    let (sender, receiver) = mpsc::sync_channel(1);
    let id = capture
        .begin(
            Kind::Screenshot,
            Instant::now() + Duration::from_secs(2),
            sender,
        )
        .unwrap();
    (id, receiver)
}

#[test]
fn superseded_completion_never_answers_a_new_request() {
    let capture = Capture::new();
    let (old, old_reply) = pending(&capture);
    let (new, new_reply) = pending(&capture);
    assert!(old != new);
    assert!(matches!(
        old_reply.try_recv(),
        Err(mpsc::TryRecvError::Disconnected)
    ));
    capture.deliver(old, |_| {
        panic!("old request must not copy or encode pixels")
    });
    assert!(matches!(
        new_reply.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
    capture.deliver(new, |_| Err("new request's error".into()));
    assert!(matches!(new_reply.recv().unwrap(), Err(error) if error == "new request's error"));
}

#[test]
fn completion_happens_at_most_once() {
    let capture = Capture::new();
    let (id, reply) = pending(&capture);
    capture.deliver(id, |_| Ok(Content::Tree(json!({"test":true}))));
    capture.deliver(id, |_| panic!("duplicate completion"));
    assert!(matches!(reply.recv().unwrap(), Ok(Content::Tree(value)) if value["test"] == true));
    assert!(capture.pending.lock().unwrap().is_none());
}

#[test]
fn cancelling_an_old_request_preserves_the_new_one() {
    let capture = Capture::new();
    let (old, _) = pending(&capture);
    let (new, reply) = pending(&capture);
    capture.cancel(old);
    capture.deliver(new, |_| Err("still active".into()));
    assert!(matches!(reply.recv().unwrap(), Err(error) if error == "still active"));
}

#[test]
fn cancelled_and_expired_callbacks_do_no_work() {
    let capture = Capture::new();
    let (id, reply) = pending(&capture);
    capture.cancel(id);
    capture.deliver(id, |_| panic!("cancelled capture copied pixels"));
    assert!(matches!(
        reply.try_recv(),
        Err(mpsc::TryRecvError::Disconnected)
    ));
    let (sender, receiver) = mpsc::sync_channel(1);
    let expired = capture
        .begin(Kind::Tree, Instant::now() - Duration::from_secs(1), sender)
        .unwrap();
    capture.deliver(expired, |_| panic!("expired capture serialized a tree"));
    assert!(matches!(
        receiver.try_recv(),
        Err(mpsc::TryRecvError::Disconnected)
    ));
}

#[test]
fn timeout_removes_pending_request() {
    let capture = Capture::new();
    let error = capture
        .request(Kind::Screenshot, Instant::now() + Duration::from_millis(1))
        .unwrap_err();
    assert!(error.contains("timed out"));
    assert!(capture.pending.lock().unwrap().is_none());
}

#[test]
fn completion_copies_pixels_but_leaves_encoding_to_worker() {
    let capture = Capture::new();
    let (id, reply) = pending(&capture);
    capture.deliver(id, |_| {
        Ok(Content::Image {
            pixels: Pixels::copy(1, 1, 4, &[1, 2, 3, 255]).unwrap(),
            metadata: json!({"stale":true, "frame":17}),
        })
    });
    let Ok(Content::Image { pixels, metadata }) = reply.recv().unwrap() else {
        panic!("missing raw pixels")
    };
    assert_eq!(metadata["frame"], 17);
    assert_eq!(metadata["stale"], true);
    assert_eq!(pixels.encode().unwrap()["format"], "png");
}

fn runtime() -> Runtime<crate::State> {
    let mut rt = Runtime::new(
        crate::empty(),
        crate::view,
        (16.0, 16.0),
        rune::Env::default(),
    );
    rt.frame(0.0);
    rt
}

fn frame(stale: bool) -> CapturedFrame<'static> {
    CapturedFrame {
        rgba: &[7, 8, 9, 255],
        width: 1,
        height: 1,
        stride: 4,
        scale: 2.0,
        frame: 12,
        clock: 1.5,
        stale,
    }
}

#[test]
fn real_callback_carries_metadata_and_does_not_hide_staleness() {
    let capture = Capture::new();
    let rt = runtime();
    let (id, reply) = pending(&capture);
    capture.complete(&rt, id, Ok(frame(true)));
    let Ok(Content::Image { pixels, metadata }) = reply.recv().unwrap() else {
        panic!("no image")
    };
    assert_eq!(metadata["frame"], 12);
    assert_eq!(metadata["clock"], 1.5);
    assert_eq!(metadata["scale"], 2.0);
    assert_eq!(metadata["stale"], true);
    assert_eq!(metadata["presentationConfirmed"], false);
    assert_eq!(pixels.encode().unwrap()["width"], 1);
}

#[test]
fn terminal_tree_reports_if_it_matches_the_retained_image() {
    let capture = Capture::new();
    let rt = runtime();
    for stale in [true, false] {
        let (sender, receiver) = mpsc::sync_channel(1);
        let id = capture
            .begin(Kind::Tree, Instant::now() + Duration::from_secs(2), sender)
            .unwrap();
        capture.complete(&rt, id, Ok(frame(stale)));
        let Ok(Content::Tree(value)) = receiver.recv().unwrap() else {
            panic!("no tree")
        };
        assert_eq!(value["kind"], "terminal-runtime-tree");
        assert_eq!(value["matchesCapturedPixels"], !stale);
        assert_eq!(value["capture"]["stale"], stale);
        assert!(value["elements"].is_array());
        assert!(!value["accessibility"].is_null());
    }
}

#[test]
fn host_failures_complete_requests_without_empty_images() {
    let capture = Capture::new();
    let rt = runtime();
    for error in [
        CaptureError::NoFrame,
        CaptureError::MapTimeout,
        CaptureError::Gpu("test failure".into()),
        CaptureError::ShutDown,
    ] {
        let (id, reply) = pending(&capture);
        capture.complete(&rt, id, Err(error));
        assert!(
            matches!(reply.recv().unwrap(), Err(error) if error.starts_with("terminal capture failed:"))
        );
        assert!(capture.pending.lock().unwrap().is_none());
    }
}
