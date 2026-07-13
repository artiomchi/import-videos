# 0014 — A failed drive doesn't stop a multi-drive batch

- Status: accepted
- Date: 2026-07-13
- Relates to: [0003 — Scan → plan → execute safety model](0003-scan-plan-execute-safety-model.md), [0012 — Copy verification reads back the written file](0012-copy-verification-reads-back-the-written-file.md)

## Context

`add-multi-drive-import` makes `scan`/`import` process every mounted volume matching a `source: auto` profile in one invocation, instead of silently taking only the first match. That raises a question a single-drive run never had to answer: what happens when one drive's scan, plan resolution, or execution fails, but other detected drives haven't been processed yet?

Two shapes were considered:

1. **Stop the whole batch** at the first failing drive, the same way a single-drive run today lets a hard error propagate all the way out and exit non-zero.
2. **Record the failure against that drive only**, and continue processing the remaining detected drives.

## Decision

A drive's hard error (or, during `import`, at least one failed file/sidecar transfer) is caught at that drive and recorded as its outcome; the batch continues to the next drive regardless. The run's overall exit code still reflects failure if *any* drive failed — nothing about the process's observable pass/fail signal changes, only whether one bad drive can prevent every other drive from being processed at all.

## Consequences

- **Drives are independent physical media.** A card with a corrupt file or a metadata-parse error says nothing about whatever's plugged into the next USB port. Stopping the batch punishes drives that have nothing to do with the failure.
- **Stopping strands already-inserted drives behind a manual workaround.** If a failing drive aborted the run, recovering the other drives' footage would require re-running with `--source <path>` once per remaining drive — exactly the one-drive-at-a-time friction this capability exists to remove.
- **The cost of "wrong" here is small and recoverable.** Continuing past a failure leaves that drive's footage sitting on its card for another day; it does not touch the verification-before-delete guarantee (ADR 0003/0012) for *any* drive, failed or not — each drive's files still only reach source-deletion eligibility after their own successful verified transfer. A stopped batch would trade that mild inconvenience for actively blocking drives that had nothing wrong with them.
- **Per-drive confirmation stays in effect.** Continuing past a failure doesn't change when or how the deletion prompt fires for the *next* drive — each drive still gets its own independent confirmation (or `--yes` skip), unaffected by any other drive's outcome.
- **Exit-code classification is preserved by catching, not converting, at the boundary.** The per-drive loop catches an `Err` and records its `Display` text rather than discarding the original error type — but only inside the multi-drive loop. Explicit sourcing (`--source`, or a profile's `source: <path>`) still propagates a hard error exactly as before this change, so its exit-code distinction between a configuration/template error (2) and a runtime failure (1) is untouched; that distinction only collapses to "some drive in this batch failed" (a non-zero exit) once more than one drive is in play.
