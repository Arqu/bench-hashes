# bench-hashes

Written by GPT-5.6 Sol and Claude Fable 5 to my (Zooko's) specifications.

A small single-threaded benchmark comparing BLAKE3, SHA-256, SHA-1DC
(SHA-1 with collision detection, the construction git uses), and BLAKE3
with SME2 kernels (Apple M4 and later).

The benchmark tests every power-of-two input size from 64 B to 1 MiB:
64 B, 128 B, 256 B, 512 B, 1 KiB, 2 KiB, 4 KiB, 8 KiB, 16 KiB, 32 KiB,
64 KiB, 128 KiB, 256 KiB, 512 KiB, and 1 MiB. Below 1 KiB a BLAKE3 input
is one chunk; from 2 KiB to 16 KiB its SIMD paths fill (4-way NEON at
4 KiB, a sixteen-lane SME2 group at 16 KiB); above that the bulk rate
settles.

It reports median, minimum, and maximum time per byte, measured with
`std::time::Instant`. Lower is better.

## Build and run

```sh
cargo run --release
```

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

SHA-256 is provided by RustCrypto's sha2 crate with its optimized
assembly features enabled, including the ARMv8 SHA-256 instructions on
AArch64 and dedicated implementations on x86-64. Unsupported targets use
the portable fallback.

BLAKE3 SME2 is the same crate from the `sme2-bench` branch of
github.com/johnservil/BLAKE3, built as a path dependency under the crate
name `blake3_sme2` so it links beside the crates.io crate. It selects
SME2 kernels at runtime when the CPU reports SME2 with a 512-bit
streaming vector length, and uses NEON otherwise, so on a machine
without SME2 it measures the fork's NEON path. Its provenance line gives
the branch and commit instead of a registry checksum.

SHA-1DC is provided by RustCrypto's sha1-checked crate: SHA-1 with the
collision-detection pass that git applies to every object hash. The
detection is pure Rust and has no hardware path, so this contender shows
what git pays today rather than what raw SHA-1 costs.

The resolved crate versions, sources, and registry checksums are included
in stdout, the text report, and the SVG metadata.

## BLAKE3 backend reporting

The benchmark reports the BLAKE3 implementation selected for the
performance-dominant path at each input size. These inferences are based
on BLAKE3 v1.8.7.

BLAKE3 divides input into 1024-byte chunks. A 64-byte input fits in one
chunk and does not enter the bulk SIMD path; on AArch64 that means the
portable compressor. Multi-chunk inputs use the widest available
hash_many implementation: AVX-512, AVX2, SSE4.1, or SSE2 selected at
runtime on x86, and four-way NEON on AArch64. Wider implementations fall
through to narrower ones when an input does not fill a complete SIMD
batch.

## Interleaving

The contenders are benchmarked in every permutation equally often, and
input-size order rotates independently. This distributes ordering effects,
thermal throttling, and competing system activity evenly. Each
algorithm/input-size combination is calibrated separately so its timed
blocks have approximately equal durations.

## The graph

The SVG shows median lines with min–max bands on a log-log grid, plus a
ratio panel giving each contender's speed relative to BLAKE3 at each
input size (BLAKE3 time ÷ contender time; above 1.0 the contender is
faster).

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
