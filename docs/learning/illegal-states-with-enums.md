# Making illegal states unrepresentable with enums

**Concept.** The Tesla `reasons` config field is allow-xor-deny: a
profile may keep only listed reasons, or drop only listed reasons, but
never both and never neither. The two "obvious" ways to model that
in a struct are `{ allow: Option<Vec<String>>, deny: Option<Vec<String>> }`
(four representable states, only two valid — every reader of the type
now has to guess or re-derive the invariant) or a single enum,
`enum Reasons { Allow(Vec<String>), Deny(Vec<String>) }` (exactly two
representable states, both valid — the invariant is the type). C# has
no equivalent to a data-carrying enum (a discriminated union); the
closest idiom is a class hierarchy with a private constructor plus a
factory, which is far more ceremony for the same guarantee. Rust's
enums make this cheap enough to reach for by default whenever a type
would otherwise need "at most one of" fields plus a comment.

**Where it lives here.** `Reasons` in `src/source/tesla.rs`. Because
it is an enum rather than two options, `config::load` never needs to
check "did the user set both / neither" — deserialization *is* that
check. Serde's default representation for a newtype-variant enum is a
single-key map (`{allow: [...]}` or `{deny: [...]}`), which happens to
match the desired YAML shape and load-time error exactly, for free
(spec: "reasons allow and deny are mutually exclusive").

That representation choice has one real cost: serde's `#[serde(flatten)]`
(used on `RawProfile.kind: SourceKind`, ADR 0004) cannot re-deserialize
a *nested* enum that was serialized in its own default (tag-based) YAML
form — flatten buffers surrounding fields into a generic `Content` type
that doesn't support enum input. This only bites a
`Serialize`-then-`Deserialize` round trip of the whole `RawProfile`; the
real config-loading path (`serde_yaml_ng::from_value` on already-parsed
`Value`s, from hand-written YAML text) never serializes anything and is
unaffected. See `config::tests::tesla_variant_serde_round_trips` and
`source::tesla::tests::reasons_round_trips_outside_flatten` for where
this is pinned down.

**Takeaway.** When a type has a "pick exactly one of" invariant, reach
for an enum before reaching for a comment next to two `Option` fields —
the compiler (and, here, serde) enforces it instead of trusting every
future editor to remember. But an enum nested inside a `#[serde(flatten)]`
field only round-trips one direction (deserialize); don't assume
`Serialize` on such a type is meant to feed back through the same
flattened struct.

See ADR 0004 (YAML config) and ADR 0006 (Tesla time handling).
