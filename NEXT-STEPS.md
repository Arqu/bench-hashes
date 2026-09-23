# Next steps

Read this file first. The work is optimizing the servil fork for the duo
benchmark on the VM, native Mac, and other platforms. Prefer improvements
that make the implementation simpler and faster together. Shared principles
and environment commands are in both repositories' `AGENTS.md` files.

## Where this session stopped

- Fork: `/workspace`, branch `sme2-bench`, commit **66bc96d**, clean,
  pushed. Branch `sme-only-workers` (pushed) holds an experiment to
  measure on the Mac.
- Benchmark: `/workspace/bench-hashes`, branch `main`, commit **8b8cd89**
  (the VM record) plus this NEXT-STEPS, pushed. No release tag since
  0.6.0.
- The VM record in `benchmark-results/aarch64.linux618520virt/` comes
  from fork 66bc96d and bench d40c0bc, both clean. The Mac record is from
  361bc4d (before the larger axes).

## What this session did

1. **Pool: ranked workers and spin polling** (fork 25e8b86). Idle
   workers calling `sched_yield` slowed the hashing threads by a third on
   the VM, and under back-to-back calls all fifteen stayed awake. Worker
   `r` now takes from a job only `20 ns x r` after registration, so the
   low ranks take a call's pieces and the rest sleep; polls spin and
   yield every 20 us. VM duo servil mt: 64 KiB .158 -> .144, 128 KiB
   .113 -> .097, 8 MiB .056 -> .052; batches of 1024 msgs 11.0 -> 9.8.
2. **`examples/host_lab.rs`** measures every effect behind the pool's
   design on the machine it runs on, in about 6 s (primitives, core
   scaling, idle-waiter interference, the SME2 -> NEON penalty, the pool
   by size, WFE), and writes `host-lab.<seconds>.txt`. **Run it on the
   Mac** (`cargo run --release --example host_lab` in the fork) and keep
   the report beside the VM's.
3. **VMs are first-class targets**, in both AGENTS.md files.
4. **Axes to 128 MiB and 262144 messages**; inputs are counter words;
   golden vectors regenerated (72 + 48); every run writes a samples file
   with a CPU identity line. servil mt levels out from 4 MiB; Rayon still
   falls at 128 MiB.
5. **Performance-regression check** (fork `tools/perf_regress.py`,
   baselines in `perf-baselines/`, pre-commit hook, CI workflow). The
   fork's AGENTS.md makes it mandatory for every code commit. The rule
   was chosen by measurement (0.13% false alarms on unchanged code; see
   the fork's NOTES). A VM baseline is committed; **the Mac needs one**.
6. **Bisect** (`tools/perf_bisect.py`) over 2fd3163..25e8b86: serial
   `hash()` at 16 and 32 KiB was 27-38% slower from b3b4bc8 through
   604abc4 and recovered in 6485bd9; cause not established.
7. Measured and set aside: WFE (a spin on the VM; test natively), plain
   loads instead of `spin_loop` (identical), SME2 workers that never run
   NEON (no gain on the VM; branch `sme-only-workers`).

## Next priorities

1. **On the Mac**: `sh tools/install-git-hooks.sh`, then
   `python3 tools/perf_regress.py record` and commit the baseline; run
   `examples/host_lab.rs`; run `cargo run --release -- --all` here; and
   measure branch `sme-only-workers` against `sme2-bench` there (ABBA).
2. **Why 16-32 KiB serial was slow for seven commits**, so it cannot
   return unseen: reproduce b3b4bc8 against 2fd3163 (their runs are in
   the fork's `tmp/bisect/`) and compare `hash()`'s frame and inlining.
3. **The 512-message cell**: servil mt 12.7 against servil 10.3 ns/msg
   in the benchmark on both machines, though both run the same serial
   code there and no isolated test reproduces the gap.
4. **SME2 kernel with scalar chunks interleaved**: the core's integer
   units sit idle while the SME unit works; the NEON hybrids already do
   this (k10).
5. **Per-process slow states** in SME2 cells (a quarter of VM runs,
   20-45% slower at 16-256 messages): find whether the duo threads'
   placement is the cause and whether the fork can avoid it.
6. `many::TABLE` and the batch split threshold (64 KiB), each against a
   native number.

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
commit: `python3 tools/perf_regress.py check` (the hook runs it).

Use no timeout for long commands; let progress stream. Never sleep in
commands. If a network operation fails, report it and stop; the user
chooses retries. Never print the credential token. Only `/workspace`
survives VM restarts.
