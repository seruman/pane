//! Time to spawn a command and collect its exit, as a source or preview pays it.
//!
//!     cargo run --release -p rmenu-core --example spawn-bench [-- RUNS [PROGRAM ARGS...]]
//!
//! Defaults to 1000 runs of `/bin/echo hi`. Prints p50, p99, max, how many runs took over
//! 5 ms, and how many failed. Run it several times, alternating builds, before comparing.

use std::time::{Duration, Instant};

use rmenu_core::command::{Cancel, Limits, run};

fn main() {
    let mut args = std::env::args().skip(1);
    let runs: usize = args.next().map_or(1000, |n| n.parse().expect("RUNS is a number"));
    let mut argv: Vec<String> = args.collect();
    if argv.is_empty() { argv = vec!["/bin/echo".into(), "hi".into()]; }
    let mut failed = 0;
    let mut ms: Vec<f64> = (0..runs).map(|_| {
        let started = Instant::now();
        match run(&argv, None, &[], Limits { timeout: Duration::from_secs(5), output_bytes: 1 << 20 }, &Cancel::default()) {
            Ok(result) if result.status.success() => {}
            _ => failed += 1,
        }
        started.elapsed().as_secs_f64() * 1000.0
    }).collect();
    ms.sort_by(f64::total_cmp);
    let over_5ms = ms.iter().filter(|&&m| m > 5.0).count();
    println!("{argv:?}, {runs} runs: p50 {:.2} ms, p99 {:.2} ms, max {:.2} ms, {over_5ms} over 5 ms, {failed} failed",
             ms[runs / 2], ms[runs * 99 / 100], ms[runs - 1]);
}
