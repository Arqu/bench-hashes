# Style Guides

## Communication

- Phrase positively or neutrally; avoid negations and "not this, but that" contrasts.
- Frame positively: show the promising, successful aspects of the recommended path. Mention an alternative only when its trade-offs deserve our attention.
- The reader has limited working memory and limited ability to search back through recent text. Include only what the current focus needs.

Sixteen actions that improve writing:
1. Sand off filler words
2. Find the real actors
3. Restore actions to verbs
4. Delete empty verbs
5. Prefer characters as subjects
6. Put subjects and verbs together
7. Put verbs and objects together
8. Make the opening familiar
9. Put new and important information last
10. Repair topic flow
11. Repair stress flow
12. Establish a clear topic sentence
13. Make subjects consistent across a passage
14. Control passive voice deliberately
15. Name responsibility
16. Trim metadiscourse

## Coding: integers first

Avoid floating point except where the domain is continuous by nature (pixel coordinates on a log axis, an elapsed-seconds display). Measurements, statistics, ratios, and thresholds are integers in fixed units: picoseconds per byte for time, permille for ratios and spreads, hundredths for opacities. Integer arithmetic is exact and reproducible; round explicitly (`(a + b / 2) / b`) at the one place a division happens. Convert to `f64` at the last moment, for drawing only.

## Coding: Design By Contract

We document and `assert` every precondition our code relies on (`debug_assert` only on hot paths). Contracts are **expansive** (the caller carries the responsibility), **conceptually simple** (a few sentences of English; simplicity beats familiarity), and **structurally simple** to enforce (few lines, types, data elements, conditionals).

We never write "defensive code" — code that complicates a contract to ease the caller's life. When running code detects that a caller misunderstood the contract, it **fails stop**: panic with a clear message. Stopping is safer than proceeding, and it lets people fix the caller or loosen the contract. Defensive codebases grow buggier over time; DBC codebases stay predictable.

# This repository

Read `NOTES.md` first: it holds the measurement design, the known threats to validity and how each was closed, and the open questions. This file is style and environment.

