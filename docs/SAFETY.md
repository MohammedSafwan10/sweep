# SAFETY.md — what sweep will and won't do

## The model

Every cleanable thing carries a label:

| Label | Meaning | Default behavior |
| ----- | ------- | ---------------- |
| `SAFE` | Regenerable byte-for-byte by the next build/install (`pub-cache`, `build/`, `.next/`, npm cache, cargo registry, old Gradle dists). | Included in `clean`. |
| `CAUTION` | Deletable, but rebuilding costs minutes or mobile data (`node_modules`, gradle `modules-2`, Android NDK/platforms, stale temp, Docker vhdx). | Needs `--only caution` (or `all`). |
| `DANGER` | Real data-loss risk if misclassified. Reserved for future detectors (e.g. anything holding live databases). | Needs `--only all` **plus** `--force`. |

> v1 note: no detector emits `Danger` yet — the Docker vhdx is `Caution`+manual
> (reported with instructions, never executed). The `All`+`--force` gate is
> covered by cleaner unit tests so future detectors inherit it.

## Guarantees

1. **Dry-run by default.** `sweep clean` without `--execute` only plans and reports.
2. **Recycle Bin by default.** Deletes go through the OS trash (`trash` crate). `--permanent` bypasses it and must be typed explicitly.
3. **v1 never runs external commands.** Findings with `run_command`/`manual` actions (Docker) are reported with instructions and land in `skipped[]` — sweep will not execute `docker system prune --volumes` for you, because volumes can hold database data.
4. **Nested symlinks and Windows reparse points are pruned from scans.** Cleaning checks target and ancestor components before planning and again before deletion. Explicit scan roots are resolved to their canonical location.
5. **Missing paths are skipped, not errors.** Concurrent modification (a build running during clean) surfaces in `errors[]` with exit code 2 instead of aborting the batch.
6. **Permanent deletes count bytes while unlinking.** The parallel deleter removes subtree contents concurrently (raw `std::fs` unlinks — benchmarked faster than shell file-operation APIs) and accumulates the logical sizes of files it actually removed; locked files surface as errors and their bytes are not counted for the failed item. Trash plans still pre-measure (the OS moves the tree). Byte totals describe logical file lengths, not guaranteed disk space recovered: hard links and sparse files can differ, and moving to the Recycle Bin does not release disk space until it is emptied.
7. **Confirmation executes the displayed plan.** It does not discover and delete new findings after approval. Duplicate paths are counted once; a finding covered by a broader surviving finding is skipped (one delete, bytes counted once). A parent containing an item excluded from the run (filtered level, danger gate) is itself skipped so the gate cannot be bypassed.
8. **Temp cleanup needs caution opt-in.** Every descendant must be older than seven days and readable; freshness is checked again before execution. Age alone cannot prove data is disposable.

These checks reduce accidental traversal and stale-plan risks; they are not an atomic filesystem transaction. Stop builds and installers before cleaning. Hostile concurrent path replacement between the last check and the OS deletion call is not fully prevented. A failed recursive deletion can also have removed some children before reporting an error.

## What sweep will never do

- Delete anything outside a detector finding or an explicit scan entry.
- Follow a symlink/junction during delete (deletion uses the literal path).
- Touch `DANGER` without `--only all --force` in the same command.
- Phone home, auto-update, or upload paths (there is no network code at all).
