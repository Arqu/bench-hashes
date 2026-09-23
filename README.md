# bench-hashes

Written by GPT-5.6 Sol and Claude Fable 5 to my (Zooko's) specifications.

A small benchmark comparing BLAKE3, SHA-256, SHA-1DC (SHA-1 with
collision detection, the construction git uses), BLAKE3 servil (a fork
with SME2 kernels for Apple M4 and later), ab-blake3 (a crate with a
`const fn` BLAKE3 and a batch entry point for many 64-byte messages),
and on Apple platforms the system's CommonCrypto SHA-256. Two
multithreaded contenders, BLAKE3 mt (the crates.io crate on a Rayon
pool) and BLAKE3 servil mt (the fork's `hash_multithreaded`), join
default and `--all` runs. Every run measures every contender under
contention, two copies at once; `--solo` adds a single-copy column
beside it: see "The duo measurement" below.

Every run measures two use cases. **One message per call**: a call
hashes one input, at twenty-four sizes from 64 B to 128 MiB, reported
per byte. **Many messages per call**: a call hashes a batch of 64-byte
messages, at twenty-four batch sizes from 1 to 262144 messages, reported
per message; see "The many-messages use case" below. The graph shows the
two as two plots, one below the other.

The one-message axis tests every power-of-two input size from 64 B to
128 MiB, plus 3 KiB and 3 MiB: 64 B, 128 B, 256 B, 512 B, 1 KiB, 2 KiB,
3 KiB, 4 KiB, 8 KiB, 16 KiB, 32 KiB, 64 KiB, 128 KiB, 256 KiB, 512 KiB,
1 MiB, 2 MiB, 3 MiB, 4 MiB, 8 MiB, 16 MiB, 32 MiB, 64 MiB, and 128 MiB.
Below 1 KiB a BLAKE3 input is one chunk; from 2 KiB to 16 KiB
its SIMD paths fill (4-way NEON at 4 KiB, a sixteen-lane SME2 group at
16 KiB); above that the bulk rate settles. 3 KiB is where the SME2 fork's
integer + NEON hybrid kernels first overtake hardware SHA-256. The sizes
past 1 MiB show the plateau: a contender whose 32, 64, and 128 MiB
medians agree has levelled out. The multithreaded contenders take
longest to get there, since a pool hand-off or a subtree merge amortises
more slowly than one kernel call (the fork's was still climbing at
8 MiB, and Rayon's still is at 128 MiB on the VM); everything from
8 MiB up is past the last-level cache on every machine this benchmark
targets. 3 MiB is to the plateau what 3 KiB is to the SIMD
ramp: a tree that is no power of two (a 2 MiB left subtree beside a
1 MiB right one), so a splitter that cuts at subtree boundaries hands
its threads unequal work there.

It reports median, minimum, and maximum time per byte in integer
picoseconds. Lower is better.

Duo samples report measured time: the copies' cycle counters describe
two threads, and no one rate normalises the later finish. Solo samples
(`--solo`) on Apple silicon report **cycles per byte at the run's
sustained clock**. Each sample reads the thread's cycle counter
(`thread_selfcounts`) around the work; the run's sustained clock is the
median over every sample of cycles ÷ elapsed time; and each sample's
cycles per byte is divided by that rate. A core boost or throttle during
a sample stretches or shrinks its elapsed time and leaves its cycles
alone, so the reported time is unmoved. On an M4 Max this took the
median cell's sample spread from 11% to 1.3%, with medians unchanged;
what remains in the band is the code's own variation, cache effects,
and the cycles a preempted thread spends warming back up. The result
reads in the unit a stopwatch gives, with the machine's frequency
excursions removed. The provenance names the rate.

Where no per-thread cycle counter is available, the reported time is
the elapsed time on the platform's hardware counter (`CLOCK_UPTIME_RAW`
on Darwin, `CLOCK_MONOTONIC` on Linux, via `std::time::Instant`), and
the provenance says so.

Elapsed time is always measured on that hardware counter: a register
read with no NTP slew that stops while the machine sleeps. Thread CPU
time was examined as an alternative and found accurate but no better
at the millisecond scale; the frequency excursions it appeared to
reveal were real, and cycles are the instrument that sees them.

## Build and run

```sh
cargo run --release
```

With no options the benchmark compares SHA-1DC with the best available
BLAKE3 and the best available SHA-256 on this machine. "Best" means
Pareto-better: at least as fast at every tested size and faster at
one. When two members of a family each win at some sizes, both are
shown and the report says so and names the size where the lead changes.
The default run measures every available contender to make that choice,
so it costs the same as `--all`.

```sh
cargo run --release -- --all                         # every contender this machine can run
cargo run --release -- --contenders sha256,sha256-cc # exactly these, in this column order
cargo run --release -- --thorough                    # three times the rounds, narrower bands
cargo run --release -- --solo                        # a solo column beside every duo column
cargo run --release -- --list                        # keys and availability here
```

Keys: `blake3`, `ab-blake3`, `blake3-servil`, `sha256`, `sha256-ring`,
`sha1dc`; and `sha256-cc`, which runs only when named with `--contenders`. It stays
available for direct comparison; on Apple silicon the ring and sha2
crates are each faster than CommonCrypto at every size, so the default
and `--all` runs leave it out. `blake3-mt` and `blake3-servil-mt` are
the multithreaded contenders; they join default and `--all` runs.
`blake3-servil-mt1`, the multithreaded call capped at one thread, runs
when named, as a check that it costs what `blake3-servil` costs.

### The many-messages use case

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
`hash_many_multithreaded` does the same over the fork's worker threads
(`blake3-servil-mt1` calls it with a budget of one). Messages are 64
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

The duo measurement below applies unchanged: each sample runs two copies
of the contender, each over its own batch, and times the later finish.

### The duo measurement

A hash tuned to take every core finishes sooner on an idle machine and
later on a busy one: when the cores it counted on are running something
else, its threads queue behind that work, and the pair finishes after
two single-threaded hashes would have. Every sample is therefore a duo
sample: two independent copies of the contender run at the same time,
each on its own thread over its own input of the size, released
together, timed to the later finish, per byte of one copy. Every
contender is measured this way, single-threaded ones included, so the
columns compare; a single-threaded hash costs about the same either way
(the two copies share memory bandwidth and, under a hypervisor, a
scheduler), and a multithreaded one shows what its threads cost when
the machine is shared.

`--solo` adds the idle-machine view: every sample interval then takes a
solo sample (one copy, one thread) and a duo sample of the same batch,
the text report gives every contender a `solo` and a `duo` column, and
the graph draws the duo medians as a dashed line with hollow dots in the
contender's colour beside the solid solo line, with both and their ratio
in the hover panel. A multithreaded contender's solo number describes an
idle machine, the one case it is built for; its duo number describes the
rest, and the default report is that number alone.

Duo samples report measured time: the copies' cycle counters describe
two threads, and no one rate normalises the later finish. Solo samples
follow the reported-time rule described at the top.

`--trace-clocks PATH` writes one CSV line per sample with the wall
(`Instant`), thread-CPU, process-CPU, and `mach_absolute_time` readings
taken around the same work, for clock diagnosis;
`tools/analyze-clock-trace.py PATH` finds windows where the clocks
disagree and says what shape the disagreement has.

### Requirements

The BLAKE3 servil contender is a path dependency on a checkout of
github.com/johnservil/BLAKE3 at `..`, with its `sme2-bench` branch
checked out: this repository lives inside that checkout, as
`BLAKE3/bench-hashes` (the fork's `.git/info/exclude` keeps it out of
the fork's status).

```sh
git clone --branch sme2-bench https://github.com/johnservil/BLAKE3
git clone https://github.com/johnservil/bench-hashes BLAKE3/bench-hashes
```

Edits to the fork take effect on the next build, and the build script
records the checkout's branch, commit, and clean or dirty state in the
provenance.

The fork's SME2 kernels are assembly, so building them needs a C
toolchain whose assembler understands `-march=armv9-a+sme2`: Clang/LLVM
17 or later (Xcode 15 or later on macOS), or GNU binutils 2.41 or later
with GCC 14 or later. The fork's build script fails when the probe
fails, naming the compiler and the fix, so a successful build always
contains the kernel. On Debian 12 and similar distributions, whose
system `cc` is GCC 12 with binutils 2.40, point the `cc` crate at a
newer compiler:

```sh
CC=clang-19 cargo run --release
```

Apple's clang from Xcode 15 or later works out of the box.

At run time the fork's `Platform::detect()` selects SME2 on a CPU that
reports SME2 with a 512-bit streaming vector length (Apple M4 and later,
or a Linux 6.4+ kernel exposing `HWCAP2_SME2`) and NEON elsewhere. The
fork describes its own kernels by input length through
`blake3_servil::kernel_report()` (and `kernel_report_multithreaded()`),
built from that same detection; the benchmark prints the report as the
kernel table ("platform SME2" or "platform NEON") and uses it for the
dot shapes and hover text, so a NEON run reads as what it is.

The build script also runs `git` on the repository to record the commit
and clean status. If the tree is owned by a different user than the one
building (as with a mounted volume in a VM or container), git refuses
with "dubious ownership" and the build fails; allow it with
`git config --global --add safe.directory <path-to-this-repo>`.

## Progress

While measuring, the benchmark reports on stderr: the current phase
(checking digests, calibrating, measuring), a bar over the sample rounds with
elapsed and estimated remaining time, and the running median for every
contender at the largest input size. On a terminal the line redraws in
place; in a log each update is its own line. Stdout carries the final
report alone, so redirecting it captures the results cleanly.

## Output layout

Results are written to a machine-specific subdirectory:

```text
benchmark-results/{CPU}.{OS}/bench-hashes.duo.result.txt
benchmark-results/{CPU}.{OS}/bench-hashes.duo.graph.svg
benchmark-results/{CPU}.{OS}/bench-hashes.duo.samples.tsv
```

The samples file holds every duo sample of every cell in the order
taken, with the provenance and a CPU identity (Linux: implementer, part,
feature list, machine model; macOS: brand and core counts per
performance level) as `# key: value` lines. The fork's
`tools/perf_regress.py` reads it.

## BLAKE3 threading

The `BLAKE3` and `BLAKE3 servil` contenders call the one-shot
`blake3::hash` function, which is single-threaded on every platform.
BLAKE3 may still use SIMD parallelism within the calling thread; that
is single-threaded execution, not operating-system-level
multithreading.

The blake3 crate is built with its `rayon` feature so that the
`BLAKE3 mt` contender can call `Hasher::update_rayon`; that feature
adds the method and leaves `blake3::hash` and every other API
single-threaded.

## Hash implementations

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

BLAKE3 servil is the same crate from the `sme2-bench` branch of
github.com/johnservil/BLAKE3, built from the local checkout at
`..` under the crate name `blake3-servil` so it links beside the
crates.io crate. The build requires a toolchain that assembles SME2
(see Requirements above) and fails stop without one. At run time the
fork reads the CPU: one that reports SME2 with a 512-bit streaming
vector length gets the SME2 group kernel for sixteen chunks and up;
every AArch64 core runs the scalar and integer + NEON hybrid kernels.
The report's kernel table names the platform the run measured. Its
provenance line gives the checkout's branch, commit, and clean or dirty
state instead of a registry checksum. For a batch the fork's `hash_many`
compresses runs of one-block messages many lanes at a time on the same
kernels its tree uses for parent nodes (sixteen per group on SME2, the
NEON hybrids below a group), and `kernel_report_many()` describes that
by batch size.

BLAKE3 mt is the crates.io crate's own multithreading, called as a
program calls it by default: `Hasher::new().update_rayon(input)` on
Rayon's global pool, which Rayon sizes to one thread per logical CPU.
The method splits the tree recursively with `rayon::join` down to the
SIMD degree, so any input above one SIMD width of chunks may cross
threads, and idle pool threads steal the halves. The two duo copies are
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
max_threads)` to cap one call's threads. The standard contender measures
the uncapped call; `--contenders blake3-servil,blake3-servil-mt1` compares
the one-thread cap with the ordinary single-threaded entry point.

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
its lanes would fail, and seed 1 gives the duo copy different bytes.
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
Checks run outside the measured samples. During timing, the two copies
still use separate buffers with different contents, preserving the duo
measurement's cache behavior.

## Interleaving and precision

The contenders run in a Williams design: a set of orders that together
place every contender in every position equally often and realise every
"Y right after X" adjacency equally often — the balance all permutations
would give (n orders for an even count of contenders, 2n for odd). Point
order (the forty input sizes and batch sizes of the two use cases
together) rotates independently. Each contender/point combination is
calibrated separately so its timed samples last about 1 ms each.

Each combination collects about 80 samples: the exact count is the
smallest multiple of both the point count and the order count at or
above 80, so every order and every point position recurs equally often
(80 for two, four, five, or eight contenders; 120 for three or six). The
runtime budget favours sample count over sample length: the median's
interval narrows with the square root of the count, and a 1 ms sample
is long enough that the clock's resolution is far below noise. A whole
run reports its elapsed time and remaining-time estimate as it progresses.

The band around each median line is the **95% bootstrap confidence
interval of the median**: the cell's samples are resampled with
replacement 400 times, each resample's median taken, and the 2.5th and
97.5th percentiles of those medians drawn. That interval says how well
the median is known. On an M4 Max with 80 rounds the typical cell's
interval is ±0.1–0.2%, and adjacent contenders' bands touch at one
size in sixty-four in that run. `--thorough` triples the target sample
count; rounding to complete orders determines the actual round count. The extremes are still reported in the text
table and in the hover panel, where they belong: a minimum and maximum
describe the run's environment, the interval describes the number.

Some cells run at two speeds. On an M4 Max, ring's SHA-256 at 128 B
spends a third of its samples near 82% of the median and the rest near
104%, a real effect of which contender ran just before. A single
median cannot express that, so when a cell's sorted samples split at a
gap of 4% or more with at least a tenth of the samples on each side,
the hover panel reports both clusters and their sizes. The band widens
honestly around the median, which sits between the modes.

The band's appearance reports the interval's width relative to the
median: under 2% a faint tint; 2–5% a deeper tint; 5% and over a
dashed outline, and the hover panel says the median is poorly
determined. The text report marks such cells with `!`.

## The graph

The SVG shows two plots, one per use case, each with median lines and
confidence bands on a log-log grid: one message per call above, many
messages per call below.

A switch above the first y axis flips both plots between rate (the
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

The names at the right edge of either plot are toggles. Clicking one
hides that contender in both plots: its marks fade out, each y axis
rescales to the contenders still showing, and its provenance line drops
out of the block below. The name stays in
place, greyed with a hollow swatch and a "hidden · click to show" hint,
anchored toward where its line would sit on the current axis. A viewer
without script support shows every contender, laid out identically.

## Native optimization

Release builds use optimization level 3, fat LTO, one codegen unit,
abort-on-panic, no incremental compilation, and target-cpu=native.
The resulting executable may fail on a different CPU; build on the
machine being measured.

## Source identification

The build script reads Cargo.lock and embeds the direct dependencies'
resolved versions, registry checksums, and source identifiers. These are
printed to stdout and included at the bottom of the SVG.

After the first build, retain Cargo.lock if you want later builds to use
the same full dependency graph.
