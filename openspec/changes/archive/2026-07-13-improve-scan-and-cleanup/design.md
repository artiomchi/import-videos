## Context

Today `scan` and `import` (dry-run or real) both funnel through `plan::build_plan`, which calls `ImportSource::scan()` and then resolves every group's destination/quarantine path via the profile's layout template before returning a single `ImportPlan`. For GoPro profiles, `ImportSource::scan()` (`GoproSource::build_session`) unconditionally runs GPS telemetry extraction for every session — opening each chapter's `gpmd` track and scanning samples for a usable fix — regardless of whether the session will end up `Keep` or `Quarantine`. This is the dominant cost of a GoPro scan, and it runs even though:
- `Quarantine` verdicts don't use the timestamp at all (`quarantine_root/session-{id}`, no date), and telemetry never influences the verdict itself (gopro-telemetry spec: "Telemetry MUST NOT influence Keep/Quarantine verdicts").
- `scan`'s whole purpose (ADR 0003) is a fast, read-only look at what's on the card — it doesn't need to be as precise as `import`'s destination-resolving plan.

Separately, `transfer::execute_inner`'s source-deletion loop (`fs::remove_file` per file) never removes the directory a group's files lived in, so a fully-imported-and-deleted session leaves an empty directory behind. For Tesla, `TeslaSource::scan()` treats any directory under `SavedClips`/`SentryClips` as an event candidate regardless of contents, so that leftover empty directory resurfaces as a phantom `Keep` group with 0 files on every future scan, forever, since nothing ever removes or renames it.

Both problems live in the same scan → plan → execute pipeline (ADR 0003) and touch the same functions (`plan.rs`, `transfer.rs`, `source/gopro.rs`), so this design covers both.

## Goals / Non-Goals

**Goals:**
- `scan` never resolves a destination path and never runs GPS telemetry — it becomes a fast, source-only inventory.
- `import` (dry-run or real) remains the single accurate source of truth for what will actually happen, including resolved destination paths and (unless disabled) GPS-corrected timestamps.
- GPS telemetry lookup can be disabled outright for a GoPro profile, via config and a per-invocation CLI override, for users who know their footage has no GPS fix or who trust the camera clock.
- Sessions that will end up `Quarantine`d never pay the telemetry cost, with no flag required — this is a strict improvement with no accuracy trade-off, since telemetry doesn't affect verdict or quarantine's path.
- A verified source deletion prunes the now-empty directories it leaves behind, bounded so it can never reach outside the scanned source root.
- A group with zero files never surfaces in a plan or a scan summary, regardless of which device produced it or why it's empty.

