# jiff: Rendering Zoned Timestamps Without the `[IANA/Name]` Suffix

## The problem

`jiff`'s `Zoned` type implements `Display` as:

```
2026-07-09T23:19:48+03:00[Europe/Vilnius]
```

The `[Europe/Vilnius]` suffix is part of the [RFC 9557 / Temporal proposal]
annotation syntax — useful for round-tripping through jiff's own parser, but
**not standard ISO-8601 or JSON**. If you write it into a sidecar JSON file,
any consumer that isn't jiff-aware will either reject it or silently include
the brackets in the string value.

## The solution — `strftime` with `%:z`

Use `jiff::fmt::strtime::format` with a format string that uses `%:z` for the
numeric offset:

```rust
const SIDECAR_TIMESTAMP_FORMAT: &str = "%Y-%m-%dT%H:%M:%S%:z";

fn format_ts(ts: Timestamp, tz: &TimeZone) -> String {
    let zoned = ts.to_zoned(tz.clone());
    jiff::fmt::strtime::format(SIDECAR_TIMESTAMP_FORMAT, &zoned)
        .expect("constant format string is always valid")
}
```

Output: `2026-07-09T23:19:48+03:00` — clean ISO-8601 with a numeric offset,
no zone annotation.

## `Timestamp::to_zoned` vs `civil::DateTime::to_zoned`

These two methods have different return types — a common source of confusion:

| Method | Input | Returns |
|--------|-------|---------|
| `Timestamp::to_zoned(tz)` | An unambiguous instant | `Zoned` (infallible — there is exactly one zoned moment for any instant+zone pair) |
| `civil::DateTime::to_zoned(tz)` | A wall-clock reading with no zone | `Result<Zoned>` (fallible — ambiguous at DST transitions; jiff returns an error for gaps/folds unless you choose a disambiguation strategy) |

The distinction matters when converting camera-clock civil times (which have
no inherent timezone) vs GPS-corrected instants (which are unambiguous UTC).

## Where this appears in the codebase

- `src/source/sidecar.rs` — `format_ts` is the canonical helper; all device
  modules call `sidecar::build()` and never format timestamps themselves.
- `src/config/layout.rs` — `{date:FORMAT}` layout tokens go through
  `Timestamp::to_zoned(tz.clone())` then `strftime`, same pattern.
- `src/report.rs` — verbose plan output uses the same strftime approach.
