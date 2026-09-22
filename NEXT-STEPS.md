# Next steps

Read this file first in a new session; both `AGENTS.md` files point here.
Layout and environment (where the repos are, `HOME`, credentials,
`clang-19`) are in the Environment section of either `AGENTS.md`; after a
VM restart run `sh /workspace/vm/setup.sh`.

## Where the last session stopped (VM now has 16 vCPUs)

The fork's multithreaded path is rebuilt (commits `b3b4bc8`..`3f28ff4` on
`sme2-bench`, tests green: 54 lib + 15 doc; `--features no_sme2`: 53;
nothing pushed yet).
Design and measurements: `/workspace/NOTES-sme2-bench.md`, section "The
pool". In one paragraph: every CPU hashes; a call cuts its input into
subtree pieces that shrink toward the end (8–128 KiB) and registers a
job in a lock-free slot table; `cpus - 1` resident workers pull pieces
from the registered jobs round-robin; a piece runs on SME2 while one of
the machine's SME units is free (a permit per unit) and on the NEON
hybrids otherwise; workers sleep only after 200 µs without a registered
job; the pool starts from a background thread so the first call costs a
call.

bench-hashes duo on the VM (`--all`; mt·1 from a separate run):

    size      servil   servil mt      size      servil   servil mt
    64 KiB    0.216    0.194          1 MiB     0.190    0.078
    128 KiB   0.213    0.134          2 MiB     0.187    0.068
    256 KiB   0.207    0.114          3 MiB     0.181    0.065
    512 KiB   0.200    0.091          8 MiB     0.184    0.056

servil mt wins every cell from 64 KiB, with the bands apart; mt·1 tracks
servil. Below 64 KiB the call is the serial one.

**The next thing to do is run this on the M4 Max** and commit the
results under `benchmark-results/AppleM4Max.darwin25/`. What to look at
there (details and the numbers behind each in the NOTES):

1. 64 KiB, where the VM margin is smallest: the first call of each
   1 ms batch pays one `notify_all` of the sleeping workers (VM: 80–100
   µs on the caller; macOS unknown). If it hurts, the options measured
   and rejected on the VM (cascades, delegated wake, longer spin) are in
   the NOTES with their numbers; a native machine may rank them
   differently, in particular a longer `SPIN_BEFORE_SLEEP` costs nothing
   there but power.
2. Three SME permits vs none: on the VM they tie, because a piece's
   SME2 ↔ NEON round trip costs ~4 µs there; the M4 pays ~1 µs and its
   SME2 is 1.5–2× NEON per thread, so permits should show a gain at
   ≥ 512 KiB. `examples/duo.rs` with a permit override would tell; the
   override was removed before the commit, re-add it locally.
3. E-cores: the shrinking pieces are the answer to a slow thread's last
   piece; `examples/duo.rs` prints solo and duo by size.
4. The 8 MiB duo cell against Rayon's 0.088.

Harness note, for the bencher's maintainers: calibration's first probe
of a size runs one iteration; a contender whose *first ever* call is
slow (a pool starting) gets a 1-iteration batch for that size and every
sample of the cell is then a cold call. The fork now starts its pool in
the background so its first call is ordinary; a warm-up call per
contender before calibration would make the harness robust to any
contender's start (Rayon's global pool starts the same way).

Scratch examples in the fork (`cargo run --release --example NAME`):
`duo` (serial / mt duo / mt solo by size), `interleave` (the bencher's
pattern: 1 ms mt batches with 4 ms of other work between), `wake`
(condvar wake cost), `transition` (SME2 ↔ NEON round-trip cost),
`probe` / `probe3` (SME2 vs NEON rates, pairs and N threads), `probe4`
(per-piece cost by piece length), `probe2` (NEON rate by group size),
`rate` (`hash()` by size).

Run everything with a timeout (`timeout 2400 cargo run ...`): a hang
in the pool during development once ran for hours unnoticed.

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
