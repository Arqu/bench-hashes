# Working on bench-hashes

This file holds what you need to change bench-hashes, or the BLAKE3 fork
it measures, and keep your results comparable with ours.
[METHODOLOGY.md](METHODOLOGY.md) explains the measurement design.

## Layout

- `src/main.rs`: the whole benchmark (contenders, timing, statistics, the
  text report, the SVG and its script).
- `src/test_vectors.rs`: golden digests, written by
  `tools/gen-test-vectors.py`.
- `build.rs`: embeds the provenance (this repository's commit and state,
  and each contender crate's version and source).
- `tools/graph-check/`: drives the graph's script in jsdom and checks its
  layout (its README says how).

## Build and test

```sh
cargo test --release
cargo run --release -- --quick     # seconds; a full run takes minutes
node tools/graph-check/check.js benchmark-results/FOLDER/bench-hashes.graph.svg
```

## Working on the fork alongside

The `blake3-servil` contender is a git dependency on the `servil` branch of
[github.com/johnservil/BLAKE3](https://github.com/johnservil/BLAKE3), at
the commit `Cargo.lock` pins. To measure your own checkout of the fork,
put this repository inside it and point the dependency there:

```sh
git clone --branch servil https://github.com/johnservil/BLAKE3
git clone https://github.com/johnservil/bench-hashes BLAKE3/bench-hashes
cd BLAKE3/bench-hashes
cargo --config 'patch."https://github.com/johnservil/BLAKE3".blake3-servil.path=".."' run --release
```

Cargo then rewrites `Cargo.lock`'s `blake3-servil` entry; restore it with
`git checkout Cargo.lock` before you commit. The build counts that one
change as clean, and the report names the fork checkout's commit and
whether its tree was clean. The fork's `CONTRIBUTING.md` covers the
fork's own tests and its performance-regression check, which runs this
benchmark.

`cargo update -p blake3-servil` moves the pin to the fork's newest
`servil` commit.

## Rules that keep results comparable

- **Contenders are black boxes.** The benchmark lists a contender, calls
  its plain entry point (single-threaded, multithreaded, or a batch call)
  with no pool, cap, or wrapper of its own, and asks the fork for its
  `kernel_report()`. A contender without a batch entry point hashes a
  batch as `for m in batch { hash(m) }`.
- **Expected digests come from independent implementations**: the BLAKE3
  reference implementation and Python's `hashlib`, through
  `tools/gen-test-vectors.py`. A new point needs its vectors there.
  Regenerating vectors is a reviewed change; tests never write their own
  expected answers.
- **Results name their code.** Commit before you publish a result: a
  report whose provenance says `dirty-…` measured uncommitted code.
- **The fork's regression check depends on this interface**:
  `--contenders`, `--points`, `--rounds`, and the columns of
  `bench-hashes.samples.tsv` (`contender`, `scenario`, `use_case`,
  `point`, `unit`, `ps_per_unit`).

## Adding a contender

A contender is a variant of `Algorithm` in `src/main.rs`, with an entry
in `Algorithm::ALL`, a key, a name, a colour, a provenance string, a mode
description, a kernel description (`detect_kernels`), and an arm in
`hash_batch`. The harness handles selection, interleaving, checking, and
reporting for any count from two to eight.

## Maintainers' notes

`AGENTS.md`, `NEXT-STEPS.md`, and `NOTES.md` are the maintainers' working
notes: environment, current work, and the reasoning behind past
decisions. Contributing needs none of them; `NOTES.md` explains why the
measurement works as it does, should you want to change it.
