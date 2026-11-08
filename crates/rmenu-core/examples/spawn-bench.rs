use std::time::{Duration, Instant};
use rmenu_core::command::{Cancel, run};
fn main() {
    let argv = vec!["/usr/bin/true".to_string()];
    let mut ms: Vec<f64> = (0..200).map(|_| {
        let t = Instant::now();
        run(&argv, None, &[], Duration::from_secs(5), 1 << 20, &Cancel::default()).unwrap();
        t.elapsed().as_secs_f64() * 1000.0
    }).collect();
    ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    println!("spawn+wait /usr/bin/true, n=200: median {:.2} ms, p90 {:.2} ms", ms[100], ms[180]);
}
