# Notes for the benchmark maintainers

For whoever maintains `bench-hashes` next. This file is about measuring
fairly, accurately, and cheaply. It says nothing about how any contender
is built; the BLAKE3 fork's maintainers keep their own notes in their
own repository, and this benchmark treats every contender as a black
box behind a function call. Keep it that way: a benchmark that knows a
contender's internals starts to accommodate them.

`AGENTS.md` covers style and the VM environment. `README.md` is the
user-facing description. This file is the reasoning behind the design
and the list of ways it can still lie.

## What is settled

**Sizes.** Twenty-four: 64 B to 128 MiB by powers of two, plus 3 KiB
and 3 MiB. Sixteen through 1 MiB came first; 2, 4, and 8 MiB were added
to show the plateau, then 16 to 128 MiB when the fork's multithreaded
rate was still climbing at 8 MiB (it levels from 4 MiB with the ranked
pool; Rayon's still falls at 128 MiB on the VM). 3 KiB and 3 MiB are
non-power-of-two trees, one at the SIMD ramp and one at the plateau; a
contender whose work splitting assumes powers of two shows it there (and
one did).

**Batch sizes.** Twenty-four on a second axis: 1 to 262144 messages of
64 B, with 3, 6, 12, 24, 48 beside the powers of two to leave SIMD
groups partly filled. Samples on that axis divide by messages, so the
statistics pipeline is unchanged and only the unit names and the rate
scale (1 GB/s per ns/B; 1000 Mmsg/s per ns/msg) differ per plot. Its
golden anchors are the SHA-256 of a batch's digests concatenated, one
line per (batch size, seed).

**Round counts** are `lcm(points, orders)` over both axes' 48 points:
96 for two, three, four, six, or eight contenders. Add points in
multiples that keep that property.

**Inputs** are little-endian 64-bit counter words `seed << 48 | index`:
every block differs (a kernel mixing up lanes fails the golden digests),
Python's `array('Q')` builds them at C speed (the generator writes every
vector, 128 MiB included, in five seconds; the earlier byte-per-step
xorshift could not), and hash speed does not depend on the bytes. The
bootstrap resampler uses SplitMix64 with multiply-shift ranges.

