# Learning notes

Short notes on Rust concepts as they show up in this codebase, written for a developer coming from .NET. A note is added when a change introduces a concept new to the project — not speculatively.

**Format** — each note answers three things:

1. **Concept** — what it is, in a sentence or two, with the C# analogue (or why there isn't one).
2. **Where it lives here** — the concrete file/function in this repo that uses it, and why it was the right tool there.
3. **Takeaway** — the rule of thumb to carry forward.

Keep notes under a page. Link related ADRs.

## Index

- [thiserror vs. anyhow](errors-thiserror-vs-anyhow.md) — library error types vs. binary-boundary error handling
- [lib.rs + thin main.rs](lib-bin-split.md) — why the logic lives in a testable library, not the binary
- [Trait objects vs. generics](trait-objects-vs-generics.md) — why `ImportSource` dispatch uses `Box<dyn Trait>`
- [Binary parsing with std](binary-parsing-with-std.md) — reading MP4 boxes with `Read`/`Seek`, no parser crate
- [Iterators and lifetimes in stream parsing](iterators-lifetimes-in-stream-parsing.md) — `KlvIter<'a>`'s borrowed items vs. `mp4.rs`'s owned, `Read`-based walker
- [Making illegal states unrepresentable with enums](illegal-states-with-enums.md) — Tesla's `Reasons` allow-xor-deny enum vs. two `Option` fields, and its `#[serde(flatten)]` round-trip limit
- [Serde view-models vs. deriving on domain types](serde-view-models-vs-domain-types.md) — `report.rs`'s `PlanJson`/`ResultsJson`, and why the JSON contract isn't derived onto `ImportPlan`/`ExecuteReport` directly
- [A custom `tracing` writer, and `OnceLock` for shared global state](makewriter-and-global-state.md) — `cli.rs`'s `MakeWriter` impl and `progress.rs`'s `OnceLock<MultiProgress>` registry
