# How bench-hashes measures

This file explains what a run measures, how it keeps the numbers honest,
and what each contender runs. [README.md](README.md) says how to run it.

Every run measures each contender in two use cases and two scenarios.
**One message per call**: a call hashes one input, at twenty-seven sizes
from 64 B to 128 MiB, reported per byte. **Many messages per call**: a
call hashes a batch of 64-byte messages, at twenty-four batch sizes from
1 to 262144 messages, reported per message. **Solo**: one copy of the
contender, the machine otherwise idle. **Shared**: two copies at once.
The report and the graph show each use case once per scenario, solo
first.

## Contenders

A default run measures BLAKE3 servil, single-threaded and multithreaded,
and SHA-256 from two crates, sha2 and ring, since each is the faster
SHA-256 at some sizes. `--all` adds every other contender the machine can
run: the crates.io BLAKE3 crate, single-threaded and on its Rayon pool
(BLAKE3 mt), ab-blake3 (a crate with a `const fn` BLAKE3 and a batch
entry point for many 64-byte messages), and SHA-1DC (SHA-1 with the
collision detection git uses). `--contenders` names any set, including
CommonCrypto's SHA-256 on Apple platforms (`sha256-cc`), which runs
only when named: on Apple silicon the ring and sha2 crates are each
faster at every size. `--list` shows every key.

## Input sizes

The one-message axis tests every power-of-two input size from 64 B to
128 MiB but 16 MiB, plus 3 KiB and 3 MiB, plus four sizes of real data
between 2 and 8 KiB: 2304 B (an 802.11 frame body at its maximum),
3839 and 7935 B (the 802.11n A-MSDU maxima), and 4470 B (the Packet over
SONET/SDH MTU). None of those four is a multiple of BLAKE3's 1 KiB
chunk, as real inputs seldom are. 16 MiB was dropped: its neighbours
predict it to within run-to-run noise.

Below 1 KiB a BLAKE3 input is one chunk; from 2 KiB to 16 KiB
its SIMD paths fill (4-way NEON at 4 KiB, a sixteen-lane SME2 group at
16 KiB); above that the bulk rate settles. 3 KiB is where the SME2 fork's
integer + NEON hybrid kernels first overtake hardware SHA-256. The sizes
past 1 MiB show the plateau: a contender whose 32, 64, and 128 MiB
medians agree has levelled out. The multithreaded contenders take
longest to get there, since a pool hand-off or a subtree merge amortises
more slowly than one kernel call (the fork's was still climbing at
8 MiB, and Rayon's still is at 128 MiB in a Linux VM); everything from
8 MiB up is past the last-level cache on every machine this benchmark
targets. 3 MiB is to the plateau what 3 KiB is to the SIMD
ramp: a tree that is no power of two (a 2 MiB left subtree beside a
1 MiB right one), so a splitter that cuts at subtree boundaries hands
its threads unequal work there.

The report gives each cell's median time per unit (integer picoseconds
inside, nanoseconds on the page); lower is better. Every time is wall
time on the platform's hardware counter (`CLOCK_UPTIME_RAW` on Darwin,
`CLOCK_MONOTONIC` on Linux, via `std::time::Instant`), so a throttled
clock, a busy SME unit, or a GPU's latency counts as the user would
feel it.

## The many-messages use case

A program with a queue of small messages to hash (a Merkle tree's
leaves, a table of records) has two ways to spend a call: one message
per call of the plain entry point, or a batch per call where the
implementation offers that. The second use case measures both as they
are. A contender without a batch entry point loops its plain entry
point over the batch, one message per call: `for m in batch { hash(m) }`.
Three have one. ab-blake3's `single_block_hash_many_exact::<N>` takes N
messages of exactly one block (64 bytes) as one array and returns N
digests; the bencher calls it with N the batch size (N is a const
generic, so each batch size on the axis is its own call). BLAKE3
servil's `hash_many(&[&[u8]], &mut [Hash])` takes messages of any
lengths and fills one digest each; BLAKE3 servil mt's
`hash_many_multithreaded` does the same over the fork's worker threads. Messages are 64
bytes for every contender because that is the one size ab-blake3's
batch entry point accepts.

