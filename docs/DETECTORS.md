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
| `pip-cache` | Recognized `http/`, `http-v2/`, `wheels/` under the configured cache | SAFE | Unknown layouts and sibling files are preserved; this is not a full `pip cache purge`. |
| `uv-cache` | uv cache size | CAUTION + manual | Direct edits unsafe per uv docs; use `uv cache prune`/`clean`. Never auto-deleted. |
| `pycache` | `__pycache__/` containing only recognized, source-backed `.pyc` files | SAFE | Unknown files, subdirectories, links and missing sources disqualify the directory. Sources are checked again before deletion. |
| `pytest-caches` | Test/lint caches next to Python markers; SQLite coverage data and Hypothesis examples | SAFE / CAUTION + manual | Coverage data and Hypothesis examples are report-only; unknown `.coverage.*` files are ignored. |
| `vite-build` | `node_modules/.vite/` in projects depending on `vite` | SAFE | `package.json` parsed; others ignored. |
| `angular-build` | `.angular/cache/` in projects depending on `@angular/cli`/`core` | SAFE | Other `.angular` contents are preserved. Custom cache paths are not detected. |
| `dotnet-build` | `bin/`, `obj/` next to `*.csproj`/`*.fsproj`/`*.vbproj` | CAUTION + manual | Never deletes whole output directories: they can contain application data. Solution-only roots are ignored. Review and use `dotnet clean`. |

Project-artifact detectors (`cargo`, `flutter-build`, `nextjs-build`,
`vite-build`, `angular-build`, `dotnet-build`, `pycache`, `pytest-caches`) walk `--roots`
(default: current directory), prune known output dirs while walking, and stop at
500 projects (200 findings for the Python/.NET walkers). Overlapping markers (e.g. a Flutter project inside a Cargo workspace)
simply produce one finding per artifact dir.

`GRADLE_USER_HOME`, `YARN_CACHE_FOLDER`, and `npm_config_cache` override their default cache locations. Docker detection reads disk metadata only; it does not launch the Docker CLI. Docker bytes are logical disk size, not an estimate of reclaimable space.

Python walkers preserve directories containing `pyvenv.cfg`, including custom-named virtual environments. Python/.NET findings are deduplicated across overlapping roots. `PIP_CACHE_DIR` and `UV_CACHE_DIR` override defaults; Unix cache defaults respect absolute `XDG_CACHE_HOME` paths. Config files and custom framework output paths are not evaluated.

The narrow Angular target follows the [Angular cache documentation](https://angular.dev/cli/cache). Whole pip-root deletion is not equivalent to [pip cache purge](https://pip.pypa.io/en/stable/topics/caching/). For .NET, [dotnet clean](https://learn.microsoft.com/en-us/dotnet/core/tools/dotnet-clean) removes tracked build outputs rather than arbitrary application data in the output directories.
