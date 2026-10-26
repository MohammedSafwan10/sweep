# Cleanup fingerprint performance

Measured locally on 26 October 2026, Windows 11 Pro, Intel Core i7-12700F.
These measurements compare the v0.4.0 and v0.5.0 **snapshot implementations**,
not PowerShell, scan speed, total cleanup time, or reclaimed disk space.

## Repeated measurements

Three sequential paired rounds, baseline first in rounds 1/3 and candidate first
in round 2. Caches were warm; no cache flush was attempted. Normal local system
activity was uncontrolled. Values below are medians, excluding process startup.
Both versions counted the same entries and logical bytes in every completed run.

| Dataset | Entries | Logical bytes | v0.4.0 | v0.5.0 | Ratio |
| --- | ---: | ---: | ---: | ---: | ---: |
| 4,000 small files in 40 directories, D: | 4,041 | 16,384,000 | 0.283 s | 0.082 s | 3.46x |
| Eight large files, D: | 9 | 268,435,456 | 0.179 s | 0.029 s | 6.22x |
| Real Dart Pub hosted cache, C: | 87,499 | 1,078,336,677 | 22.195 s | 5.101 s | 4.35x |

[All 18 samples and methodology](benchmarks/snapshot-v050.json).

An earlier first-observation v0.4.0 Dart check took 129.91 s, compared with 5.26 s
in the first completed optimized check. That comparison has a much larger cache
state/order bias; the repeated warm-cache medians above are the main result.
An initial v0.4.0 Cargo-source check was cancelled before completion and is not
included as a numeric speed comparison. These are local observations, not general
speed guarantees or cold-cache measurements.

## What changed

Discovery of Dart/Cargo findings was already quick (about 2.3 s combined in the
initial diagnostic run). The full-content fingerprint pass caused the long wait.
v0.4.0 read files serially and separately reopened names for identity queries.
v0.5.0 uses a shared pool capped at eight workers, reuses the content handle for
identity/metadata queries, and requests only attribute access for identity probes
on Windows. It folds per-entry SHA-256 hashes in deterministic traversal order.
Contents are still read in full. Metadata/identity checks, no-follow opens, final
directory verification, and whole-target refusal remain part of the check.

The inventory uses memory proportional to entry count. Streams use 64 KiB buffers;
file contents are not buffered in full. Hash results are collected in bounded
512-entry batches. Large single files remain single streaming hashes.

## Reproduce

Build `cargo build --release --example profile_cleanup --locked` and run:

```powershell
target\release\examples\profile_cleanup.exe C:\path\to\cache
target\release\examples\profile_cleanup.exe --discover D:\projects
```

The example is read-only. Snapshot output includes elapsed seconds, entries,
bytes, and digest. For a v0.4.0 checkout, use the same timing wrapper around its
public `snapshot::capture(path)` API (it predates `capture_with_progress`). Build
both helpers optimized and keep their executables outside build output being
overwritten. `scripts/benchmark_snapshots.ps1` accepts two such JSON-emitting
profilers and alternates order over three rounds. Use identical unchanged paths.

The small fixture here had 40 directories with 100 files of 4,096 bytes each.
The large fixture had eight files of 33,554,432 bytes each. Both used deterministic
bytes; the Dart dataset was an existing real cache. The test did not delete them.

For real cleanup, JSON `timings` separates discovery, planning, and execution
wall time. `execution_timings` separates summed per-target recheck and deletion
times; parallel jobs overlap, so worker sums need not equal wall time. `--progress`
writes stage time, checked files, and read bytes to stderr without mixing output
into the JSON receipt. Progress counters describe work performed, not approval or
a promise of physical space reclaimed.