The axis counts messages per batch: 1, 2, 3, 4, 6, 8, 12, 16, 24, 32,
48, 64, 128, 256, 512, 1024, 2048, 4096, 8192, 16384, 32768, 65536,
131072, 262144 (16 MiB of input at the end). Powers of two up to 16 show a SIMD batch filling (the blake3
crate's `hash_many` takes four blocks at a time on NEON, sixteen with
AVX-512); 3, 6, 12, 24, and 48 leave a group partly filled or leave a
remainder past the sixteen-message groups ab-blake3 forms; from 64 up
the per-batch overhead amortises and the rate settles. Results read in
nanoseconds per message and million messages per second.

BLAKE3 mt takes no part in this use case: a 64-byte message is a call
to `update_rayon` that no program would make, and the crate has no
batch entry point. The bencher writes no wrapper of its own around any
contender; the contenders' own entry points are the whole of what it
calls.

Both scenarios apply unchanged: in the shared one each copy hashes its
own batch.

## Solo and shared

A reader of these results wants to compare contenders on a load pattern,
to spot a regression, or to estimate speed in a system they are
designing. Each needs two numbers per contender, so every sample
interval takes two samples of the same batch:

- **Solo**: one copy of the contender on one thread, the machine
  otherwise idle. What a program gets with the machine to itself.
- **Shared**: two independent copies at once, each on its own thread
  over its own input, released together; each copy's own time is a
  sample. What each of two users of the same code gets. They compete for
  every resource the code uses: cores and memory bandwidth, and for the
  SME2 fork an SME unit, which serves a whole cluster of cores.

A single-threaded hash costs about the same in both. A multithreaded one
shows in the shared scenario what its threads cost when the machine is
shared; an SME2 kernel shows what sharing its unit costs.

The report's CHECKS section lists, for the servil contenders, every cell
slower than another contender (single-threaded servil against the
single-threaded contenders, servil mt against all, and servil mt against
servil), and every larger point slower per unit than a smaller point that
divides it, which could have been done as that smaller work repeated.
Each comparison pairs the samples taken in the same round, back to back,
so a moment that slows both sides (an efficiency core, a lowered clock)
cancels out, and a slowdown of one side (two copies sharing an SME unit)
counts; where the round-by-round ratios split in two, the worse one is
judged. A finding needs that ratio 5% or more above 1, with its 95%
interval above 1; the worst come first.

## Hash implementations

The `BLAKE3` and `BLAKE3 servil` contenders call the one-shot `hash`
function, which is single-threaded on every platform.
BLAKE3 may still use SIMD parallelism within the calling thread; that
is single-threaded execution, not operating-system-level
multithreading.

The blake3 crate is built with its `rayon` feature so that the
`BLAKE3 mt` contender can call `Hasher::update_rayon`; that feature
adds the method and leaves `blake3::hash` and every other API
single-threaded.

BLAKE3 is provided by the blake3 crate through the one-shot
blake3::hash function, which is single-threaded (see "BLAKE3
threading").

ab-blake3 is the ab-blake3 crate (0.2), "optimized and more exotic APIs
around BLAKE3". For one message the bencher calls `const_hash`, a
`const fn` copy of the reference tree: portable compression at every
size with no run-time SIMD dispatch, so above one chunk it runs below
the crates.io crate. For a batch of 64-byte messages it calls
`single_block_hash_many_exact::<N>`, which hands each full group of
sixteen blocks to the blake3 crate's platform `hash_many` (the SIMD
path the BLAKE3 kernel table names) and compresses the blocks past the
last full group one at a time; below sixteen messages every block is
its own compression.

SHA-256 is provided by RustCrypto's sha2 crate (0.11), whose built-in
backends use the ARMv8 SHA-256 instructions on AArch64 and SHA-NI on
x86, selected at runtime; other targets use its portable code.

SHA-256 ring is provided by the ring crate: BoringSSL's assembly,
which interleaves the next block's message schedule with the current
block's rounds. That pipelining wins about 13% per byte over sha2's
straightforward per-block loop on Apple silicon, and costs a few
nanoseconds of setup that sha2 wins back on inputs of one or two
blocks. The two kernels are the two sides of one design trade-off, so
the crossover near 128–256 B is structural.

