# Next steps

Read this file first. The work: make the servil fork the fastest BLAKE3
in every situation a user meets (minimax: judge by the worst plausible
case), on the Mac natively and in the VM (both first-class), measured by
this benchmark. Prefer changes that are simpler and faster together.
Principles are in both repos' `AGENTS.md` (read them: minimax, "we own
every slowdown a user could meet", presentation costs, branches); every
measurement is in the fork's `NOTES-servil.md` and this repo's `NOTES.md`.

## For the user this morning (the night of September 23-24, 2026)

The night's objective, from the user: shrink the list of cells where a
competitor beats servil or servil mt; `pypy3 tools/losses.py SAMPLES.tsv`
in the fork prints it (score = cells lost by more than 3%). Final servil
(6aa2053): Mac 37 (job 082: SHA-256 at every one-message size to 4 KiB
and a batch of one, plus one two-speed servil mt/servil coin toss; no
BLAKE3 wins anywhere); VM 43 (SHA-256 alone: those, plus seven shared
bulk cells that candidate 1 removes). Details and numbers: the fork's NOTES-servil.md,
section "Session: kernels by core kind, the E-core trigger, the minimax
list".

**Landed on servil** (each through the whole gate, verdicts in
`refs/notes/perf`): k7 and k9 + k3 for seven and twelve chunks (faster on
P, E, and the VM); arrays sized for sixteen chaining values in hash() at
2-16 KiB (Mac 2-4 KiB 4-5% faster; 4 KiB solo now within 2% of SHA-256
ring); servil mt skips its length pass for batches of fewer than 1024
one-block messages (512 messages: 12.40 -> 9.86 ns/msg, level with servil,
off the list on both machines); hash() to 1 KiB reads a full final block
in place (64 B -1.3 to -1.9%, now ahead of ab-blake3 there; 128 B-1 KiB
+0.0 to +0.7%). Tools: the runner, `wait_for.py`, `losses.py`, the hook fixes (an
empty commit, a227f6d, came from the hook overwriting the index).

**Decisions waiting for the user** (each a candidate branch):

