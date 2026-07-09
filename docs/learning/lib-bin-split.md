# lib.rs + thin main.rs

**Concept.** A cargo package can build both a library target (`src/lib.rs`)
and a binary target (`src/main.rs`) at once; the binary just depends on the
library like any other crate would. There's no C# equivalent to reach for —
the closest mental model is splitting a console app's `Program.Main` into a
class library plus a tiny console project that references it, except here
it's the *same package*, so there's no extra project file, versioning, or
NuGet step. The payoff is what you'd expect from that split: the library
half is unit- and integration-testable directly (`cargo test` compiles it
standalone), without spawning a process or parsing argv.

**Where it lives here.** `src/main.rs` is two lines:

```rust
fn main() {
    std::process::exit(import_videos::run());
}
```

Everything else — CLI parsing, config loading, planning, transfer — lives
under `src/lib.rs` and its modules, all `pub` enough for `tests/*.rs`
(compiled as separate crates linking against the library) to call directly.
`tests/integration.rs` does exactly this: it builds `Profile` and
`ImportPlan` values in Rust and calls `transfer::execute()` without ever
spawning the compiled binary, which is what makes the "verify a hash
mismatch keeps the source file" class of test fast and deterministic
instead of a slower subprocess test. A few CLI-level tests (exit codes,
config error messages) *do* spawn the real binary via
`env!("CARGO_BIN_EXE_import-videos")` — that's the seam where "does the
process exit with the right code" can only be answered by actually running
the process.

**Takeaway.** If a Rust package has any logic worth unit testing, give it a
`lib.rs` and keep `main.rs` to argument parsing plus a call into the
library. The split costs nothing and buys back the ability to test without
a subprocess.

See ADR 0005.