`bench-hashes` is a single-crate Rust benchmark (`src/main.rs`, `build.rs`) comparing BLAKE3, SHA-256, SHA-1DC, BLAKE3 servil (the fork with SME2 kernels), and under `--duo` the two multithreaded contenders BLAKE3 mt (crates.io `Hasher::update_rayon`) and BLAKE3 servil mt (the fork's `hash_multithreaded`: subtrees over the caller's thread and the fork's own workers, shared fairly between concurrent callers). `--duo` runs two copies of every contender at once and scores the later finish; its results land in `bench-hashes.duo.*` beside the solo files. The `BLAKE3` column measures the crates.io `blake3` crate maintained by the BLAKE3 authors. The `BLAKE3 servil` column measures the `sme2-bench` branch of github.com/johnservil/BLAKE3, built as a path dependency named `blake3-servil` from the enclosing checkout at `..` (`/workspace` in the VM; see Environment).

Results land in `benchmark-results/{CPU}.{OS}/` as a text report and an SVG. Every run overwrites them. The fork fails stop at build time when the assembler lacks SME2. At run time it selects its kernels from the CPU: the SME2 group kernel where the CPU reports SME2 with 512-bit streaming vectors, the integer + NEON hybrid kernels alone elsewhere. Both are real results; the report's kernel table names the platform the run measured.

Vocabulary: an *implementation* is a crate (crates.io `blake3`, the servil fork, `sha2`, ...) and is what `--list` and `--contenders` select. A *kernel* is the code path an implementation runs at one input size, chosen at run time and reported per size. A *mode* is how many threads a contender may use: single-threaded or multithreaded. Availability is a property of the build's platform (CommonCrypto on Apple), never of the machine's capacity; the fork reports no capacity to the benchmarker, and the benchmarker asks for none.

# Environment

## Where things are

- This repository (github.com/johnservil/bench-hashes, branch `main`) is checked out at `/workspace/bench-hashes`, nested inside the fork it measures.
- `/workspace` is the fork checkout (github.com/johnservil/BLAKE3, branch `sme2-bench`), the `blake3-servil` path dependency at `..`. Edits there take effect on the next build; `build.rs` embeds that checkout's branch, commit, and clean or dirty fingerprint in the provenance, and skips untracked directories (this one) when fingerprinting the fork's tree.
- `/workspace` is the host checkout mounted through sandboxfs and is the only path that survives a VM restart. `/workspace/vm/` holds the guest-side environment: `vm/home` (the `HOME` for `git` and `cargo`, with `safe.directory = *`, John Servil's identity, and the credential helper), `vm/home/bin/gh-cred.sh` (reads the johnservil classic token from `/workspace/ghtokenclassic.txt`; never print that file), and `vm/setup.sh`, which installs `clang-19` and re-points both repos' credential helpers. Run `sh /workspace/vm/setup.sh` first after a restart. The fork's `AGENTS.md` describes the same layout from its side.

## Building and running

- The VM is Debian 12 on AArch64 with two cores. Its CPU exposes SME2 with 512-bit streaming vectors (`/proc/cpuinfo` lists `sme2`), so the fork's kernels run here. Absolute timings differ from Apple hardware; relative comparisons hold.
- The fork's SME2 kernel is `c/blake3_sme2_aarch64.S`, compiled by the `cc` crate with `-march=armv9-a+sme2`. The system `cc` (GCC 12) and `as` (binutils 2.40) predate SME2, so the fork's build script fails under them with a message naming the fix. `clang-19` assembles SME2; `TMPDIR` gives clang a temporary directory that exists in the guest.
- Run: `HOME=/workspace/vm/home CARGO_TARGET_DIR=/tmp/target CC=clang-19 TMPDIR=/tmp cargo run --release --manifest-path /workspace/bench-hashes/Cargo.toml -- --contenders blake3,blake3-servil`
- Every `git` and `cargo` command takes `HOME=/workspace/vm/home`; files on the mount show as uid 501 while the guest runs as uid 0, which `safe.directory` covers. `CARGO_TARGET_DIR=/tmp/target` is a tmpfs build cache; `CARGO_HOME=/usr/local/cargo`. The toolchain is rustc 1.98.1 without the `rustfmt` component, so there is no formatting check in the guest.
- The contender set is a runtime `Roster` (see `--list`, `--all`, `--contenders`). CommonCrypto SHA-256 reports itself unavailable off Apple; its FFI module compiles only under `target_vendor = "apple"`. `rustup target add aarch64-apple-darwin` lets `cargo check --target aarch64-apple-darwin` type-check that path; the full crate fails to *build* for that target in this VM because the fork's C files need Apple headers.
- Sample timing uses `std::time::Instant` (a hardware counter: `CLOCK_UPTIME_RAW` on Darwin, `CLOCK_MONOTONIC` on Linux). A run under thread CPU time once showed a 12% floor shared by three contenders; two experiments cleared the clock and pointed at a ~12 ms core-frequency boost. `--trace-clocks PATH` records wall, thread-CPU, mach ticks, and (Apple) per-core-kind cycles per sample; `tools/analyze-clock-trace.py` reads it. github.com/johnservil/measure-clocks3 (needs `cargo +nightly`; clone it under `/workspace/tmp` if needed again) has `--pitfall` and `CPU-TIME-CLOCKS-AND-FREQUENCY.md`.
- `rsvg-convert` (librsvg2-bin, reinstall after a restart) renders an SVG to PNG to eyeball it: `rsvg-convert -w 1200 file.svg -o out.png`.
- Commands for the user go on one line, with no `\` continuations.
- Never `sleep` in commands. When a network call fails, report it and stop; the user decides about retries.
- Run long commands (builds, benchmark runs, package installs) without a timeout and let their output stream, so the user can watch progress and interrupt when they choose.
