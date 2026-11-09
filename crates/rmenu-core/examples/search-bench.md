# Search scheduling comparison

The decision was whether direct cancellation checks and inline single-chunk matching remove the small-list scheduling floor without a measurable regression at one million rows.

The ten-row `swift` search is faster in this library benchmark. Its median fell from 0.317 ms to 0.003 ms. The one-million-row `swift` results show no measurable difference because the before and after ranges overlap. This is not a key-to-pixels measurement. No UI latency claim follows from these results.

## Method

Both binaries used release builds of the same benchmark and dependencies. The baseline is commit `6cdc674`. The candidate removes the supervisor sleep and matches a single chunk on the calling thread. An intervening change only centralizes the existing pattern configuration.

The machine reported 18 logical CPUs. Load averages were 5.56, 7.71, and 6.06 before the comparison and 6.41, 6.66, and 5.88 afterward. Other workspace builds were active. Five before and after pairs alternate to reduce that noise. Each small-list run makes 100 calls per query. Each million-row run makes one call per query. Both sides warm each query once before timing. The benchmark verifies every returned offset against the warm result outside the timer and checks known all-match and no-match queries. All calls completed without errors.

Build each revision with `CARGO_INCREMENTAL=0 cargo build --release -p rmenu-core --example search-bench`. Save each executable before rebuilding the other revision. For each of five pairs, run the saved before binary followed by the saved after binary with `10 1 100`, then repeat with `1000000 1 1`.

| Rows | Order | Query | Before median and range, ms | After median and range, ms |
| --- | --- | --- | --- | --- |
| 10 | Ranked | swift | 0.317, 0.315 to 0.320 | 0.003, 0.003 to 0.003 |
| 10 | Input | swift | 0.315, 0.315 to 0.322 | 0.003, 0.003 to 0.003 |
| 10 | Ranked | zzzz-no-match | 0.315, 0.312 to 0.322 | 0.001, 0.001 to 0.001 |
| 1,000,000 | Ranked | swift | 38.658, 35.882 to 43.607 | 37.821, 37.065 to 40.461 |
| 1,000,000 | Input | swift | 18.802, 18.378 to 20.129 | 18.461, 17.289 to 20.194 |
| 1,000,000 | Ranked | zzzz-no-match | 1.027, 0.960 to 1.095 | 0.770, 0.716 to 0.883 |

## Limiter and limits

Separate, unreported timing runs used macOS `sample` and `/usr/bin/time -lp`. The baseline sample spent most main-thread samples in `nanosleep` or thread join and deallocation. A 1,000-call-per-query baseline run took 4.82 seconds elapsed but only 0.10 seconds user CPU and 0.29 seconds system CPU. That supports scheduling waits as the small-list limiter, rather than matcher throughput.

The candidate sample instead reached nucleo pattern scoring and fuzzy-match matrix setup. A 10,000-call-per-query candidate run took 0.28 seconds elapsed, 0.23 seconds user CPU, and 0.04 seconds system CPU. Its limiter is CPU work in matching and allocation. Removing the approximately 0.315 ms wait explains the small-list saving without requiring impossible matching throughput. Full matching still runs inside each timed call. The million-row results retain the existing parallel path.

The experiment does not measure terminal rendering or command startup. Those remain gaps for an end-to-end latency claim. The deterministic cancellation test fails on the baseline because workers cannot consult the callback, then passes with direct worker checks.
