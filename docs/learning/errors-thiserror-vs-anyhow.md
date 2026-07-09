# thiserror vs. anyhow

**Concept.** Rust has no exception hierarchy — a fallible function's error
type is part of its signature (`Result<T, E>`), so callers know exactly
what can go wrong without reading the implementation. That cuts both ways:
a library has to *pick* an `E`. `thiserror` is a derive macro that turns an
enum into a proper `std::error::Error` with minimal boilerplate (it writes
the `Display` and `source()` impls for you); `anyhow::Error` is a single
type-erased box that can hold *any* error, with no fixed shape. The C#
analogue for `thiserror` is a custom exception type; there isn't a good
analogue for `anyhow` — closest is "just catch `Exception`," except
`anyhow::Error` still preserves the chain (`.source()`/`context()`) instead
of discarding it.

**Where it lives here.** `src/error.rs` defines `Error` as a `thiserror`
enum with variants that carry the path or paths involved (`Io { path,
source }`, `VerifyMismatch { src, dest }`, ...), because every caller in
this crate — and every message printed to the user — needs to know *which
file* failed, not just that "an IO error happened." `lib.rs::run()` is the
only place that doesn't care about the variant: it just prints `Display`
and maps to an exit code via `Error::exit_code()`. If `main.rs` ever grew
its own fallible setup steps unrelated to the library (reading an env var,
say), `anyhow` would be the right tool there — but this crate's `main.rs`
is a one-liner, so that need never arose (see [[lib-bin-split]]).

**Takeaway.** `thiserror` at a library boundary, where callers might match
on the error; `anyhow` at a binary's outermost layer, where nobody matches
on anything and you just want good messages with minimal ceremony. Don't
reach for `anyhow` inside library code — it throws away the caller's
ability to distinguish "config problem" from "disk full."

See ADR 0005 (single-crate split) and [[lib-bin-split]].
