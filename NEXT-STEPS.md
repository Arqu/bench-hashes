# Next steps (written 2026-09-22, before a VM restart)

## Layout after the move

- `bench-hashes` lives nested inside the fork checkout at `/workspace/bench-hashes`
  (host: `.../blake3-servil/BLAKE3/bench-hashes`). Its `Cargo.toml` and
  `build.rs` point the `blake3_sme2` path dependency at `..`; upstream
  bench-hashes expects a sibling `../BLAKE3`. `build.rs` skips untracked
  directories when fingerprinting the fork's tree.
- Run: `HOME=/tmp/home CARGO_TARGET_DIR=/tmp/target CC=clang-19 TMPDIR=/tmp cargo run --release --manifest-path /workspace/bench-hashes/Cargo.toml -- --contenders blake3,blake3-servil`
- After a restart recreate: `/tmp/home` (`.gitconfig` with `safe.directory = *`,
  `user.name John Servil`), `/tmp/home/bin/gh-cred.sh` speaking the git
  credential protocol (`username=johnservil` / `password=<token>` on `get`),
  `credential.helper` on both repos, `clang-19` from apt.llvm.org.

## Done and pushed

- bench-hashes@main `e002ace`: duo-only by default, `--solo` opt-in; graph
  polish (quiet bands, no rings, shaped aligned legend with tooltips, two-way
  hover highlight, folding provenance, 1300 px canvas); servil contenders
  always available; Contenders note names the `--contenders` keys.
- BLAKE3@sme2-bench `20d0521`: AGENTS.md section "Interfaces: fewest new
  concepts".

## Open, in priority order

1. **Accurate kernel reporting.** `Algorithm::mode()` and
   `detect_blake3_sme2_implementation()` hardcode "SME2". Read
   `blake3_sme2::platform::Platform::detect()` at run time and describe the
   NEON path (hybrid kernels, no SME2 regime) when that ran. Drop the
   "fails stop at run time (CPU lacks SME2)" line from `bench-hashes/AGENTS.md`;
   the fork selects NEON without SME2 and that is a real result.
2. **Vocabulary: implementation / kernel / mode.** Implementation = upstream
   crate vs servil fork (what `--list` and `--contenders` select). Kernel = the
   per-size code path, chosen at run time. Mode = single-threaded /
   multithreaded / capped. Availability never touches `--list`; the library
   reports no machine capacity to the caller.
3. **Stop consuming capacity information.** Remove the benchmarker's use of
   `lanes::lane_count`, `lanes::describe_lanes`, `BLAKE3_LANES`, and
   `lanes::MIN_SPLIT_LEN` (derive the split regime boundary another way or
   drop it).
4. **Fork API rework (proposal, unimplemented).** Replace the `lanes` module's
   public surface with `hash`, `hash_multithreaded`, and
   `hash_multithreaded_with_budget(input, max_threads)`; no lanes, admission,
   or cluster concepts in public docs. Same hash from every entry point.
5. **Performance question.** Under duo, servil mt is slower than serial servil
   at every size >= 128 KiB (M4 Max: 1 MiB 0.223 vs 0.185 ns/B, 8 MiB 0.207 vs
   0.181). Solo it is ~2x faster. Likely causes: `ceil(L / callers)` share
   oversubscribes odd lane counts (3 lanes, 2 callers -> 4 claims), and
   `MIN_SPLIT_LEN` was tuned solo. Measure as its own experiment.
6. Token in `ghtokenclassic.txt` was echoed once into tool output by a
   mis-speaking credential helper; consider rotating it.
