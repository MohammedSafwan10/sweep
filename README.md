# sweep 🧹

Find what's eating your disk and clean it safely — in seconds.

![CI](https://github.com/MohammedSafwan10/sweep/actions/workflows/ci.yml/badge.svg)
![License: MIT](https://img.shields.io/badge/license-MIT-green)

PowerShell took **3+ minutes and timed out** scanning one folder. `sweep` scans large trees quickly, recognizes common developer caches, and only deletes what you approve — to the Recycle Bin first, never permanently by default.

```text
> sweep scan C:\ --top 20
> sweep review D:\Dev --top 20
> sweep detectors
> sweep clean                 # preview only
> sweep bin --drive C         # preview Recycle Bin contents
```

## Install

```sh
cargo install --git https://github.com/MohammedSafwan10/sweep
```

No Rust? Download `sweep.exe` from [Releases](https://github.com/MohammedSafwan10/sweep/releases)
and run it in a terminal — no install needed. winget / scoop packages are on the roadmap.

## Usage

```sh
sweep scan <PATH> [--top 30] [--min-size 1MB]   # what's big under PATH
sweep scan <PATH> --allocated                    # slower on-disk file allocation estimate
sweep review <PATH> [--top 20] [--min-size 500MB] # large unclassified paths; review only
sweep detectors [--roots DIR...]                # known caches + build junk
sweep clean                                     # dry-run plan (default: safe items, to trash)
sweep clean --execute                           # ask, then delete to Recycle Bin
sweep clean --execute --permanent               # skip the bin for regenerable build output
sweep clean --execute --only caution --yes      # include caution items, no prompt
sweep bin --drive C --drive D                   # inspect Recycle Bins; dry-run
sweep bin --drive C --drive D --execute         # confirm permanent emptying
```

`scan` counts logical file sizes and shows filesystem used/free capacity separately.
Protected or unreadable entries make a scan **PARTIAL**; its byte count then
covers only observed files. Add `--strict` to return exit code 2 for a partial
scan while retaining the report (including in `--json` output). Sparse files,
hard links, compression, and filesystem metadata can make logical totals differ
from actual drive usage.
`--allocated` queries allocated bytes for each file path. It may count hard
links more than once, so it is a diagnostic estimate, not guaranteed space
reclaimable by deletion. Partial scans also report issue counts and sample
paths to retry after access problems are resolved.

`detectors` adds a short review order for known cleanup candidates and notes
what may need rebuilding. Large unclassified paths remain review-only.
`review` lists large paths not covered by a known project artifact finding.
Use `--all-detectors` for drive-wide reviews that include global caches. It
never authorizes deleting those paths and does not perform cleanup.
After an executed `clean`, the receipt shows available space before and after
on each affected volume. Recycle Bin moves often leave available space nearly
unchanged until the bin is emptied; other system activity can also affect the
measurement. `sweep clean` never empties the bin. On Windows, `sweep bin`
inspects selected drive letters and requires a separate `--execute` and
confirmation to empty them permanently.

Every command accepts `--json` for scripts and coding agents (see [docs/AGENTS.md](docs/AGENTS.md)).
Full flags: `sweep scan --help`, `sweep detectors --help`, `sweep clean --help`, `sweep bin --help`.

## Safety first

- **Dry-run by default.** `sweep clean` without `--execute` deletes nothing.
- **Recycle Bin by default.** `--permanent` is opt-in.
- **Three safety labels:** `SAFE` (regenerable caches) · `CAUTION` (slow to rebuild) · `DANGER` (data-loss risk — needs `--only all --force`).
- npm cleanup targets `_cacache` only, preserving `_npx` packages that may be running as tools.
- `docker` volumes, databases and anything unclassified are never touched silently.

Details: [docs/SAFETY.md](docs/SAFETY.md) · detectors: [docs/DETECTORS.md](docs/DETECTORS.md)

## For coding agents

`sweep` is built to be driven by agents (OpenCode, etc.): stable `--json` schemas, exit codes, and a strict opt-in model for destructive actions. Contract: [docs/AGENTS.md](docs/AGENTS.md).

## Performance

- **Parallel scanner** — one worker per core with thread-local aggregation: ~2M files in seconds.
- **Parallel one-pass deleter** — raw filesystem unlinks across independent subtrees, sizes counted while deleting: a 30k-file tree deletes in ~1.5s (v0.1.0 took 56s on the same fixture).
- **Parallel detectors** — the full registry scans concurrently; project walks prune managed cache trees instead of descending into them.

## Roadmap

- [x] M1 — parallel scanner + CLI (+ `--json`)
- [x] M2 — 20-detector registry (Rust, Dart, JS, TS, Python, Go, .NET, Gradle, Android, Docker, AI-agent artifacts, browser caches…)
- [x] M3 — safe cleaner (trash-first, receipts)
- [x] M5 (part 1) — v0.2.0 released with Windows exe + parallel one-pass deleter
- [x] v0.3.0 — partial-scan reporting, allocated-size option, review command, free-space receipts, explicit Windows Recycle Bin command
- [ ] M4 — interactive TUI (`ratatui`)
- [ ] M5 (part 2) — winget/scoop, signed Windows builds

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Bug reports with `sweep scan --json` output attached are gold.

## License

MIT — [LICENSE-MIT](LICENSE-MIT).
