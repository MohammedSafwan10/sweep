# Sweep

Disk cleanup for coding agents. Built in Rust.

![CI](https://github.com/MohammedSafwan10/sweep/actions/workflows/ci.yml/badge.svg)
![License: MIT](https://img.shields.io/badge/license-MIT-green)

Old builds, package caches, and project output accumulate across a developer's SSD.
Sweep gives coding agents a CLI to find what is taking space, recognize known
developer artifacts, preview cleanup, and report the result as structured JSON.

**Scan → review → approve → clean → verify.** Cleanup is a dry run unless
`--execute` is supplied, and uses the Recycle Bin or system trash by default.

### Demo: 31.43 GB cleanup — PowerShell 7 vs Sweep

[Open the 40-second video](https://github.com/MohammedSafwan10/sweep/blob/main/docs/assets/sweep-demo.mp4)


PowerShell 7 and Sweep on isolated copies of **31.43 GB of real build output and package caches**.
This is an edited agent walkthrough using measured command results;
accelerated sections are labeled. Original projects were not deleted.

## Install




Download `sweep.exe` from [Releases](https://github.com/MohammedSafwan10/sweep/releases),
place it in a directory on your agent's `PATH`, and check `sweep --version`.
There is no installer or background service to configure.

Or install from source with Rust 1.88 or newer:

```sh
cargo install --git https://github.com/MohammedSafwan10/sweep
```

## Give it to your agent

An agent that can run local commands can use Sweep directly. No MCP server is
required. Give it the [agent contract](docs/AGENTS.md), or add these instructions
to your project's agent guidance:

```text
Use Sweep for disk audits and developer-cache cleanup.
Read docs/AGENTS.md from the Sweep repository for its command/JSON contract.
Use --json and require schema_version 1. Start with read-only commands.
Show exact paths, safety labels, and rebuild costs before requesting approval.
Treat scan totals and review-only entries as information, not deletion permission.
Never execute cleanup without approval for its scope. Never use --permanent,
--force, or bin --execute without explicit approval for that action.
Report errors, skipped items, and observed space changes from the receipt.
```

Start with a scan and a scoped preview:

```powershell
sweep scan D:\Dev --top 20 --json
sweep detectors --roots D:\Dev --json
sweep clean --roots D:\Dev --id nextjs-build --id flutter-build --json
```

`--roots` controls project-artifact searches; global cache detectors still use
their configured cache locations. Choose detector IDs deliberately when previewing
or cleaning. See the [detector reference](docs/DETECTORS.md) for IDs and cache-location
environment variables.

**A separate execution command rescans.** Sweep does not currently accept a saved
reviewed plan as input. Interactive `clean --execute` displays a freshly built plan,
asks for confirmation, then executes that in-memory plan. An agent using
`--execute --json --yes` receives no new interactive confirmation; a prior dry-run
file is not bound to that execution. Stop builds/installers, keep the approved
scope narrow, and use interactive confirmation when the fresh exact plan must be
reviewed. See [safety details](docs/SAFETY.md).

## What it recognizes

| Ecosystem | Examples |
| --- | --- |
| Rust | Cargo registry downloads and project `target/` |
| JavaScript / TypeScript | npm, pnpm, Yarn, Next.js, Vite, Angular |
| Flutter / Dart | `build/`, `.dart_tool/`, pub package cache |
| Python | pip, uv, source-backed bytecode, test and lint caches |
| Go | Build and module caches |
| Android / JVM / .NET | SDK versions, Gradle caches, project output |
| Other disk consumers | Docker disks, browser caches, agent artifacts, Windows update files |

Recognition does not mean automatic deletion. Gradle, uv, Docker disks, .NET
output, and Windows update findings require manual action; other items may require
`CAUTION` opt-in. The [20-detector registry](docs/DETECTORS.md) documents each case.
Unknown large folders remain review-only.

## Measured comparison

One paired run on Windows 11 with **PowerShell 7.6.7** and **Sweep 0.3.1**, using
equivalent isolated copies of real Flutter, Rust, Next.js, npm, and Dart artifacts:

| Operation | PowerShell 7 | Sweep |
| --- | ---: | ---: |
| Scan 167,684 files | 5.43 s | 0.66 s |
| Permanently clean 31.43 GB and verify protected files | 25.43 s | 9.76 s |

Both scans returned the same file count and logical byte total. Both cleanups
removed the same 14 approved paths, preserved all 19 protected files, and reported
zero errors. PowerShell received the approved path list; this compares command
execution, not agent reasoning. Runs were sequential, PowerShell first, so cache
state and other system activity may influence results. These are single-run
observations, not a general speed guarantee.

[Methodology, exact timings, and sanitized results](docs/BENCHMARKS.md).

## Usage

```sh
sweep scan <PATH> [--top 30] [--min-size 1MB]   # what's big under PATH
sweep scan <PATH> --allocated                    # slower on-disk file allocation estimate
sweep review <PATH> [--top 20] [--min-size 500MB] # large unclassified paths; review only
sweep detectors [--roots DIR...]                # known caches + build junk
sweep clean                                     # dry-run plan (default: safe items, to trash)
sweep clean --execute                           # ask, then delete to Recycle Bin
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

## Roadmap

- Interactive terminal UI
- winget / scoop packages
- Signed Windows builds

Past releases: [CHANGELOG.md](CHANGELOG.md).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Bug reports with `sweep scan --json` output attached are gold.

## License

MIT — [LICENSE-MIT](LICENSE-MIT).
