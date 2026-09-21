# bench-hashes

Written by GPT-5.6 Sol and Claude Fable 5 to my (Zooko's) specifications.

A small single-threaded benchmark comparing BLAKE3, SHA-256, SHA-1DC
(SHA-1 with collision detection, the construction git uses), BLAKE3
servil (a fork with SME2 kernels for Apple M4 and later), and on Apple
platforms the system's CommonCrypto SHA-256.

The benchmark tests every power-of-two input size from 64 B to 1 MiB,
plus 3 KiB: 64 B, 128 B, 256 B, 512 B, 1 KiB, 2 KiB, 3 KiB, 4 KiB, 8 KiB,
16 KiB, 32 KiB, 64 KiB, 128 KiB, 256 KiB, 512 KiB, and 1 MiB. Below 1 KiB
a BLAKE3 input is one chunk; from 2 KiB to 16 KiB its SIMD paths fill
(4-way NEON at 4 KiB, a sixteen-lane SME2 group at 16 KiB); above that the
bulk rate settles. 3 KiB is where the SME2 fork's integer + NEON hybrid
kernels first overtake hardware SHA-256.

It reports median, minimum, and maximum time per byte in integer
picoseconds. Lower is better.

On Apple silicon the reported time is **cycles per byte at the run's
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
cargo run --release -- --list                        # keys and availability here
```

Keys: `blake3`, `blake3-servil`, `sha256`, `sha256-ring`, `sha1dc`; and
`sha256-cc`, which runs only when named with `--contenders`. It stays
available for direct comparison; on Apple silicon the ring and sha2
crates are each faster than CommonCrypto at every size, so the default
and `--all` runs leave it out.

`--trace-clocks PATH` writes one CSV line per sample with the wall
(`Instant`), thread-CPU, process-CPU, and `mach_absolute_time` readings
taken around the same work, for clock diagnosis;
`tools/analyze-clock-trace.py PATH` finds windows where the clocks
disagree and says what shape the disagreement has.

### Requirements

The BLAKE3 servil contender is a path dependency on a local checkout of
github.com/johnservil/BLAKE3 at `../BLAKE3`, a sibling of this
repository, with its `sme2-bench` branch checked out:

```sh
git clone --branch sme2-bench https://github.com/johnservil/BLAKE3 ../BLAKE3
```

Edits to that checkout take effect on the next build, and the build
script records the checkout's branch, commit, and clean or dirty state
in the provenance.

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

At run time the fork's `Platform::detect()` requires a CPU that reports
SME2 with a 512-bit streaming vector length (Apple M4 and later, or a
Linux 6.4+ kernel exposing `HWCAP2_SME2`) and panics otherwise, and the
benchmark asserts that selection at startup. The BLAKE3 servil column
therefore always measures the SME2 kernel; on other hardware the
benchmark stops with a message instead of timing NEON under that
heading.

The build script also runs `git` on the repository to record the commit
and clean status. If the tree is owned by a different user than the one
building (as with a mounted volume in a VM or container), git refuses
with "dubious ownership" and the build fails; allow it with
`git config --global --add safe.directory <path-to-this-repo>`.

## Progress

While measuring, the benchmark reports on stderr: the current phase
(calibrating, warming up, measuring), a bar over the sample rounds with
elapsed and estimated remaining time, and the running median for every
contender at the largest input size. On a terminal the line redraws in
place; in a log each update is its own line. Stdout carries the final
report alone, so redirecting it captures the results cleanly.

## Output layout

Results are written to a machine-specific subdirectory:

```text
benchmark-results/{CPU}.{OS}/bench-hashes.result.txt
benchmark-results/{CPU}.{OS}/bench-hashes.graph.svg
```

## BLAKE3 threading

The BLAKE3 dependency is built with only its std feature. Its optional
Rayon support is not enabled, and the benchmark uses the ordinary one-shot
blake3::hash function.

BLAKE3 may still use SIMD parallelism within the calling thread. That is
single-threaded execution, not operating-system-level multithreading.

## Hash implementations

BLAKE3 is provided by the blake3 crate, built with only its std
feature. Rayon is not enabled, and the benchmark uses the one-shot
blake3::hash function. BLAKE3 may still use SIMD parallelism within the
calling thread; that is single-threaded execution, not multithreading.

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
`../BLAKE3` under the crate name `blake3_sme2` so it links beside the
crates.io crate. It selects
its SME2 kernels at runtime and requires a CPU that reports SME2 with a
512-bit streaming vector length; the build requires a toolchain that
assembles SME2 (see Requirements above). Both requirements fail stop,
so this column measures the SME2 kernel on every machine where the
benchmark runs. Its provenance line gives the checkout's branch, commit,
and clean or dirty state instead of a registry checksum.

SHA-256 CommonCrypto, on Apple platforms only, calls the system's
libSystem through FFI using `CC_SHA256_Init`, `CC_SHA256_Update`, and
`CC_SHA256_Final`. This is the implementation most Apple software
reaches for, so it anchors the sha2 crate's number against the
platform's own. The benchmark checks that the two agree on every input
before timing them. Its provenance is the running OS rather than a
crate version.

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
kernels, and groups of sixteen on the SME2 kernel (16 KiB and above). SHA-256 and SHA-1DC run one path at every size.

The text report lists the path at each size for both BLAKE3s and marks
where a new one begins. In the graph, dot shape carries the same
information: a circle for a contender's first path, a diamond for its
second, a square for its third. The first dot of a new path wears a
ring; hovering any dot names its path, and hovering a ringed dot adds a
sentence on why the path changes there. A legend under the plot
explains the shapes. Colour stays with the contender, so a line keeps
one colour while its dots change shape.

These inferences follow BLAKE3 v1.8.7's `src/platform.rs` and the
fork's `src/ffi_sme2.rs` and `src/ffi_neon_hybrid.rs`.

## Interleaving and precision

The contenders run in a Williams design: a set of orders that together
place every contender in every position equally often and realise every
"Y right after X" adjacency equally often — the balance all permutations
would give (n orders for an even count of contenders, 2n for odd). Input-size order
rotates independently. Each contender/size combination is calibrated
separately so its timed samples last about 1 ms each.

Each combination collects about 80 samples: the exact count is the
smallest multiple of both the size count and the order count at or
above 80, so every order and every size position recurs equally often
(80 for two, four, or five contenders; 96 for three or six). The
runtime budget favours sample count over sample length: the median's
interval narrows with the square root of the count, and a 1 ms sample
is long enough that the clock's resolution is far below noise. A whole
run takes about six seconds on this benchmark's development machines.

The band around each median line is the **95% bootstrap confidence
interval of the median**: the cell's samples are resampled with
replacement 400 times, each resample's median taken, and the 2.5th and
97.5th percentiles of those medians drawn. That interval says how well
the median is known. On an M4 Max with 80 rounds the typical cell's
interval is ±0.1–0.2%, and adjacent contenders' bands touch at one
size in sixty-four; `--thorough` triples the rounds and narrows the
intervals by about 1/√3. The extremes are still reported in the text
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

The SVG shows median lines with confidence bands on a log-log grid.

A switch above the y axis flips the graph between ns/B (the default;
lower is better) and GB/s (higher is better). GB/s is the reciprocal of
ns/B, so on the log axis the plot mirrors through its middle: the
switch animates each point along a straight line to its mirrored
position over 0.7 s while the two axes cross-fade, and every label,
value, and hover figure follows the chosen unit. Ratios between
contenders are unitless and stay put.

Hovering a dot opens a panel for that input size: the hovered
contender's median, range, and code path, then every visible contender ranked
fastest first with its ns/B, GB/s, and speed relative to the hovered
one ("▲ 1.35× faster" in green, "about the same" in grey, "▼ 3.22×
slower" in red; contender colours stay away from those two hues).
Hidden contenders stay out of the ranking.

The names at the right edge are toggles. Clicking one hides that
contender: its marks fade out, the y axis rescales to the contenders
still showing, and its provenance line drops out of the block below. The name stays in
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
