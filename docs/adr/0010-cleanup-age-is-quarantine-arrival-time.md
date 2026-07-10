# 0010 — Cleanup age is quarantine arrival time, not recording time

- Status: accepted
- Refines: [0003 — Scan → plan → execute safety model](0003-scan-plan-execute-safety-model.md)
- Date: 2026-07-10

## Context

`cleanup --older-than` decides which quarantine entries to purge. Imported and
quarantined files' mtimes are deliberately stamped to their *recording* time
(gopro-telemetry design D8, ADR 0008's unified sidecar), not the time they
were copied — a correct choice for the destination library, where "when was
this filmed" is what date-organized folders should reflect.

That same stamped mtime is a trap for `cleanup`. A commute recorded in March
but only quarantined (and reviewed) yesterday would, if age were read from
the files' mtimes, look months old and be purged by `--older-than 30d`
immediately — deleting footage the user has not actually had a chance to
review yet. "Older than" must mean "has sat in quarantine unreviewed for that
long," not "was recorded that long ago."

## Decision

`cleanup`'s age filter reads each quarantine entry's own mtime — the
directory's mtime for a group directory (set when the files were copied in),
or the file's own mtime for a stray loose file — never the recording-stamped
mtimes of files nested inside a group directory. This mirrors the design D2
quarantine-entry model in `add-maintenance-commands`.

A re-import that adds a file to an existing quarantine group refreshes that
group directory's mtime, resetting its age. This is accepted: it only ever
errs toward *keeping* footage longer, never purging it sooner.

*Alternative considered:* the newest file mtime inside the group directory.
Rejected — same wrong semantics as reading file mtimes directly (recording
time, not arrival time), for no benefit over the directory's own mtime.

## Consequences

- `cleanup --older-than` is safe to run against footage recorded in
  arbitrary timezones or on cameras with drifted clocks — the filter never
  looks at recording time at all.
- Stray files dropped directly into the quarantine root (not inside a group
  directory) have no separate "arrival" signal beyond their own mtime; this
  is documented as a known limitation in the CLI reference, not a defect.
- The one failure mode (re-import silently resetting a group's age) is
  strictly conservative: it can only delay a purge, never trigger an early
  one.
