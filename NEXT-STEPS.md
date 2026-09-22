# Next steps (written 2026-09-22, before a VM restart)

## Layout after the move

- `bench-hashes` lives nested inside the fork checkout at `/workspace/bench-hashes`
  (host: `.../blake3-servil/BLAKE3/bench-hashes`). Its `Cargo.toml` and
  `build.rs` point the `blake3_sme2` path dependency at `..`; upstream
  bench-hashes expects a sibling `../BLAKE3`. `build.rs` skips untracked
  directories when fingerprinting the fork's tree.
- Run: `HOME=/workspace/vm/home CARGO_TARGET_DIR=/tmp/target CC=clang-19 TMPDIR=/tmp cargo run --release --manifest-path /workspace/bench-hashes/Cargo.toml -- --contenders blake3,blake3-servil`
- After a restart run `sh /workspace/vm/setup.sh` (installs `clang-19`, creates
  `/tmp/target`, re-points both credential helpers). `HOME`, the credential
  helper, and the git identity live in `/workspace/vm/home` and persist; both
  `AGENTS.md` files describe the layout.

## Done and pushed

- bench-hashes@main `e002ace`: duo-only by default, `--solo` opt-in; graph
  polish (quiet bands, no rings, shaped aligned legend with tooltips, two-way
  hover highlight, folding provenance, 1300 px canvas); servil contenders
  always available; Contenders note names the `--contenders` keys.
- BLAKE3@sme2-bench `20d0521`: AGENTS.md section "Interfaces: fewest new
  concepts".

## Done in the working tree (2026-09-22, after the restart; uncommitted)

1. **Accurate kernel reporting.** `detect_blake3_servil_kernels` reads
   `Platform::detect()` at run time: SME2 adds the group kernel; NEON reports
   the scalar + hybrid kernels alone. The text report prints every
   contender's kernel(s) with the platform; AGENTS.md and README drop the
   run-time SME2 assertion wording.
2. **Vocabulary.** Enum variants `Blake3Servil` / `Blake3ServilMt`;
   `Kernels`/`Kernel` replace `Implementation`/`Regime`; `mode()` says
   single-/multithreaded only; availability is a platform property (the
   Rayon CPU-count check is gone). AGENTS.md states the vocabulary.
3. **No capacity consumption.** `describe_lanes`, `lane_count`,
   `MIN_SPLIT_LEN`, `BLAKE3_LANES` are gone from the benchmarker; the 128 KiB
   threshold is `SERVIL_MULTITHREADED_FROM`, read from the fork's docs.
4. **Fork API.** Crate renamed `blake3-servil` (lib `blake3_servil`).
   `lanes` is private; public: `hash`, `hash_multithreaded`,
   `hash_multithreaded_with_budget(input, max_threads)` (asserts
   `max_threads >= 1`; 1 is the serial path). `BLAKE3_LANES` and
   `describe_lanes` removed. Upstream doctests fixed to the crate name;
   `cargo test` is green (55 + 15).

## Open, in priority order

1. **Performance question.** Under duo, servil mt is slower than serial servil
   at every size >= 128 KiB (M4 Max: 1 MiB 0.223 vs 0.185 ns/B, 8 MiB 0.207 vs
   0.181). Solo it is ~2x faster. Likely causes: `ceil(L / callers)` share
   oversubscribes odd lane counts (3 lanes, 2 callers -> 4 claims), and
   `MIN_SPLIT_LEN` was tuned solo. Measure as its own experiment;
   `hash_multithreaded_with_budget` now gives a cheap way to test caps.
2. Token in `ghtokenclassic.txt` was echoed once into tool output by a
   mis-speaking credential helper; consider rotating it.
