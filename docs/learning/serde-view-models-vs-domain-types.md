# Serde view-models vs. deriving on domain types

**Concept** — In .NET, exposing a domain object as JSON usually means either
slapping `[JsonIgnore]`/`[JsonPropertyName]` attributes directly on the
entity, or reaching for a mapping library (AutoMapper) to project it onto a
DTO. Rust's `serde` has no attribute-driven "ignore this for this one
endpoint" story and no reflection-based auto-mapping — `#[derive(Serialize)]`
bakes one shape into the type, permanently. If two call sites need different
JSON shapes from the same struct, or the JSON needs to diverge from the
struct's fields at all, the derive can't flex per-call the way an attribute
can flex per-endpoint.

The idiom this pushes you toward: define a **separate, dedicated struct**
that mirrors only what the JSON contract needs, and write an explicit,
ordinary function that builds one from the domain type. No attributes, no
reflection — just a plain conversion function the compiler checks like any
other code.

**Where it lives here** — `src/report.rs`'s `PlanJson`, `ResultsJson`,
`CleanupJson`, `InspectJson` (and friends) are exactly this: separate
`#[derive(Serialize)]` structs, populated by `plan_to_json`/`results_to_json`/
etc. rather than deriving `Serialize` on `ImportPlan`/`ExecuteReport`
directly (design D4 of `add-maintenance-commands`). The domain types
(`plan::ImportPlan`, `transfer::ExecuteReport`) stay free of any
serialization concern; the JSON shape lives in one place and changes only
when someone deliberately edits a `*Json` struct.

**Takeaway** — When Rust code needs a JSON (or other wire) shape that isn't
simply "the struct, verbatim," don't reach for derive macros or `#[serde(skip)]`
gymnastics on the domain type. Write the little view-model struct and the
explicit mapping function instead — it's more typing than an attribute, but
the JSON contract becomes a piece of code you can read, test, and change on
purpose, rather than a side effect of whatever fields the domain type happens
to have this week.
