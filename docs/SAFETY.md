# SAFETY.md — what sweep will and won't do

## The model

Every cleanable thing carries a label:

| Label | Meaning | Default behavior |
| ----- | ------- | ---------------- |
| `SAFE` | Regenerable byte-for-byte by the next build/install (`pub-cache`, `build/`, `.next/`, npm cache, cargo registry, old Gradle dists, stale temp). | Included in `clean`. |
| `CAUTION` | Deletable, but rebuilding costs minutes or mobile data (`node_modules`, gradle `modules-2`, Android NDK/platforms, Docker vhdx). | Needs `--only caution` (or `all`). |
| `DANGER` | Real data-loss risk if misclassified. Reserved for future detectors (e.g. anything holding live databases). | Needs `--only all` **plus** `--force`. |

> v1 note: no detector emits `Danger` yet — the Docker vhdx is `Caution`+manual
> (reported with instructions, never executed). The `All`+`--force` gate is
> covered by cleaner unit tests so future detectors inherit it.

## Guarantees

1. **Dry-run by default.** `sweep clean` without `--execute` only plans and reports.
2. **Recycle Bin by default.** Deletes go through the OS trash (`trash` crate). `--permanent` bypasses it and must be typed explicitly.
3. **v1 never runs external commands.** Findings with `run_command`/`manual` actions (Docker) are reported with instructions and land in `skipped[]` — sweep will not execute `docker system prune --volumes` for you, because volumes can hold database data.
4. **Symlinks/junctions are never followed or counted**, so scans can't loop or double-count, and cleaning never traverses out of the target.
5. **Missing paths are skipped, not errors.** Concurrent modification (a build running during clean) surfaces in `errors[]` with exit code 2 instead of aborting the batch.
6. **Sizes are re-statted at delete time** for the receipt, so numbers stay honest.

## What sweep will never do

- Delete anything outside a detector finding or an explicit scan entry.
- Follow a symlink/junction during delete (deletion uses the literal path).
- Touch `DANGER` without `--only all --force` in the same command.
- Phone home, auto-update, or upload paths (there is no network code at all).
