# Copy-on-write reflinks

**Concept.** Some filesystems (btrfs, XFS with reflink support, bcachefs)
can make two files share the same on-disk data blocks ("extents") instead
of duplicating them. `FICLONE` is the Linux ioctl that asks the kernel to
do this: point a new file at an existing file's extents, mark them
copy-on-write, and return — no bytes are read or written. If either file
is later modified, the kernel splits only the touched blocks off into a
private copy at write time; everything else keeps being shared. There's no
real .NET analogue — `File.Copy` always performs a full byte-for-byte
duplication; the closest mental model is a persistent/immutable data
structure sharing unmodified nodes between two versions, except the
"nodes" are filesystem extents and the sharing is transparent to every
reader.

This is different from both an ordinary copy and a hard link:

- An ordinary copy duplicates the bytes immediately — two independent
  files, twice the disk space, from the first write.
- A hard link makes two *names* point at the same inode — there's only
  ever one file; changing "either" changes both, and their metadata
  (including mtime) is the same value, not just the same content.
- A reflink clone is a new, independent inode (its own metadata, its own
  mtime) that happens to start out pointing at the same extents as the
  source. It behaves exactly like a real copy from every caller's
  perspective — the sharing is a storage optimization the kernel manages
  invisibly.

**Where it lives here.** `src/transfer.rs`'s reflink fast path
(`add-reflink-transfer`) calls `reflink_copy::reflink(src, &part_path)`
before falling back to the crate's existing stream-copy-and-hash path. The
crate's `reflink()` signature deliberately mirrors `std::fs::copy`'s shape
(`(impl AsRef<Path>, impl AsRef<Path>) -> io::Result<...>`) — a small but
useful Rust convention: a fallback-shaped function reuses the *exact*
argument and error shape of the thing it's a faster alternative to, so a
caller can reason about it as "try this, and on any `io::Error`, you
already know how to fall back." That's exactly what `transfer_inner` does:
any `Err` from `reflink()`, regardless of *why* it failed (different
filesystem, filesystem without CoW support, or something else), is treated
identically — remove the failed attempt's `.part` file and fall through to
`copy_and_hash`. [ADR 0013](../adr/0013-reflink-structural-verification.md)
covers the resulting safety question: because a successful clone is
all-or-nothing and byte-identical by construction, it's treated as fully
verified without a read-back hash.

**Takeaway.** When wrapping a fast/unsafe OS primitive with a slower,
always-correct fallback, matching the fallback's function signature (same
inputs, same error type) makes the "try fast, catch-all fall back to slow"
pattern a one-line `match` at the call site instead of bespoke plumbing per
failure mode. And: "no error" from an all-or-nothing kernel operation can
be a *stronger* correctness guarantee than re-reading and hashing the
result yourself — verification doesn't always mean re-checking; sometimes
it means trusting a primitive that can't partially fail.

See [ADR 0013](../adr/0013-reflink-structural-verification.md) and
[[errors-thiserror-vs-anyhow]].
