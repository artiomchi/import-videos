# `SystemTime` ↔ `jiff::Timestamp` and Multi-Predicate Safety Gates

## Concept: `std::time::SystemTime` to `jiff::Timestamp`

Rust's standard library uses `std::time::SystemTime` for filesystem metadata
(e.g. `fs::Metadata::modified()`), while this codebase uses `jiff::Timestamp`
as its canonical instant type. The two are separate types with no automatic
conversion — you have to bridge them manually.

`SystemTime::duration_since(UNIX_EPOCH)` gives an `Ok(Duration)` for instants
after epoch, and an `Err(SystemTimeError)` containing the negative duration for
instants before (rare for real filesystem mtimes, but correct to handle). In
both cases `jiff::Timestamp::new(secs, nanos)` accepts the raw values:

```rust
// src/transfer.rs — systemtime_to_timestamp
fn systemtime_to_timestamp(t: std::time::SystemTime) -> Timestamp {
    match t.duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => Timestamp::new(d.as_secs() as i64, d.subsec_nanos() as i32)
            .unwrap_or(Timestamp::UNIX_EPOCH),
        Err(e) => {
            let d = e.duration();
            Timestamp::new(-(d.as_secs() as i64), -(d.subsec_nanos() as i32))
                .unwrap_or(Timestamp::UNIX_EPOCH)
        }
    }
}
```

The C# equivalent would be `DateTimeOffset.FromFileTime()` or converting from
`DateTime.UnixEpoch + TimeSpan` — there's a ready-made helper. In Rust you
write the bridge yourself, which also makes the edge case (before-epoch)
explicit instead of hidden.

**Takeaway:** when you need `jiff::Timestamp` from filesystem metadata, call
`systemtime_to_timestamp`; the function is self-contained and unit-testable
without touching the filesystem.

## Concept: two predicates instead of one (safety gate split)

`src/transfer.rs` originally had a single `outcome_is_success` predicate used
for two unrelated decisions: "should we write the sidecar?" and "is this group
eligible for source deletion?". That worked while the only non-copying outcomes
were failures — anything that landed successfully at the destination was also
safe to delete from the source.

`SkippedQuickMatch` broke the coupling: a quick-matched file *is* at the
destination (sidecar should be written) but its content was *not* verified
(deletion is not safe). The right fix is to split into two predicates, each
encoding exactly one invariant:

```rust
// in-place: used for sidecar-writing gate
fn in_place_at_destination(o: &TransferOutcome) -> bool { ... }

// content-verified: used for source-deletion gate (ADR 0003 invariant)
fn content_verified(o: &TransferOutcome) -> bool { ... }
```

In C# this would typically be a property or method on an enum-like class, and
the coupling would be less visible. In Rust, the pattern of separate
`match`-based predicate functions keeps each safety invariant small, named, and
testable in isolation — and the compiler will remind you to handle new variants
in both.

**Takeaway:** when a single boolean gate starts serving two concerns that can
diverge, split it. Name each predicate after the invariant it enforces, not
after its current implementation.
