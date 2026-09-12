# sweep 🧹

Find what's eating your disk and clean it safely — in seconds.

![CI](https://github.com/MohammedSafwan10/sweep/actions/workflows/ci.yml/badge.svg)
![License: MIT](https://img.shields.io/badge/license-MIT-green)

PowerShell took **3+ minutes and timed out** scanning one folder. `sweep` does the whole PC in seconds, knows where every dev ecosystem hides its caches, and only deletes what you approve — to the Recycle Bin first, never permanently by default.

```text
> sweep detectors

Reclaimable: 8.1 GiB across 30 items

   2.1 GiB  [CAUTION]  android ndk/28.2.13676358
 983.6 MiB  [SAFE  ]  cargo target/ (myapp)
 553.9 MiB  [SAFE  ]  pnpm store
 ...
Run `sweep clean` for a dry-run plan, `sweep clean --execute` to act.
```

## Install

```sh
cargo install sweep
```

Windows binary: see [Releases](https://github.com/MohammedSafwan10/sweep/releases).
winget / scoop packages are on the roadmap.

## Usage

```sh
sweep scan <PATH> [--top 30] [--min-size 1MB]   # what's big under PATH
sweep detectors [--roots DIR...]                # known caches + build junk
sweep clean                                     # dry-run plan (default: safe items, to trash)
sweep clean --execute                           # ask, then delete to Recycle Bin
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

## Roadmap

- [x] M1 — parallel scanner + CLI (+ `--json`)
- [x] M2 — 9-detector registry
- [x] M3 — safe cleaner (trash-first, receipts)
- [ ] M4 — interactive TUI (`ratatui`)
- [ ] M5 — winget/scoop, signed Windows builds

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Bug reports with `sweep scan --json` output attached are gold.

## License

MIT — [LICENSE-MIT](LICENSE-MIT).
