# AGENTS.md — driving `sweep` from a coding agent

This is the machine contract. Human docs: `README.md`, `docs/SAFETY.md`.

## Rules (non-negotiable)

1. **Read-only first.** Always run `scan` / `detectors` and show the human the plan before anything destructive.
2. **`clean` is dry-run by default.** `--execute` requires `--yes` when combined with `--json`. Never pass `--yes` without showing the plan to the human first.
3. **Never `--permanent`, never `--force`** without explicit human approval in this session. `DANGER` items are off-limits by default (v1 ships zero `Danger` findings — the gate is covered by unit tests for future detectors; the Docker vhdx is `Caution`+manual and always skipped, never auto-deleted).
4. **Pin `schema_version`.** If `schema_version != 1`, stop and report — the contract changed.
5. **Parse JSON, never screen-scrape** human output.
6. Prefer `--id <detector>` to scope deletes (e.g. `--id pub-cache`). Unknown `--id` values exit 1 — treat as a typo signal, not as "nothing found".
7. `bin --execute` permanently removes Recycle Bin contents. Only run it after explicit human approval for the named drive letters; first show the `bin` dry-run report. A prior `clean --execute` approval does not authorize emptying the bin.

## Commands

```sh
sweep scan <PATH> [--top 30] [--min-size 1MB] [--min-file 10MB] [--same-fs] [--allocated] [--strict] [--json]
sweep review <PATH> [--top 20] [--min-size 500MB] [--all-detectors] [--strict] [--json]
sweep detectors [--roots DIR...] [--no-docker] [--json]
sweep clean [--id ID...] [--only safe|caution|all] [--force] [--permanent]
            [--execute] [--yes] [--json] [--roots DIR...] [--no-docker]
sweep bin --drive C [--drive D...] [--execute] [--yes] [--json]  # Windows
```

- `--only` default is `safe`. `caution` adds slow-to-rebuild items. `all` still needs `--force` for `danger`.
- `--roots` replaces the default project root (current directory) for artifact detectors; repeatable.
- `--no-docker` skips Docker detection (daemon down / offline machines).
- `scan` reports observed logical bytes. `complete: false` means unreadable entries or a count mismatch made the measured total incomplete. `--strict` returns exit 2 in that case while still printing the report. `volume` gives filesystem capacity and free space when the OS supplies it; logical scan bytes need not match physical used space.
- `--allocated` adds a slower per-path allocation sum. Hard links can be counted more than once; it is never a claim of reclaimable space.
- `detectors` includes `recommendations[]` for review order. Unclassified paths have no cleanup action.
- `review` lists large paths not matched by a project detector removal finding. `--all-detectors` also checks global caches. Its JSON has `review_only: true`; do not turn these entries into automatic delete targets.
- `bin` is dry-run by default and requires explicit drive letters. Its JSON reports per-drive `before` and, after execution, `after` item/byte counts plus available space. `--json --execute` requires `--yes`.

## Exit codes

| Code | Meaning |
| ---- | ------- |
| 0 | Success (including "nothing found" and clean dry-runs) |
| 1 | App fatal: unreadable scan root, bad `--only`, unknown `--id`, `--execute --json` without `--yes` |
| 2 | CLI usage error (bad flags, unparsable sizes) OR clean finished with per-item errors (see `errors[]`; `removed[]` still lists what worked) |

## JSON schemas (`schema_version: 1`)

`scan --json` → `ScanReport`:

```json
{
  "schema_version": 1,
  "root": "C:\\Users\\you\\Projects",
  "complete": false,
  "volume": {"total_bytes": 500000000000, "free_bytes": 100000000000, "available_bytes": 100000000000},
  "total_bytes": 123456789,
  "allocated_bytes": null,
  "total_files": 42000,
  "total_dirs": 3100,
  "warnings": ["permission denied: ..."],
  "warnings_suppressed": 0,
  "issues": {"permission_denied": 1, "not_found": 0, "other": 0},
  "retry_paths": ["C:\\Users\\you\\Projects\\private"],
  "entries": [
    {"path": "...", "bytes": 999, "files": 12, "depth": 1, "is_dir": true}
  ]
}
```

`detectors --json` → `{schema_version, findings[], total_bytes, recommendations[]}` where each finding is:

```json
{
  "detector_id": "pub-cache",
  "label": "pub cache/hosted",
  "bytes": 274726912,
  "safety": "safe",
  "detail": "Downloaded pub.dev packages; restored by `flutter pub get`.",
  "action": {"kind": "remove_path", "path": "C:\\...\\Pub\\Cache\\hosted"}
}
```

`action.kind` is `remove_path` | `run_command` | `manual`. **Only `remove_path` is ever executed by `clean`; the other two come back in `skipped[]` with instructions.**

`clean --json` → `CleanReceipt`:

```json
{
  "schema_version": 1,
  "dry_run": true,
  "freed_bytes": 1234,
  "removed": [{"path": "...", "bytes": 1234, "via": "trash", "safety": "safe"}],
  "skipped": [{"label": "...", "safety": "caution", "reason": "manual steps required: ..."}],
  "errors": [],
  "space_changes": []
}
```

Paths in findings/receipts are canonicalized absolute paths; on Windows
they may carry the `\\?\` verbatim prefix. Compare with canonicalization
(or suffix-match), never raw string equality. Human terminal output shows
the stripped display form instead.

`freed_bytes` counts logical bytes actually deleted: permanent runs
accumulate sizes while unlinking, so an item that fails (locked file)
surfaces in `errors[]` and its bytes are **not** counted; trash runs
measure before handing the tree to the OS. `removed[]` lists every item
that fully succeeded — `sum(removed[].bytes) == freed_bytes` on runs
without partial failures. Interactive `--execute` runs print per-item
progress to **stderr**; `--json` output on stdout stays pure.
`space_changes[]` records available bytes before and after an executed clean
for affected volumes. It is empty in dry runs. These measurements include
other system activity and Recycle Bin moves may show little immediate gain.

## Recommended agent flow

```sh
sweep detectors --json > findings.json          # 1. discover
sweep clean --json > plan.json                  # 2. dry-run plan (default safe-only)
# 3. show the human: total + top items + what stays untouched
sweep clean --json --execute --yes > receipt.json   # 4. only after approval
# 5. report receipt.freed_bytes; surface skipped[]/errors[]
```

For "what's big" questions: `sweep scan <PATH> --json --top 20`.
For targeted work: `sweep clean --id cargo --id pub-cache --json` to preview one ecosystem.
