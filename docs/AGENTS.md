# AGENTS.md — driving `sweep` from a coding agent

This is the machine contract. Human docs: `README.md`, `docs/SAFETY.md`.

## Rules (non-negotiable)

1. **Read-only first.** Always run `scan` / `detectors` and show the human the plan before anything destructive.
2. **`clean` is dry-run by default.** `--execute` requires `--yes` when combined with `--json`. Never pass `--yes` without showing the plan to the human first.
3. **Never `--permanent`, never `--force`** without explicit human approval in this session. `DANGER` items are off-limits by default (v1 ships zero `Danger` findings — the gate is covered by unit tests for future detectors; the Docker vhdx is `Caution`+manual and always skipped, never auto-deleted).
4. **Pin `schema_version`.** If `schema_version != 1`, stop and report — the contract changed.
5. **Parse JSON, never screen-scrape** human output.
6. Prefer `--id <detector>` to scope deletes (e.g. `--id pub-cache`). Unknown `--id` values exit 1 — treat as a typo signal, not as "nothing found".

## Commands

```sh
sweep scan <PATH> [--top 30] [--min-size 1MB] [--min-file 10MB] [--same-fs] [--json]
sweep detectors [--roots DIR...] [--no-docker] [--json]
sweep clean [--id ID...] [--only safe|caution|all] [--force] [--permanent]
            [--execute] [--yes] [--json] [--roots DIR...] [--no-docker]
```

- `--only` default is `safe`. `caution` adds slow-to-rebuild items. `all` still needs `--force` for `danger`.
- `--roots` replaces the default project root (current directory) for artifact detectors; repeatable.
- `--no-docker` skips Docker detection (daemon down / offline machines).

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
  "total_bytes": 123456789,
  "total_files": 42000,
  "total_dirs": 3100,
  "warnings": ["permission denied: ..."],
  "warnings_suppressed": 0,
  "entries": [
    {"path": "...", "bytes": 999, "files": 12, "depth": 1, "is_dir": true}
  ]
}
```

`detectors --json` → `{schema_version, findings[], total_bytes}` where each finding is:

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
  "errors": []
}
```

Paths in findings/receipts are canonicalized absolute paths; on Windows
they may carry the `\\?\` verbatim prefix. Compare with canonicalization
(or suffix-match), never raw string equality. Human terminal output shows
the stripped display form instead.

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
