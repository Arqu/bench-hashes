# Next steps

Read this file first. The work is optimizing the servil fork for the duo
benchmark on the VM, native Mac, and other platforms. Prefer improvements
that make the implementation simpler and faster together. Shared principles
and environment commands are in both repositories' `AGENTS.md` files.

## Where this session stopped

- Fork: `/workspace`, branch `sme2-bench`, commit **04c3394**.
- Benchmark: `/workspace/bench-hashes`, branch `main`; this handoff ships
  with the golden-vector checks, documentation cleanup, and latest Mac
  graph/text record. Use `git log -1` for its commit.
- VM currently has **16 vCPUs**. Inspect `nproc` after a restart; run
  `sh /workspace/vm/setup.sh` to restore the toolchain environment.
- The next priority is interpreting and repeating the latest Mac run,
  then improving bulk latency without restoring complicated coordination.

### Fork changes retained

1. Pieces reuse the one-shot subtree code, with explicit key, counter,
   flags, and platform. The duplicate owned-mode enum and per-piece
   incremental Hasher setup are gone.
2. The caller merges CVs through SIMD a level at a time, in place. The
   old recursive merger remains only as a structural test oracle.
3. One active-thread count enforces each call's budget and signals its
   completion. Reservations are atomic, fixing the old cap-check race.
   The caller clears its slot and drains readers before waiting for zero;
   this ordering is essential to the raw job-pointer lifetime proof.
4. Global admission happens once per call. A call arriving when callers
   already fill the CPUs hashes its input whole, sharing the same SME
   permits as workers. The pool-wide busy count is gone. The fixed pool
   and caller threads may overlap; **the old whole-machine thread-count
   ceiling is no longer the contract**. Each call's explicit budget remains
   exact. A single-CPU call now works; the old merge could panic there.
5. Removed redundant unsafe Send/Sync declarations, unused permit-total
   state, and a racy global quiet-state test. ARM-specific diagnostic
   examples now build on configurations without NEON, including `pure`.

The shrinking 8–128 KiB schedule, 64 KiB split threshold, SME permit
policy, and yielding/sleeping mechanism remain. Equal-size pieces, extra
cache-line padding, a live-slot bitmap, retained CPU reservations, and
fixed-width recursion failed to justify their cost or complexity.
Details and measurements are in `/workspace/NOTES-sme2-bench.md`.

### What measurements establish

Two baseline and two candidate runs through the same updated benchmark,
ABBA order, 240 rounds each, with serial/mt/mt·1 selected:

| Input | Baseline mt (`2ce77d7`) | Candidate mt |
|---|---:|---:|
| 64 KiB | .174–.175 | .161–.162 |
| 128 KiB | .125–.126 | .113–.115 |
| 256 KiB | .108 | .095–.096 |
| 512 KiB | .089 | .080–.081 |
| 1 MiB | .074–.076 | .069–.073 |
| 8 MiB | .055–.056 | .055–.059 |

Values are duo ns/B, ranges across runs. **64–512 KiB improves about
7–12% with disjoint 95% median bands.** Bulk results show between-run
variation; an established bulk gain remains open. The final permit-policy
refactor's check was .161/.114/.096/.080/.069/.061/.060/.059/.056 across
64 KiB–8 MiB, all median intervals narrower than 5%. mt·1 tracks serial.
The --all VM run still leads other algorithms from 64 KiB upward.

One-/two-CPU affinity diagnostics and 2–32-caller diagnostics were also
run. Single-CPU correctness is fixed; constrained-CPU and high-caller
results are promising. Treat diagnostic examples as probes, not formal
confidence-band measurements.

Raw logs, snapshots and SVGs persist under `/workspace/tmp/mt-session/`:
`checked-baseline-*`, `checked-candidate-*`, `review-final.*`,
`checked-all.*`, and `many-{baseline,candidate}.log`. Binaries under `/tmp`
are disposable. `examples/many.rs` is now committed.

## Latest Mac record: inspect this first

