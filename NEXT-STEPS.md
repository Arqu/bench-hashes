# Next steps

Read this file first. The work: make the servil fork the fastest BLAKE3 in
every situation a user meets (minimax: judge by the worst plausible case),
natively on the Mac and in the VM (both first-class), measured by this
benchmark. Prefer changes that are simpler and faster together. The
principles are in both repositories' `AGENTS.md`; the fork's hardware
facts, design, and rejected ideas are in its `NOTES-servil.md` (read it
before touching kernels or the pool); this repository's are in `NOTES.md`.

## Resume here (checkpoint, September 25, 2026, night)

Zooko is asleep; work autonomously with the usual practices (candidate
branches, the gate, probes on the Mac, notes, commits). In order:

1. **Promote `candidate/sme2-flat`** (fork, pushed, a3aa406): the flat walk
   for whole subtrees of 256 KiB to 1 MiB on SME2, with the 18-chunk
   kernel (16 SME2 lanes + 2 integer chunks; `tools/gen_sme2_hybrid.py`,
   `c/blake3_sme2_hybrid_aarch64.S`, `sme2::compress_subtree_flat`). Done:
   lib suites 70 / 67 / 57; the VM check (hook): no regression, faster
   256 KiB -6.3%, 1 MiB -7.1%, 3 MiB -4.8%. To do: doc tests, vectors,
   bench tests (patched); Mac `perf_regress compare servil
   candidate/sme2-flat` (runner job); update `kernel_report()` (lib.rs,
   about line 1304: add the flat walk from 256 KiB, "SME2 groups with an
   integer lane"); fast-forward, gate note (`git notes --ref perf`), push,
   delete the branch; pin bench-hashes (`cargo update -p blake3-servil`),
   re-record both machines (Mac: runner job, `--all`; rerun if its load
   line says busy), commit, push. Then consider widening the flat walk
   (below 256 KiB; the Hasher's pieces) with measurements.
2. **The `efficient` module** (Zooko approved): `blake3_servil::efficient::
   {hash, hash_many, hash_multithreaded, Hasher}`, same results, tuned for
   E-cores and energy: no SME2 unless measured cheaper, NEON plans without
   the second scalar lane (the "minimax plans", E +16-24%), few threads,
   workers that sleep. The default functions stay P-core first.
3. **An E-core scenario and an energy probe** to judge step 2: the cells at
   background QoS; `proc_pid_rusage` energy (`ri_billed_energy`, nJ),
   checked for reliability first.
4. Then, in Zooko's order of interest: the **SME2 thread** for mt mode (the
   calling thread streams SME2 continuously, integer lanes beside, workers
   NEON; two SME2 units may be reachable: github.io/blake3-sme2 measured
   two SME2 threads at 2x one); the **pipelined streaming hasher** (below);
   a **"for best performance"** section in the API docs and README; the
   single-session tree (chunks, parents, root in one streaming session).

## Where things stand (September 25, 2026)

- **Decisions of the day** (fork AGENTS.md): the recommended usage first
  (one thread makes all calls; shared and misuse measured and reported in
  every benchmark, no longer a veto); `perf_regress` holds a change past 3%
  in any solo cell or 10% in any shared cell; accepted trades need every
  slowed cell ahead of every competitor and Zooko's decision; measure wall
  time and cycles, both, always (`examples/support/clocks.rs`).
- **Findings** (fork NOTES-servil.md): integer work runs beside the SME
  unit for free (job 142); streaming mode lowers the P clock to 3.93 GHz
  from 4.51; the SME unit has a slow state (3.2 cycles per ns) after idle
  time; the 18-chunk kernel is faster on P (-10%) and E (cycles -2%).
- Fork `servil`: release **blake3-servil 0.1.0** (tag `v0.1.0+7fe31c3...`,
  `tools/gen-ver.py`, Zooko's technique), then `Hasher::update_multithreaded`
  (c6d61a6) and docs. bench-hashes: 0.7.0 released, then the **streamed use
  case** (64 KiB pieces, solo and shared; six plots); records on fork
  c6d61a6, both quiet.
- Kept probes: `probe/sme-scalar`, `probe/sme2-hybrid`, `probe/neon-cold`,
  `probe/mixed-parents`, `probe/overlap-old`/`-new`.

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
- **Records** measure the pinned fork commit: after a promotion,
  `cargo update -p blake3-servil` here and commit the lock; the VM's with
  `cargo run --release -- --all` from this directory (unpatched; writes
  `benchmark-results/` here); the Mac's as a runner job naming that fork
  commit, flags `["--all"]`, its files copied into
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

## Decided September 25 (Zooko)

- **The recommended usage first** (fork AGENTS.md): optimize for one
  thread making all calls; misuse and shared machines measured and cared
  for, no longer a veto. To ask: should `perf_regress` report shared-cell
  regressions without stopping? Also to write: a "for best performance"
  section in the API docs and the fork's README.

- The fork builds without SME2 (a warning) when the compiler cannot
  assemble it: `candidate/sme2-optional-build`, gated like any code change.
- README invites results as pull requests (a folder per machine).
- Every run reports other programs' load in its provenance (NOTES.md).

## Idea: a truly streaming (pipelined) hasher (Zooko, September 25)

`Hasher::update` is synchronous: the caller waits while we hash, and our
resources idle while the caller produces the next piece, a pipeline
bubble at every call. A pipelined API buffers between the two: the caller
hands over pieces and returns at once while our threads (the SME2
streamer, NEON workers) hash behind it; `finalize` drains. BLAKE3 suits
this as SHA-256 cannot: every piece's place in the tree is known from its
offset, so pieces hash in parallel and out of order, and only the CV-stack
merge runs in order. Design points:
- Back-pressure: bounded buffers; when full, the producer blocks (simplest,
  the standard bounded-channel answer), or an async form returns Pending.
- Copying: `update(&[u8])` borrows, so hashing after return means copying,
  which on M4 costs about what hashing costs at mt speeds. Zero-copy
  forms: the caller fills our buffers (`buffer() -> &mut [u8]`, then
  `submit(n)`; blocking on `buffer()` is the back-pressure), or hands us
  owned buffers. Buffers of one power-of-two size make every piece a whole
  subtree.
- Contract: multithreaded by nature (another thread hashes); fits the
  "one caller thread, we spread under the hood" recommended usage.
- Benchmark: a use case where the producer does work per piece (a copy
  from a source buffer, as a read would), timed end to end, so the overlap
  shows; synchronous contenders run the same producer.

## Open problems

Each stays open until controlled, explained to users with how to control
it, or at least predicted (AGENTS.md, "we own every slowdown").

1. **2-4 KiB and 2304-4470 B against SHA-256** (the list's winnable part):
   2 KiB is one NEON pair's chain, 3 KiB a pair beside a free scalar
   chunk, 4 KiB two scalars beside a pair (integer-bound); ideas estimated,
   not built: parents and root inside k4 (about 3.6%), a direct small-tree
   path (1-2%); a faster pair chain would move 2-3 KiB.
2. **Benchmarks on hardware they cannot see or steer** (the VM): runs
   report other programs' load from OS counters, but this hypervisor
   reports no steal time, so host load stays invisible in the guest (a
   reference loop timed beside the samples would show it; NOTES.md, "Load
   from other programs"). The host
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
   messages cost 23% more per message than 1024 in a tight loop). Probed
   September 25 (fork NOTES, "SME2 remainders"): the remainder's order is
   not the cause. The SME unit has a slow state (cycles per ns 3.2
   against 3.93) entered after idle time of about a quarter microsecond;
   what else enters it is open (fork NOTES, "SME2 remainders"). The
   overlap group (candidate/overlap-group) trades and waits unpromoted.
   Next: measure the state machine directly (SME2 work, then X ns of
   other work, then SME2 work: speed against X and against the first
   stretch's length, and against the number of streaming sessions), then
   an overlap group inside one streaming session (a kernel entry).
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
