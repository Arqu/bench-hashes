# Next steps

Read this file first in a new session; both `AGENTS.md` files point here.
Layout and environment (where the repos are, `HOME`, credentials,
`clang-19`) are in the Environment section of either `AGENTS.md`; after a
VM restart run `sh /workspace/vm/setup.sh`.

## The goal now: optimise BLAKE3 servil

Make the servil fork (`/workspace`, branch `sme2-bench`) score as high as
possible on this benchmark, alone and beside a copy of itself, at every
input size. Both repositories are in a settled state for that work:

- The bencher touches an implementation in three ways only: lists it,
  calls its plain entry point (`hash`, `hash_multithreaded`, or upstream's
  `Hasher::update_rayon` on Rayon's global pool) with no cap or pool of its
  own, and prints the fork's `kernel_report()`. Keep it that way; tune the
  fork, never the harness.
- Every run is a duo run (two copies at once, later finish scored);
  `--solo` adds the single-copy column beside it.
- `blake3-servil-mt1` (`hash_multithreaded_with_budget(input, 1)`) is a
  sanity check and tracks `blake3-servil` within noise on the VM; run it
  again after any change to the multithreaded path.

Run: `HOME=/workspace/vm/home CARGO_TARGET_DIR=/tmp/target CC=clang-19 TMPDIR=/tmp cargo run --release --manifest-path /workspace/bench-hashes/Cargo.toml -- --all --solo`

### Where the time goes (from the last full runs; see `benchmark-results/`)

1. **Multithreaded beside a copy of itself.** Under duo, servil mt was
   slower than serial servil at every size >= 128 KiB (M4 Max: 1 MiB 0.223
   vs 0.185 ns/B; 8 MiB 0.207 vs 0.181). Solo it is ~2x faster. Suspects:
   the fair share `ceil(L / callers)` oversubscribes odd lane counts
   (3 lanes, 2 callers -> 4 claims); `MIN_SPLIT_LEN` (128 KiB) and
   `MIN_BALANCED_PIECE_LEN` were tuned solo; the admission wait. Measure as
   an experiment first (`hash_multithreaded_with_budget` gives cheap caps
   to compare against), then change one thing at a time. Note that BLAKE3
   mt's duo numbers changed meaning in `5d2fed9` (one shared Rayon pool
   now), so re-run before comparing against old reports.
2. **Small inputs (64 B – 1 KiB).** The scalar c1 kernel runs the whole
   input in one call; the remaining cost is per-call overhead. Compare
   against upstream's numbers at 64–512 B and against SHA-256's hardware
   path, which wins below ~3 KiB.
3. **2–15 chunks.** Hybrid kernels; the crossover with SHA-256 sits near
   3 KiB. A partial-group SME2 kernel for 3–15 chunks was never tried
   (streaming-mode entry, ~0.5 µs, is the cost to beat).
4. **Bulk (>= 16 KiB, SME2 groups).** `DEGREE = 128` amortises the
   streaming-mode switch; the plateau is ~0.17 ns/B on the VM. Check the
   parent-level path and the remainder handling below a group.
5. **E-cluster weight (Apple).** `deal_to_lanes` gives every lane equal
   bytes; an E-core SME unit is slower. Needs an Apple machine to measure.

The fork's own notes for maintainers are `/workspace/NOTES-sme2-bench.md`
(design, measurements behind each change, open questions). Commit
messages on `sme2-bench` carry the numbers behind each change; keep
doing that.

### Method reminders

- Change the fork, rebuild the bencher (path dependency at `..`), run,
  compare medians; the bands are 95% intervals of the median, so a
  difference inside overlapping bands is nothing.
- The VM has two cores and SME2; Apple hardware differs in absolute
  numbers and in lane count (M4 Max: 3 clusters). Relative comparisons on
  the VM hold; anything about lane counts above two needs Apple.
- Keep `cargo test --release` green in the fork (unit + doc tests);
  `lanes::test` covers split shapes, merges, admission arithmetic, and
  that every budget gives `hash()`'s result.

## Housekeeping

- The token in `ghtokenclassic.txt` was echoed once into tool output by an
  earlier credential helper; consider rotating it.
- `benchmark-results/AppleM4Max.darwin25/` holds an untracked M4 Max run
  from before the vocabulary and pool changes; commit or delete after the
  next run there.
