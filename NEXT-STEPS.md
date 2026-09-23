# Next steps

Read this file first. The work is optimizing the servil fork for the duo
benchmark on the VM, native Mac, and other platforms; VMs are
first-class targets. Prefer improvements that make the implementation
simpler and faster together. Shared principles and environment commands
are in both repositories' `AGENTS.md` files; the fork's
`NOTES-sme2-bench.md` has every measurement behind the design.

## Where this session stopped

- Fork: `/workspace`, branch `sme2-bench`, commit **f4d0a6c**, clean,
  pushed. Branch `sme-only-workers` (pushed) holds an experiment for the
  Mac (see priorities).
- Benchmark: `/workspace/bench-hashes`, branch `main`, commit **cb478cd**
  (the VM record) plus this NEXT-STEPS, pushed. No release tag since
  0.6.0; the fork has none of its own.
- VM record: fork f4d0a6c, bench f2820fa, both clean, 80.5 s a run.
- Mac record (`benchmark-results/AppleM4Max.darwin25/`): fork c1ec71f,
  bench 04dc84f, both clean (before 16 MiB was dropped; still valid).
- Mac `host_lab` report: the fork's
  `host-lab-reports/M4Max.macos.5825e4d.txt`.
- After a VM restart: `sh /workspace/vm/setup.sh` reinstalls clang-19,
  PyPy, and librsvg2-bin, and the guest's git hooks.

## What this session did

1. **Pool: ranked workers and spin polling** (fork 25e8b86). Idle workers
   polling with `sched_yield` slowed the hashing threads by a third on the
   VM; now worker `r` takes from a job only `20 ns x r` after
   registration, so a small call keeps only the workers it needs awake.
   M4 Max, servil mt duo against the previous record: 64 KiB .193 ->
   .140, 128 KiB .124 -> .089, 256 KiB .101 -> .087, 512 KiB .078 -> .068;
   flat .047 from 16 MiB. Batches: 4.2 ns/msg from 65536 messages
   (238 Mmsg/s, 5.7x ab-blake3, 7.7x SHA-256).
2. **`examples/host_lab.rs`** measures the pool's design assumptions on
   any machine in seconds; reports go in the fork's `host-lab-reports/`.
3. **VMs are first-class optimization targets** (both AGENTS.md).
4. **The axes**: one message per call, 64 B to 128 MiB (23 sizes; 16 MiB
   dropped as redundant); batches of 1 to 262144 64-byte messages (24).
   Inputs are counter words (`seed << 48 | index`); golden vectors
   regenerated from the reference implementation and hashlib. Every run
   writes `bench-hashes.duo.samples.tsv` (every sample in time order, with
   a CPU identity line). `--points` and `--rounds` narrow a run.
5. **Performance-regression check, side by side** (fork
   `tools/perf_regress.py check`, mandatory for every code commit, run by
   the pre-commit hook and in CI): HEAD against the working tree,
   A B B A A B B A, 24 points, 48 rounds, SHA-256 as a same-code control;
   about 40 s plus builds. No false flag in 2400 same-code cell
   comparisons; +10% caught in 95% of cells; a planted +20% found in
   exactly its four cells. Stored per-machine baselines were built,
   measured, and dropped (NOTES has why). Before a release:
   `check --against <previous release>`.
6. **Faster benchmark runs, same accuracy**: long cells (a hash of 4 ms or
   more) are sampled in every 4th round, every 2nd while their median is
   unsure; round counts are a plain 96; a full `--all` went from 150 s to
   80 s, each change checked against run-to-run noise. Rejected with
   numbers: 0.5 ms samples (every median 1.6% slow), halving all rounds (a
   bimodal median moved 35%), dropping 32, 64, or 128 MiB (each carries
   something of its own).
7. **Bisect** (`tools/perf_bisect.py`): serial `hash()` at 16-32 KiB was
   27-38% slower from b3b4bc8 through 604abc4, recovered in 6485bd9;
   cause unknown.
8. **Measured and set aside**: WFE (a spin on the VM and on macOS: it
   returns every 1.3 µs); plain loads instead of `spin_loop` in the pool
   (no difference); SME2 workers that never run NEON (no gain on the VM).
9. **Two SME2 callers at once do not share an SME unit** on the M4 Max
   (host_lab section 6: x1.08-1.13), refuting the first explanation of the
   Mac's two-speed batch cells (priority 1).

## Next priorities

1. **The Mac's two-speed batch cells.** Servil and servil mt, 16 to 8192
   messages: samples 2.0x apart (about 9.5 and 19 ns/msg), 40-45% of them
   slow; NEON contenders never. The slow samples fall in the same rounds
   for every batch cell and follow the preceding contender (15% slow after
   servil mt, 53-70% after single-threaded NEON contenders); one-message
   SME2 cells in those rounds stay fast. Next: one Mac run with
   `--solo --trace-clocks PATH` (per-sample P and E cycles) to see whether
   slow samples ran on E-cores; then what the batch path does differently
   from the tree path (short kernel calls, streaming mode entered per 128
   messages).
2. **On the Mac**: `sh tools/install-git-hooks.sh` in the fork. A/B
   `sme-only-workers` against `sme2-bench`
   (`pypy3 tools/perf_regress.py compare sme2-bench sme-only-workers`):
   the first-NEON-after-SME2 penalty is 3.7 µs natively too, so it may pay
   there. A/B the pool's poll pause against yield-only polling: on the
   Mac, idle `sched_yield` pollers cost hashers 2% and spinners 18%.
3. **Why 16-32 KiB serial was slow for seven commits**:
   `pypy3 tools/perf_regress.py compare 2fd3163 b3b4bc8`, then `hash()`'s
   frame and inlining, so it cannot return unseen.
4. **The 512-message cell**: servil mt's fast mode is 12.2 against
   servil's 9.7 ns/msg on the Mac, same serial code; not reproduced in
   isolation.
5. **SME2 kernel with scalar chunks interleaved**: the core's integer
   units sit idle while the SME unit works; the NEON hybrids already do
   this (k10).
6. **Batches over the pool with two callers**: 2048 messages at 0.83x of
   serial on the Mac. `many::TABLE` and the batch split threshold (64 KiB)
   against native numbers.
7. Release 0.7.0 of bench-hashes (`python3 tools/gen-ver.py 0.7.0`), and a
   first fork release tag, so `check --against <release>` has a base.

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
