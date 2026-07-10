# Iterators and lifetimes in stream parsing

**Concept.** A Rust `Iterator` can borrow from the data it's iterating
instead of owning it: `KlvIter<'a>` holds a `&'a [u8]` and yields
`Klv<'a>` items whose `value: &'a [u8]` field points straight back into
that same slice — no copying, no allocation, and the borrow checker
guarantees the items can't outlive the bytes they point into. The
lifetime `'a` here is the whole story: it's the same lifetime on the
struct, the impl, and every borrowed field, so the compiler can verify
"this `Klv` is only valid as long as the payload it came from" without
any runtime bookkeeping. The closest C# analogue is `IEnumerable<T>`
over a `ReadOnlySpan<byte>` (or `Memory<byte>`) — `Span<T>` is the one
C# type that also carries a compile-time-checked lifetime (the
`ref struct` rule that stack-only-ness enforces), but ordinary
`IEnumerable<T>` has no such guarantee: nothing stops a consumer from
stashing a slice reference past the buffer's lifetime except discipline
and, at best, a `Memory<T>` wrapper.

**Where it lives here.** `src/media/gpmf.rs`'s `KlvIter<'a>` walks a
GPMF payload's flat key-length-value records, and `Klv<'a>::children()`
returns a *new* `KlvIter<'a>` over the current item's own value slice —
recursion into nested containers (GPMF type `0x00`) without ever
allocating a tree. Contrast this with `src/media/mp4.rs`'s box walker
from an earlier changeset ([[binary-parsing-with-std]]): that one reads
through a `Read + Seek` stream and returns owned byte arrays, because
MP4 boxes are scattered across a file too large to hold in memory at
once. GPMF payloads are the opposite case — a few KB, read whole, and
parsed immediately — so borrowing beats streaming: `KlvIter` is `Copy`
(it's just a slice and a cursor), cloning it to "look ahead" costs
nothing, and every accessor (`as_i32s`, `as_utc`, ...) decodes directly
from the borrowed bytes with no intermediate owned buffer.

**Takeaway.** Reach for a borrowing iterator (`&'a [u8]` in, `&'a [u8]`
out) when the whole payload already fits in memory and outlives every
item you'll yield from it — the lifetime parameter is free correctness,
not ceremony. Switch to an owned-`Read`-based walker (like `mp4.rs`'s)
the moment the source is bigger than you want resident at once, or the
items need to outlive the buffer they came from.

See ADR 0002 (hand-rolled MP4/GPMF parsers) and
[[binary-parsing-with-std]] (the `Read`/`Seek` counterpart to this
borrowing style).
