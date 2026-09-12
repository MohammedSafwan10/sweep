# Contributing to sweep

## Setup

Rust 1.88 or newer is required by the locked dependency set. CI checks this minimum.

```sh
git clone https://github.com/MohammedSafwan10/sweep
cd sweep
cargo build --workspace
```

## Gates (CI runs all three on Windows + Ubuntu)

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Adding a detector

1. New file `crates/sweep-core/src/detectors/<name>.rs` implementing the
   `Detector` trait (`id`, `label`, `scan(&Ctx) -> Vec<Finding>`).
2. Register it in `all_detectors()` in `detectors/mod.rs`.
3. Resolve paths through `Ctx` (never ambient env) so tests can inject a
   fake home via `Ctx::for_tests`.
4. Safety labeling: when in doubt, pick the stricter label. Unknown directory
   names inside a cache root must never be flagged.
5. Add unit tests with `tempfile` fixtures + one row in `docs/DETECTORS.md`.

## JSON stability

`--json` output is a contract with agents (see `docs/AGENTS.md`):
only **add** fields or enum values, never rename/remove, until a major
version. Bump `SCHEMA_VERSION` on any breaking change.

## Style

- No `unwrap()` on user-input paths in non-test code; collect into warnings/errors.
- Hot loop (scanner): no clones, no channel spam, no syscalls beyond metadata.
- Keep dependencies minimal; justify every new crate in the PR.
