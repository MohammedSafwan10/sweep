# Windows cleanup comparison

Measured on 23 October 2026 using Sweep 0.3.1 and PowerShell 7.6.7 on Windows 11
Pro, Intel Core i7-12700F (12 cores / 20 logical processors). Both copies lived on
the same D: volume. This is one paired local run, not a cold-cache benchmark suite.

These are historical **v0.3.1** results. v0.4.0 adds full-content fingerprints
before review and again before deletion; the cleanup timings below do not include
that additional validation and must not be quoted as current-version performance.

## Dataset

Two isolated copies of existing Flutter `build/` and `.dart_tool/`, Rust `target/`,
Next.js `.next/`, npm `_cacache`, Dart hosted packages, and Cargo registry sources.
Source projects and original caches were only read. Nothing was inflated with
dummy files. Separate generated-fixture tests are not included in these numbers.

Each copy contained **167,684 files / 31,429,822,393 logical bytes** before cleanup.
The approved set was **14 paths / 31,429,738,876 logical bytes**. Nineteen protected
files outside those paths included project markers, copied source files, and planted
unknown-data, dependency, and npm-tool sentinels. Published paths use project aliases.

## Commands

Both workflows received the same approved paths. The native baseline directly
deleted that list, rather than using an artificial per-file loop. Cache-location
environment variables and project roots pointed Sweep at each isolated copy.

```powershell
# PowerShell 7 scan
Get-ChildItem -LiteralPath $fixture -Recurse -File -Force |
  Measure-Object -Property Length -Sum

# Sweep scan
sweep scan $fixture --top 12 --json

# Sweep preview, with cache locations redirected into the copy
sweep clean --roots $projects --no-docker --id cargo --id nextjs-build `
  --id flutter-build --id js-caches --id pub-cache --json

# Native deletion; $approved is the checked path array inside its copy
Remove-Item -LiteralPath $approved -Recurse -Force

# Approved disposable copy only; permanent mode in both workflows
sweep clean --roots $projects --no-docker --id cargo --id nextjs-build `
  --id flutter-build --id js-caches --id pub-cache `
  --execute --permanent --yes --json
```

Sweep's `clean` rescans; it does not consume a saved plan. The harness checked
previews against a fixed approved relative-path list and checked the execution
receipt against that same set. Every target was checked to remain inside the
isolated fixture root. Original project/cache paths were never execution targets.

## Results

| Measurement | PowerShell 7.6.7 | Sweep 0.3.1 |
| --- | ---: | ---: |
| Scan wall time | 5.425550 s | 0.657945 s |
| Cleanup wall time | 25.424587 s | 9.752752 s |
| Post-clean verification | 0.003466 s | 0.003075 s |
| Cleanup + verification | 25.428053 s | 9.755827 s |
| Approved paths absent | 14 | 14 |
| Protected file hashes unchanged | 19 | 19 |
| Reported operation errors | 0 | 0 |

[Sanitized measured results](benchmarks/windows-real-artifacts.json).

Wall times include process startup and JSON/output handling. PowerShell's fresh
process also loaded the baseline script and checked approved targets. Sweep cleanup
includes detection, planning, deletion, and its receipt. Verification separately
checks selected paths are absent and all protected SHA-256 hashes match.

## Limits

- Runs were sequential, PowerShell first. Cache warming and background activity
  can affect the second run. No cold-cache reset or repeated real-artifact trials
  were performed.
- The native scan sums file lengths; Sweep also ranks directories. Their report
  formats and work are not identical.
- PowerShell received the approved path list. This is not a measurement of AI
  reasoning, script authoring, cache recognition, or response latency.
- Preservation checks cover only the copied files in this test. They do not
  establish that all build directories on all machines are disposable.
- Removed bytes are logical file lengths. Available-space deltas can differ due
  to allocation and unrelated drive activity.
- Both cleanups permanently deleted approved copies. This is not Sweep's default,
  and is not a permanent-vs-Recycle-Bin comparison.

## Video

The [40-second demo](assets/sweep-demo.mp4) reconstructs terminal output and agent
narration from the command results. It is an edited walkthrough, not an unedited
live AI-agent recording. Accelerated intervals are labeled `TIME COMPRESSED`; the
counter reaches the measured elapsed duration. Clocks exclude model reasoning.
Command excerpts are abbreviated; the full scoped commands are above. Original
projects remained untouched, and both temporary artifact copies were cleaned.
