# Next steps

Read this file first in a new session; both `AGENTS.md` files point here.
Layout and environment (where the repos are, `HOME`, credentials,
`clang-19`) are in the Environment section of either `AGENTS.md`; after a
VM restart run `sh /workspace/vm/setup.sh`.

## Where the last session stopped (VM restart pending for more cores)

Uncommitted in the fork, all tests green (`cargo test --release --lib`: 56;
with `--features no_sme2`: 55):

- `src/ffi_neon_hybrid.rs`: `GROUP = 16`; chunk plans for 13–16 inputs lead
  with k10 (`[10,3] [10,4] [10,5] [10,6]`), parent plan 16 = `[8,8]`; the
  count test runs 1..=16.
- `src/platform.rs`: `Platform::NEON.simd_degree()` is 16 when the SHA-3
  extension is present (`MAX_SIMD_DEGREE` 16 under `blake3_neon_hybrid`);
  the hash_many name says "integer + NEON hybrid kernels (16 chunks per
  call)".
- `src/lib.rs`: the kernel report's NEON sentence says sixteen per call.

Why: raw kernel rates on the VM (`examples/probe2.rs`), 1 MiB of chunks
through `Platform::NEON.hash_many` at a fixed group size: k4 0.289 ns/B,
k8 0.255, k9 0.228, **k10 0.212**, k15 0.239, 16 = 15 + 1: 0.265. The tree
walk hands the NEON platform its degree at a time, so degree 4 kept it on
k4. Still to do: measure whole-hash `blake3_servil::hash` on the NEON
platform before and after (`cargo run --release --features no_sme2
--example rate`); after the change it reads 0.242 ns/B at 64 KiB–8 MiB.
The "before" run was lost to a `git stash` without `HOME=/workspace/vm/home`.
Then commit with the numbers.

Scratch helpers, uncommitted: `examples/probe.rs` (SME2 vs NEON rates,
alone and in pairs), `examples/probe2.rs` (NEON rate by group size),
`examples/rate.rs` (`hash()` rate by size on the detected platform).

Finding that shapes the mt plan (`examples/probe.rs`, VM): two threads
SME2 + NEON run at almost full speed each (0.164 / 0.302 ns/B), while
SME2 + SME2 sometimes collide (0.23 each, when the host puts both vCPUs on
one cluster). The SME unit is per cluster; the NEON units are per core.

### Plan for the multithreaded path

On the M4 Max, servil mt uses 3 lanes (one SME2 thread per cluster) and
BLAKE3 mt uses 16 NEON cores; that is why Rayon wins from 1 MiB up. The
plan is to use every core, each with the kernel it can run at full speed:

1. One pool of `available_parallelism() - 1` resident workers. A worker
   takes a piece only while hashing threads (callers inside a call plus
   workers with a piece) number fewer than the CPUs, so two callers on a
   two-CPU machine run serial, and never three threads on two CPUs.
2. Each call registers a job (its pieces, an atomic cursor, a done count)
   in the pool's active list; workers serve jobs round-robin, one piece
   at a time via `fetch_add` on the cursor; the caller takes pieces from
   its own job and waits for `done`. This replaces the lanes/callers
   admission word, the fair share, and the 20 ms wait: fairness comes
   from the round-robin, balance from the dynamic pull.
3. `lane_count()` SME2 permits (an atomic count). A thread takes a permit
   before each piece if one is free and hashes with `Platform::SME2`;
   otherwise with `Platform::NEON` (degree 16 → k10, ~0.21–0.29 ns/B).
   Needs a way to hash a piece with an explicit platform (`Hasher` takes
   its platform from `Platform::detect()` today; add a crate-internal
   constructor).
4. Pieces: subtrees from `split_subtrees`, target size around 128 KiB
   (one SME2 streaming entry) for bulk; many pieces let slow threads
   (E cores) take fewer. Tail is one piece on the slowest thread; if that
   shows on the M4, cut the last pieces finer.
5. `hash_multithreaded_with_budget(input, n)`: a per-job cap on threads
   with a piece in hand.

Expected on the M4 Max under duo at 8 MiB: 3 SME lanes (~5.5 GB/s each)
plus 13 NEON cores (~3.4 GB/s each) ≈ 50 GB/s for two copies, against
Rayon's 0.088 ns/B (≈ 23 GB/s). Even at half efficiency it wins.

The VM cannot show most of this (two CPUs, both SME2 lanes); it checks
correctness, per-piece overhead, and the two-callers-serial case. Once the
VM has more cores, `nproc` and a spin test (N busy processes, wall time
against N) tell whether they are real; with two CPUs the time doubled
exactly from 2 to 4 to 8 processes.

## The goal now: optimise BLAKE3 servil for the duo score

Make the servil fork (`/workspace`, branch `sme2-bench`) score as high as
possible on this benchmark **under duo** at every input size. Duo is the
only score: two copies of the contender run at once on two threads, and
the later finish is the sample. There is no single-copy target; `--solo`
is a diagnostic column, and no effort goes toward looking good in it.