**Time budget for long cells.** 78% of a full run went to 58 cells
whose single hash takes over 2 ms (SHA-1DC at 128 MiB, 170 ms a sample).
Sampling them in every fourth round alone moved noisy cells' medians by
up to 16% (Rayon at 16 MiB), so the budget adapts: from 4 ms a hash, every
fourth round, and every round while the median's 95% interval is wider
than 1%. Simulated on the recorded samples: every such median within
0.6% of the full one. Measured: 150 s -> 101 s a run; budgeted cells
agree with a full run to a median 0.68%, against 1.37% run-to-run for
cells sampled every round. Then a 2% target and at most every second
round for unsure long cells, and a 250 us calibration probe: 80 s a run,
medians at x0.9955 and x1.0072 of two full-sample runs, which differ from
each other by x0.9886. Rejected: all cells in every second round (a
bimodal cell's median moved 35%), adaptive sampling for short cells
(1 ms samples rarely reach a 1% interval, so little is saved), and 0.5 ms
samples (62 s, every median 1.6% slow).

**Interleaving.** Williams orders over the contenders, size order
rotated per round. Every contender takes every position and follows
every other equally often. Solo and duo samples of a batch are taken
back to back within one interval, so a duo/solo pair share whatever the
machine was doing at that moment.

**Time basis.** On Apple, solo samples are cycles per byte at the run's
sustained clock, which removes frequency excursions; samples whose cycle
count fell short of that rate (SME2 streaming mode counts at 70–75% of
core rate) keep measured time. Duo samples are always measured time:
two threads' cycle counters describe two threads and no one rate
normalises the later finish.

**Duo.** Two independent copies of a contender on two persistent
threads, each over its own input of the size (different contents, so
the copies share no cache lines), released together, scored by the
later finish per byte of one copy. Every contender in the run gets it,
single-threaded ones included, so columns compare. Every run measures duo. `--solo` also reports solo beside duo; in that
view the graph draws duo as a dashed line with hollow dots.

**Correctness.** Before calibration, selected contenders hash identical
inputs and assert equality with checked-in golden digests. The deterministic
RNG and both seeds are frozen by 64 vectors in `src/test_vectors.rs`.
Expected digests were established with the BLAKE3 reference implementation
and Python hashlib, independently of the optimized fork. Regeneration is
an explicit review step using `tools/gen-test-vectors.py`. Checks cover
both timed input sets, empty input and short boundary tails,
and two simultaneous calls to multithreaded entries. `hash_batch` contains
the one dispatch used by both checking and timing; a monomorphized callback
asserts digest equality or black-boxes the digest. Timed duo copies retain
their separate, differently seeded buffers. A failed check stops the run.

**Provenance.** The build script embeds the git state of this
repository and of the fork checkout (branch, commit, clean or a hash of
the diff). A report that says `dirty-…` measured uncommitted code.
Before publishing a result, commit first.

## Threats to validity, and what was done about each

These are the ways the benchmark has been wrong so far. Each was found
by a result that looked too neat.

1. **Waking a thread is not free.** The duo release was a
   `std::sync::Barrier`. On a 2-CPU VM, one copy woke 300 µs after the
   other because the caller's own thread had just used that CPU. The
   copies now poll an atomic generation counter and are already in the
   instruction stream when it flips; each reads the sample clock as its
   first act. Any future "release together" mechanism must not depend
   on the OS scheduler.

2. **A busy spin steals a scheduler quantum.** The first polling
   release used `spin_loop`. Two such spinners on a 2-CPU machine held
   both CPUs for ~2 ms each, and any contender with worker threads saw
   them start 2 ms late: a 29 µs hash measured 2 ms. Polls in the
   harness now `yield_now()` between checks. Rule: the harness must
   never hold a CPU it isn't measuring on.

3. **The caller must not be a third contender.** In duo, the main
   thread posts the job then *sleeps* on a condvar until both copies
   finish. If it spun, it would be a third thread competing for CPUs on
   a 2-CPU machine. Solo samples run on the main thread with the copy
   threads asleep.

4. **Calibration must match the measured shape.** Iterations per batch
   are calibrated solo (~1 ms). A duo batch of the same iterations takes
   at least as long, so it lands at or above the target; that is
   acceptable. Calibrating under duo would tie the batch size to the
   contender's contention behaviour.

5. **Two copies must hash different bytes.** `make_input_seeded(size,
   1)` for copy 1. Sharing one buffer lets two copies share L2 lines
   and understates memory cost.

6. **Stack frames count.** A 6 KiB frame inlined into `hash()` cost
   every 64-byte call a page probe. The harness is not immune: keep the
   timed region a straight line from `now()` to `since_ns()` around
   `run_batch`, with nothing allocated inside.

7. **Two-mode cells are real.** `find_modes` splits a cell whose
   samples cluster ≥4% apart with ≥10% on each side; the hover shows
   both. Don't "fix" this by taking more samples — it usually means the
   contender behaves two ways depending on what ran before it, which is
   information.

## Things a contender could do that this benchmark would reward unfairly

Watch for these when reading a result. None is currently detected
automatically.

- **Caching across calls.** Every batch hashes the same buffer
  `iterations` times. A contender that memoised on pointer+length would
  score infinitely well. Inputs are `black_box`ed but the bytes don't
  change. Defence if needed: rotate among several equal-size buffers
  within a batch, or perturb one byte per iteration outside the timed
  region. Not done yet because no contender does this and it costs
  cache locality that every contender would then pay.

- **Knowing the harness's thread count.** A multithreaded contender
  could detect "exactly two callers" and behave specially. The duo count
  is fixed at two; a `--trio` or `--n N` mode would make gaming it
  harder and is a natural next step (see below).

- **Persistent worker threads that stay hot.** A contender whose workers
  keep polling between calls looks better in a tight benchmark loop than
  in a program that hashes once a second. Spins that yield are fair to
  other threads; spins that don't are threat #2 from the contender's
  side. Consider a `--gap MS` option that sleeps between batches so
  workers must actually wake.

- **Reading the environment.** The fork once honoured a `BLAKE3_LANES`
  override; that is gone, and `hash_multithreaded_with_budget` is the
  way to cap threads. The fork reads no `BLAKE3_*` variables now, so the
  report has nothing to record there.

- **Solo-tuned defaults.** The default report uses duo medians, including
  "best per family" selection. With `--solo`, selection still uses the
  solo column; interpreting that diagnostic view needs care.

## Open questions and next steps

- **More than two copies.** Duo catches whole-machine pools but a
  contender sized to half the machine looks perfect under duo and bad
  under trio. `--copies N` generalising `Duo` is the obvious extension;
  `Duo` was written with two hardcoded, and `finished: [Option<u64>; 2]`
  is the main thing to generalise.

- **Cross-process contention.** Duo copies are threads in one process.
  A contender can coordinate across threads (shared counters) in ways it
  can't across processes. A `--duo-process` mode spawning a second
  `bench-hashes` would test the honest case. Measured by hand once: two
  processes of the fork's mt hash each ran at single-threaded speed,
  which is the right answer, but nothing automated checks it.

- **Idle between calls.** See "persistent worker threads" above.

- **Best-per-family with `--solo`.** Consider Pareto over both columns
  or reporting two bests. The default duo-only selection already uses duo.

- **Per-copy clock traces.** `--trace-clocks` requires `--solo` and records
  the solo sample. A trace for each duo copy would extend the diagnosis.

- **Noise floor.** `!` marks cells whose 95% median interval is at least
  5% of its median. Keep VM and native results separate: both are target
  deployments. Narrow within-run bands still allow between-run drift;
  alternate baseline and candidate builds when assessing small gains.

## Running it

    cargo run --release -- --all --thorough
    cargo run --release -- --all --solo
    cargo run --release -- --contenders blake3,blake3-servil-mt

Every run measures duo; `--solo` adds the diagnostic single-copy column.
Results are `benchmark-results/{CPU}.{OS}/bench-hashes.duo.result.txt`
and `.graph.svg`. In the VM prefix commands with
`HOME=/workspace/vm/home CC=clang-19 TMPDIR=/tmp CARGO_TARGET_DIR=/tmp/target`.
On macOS these environment overrides are unnecessary.

The fork is the path dependency `..`. Its provenance records the commit
and working-tree fingerprint. Keep that provenance with each measurement;
changing the fork rebuilds the benchmark against the new code.
