# sweep 🧹

Find what's eating your disk and clean it safely — in seconds.

![CI](https://github.com/MohammedSafwan10/sweep/actions/workflows/ci.yml/badge.svg)
![License: MIT](https://img.shields.io/badge/license-MIT-green)

PowerShell took **3+ minutes and timed out** scanning one folder. `sweep` does the whole PC in seconds, knows where every dev ecosystem hides its caches, and only deletes what you approve — to the Recycle Bin first, never permanently by default.

```text
> sweep detectors

Detected: 48.2 GiB logical bytes across 241 items

   7.5 GiB  [SAFE  ]  gradle version cache (9.1.0)
   6.5 GiB  [SAFE  ]  npm cache
   4.9 GiB  [CAUTION]  agent task output (noor_flutter_build)
   4.0 GiB  [CAUTION]  chrome on-device model
   ...
Run `sweep clean` for a dry-run plan, `sweep clean --execute` to act.
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
sweep detectors [--roots DIR...]                # known caches + build junk
sweep clean                                     # dry-run plan (default: safe items, to trash)
sweep clean --execute                           # ask, then delete to Recycle Bin
sweep clean --execute --permanent               # skip the bin for regenerable build output
sweep clean --execute --only caution --yes      # include caution items, no prompt
```

Every command accepts `--json` for scripts and coding agents (see [docs/AGENTS.md](docs/AGENTS.md)).
Full flags: `sweep scan --help`, `sweep detectors --help`, `sweep clean --help`.

## Safety first

- **Dry-run by default.** `sweep clean` without `--execute` deletes nothing.
- **Recycle Bin by default.** `--permanent` is opt-in.
- **Three safety labels:** `SAFE` (regenerable caches) · `CAUTION` (slow to rebuild) · `DANGER` (data-loss risk — needs `--only all --force`).
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
- [ ] M4 — interactive TUI (`ratatui`)
- [ ] M5 (part 2) — winget/scoop, signed Windows builds

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Bug reports with `sweep scan --json` output attached are gold.

## License

MIT — [LICENSE-MIT](LICENSE-MIT).