Overfitting: avoid tuning to the exact structure of the M4 Max MacBook Pro
the results come from. Anything that behaves reasonably across machines
is fair, for example:

1. Fixed heuristics ("spawn N threads"), accepted as roughly right on many
   platforms.
2. Inspecting the machine once at first use (syscalls, topology, a timing
   probe), caching the answer, and acting on it.

The bencher does not charge a one-time inspection, and cannot without
becoming a different benchmark: calibration runs every contender at every
size to pick iteration counts before the first timed sample, so a probe
at first use (and the worker pool's start) happens there. Were something
to land inside the measured phase anyway, it would be one ~1 ms sample
among 80+ per cell and the median would drop it. The fork's current Linux
lane probe (~30 ms) is hidden this way. The separate warm-up phase was
redundant with calibration and is gone (`bench-hashes` after `cb022f1`).
Cold-start cost is therefore invisible here; a cold-process benchmark
would be the tool for it.

Both repositories are in a settled state for the work:

- The bencher touches an implementation in three ways only: lists it,
  calls its plain entry point (`hash`, `hash_multithreaded`, or upstream's
  `Hasher::update_rayon` on Rayon's global pool) with no cap or pool of its
  own, and prints the fork's `kernel_report()`. Keep it that way; tune the
  fork, never the harness.
- `blake3-servil-mt1` (`hash_multithreaded_with_budget(input, 1)`) is a
  sanity check and tracks `blake3-servil` within noise on the VM; run it
  again after any change to the multithreaded path.

Run: `HOME=/workspace/vm/home CARGO_TARGET_DIR=/tmp/target CC=clang-19 TMPDIR=/tmp cargo run --release --manifest-path /workspace/bench-hashes/Cargo.toml -- --all`

### What winning means

1. **Breadth before margin.** Beating a competitor at a size where servil
   mt currently loses is worth more than widening a lead at a size where it
   already wins. Competitors are every other column: other hash functions,
   the crates.io implementation, and the single-threaded servil call.
2. **The best result** is BLAKE3 servil mt measurably and reliably better
   (non-overlapping 95% bands) than every alternative at as many sizes as
   possible. Below about 3 KiB, SHA-256's hardware path is out of reach;
   accept that and win everywhere else.
3. **Portability over this machine.** The code will run on other systems.
   Avoid strategies that fit this M4 Max and would likely carry a strong
   penalty elsewhere (a fixed cluster layout, a lane count, a probe result
   assumed rather than measured). Heuristics that are roughly right
   anywhere, or a one-time inspection with the answer cached, are fine.

### Baseline (M4 Max, fork `0220be3`, bencher `f81859a`; results committed in `benchmark-results/AppleM4Max.darwin25/`)

Duo medians, ns/B:

    size      BLAKE3  SHA-256  servil  BLAKE3 mt  servil mt
    64 KiB    0.381   0.344    0.208   0.939      0.208
    128 KiB   0.382   0.345    0.202   0.566      0.367
    256 KiB   0.382   0.345    0.192   0.372      0.289
    512 KiB   0.381   0.344    0.184   0.258      0.246
    1 MiB     0.379   0.344    0.184   0.186      0.220
    2 MiB     0.383   0.346    0.182   0.140      0.216
    4 MiB     0.381   0.343    0.179   0.108      0.196
    8 MiB     0.382   0.341    0.180   0.088      0.190

Reading it against "what winning means":

- **servil mt loses to single-threaded servil at every size from 128 KiB
  up** (0.367 vs 0.202 at 128 KiB; 0.190 vs 0.180 at 8 MiB). The
  multithreaded call is a net loss under duo today. The first job is to
  find out why: suspects are the fair share `ceil(L / callers)` on three
  lanes (2 callers -> 4 claims), `MIN_SPLIT_LEN` and
  `MIN_BALANCED_PIECE_LEN` tuned solo, the 20 ms admission wait, and the
  hand-off cost at 128–512 KiB. `hash_multithreaded_with_budget` gives
  cheap caps to compare against; the two-core VM reproduces the shape
  of the loss but Apple hardware has the three lanes.
- **BLAKE3 mt (Rayon's global pool shared by the two copies) beats
  servil mt from 1 MiB up, by more than 2x at 8 MiB.** Work-stealing over
  every CPU, shared by two callers, wins at bulk sizes on this machine;
  whatever servil mt does must at least match it there.
- **servil (single-threaded) beats everything from 3 KiB to 512 KiB.**
  Below 3 KiB SHA-256 wins, as expected.

So the sizes to win, in order of value: 128 KiB–8 MiB for servil mt
(currently lost to servil itself, and to BLAKE3 mt at >= 1 MiB), then
the small end where per-call overhead sets the floor (64 B–1 KiB run at
~0.68 ns/B on the VM against SHA-256's hardware path).

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
