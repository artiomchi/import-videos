# Canonicalized path boundaries

**Concept.** `Path::starts_with` is a purely lexical, component-wise
comparison — it never touches the filesystem. `Path::new("/a/b").starts_with("/a")`
is `true`, but so is the lexical comparison for a path that *looks* nested
under a boundary only because of a `..` segment, a trailing symlink, or a
relative path resolved against a different working directory than the
boundary was. None of that is hypothetical on a real filesystem: a
symlink inside the tree being walked can point anywhere, and a boundary
computed once at start-up can be a relative path while a candidate
directory discovered later is absolute (or vice versa). `Path::canonicalize`
(the `realpath(3)` equivalent — no direct .NET analogue;
`Path.GetFullPath` is the closest, but it only resolves `.`/`..` and
drive letters, never symlinks) resolves a path against the real
filesystem: it follows every symlink, resolves `.`/`..`, and returns an
absolute path. Canonicalizing *both* sides of a "is this inside that
boundary?" check before comparing turns a lexical guess into a real
containment guarantee.

**Where it lives here.** `transfer.rs`'s `prune_empty_ancestors`
(improve-scan-and-cleanup design D6) climbs from a just-emptied source
directory upward, removing each ancestor while it's still empty, but
must never remove — or climb past — the scanned source root itself. Both
the candidate directory and `source_root` are canonicalized before every
comparison:

```rust
let Ok(source_root) = source_root.canonicalize() else { return };
// ...
let Ok(canonical) = current.canonicalize() else { return };
if canonical == source_root || !canonical.starts_with(&source_root) {
    return;
}
```

A canonicalization failure (the path vanished, or a permissions error)
is treated the same as "stop climbing" — not an error to propagate. This
is deliberately the same discipline `cleanup.rs::resolve_and_check_quarantine_root`
already applies on the quarantine side (`root == destination ||
destination.starts_with(&root)`), just with canonicalization added
because pruning walks a live, attacker-adjacent directory tree
one level at a time rather than comparing two already-trusted, one-shot
config paths.

**Takeaway.** Any time code decides whether one path is "inside" another
and the answer gates a destructive operation (delete, prune, chroot-like
containment), canonicalize both sides first — `starts_with` alone is a
string check, not a filesystem check, and the two only agree by
coincidence once symlinks or mixed relative/absolute paths are in play.

See [ADR 0003](../adr/0003-scan-plan-execute-safety-model.md) and
[[serde-view-models-vs-domain-types]] for the sibling design principle of
keeping one invariant enforced in exactly one place.