BLAKE3 servil is the same crate from the `servil` branch of
github.com/johnservil/BLAKE3, at the commit `Cargo.lock` pins, under
the crate name `blake3-servil` so it links beside the crates.io crate.
On AArch64 Linux and macOS the build includes the SME2 kernel when the
C compiler assembles SME2 (Clang 17 or later, Xcode 15 or later, or GCC
14 or later with binutils 2.41), and leaves it out with a warning
otherwise; only CPUs with SME2 run it. At run time the
fork reads the CPU: one that reports SME2 with a 512-bit streaming
vector length gets the SME2 group kernel for sixteen chunks and up;
every AArch64 core runs the scalar and integer + NEON hybrid kernels.
On other CPUs it runs the kernels of the crates.io crate it forks
(SSE4.1, AVX2, AVX-512 on x86). The report's kernel table names the
platform the run measured. Its provenance line gives the repository,
branch, and commit instead of a registry checksum. For a batch the fork's `hash_many`
compresses runs of one-block messages many lanes at a time on the same
kernels its tree uses for parent nodes (sixteen per group on SME2, the
NEON hybrids below a group), and `kernel_report_many()` describes that
by batch size.

BLAKE3 mt is the crates.io crate's own multithreading, called as a
program calls it by default: `Hasher::new().update_rayon(input)` on
Rayon's global pool, which Rayon sizes to one thread per logical CPU.
The method splits the tree recursively with `rayon::join` down to the
SIMD degree, so any input above one SIMD width of chunks may cross
threads, and idle pool threads steal the halves. The two shared copies are
two callers in one process sharing that one pool, the same situation
the servil fork's fair sharing addresses, so the two multithreaded
columns compare like for like.

BLAKE3 servil mt is the fork's `blake3_servil::hash_multithreaded`,
which returns the same hash as `blake3_servil::hash`. Inputs below
the threshold shown in the kernel table stay on the calling thread.
Larger inputs can split at subtree
boundaries across the calling thread and worker threads the fork starts
once per process and keeps; the caller merges the chaining values. How
many threads a call uses is the fork's decision from the input and the
machine, and concurrent callers in one process share the workers
fairly: two callers at once each get about half the machine. Across
processes the operating system's scheduler shares the workers' CPUs.
The fork also offers `hash_multithreaded_with_budget(input,
max_threads)` to cap one call's threads; the contender measures the
uncapped call.

The benchmark touches each implementation in three ways only: it lists
it, it calls its single-threaded (`hash`, `const_hash`), multithreaded
(`hash_multithreaded`, `Hasher::update_rayon`), or batch
(`single_block_hash_many_exact`, `hash_many`, `hash_many_multithreaded`)
entry point with no cap or pool of its own, and it asks the servil fork
to describe its kernels (`kernel_report()` and its `_many` and
`_multithreaded` forms). It asks for no machine capacity, sets no
environment, and checks returned digests through those same entry points
before timing. Implementation-specific tests remain in each crate.

SHA-256 CommonCrypto, on Apple platforms only, calls the system's
libSystem through FFI using `CC_SHA256_Init`, `CC_SHA256_Update`, and
`CC_SHA256_Final`. This is the implementation most Apple software
reaches for, so it anchors the sha2 crate's number against the
platform's own. Its provenance is the running OS rather than a crate
version.

The three-call form is the fastest route into corecrypto. Measured on
an M4 Max, a 64-byte digest takes 51 ns through Init/Update/Final and
182 ns through the one-shot `CC_SHA256()`, whose finalisation spends
about 110 ns per compression; bulk throughput is identical on both.
Callers hashing small inputs through CommonCrypto gain most from the
streaming calls.

SHA-1DC is provided by RustCrypto's sha1-checked crate: SHA-1 with the
collision-detection pass that git applies to every object hash. The
detection is pure Rust and has no hardware path, so this contender shows
what git pays today rather than what raw SHA-1 costs.

The resolved crate versions, sources, and registry checksums are included
in stdout, the text report, and the SVG metadata.

## Code paths by input size

Each contender may switch implementation as the input grows. BLAKE3
divides input into 1024-byte chunks; the crates.io crate runs a single
chunk through its one-chunk compressor and batches whole chunks into
the widest SIMD `hash_many` it can fill (four-way NEON on AArch64, so
four chunks at 4 KiB; AVX-512, AVX2, SSE4.1, or SSE2 on x86). The SME2
fork runs an input of one chunk or less through one call of its scalar
kernel (every block including the root compression, with the state in
registers throughout), two to fifteen chunks on integer + NEON hybrid
kernels, and groups of sixteen on the SME2 kernel (16 KiB and above).
BLAKE3 mt leaves the caller's thread above one SIMD width of chunks;
BLAKE3 servil mt can split over threads from 64 KiB, its fourth path, drawn
as a triangle. SHA-256 and SHA-1DC run one path at every size.

