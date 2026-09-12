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
| `pip-cache` | pip HTTP/wheel cache | SAFE | Equals `pip cache purge`; re-downloaded on demand. |
| `uv-cache` | uv cache size | CAUTION + manual | Direct edits unsafe per uv docs; use `uv cache prune`/`clean`. Never auto-deleted. |
| `pycache` | `__pycache__/` where every `.pyc`/`.pyo` has its source | SAFE | One source-less file disqualifies the whole dir. |
| `pytest-caches` | `.pytest_cache`, `.mypy_cache`, `.ruff_cache`, `.hypothesis`, `.coverage*` next to Python markers | SAFE | Marker-validated; stray lookalikes ignored. |
| `vite-build` | `node_modules/.vite/` in projects depending on `vite` | SAFE | `package.json` parsed; others ignored. |
| `angular-build` | `.angular/` in projects depending on `@angular/cli`/`core` | SAFE | `ng build` restores. |
| `dotnet-build` | `bin/`, `obj/` next to `*.csproj`/`*.fsproj`/`*.vbproj`/`*.sln` | SAFE | `dotnet build` restores; bare `bin/` dirs ignored. |

Project-artifact detectors (`cargo`, `flutter-build`, `nextjs-build`,
`vite-build`, `angular-build`, `dotnet-build`, `pycache`, `pytest-caches`) walk `--roots`
(default: current directory), prune known output dirs while walking, and stop at
500 projects (200 findings for the Python/.NET walkers). Overlapping markers (e.g. a Flutter project inside a Cargo workspace)
simply produce one finding per artifact dir.

`GRADLE_USER_HOME`, `YARN_CACHE_FOLDER`, and `npm_config_cache` override their default cache locations. Docker detection reads disk metadata only; it does not launch the Docker CLI. Docker bytes are logical disk size, not an estimate of reclaimable space.
