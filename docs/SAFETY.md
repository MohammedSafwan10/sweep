# SAFETY.md — what sweep will and won't do

## The model

Every cleanable thing carries a label:

| Label | Meaning | Default behavior |
| ----- | ------- | ---------------- |
| `SAFE` | Regenerable byte-for-byte by the next build/install (`pub-cache`, `build/`, `.next/`, npm download cache, cargo registry). | Included in `clean`. |
| `CAUTION` | Deletable, but rebuilding costs minutes or mobile data (`node_modules`, gradle `modules-2`, Android NDK/platforms, stale temp, Docker vhdx). | Needs `--only caution` (or `all`). |
| `DANGER` | Real data-loss risk if misclassified. Reserved for future detectors (e.g. anything holding live databases). | Needs `--only all` **plus** `--force`. |

> No current detector emits `Danger` — the Docker vhdx is `Caution`+manual
> (reported with instructions, never executed). The `All`+`--force` gate is
> covered by cleaner unit tests so future detectors inherit it.

## Cleanup checks and behavior

1. **Dry-run by default.** `sweep clean` without `--execute` only plans and reports.
2. **Recycle Bin by default.** Deletes go through the OS trash (`trash` crate). `--permanent` bypasses it and must be typed explicitly.
3. **Sweep never runs external cleanup commands.** Findings with `run_command`/`manual` actions (Docker) are reported with instructions and land in `skipped[]` — sweep will not execute `docker system prune --volumes` for you, because volumes can hold database data.
4. **Nested symlinks and Windows reparse points are pruned from scans.** Cleaning checks target and ancestor components before planning and again before deletion. Explicit scan roots are resolved to their canonical location.
5. **Missing paths are skipped, not errors.** Concurrent modification (a build running during clean) surfaces in `errors[]` with exit code 2 instead of aborting the batch.
6. **Changed or unverifiable targets are refused.** Planning records a fingerprint of the whole target: names, entry types, filesystem identities, sizes, timestamps, read-only metadata, empty directories, and SHA-256 file contents. Execution recomputes it immediately before deleting that target. A mismatch or read failure leaves the whole target intact and reports an error. Links/reparse points and special files inside a tree prevent it being snapshotted. Other independent unchanged targets can still succeed; a vanished target is skipped.
7. **Confirmation executes the displayed plan.** It does not discover and delete new findings after approval. Duplicate paths are counted once; a finding covered by a broader surviving finding is skipped (one delete, bytes counted once). A parent containing an item excluded from the run (filtered level, danger gate) is itself skipped so the gate cannot be bypassed.
8. **Temp cleanup needs caution opt-in.** Every descendant must be older than seven days and readable; freshness is checked again before execution. Age alone cannot prove data is disposable.
9. **Executed cleans report observed free space.** Receipts include available bytes before and after on affected volumes. Other programs can change that measurement during a clean, and moving items to the Recycle Bin usually does not free space until the bin is emptied. Dry runs contain no space measurements.
10. **Emptying the Recycle Bin is a separate Windows command.** `sweep bin --drive C` only reports item count and bytes. `--execute` permanently empties that drive's bin after confirmation; `--json --execute` also requires `--yes`. `sweep clean` never empties the bin implicitly.

These checks reduce accidental traversal and stale-plan risks; they are not an atomic filesystem transaction. Stop builds and installers before cleaning. Hostile concurrent path replacement between the last check and the OS deletion call is not fully prevented. A failed recursive deletion can also have removed some children before reporting an error.

## Saved previews and executions

Use `clean --save-plan plan.json` to capture an immutable-by-default preview file;
it refuses to overwrite existing files or write inside a selected target. Review
its exact targets, safety labels, snapshots, and deletion mode. After approval,
`clean --from-plan plan.json --execute` uses only those targets, without discovery.
`--json` execution still requires `--yes`. A permanent plan requires explicit
`--permanent` at creation and execution; safety ceiling and danger gates still apply.
Changing deletion mode after review is refused. Do not edit a reviewed plan;
generate and approve a new file instead.

Saved-plan schema version 2 is separate from receipt `schema_version: 1`.
v0.4.0 schema-1 plans are rejected; recreate and review them after upgrading.
The SHA-256 `plan_id` detects accidental edits and is echoed in CLI receipts. It is
not a signature: someone who can modify a plan can also recompute its checksum.
Only execute plan files you trust and have reviewed. JSON fields do not establish
human approval. Loading an older plan without snapshots, an unsupported plan
schema, or inconsistent data fails closed.

Interactive `clean --execute` snapshots before showing its confirmation prompt.
A separate plain `clean --execute` still rescans unless `--from-plan` is provided;
a dry-run receipt redirected to a JSON file is not a saved plan.

Fingerprints are conservative and local to the filesystem/tool implementation;
even metadata-only changes or replacing a root with identical contents require
review again. Full content hashing adds I/O and does not eliminate races after the
last check. Stop writers: a new file arriving during deletion may still be removed,
and a recursive deletion error can occur after some children were removed.
Trash receipts use the just-verified logical byte count; permanent receipts count
bytes while unlinking. Neither is a promise of physically reclaimed disk space.

v0.5.0 hashes full contents with a shared pool of at most eight workers. Each
entry includes its relative name, type, metadata, filesystem identity, and (for a
file) content SHA-256. Entry hashes are folded in deterministic traversal order.
Workers reuse the read handle for identity/metadata checks; separate identity
probes request only metadata access on Windows. No-follow opens reject final-path
links, and directories are revalidated after all file hashes finish. Inventory
memory scales with entry count; file contents are streamed, never kept in memory.
Parallel checks do not turn this into an atomic deletion transaction.

## What sweep will never do

- Delete anything outside a detector finding during `clean`. The separate `bin --execute` command permanently removes only the selected drives' Recycle Bin contents after confirmation.
- Follow a symlink/junction during delete (deletion uses the literal path).
- Touch `DANGER` without `--only all --force` in the same command.
- Phone home, auto-update, or upload paths (there is no network code at all).
