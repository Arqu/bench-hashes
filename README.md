# bench-hashes

Written by GPT-5.6 Sol and Claude Fable 5 to my (Zooko's) specifications.

A small single-threaded benchmark comparing BLAKE3, SHA-256, SHA-1DC
(SHA-1 with collision detection, the construction git uses), BLAKE3
with SME2 kernels (Apple M4 and later), and on Apple platforms the
system's CommonCrypto SHA-256.

The benchmark tests every power-of-two input size from 64 B to 1 MiB,
plus 3 KiB: 64 B, 128 B, 256 B, 512 B, 1 KiB, 2 KiB, 3 KiB, 4 KiB, 8 KiB,
16 KiB, 32 KiB, 64 KiB, 128 KiB, 256 KiB, 512 KiB, and 1 MiB. Below 1 KiB
a BLAKE3 input is one chunk; from 2 KiB to 16 KiB its SIMD paths fill
(4-way NEON at 4 KiB, a sixteen-lane SME2 group at 16 KiB); above that the
bulk rate settles. 3 KiB is where the SME2 fork's integer + NEON hybrid
kernels first overtake hardware SHA-256.

It reports median, minimum, and maximum time per byte in integer
picoseconds, measured on the calling thread's CPU-time clock
(`CLOCK_THREAD_CPUTIME_ID`) so time spent descheduled stays out of the
samples. Lower is better. The clock is named in the report's provenance.

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
cargo run --release -- --list                        # keys and availability here
```

Keys: `blake3`, `blake3-sme2`, `sha256`, `sha256-ring`, `sha256-cc`,
`sha1dc`. The
baseline for ratios is the first BLAKE3 contender in the column order,
or the first contender when no BLAKE3 is selected.

### Requirements

The BLAKE3 SME2 contender is a git dependency on the `sme2-bench`
branch of github.com/johnservil/BLAKE3. Cargo fetches it on the first
build, and `Cargo.lock` pins the exact commit, so a fresh clone builds
with network access and nothing else. `cargo update -p blake3_sme2`
moves the pin to the branch tip.

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
benchmark asserts that selection at startup. The BLAKE3 SME2 column
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

BLAKE3 SME2 is the same crate from the `sme2-bench` branch of
github.com/johnservil/BLAKE3, built as a git dependency under the crate
name `blake3_sme2` so it links beside the crates.io crate. It selects
its SME2 kernels at runtime and requires a CPU that reports SME2 with a
512-bit streaming vector length; the build requires a toolchain that
assembles SME2 (see Requirements above). Both requirements fail stop,
so this column measures the SME2 kernel on every machine where the
benchmark runs. Its provenance line gives the branch and commit instead
of a registry checksum.

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
fork runs one chunk on a scalar kernel, two to fifteen on integer + NEON
hybrid kernels, and groups of sixteen on the SME2 kernel (16 KiB and
above). SHA-256 and SHA-1DC run one path at every size.

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
(80 for two, four, or five contenders; 96 for three or six). The runtime budget favours sample
count over sample length: fewer samples thin the evidence behind the
min–max band and let it look tight while the true spread is wider,
whereas shorter samples keep the count and let any disturbance widen
the band honestly. A whole run takes about six seconds on this
benchmark's development machines.

The band's appearance reports precision. Spread is (maximum − minimum)
÷ median at a size; a contender's band takes its worst spread across
sizes. Under 10% the band is a faint tint; from 10% to 25% the tint
deepens; at 25% and above a dashed outline appears. The hover panel
prints each point's spread as ±% and names it when it is noticeable or
wide, and the text report marks wide cells with `!` and counts them.

## The graph

The SVG shows median lines with min–max bands on a log-log grid. The
headline sentence beneath the title states each contender's speed
relative to BLAKE3 across the size range.

Hovering a dot opens a panel for that input size: the hovered
contender's median, range, and code path, then every visible contender ranked
fastest first with its ns/B, GB/s, and speed relative to the hovered
one ("▲ 1.35× faster" in green, "about the same" in grey, "▼ 3.22×
slower" in red; contender colours stay away from those two hues).
Hidden contenders stay out of the ranking.

The names at the right edge are toggles. Clicking one hides that
contender: its marks fade out, the y axis rescales to the contenders
still showing, the headline sentence restates itself for that set, and
its provenance line drops out of the block below. The name stays in
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
