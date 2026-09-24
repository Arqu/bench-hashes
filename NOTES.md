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

**Sizes.** Twenty-seven: 64 B to 128 MiB by powers of two but 16 MiB,
plus 3 KiB and 3 MiB, plus four sizes of real data between 2 and 8 KiB,
chosen by use rather than by any implementation's structure: 2304 B (an
802.11 frame body at its maximum), 3839 and 7935 B (the 802.11n A-MSDU
maxima), 4470 B (the Packet over SONET/SDH MTU). None is a multiple of a
chunk, as real inputs seldom are; they showed a trailing partial chunk
costing the fork 7-65% until it ran beside the whole ones. Sixteen through 1 MiB came first; 2, 4, and 8 MiB were added
to show the plateau, then 16 to 128 MiB when the fork's multithreaded
rate was still climbing at 8 MiB (it levels from 4 MiB with the ranked
pool; Rayon's still falls at 128 MiB on the VM). 16 MiB was dropped
after both machines' records showed every contender's median there
within run-to-run noise of the value 8 and 32 MiB predict (log-log);
32, 64, and 128 MiB each carry something the others do not (Rayon's bend
on the VM, serial servil's rise at 128 MiB on the Mac). 3 KiB and 3 MiB are
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

**Round counts** are a plain 96 (24 with `--quick`). They used to
be the least multiple of the point count and the order count at or above
a target, so every order and starting point recurred equally often; that
made removing one point cost several times the run (47 points and 8
orders: 376 rounds). The imbalance a plain count leaves is a fraction of
a sample per cell.

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
every other equally often. The solo and shared samples of a batch are
taken back to back within one interval, so they share whatever the
machine was doing at that moment.

**Time basis.** Wall time, always. Cycle normalisation (cycles per byte
at the run's sustained clock) was used for solo samples on Apple until
September 2026 and removed: the core's cycle counter does not see time
spent waiting on the SME unit (a 25% slower SME2 batch cell read the
same cycles per message at a lower apparent clock), so normalising
hid real SME2 slowdowns, and would hide a GPU's the same way; and a
contender's own power draw throttling the clock is a cost its user pays.

**Scenarios.** Every run measures solo (one copy on one thread) and
shared (two independent copies on two persistent threads, each over its
own input of the size, with different contents so they share no cache
lines, released together). Each shared copy's own time is a sample, two
per interval; the later finish, used until September 2026, measured how
unevenly two copies are served rather than what each user gets.

**Two speeds.** A cell whose samples split at a gap of 4% of the median
or more, a tenth or more on each side, with the sides' medians 1.25× or
more apart, has two speeds; every report shows both with equal weight
(text `a|b`, a forked line in the graph).
The 1.25× floor: on the VM the machine's own noise puts a tenth to a
third of many cells' samples 10–14% slow for every contender alike, which
drew a second line nearly everywhere at 4% alone; SME2 unit sharing splits
1.7–2.0×. Open: `perf_regress` judges a cell by its 5th percentile, the
faster speed alone; a regression confined to the slower speed passes it.

**Full by default, `--quick` on request** (September 2026, for people
who run it once and publish what they get). A full run: every point, 96
rounds, the long-cell budget, SHA-1DC in `--all`: 60 s on the VM for the
default roster, about 140 s for `--all`. `--quick` stops below 1 MiB
and 10,000 messages, 24 rounds, SHA-1DC only when named: about 12 s, and
it may misread a cell.

**The default roster is fixed** (September 2026): BLAKE3 servil, servil
mt, sha2, and ring. It replaced a run that measured every contender and
kept the Pareto-best per family, which cost `--all`'s time and printed
whatever the measurement chose. SHA-256 keeps two crates because
neither dominates on either machine: sha2 leads at 64 B (0.50 against
0.65 ns/B) and 128 B and in every batch of 64-byte messages (30 against
40 ns/msg), ring from 256 B (0.30 against 0.35 ns/B from 1 KiB). Whether
ring leads on x86 is unmeasured (it has SHA-NI, AVX, and SSSE3 paths,
sha2 SHA-NI and portable code). The graph opens showing servil mt, both
SHA-256 crates, and crates.io BLAKE3 (`SHOWN_AT_FIRST`); a run without
any of them shows everything.

**Load from other programs** (September 25, 2026, after a Mac record
read 8-10% slow across every contender with dips in the batch plot, most
likely from the user's own work on the Mac): at round boundaries the run
reads the machine's busy CPU time (`/proc/stat`, macOS
`host_statistics(HOST_CPU_LOAD_INFO)`) and its own process CPU time; the
difference over 5 s windows (10 ms ticks make shorter ones noisy) is other
programs' load, in milli-CPUs. Busy: the busiest window at one CPU or
more (first 0.5; the Mac's steady desktop-plus-VM background of 0.40-0.56
CPUs, job 114, flagged a run whose medians matched e16e836's to 1%). Provenance, text report, and samples (`# load:`, `# other load by
5 s window`, `# steal by 5 s window`) carry it. VM quiet 0.02 CPUs
average, 0.07 worst; two `yes` loops read 2.04. **Open**: this VM's
hypervisor reports no steal (0 since boot through Mac jobs), so host load
stays invisible in the guest; a reference loop timed beside the samples
would see it.

**Graph labels** (September 2026, after overlaps in the published
graph): right-hand names stack 34 px apart, so eight fit inside a plot;
x labels place the powers of two first, then the sizes between, on two
rows, a second-row tick never crossing a first-row label; value labels
go on columns at least 110 px apart with 32 px free on each side (none
in the crowded 2-8 KiB stretch until the zoom spreads it), and step
clear of every shown dot in their column. The static render uses the
contenders shown at first for its axes and labels, as the script does.

**Checks compare round by round.** Judging each cell by its slower speed
(30f6776) compared unlike moments: on the Mac a tenth of the solo samples
from 256 B to 8 KiB ran 1.65-3.3x slow for every contender (every fourth
round, when the full run's long all-core cells are sampled; likely an
efficiency core), and a cell that happened to split was compared at its
slow speed with a neighbour that had not: servil "x4.70 slower than SHA-256
ring" at 256 B. Every comparison now pairs the samples of one round (each
cell records its rounds), judges the ratio's worse speed where the ratios
split, and needs it 5% above 1 with its 95% interval above 1. A moment
that slows both sides cancels; a slowdown of one side counts.

**Checks.** The report's CHECKS section lists what a regression hunter
looks for, for the servil contenders: slower than another contender at a
point (servil against single-threaded contenders, servil mt against all,
servil mt against servil), and slower per unit at a point N than at a
smaller point M dividing N (M's work N / M times would have been
faster); 5% or more, intervals apart; identical claims merge across the
two contenders and scenarios; worst first. "Divides" matters: 3 messages
slower per message than 2 is no defect, 128 slower than 64 is.

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

- **Noise floor.** `~` marks cells whose 95% median interval is at least
  5% of its median. Keep VM and native results separate: both are target
  deployments. Narrow within-run bands still allow between-run drift;
  alternate baseline and candidate builds when assessing small gains.

## Running it

    cargo run --release -- --all
    cargo run --release -- --quick --all
    cargo run --release -- --contenders blake3,blake3-servil-mt

Results are `benchmark-results/{CPU}.{OS}/bench-hashes.result.txt`,
`.graph.svg`, and `.samples.tsv`. In the VM prefix commands with
`HOME=/workspace/vm/home CC=clang-19 TMPDIR=/tmp CARGO_TARGET_DIR=/tmp/target`.
On macOS these environment overrides are unnecessary.

The fork is a git dependency at the commit `Cargo.lock` pins; with
`--config 'patch."https://github.com/johnservil/BLAKE3".blake3-servil.path=".."'`
it is the enclosing checkout, and its provenance records that
checkout's commit and working-tree fingerprint. Keep that provenance
with each measurement.
