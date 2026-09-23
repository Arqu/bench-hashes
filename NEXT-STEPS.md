# Next steps

Read this file first. The work is optimizing the servil fork for the duo
benchmark on the VM, native Mac, and other platforms; VMs are
first-class targets. Prefer improvements that make the implementation
simpler and faster together. Shared principles and environment commands
are in both repositories' `AGENTS.md` files; the fork's
`NOTES-sme2-bench.md` has every measurement behind the design.

## Where this session stopped

- Fork: `/workspace`, branch `sme2-bench`, commit **737929b**, clean,
  pushed. Branch `sme-only-workers` is superseded (the pool no longer
  runs SME2 at all); keep it only for its SME2-only subtree code.
- Benchmark: `/workspace/bench-hashes`, branch `main`, commit **792d406**
  (the VM record) plus this NEXT-STEPS, pushed. No release tag since
  0.6.0; the fork has none of its own.
- VM record: fork 737929b, bench 40247da, both clean.
- Mac record (`benchmark-results/AppleM4Max.darwin25/`): fork c1ec71f,
  bench 04dc84f; predates this session's pool change.
- Raw data and probes from this session: the fork's `tmp/mac-trace/`,
  `tmp/mac-trace2/` (per-copy duo trace), `tmp/mac-neon/` (no_sme2 run),
  `tmp/sme2-session/` (probes, the one-thread patch).

## What this session did

1. **Minimax strategy** in both AGENTS.md: judge a design by its worst
   plausible case; mt slower than st, or a bigger task more than
   proportionally slower, is a defect.
2. **The Mac's two-speed batch cells, solved.** `--trace-clocks` now
   records each duo copy's P/E counts (bench 5cfba2f; 40247da makes the
   provenance follow a commit made right after a hooked build). In every
   slow sample both copies ran on P-cores at full clock, each taking twice
   the cycles: the two threads shared one P-cluster's SME unit. E-cores
   refuted. Rounds without Rayon or the pool let macOS consolidate the
   process onto one cluster.
3. **mt slower than st at 512 messages; st slower at 1024+ than at
   512.** Both are one hardware effect: core work between SME2 calls (any
   kind: a scan, an ALU loop) makes the calls ~25% slower; NEON is
   immune. mt's one pass over the lengths and the benchmark's per-sample
   allocations are that work. Code identical; no cheap fix (NOTES).
4. **The pool runs on NEON alone** (bf12b5e): SME2 permits, the unit
   count, and the 40 ms Linux measurement gone; `initialize()` 0.5 ms.
   VM: mt 256 KiB -16 to -20%, 1 MiB -10%, 4096 messages -11 to -16%.
   Mac duo evidence from the no_sme2 run; Mac solo unmeasured.
5. **Serial SME2 by case.** Case 1 (quiet) and case 2 (other work on
   other cores, measured: 0.185-0.22 beside 15 busy threads vs 0.170
   alone) favour SME2 over NEON (0.25). Case 3 (another thread on the
   same unit) is 2-5x worse than NEON (`examples/scaling.rs`, n=16:
   0.87-0.94 ns/B per thread, slowest 1.2-1.35; NEON 0.31-0.33).
   Tried: one SME2 thread per process (loses duo), two permits (slowest
   0.84 at n=16), pacing against NEON (0c206a3, reverted in 737929b: it
   fixed the scaling probe but locked both duo copies of the full
   benchmark into a mode slower than NEON in 3 of 4 runs). The benchmark's
   duo on the VM is itself case 3: in a full run ~60% of serial cells from
   64 KiB sit at 0.31 (unit shared), the rest at 0.17.
6. **History review:** no optimisation lost (NOTES lists what was
   checked).
7. **Set aside with numbers:** two scalar chunks for 2 KiB (0.587 vs
   0.455), a 32 KiB split with NEON pieces, 64/256 KiB longest pieces.

## Next priorities

