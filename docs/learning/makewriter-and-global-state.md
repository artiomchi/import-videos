# A custom `tracing` writer, and `OnceLock` for shared global state

**Concept.** `tracing-subscriber`'s `fmt` layer doesn't write to a
fixed stream — it takes anything implementing `MakeWriter<'a>`, a
factory trait with one method, `make_writer(&'a self) -> Self::Writer`,
called once per log event to produce a `std::io::Write` target. This is
the same shape as `IHttpMessageHandlerFactory` in .NET: the subscriber
doesn't hold a writer, it holds a *factory* for one, so it can hand out
a fresh (or differently-configured) sink per call without the caller
managing lifetimes. Implementing it is small — return something that
implements `Write`, typically `Self` by value if it's stateless.

Coordinating that writer with a `ProgressBar` needs one piece of shared
state: a place to register active bars so a log line can suspend them
before printing. A `static` can't hold a `MultiProgress` directly (no
`const` constructor), so the crate uses `std::sync::OnceLock` — C#'s
nearest equivalent is `Lazy<T>` (or `LazyInitializer` pre-.NET 9):
`OnceLock::get_or_init` runs its closure at most once, thread-safely,
and every caller after the first gets the already-built value. The
difference from `Lazy<T>` is API shape only; the guarantee (single
initialization, safe concurrent access) is the same.

**Where it lives here.** `src/progress.rs`'s `REGISTRY: OnceLock<MultiProgress>`
is populated the first time any visible `Progress` bar is constructed
(`REGISTRY.get_or_init(MultiProgress::new).add(bar)`); before that,
`REGISTRY.get()` returns `None` and `suspend()` degrades to a plain
function call, so a run with no progress bar pays nothing. `src/cli.rs`'s
`DiagnosticWriter` implements `std::io::Write` by routing every
`write()` through `progress::suspend`, then implements `MakeWriter` by
returning itself (it's a stateless unit struct, so `make_writer` just
copies it). Wiring it in is a single `.with_writer(DiagnosticWriter)` on
the `fmt` builder in `init_tracing`.

**Takeaway.** Reach for a factory trait like `MakeWriter` when the
"where output goes" decision needs to be made per-call rather than
once at startup — it composes with the tracing subscriber's per-event
model instead of fighting it. Reach for `OnceLock` (not a
`lazy_static`-style crate, which predates it) for a single
process-global value with no safe way to construct it at compile time;
keep the static itself private and expose only narrow accessor
functions (here, `suspend`), so the rest of the crate never has direct
`MultiProgress` access to leak into unrelated code paths.

See design D8 in `openspec/changes/improve-console-output/design.md`.
