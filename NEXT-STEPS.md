# Next steps

Read this file first. The work: make the servil fork the fastest BLAKE3 in
every situation a user meets (minimax: judge by the worst plausible case),
natively on the Mac and in the VM (both first-class), measured by this
benchmark. Prefer changes that are simpler and faster together. The
principles are in both repositories' `AGENTS.md`; the fork's hardware
facts, design, and rejected ideas are in its `NOTES-servil.md` (read it
before touching kernels or the pool); this repository's are in `NOTES.md`.

## Where things stand (September 24, 2026)

- Fork `/workspace`, branch **`servil`** (2a82c8c and later docs), clean,
  pushed; no candidate branches pending. Gate verdicts of every promotion:
  `git notes --ref perf show <commit>`.
- Benchmark `/workspace/bench-hashes`, branch `main`, clean, pushed.
  Records for both machines (e16e836): thorough `--all` runs of fork
  2a82c8c, the Mac's through the runner.
- **The minimax list** (`pypy3 tools/losses.py <samples.tsv>` in the fork):
  50 cells on each machine, every one lost to SHA-256 or SHA-256 ring: one
  message to 4 KiB (solo and shared, servil and servil mt), 2304, 3839,
  4470 B, and a batch of one message. Servil solo against SHA-256 ring on
  the Mac: 2 KiB 1.53x slower, 3 KiB 1.11x, 4 KiB 1.06x, 2304 B 1.47x,
  3839 B 1.11x, 4470 B 1.18x; 7935 B 0.89x and 8 KiB 0.86x (faster). Up to
  2 KiB this is structural (NOTES: a chunk's dependency chain).

## How to work

- **VM setup** after a restart: `sh /workspace/vm/setup.sh` (clang-19,
  pypy3, rsvg, the guest's pre-commit hook). Node and npm for the graph
  check: `apt-get install -y nodejs npm`.
- **Every `git` and `cargo` command** in the VM takes
  `HOME=/workspace/vm/home CARGO_TARGET_DIR=/tmp/target CC=clang-19 TMPDIR=/tmp`,
  `git commit` included (the hook builds; without `CC` it aborts the
  commit and leaves the branch where it was).
- **The gate to `servil`** (fork AGENTS.md "Branches"): work on
  `candidate/<topic>`; every suite; `perf_regress` on the VM (the hook, or
  `compare servil candidate/<topic>`) and on the Mac (a runner job); a
  fast-forward; the verdicts as a git note. Changes that trade one cell for
  another go to the user with their numbers.
- **The Mac** (fork `tools/runner/README.md`): the user starts the runner
  with `sh ~/piplayground/blake3-servil/tools/runner/setup-mac.sh`. Write
  `runner/jobs/NNN-name.json` naming pushed commits; wait with
  `pypy3 tools/runner/wait_for.py NNN-name`; results in
  `runner/results/`. Keep the VM idle while a Mac job runs. Mac-only
  measurements (cycles by core kind, QoS) go in a `probe/<topic>` branch
  that replaces `examples/host_lab.rs` (fork NOTES, "Probes on the Mac").
- **Records**: the VM's from this directory with
  `cargo run --release -- --all --thorough` (writes `benchmark-results/`
  here); the Mac's as a runner job, its files copied into
  `benchmark-results/AppleM4Max.darwin25/`. Before committing, run the
  graph check on both graphs and the list script on both samples files.
- **The graph's script**: `tools/graph-check/README.md` (jsdom harness,
  snapshots to render with `rsvg-convert` and look at).
- **Golden vectors** come from `tools/gen-test-vectors.py` (reference
  implementation and hashlib); a new benchmark size needs its vector
  there, and a regeneration that changes existing lines is a review item.

## Decisions made (don't re-ask)

- Contenders: at most two settings each (single-threaded, multithreaded
  uncapped); two scenarios (solo; shared = two copies of itself); wall
  time for everyone; tables per scenario; the text report keeps KERNELS;
  user views omit maintainer detail.
- One SME2 call at a time per process (fork 30c599b): taken, costs in
  shared cells accepted.
- k8 as two scalars beside a quad and a pair (P -16%, E +7% at 8 KiB):
  taken. k4 as two pairs and the "minimax" plans: rejected.
- Branch naming `candidate/<topic>`; no promotion without the Mac verdict.
- The Mac runner is launched manually by the user; code from GitHub only.

## Open problems

Each stays open until controlled, explained to users with how to control
it, or at least predicted (AGENTS.md, "we own every slowdown").

1. **2-4 KiB and 2304-4470 B against SHA-256** (the list's winnable part):
   2 KiB is one NEON pair's chain, 3 KiB a pair beside a free scalar
   chunk, 4 KiB two scalars beside a pair (integer-bound); ideas estimated,
   not built: parents and root inside k4 (about 3.6%), a direct small-tree
   path (1-2%); a faster pair chain would move 2-3 KiB.
2. **Benchmarks on hardware they cannot see or steer** (the VM): the host
   places vCPUs on P- or E-cores at will; cells come out two-speed with
   run-to-run splits. Round-by-round pairing and two-speed reporting exist;
   to weigh: inferring each sample's core kind from a reference loop timed
   beside it, extending runs until each speed's share is known.
3. **P/E classification of every sample on the Mac** (the counters exist
   in `--trace-clocks`): tables from P-core samples, E shares in the
   maintainer report, `perf_regress` P against P.
4. **Judging two-speed changes**: `perf_regress` reports each cell's 90th
   percentile but judges the 5th; the turn got no verdict because it moves
   the control. A rule for such changes is open.
5. **Shared cells are coin tosses** under the turn (which copy holds it):
   records of identical code differ by up to 60% in shared small batches
   on the VM. Predict or control.
6. **NEON goes cold** after stretches without vector work (1000 one-block
   messages cost 23% more per message than 1024 in a tight loop).
7. **SME2 batch rates with work between calls** (about 12 ns/msg, not the
   benchmark's 10): whether batches should use SME2 from 16 messages.
8. **The E-core trigger's mechanism** (controlled by the turn; unexplained).
9. Later: `tools/promote.py` (check the gate, write the note, fast-forward;
   a pre-push hook refusing a `servil` tip without both verdicts); the
   Mac's serial 128 MiB rise; `many::TABLE` natively; release 0.7.0 of
   bench-hashes and a first fork tag; a GPU kernel.

## Commands

From `/workspace` in the VM, each with the prefix above:

    cargo test --release --lib [--features no_sme2 | --features pure]
    cargo test --release --doc
    cargo test --release --manifest-path test_vectors/Cargo.toml
    cargo test --release --manifest-path bench-hashes/Cargo.toml
    pypy3 tools/perf_regress.py check | compare OLD NEW
    cargo run --release --example host_lab

Expected: 67 / 66 / 56 library tests, 19 doc tests, 2 vectors, 7 benchmark
tests. Release: `python3 tools/gen-ver.py X.Y.Z` from a clean tree (two
version commits and a lightweight tag; push `main`, then the tag by name).
Never print the credential token (`/workspace/ghtokenclassic.txt`). Only
`/workspace` survives VM restarts. Commands for the user go on one line.
