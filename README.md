# bench-hashes

Written by GPT-5.6 Sol, Claude Fable 5, and Claude Opus 5.5 to my (Zooko's) specifications.

How fast is BLAKE3 on your computer, and how fast is SHA-256? bench-hashes
measures both, from 64-byte inputs to 128 MiB and in batches of many small
messages. It then draws the results as an interactive graph you open in a
web browser.

Results so far:

- [Apple M4 Max, macOS](https://johnservil.github.io/bench-hashes/benchmark-results/AppleM4Max.darwin25/bench-hashes.graph.svg)
- [A Linux VM on that Mac](https://johnservil.github.io/bench-hashes/benchmark-results/aarch64.linux618520virt/bench-hashes.graph.svg)

## Run it on your computer

You need [Rust](https://rustup.rs) and a C compiler: Xcode's command-line
tools on macOS (`xcode-select --install`), gcc or clang on Linux, or the
Visual Studio C++ build tools on Windows. Then:

```sh
git clone https://github.com/johnservil/bench-hashes
cd bench-hashes
cargo run --release
```

The first build takes a minute or two. The run checks every hash against
known answers, then measures for a few minutes. The numbers come out most
accurate when nothing else busy runs on the computer meanwhile.

On Linux on 64-bit ARM, the build needs Clang 17 or later, or GCC 14 or
later. With an older system compiler, install a newer Clang and name it
in the command, for example `CC=clang-19 cargo run --release`.

## Read your results

The run writes three files to `benchmark-results/`, in a folder named after
your CPU and operating system:

- `bench-hashes.graph.svg`: the graph. Open it in a web browser.
- `bench-hashes.result.txt`: the same numbers as text tables.
- `bench-hashes.samples.tsv`: every single measurement, for your own
  analysis.

The graph has four plots. The upper two show one hash call on a single
thread of an idle machine. The lower two show two copies of a hash
running at once, as when two programs hash side by side. In each pair,
the first plot hashes one input per call, from 64 B to 128 MiB. The
second hashes a batch of 64-byte messages per call. Higher is faster.
Hover over a dot, or tap it, to compare every contender at that point.
Click a name at the right to show or hide that contender.

The contenders:

- **BLAKE3 servil**: [a fork](https://github.com/johnservil/BLAKE3) of the
  official BLAKE3 Rust crate, with extra kernels for Apple M4-class chips.
  On other CPUs it runs the official crate's kernels.
- **BLAKE3 servil mt**: the same, spreading large inputs over all your
  CPU cores.
- **SHA-256** (the `sha2` crate) and **SHA-256 ring** (the `ring`
  crate): SHA-256 with the CPU's SHA-256 instructions where it has them.
  `sha2` is faster for the smallest inputs, `ring` from about 256 bytes.

## Share your results

Your graph is one self-contained file. Post it and `bench-hashes.result.txt`
anywhere people can download them: an issue, a gist, a forum. Anyone who
opens the graph in a browser sees it just as you do.

Or publish them on the web from your own copy of this repository, as the
results above are:

1. On GitHub, fork `johnservil/bench-hashes`. If you cloned it before
   forking, point your clone at your fork:
   `git remote set-url origin https://github.com/YOU/bench-hashes`
2. Commit your results and push them:
   ```sh
   git add benchmark-results
   git commit -m "Results for my computer"
   git push
   ```
3. On GitHub, in your fork's Settings, under Pages, choose "Deploy from a
   branch", the branch `main`, and the folder `/ (root)`.

A minute later your graph is at
`https://YOU.github.io/bench-hashes/benchmark-results/FOLDER/bench-hashes.graph.svg`,
where FOLDER is the folder the run created.

## More

`cargo run --release -- --help` lists the other options, such as more
BLAKE3 and SHA implementations. [METHODOLOGY.md](METHODOLOGY.md) explains
how bench-hashes measures, and what each contender runs.
