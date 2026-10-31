# Performance baselines

`crates/ondin-render/tests/perf_suite.rs` generates a fixed set of documents and times what the
editor does with them. `baselines/<machine>.json` holds the last recorded figures for each machine;
git history is the record over time. Figures compare only on the same machine, and only between
runs of the same fixtures version.

## Running

```bash
# Measure and compare with this machine's baseline (about a minute):
cargo test -p ondin-render --release --test perf_suite -- --ignored --nocapture measure

# The same, and write the results into baselines/<machine>.json:
ONDIN_PERF_RECORD=1 cargo test -p ondin-render --release --test perf_suite -- --ignored --nocapture measure

# Write the fixtures to target/perf/*.ondin, to open them in the app:
cargo test -p ondin-render --release --test perf_suite -- --ignored --nocapture write_fixtures
```

Run with nothing else busy: the GPU figures move with load and clock state. Recording refuses a
debug build.

| Variable | Does |
| --- | --- |
| `ONDIN_PERF_RECORD=1` | Write the results into the baseline, replacing those fixtures' entries. |
| `ONDIN_PERF_ONLY=control,wide` | Run only these fixtures. |
| `ONDIN_PERF_EXTRA=a.ondin;b.ondin` | Also time local documents, recorded as `local:<stem>`. Use the platform's path separator. For files that cannot be committed, such as imported kits. |
| `ONDIN_PERF_MACHINE=name` | The baseline's name, in place of the host name. |

## Fixtures

Generated from a fixed seed (`tests/perf_suite/fixtures.rs`). Each run of one version builds the
same document. Bump `VERSION` when a generator changes what it builds: an older baseline is then
shown but not compared.

| Fixture | Is |
| --- | --- |
| `control` | 3 pages × 15 screens, 40–120 layers each, mobile and desktop: the everyday document. |
| `wide` | 1 page, 1,000 screens of ~150 layers (~149k layers): the ceiling. |
| `components` | 60 screens built from instances nested five deep (panel › section › row › button › icon), some overridden, with their mains on a second page. |

## Metrics

All times are in ms, p50 and p10 over up to 30 runs, after one warm-up run that is not counted.

| Metric | Times |
| --- | --- |
| `save`, `load` | `io::save` and `io::load` of the whole document. |
| `resolve` | `Resolved::rebuild`. |
| `scene_fit`, `scene_work` | The CPU scene walk of the first page: whole at 2560×1440 (`fit`), and at 100% on its first frame (`work`). |
| `frame_fit`, `frame_work` | The same views drawn on the GPU. |
| `pan_fit` | A pan at `fit`: the exposed strip drawn, the rest shifted. |
| `commit`, `commit_undo` | Recolouring a rect as the app commits it (component passes, history, resolve update), then its undo. |
| `commit_main`, `commit_main_undo` | The same on the icon main's dot, which reaches every copy (`components` only). |
