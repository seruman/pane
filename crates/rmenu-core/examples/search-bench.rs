//! Run with `cargo run --release -p rmenu-core --example search-bench -- [rows] [runs] [calls]`.

use std::hint::black_box;
use std::time::{Duration, Instant};

use rmenu_core::item::MenuItem;
use rmenu_core::search::SearchIndex;

fn main() {
    let args: Vec<usize> = std::env::args().skip(1).map(|a| a.parse().expect("expected a positive integer")).collect();
    assert!(args.len() <= 3 && args.iter().all(|&n| n > 0), "usage: search-bench [rows] [runs] [calls]");
    let counts = args.first().map_or_else(|| vec![10, 100_000], |&count| vec![count]);
    let runs = args.get(1).copied().unwrap_or(5);
    println!("available_parallelism={}", std::thread::available_parallelism().map_or(1, |n| n.get()));
    println!("Times include scheduling, matching, ranking, and result allocation. Index construction is separate.");
    for count in counts {
        let calls = args.get(2).copied().unwrap_or(if count <= 10 { 100 } else { 1 });
        bench(count, runs, calls);
    }
}

fn bench(count: usize, runs: usize, calls: usize) {
    let modules = ["MenuCore", "Networking", "Storage", "Authentication", "Workspace", "Rendering", "Utilities"];
    let names = ["SearchIndex", "RecordDecoder", "HTTPClient", "UserProfile", "CacheStore", "Settings", "MainWindow"];
    let roots = ["Sources", "Tests", "Examples", "Packages"];
    let items: Vec<MenuItem> = (0..count)
        .map(|i| {
            let path = format!("workspace/project-{}/{}/{}/{}-{i}.swift", i % 97, roots[i % roots.len()],
                               modules[i % modules.len()], names[(i / 7) % names.len()]);
            MenuItem::new(i.to_string(), path)
        })
        .collect();
    let start = Instant::now();
    let index = SearchIndex::new(&items);
    println!("{count} paths, index {:.3} ms, {runs} runs, {calls} calls per run", ms(start.elapsed()));
    let queries = ["", "swift", "srchidx", "mcore srch", "tests cache", "project-42 settings", "不存在", "zzzz-no-match"];
    for sorted in [true, false] {
        for query in queries {
            let expected = index.search(query, sorted, &|| false).unwrap();
            if query.is_empty() || query == "swift" { assert_eq!(expected.len(), count); }
            if query == "不存在" || query == "zzzz-no-match" { assert!(expected.is_empty()); }
            let mut times = Vec::new();
            for _ in 0..runs {
                let mut elapsed = Duration::ZERO;
                for _ in 0..calls {
                    let start = Instant::now();
                    let results = black_box(index.search(black_box(query), sorted, &|| false).unwrap());
                    elapsed += start.elapsed();
                    assert_eq!(results, expected);
                }
                times.push(elapsed.as_secs_f64() * 1e3 / calls as f64);
            }
            times.sort_by(f64::total_cmp);
            println!("{} {query:?}: min {:.3}, median {:.3}, max {:.3} ms, {} matches, {} verified calls, 0 errors",
                     if sorted { "ranked  " } else { "unsorted" }, times[0], times[runs / 2], times[runs - 1], expected.len(), runs * calls);
        }
    }
}

fn ms(d: Duration) -> f64 { d.as_secs_f64() * 1e3 }
