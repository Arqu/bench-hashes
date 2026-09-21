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

**Sizes.** Twenty: 64 B to 8 MiB by powers of two, plus 3 KiB and 3 MiB.
Sixteen through 1 MiB came first; 2, 4, and 8 MiB were added to show
the plateau (every single-threaded contender is flat from 1 MiB; the
multithreaded ones take longer to level out and 8 MiB is past the
last-level cache on every target). 3 KiB and 3 MiB are non-power-of-two
trees, one at the SIMD ramp and one at the plateau; a contender whose
work splitting assumes powers of two shows it there (and one did). Round
counts are `lcm(sizes, orders)`, and twenty shares a factor with every
order count from two to eight; nineteen would have been prime and
multiplied run time by up to seven. Add sizes in multiples that keep
that property.

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
single-threaded ones included, so columns compare. The report shows
solo and duo side by side in every cell; the graph draws duo as a
dashed line with hollow dots.

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

- **Reading the environment.** Both fork contenders honour environment
  variables (e.g. lane count overrides). The harness inherits the user's
  environment. A report should record any `BLAKE3_*` variables present;
  it doesn't yet.

- **Solo-tuned defaults.** A contender may be tuned for the solo
  columns. The duo columns exist to catch exactly this. Both are shown
  so the reader sees the trade, but the summary "best per family" logic
  still uses solo medians only. Deciding how duo should weigh into
  "best" is open.

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

- **Best-per-family under duo.** `choose_best_per_family` ignores
  duo. Options: Pareto over (solo, duo) pairs; or report two bests.

- **Apple `--trace-clocks` under duo** is refused (two threads, one
  trace). Per-copy traces would be useful and aren't hard.

- **Noise floor.** `!` marks cells whose 95% median interval exceeds
  5%. On the 2-CPU VM about 15–25% of duo cells are marked; on the M4
  Max with 16 cores, about 2%. If VM results are ever quoted, quote the
  M4 Max instead.

## Running it

    cargo run --release -- --all --thorough            # solo, ~35 s on the VM
    cargo run --release -- --duo --all --thorough      # solo + duo, ~60 s
    cargo run --release -- --contenders blake3,blake3-servil-mt --duo

Results: `benchmark-results/{CPU}.{OS}/bench-hashes.result.txt` and
`.graph.svg`; duo runs write `bench-hashes.duo.*` beside them. On the
VM prefix `HOME=/tmp/home CC=clang-19 TMPDIR=/tmp CARGO_TARGET_DIR=/tmp/target`
(see `AGENTS.md`). On macOS, none of that.

The fork is a path dependency at `../BLAKE3`. To pin the benchmark to a
specific fork commit for a publication, note the commit the provenance
line prints; to compare two fork versions, check out each in `../BLAKE3`
and run twice — the provenance line in each report names which.