1. **Decided and landed** (30c599b, the user's decision on September 24;
   the flag became a plain load and store, since the swap cost solo
   batches of 24 messages 3-9%). Was `candidate/one-sme2-call`: one SME2 call at a time per process; a
   concurrent call runs NEON. Controls open problem 4: solo samples on
   E-cores 3.7% -> 0.0% (every contender). Solo cells unchanged. Shared
   cells lose the lucky mode (both copies on their own SME unit: 1 MiB
   0.178 -> 0.224, batches 128+ +25-50%) and gain the unlucky one
   (batches 16-64: 18.9 -> 12.3, -35%); worst case bounded by NEON. The
   list: 38 -> 41 (servil mt against servil in shared batches of 32-64,
   which copy gets SME2 being a coin toss). Runner jobs 060-062.
   Rebased onto servil (d6e6c18, with the batch change of fc8cc85: the
   prefix scan takes the turn too); suites pass. `perf_regress` gives no
   verdict on it on either machine: the change moves the control (SHA-256
   in the same process) on the new side. A decision here also needs a
   rule for judging it, e.g. worst speed per cell, or E-core share.
   Four more thorough Mac runs, servil / turn / turn / servil (jobs
   069-072): list 36, 37, 38, 36; small solo samples on E-cores 5.1%,
   0.0%, 0.0%, 4.4%; servil's shared cells, median / 90th percentile:
   16 KiB-8 MiB 0.18-0.22 / 0.18-0.24 with servil (both runs drew the
   mode where the copies hold separate SME units; earlier records drew
   the shared-unit mode, 0.31-0.35, in 30-70% of rounds) and 0.19-0.23 /
   0.27-0.30 with the turn; 64 MiB 0.241 against 0.172; batches, 90th
   percentile, 18.9-21.2 against 14.7-17.7. The turn trades the lucky
   mode's speed for a bounded worst case, and ends the E-core placement.
   **On the VM it wins outright** (thorough runs servil / turn / turn /
   servil, in the fork's tmp/vm-thorough-*): list 43, 36, 37, 43. There
   the two SME2 copies nearly always share one unit: servil shared 64
   KiB-8 MiB 0.311-0.316 ns/B, behind SHA-256 ring's 0.30 (7 lost cells);
   with the turn, median 0.173-0.218, 90th percentile 0.263-0.288, and
   those cells leave the list. Recommendation: take it (VM clearly better,
   Mac worst cases better, Mac lucky mode worse; a native/VM split that
   needs the user's decision on the record, per AGENTS).
2. `candidate/neon-k4-pairs`: k4 as two NEON pairs. E-core 4 KiB -22%
   (beats upstream there), P-core 4 KiB +14%, which puts 4 KiB back on
   the list against SHA-256 ring. By the list: no.
3. `candidate/neon-plans-minimax`: k4 and k6 without a second scalar
   chunk, 14 and 16 chunks on k9 + k5 / k9 + k7. mt 3-8 MiB -6 to -13%,
   mt 256 KiB-1 MiB +3.5 to +10.5%, 4 KiB as in 2. By the list: no.
   (2 and 3 predate two commits on servil; rebase before any use.)

**What the list is made of.** Of the 37-38 cells, about 30 are single
messages to 2 KiB and a batch of one: one BLAKE3 chunk is a dependency
chain of at least 168 cycles per 64-byte block (2.6 cycles per byte) on
this hardware, against hardware SHA-256's 1.4-1.6; folding G's rotations
into its xors was 7-13% slower (an xor with a rotated operand takes two
cycles). 3 KiB (SHA-256 ring +11-17%) is bound by the NEON pair kernel; at
zero overhead it would gain 4%. 4 KiB sits within 2-9% of SHA-256 ring
depending on the run. On the VM the list is 42 cells: the same, plus
servil mt against servil at 512 messages (fixed since) and in a few
two-speed shared cells. Candidates 1-3 predate the last servil commits:
rebase before any use.

## Where things stand (end of the September 23, 2026 session)

- **Fork** `/workspace`: main branch **`servil`** (renamed from
  `sme2-bench`; the old branch is deleted), clean, pushed. New work goes
  on **`candidate/<topic>`** branches; the gate to `servil` is in the
  fork's AGENTS.md ("Branches"): fast-forward, all suites, and
  `perf_regress compare servil candidate/<topic>` passing on the VM **and**
  natively on the Mac. Never promote on the VM verdict alone.
  Tag `experiment/sme-only-workers` (100afbc) keeps `sme2::subtree_cv`.
- **Benchmark** `/workspace/bench-hashes`, branch `main`, clean, pushed.
  Records: VM thorough (fork 90172ae, bench 8e0be9f era); Mac quick
  (fork 90172ae, bench d4afe72). No new thorough Mac record: it would
  show the open two-speed problems (below); runs kept in `tmp/`:
  `mac-thorough-90172ae/`, `mac-thorough-30f6776/`, `mac-trace-1683ebb/`
  (with `trace-ecore.csv`). The fork's `tmp/sme2-session/` has the A/B
  data (`mac-ab/`) and the probes.
- **Node 18** was installed in the VM (apt, lost on restart) to check the
  graph's script: `node --check` on the extracted script, and a stub-DOM
  run (see "Checking the graph script" below).

## What this session did

1. **Principles** added to both AGENTS.md: minimax; presentation (every
   item costs the reader; maintainer information stays out of user
   views); "we own every slowdown a user could meet" (control it, tell
   users how to control it, or at least predict it; until then it stays
   open and blocks a no-regression claim); the candidate-branch gate.
2. **Fork: the pool runs on NEON alone** (bf12b5e): permits, SME-unit
   count, and the 40 ms Linux measurement gone. mt faster on the VM and
   the Mac (Mac shared 1 MiB .057 -> .050, 4096 messages 5.9 -> 5.4).
   Pacing SME2 against NEON was tried and reverted (737929b): it locked
   duo copies into a mode slower than NEON. `examples/scaling.rs` added.
3. **Benchmark revamp** (8e0be9f onward): every run measures **solo**
   and **shared** (two copies at once, each copy's own time a sample);
   wall time only (cycle normalisation hid SME2 slowdowns); quick run by
   default (below 1 MiB and 10,000 messages, 24 rounds, SHA-1DC only when
   named, ~12 s), `--thorough` (every point, 96 rounds, ~150 s); the
   `mt·1` contender and `--solo` removed; outputs `bench-hashes.*` (the
   text report is the maintainer report, the SVG the user report, the
   samples TSV v2 has a scenario column); the report leads with one table
   per scenario and use case, then CHECKS, TWO SPEEDS, KERNELS (keep
   them), PROVENANCE.
4. **Two-speed cells** (30f6776): a cell whose samples split (gap >= 4%,
   >= 10% each side, medians >= 1.25x apart) reports both speeds with
   equal weight: `a|b` in tables, a TWO SPEEDS section for servil, a
   forked line (two equal lines, dots, bands, `a | b` labels) and both
   speeds in the graph's hover.
5. **CHECKS compare round by round** (1683ebb): each sample records its
   round; a check pairs two cells' samples of the same round (copy with
   copy when shared) and judges the worse ratio where the ratios split,
   5% or more with the interval above 1. Covers servil slower than a
   competitor (st against st contenders; mt against all and against st),
   and slower per unit at N than at a divisor M of N.
6. **`perf_regress`** reads the new samples file (both scenarios) and
   caches each build by fork commit **and** benchmark-source hash
   (da92669: stale cached binaries had broken an A/B). `build.rs` watches
   the reflog so provenance follows commits made after a hooked build.
7. **Findings on the Mac** (all confirmed with per-thread P/E counters):
   - Two SME2 threads of one process share one P-cluster's SME unit in
     28-70% of rounds (run-dependent): shared SME2 cells run at full or
     half speed. The NEON-only pool shows no detectable effect on this
     (A/B old/new/new/old, NOTES-servil).
   - A tenth of the benchmark's **solo** samples at one-message 256 B -
     8 KiB run on **E-cores**, every contender alike, none for batches of
     the same bytes; cause unknown (suspect Rayon's idle threads). servil
     loses 3.3x there against SHA-256's 1.65x.
8. **Branch renamed** to `servil`; notes file `NOTES-servil.md`.

## Decisions made (don't re-ask)

- Contenders: at most two settings (single-threaded, multithreaded
  uncapped); two scenarios (solo, shared = a copy of itself); separate
  tables per scenario; wall time for everyone; no concurrent solo runs.
- Text report keeps KERNELS. User views omit maintainer detail.
- Branch naming `candidate/<topic>`; no promotion without the perf check
  on both machines.
- The Mac benchmark runner is launched **manually** by the user (no
  system service, no root-owned files) as a separate hidden standard
  account; the exchange folder lives inside `/workspace` (no second VM
  mount).

## The Mac benchmark runner (working since September 24, 2026)

Jobs run on the Mac as the hidden standard account `benchrunner` (login
shell `/usr/bin/false`, random password nobody holds, home 700), with code
cloned from GitHub alone at the commits a job names; the user's checkout
(and `ghtokenclassic.txt`, mode 600) stays out of its reach.

- **Files**: the code in the fork's `tools/runner/` (`runner.py`, whose
  docstring is the job format; `setup-mac.sh`; `README.md`); the
  exchange folders in the fork's `runner/` (kept out of git by
  `.git/info/exclude`): `jobs/` (ours, readable) and `results/` (the
  runner's, readable).
- **Start** on the Mac, as the user: `sh
  ~/piplayground/blake3-servil/tools/runner/setup-mac.sh`. It checks and
  repeats the setup (skipping what is done), copies `runner.py` to
  `/Users/Shared/bench-runner/` (the user's, read-only to the runner), and
  starts it under PyPy; Ctrl-C stops it after the current job. The runner
  prints a line as each job starts and ends.
- **Jobs**: write `jobs/NNN-name.json` from the VM (full hex commits, both
  pushed); the runner takes new files in name order and records each in
  `~benchrunner/.benchrunner_done`, so a rerun needs a new name. Types:
  `benchmark` (flags `--all`, `--thorough`; `contenders`, `points`,
  `rounds`, `trace_clocks`), `perf_regress` (`old_commit`, `new_commit`,
  `bench_commit`), `example` (`scaling`, `host_lab`; `features`). Each
  result folder holds `runner.log` (every command and its output),
  `verdict.json`, and the job's files.
- **Compiler**: the runner's toolchain is pinned by `setup-mac.sh` to the
  user's own rustc, matched by commit hash (rustc 1.98.0-nightly
  f428d123a 2026-06-19). A channel name would install the newest nightly:
  job 001 got 1.100.0 that way and was set aside.
- **Runner against Terminal** (job 002 and the same `--all --thorough`
  run from the user's Terminal, fork b931309, bench 709f17f; data and
  `compare.py` in the fork's `tmp/mac-terminal-vs-runner-b931309/`): 704
  cells, terminal/runner median of cell medians 1.001, of 5th percentiles
  1.000; slow-sample shares solo 5.3% / 5.8%, shared 8.7% / 8.5%. One cell
  differs by 10% or more, servil shared 64 MiB (0.89), a known two-speed
  SME2 cell (50% / 63% slow). The E-core cells (solo, 256 B-8 KiB) are
  alike under both accounts. The runner's results stand in for
  Terminal runs.
- **Later**: `tools/promote.py candidate/<topic>` checks the gate, records
  verdicts as git notes (`refs/notes/perf`: machine, base, result),
  fast-forwards `servil`, pushes; a pre-push hook refuses a `servil` tip
  without both machines' passing notes. Until then the Mac verdict is a
  runner `perf_regress` job's `verdict.json`, cited in the merge message.

## Next priorities

Open problems stay open until they reach one of the outcomes in AGENTS
("we own every slowdown a user could meet"): controlled, explained to
users with how to control it, or at least predicted.

1. **Use the runner**: the Mac half of the gate to `servil` (a
   `perf_regress` job per candidate), and the Mac-only probes below
   (problems 3-6) as jobs.
2. **Benchmarks that stay useful on hardware they can neither see nor
   steer.** In the VM (and anywhere else without per-core counters or
   affinity) the host runs vCPUs on P- or E-cores at will, so timings come
   out bimodal and the split varies run to run. VMs are a first-class
   target, so the benchmark must still give readers comparisons,
   regression signals, and planning figures there. How is open. Ideas to
   weigh: round-by-round pairing (CHECKS do it already); reporting both
   speeds and their shares (done); inferring each sample's core kind from
   a reference kernel timed beside it (a fixed scalar loop whose P and E
   speeds are known), then classifying samples as on native Apple;
   repeating or extending a run until each cell's share of each speed is
   known; and stating in the report which figures are placement-dependent.
3. **P/E classification of every sample on Apple**: read the per-thread
   counters around every solo sample and shared copy; tables and graph
   from P-core samples, each cell's E-core share (and E speed) in the
   maintainer report; `perf_regress` P against P, warning when E shares
   differ. A thorough-only pass at background QoS (E-cores by rule) to
   measure the E-core case reproducibly; a test of user-interactive QoS
   for the benchmark's own threads.
4. **Why the solo thread lands on E-cores** at one-message 256 B-8 KiB
   (up to 18% of samples, every contender, none for batches of the same
   bytes). Suspect: Rayon's idle pool threads (BLAKE3 mt splits from 8
   KiB). Test: the thorough run without `blake3-mt`, with
   `--trace-clocks` (data of the run with it: `tmp/mac-trace-1683ebb/`).
5. **servil is weak on E-cores**: 3.3x slower there at 4 KiB against
   SHA-256's 1.65x, so on an E-core it trails SHA-256. Kernel work.
6. **Two SME2 threads of one process share an SME unit** (macOS keeps a
   thread group on one P-cluster): shared SME2 cells run at full or half
   speed, 28-70% of rounds at full, varying by run. Leads: an
   `os_workgroup` per SME2 thread; the sharing-techniques batch below.
7. **Sharing techniques** (stacked): pool work at the caller's QoS/nice,
   cache footprint (non-temporal loads), placement hints, cross-process
   SME locks, and racing SME2 against NEON on the same block and keeping
   whichever finishes first.
8. **`perf_regress` sees only the faster speed** (5th percentile): make
   it judge each speed of a two-speed cell.
9. **Decide serial SME2 under case 3** (the user's call): keep, NEON only,
   or a detector that can never lock into a slow state (pacing did).
10. **Thorough Mac record**: committed only once the regressions it shows
    (shared two-speed cells) reach an outcome; the quick record stands.
11. Older: the Mac's serial 128 MiB rise; the pool's poll pause against
    yield-only polling on the Mac; batches over the pool with two
    callers and `many::TABLE` natively; release 0.7.0 of bench-hashes and
    a first fork tag; a GPU kernel (NOTES, future work).

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

On the Mac, from `bench-hashes`: `git checkout -- benchmark-results &&
cargo run --release -- --all --thorough` for a record (quick runs omit
the MiB inputs and big batches). Results overwrite that machine's files;
keep the results folder free of extra files so the provenance reads
`clean`. Commit a record only when it shows no regression against the
previous one (compare cell medians, servil and servil mt, both
scenarios); otherwise save it to `tmp/` and restore. Measure on one
machine at a time: the VM shares the Mac's cores.

Native A/B of two fork commits on the Mac (from the fork's root): build
with `python3 -c "import sys; sys.path.insert(0, 'tools'); import
perf_regress as p; print(p.commit_bench('OLD')[0]); print(p.commit_bench('NEW')[0])"`
(binaries land in `bench-hashes/target/perf-ab/<commit12>-<benchhash>/`),
then run each with `--all --thorough` from its own `tmp/ab-*` folder in
the order old, new, new, old; compare per-run shares of each speed, not
single medians (the split varies run to run).

Checking the graph script (VM): `apt-get install -y nodejs`, extract the
`<script>` CDATA from a generated SVG, `node --check` it, and run it
against a stub DOM (a recursive Proxy standing in for `document`) calling
`relayoutPlot`, `flipUnit`, and `showHover` on every point; render the
SVG with `rsvg-convert` to look at it.

Release: `python3 tools/gen-ver.py X.Y.Z` from a clean tree makes two
version commits and a lightweight tag `vX.Y.Z+<commit>`; push with
`git push origin main` and then the tag by name (`--follow-tags` skips
lightweight tags).

Expected suites: fork 63 library + 19 doc tests (`no_sme2` 62 + 19,
`pure` 53 + 19); official vectors 2; benchmark 6. Before any fork code
commit: `pypy3 tools/perf_regress.py check` (the hook runs it; exit 1
aborts, see the fork's AGENTS.md). Two commits side by side:
`pypy3 tools/perf_regress.py compare OLD NEW`. The platform facts behind
the pool: `cargo run --release --example host_lab` in the fork.

Use no timeout for long commands; let progress stream. Never sleep in
commands. If a network operation fails, report it and stop; the user
chooses retries. Never print the credential token. Only `/workspace`
survives VM restarts.
