# Next steps

Read this file first. The work is optimizing the servil fork for the duo
benchmark on the VM, native Mac, and other platforms. Prefer improvements
that make the implementation simpler and faster together. Shared principles
and environment commands are in both repositories' `AGENTS.md` files.

## Where this session stopped

- Fork: `/workspace`, branch `sme2-bench`, commit **604abc4**, clean, pushed.
- Benchmark: `/workspace/bench-hashes`, branch `main`, commit **2c5f272**,
  tag **`v0.6.0+4693c2fe…`**, clean, pushed.
- Latest Mac record: `benchmark-results/AppleM4Max.darwin25/bench-hashes.duo.*`,
  **2026-09-22 17:18:13 UTC**, `--all`, provenance clean on both commits.
- VM: 16 vCPUs; run `sh /workspace/vm/setup.sh` after a restart.

## What the last session did

A review of both repositories for bugs, security, and stale material.

Fork:

1. **`initialize()` is public and its cost is the contract.** It creates
   the pool synchronously: on Linux it measures the SME unit count (about
   40 ms on the VM, every CPU busy; Apple reads `sysctl`), then spawns the
   workers. The first multithreaded call that leaves its thread does the
   same when the program has yet to call it; the docs say "up to tens of
   milliseconds". The starter thread and the permits-grow-later state are
   gone. `measure_sme_units` uses `REPEAT = 20` (was 40, 78 ms); six runs
   gave the same unit count.
2. Sleeper bookkeeping (`sleepers`, `notified`) is exact under
   `sleep_lock`; `notified` can no longer stay stale after a sleeper takes
   a piece without waiting. `jobs_recently` saturates instead of wrapping
   (a debug-build panic in a worker under a clock/store race).
3. `test_vectors/` and `b3sum/` build again: their manifests name
   `package = "blake3-servil"`. The official published vectors pass:
   `cargo test --release --manifest-path test_vectors/Cargo.toml`.
4. `build.rs` gates the SME2 kernel on `CARGO_CFG_TARGET_VENDOR`/`_OS`,
   matching `platform.rs` and `Cargo.toml` (an `aarch64-linux-android`
   build would have failed to compile).
5. Metadata points at the fork; the README opens with a note on what the
   branch is; `NOTES-sme2-bench.md` records the start-up contract and the
   in-tree vector harness.

Benchmark:

6. Every run is duo; `--solo` adds the solo column. The code has one
   `solo` flag (the always-true `duo` option, `duo_only()`, the `--duo`
   flag, and the `roster.duo = solo` reuse are gone). README and AGENTS
   describe the current design, including that cycle normalisation applies
   to `--solo` samples alone and that this repository lives inside the fork
   checkout at `..`.
7. **Touch screens.** The graph separates hover from tap: a mouse hovering
   a dot shows the panel and leaving hides it; a tap pins the panel, and a
   second tap or the background clears it. Name highlighting follows the
   mouse only. `:hover` styles sit under `@media (hover: hover)`.

Reviewed and found sound: the SIMD merge (3000 random lengths against the
recursive oracle), the slot-reader/active-count lifetime argument, the
reservation cap, the wait/notify handshakes, the SME2 assembly's mode
switching and register saving, the golden-vector generator.

Left alone by choice: a worker panic inside `hash_piece` hangs its caller
(a contract violation; DBC says no defensive code); `blake3_sme2_*` symbol
prefixes; the credential helper in `vm/` answers every host (scope it to
`github.com` if that ever matters).

## Latest Mac record

`--all` at fork 604abc4, the first native run of the simplified pool.
Servil mt duo medians, 64 KiB through 8 MiB in the usual nine-size order:
`.193, .124, .101, .078, .063, .054, .052, .050, .048` ns/B. The 15:21
record at fork `2ce77d7`/dirty was `.203, .134, .107, .078, .063, .055,
.053, .051, .049`: 64–256 KiB improved 5–6%, bulk is level. The three
marked cells belong to Rayon. Servil mt's 64 KiB range is `.160–.327`,
wider than its neighbours; that cell and bulk latency remain the targets.

## Next priorities

1. **Bulk latency** (2–8 MiB): profile stragglers and wake costs before
   adding mechanisms; keep comparisons interleaved ABBA between baseline
   and candidate builds, and inspect bands before attributing differences.
2. **The 64 KiB tail.** The caller waits for the last pieces; it could take
   the smallest pieces itself, or the cut could end finer.
3. **Incremental `Hasher` over the pool** and **SME2 for 2–15 chunks**.

## Commands

From `/workspace` in the VM (every `git`/`cargo` command takes this `HOME`):

`HOME=/workspace/vm/home CARGO_TARGET_DIR=/tmp/target CC=clang-19 TMPDIR=/tmp cargo test --release`

`HOME=/workspace/vm/home CARGO_TARGET_DIR=/tmp/target CC=clang-19 TMPDIR=/tmp cargo test --release --features no_sme2`

`HOME=/workspace/vm/home CARGO_TARGET_DIR=/tmp/target CC=clang-19 TMPDIR=/tmp cargo test --release --features pure`

`HOME=/workspace/vm/home CARGO_TARGET_DIR=/tmp/target CC=clang-19 TMPDIR=/tmp cargo test --release --manifest-path /workspace/test_vectors/Cargo.toml`

`HOME=/workspace/vm/home CARGO_TARGET_DIR=/tmp/target CC=clang-19 TMPDIR=/tmp cargo test --release --manifest-path /workspace/bench-hashes/Cargo.toml`

Benchmark runs write `benchmark-results/` relative to the **current
directory**; run them from `/workspace/bench-hashes` so results land in
the repository:

`cd /workspace/bench-hashes && HOME=/workspace/vm/home CARGO_TARGET_DIR=/tmp/target CC=clang-19 TMPDIR=/tmp cargo run --release -- --all`

On the Mac, from `bench-hashes`: `cargo run --release -- --all` (add
`--thorough` for narrower bands). Results overwrite that machine's files;
copy a baseline aside before a comparison. Commit before publishing so the
provenance reads `clean`.

Release: `python3 tools/gen-ver.py X.Y.Z` from a clean tree makes two
version commits and a lightweight tag `vX.Y.Z+<commit>`; push with
`git push origin main` and then the tag by name (`--follow-tags` skips
lightweight tags).

Expected suites: fork 56 library + 16 doc tests (`no_sme2` 55 + 16,
`pure` 46 + 16); official vectors 2; benchmark 3.

Use no timeout for long commands; let progress stream. Never sleep in
commands. If a network operation fails, report it and stop; the user
chooses retries. Never print the credential token. Only `/workspace`
survives VM restarts.
