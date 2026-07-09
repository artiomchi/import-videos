# Trait objects vs. generics

**Concept.** Rust has two ways to write code against "something that
implements trait `T`": generics (`fn scan<S: ImportSource>(s: &S)`),
monomorphized at compile time into one specialized copy per concrete type
(zero runtime cost, like C# generics over `struct` constraints — but Rust
always monomorphizes, it never falls back to shared/boxed code the way
`List<int>` and `List<string>` can share IL under some JIT strategies), or
trait objects (`&dyn ImportSource` / `Box<dyn ImportSource>`), which store
a vtable pointer and dispatch at runtime — directly analogous to a C#
interface reference, with a similar (small, usually irrelevant) indirect-call
cost. The trade-off isn't really performance at this scale; it's whether
the set of implementing types needs to be *heterogeneous at runtime* in one
collection. Generics give you one concrete type per instantiation; trait
objects let you put a `GoproSource` and a `TeslaSource` in the same `Vec`.

**Where it lives here.** `src/config.rs`'s `SourceKind::build()` returns
`Box<dyn ImportSource>` because the whole point of a profile is to name a
device type at *runtime* (from a YAML `type:` field) and get back
*something* that knows how to `scan()` — the caller in `lib.rs` doesn't
know or care which concrete struct it got. A generic `fn run<S:
ImportSource>(source: S)` can't express that: the concrete type would have
to be known at the call site, which defeats the purpose of a config-driven
device registry. This is also why the registry is a `match` over a
compile-time-known enum rather than a `HashMap<&str, Box<dyn Fn() -> Box<dyn
ImportSource>>>` — the compiler forces every arm to be handled when a
variant is added, which a runtime map can't guarantee (see the doc comment
on `SourceKind::build` in `src/config.rs`).

**Takeaway.** Reach for `dyn Trait` when the concrete type is decided at
runtime and needs to live alongside other concrete types implementing the
same trait (a plugin/registry shape); reach for generics when there's
exactly one concrete type per call site and you want it inlined. Dispatch
cost is not the deciding factor for a tool operating at "files per SD
card" scale either way.

See ADR 0005 (design decision D3).
