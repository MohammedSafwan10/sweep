# Changelog

## v0.3.0 — 2026-09-26

- Scans now flag incomplete traversal, group access errors, show retry paths, and offer `--strict`. `--allocated` reports file allocation separately from logical sizes and drive capacity.
- Added read-only `review` for large paths outside known project artifact findings. Project detectors are the default for responsive reviews; `--all-detectors` includes global caches.
- Detector output ranks known cleanup candidates and notes rebuild costs. Executed clean receipts record available space before and after on affected volumes.
- Added `bin` on Windows: named drives, dry-run by default, explicit confirmation for permanent Recycle Bin emptying. `clean` never empties the bin automatically.
- Gradle version caches and distributions are now manual-only findings because version order cannot prove an active daemon has released them.

The new JSON fields are additive; `schema_version` remains 1. `review` and `bin` have their own JSON reports. Physical free-space changes can differ from logical byte counts, especially for hard links, sparse files, and Recycle Bin moves.

## v0.2.0 — 2026-09-15

- Parallel one-pass deletion, live cleanup progress, expanded detectors, and lock-aware uv cache reporting.
