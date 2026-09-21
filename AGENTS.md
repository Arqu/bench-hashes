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

`bench-hashes` is a single-crate Rust benchmark (`src/main.rs`, `build.rs`) comparing BLAKE3, SHA-256, SHA-1DC, BLAKE3 servil (the fork with SME2 kernels), and under `--duo` the two multithreaded contenders BLAKE3 mt (crates.io `Hasher::update_rayon`) and BLAKE3 servil mt (the fork's `lanes` module: subtrees over the machine's execution lanes with cooperative admission). `--duo` runs two copies of every contender at once and scores the later finish; its results land in `bench-hashes.duo.*` beside the solo files. The `BLAKE3` column measures the crates.io `blake3` crate maintained by the BLAKE3 authors. The `BLAKE3 servil` column measures the `sme2-bench` branch of github.com/johnservil/BLAKE3, built as a path dependency named `blake3_sme2` from the sibling checkout at `../BLAKE3` (`/workspace/BLAKE3` in the VM). Edits there take effect on the next build; `build.rs` embeds that checkout's branch, commit, and clean or dirty fingerprint in the provenance.

Results land in `benchmark-results/{CPU}.{OS}/` as a text report and an SVG. Every run overwrites them. The fork fails stop at build time (assembler lacks SME2) and at run time (CPU lacks SME2), and `main()` asserts the SME2 platform, so any report that exists measured the SME2 kernel.

# Environment

- The VM is Debian 12 on AArch64 with two cores. Its CPU exposes SME2 with 512-bit streaming vectors (`/proc/cpuinfo` lists `sme2`), so the fork's kernels run here. Absolute timings differ from Apple hardware; relative comparisons hold.
- The fork's SME2 kernel is `c/blake3_sme2_aarch64.S`, compiled by the `cc` crate with `-march=armv9-a+sme2`. The system `cc` (GCC 12) and `as` (binutils 2.40) predate SME2, so the fork's build script fails under them with a message naming the fix. `clang-19` is installed and assembles SME2. Build with `CC=clang-19 TMPDIR=/tmp cargo run --release`; `TMPDIR` gives clang a temporary directory that exists in the guest.
- The johnservil classic token is in `ghtokenclassic.txt` (gitignored; never print it). Both this repository and the fork checkout at `/workspace/BLAKE3` (the `sme2-bench` clone the benchmark builds from) have `credential.helper` set to `/tmp/home/bin/gh-cred.sh`, which reads that file; the fork checkout also has `user.name`/`user.email` set to John Servil. The checkout and the helper live on the guest disk, so recreate them after a VM restart before pushing.
- `clang-19` came from the apt.llvm.org bookworm repository (`/etc/apt/sources.list.d/llvm19.list`); a fresh VM needs it installed again before the fork builds.
- `/workspace` holds both checkouts (`bench-hashes` and `BLAKE3`) mounted from the host through sandboxfs. Files show as uid 501 while the guest runs as uid 0, so git needs `safe.directory`. `HOME` points at an absent host path; use `HOME=/tmp/home` (which holds a `.gitconfig` with `safe.directory = /workspace`) for both `git` and `cargo` commands. `/tmp/home/.gitconfig` also sets `safe.directory = *`. `/tmp/home` lives on the guest disk and vanishes with the VM; `/workspace` persists.
- `CARGO_TARGET_DIR=/tmp/target` on a tmpfs; `CARGO_HOME=/usr/local/cargo`. The toolchain is rustc 1.98.1 without the `rustfmt` component, so there is no formatting check available in the guest.
- The contender set is a runtime `Roster` (see `--list`, `--all`, `--contenders`). CommonCrypto SHA-256 reports itself unavailable off Apple; its FFI module compiles only under `target_vendor = "apple"`. `rustup target add aarch64-apple-darwin` is installed here so `cargo check --target aarch64-apple-darwin` type-checks that path; the full crate fails to *build* for that target in this VM because the fork's C files need Apple headers.
- Sample timing uses `std::time::Instant` (a hardware counter: `CLOCK_UPTIME_RAW` on Darwin, `CLOCK_MONOTONIC` on Linux). A run under thread CPU time once showed a 12% floor shared by three contenders; two experiments cleared the clock and pointed at a ~12 ms core-frequency boost. `--trace-clocks PATH` records wall, thread-CPU, mach ticks, and (Apple) per-core-kind cycles per sample; `tools/analyze-clock-trace.py` reads it. The fork at github.com/johnservil/measure-clocks3 (checkout at `/tmp/measure-clocks3`, credential helper set, needs `cargo +nightly`) has `--pitfall` and `CPU-TIME-CLOCKS-AND-FREQUENCY.md`.
- `rsvg-convert` (librsvg2-bin) is installed for rendering an SVG to PNG to eyeball it: `rsvg-convert -w 1200 file.svg -o out.png`. Earlier SVG test harnesses on the guest disk are gone with a VM restart.
- Commands for the user go on one line, with no `\` continuations.
- Never `sleep` in commands. When a network call fails, report it and stop; the user decides about retries.
- Run long commands (builds, benchmark runs, package installs) without a timeout and let their output stream, so the user can watch progress and interrupt when they choose.