**Non-Goals:**
- Not attempting to derive GPS telemetry from the copy/reflink destination during `import` execution (explored and rejected: it would require destination-path resolution to happen *after* a file is already copied, which breaks ADR 0003's "plan fully resolved before execution" ordering, and would force sidecar construction — currently a scan-time, device-only concern — to move into the device-agnostic `transfer.rs`, growing the `ImportSource` trait boundary for a marginal win).
- Not adding any destination-side checking (quick-match, collision detection) to `scan` — `scan` only ever looks at the source.
- Not changing `src/source/tesla.rs` — the zero-file-group fix is a generic, device-agnostic filter applied after any `ImportSource::scan()` call.
- Not extending the same directory-pruning behavior to `cleanup`'s quarantine purge in this change (see Open Questions).
- Not changing how `Keep`/`Quarantine` verdicts are decided — only *when* the (already verdict-independent) telemetry lookup runs.

## Decisions

### D1 — `scan` gets its own lightweight inventory, separate from `ImportPlan`

Add `plan::build_scan_summary(profile, source_impl, source_root, progress) -> Result<ScanSummary>`, used only by `run_scan`. It calls `ImportSource::scan()` exactly like `build_plan` does, but instead of resolving `destination`/`quarantine_path` per group via the layout template, it just tallies:

```rust
pub struct ScanSummary {
    pub entries: Vec<ScanEntry>,
}
pub struct ScanEntry {
    pub name: String,
    pub verdict: Verdict,
    pub file_count: usize,
    pub total_size: u64,
    pub recorded_at: Timestamp,      // whatever the group already carries — camera-clock
    pub unrecognized_files: Vec<String>, // only populated for the unrecognized-files group
}
```

`group.timestamp` itself stays cheap to obtain regardless of GPS: `chapter_civil_time` (a single `mvhd` box read) already runs for every session independent of telemetry, so `ScanEntry.recorded_at` is a real, already-computed value — just not GPS-corrected. It's fine to show as an approximate per-session time in the summary; the thing we stop doing is claiming a *destination path*, since that's the value that can be silently wrong at a day boundary.

`build_plan` (used by `import`, dry-run and real) is unchanged in shape — it still resolves every action's destination/quarantine path, and remains the one place that does. `PlannedAction.destination: Option<PathBuf>` keeps its existing meaning (`None` only for `copy_quarantine: false`); `ScanSummary`/`ScanEntry` is a distinct type specifically so scan's structural absence of a path is never confused with that case.

Rejected alternative: reuse `ImportPlan`/`PlannedAction` for scan with `destination` forced to `None`. Rejected because `None` already means something else there (quarantine copy disabled), and a renderer would have to disambiguate by verdict — a latent rendering bug waiting to happen.

### D2 — GPS lookup gated by `ScanContext.gps_lookup`, resolved once by the caller

Add `gps_lookup: bool` to `ScanContext` (`src/source/mod.rs`), alongside `ignore`/`tz`/`imported_at`/`progress` — same pattern the struct's own doc comment already describes for fields a device may simply not read (`TeslaSource` never reads it, same as it never reads `imported_at` today).

The caller (`lib.rs`) resolves this once per command, never per-device:
- `run_scan`: always `false` — structural, not driven by any flag. `scan` never does telemetry, full stop.
- `run_import` (dry-run or real): `profile`'s effective GoPro `gps_lookup` field (see D3), after CLI overrides are applied.

`GoproSource::build_session` checks `ctx.gps_lookup`; when `false`, it skips `open_chapter_telemetry`/`derive_session_offset` entirely and takes the branch that already exists for "no chapter yielded a usable fix" (`session_offset: None`, camera-clock timestamp, no geo). No new degraded-mode logic — disabling the lookup is architecturally identical to the lookup finding nothing, a path the gopro-telemetry spec already requires to degrade gracefully.

### D3 — New GoPro-only config field + paired CLI override, mirroring `require_marker`

`SourceKind::Gopro` gains `gps_lookup: bool` (default `true`, via `#[serde(default = "default_gps_lookup")]`), following `require_marker`'s exact shape (ADR 0011). CLI gets `--gopro-gps-lookup` / `--no-gopro-gps-lookup`, collapsed to `Overrides.gopro_gps_lookup: Option<bool>` via the existing `override_pair` helper, applied in `apply_overrides` with the same "only valid for gopro profiles" `Error::Config` this project already raises for `require_marker` on a non-GoPro profile.

Placement: unlike `require_marker` (which lives in the shared `OverrideFlags`, since it changes verdict counts `scan` still reports), this flag only has an observable effect on `import` (per D2, `scan` never reads it) — so it belongs on the `Import` variant directly, alongside `--reflink`/`--no-reflink` and `--delete-source`/`--no-delete-source`, not in `OverrideFlags`. `scan --gopro-gps-lookup` is then a clap usage error (unrecognized argument), the same way `scan --reflink` already is today, rather than a silently-accepted no-op.

### D4 — Marker-first ordering in `build_session`, always on

Today `GoproSource::build_session` derives `session_offset` (the expensive telemetry search) *then* walks chapters collecting HiLight markers, using `session_offset`/per-chapter `telemetry[i]` to compute each marker's corrected wall time and coordinates — verdict is decided only after both are done.

Restructure so marker *offsets* are collected first — `chapter_markers(path)` (an `HMMT` box read, no telemetry) — and the verdict (`Keep` iff `!require_marker || !offsets.is_empty()`, unchanged logic) is decided from that alone. Telemetry (`derive_session_offset`, per-chapter `ChapterTelemetry::open`) only runs when the session will be `Keep` — either because it has markers, or because `require_marker: false` makes every session `Keep` (in which case this reordering has no effect: telemetry always runs, exactly like today). Once telemetry has run (or been skipped), marker wall-times/coordinates are computed exactly as today, just in a second pass instead of interleaved with the offset search.

This is independent of D2/D3: it only has an effect when `require_marker: true` and `gps_lookup` is `true` (if `gps_lookup` is `false`, D2 already skips telemetry entirely and D4 has nothing to reorder around).

Progress-bar impact: `ctx.progress.set_length(total_chapters)` already computes the total up front from `discover()`'s chapter count, independent of whether telemetry runs per chapter — that stays correct. What changes is *how* a `Quarantine`-bound session's chapters get ticked: today they're ticked one-by-one inside `derive_session_offset`'s loop; after this change, a skipped-telemetry session's chapters are never visited by that loop, so they need the same kind of "catch-up" increment `build_session` already applies for chapters the GPS search never reached once a fix is found early (`ctx.progress.inc(chapters.len() as u64 - visited as u64)`, generalized to `visited = 0` when telemetry is skipped outright). Existing progress tests that assume telemetry always runs (`scan_progress_reaches_total_chapter_count`) need updating to cover the skipped-telemetry case explicitly.

### D5 — Zero-file groups filtered once, generically, in core

Both `build_plan` and `build_scan_summary` call `ImportSource::scan()` and then must drop any `(group, verdict)` where `group.files.is_empty()` before the group is tallied or turned into a `PlannedAction`/`ScanEntry`. Factor this into one private helper (e.g. `fn scan_nonempty(source_impl, root, ctx) -> Result<Vec<(MediaGroup, Verdict)>>`) that both call, so the guarantee ("a plan or scan summary never contains a 0-file group") can't drift between the two entry points.

No change to `TeslaSource::scan()` itself — it can keep listing every directory as an event candidate; the empty ones are simply discarded one layer up, uniformly, regardless of why they're empty (this tool's own leftover, a manually cleared folder, or anything else).

