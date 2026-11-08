//! Search time over MMenuBench's synthetic paths: `cargo run --release -p rmenu-core --example search-bench -- 1000000`.

use std::time::{Duration, Instant};

use rmenu_core::item::MenuItem;
use rmenu_core::search::SearchIndex;

fn main() {
    let count: usize = std::env::args().nth(1).map_or(100_000, |a| a.parse().expect("positive row count"));
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
    println!("{count} paths; index {:.1} ms", ms(start.elapsed()));
    let queries = ["", "swift", "srchidx", "mcore srch", "tests cache", "project-42 settings", "不存在", "zzzz-no-match"];
    for sorted in [true, false] {
        for query in queries {
            index.search(query, sorted, &|| false).unwrap();
            let mut times = Vec::new();
            let mut matches = 0;
            for _ in 0..5 {
                let start = Instant::now();
                matches = index.search(query, sorted, &|| false).unwrap().len();
                times.push(start.elapsed());
            }
            times.sort();
            println!("{} {query:?}: {:.1} ms; {matches} matches", if sorted { "ranked  " } else { "unsorted" }, ms(times[2]));
        }
    }
}

fn ms(d: Duration) -> f64 { d.as_secs_f64() * 1e3 }
