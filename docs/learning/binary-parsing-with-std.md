# Binary parsing with std

**Concept.** Parsing a binary container format (MP4 boxes, in this case)
doesn't need a parser-combinator crate — `std::io::{Read, Seek}` plus
`u32::from_be_bytes`/`u64::from_be_bytes` is enough for a format that's
just a flat sequence of length-prefixed records. The pattern: read a
fixed-size header into a stack array (`let mut buf = [0u8; 8];
reader.read_exact(&mut buf)?;`), decode the fields you need out of
slices of it, and use `Seek::seek(SeekFrom::Start/Current(n))` to skip
whatever you don't care about instead of reading and discarding it.
The C# analogue is `BinaryReader`/`BinaryPrimitives.ReadUInt32BigEndian`
over a `Stream` — same idea, Rust just makes the "did I read enough
bytes" question a compile-time-checked array size instead of a runtime
length check.

**Where it lives here.** `src/media/mp4.rs`'s `find_box` reads an
8-byte box header (u32 BE size + 4-byte fourcc), optionally a 16-byte
extended header when `size == 1`, and seeks past any box whose fourcc
doesn't match what the caller is looking for — so scanning a whole
`moov` container to find `mvhd` never actually touches most of its
bytes. The two extraction functions (`read_hilights`, `read_creation_time`)
build on that to decode exactly the fields the GoPro pipeline needs. A
subtlety worth flagging for a C# background: `read_exact` fails
(rather than short-reading) if the buffer can't be fully filled, which
is exactly the "truncated file" signal the parser needs — no separate
length check required before decoding.

**Takeaway.** Reach for `std::io` directly when a format is a flat,
self-describing sequence of typed fields (length + tag + payload) and
you only need a handful of them — a full parser-combinator crate earns
its keep when the grammar has real recursion, backtracking, or many
callers who need the whole structure, not two callers who each want
one field.

See ADR 0002 (hand-rolled MP4/GPMF parsers) and [[errors-thiserror-vs-anyhow]]
(`Mp4Error` follows the same library-error-type pattern).
