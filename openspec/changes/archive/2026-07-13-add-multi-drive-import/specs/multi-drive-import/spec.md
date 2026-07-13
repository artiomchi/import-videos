## ADDED Requirements

### Requirement: Every matching drive is processed in one invocation
For a `source: auto` profile, `scan` and `import` SHALL detect and process every mounted volume accepted by the device implementation's `detect()` in a single invocation — no matching volume SHALL be silently dropped. Drives SHALL be processed sequentially, one at a time, in ascending order of resolved path (see cli-core, "Source resolution via explicit path or mount probing"). This applies only to `source: auto`; an explicit `--source PATH` or a profile with `source: <path>` SHALL continue to resolve and process exactly one path, unaffected by this capability.

#### Scenario: Two matching drives are both processed
- **WHEN** `scan gopro` runs and two mounted volumes both satisfy the GoPro device's `detect()`
- **THEN** the inventory for both drives is printed in the same invocation, in ascending path order

#### Scenario: Explicit source is unaffected
- **WHEN** `import gopro --source /mnt/card` runs while other GoPro-matching volumes are also mounted
- **THEN** only `/mnt/card` is scanned and imported; the other volumes are not touched or mentioned

#### Scenario: Zero matching drives
- **WHEN** no mounted volume under the configured mount roots satisfies the profile's `detect()`
- **THEN** the command reports no sources found and exits with code 0, exactly as a single-source run does today

### Requirement: Each drive is identified by name and path before its output
For every drive processed under a `source: auto` profile, both `scan` and `import` SHALL print that drive's name (its mount-point directory name) and full resolved path before printing anything else about that drive — its scan summary, its plan, its confirmation prompt, or its execution report. The name is drawn from the same directory entry already used for detection; no additional filesystem or device query SHALL be performed to obtain it. In `--json` mode, each drive's JSON entry SHALL carry its `name` and `path` fields alongside its payload (see "Multi-drive JSON output enumerates every drive").

#### Scenario: Drive header precedes its scan summary
- **WHEN** `scan` processes a drive mounted at `/run/media/artiom/GOPRO075`
- **THEN** a line naming `GOPRO075` and its full path is printed before that drive's scan summary

#### Scenario: Drive header precedes its confirmation prompt and report
- **WHEN** `import` processes a drive and reaches its confirmation prompt
- **THEN** that drive's name and path have already been printed, before the prompt and before its execution report

#### Scenario: Two same-named drives both show their full path
- **WHEN** two mounted volumes share the same directory name (for example, both read `NO NAME`) under different mount roots
- **THEN** each drive's header still shows its distinguishing full path, and both are processed as separate drives

### Requirement: Import processes drives sequentially with independent confirmation
For a `source: auto` profile, `import` SHALL run the full scan → build plan → print plan → confirm → execute → print report cycle independently for each detected drive, completing one drive's cycle (including printing its execution report) before beginning the next drive's cycle. `--dry-run` SHALL apply the same per-drive cycle through printing the plan, without executing or prompting for any drive.

#### Scenario: One drive fully completes before the next starts
- **WHEN** `import` processes two drives
- **THEN** drive 1's plan, confirmation, and execution report are all printed before drive 2's plan is printed

#### Scenario: Dry run prints every drive's plan without executing any
- **WHEN** `import --dry-run` runs against a `source: auto` profile matching three drives
- **THEN** all three drives' plans are printed and no filesystem changes occur for any of them

### Requirement: A drive's failure does not block the remaining drives
If a drive's scan, plan resolution, or execution fails — whether a hard error, or, for execution, at least one file or sidecar transfer outcome of `Failed` — that failure SHALL be recorded against that drive only. Subsequent drives in the same invocation SHALL still be scanned or imported normally; a failure on one drive MUST NOT prevent any other drive from being processed. The process SHALL exit with a non-zero code if any drive recorded a hard error or a failed transfer, even if every other drive completed successfully.

#### Scenario: A later drive still imports after an earlier drive fails
- **WHEN** `import` processes drive 1 (which has one file fail verification) and drive 2 (which transfers cleanly) in the same invocation
- **THEN** drive 2's files are transferred and, if eligible, its source is cleaned, exactly as if drive 1 had not failed

#### Scenario: Exit code reflects any drive's failure
- **WHEN** drive 2 of 3 has a failed file transfer and drives 1 and 3 complete without error
- **THEN** the process exits with code 1, and the report identifies which drive failed

#### Scenario: A hard error on one drive is caught, not propagated
- **WHEN** a device's `scan()` returns an error for drive 2 (for example, malformed metadata)
- **THEN** drive 2's outcome records that error, drive 3 is still scanned and imported, and no drive's already-printed output is lost

### Requirement: A detected drive with nothing to import is reported distinctly
A drive that is detected (matches `detect()`) but whose scan produces zero groups SHALL be reported as an empty drive — its name and path are printed along with a statement that no media was found — distinct from a profile that detects zero drives. An empty drive SHALL NOT trigger a confirmation prompt or an execution attempt, and SHALL NOT be counted as a failure.

#### Scenario: An empty drive is named, not skipped
- **WHEN** a mounted volume matches `detect()` but contains no media the profile would keep, quarantine, or need to ignore
- **THEN** that drive's name and path are printed along with a "no media found" statement, and the run continues to the next drive without prompting

#### Scenario: An empty drive among others does not affect exit code
- **WHEN** one of three drives is empty and the other two complete successfully
- **THEN** the process exits with code 0

### Requirement: Multi-drive JSON output enumerates every drive
When a `source: auto` profile's `scan` or `import --json` run processes more than zero drives, the emitted document SHALL contain a `drives` array with one entry per detected drive, in the same ascending-path order used for processing. Each entry SHALL carry `name`, `path`, and a `status` of `completed`, `completed_with_failures`, `empty`, or `error`; entries with `status: "error"` SHALL carry an `error` message and no result payload; all other statuses SHALL carry the same payload shape a single-drive JSON response would carry for that command (`summary` for `scan`, `plan` for `import --dry-run`, `results` for `import`). For `import`, the document SHALL also carry an aggregate `any_failed` boolean. This SHALL NOT change the JSON shape for an explicit `--source` or non-`auto` profile, which continues to emit today's single flat document with no `drives` key. The "exactly one JSON document per invocation" guarantee (cli-core, "Import executes exactly the scanned plan") SHALL still hold — this document's `drives` array is what that one document now contains for `auto` profiles.

#### Scenario: Scan JSON lists every drive
- **WHEN** `scan gopro --json` runs against a `source: auto` profile matching two drives
- **THEN** stdout parses as one JSON document containing a `drives` array with exactly two entries, each with its name, path, and scan summary

#### Scenario: A failed drive's JSON entry carries its error, not a result
- **WHEN** `import gopro --json --yes` runs and drive 2's scan fails with a hard error
- **THEN** drive 2's entry has status `"error"` and an error message, drives on either side of it have their normal result payloads, and `any_failed` is `true`

#### Scenario: Explicit source keeps today's flat JSON shape
- **WHEN** `scan gopro --source /tmp/fake-card --json` runs
- **THEN** stdout parses as today's single flat document with no `drives` key
