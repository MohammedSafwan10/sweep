# Changelog

## v0.5.0 — 2026-10-26

- Full-content fingerprints now use a shared pool of at most eight workers,
  deterministic per-entry SHA-256 digests, streamed reads, and fewer metadata/
  identity opens. Windows identity probes request metadata access rather than
  content access. Files are checked through their read handles and reopened names;
  directories are revalidated after the full content pass.
- Cleanup prints stage/fingerprint progress for humans; `--progress` enables stderr
  progress alongside JSON. Clean JSON adds stage timings and executed receipts add
  separate recheck/deletion worker timings.
- Saved-plan schema is now **2**. v0.4.0 plans must be recreated and reviewed; they
  cannot silently execute with the new fingerprint format. Receipt schema stays 1.
- Added deterministic-worker, progress, changed-file, and old-plan regressions,
  plus a read-only profiling example and local performance methodology.

## v0.4.0 — 2026-10-23

- `clean --save-plan FILE` writes a checksummed, fingerprinted preview without
  overwriting an existing file. `clean --from-plan FILE` reuses only its reviewed
  targets, without detector rediscovery. CLI receipts echo the saved `plan_id`.
- All cleanup plans now snapshot full file contents, entry metadata, and filesystem
  identities before review. Each target is rechecked immediately before deletion;
  changed or unreadable targets are refused whole, including after interactive
  confirmation. Other unchanged targets can proceed. This is not an atomic
  filesystem transaction; stop writers before cleaning.
- Saved-plan execution preserves deletion-mode and safety gates. Plans inside
  deletion targets, malformed/checksum-modified plans, and plans missing snapshots
  are refused. Plan files are trusted local input, not signed authorization tokens.
- Full-content validation adds I/O. The published v0.3.1 demo/benchmark predates it.

## v0.3.1 — 2026-09-26

- npm cleanup now removes only the regenerable `_cacache` download store. Running tools installed under `_npx` remain available; moving the whole npm cache could fail on Windows while those tools were active.

## v0.3.0 — 2026-09-26

- Scans now flag incomplete traversal, group access errors, show retry paths, and offer `--strict`. `--allocated` reports file allocation separately from logical sizes and drive capacity.
- Added read-only `review` for large paths outside known project artifact findings. Project detectors are the default for responsive reviews; `--all-detectors` includes global caches.
- Detector output ranks known cleanup candidates and notes rebuild costs. Executed clean receipts record available space before and after on affected volumes.
- Added `bin` on Windows: named drives, dry-run by default, explicit confirmation for permanent Recycle Bin emptying. `clean` never empties the bin automatically.
- Gradle version caches and distributions are now manual-only findings because version order cannot prove an active daemon has released them.

The new JSON fields are additive; `schema_version` remains 1. `review` and `bin` have their own JSON reports. Physical free-space changes can differ from logical byte counts, especially for hard links, sparse files, and Recycle Bin moves.

## v0.2.0 — 2026-09-15

- Parallel one-pass deletion, live cleanup progress, expanded detectors, and lock-aware uv cache reporting.