In the many-messages use case a contender looping one message per call
runs its 64 B kernel at every batch size; ab-blake3's batch entry point
changes path at sixteen messages, where the first full SIMD group forms;
BLAKE3 servil's changes at two (the NEON hybrid parent kernels) and
sixteen (the SME2 group kernel), and servil mt's again at 1024, where a
64 KiB batch may leave the calling thread.

The text report lists the kernel at each point for every contender in
each use case (one line for a contender with a single kernel) and marks
where a new one begins. In the graph, dot shape carries the same information: a circle
for a contender's first kernel, a diamond for its second, a square for
its third, a triangle for a fourth. Hovering any dot names its kernel,
and hovering the first dot of a new kernel adds a sentence on why the
kernel changes there. A legend under the plot
explains the shapes. Colour stays with the contender, so a line keeps
one colour while its dots change shape.

These inferences follow BLAKE3 v1.8.7's `src/platform.rs` and the
fork's `src/ffi_sme2.rs` and `src/ffi_neon_hybrid.rs`.

## Correctness before timing

Before calibration, every selected implementation receives identical,
deterministically generated bytes and checks its digest against
`src/test_vectors.rs`. An input of `n` bytes for seed `s` is the
little-endian 64-bit words `s << 48 | 0, s << 48 | 1, ...` cut to `n`
bytes: every block of every input differs, so a kernel that mixed up
its lanes would fail, and seed 1 gives the second shared copy different bytes.
Its 72 one-message vectors cover both input seeds at every benchmark
size, empty input, and short boundary tails; its 48 batch vectors cover both seeds at every batch size, each the SHA-256 of
the batch's digests concatenated in message order, so a batch entry
point is checked digest by digest against a one-line anchor.
Multithreaded entries also hash the same vectors in two simultaneous
calls.

Golden BLAKE3 outputs come from the upstream reference implementation;
SHA-256 and SHA-1 outputs come from Python's `hashlib`. The generator is
`tools/gen-test-vectors.py`; it records the BLAKE3 reference source's
SHA-256. Regeneration is an explicit review step, outside tests and builds.
The optimized fork never supplies the expected answers. A mismatch stops
the run with the implementation, algorithm, input length, seed, and both
digests in the error.

Correctness and timing share one implementation dispatch. The timed loop
black-boxes digest bytes; the checking loop asserts their equality.
Checks run outside the measured samples. During timing, the two shared
copies use separate buffers with different contents, as two programs
would.

## Interleaving and precision

The contenders run in a Williams design: a set of orders that together
place every contender in every position equally often and realise every
"Y right after X" adjacency equally often — the balance all permutations
would give (n orders for an even count of contenders, 2n for odd). Point
order (the fifty-one input sizes and batch sizes of the two use cases
together) rotates independently. Each contender/point combination is
calibrated separately so its timed samples last about 1 ms each.

Each combination collects 96 solo samples and 192 shared ones (fewer
for long cells, below), or 24 and 48 in a `--quick` run, which also stops
below 1 MiB and 10,000 messages. The
rounds cycle through the orders and rotate the point that starts a
round; a round count that is no multiple of the order or point count
leaves some orders or starting points once more than others, a fraction
of a sample per cell, far below the difference between two runs. The
runtime budget favours sample count over sample length: the median's
interval narrows with the square root of the count, and a 1 ms sample
is long enough that the clock's resolution is far below noise.

Cells whose single hash takes 4 ms or more (the
plateau sizes, where a sample is one hash of tens of milliseconds) get a
time budget: such a cell is sampled in every fourth round, at an offset
of its own so its samples span the run, and in every second round while
the 95% interval of its median is wider than 2% of it. Shorter samples
(0.5 ms) were tried and rejected: every median read 1.6% slower, since
a sample's fixed cost weighs twice as much.

The band around each median line is the **95% bootstrap confidence
interval of the median**: the cell's samples are resampled with
replacement 400 times, each resample's median taken, and the 2.5th and
97.5th percentiles of those medians drawn. That interval says how well
the median is known. The hover panel also gives each cell's minimum and
maximum, which describe the run's environment.

