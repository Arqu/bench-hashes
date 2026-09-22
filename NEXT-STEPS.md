# Next steps

Read this file first. The work is optimizing the servil fork for the duo
benchmark on the VM, native Mac, and other platforms. Prefer improvements
that make the implementation simpler and faster together. Shared principles
and environment commands are in both repositories' `AGENTS.md` files.

## Where this session stopped

- Fork: `/workspace`, branch `sme2-bench`, commit **6485bd9**, clean,
  **unpushed**.
- Benchmark: `/workspace/bench-hashes`, branch `main`, commit **fbc3f9a** (plus this
  NEXT-STEPS commit), clean, **unpushed**, no release tag yet (0.6.0 is the last).
  Run `python3 tools/gen-ver.py 0.7.0` and push both repositories once
  the Mac has confirmed the numbers.
- Latest Mac record is still the 17:18 run of 0.6.0 (one plot). The VM
  record in `benchmark-results/aarch64.linux618520virt/` is from the new
  code (`--all`, 80 rounds, both repositories clean).

## What the last two sessions did

**Session A (benchmark only).** ab-blake3 contender (`const_hash` for
one message); the many-messages use case (twenty batch sizes of 64-byte
messages, `POINTS` holds forty points, `Results` is
`Vec<Vec<Option<Cell>>>`, samples divide by messages: ns/msg and
Mmsg/s); two plots in one SVG under one unit switch; `MANY_VECTORS`
golden anchors (SHA-256 of a batch's digests) from the reference
implementation via `tools/gen-test-vectors.py`.

**Session B (fork, then benchmark).** The fork's answer to the use case,
with no new assembly:

1. `blake3_servil::hash_many(inputs: &[&[u8]], outputs: &mut [Hash])`.
   Runs of one-block messages go through `Platform::hash_many::<64>`
   with `CHUNK_START | CHUNK_END | ROOT` at counter zero, the shape the
   parent kernels compress a lane at a time: the SME2 sixteen-lane
   kernel (eight groups per entry into streaming mode, `many::TABLE =
   128`) and the NEON hybrids' p8/p4/p2 + k1. Every other message runs
   `hash()`'s path. `Hash` is `repr(transparent)`.
2. `hash_many_multithreaded` and `_with_budget`: batches of 64 KiB and
   up cut into message ranges over the pool (`Work::Messages`,
   `Pool::run_job` shared with the tree). Same permits, same fairness.
3. `kernel_report_many()` and `kernel_report_many_multithreaded()`.
4. Benchmark: `blake3-servil` calls `hash_many` for a batch,
   `blake3-servil-mt` joins the use case through
   `hash_many_multithreaded`, `mt1` through the budget-one form.

VM `--all` duo, ns per message at 1 / 16 / 64 / 1024 / 16384 messages:
BLAKE3 servil 44 / 18.6 / 9.9 / 12.2 / 10.9; servil mt 44 / 18.8 /
10.0 / 11.1 / **5.7** (180 Mmsg/s); ab-blake3 45 / 22.4 / 22.0 / 22.1 /
22.6; SHA-256 (sha2) 31.5 flat; BLAKE3 44 flat. The fork leads every
contender from two messages up.

Measured and recorded in the fork's `NOTES-sme2-bench.md`: the `TABLE`
run-length effect on the VM (128 → 9.7, 1024 → 12.5 ns/msg; kernel call
length, frame, alignment, and the loops each ruled out), the
uninitialised pointer table (an initialised one cost 90 ns per call),
and an SME2 permit for every message range (within noise; reverted).

## Next priorities

1. **Run `--all` on the Mac.** Both plots; then `gen-ver.py 0.7.0` and
   push fork and benchmark.
2. **`many::TABLE` on the M4.** Entering streaming mode costs about a
   microsecond there against ~65 ns on the VM, so 128 messages per entry
   may be too few; `examples/many_probe.rs` measures it. Change it only
   with a native number.
3. **Equal-length messages other than 64 B** through the batch path: a
   fixed-counter, variable-block-count form of the chunk kernels (the
   NEON kernels already take a block count and last-block length; the
   SME2 chunk kernel fixes sixteen blocks), and a per-call `block_len`
   in the parent kernels for uniform short messages.
4. **Pool hand-off costs** bound both the tree and the batch at
   64–256 KiB (two threads lose to one at 64 KiB); the one-message
   priorities stand: bulk latency, the 64 KiB tail, incremental `Hasher`
   over the pool, SME2 for 2–15 chunks.

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
`pure` 53 + 19); official vectors 2; benchmark 5.

Use no timeout for long commands; let progress stream. Never sleep in
commands. If a network operation fails, report it and stop; the user
chooses retries. Never print the credential token. Only `/workspace`
survives VM restarts.
