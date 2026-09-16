# Ondin

Ondin is a native, open-source 2D design tool written in Rust — Figma-class scope
at a deliberately minimal v1, for Windows and Linux. Multiple artboards on an
infinite canvas, a nested node tree of shapes, paths and text, multiple
fills and strokes per layer, masks, effects, image adjustments, and SVG/PNG
export. Its document model is API-first: the same operation path that serves the
UI serves AI agents over MCP. The project is foundations-first — identity, the
mutation path, the renderer boundary and serialization matter more than feature
count, because features are cheap to add and seams are expensive to change.

Still early — `v0.1.0` is the first tag, and the v1 feature list is not complete.

## Building

```sh
cargo build -p ondin-app     # target/debug/ondin.exe
cargo test --workspace
```

Rendering can be checked without opening a window — `ondin export` runs the whole
document → pixels/markup path headlessly:

```sh
cargo run -p ondin-app -- export path/to/doc.ondin --svg -o out.svg
```

## Documentation

- [`docs/architecture.md`](docs/architecture.md) — the design and its invariants.
- [`docs/decisions.md`](docs/decisions.md) — every deviation from that design, with a verdict.
- [`docs/roadmap.md`](docs/roadmap.md) — open work, decided non-goals, post-v1.

## License

MIT — see [`LICENSE`](LICENSE). Bundled fonts and other third-party material keep
their own licenses; see [`THIRD-PARTY-NOTICES.md`](THIRD-PARTY-NOTICES.md).