Some cells run at two speeds, and then every report shows both, with
equal weight, faster first. The clearest case: two copies of an SME2
kernel run at full speed when macOS places them on different P-clusters
and at about half when it places them on one, which shares its SME unit;
the share of rounds in each state varies from run to run, so a single
median would land on either speed by chance. A cell has two speeds when
its sorted samples split at a gap of 4% or more, with a tenth or more of
the samples on each side and the two sides' medians 1.25× or more apart.
The text tables print such a cell as `a|b`, and the TWO SPEEDS section
lists the servil cells that did. In the graph the contender's line
follows each point's common speed (the one with more samples); where a
point ran at two, the rare speed adds its own dot and line segments,
drawn fainter in proportion to its share (its samples over the common
speed's, at least 0.15 opacity). The value label gives both (`a | b`),
the hover panel says "Two speeds observed" with each speed's median,
interval, and share of samples, and a footnote under the plots names
common causes: performance and efficiency cores, two copies sharing one
unit of the chip, a VM's host moving it between cores.

The band's appearance reports the interval's width relative to the
median: under 2% a faint tint; 2–5% a deeper tint; 5% and over a
dashed outline, and the hover panel says the median is poorly
determined. The text report marks such cells with `~`.

## The graph

The SVG shows four plots, each use case solo and then shared, each with
median lines and confidence bands on a log-log grid.

A switch above the first y axis flips every plot between rate (the
default; higher is better: GB/s above, million messages per second
below) and time (lower is better: ns/B above, ns per message below).
Rate is the reciprocal of time, so on the log axis each plot mirrors
through its middle: the switch animates each point along a straight
line to its mirrored position over 0.7 s while the axes cross-fade, and
every label, value, and hover figure follows the chosen unit. Ratios
between contenders are unitless and stay put.

Hovering a dot opens a panel for that point: the hovered
contender's median, range, and code path, then every visible contender
of that plot ranked fastest first with its time, rate, and speed
relative to the hovered one ("▲ 1.35× faster" in green, "about the same" in grey, "▼ 3.22×
slower" in red; contender colours stay away from those two hues).
Hidden contenders stay out of the ranking. On a touch screen, tapping a
dot pins the panel; tapping it again or the background clears it. Name
highlighting follows the mouse, since a finger has no way to leave.

The names at the right edge of each plot are toggles. Clicking one
hides that contender in every plot: its marks fade out, each y axis
rescales to the contenders still showing, and its provenance line drops
out of the block below. The name stays in
place, greyed with a hollow swatch and a "hidden · click to show" hint,
anchored toward where its line would sit on the current axis. A viewer
without script support shows every contender, laid out identically.

## Output

The run prints the text report on stdout and progress on stderr (the
phase, a bar over the sample rounds, and the running median of every
contender at the largest input size), and writes three files to
`benchmark-results/{CPU}.{OS}/`: `bench-hashes.result.txt` (the
report), `bench-hashes.graph.svg` (the graph), and
`bench-hashes.samples.tsv` (every sample of every cell, both scenarios,
in the order taken, with the provenance and the CPU's identity as
`# key: value` lines).

## Load from other programs

While it measures, the run reads how much CPU time the whole machine
spent busy and how much this process used; the difference is CPU time
other programs took. Linux also reports steal time, CPU time a
hypervisor withheld from a virtual machine's CPUs for other work on the
host. The OS counts both in 10 ms ticks, so the run sums them over
windows of 5 seconds. The report, the samples file, and the graph's
Provenance section give the run's average and its busiest window, in
CPUs kept busy, and call the run busy when the busiest window reached a
whole CPU (other programs or steal). Measured in a quiet 16-CPU Linux
VM: 0.02 CPUs on average; with two busy loops beside the run: 2.04. An
Apple M4 Max desktop running a Linux VM keeps 0.40-0.56 CPUs busy, and
its results then match a quieter run's to about 1%.

Hypervisors that report no steal time (Apple's Virtualization framework,
for one) keep a VM's guest from seeing load on the host, so a VM can
read quiet while the host is busy.

## Build settings and provenance

Release builds use optimization level 3, fat LTO, one codegen unit,
abort-on-panic, no incremental compilation, and target-cpu=native, so
the executable may fail on a different CPU: build on the machine being
measured.

The build script reads `Cargo.lock` and embeds each contender crate's
resolved version, registry checksum or git commit, and source, and this
repository's own commit and whether its tree was clean. The report, the
samples file, and the graph's Provenance section carry them, so a result
names the exact code it measured. `Cargo.lock` is checked in, so every
build of one commit measures the same code.
