# ADR-0002: Caching the DuckDB build

- **Status:** Accepted (2026-11-02)
- **Context owner:** Engineering (platform)
- **Related:** ADR-0001 (embedded DuckDB, bundled into the app)

## Problem

`vda-core` builds DuckDB from source (`duckdb` crate, `bundled` + `parquet` features). On a 2-core machine a cold build takes **about 23 minutes**, almost all of it C++ compilation inside the `libduckdb-sys` build script. Observed so far:

- The first build pays it.
- Changing DuckDB features (adding `parquet`) paid it again.
- Running `cargo clippy` after `cargo build` paid it again, because the build script re-ran under a different build configuration.
- Every fresh CI runner, worktree or `cargo clean` will pay it again.

Ordinary edits to `vda-core` do **not** rebuild DuckDB; incremental work is fast. The cost is in everything that resets or changes the build configuration.

## Decision

Cache at two layers, keep the cached artifacts small, and pin DuckDB.

### 1. sccache for the C++ objects (everywhere)

Use [sccache](https://github.com/mozilla/sccache) as `RUSTC_WRAPPER` on developer machines and in CI. The `cc` crate (1.11, used by `libduckdb-sys` with parallel compilation) detects `RUSTC_WRAPPER=sccache` and routes **each C++ object file** through it. That makes every re-run of the DuckDB build script (clippy, profile changes, `cargo clean`) a cache hit, without depending on Cargo's own fingerprints.

**Path normalization is required for other checkouts.** The DuckDB build script unpacks DuckDB's sources inside the build directory, so the absolute paths in every compile command differ between checkouts. Without normalization, building in a new worktree or clone at another path missed almost the whole cache (measured: about 300 of the C++ compilations missed). Set `SCCACHE_BASEDIRS` to the checkout roots (colon-separated; `;` on Windows), or `basedirs = [...]` in `~/.config/sccache/config`. The sccache server reads it when it starts, so restart it after a change (`sccache --stop-server`).

- Local: disk backend, default `~/.cache/sccache`; set `SCCACHE_CACHE_SIZE=20G` (debug and release objects for each platform add up).
- CI: GitHub Actions cache backend via `mozilla-actions/sccache-action` with `SCCACHE_GHA_ENABLED=true`, and `SCCACHE_BASEDIRS` set to the workspace so cache entries don't depend on the runner's checkout path.
- `CARGO_INCREMENTAL=0` in CI: sccache cannot cache incremental Rust builds, and CI gains nothing from incremental compilation.

### 2. Target-directory cache in CI

Use `Swatinem/rust-cache@v2`. It restores the whole compiled dependency tree, keyed on `Cargo.lock`, the toolchain and the job, so on a hit the DuckDB build script **doesn't run at all**. sccache is the fallback when this key misses (lockfile change, new toolchain).

- Save caches only from `main` (`save-if`), so pull requests read the warm cache without filling the 10 GB repository quota with near-duplicates.
- Each OS and each profile (test vs. release) is its own cache entry.

### 3. Pin DuckDB exactly and upgrade deliberately

Keep `duckdb = "=1.20000.1"` (exact). A DuckDB upgrade is its own pull request: it pays the cold build once, and merging it warms the cache for everyone. Never bump DuckDB inside an unrelated change.

### 4. No debug info for dependencies

The workspace `Cargo.toml` sets `[profile.dev.package."*"] debug = false`. Cargo passes this to DuckDB's build script, so the C++ objects are compiled without debug symbols. Our own crates keep full debug info. Debugging inside DuckDB's C++ is rare; when needed, override it in an untracked `.cargo/config.toml` with `[profile.dev.package.libduckdb-sys] debug = true`.

Without this, one debug build of DuckDB took 4–5 GB, the build directory reached 27 GB across a few configurations and filled the disk, and a single OS's target cache would take a large share of GitHub's 10 GB-per-repository cache quota.

## Measured (2026-11-02, 2-core Linux machine, sccache 0.19.0 with local disk cache)

| Step | Result |
|---|---|
| Cold build of tests and examples, no debug info for dependencies, empty cache | **1,007 s** (about 17 min; 385 C/C++ cache misses). Before the debug-info change: about 23 min. |
| Size of the build directory afterwards | **3.4 GB** total, of which DuckDB's build output 1.4 GB (previously 4–5 GB for DuckDB alone) |
| `cargo clippy --all-targets -- -D warnings` right after | **17 s**, clean (previously re-ran the full DuckDB build) |
| Delete DuckDB's build output (`cargo clean -p libduckdb-sys`), rebuild with warm sccache | **38 s**, 369 of 369 C/C++ compilations served from cache |
| sccache disk cache size | 1.3 GB (holds objects from both debug-info settings) |
| Fresh clone at a different path, no `SCCACHE_BASEDIRS` | about 300 C/C++ cache misses: close to a cold build |
| Two clones at different paths, both listed in `SCCACHE_BASEDIRS`: first build, then second clone | first 945 s (cold); **second 53 s, 385 of 385 C/C++ compilations from cache** |

## Expected effect in CI

| Situation | Without caching | With this ADR |
|---|---|---|
| Edit `vda-core`, rebuild | seconds | seconds (unchanged) |
| `cargo clippy` after `cargo build` | ~23 min | seconds (measured: 17 s) |
| CI run, lockfile unchanged | ~23 min per OS | rust-cache hit: DuckDB not rebuilt |
| CI run after a lockfile change (not DuckDB) | ~17–23 min | rust-cache miss, sccache hit on C++ objects (locally: under a minute) |
| DuckDB upgrade PR | ~23 min | ~23 min once, then warm |

The local measurements above support the sccache rows. The CI figures are still estimates until the workflow has run; check `sccache --show-stats` (printed in every job) and the size of each rust-cache entry on the first runs.

## Options not chosen (for now)

- **Link a prebuilt libduckdb for development** (`DUCKDB_LIB_DIR` / the crate's `download-lib` feature). Fastest dev builds, but development and release would link different DuckDB binaries (dynamic vs. static, different build flags), and the app would have to ship a shared library. Revisit only if sccache proves insufficient.
- **Committing a repository `.cargo/config.toml` with `rustc-wrapper = "sccache"`.** It would break builds for anyone without sccache installed. Developers opt in with an environment variable instead (README).
- **Remote shared sccache (S3/R2) for developers.** Useful once there is a team; the GitHub Actions backend is enough for CI today.
- **Splitting DuckDB into its own crate.** Doesn't help: Cargo already avoids rebuilding it on `vda-core` edits. The expensive cases are configuration resets, which caching addresses.

## Consequences

- CI depends on GitHub's cache service. A cache outage means slow builds, not failures.
- Developers who skip sccache still build correctly; only their cold builds are slow.
- When the Tauri app crate arrives it adds its own large dependency tree. The same two layers cover it with no further work.
