# Next steps

Read this file first. The work is optimizing the servil fork for the duo
benchmark on the VM, native Mac, and other platforms. Prefer improvements
that make the implementation simpler and faster together. Shared principles
and environment commands are in both repositories' `AGENTS.md` files.

## Where this session stopped

- Fork: `/workspace`, branch `sme2-bench`, commit **c1ec71f**, clean,
  pushed. Branch `sme-only-workers` (pushed) holds an experiment to
  measure on the Mac.
- Benchmark: `/workspace/bench-hashes`, branch `main`, commit **56e175d**
  (the VM record) plus this NEXT-STEPS, pushed. No release tag since
  0.6.0.
- The VM record comes from fork c1ec71f and bench 809861f, both clean.
  The Mac record is from 361bc4d; the user is running a new one.

## What this session did

1. **Pool: ranked workers and spin polling** (fork 25e8b86): idle workers
   no longer steal time from the ones hashing. VM duo servil mt: 64 KiB
   .158 -> .144, 128 KiB .113 -> .097, 8 MiB .056 -> .052.
2. **`examples/host_lab.rs`**: the pool's design assumptions measured on
   any machine in 6 s. Run it on the Mac and keep the report.
3. **VMs are first-class targets** (both AGENTS.md files).
4. **Axes to 128 MiB and 262144 messages**, counter-word inputs,
   regenerated golden vectors, a samples file per run.
5. **Performance-regression check, side by side** (fork
   `tools/perf_regress.py check`): builds HEAD and the working tree and
   runs them A B B A A B B A; SHA-256 as a same-code control; about 40 s
   plus builds. Calibrated: no false flag in 2400 same-code cell
   comparisons; +10% caught in 95% of cells. Mandatory for every code
   commit (fork AGENTS.md); the pre-commit hook runs it. Stored baselines
   were built, measured, and dropped (NOTES has why).
6. **Faster benchmark runs**: a time budget for cells whose hash takes
   4 ms or more; a full `--all` from 150 s to 80 s, medians inside
   run-to-run variation. 0.5 ms samples were tried and rejected (every
   median 1.6% slow). `--points` and `--rounds` narrow a run.
7. **Bisect**: serial `hash()` at 16-32 KiB was 27-38% slower from
   b3b4bc8 through 604abc4, recovered in 6485bd9; cause unknown.

## Next priorities

1. **On the Mac**: `sh tools/install-git-hooks.sh` in the fork; run
   `examples/host_lab.rs`; A/B branch `sme-only-workers` against
   `sme2-bench` (`pypy3 tools/perf_regress.py compare sme2-bench
   sme-only-workers`).
2. **Why 16-32 KiB serial was slow for seven commits**: compare
   b3b4bc8 and 2fd3163 side by side, then `hash()`'s frame and inlining.
3. **The 512-message cell**: servil mt 12.7 against servil 10.3 ns/msg
   in the benchmark on both machines, same serial code, not reproduced
   in isolation.
4. **SME2 kernel with scalar chunks interleaved** (the core's integer
   units idle while the SME unit works).
5. **Per-process slow states** in SME2 batch cells (a quarter of VM
   runs, 20-45% slower at 16-256 messages).
6. `many::TABLE` and the batch split threshold against native numbers.

## Latest Mac record

`--all` at fork 604abc4, the first native run of the simplified pool.
Servil mt duo medians, 64 KiB through 8 MiB in the usual nine-size order:
`.193, .124, .101, .078, .063, .054, .052, .050, .048` ns/B. The 15:21
record at fork `2ce77d7`/dirty was `.203, .134, .107, .078, .063, .055,
.053, .051, .049`: 64–256 KiB improved 5–6%, bulk is level. The three
marked cells belong to Rayon. Servil mt's 64 KiB range is `.160–.327`,
wider than its neighbours; that cell and bulk latency remain the targets.

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
`--thorough` for narrower bands). Results overwrite that machine's files;
copy a baseline aside before a comparison. Commit before publishing so the
provenance reads `clean`.

Release: `python3 tools/gen-ver.py X.Y.Z` from a clean tree makes two
version commits and a lightweight tag `vX.Y.Z+<commit>`; push with
`git push origin main` and then the tag by name (`--follow-tags` skips
lightweight tags).

Expected suites: fork 63 library + 19 doc tests (`no_sme2` 62 + 19,
`pure` 53 + 19); official vectors 2; benchmark 5. Before any fork code
commit: `pypy3 tools/perf_regress.py check` (the hook runs it).

Use no timeout for long commands; let progress stream. Never sleep in
commands. If a network operation fails, report it and stop; the user
chooses retries. Never print the credential token. Only `/workspace`
survives VM restarts.