`benchmark-results/AppleM4Max.darwin25/bench-hashes.duo.{result.txt,graph.svg}`
was updated on the host during wrap-up, timestamp **2026-09-22 15:21:35 UTC**.
The graph and text are preserved together. This run passed the golden
checks and recorded the dirty fork fingerprint
`e776ddc89e599403d809ab9ac21fea22ab1f031a0b55f4b88baba5c589409ee3`
on base `2ce77d7`; retain that provenance as historical evidence.

Servil mt medians, 64 KiB through 8 MiB in the usual nine-size order:
`.203, .134, .107, .078, .063, .055, .053, .051, .049` ns/B.
The mt bands are narrow; its four marked cells belong to Rayon.
The earlier 13:05 host run was `.198, .133, .107, .081, .067, .056,
.053, .051, .049`. Native 512 KiB and 1 MiB look promising; 64 KiB
needs attention, and bulk is similar. Repeat native A/B runs and inspect
bands before attributing the differences to code.

On the host, from the fork checkout:

`cargo run --release --manifest-path bench-hashes/Cargo.toml -- --thorough --contenders blake3-servil,blake3-servil-mt,blake3-servil-mt1`

Then run `--all`. Results overwrite that machine's files, so preserve
baseline artifacts before a comparison. The graph defaults to GB/s and
labels its logarithmic axis; the toggle also shows ns/B.

## Correctness policy and benchmark checks

The benchmark now has **64 fixed known-input/known-output vectors** in
`src/test_vectors.rs`. Inputs come from a frozen deterministic RNG, both
seeds, every timed size, and twelve additional boundary/empty lengths.
Golden BLAKE3 digests were generated with the upstream reference code;
SHA-256 and SHA-1 with Python hashlib. The generator records the reference
source hash and never uses the optimized servil implementation.

Before calibration, each selected implementation checks the same bytes
against those golden digests. Multithreaded entries also check simultaneous
calls. `hash_batch` is the single dispatch used for checking and timing;
its monomorphized callback asserts or black-boxes the digest. Timed duo
copies still use separate, differently seeded buffers. A failed check
stops the run and names the implementation, family, length, seed and digests.

Regeneration is explicit: from the benchmark repo,
`python3 tools/gen-test-vectors.py > src/test_vectors.rs`.
Review changes; tests/builds never regenerate expectations automatically.

**Fixed vectors can test every execution mode**, including thread budgets
and concurrent scheduling. Do not conflate fixed input data with fixed
execution order. Existing fork differential/reference tests remain useful;
further golden-vector expansion in the fork is follow-up work. Preserve
published vectors and independent expected answers when extending them.
The benchmark calls public entry points and kernel reports; it supplies
no private pool, worker cap, or tuning environment to improve a score.

## Validation and commands

Latest suites pass:

- Fork default: 56 library + 15 doc tests.
- `no_sme2`: 55 + 15; `pure`: 46 + 15, including example builds.
- Rayon library: 57; debug library: 56; no-default-features check passed.
- Benchmark: 3 tests, including all available implementations on all
  golden vectors, published empty-input hashes, and mismatch diagnostics.
- Single-CPU hash/mode test and two-CPU concurrent-caller test passed.

In the VM, from `/workspace`:

`HOME=/workspace/vm/home CARGO_TARGET_DIR=/tmp/target CC=clang-19 TMPDIR=/tmp cargo test --release`

`HOME=/workspace/vm/home CARGO_TARGET_DIR=/tmp/target CC=clang-19 TMPDIR=/tmp cargo test --release --features no_sme2`

`HOME=/workspace/vm/home CARGO_TARGET_DIR=/tmp/target CC=clang-19 TMPDIR=/tmp cargo test --release --features pure`

`HOME=/workspace/vm/home CARGO_TARGET_DIR=/tmp/target CC=clang-19 TMPDIR=/tmp cargo test --release --manifest-path /workspace/bench-hashes/Cargo.toml`

`HOME=/workspace/vm/home CARGO_TARGET_DIR=/tmp/target CC=clang-19 TMPDIR=/tmp cargo run --release --manifest-path /workspace/bench-hashes/Cargo.toml -- --all`

Use no timeout for long commands; let progress stream. Never sleep in
commands. If a network operation fails, report it and stop; the user
chooses retries. Every git/cargo command in this VM uses the HOME above.
Never print the credential token. Only `/workspace` survives VM restarts.