1. **Decide serial SME2 under case 3** (the user's call). The options and
   their per-thread worst cases are in NOTES ("Serial SME2, by case"):
   keep SME2 (best at 1-2 threads, 3-5x worse than NEON at 4+ threads
   of one program), NEON only (flat 0.25-0.33, 47% slower alone), or a
   better detector than pacing (it must never lock into a slow state;
   judge on several full `--all` runs, not the three-contender check).
2. **Mac, first thing:** `sh tools/install-git-hooks.sh`, then
   `pypy3 tools/perf_regress.py compare 8f3bde3 737929b` (the pool on
   NEON, natively, solo and duo), `cargo run --release --example scaling`
   and `cargo run --release --features no_sme2 --example scaling` (case 3
   natively), then a fresh Mac record: from `bench-hashes`,
   `git checkout -- benchmark-results && cargo run --release -- --all`.
3. **The Mac's serial 128 MiB rise** (0.176 -> 0.210): flat on the VM.
4. **A/B the pool's poll pause against yield-only polling on the Mac**
   (idle `sched_yield` pollers cost hashers 2% there, spinners 18%).
5. **Batches over the pool with two callers**; `many::TABLE` natively.
6. Release 0.7.0 of bench-hashes and a first fork tag.
7. Future work (NOTES): a GPU kernel.

## Commands

From `/workspace` in the VM (every `git`/`cargo` command takes this `HOME`):

`HOME=/workspace/vm/home CARGO_TARGET_DIR=/tmp/target CC=clang-19 TMPDIR=/tmp cargo test --release`

`HOME=/workspace/vm/home CARGO_TARGET_DIR=/tmp/target CC=clang-19 TMPDIR=/tmp cargo test --release --features no_sme2`

`HOME=/workspace/vm/home CARGO_TARGET_DIR=/tmp/target CC=clang-19 TMPDIR=/tmp cargo test --release --features pure`

`HOME=/workspace/vm/home CARGO_TARGET_DIR=/tmp/target CC=clang-19 TMPDIR=/tmp cargo test --release --manifest-path /workspace/test_vectors/Cargo.toml`

`HOME=/workspace/vm/home CARGO_TARGET_DIR=/tmp/target CC=clang-19 TMPDIR=/tmp cargo test --release --manifest-path /workspace/bench-hashes/Cargo.toml`

Benchmark runs write `benchmark-results/` relative to the **current
directory**; run them from `/workspace/bench-hashes` so results land in
the repository:

`cd /workspace/bench-hashes && HOME=/workspace/vm/home CARGO_TARGET_DIR=/tmp/target CC=clang-19 TMPDIR=/tmp cargo run --release -- --all`

On the Mac, from `bench-hashes`: `cargo run --release -- --all` (add
`--thorough` for narrower bands). Results overwrite that machine's files.
Commit before publishing so the provenance reads `clean`, and measure on
one machine at a time: the VM shares the Mac's cores, so a run on one
slows the other (seen: the control x1.56 slower while the host was busy).

Release: `python3 tools/gen-ver.py X.Y.Z` from a clean tree makes two
version commits and a lightweight tag `vX.Y.Z+<commit>`; push with
`git push origin main` and then the tag by name (`--follow-tags` skips
lightweight tags).

Expected suites: fork 63 library + 19 doc tests (`no_sme2` 62 + 19,
`pure` 53 + 19); official vectors 2; benchmark 5. Before any fork code
commit: `pypy3 tools/perf_regress.py check` (the hook runs it; exit 1
aborts, see the fork's AGENTS.md). Two commits side by side:
`pypy3 tools/perf_regress.py compare OLD NEW`. The platform facts behind
the pool: `cargo run --release --example host_lab` in the fork.

Use no timeout for long commands; let progress stream. Never sleep in
commands. If a network operation fails, report it and stop; the user
chooses retries. Never print the credential token. Only `/workspace`
survives VM restarts.
