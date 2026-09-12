# DETECTORS.md — the v1 registry

`detector_id` values are stable and selectable via `sweep clean --id <id>`.

| id | Finds | Safety | Notes |
| -- | ----- | ------ | ----- |
| `cargo` | `registry/src`, `registry/cache` under `CARGO_HOME`; per-project `target/` next to `Cargo.toml` | SAFE | Redownloaded/rebuilt on demand. |
| `pub-cache` | `hosted/`, `git/`, `_temp/` under `PUB_CACHE` (`%LOCALAPPDATA%\Pub\Cache`) | SAFE | `flutter pub get` restores. |
| `js-caches` | npm cache, pnpm store, Yarn Cache subdirectory | SAFE | `pnpm store prune` equivalent included in effect. |
| `gradle` | Superseded `wrapper/dists/gradle-*-all` (newest kept); stale `caches/<ver>`; `modules-2` | SAFE / CAUTION (`modules-2`) | Strict version parsing keeps both bin/all variants of the newest version; unknown dir names are never flagged. |
| `android-sdk` | Per-version `ndk/`, `build-tools/`, `platforms/`, `cmake/`, `sources/` | CAUTION | sweep never guesses which version is "current" — you pick. |
| `flutter-build` | `build/`, `.dart_tool/` next to `pubspec.yaml` | SAFE | `flutter clean` equivalent. |
| `nextjs-build` | `.next/` (SAFE) + `node_modules/` (CAUTION) in projects depending on `next` | mixed | `package.json` is parsed; non-Next projects ignored. |
| `docker` | Docker Desktop data vhdx size | CAUTION + manual | Never auto-deleted; instructions printed. Volumes may hold DB data. |
| `temp` | Top-5 entries in `%TEMP%` whose entire readable tree is older than 7 days and at least 1 MiB | CAUTION | Fresh descendants and linked trees are excluded; checked again before deletion. |

Project-artifact detectors (`cargo`, `flutter-build`, `nextjs-build`) walk `--roots`
(default: current directory), prune known output dirs while walking, and stop at
500 projects. Overlapping markers (e.g. a Flutter project inside a Cargo workspace)
simply produce one finding per artifact dir.

`GRADLE_USER_HOME`, `YARN_CACHE_FOLDER`, and `npm_config_cache` override their default cache locations. Docker detection reads disk metadata only; it does not launch the Docker CLI. Docker bytes are logical disk size, not an estimate of reclaimable space.