### D6 — Directory pruning after verified source deletion, bounded to the source root

`transfer::execute`/`execute_inner` gains a `source_root: &Path` parameter (threaded from `lib.rs::run_import`, which already has it from `plan::resolve_source`). After a group's files are removed (`fs::remove_file` per file, unchanged), collect the distinct parent directories of the files just deleted; for each, walk upward removing the directory while it is both empty and a strict descendant of `source_root` (canonicalized on both sides before comparing, to avoid a symlink or relative-path edge case producing a false "still inside the boundary" result) — `source_root` itself is never removed, mirroring the boundary discipline `cleanup.rs::resolve_and_check_quarantine_root` already applies on the quarantine side. A directory that fails to remove (not actually empty — e.g. a hidden file the scan ignored, or a permissions error) stops the climb at that point rather than failing the import; this is metadata-tier cleanup, not a correctness-critical step, same tier as `stamp_mtime`'s existing "log and move on" failure handling.

Scope: only directories that held one of *this group's just-deleted* files are candidates — no proactive sweep of the whole source tree for unrelated pre-existing empty directories. That sweep is what D5 handles, at scan time, for the read side.

### D7 — `--quarantine`, `--copy-quarantine` / `--no-copy-quarantine` move to `import`-only

Since D1 makes `scan` structurally blind to every destination and quarantine path, these flags — currently in the shared `OverrideFlags` used by both `scan` and `import` — no longer have any observable effect on `scan` either, the same situation D3's new `gps_lookup` flag would have been in if left there. For consistency, move `quarantine: Option<PathBuf>`, `copy_quarantine`, and `no_copy_quarantine` off `OverrideFlags` and onto the `Import` command variant directly, alongside `delete_source`/`no_delete_source`, `reflink`/`no_reflink`, and the new `gps_lookup`/`no_gps_lookup`. `scan --quarantine ...` / `scan --copy-quarantine` / `scan --no-copy-quarantine` become clap usage errors, matching `scan --reflink` today, rather than silently-accepted no-ops.

`OverrideFlags` shrinks to just `gopro_require_marker`/`no_gopro_require_marker` — the one remaining flag pair whose effect `scan`'s inventory can still show, since it changes verdict counts. Whether to keep `OverrideFlags` as a named struct for a single remaining pair, or fold it directly into each command, is left to implementation (tasks.md) as a cosmetic call.

This removes the now-obsolete "Scan accepts but cannot preview quarantine-path overrides" scenario from the cli-core spec delta — `scan` never accepts these flags at all now, so the earlier asymmetry (GPS flags rejected outright, quarantine flags silently accepted-but-ignored) no longer exists.

## Risks / Trade-offs

- **[Risk]** `scan`'s output no longer shows a destination path or GPS-accurate time — a real capability loss for anyone using `scan` as a path preview today. → **Mitigation**: called out as **BREAKING** in the proposal; `import --dry-run` is the documented replacement for "show me exactly what will happen."
- **[Risk]** Canonicalizing paths for D6's boundary check adds a failure mode (a source root that can't be canonicalized, e.g. it vanished mid-run). → **Mitigation**: treat a canonicalization failure as "stop climbing," same as any other pruning failure — never escalate to aborting the deletion that already succeeded.
- **[Risk]** D4's restructuring touches the same function the gopro-telemetry test suite pins tick-by-tick. → **Mitigation**: this design keeps the total-length computation untouched and only changes which loop ticks which chapters; existing progress tests need explicit updates (tracked in tasks.md), not silent breakage.
- **[Risk]** Two independent "skip telemetry" paths now exist (D2's flag, D4's quarantine-bound skip) — risk of divergent behavior if one is updated without the other. → **Mitigation**: both funnel through the same "telemetry not attempted" state (`session_offset: None`), never a separate code path each.

## Migration Plan

No persisted data or schema to migrate — this is a stateless CLI. Rollout is a normal release:
- Update `docs/adr/0003-scan-plan-execute-safety-model.md`'s "Plan review" language, since `scan` and `import --dry-run` no longer produce equivalent plans.
- Call out the `scan` output shape change (text and JSON) in the changelog/release notes as breaking for scripted consumers.
- No config migration needed: the new `gps_lookup` field defaults to `true`, so existing GoPro profiles behave identically until a user opts in to disabling it.
- Rollback is reverting the release; no forward-only state is created.

## Open Questions

- Should `cleanup`'s quarantine-purge path get the same empty-directory pruning, for symmetry with D6? Left out of this change's scope — quarantine directories are purged wholesale by `cleanup`, not per-file, so the same "leftover empty dir" failure mode may not even apply there. Worth a quick look in a follow-up rather than bundling here.
- Exact wording/layout of `scan`'s human-readable summary (counts-first vs. list-first, how verbose `-v` should get) is left to implementation in tasks.md rather than pinned here — it's a cosmetic decision, not an architectural one.
