use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use std::fs;
use std::hint::black_box;
use std::io::{IsTerminal, Write as _};
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use sysinfo::System;

#[cfg(target_arch = "wasm32")]
compile_error!("bench-hashes currently supports native targets only");

/*
 * Every (contender, size) cell collects SAMPLE_ROUNDS samples of about
 * TARGET_SAMPLE_NS each. The two knobs trade off differently:
 *
 * - Fewer rounds thin the evidence behind the min–max band, so the band can
 *   look tight while the true spread is wider: false precision.
 * - Shorter samples keep the sample count. Any disturbance (an interrupt, a
 *   clock step) is a larger share of a short sample, so it widens the band
 *   rather than averaging away inside it. The band then tells the truth
 *   about how noisy the run was.
 *
 * So the runtime budget goes to rounds first. 1 ms is long enough that the
 * clock's own resolution (tens of nanoseconds) is under 0.01% of a sample.
 * Rounds are a multiple of the sixteen sizes and of the order count.
 */
const SAMPLE_ROUNDS: usize = 80;
const CALIBRATION_PROBE_NS: u128 = 500_000;
const TARGET_SAMPLE_NS: u128 = 1_000_000;

const INPUT_COUNT: usize = 16;

const BENCH_VERSION: &str = env!("CARGO_PKG_VERSION");
const GIT_SOURCE: &str = env!("BENCH_GIT_SOURCE");
const GIT_COMMIT: &str = env!("BENCH_GIT_COMMIT");
const GIT_TAG: &str = env!("BENCH_GIT_TAG");
const GIT_CLEAN_STATUS: &str = env!("BENCH_GIT_CLEAN_STATUS");

const RUSTC_VERSION: &str = env!("BENCH_RUSTC_VERSION");
const BUILD_TARGET: &str = env!("BENCH_BUILD_TARGET");
const TARGET_FEATURES: &str = env!("BENCH_TARGET_FEATURES");

const BLAKE3_SOURCE_INFO: &str = env!("BLAKE3_SOURCE_INFO");
const SHA2_SOURCE_INFO: &str = env!("SHA2_SOURCE_INFO");
const SHA2_ASM_SOURCE_INFO: &str = env!("SHA2_ASM_SOURCE_INFO");
const SHA1_CHECKED_SOURCE_INFO: &str = env!("SHA1_CHECKED_SOURCE_INFO");
const BLAKE3_SME2_SOURCE_INFO: &str = env!("BLAKE3_SME2_SOURCE_INFO");

/*
 * Every power of two from 64 B to 1 MiB, plus 3 KiB. Between 64 B and 1 KiB
 * BLAKE3 is inside one chunk; from 2 KiB to 16 KiB its SIMD paths fill up
 * (4-way NEON at 4 KiB, a sixteen-lane SME2 group at 16 KiB); above that
 * the bulk rate settles. 3 KiB is where the fork's integer + NEON hybrid
 * kernels first overtake hardware SHA-256: one chunk on the integer ALUs
 * beside a NEON pair costs the same as the pair alone. SHA-1DC and SHA-256
 * are block-serial and have only the per-message overhead to show.
 */
const INPUT_SIZES: [InputSize; INPUT_COUNT] = [
    InputSize { label: "64 B", bytes: 64 },
    InputSize { label: "128 B", bytes: 128 },
    InputSize { label: "256 B", bytes: 256 },
    InputSize { label: "512 B", bytes: 512 },
    InputSize { label: "1 KiB", bytes: 1024 },
    InputSize { label: "2 KiB", bytes: 2 * 1024 },
    InputSize { label: "3 KiB", bytes: 3 * 1024 },
    InputSize { label: "4 KiB", bytes: 4 * 1024 },
    InputSize { label: "8 KiB", bytes: 8 * 1024 },
    InputSize { label: "16 KiB", bytes: 16 * 1024 },
    InputSize { label: "32 KiB", bytes: 32 * 1024 },
    InputSize { label: "64 KiB", bytes: 64 * 1024 },
    InputSize { label: "128 KiB", bytes: 128 * 1024 },
    InputSize { label: "256 KiB", bytes: 256 * 1024 },
    InputSize { label: "512 KiB", bytes: 512 * 1024 },
    InputSize { label: "1 MiB", bytes: 1024 * 1024 },
];

/// results[contender_index][size_index], contenders in the roster's order.
type Results = Vec<[Statistics; INPUT_COUNT]>;
type Samples = Vec<[Vec<f64>; INPUT_COUNT]>;

#[derive(Clone, Copy)]
struct InputSize {
    label: &'static str,
    bytes: usize,
}

/*
 * A contender is one hash implementation under test. Adding one means a
 * variant here, an entry in ALL, a key, a name, a color, a provenance
 * string, an implementation description, and an arm in run_batch; the
 * harness handles selection, interleaving, and reporting for any count.
 */
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Algorithm {
    Blake3,
    Sha256,
    Sha1Dc,
    Blake3Sme2,
    /// Apple's CommonCrypto SHA-256 through CC_SHA256_Init/Update/Final.
    Sha256CommonCrypto,
}

/// The hash function a contender implements; "best available" is chosen
/// within a family.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Family {
    Blake3,
    Sha256,
    Sha1Dc,
}

impl Family {
    fn name(self) -> &'static str {
        match self {
            Self::Blake3 => "BLAKE3",
            Self::Sha256 => "SHA-256",
            Self::Sha1Dc => "SHA-1DC",
        }
    }
}

impl Algorithm {
    const ALL: [Algorithm; 5] = [
        Algorithm::Blake3,
        Algorithm::Sha256,
        Algorithm::Sha1Dc,
        Algorithm::Blake3Sme2,
        Algorithm::Sha256CommonCrypto,
    ];

    /// Command-line key, as in `--contenders blake3,sha256-cc`.
    fn key(self) -> &'static str {
        match self {
            Self::Blake3 => "blake3",
            Self::Sha256 => "sha256",
            Self::Sha1Dc => "sha1dc",
            Self::Blake3Sme2 => "blake3-sme2",
            Self::Sha256CommonCrypto => "sha256-cc",
        }
    }

    fn family(self) -> Family {
        match self {
            Self::Blake3 | Self::Blake3Sme2 => Family::Blake3,
            Self::Sha256 | Self::Sha256CommonCrypto => Family::Sha256,
            Self::Sha1Dc => Family::Sha1Dc,
        }
    }

    /// Whether this contender can run on the current machine, or why not.
    fn availability(self) -> Result<(), String> {
        match self {
            Self::Blake3 | Self::Sha256 | Self::Sha1Dc => Ok(()),
            Self::Blake3Sme2 => {
                let platform = blake3_sme2::platform::Platform::detect();
                if format!("{platform:?}") == "SME2" {
                    Ok(())
                } else {
                    Err(format!(
                        "the blake3_sme2 crate selected {platform:?} on this machine; BLAKE3 SME2 needs a CPU with SME2 and 512-bit streaming vectors (Apple M4 and later)"
                    ))
                }
            }
            Self::Sha256CommonCrypto => {
                if cfg!(target_vendor = "apple") {
                    Ok(())
                } else {
                    Err("CommonCrypto is Apple's system library; this build is not for an Apple platform".to_owned())
                }
            }
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Blake3 => "BLAKE3",
            Self::Sha256 => "SHA-256",
            Self::Sha1Dc => "SHA-1DC",
            Self::Blake3Sme2 => "BLAKE3 SME2",
            Self::Sha256CommonCrypto => "SHA-256 CommonCrypto",
        }
    }

    /*
     * Contender colours stay off pure green and pure red, which the hover
     * panel reserves for "faster" and "slower".
     */
    fn color(self) -> &'static str {
        match self {
            Self::Blake3 => "#3b82f6",
            Self::Sha256 => "#e07a45",
            Self::Sha1Dc => "#8a7a1e",
            Self::Blake3Sme2 => "#7c3aed",
            Self::Sha256CommonCrypto => "#0e9aa7",
        }
    }

    /// The Cargo.lock description of the crate that implements this contender.
    fn source(self) -> &'static str {
        match self {
            Self::Blake3 => BLAKE3_SOURCE_INFO,
            Self::Sha256 => SHA2_SOURCE_INFO,
            Self::Sha1Dc => SHA1_CHECKED_SOURCE_INFO,
            Self::Blake3Sme2 => BLAKE3_SME2_SOURCE_INFO,
            Self::Sha256CommonCrypto => "CommonCrypto CC_SHA256_Init/Update/Final from the running macOS (libSystem); version follows the OS",
        }
    }

    /// One line on how this contender runs, for the report header.
    fn mode(self) -> &'static str {
        match self {
            Self::Blake3 => "single-threaded; Rayon not enabled",
            Self::Sha256 => "assembly backends where available (ARMv8 SHA-256 instructions on AArch64)",
            Self::Sha1Dc => "SHA-1 with collision detection, pure Rust (the construction git uses)",
            Self::Blake3Sme2 => "single-threaded; SME2 kernel for groups of sixteen chunks, integer + NEON hybrid kernels below that; needs a CPU with SME2",
            Self::Sha256CommonCrypto => "Apple CommonCrypto CC_SHA256_Init/Update/Final via FFI; the system's own SHA-256 (corecrypto, ARMv8 SHA-256 instructions on Apple silicon). The one-shot CC_SHA256 is avoided: its finalisation costs ~110 ns per compression",
        }
    }
}

#[derive(Clone, Copy)]
struct Statistics {
    minimum: f64,
    median: f64,
    maximum: f64,
}

impl Statistics {
    const ZERO: Self = Self {
        minimum: 0.0,
        median: 0.0,
        maximum: 0.0,
    };
}

struct MachineMetadata {
    timestamp: String,
    cpu_type: String,
    cpu_count: usize,
    os_type: String,
}

/*
 * The contenders selected for this run, in column order, with the
 * interleaving orders that balance them and the index of the baseline
 * every ratio is taken against.
 *
 * Contract: two to six contenders, each available on this machine, no
 * duplicates. The baseline is the first BLAKE3-family contender when one
 * is present, otherwise the first contender.
 */
struct Roster {
    algorithms: Vec<Algorithm>,
    orders: Vec<Vec<usize>>,
    baseline: usize,
}

impl Roster {
    fn new(algorithms: Vec<Algorithm>) -> Self {
        assert!(
            (2..=6).contains(&algorithms.len()),
            "a run compares two to six contenders; {} were selected",
            algorithms.len()
        );
        for (index, algorithm) in algorithms.iter().enumerate() {
            assert!(
                !algorithms[..index].contains(algorithm),
                "{} was selected twice",
                algorithm.name()
            );
            if let Err(reason) = algorithm.availability() {
                panic!("{} cannot run here: {reason}", algorithm.name());
            }
        }
        let orders = williams_orders(algorithms.len());
        assert_eq!(
            SAMPLE_ROUNDS % orders.len(),
            0,
            "SAMPLE_ROUNDS ({SAMPLE_ROUNDS}) must be a multiple of the order count ({})",
            orders.len()
        );
        let baseline = algorithms
            .iter()
            .position(|algorithm| algorithm.family() == Family::Blake3)
            .unwrap_or(0);
        Self { algorithms, orders, baseline }
    }

    fn len(&self) -> usize {
        self.algorithms.len()
    }
}

/*
 * A Williams design on n contenders: n orders when n is even, 2n when odd.
 * Every contender takes every position equally often and every ordered
 * adjacency "Y right after X" occurs equally often, so the set balances
 * carry-over effects the way all n! permutations would.
 */
fn williams_orders(n: usize) -> Vec<Vec<usize>> {
    assert!(n >= 2, "a Williams design needs at least two contenders");
    let mut rows: Vec<Vec<usize>> = (0..n)
        .map(|start| {
            (0..n)
                .map(|k| {
                    let offset = if k % 2 == 1 { (k + 1) / 2 } else { n - k / 2 };
                    (start + offset) % n
                })
                .collect()
        })
        .collect();
    if n % 2 == 1 {
        let reversed: Vec<Vec<usize>> = rows.iter().map(|row| row.iter().rev().copied().collect()).collect();
        rows.extend(reversed);
    }
    assert_orders_balanced(&rows, n);
    rows
}

/// How the user chose the contenders, for the report header.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Selection {
    /// Default: SHA-1DC and the best available member of each other family.
    Best,
    /// `--all`: every contender that can run here.
    All,
    /// `--contenders a,b,c`.
    Explicit,
}

const USAGE: &str = "\
bench-hashes: single-threaded hash throughput by input size

  bench-hashes                     SHA-1DC plus the best available BLAKE3 and
                                   SHA-256 on this machine (best = Pareto-better
                                   at every size; the run says so if none is)
  bench-hashes --all               every contender this machine can run
  bench-hashes --contenders K,...  exactly these, in this column order
  bench-hashes --list              contenders and their availability here

Keys: blake3, blake3-sme2, sha256, sha256-cc, sha1dc
";

fn parse_arguments() -> (Selection, Vec<Algorithm>) {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    match arguments.as_slice() {
        [] => (Selection::Best, Vec::new()),
        [flag] if flag == "--all" => (Selection::All, Vec::new()),
        [flag] if flag == "--list" => {
            for algorithm in Algorithm::ALL {
                let status = match algorithm.availability() {
                    Ok(()) => "available".to_owned(),
                    Err(reason) => format!("unavailable: {reason}"),
                };
                println!("  {:<12} {:<22} {status}", algorithm.key(), algorithm.name());
            }
            std::process::exit(0);
        }
        [flag, keys] if flag == "--contenders" => {
            let algorithms = keys
                .split(',')
                .map(|key| {
                    Algorithm::ALL
                        .into_iter()
                        .find(|algorithm| algorithm.key() == key.trim())
                        .unwrap_or_else(|| {
                            eprintln!("unknown contender {key:?}\n\n{USAGE}");
                            std::process::exit(2);
                        })
                })
                .collect();
            (Selection::Explicit, algorithms)
        }
        [flag] if flag == "--help" || flag == "-h" => {
            print!("{USAGE}");
            std::process::exit(0);
        }
        _ => {
            eprint!("{USAGE}");
            std::process::exit(2);
        }
    }
}

fn main() {
    assert_eq!(
        SAMPLE_ROUNDS % INPUT_SIZES.len(),
        0,
        "SAMPLE_ROUNDS must use every input-size position equally"
    );

    let (selection, explicit) = parse_arguments();
    let available: Vec<Algorithm> = Algorithm::ALL
        .into_iter()
        .filter(|algorithm| algorithm.availability().is_ok())
        .collect();

    let machine = machine_metadata();

    /*
     * The default run picks the best available member of each family. That
     * needs measurements, so it measures every available contender, then
     * keeps the Pareto-best per family and reports on those alone. Timing
     * cost is the same as --all; only the report narrows.
     */
    let (roster, results, selection_note) = match selection {
        Selection::Explicit => {
            let roster = Roster::new(explicit);
            let results = measure_all(&roster);
            (roster, results, String::from("contenders chosen on the command line"))
        }
        Selection::All => {
            let roster = Roster::new(available);
            let results = measure_all(&roster);
            (roster, results, String::from("every contender available on this machine"))
        }
        Selection::Best => {
            let full = Roster::new(available);
            let full_results = measure_all(&full);
            let (keep, note) = choose_best_per_family(&full, &full_results);
            let roster = Roster::new(keep.iter().map(|&index| full.algorithms[index]).collect());
            let results: Results = full_results
                .iter()
                .enumerate()
                .filter(|(index, _)| keep.contains(index))
                .map(|(_, row)| *row)
                .collect();
            (roster, results, note)
        }
    };

    let text = generate_text(&roster, &results, &machine, &selection_note);
    let svg = generate_svg(&roster, &results, &machine, &selection_note);

    print!("{text}");

    let directory = output_directory(&machine);

    fs::create_dir_all(&directory).unwrap_or_else(|error| {
        panic!(
            "failed to create output directory {}: {error}",
            directory.display(),
        )
    });

    let text_path = directory.join("bench-hashes.result.txt");
    let svg_path = directory.join("bench-hashes.graph.svg");

    fs::write(&text_path, &text).unwrap_or_else(|error| {
        panic!("failed to write {}: {error}", text_path.display())
    });

    fs::write(&svg_path, &svg).unwrap_or_else(|error| {
        panic!("failed to write {}: {error}", svg_path.display())
    });

    println!(
        "# Data results (text) are in \"{}\" .",
        text_path.display(),
    );
    println!(
        "# Graph results (SVG) are in \"{}\" .",
        svg_path.display(),
    );
}

/*
 * The orders must place every contender in every position equally often
 * and realise every ordered adjacency equally often. This is what lets a
 * handful of orders stand in for all permutations.
 */
fn assert_orders_balanced(rows: &[Vec<usize>], n: usize) {
    assert_eq!(rows.len() % n, 0, "the order count must be a multiple of the contender count");
    let per_position = rows.len() / n;
    let per_adjacency = rows.len() / n;

    let mut positions = vec![vec![0usize; n]; n];
    let mut adjacencies = vec![vec![0usize; n]; n];

    for order in rows {
        assert_eq!(order.len(), n);
        for (position, &algorithm) in order.iter().enumerate() {
            positions[algorithm][position] += 1;
            if position > 0 {
                adjacencies[order[position - 1]][algorithm] += 1;
            }
        }
    }

    for algorithm in 0..n {
        for position in 0..n {
            assert_eq!(
                positions[algorithm][position], per_position,
                "contender {algorithm} must take position {position} {per_position} time(s) across the orders"
            );
        }
        for follower in 0..n {
            let expected = if follower == algorithm { 0 } else { per_adjacency };
            assert_eq!(
                adjacencies[algorithm][follower], expected,
                "contender {follower} must run right after {algorithm} {expected} time(s) across the orders"
            );
        }
    }
}

/*
 * For each family with more than one available contender, the member that
 * is at least as fast (by median) at every size, and strictly faster at
 * one, is the best. Without such a member the family has no best: both
 * are kept and the note says so. Returns the kept indices in roster order.
 */
fn choose_best_per_family(roster: &Roster, results: &Results) -> (Vec<usize>, String) {
    let mut keep: Vec<usize> = Vec::new();
    let mut notes: Vec<String> = Vec::new();

    for family in [Family::Blake3, Family::Sha256, Family::Sha1Dc] {
        let members: Vec<usize> = (0..roster.len())
            .filter(|&index| roster.algorithms[index].family() == family)
            .collect();
        if members.len() <= 1 {
            keep.extend(&members);
            continue;
        }
        let dominates = |a: usize, b: usize| {
            let mut strictly = false;
            for size_index in 0..INPUT_COUNT {
                let (ma, mb) = (results[a][size_index].median, results[b][size_index].median);
                if ma > mb {
                    return false;
                }
                if ma < mb {
                    strictly = true;
                }
            }
            strictly
        };
        let best: Vec<usize> = members
            .iter()
            .copied()
            .filter(|&a| members.iter().all(|&b| a == b || dominates(a, b)))
            .collect();
        match best.as_slice() {
            [winner] => {
                keep.push(*winner);
                let others: Vec<&str> = members
                    .iter()
                    .filter(|&&index| index != *winner)
                    .map(|&index| roster.algorithms[index].name())
                    .collect();
                notes.push(format!(
                    "{} is the best {} here (faster than {} at every size)",
                    roster.algorithms[*winner].name(),
                    family.name(),
                    others.join(" and "),
                ));
            }
            _ => {
                keep.extend(&members);
                let names: Vec<&str> = members.iter().map(|&index| roster.algorithms[index].name()).collect();
                let crossover = first_crossover(results, members[0], members[1]);
                notes.push(format!(
                    "no best {} here: {} each win at some sizes{}; both are shown",
                    family.name(),
                    names.join(" and "),
                    crossover.map(|label| format!(" (lead changes at {label})")).unwrap_or_default(),
                ));
            }
        }
    }
    keep.sort_unstable();
    (keep, notes.join("; "))
}

/// The first tested size at which the faster of two contenders changes.
fn first_crossover(results: &Results, a: usize, b: usize) -> Option<&'static str> {
    let leader = |size_index: usize| results[a][size_index].median < results[b][size_index].median;
    (1..INPUT_COUNT)
        .find(|&size_index| leader(size_index) != leader(size_index - 1))
        .map(|size_index| INPUT_SIZES[size_index].label)
}

fn measure_all(roster: &Roster) -> Results {
    let inputs: [Vec<u8>; INPUT_COUNT] =
        std::array::from_fn(|index| make_input(INPUT_SIZES[index].bytes));

    /*
     * The two BLAKE3 contenders must produce the same digest on every input;
     * a mismatch means one of them is wrong, and timing it would be noise.
     */
    for input in &inputs {
        assert_eq!(
            blake3::hash(input).as_bytes(),
            blake3_sme2::hash(input).as_bytes(),
            "BLAKE3 SME2 must agree with crates.io blake3 on a {}-byte input",
            input.len(),
        );
        if roster.algorithms.contains(&Algorithm::Sha256CommonCrypto) {
            assert_eq!(
                Sha256::digest(input).as_slice(),
                &common_crypto::sha256(input)[..],
                "CommonCrypto SHA-256 must agree with the sha2 crate on a {}-byte input",
                input.len(),
            );
        }
    }

    /*
     * Each algorithm/input combination gets its own calibrated iteration
     * count so that timed blocks have approximately equal durations.
     */
    let mut progress = Progress::new(roster);
    progress.phase("calibrating");

    let mut batch_iterations: Vec<[usize; INPUT_COUNT]> = vec![[1usize; INPUT_COUNT]; roster.len()];

    for size_index in 0..INPUT_COUNT {
        for algorithm_index in 0..roster.len() {
            batch_iterations[algorithm_index][size_index] =
                calibrate_batch(
                    roster.algorithms[algorithm_index],
                    &inputs[size_index],
                );
        }
    }

    progress.phase("warming up");

    /*
     * Warm every algorithm in every ordering position, on every input size.
     * The input size that runs first is rotated as well.
     */
    for warmup_round in 0..roster.orders.len() {
        let order = &roster.orders[warmup_round];

        for size_offset in 0..INPUT_COUNT {
            let size_index =
                (size_offset + warmup_round) % INPUT_COUNT;

            for &algorithm_index in order {
                run_batch(
                    roster.algorithms[algorithm_index],
                    &inputs[size_index],
                    batch_iterations[algorithm_index][size_index],
                );
            }
        }
    }

    let mut samples: Samples = (0..roster.len())
        .map(|_| std::array::from_fn(|_| Vec::with_capacity(SAMPLE_ROUNDS)))
        .collect();

    /*
     * The algorithm order cycles through all six permutations. Input-size
     * order rotates independently. This distributes ordering, thermal, and
     * system-load effects across the algorithms.
     */
    progress.phase("measuring");

    for round in 0..SAMPLE_ROUNDS {
        progress.round(round, &samples);

        let algorithm_order = &roster.orders[round % roster.orders.len()];

        for size_offset in 0..INPUT_COUNT {
            let size_index =
                (size_offset + round) % INPUT_COUNT;

            let input = &inputs[size_index];

            for &algorithm_index in algorithm_order {
                let algorithm = roster.algorithms[algorithm_index];
                let iterations =
                    batch_iterations[algorithm_index][size_index];

                let started = Instant::now();

                run_batch(algorithm, input, iterations);

                let elapsed = started.elapsed();
                let total_bytes =
                    input.len() as f64 * iterations as f64;

                let nanoseconds_per_byte =
                    elapsed.as_secs_f64() * 1_000_000_000.0
                    / total_bytes;

                assert!(
                    nanoseconds_per_byte.is_finite()
                        && nanoseconds_per_byte > 0.0,
                    "every timing sample must be finite and positive"
                );

                samples[algorithm_index][size_index]
                    .push(nanoseconds_per_byte);
            }
        }
    }

    progress.finish(&samples);

    let mut results: Results = vec![[Statistics::ZERO; INPUT_COUNT]; roster.len()];

    for algorithm_index in 0..roster.len() {
        for size_index in 0..INPUT_COUNT {
            results[algorithm_index][size_index] =
                summarize(&mut samples[algorithm_index][size_index]);
        }
    }

    results
}

/*
 * Live progress on stderr, so stdout stays a clean report. Shows the phase,
 * a bar over the sample rounds, the elapsed and estimated remaining time,
 * and the running median for every contender at the largest input size.
 * The line redraws in place on a terminal; elsewhere each update is its
 * own line, so a log still shows the run advancing.
 */
struct Progress<'a> {
    roster: &'a Roster,
    started: Instant,
    measuring_started: Option<Instant>,
    interactive: bool,
    last_width: usize,
}

impl<'a> Progress<'a> {
    const BAR_WIDTH: usize = 30;

    fn new(roster: &'a Roster) -> Self {
        let interactive = std::io::stderr().is_terminal();
        Self {
            roster,
            started: Instant::now(),
            measuring_started: None,
            interactive,
            last_width: 0,
        }
    }

    fn phase(&mut self, name: &str) {
        if name == "measuring" {
            self.measuring_started = Some(Instant::now());
        }
        self.draw(&format!(
            "[{:>5.1}s] {name}…",
            self.started.elapsed().as_secs_f64(),
        ));
    }

    /* Called at the start of each round; `samples` holds every round so far. */
    fn round(&mut self, round: usize, samples: &Samples) {
        let measuring_started = self
            .measuring_started
            .expect("round() runs inside the measuring phase");

        let done = round as f64 / SAMPLE_ROUNDS as f64;
        let filled = (done * Self::BAR_WIDTH as f64).round() as usize;
        let bar: String = "█".repeat(filled) + &"░".repeat(Self::BAR_WIDTH - filled);

        let elapsed = measuring_started.elapsed().as_secs_f64();
        let remaining = if round > 0 {
            format!("{:>3.0}s left", elapsed / done * (1.0 - done))
        } else {
            " estimating".to_owned()
        };

        self.draw(&format!(
            "[{:>5.1}s] measuring {bar} {:>3}/{SAMPLE_ROUNDS} rounds · {remaining} · {}",
            self.started.elapsed().as_secs_f64(),
            round,
            running_medians(self.roster, samples, INPUT_COUNT - 1),
        ));
    }

    fn finish(&mut self, samples: &Samples) {
        let bar = "█".repeat(Self::BAR_WIDTH);
        self.draw(&format!(
            "[{:>5.1}s] measured  {bar} {SAMPLE_ROUNDS}/{SAMPLE_ROUNDS} rounds · {}",
            self.started.elapsed().as_secs_f64(),
            running_medians(self.roster, samples, INPUT_COUNT - 1),
        ));
        eprintln!();
    }

    fn draw(&mut self, line: &str) {
        let mut stderr = std::io::stderr().lock();
        if self.interactive {
            /* Return to column 0, overwrite, blank any leftover from a longer line. */
            let padding = self.last_width.saturating_sub(line.chars().count());
            let _ = write!(stderr, "\r{line}{}", " ".repeat(padding));
        } else {
            let _ = writeln!(stderr, "{line}");
        }
        let _ = stderr.flush();
        self.last_width = line.chars().count();
    }
}

/*
 * "BLAKE3 0.39 · SHA-256 0.33 · … ns/B at 1 MiB" from the samples collected
 * so far, or a placeholder before the first round completes.
 */
fn running_medians(roster: &Roster, samples: &Samples, size_index: usize) -> String {
    if samples[0][size_index].is_empty() {
        return format!("medians at {} pending", INPUT_SIZES[size_index].label);
    }

    let parts: Vec<String> = (0..roster.len())
        .map(|algorithm_index| {
            let mut sorted = samples[algorithm_index][size_index].clone();
            sorted.sort_by(f64::total_cmp);
            format!("{} {:.3}", roster.algorithms[algorithm_index].name(), median_of_sorted(&sorted))
        })
        .collect();

    format!("{} ns/B at {}", parts.join(" · "), INPUT_SIZES[size_index].label)
}

fn make_input(size: usize) -> Vec<u8> {
    assert!(size > 0, "input size must be positive");

    let mut input = vec![0_u8; size];

    /*
     * Deterministic input generation happens outside timed intervals.
     * Cryptographic hash performance should not depend on these byte values.
     */
    let mut state =
        0x6a09_e667_f3bc_c909_u64 ^ (size as u64).rotate_left(17);

    for byte in &mut input {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        *byte = (state >> 24) as u8;
    }

    input
}

fn run_batch(
    algorithm: Algorithm,
    input: &[u8],
    iterations: usize,
) {
    assert!(iterations > 0, "batch size must be positive");

    match algorithm {
        Algorithm::Blake3 => {
            for _ in 0..iterations {
                let digest = blake3::hash(black_box(input));
                let _ = black_box(digest);
            }
        }
        Algorithm::Sha256 => {
            for _ in 0..iterations {
                let digest = Sha256::digest(black_box(input));
                let _ = black_box(digest);
            }
        }
        Algorithm::Sha1Dc => {
            for _ in 0..iterations {
                let result = sha1_checked::Sha1::try_digest(black_box(input));
                let _ = black_box(result.hash());
            }
        }
        Algorithm::Blake3Sme2 => {
            for _ in 0..iterations {
                let digest = blake3_sme2::hash(black_box(input));
                let _ = black_box(digest);
            }
        }
        Algorithm::Sha256CommonCrypto => {
            for _ in 0..iterations {
                let digest = common_crypto::sha256(black_box(input));
                let _ = black_box(digest);
            }
        }
    }
}

/*
 * Apple's CommonCrypto SHA-256, linked from libSystem, through the
 * streaming Init/Update/Final calls.
 *
 * The one-shot CC_SHA256() (and CCDigest()) is avoided on purpose: measured
 * on an M4 Max, its finalisation costs about 110 ns per compression against
 * 17 ns for the same arithmetic elsewhere, so a 64-byte digest took 182 ns
 * one-shot and 51 ns through Init/Update/Final, with identical bulk
 * throughput. The streaming path is the efficient way to call corecrypto.
 */
#[cfg(target_vendor = "apple")]
mod common_crypto {
    pub const DIGEST_LEN: usize = 32;

    /// CC_SHA256_CTX: two 32-bit counters, eight state words, a 64-byte
    /// block buffer. Layout fixed by <CommonCrypto/CommonDigest.h>.
    #[repr(C)]
    struct Context {
        count: [u32; 2],
        hash: [u32; 8],
        wbuf: [u32; 16],
    }

    unsafe extern "C" {
        fn CC_SHA256_Init(ctx: *mut Context) -> i32;
        /// CC_LONG is uint32_t, so one Update takes at most 4 GiB; every
        /// input here is at most 1 MiB.
        fn CC_SHA256_Update(ctx: *mut Context, data: *const u8, len: u32) -> i32;
        fn CC_SHA256_Final(md: *mut u8, ctx: *mut Context) -> i32;
    }

    pub fn sha256(input: &[u8]) -> [u8; DIGEST_LEN] {
        let len = u32::try_from(input.len()).expect("CC_SHA256_Update takes a 32-bit length");
        let mut context = Context { count: [0; 2], hash: [0; 8], wbuf: [0; 16] };
        let mut digest = [0u8; DIGEST_LEN];
        // Safe: `context` is a valid CC_SHA256_CTX for the three calls,
        // `input` is valid for `len` bytes, and Final writes exactly 32
        // bytes to `digest`. Each call returns 1 on success.
        unsafe {
            assert_eq!(CC_SHA256_Init(&mut context), 1, "CC_SHA256_Init failed");
            assert_eq!(CC_SHA256_Update(&mut context, input.as_ptr(), len), 1, "CC_SHA256_Update failed");
            assert_eq!(CC_SHA256_Final(digest.as_mut_ptr(), &mut context), 1, "CC_SHA256_Final failed");
        }
        digest
    }
}

#[cfg(not(target_vendor = "apple"))]
mod common_crypto {
    /// Never called: Roster::new rejects the contender off Apple.
    pub fn sha256(_input: &[u8]) -> [u8; 32] {
        unreachable!("CommonCrypto SHA-256 is an Apple-only contender")
    }
}

fn calibrate_batch(
    algorithm: Algorithm,
    input: &[u8],
) -> usize {
    let mut iterations = 1usize;

    loop {
        let started = Instant::now();
        run_batch(algorithm, input, iterations);
        let elapsed_ns = started.elapsed().as_nanos();

        /*
         * A sufficiently short interval can be below a platform timer's
         * effective resolution. Increase the batch until it is measurable.
         */
        if elapsed_ns == 0 {
            iterations = iterations
                .checked_mul(2)
                .expect("calibration iteration count overflowed");

            continue;
        }

        if elapsed_ns >= CALIBRATION_PROBE_NS {
            let scaled = (
                iterations as u128 * TARGET_SAMPLE_NS
                    + elapsed_ns / 2
            ) / elapsed_ns;

            let scaled = scaled.max(1);

            assert!(
                scaled <= usize::MAX as u128,
                "calibrated iteration count must fit in usize"
            );

            return scaled as usize;
        }

        let growth =
            (CALIBRATION_PROBE_NS + elapsed_ns - 1) / elapsed_ns;

        assert!(
            growth >= 2,
            "a sub-probe measurement must require a larger batch"
        );

        iterations = iterations
            .checked_mul(
                usize::try_from(growth)
                    .expect("calibration growth factor must fit in usize"),
            )
            .expect("calibration iteration count overflowed");
    }
}

/* Requires a non-empty, ascending slice. */
fn median_of_sorted(sorted: &[f64]) -> f64 {
    assert!(!sorted.is_empty(), "median requires at least one sample");
    debug_assert!(sorted.windows(2).all(|pair| pair[0] <= pair[1]));

    let middle = sorted.len() / 2;
    if sorted.len() % 2 == 0 {
        (sorted[middle - 1] + sorted[middle]) / 2.0
    } else {
        sorted[middle]
    }
}

fn summarize(samples: &mut [f64]) -> Statistics {
    assert_eq!(
        samples.len(),
        SAMPLE_ROUNDS,
        "every result must contain exactly SAMPLE_ROUNDS samples"
    );

    assert!(
        samples
            .iter()
            .all(|sample| sample.is_finite() && *sample > 0.0),
        "all samples must be finite and positive"
    );

    samples.sort_by(f64::total_cmp);

    let median = median_of_sorted(samples);

    Statistics {
        minimum: samples[0],
        median,
        maximum: samples[samples.len() - 1],
    }
}

/*
 * One code path a contender uses for a range of input sizes. `first` is the
 * smallest input in bytes that takes this path; a contender's regimes are
 * listed in ascending order of `first`, the first starting at 0.
 */
#[derive(Clone, Copy)]
struct Regime {
    first: usize,
    /// Short name for the report and the hover panel, e.g. "NEON hash_many".
    name: &'static str,
    /// One sentence on why the path changes here, for the first dot of the regime.
    why: &'static str,
    /// Mark drawn at every dot in this regime.
    mark: Mark,
}

/// Dot shapes: the same colour keeps the contender's identity, the shape
/// says which code path produced the point.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mark {
    Circle,
    Diamond,
    Square,
}

impl Mark {
    fn name(self) -> &'static str {
        match self {
            Self::Circle => "circle",
            Self::Diamond => "diamond",
            Self::Square => "square",
        }
    }
}

/*
 * A contender's code paths by input size, with the platform name for the
 * report header. `regimes` is non-empty, ascending in `first`, and starts
 * at 0.
 */
struct Implementation {
    platform: &'static str,
    regimes: Vec<Regime>,
}

impl Implementation {
    fn new(platform: &'static str, regimes: Vec<Regime>) -> Self {
        assert!(!regimes.is_empty(), "a contender has at least one regime");
        assert_eq!(regimes[0].first, 0, "the first regime covers the smallest inputs");
        assert!(
            regimes.windows(2).all(|pair| pair[0].first < pair[1].first),
            "regimes ascend in their first input size"
        );
        Self { platform, regimes }
    }

    /// Index of the regime for this input, and whether this is the smallest
    /// tested input in that regime.
    fn regime_index_for(&self, size_index: usize) -> (usize, bool) {
        let bytes = INPUT_SIZES[size_index].bytes;
        let index = self
            .regimes
            .iter()
            .rposition(|regime| bytes >= regime.first)
            .expect("the first regime starts at 0");
        let first_in_regime = size_index == 0
            || self.regime_index_for(size_index - 1).0 != index;
        (index, first_in_regime)
    }
}

/*
 * Code paths of the crates.io blake3 crate, from BLAKE3 v1.8.7's
 * src/platform.rs and the SIMD hash_many fallback chains. One chunk is
 * 1024 bytes; hash_many batches whole chunks and hands leftovers below the
 * SIMD degree to the single-chunk path.
 */
fn detect_blake3_implementation() -> Implementation {
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        let (platform, one, wide, degree) = if std::arch::is_x86_feature_detected!("avx512f")
            && std::arch::is_x86_feature_detected!("avx512vl")
        {
            ("AVX-512", "AVX-512 compression", "AVX-512 hash_many (16-way)", 16)
        } else if std::arch::is_x86_feature_detected!("avx2") {
            ("AVX2", "SSE4.1 compression", "AVX2 hash_many (8-way)", 8)
        } else if std::arch::is_x86_feature_detected!("sse4.1") {
            ("SSE4.1", "SSE4.1 compression", "SSE4.1 hash_many (4-way)", 4)
        } else if std::arch::is_x86_feature_detected!("sse2") {
            ("SSE2", "SSE2 compression", "SSE2 hash_many (4-way)", 4)
        } else {
            ("portable", "portable compression", "portable hash_many", 1)
        };
        let mut regimes = vec![Regime {
            first: 0,
            name: one,
            why: "Up to one chunk, so a single compression handles the whole input.",
            mark: Mark::Circle,
        }];
        if degree > 4 {
            regimes.push(Regime {
                first: 4 * 1024,
                name: "SSE4.1 hash_many (4-way fallback)",
                why: "Four whole chunks fill the narrowest SIMD batch; wider batches wait for more chunks.",
                mark: Mark::Diamond,
            });
        }
        if degree > 1 {
            regimes.push(Regime {
                first: degree * 1024,
                name: wide,
                why: "Enough whole chunks to fill the widest SIMD batch on this CPU.",
                mark: if degree > 4 { Mark::Square } else { Mark::Diamond },
            });
        }
        return Implementation::new(platform, regimes);
    }

    #[cfg(target_arch = "aarch64")]
    {
        return Implementation::new(
            "NEON",
            vec![
                Regime {
                    first: 0,
                    name: "portable compression",
                    why: "Fewer than four whole chunks: each runs through the portable single-chunk compressor, so 2 KiB and 3 KiB take this path too.",
                    mark: Mark::Circle,
                },
                Regime {
                    first: 4 * 1024,
                    name: "NEON hash_many (4-way)",
                    why: "Four whole chunks fill a NEON batch; from here the bulk of the input runs four chunks at a time.",
                    mark: Mark::Diamond,
                },
            ],
        );
    }

    #[allow(unreachable_code)]
    Implementation::new(
        "portable",
        vec![Regime {
            first: 0,
            name: "portable compression",
            why: "This build has no SIMD path; every size runs the portable compressor.",
            mark: Mark::Circle,
        }],
    )
}

/*
 * Code paths of the SME2 fork, from its src/ffi_sme2.rs and
 * src/ffi_neon_hybrid.rs. main() has asserted Platform::SME2.
 */
fn detect_blake3_sme2_implementation() -> Implementation {
    Implementation::new(
        "SME2",
        vec![
            Regime {
                first: 0,
                name: "scalar kernel k1",
                why: "One chunk runs on the integer ALUs alone.",
                mark: Mark::Circle,
            },
            Regime {
                first: 2 * 1024,
                name: "integer + NEON hybrid kernels",
                why: "Two or more whole chunks: hybrid kernels keep the integer and NEON units busy together, up to fifteen chunks at a time.",
                mark: Mark::Diamond,
            },
            Regime {
                first: 16 * 1024,
                name: "SME2 hash16_chunks kernel",
                why: "Sixteen whole chunks fill an SME2 group on 512-bit streaming vectors; leftovers below sixteen stay on the hybrid kernels.",
                mark: Mark::Square,
            },
        ],
    )
}

/*
 * SHA-256 and SHA-1DC each run one code path at every size. Their regimes
 * still carry a name so the hover panel can say what produced the point.
 */
fn detect_sha256_implementation() -> Implementation {
    let name = if cfg!(target_arch = "aarch64") {
        "ARMv8 SHA-256 instructions (sha2-asm)"
    } else if cfg!(any(target_arch = "x86", target_arch = "x86_64")) {
        "sha2-asm x86-64 assembly (SHA-NI where present)"
    } else {
        "sha2 portable"
    };
    Implementation::new(
        "sha2",
        vec![Regime {
            first: 0,
            name,
            why: "One implementation at every size.",
            mark: Mark::Circle,
        }],
    )
}

fn detect_sha1dc_implementation() -> Implementation {
    Implementation::new(
        "sha1-checked",
        vec![Regime {
            first: 0,
            name: "SHA-1 with collision detection, pure Rust",
            why: "One implementation at every size.",
            mark: Mark::Circle,
        }],
    )
}

fn detect_common_crypto_implementation() -> Implementation {
    Implementation::new(
        "CommonCrypto",
        vec![Regime {
            first: 0,
            name: "CC_SHA256_Init/Update/Final (corecrypto, ARMv8 SHA-256 instructions)",
            why: "One implementation at every size.",
            mark: Mark::Circle,
        }],
    )
}

fn detect_implementation(algorithm: Algorithm) -> Implementation {
    match algorithm {
        Algorithm::Blake3 => detect_blake3_implementation(),
        Algorithm::Sha256 => detect_sha256_implementation(),
        Algorithm::Sha1Dc => detect_sha1dc_implementation(),
        Algorithm::Blake3Sme2 => detect_blake3_sme2_implementation(),
        Algorithm::Sha256CommonCrypto => detect_common_crypto_implementation(),
    }
}

fn append_implementation_report(output: &mut String, algorithm: Algorithm) {
    let implementation = detect_implementation(algorithm);

    writeln!(output, "{} implementation selection:", algorithm.name()).unwrap();
    if algorithm == Algorithm::Blake3Sme2 {
        let platform = blake3_sme2::platform::Platform::detect();
        writeln!(
            output,
            "  selected platform: {platform:?} (512-bit streaming vectors, \
             sixteen-lane groups, hash_many degree {})",
            platform.simd_degree(),
        )
            .unwrap();
    } else {
        writeln!(output, "  selected platform: {}", implementation.platform).unwrap();
    }

    for (size_index, input_size) in INPUT_SIZES.iter().enumerate() {
        let (regime_index, first) = implementation.regime_index_for(size_index);
        let regime = &implementation.regimes[regime_index];
        writeln!(
            output,
            "  {:>7}: {}{}",
            input_size.label,
            regime.name,
            if first && regime_index > 0 { "  ← new path from here" } else { "" },
        )
            .unwrap();
    }

    writeln!(output).unwrap();
}

fn generate_text(
    roster: &Roster,
    results: &Results,
    machine: &MachineMetadata,
    selection_note: &str,
) -> String {
    let mut output = String::new();

    writeln!(output, "TIMESTAMP: {}", machine.timestamp).unwrap();
    writeln!(output, "git source: {GIT_SOURCE}").unwrap();
    writeln!(output, "git commit: {GIT_COMMIT}").unwrap();
    writeln!(output, "git tag: {GIT_TAG}").unwrap();
    writeln!(
        output,
        "git clean status: {GIT_CLEAN_STATUS}"
    )
        .unwrap();
    writeln!(
        output,
        "bench-hashes version: {BENCH_VERSION}"
    )
        .unwrap();
    writeln!(output, "CPU type: {}", machine.cpu_type).unwrap();
    writeln!(output, "CPU count: {}", machine.cpu_count).unwrap();
    writeln!(output, "OS type: {}", machine.os_type).unwrap();
    writeln!(output, "Rust compiler: {RUSTC_VERSION}").unwrap();
    writeln!(output, "Build target: {BUILD_TARGET}").unwrap();
    writeln!(output, "Target features: {TARGET_FEATURES}").unwrap();
    writeln!(output, "Contenders: {}", selection_note).unwrap();
    for algorithm in &roster.algorithms {
        writeln!(output, "{} source: {}", algorithm.name(), algorithm.source()).unwrap();
    }
    writeln!(
        output,
        "SHA-256 assembly source: {SHA2_ASM_SOURCE_INFO}"
    )
        .unwrap();
    for algorithm in &roster.algorithms {
        writeln!(output, "{} mode: {}", algorithm.name(), algorithm.mode()).unwrap();
    }
    writeln!(output).unwrap();

    for algorithm in &roster.algorithms {
        if detect_implementation(*algorithm).regimes.len() > 1 {
            append_implementation_report(&mut output, *algorithm);
        }
    }

    writeln!(
        output,
        "============================================================"
    )
        .unwrap();
    writeln!(output, "BENCHMARK SUMMARY").unwrap();
    writeln!(
        output,
        "============================================================"
    )
        .unwrap();
    writeln!(output).unwrap();

    writeln!(
        output,
        "Time per byte in ns/B: median, with minimum–maximum beneath; lower is better."
    )
        .unwrap();
    writeln!(output).unwrap();

    /* Header row: one column per contender; wide names get a short form. */
    write!(output, "  {:<8}", "size").unwrap();
    for algorithm in &roster.algorithms {
        write!(output, "  {:>13}", column_heading(*algorithm)).unwrap();
    }
    writeln!(output).unwrap();

    for size_index in 0..INPUT_COUNT {
        write!(output, "  {:<8}", INPUT_SIZES[size_index].label).unwrap();
        for algorithm_index in 0..roster.len() {
            write!(
                output,
                "  {:>13.3}",
                results[algorithm_index][size_index].median,
            )
                .unwrap();
        }
        writeln!(output).unwrap();

        write!(output, "  {:<8}", "").unwrap();
        for algorithm_index in 0..roster.len() {
            let statistics = results[algorithm_index][size_index];
            /* A trailing mark flags a wide spread; the legend below explains it. */
            let flag = if spread(statistics) >= SPREAD_WIDE { "!" } else { " " };
            write!(
                output,
                "  {:>12}{flag}",
                format!("{:.3}–{:.3}", statistics.minimum, statistics.maximum),
            )
                .unwrap();
        }
        writeln!(output).unwrap();
    }

    let wide_cells = results
        .iter()
        .flatten()
        .filter(|statistics| spread(**statistics) >= SPREAD_WIDE)
        .count();
    writeln!(output).unwrap();
    if wide_cells > 0 {
        writeln!(
            output,
            "!  marks a cell whose minimum–maximum spread is at least {:.0}% of its median: {wide_cells} of {} cells; treat those medians as low precision.",
            SPREAD_WIDE * 100.0,
            INPUT_COUNT * roster.len(),
        )
            .unwrap();
    } else {
        writeln!(
            output,
            "Every cell's minimum–maximum spread is under {:.0}% of its median.",
            SPREAD_WIDE * 100.0,
        )
            .unwrap();
    }
    writeln!(output).unwrap();
    writeln!(output, "{}", generate_takeaway(roster, results)).unwrap();
    writeln!(output).unwrap();

    output
}

/// Column heading that fits the 13-character summary columns.
fn column_heading(algorithm: Algorithm) -> &'static str {
    match algorithm {
        Algorithm::Sha256CommonCrypto => "SHA-256 CC",
        other => other.name(),
    }
}

fn machine_metadata() -> MachineMetadata {
    let mut system = System::new_all();
    system.refresh_all();

    let cpus = system.cpus();

    assert!(
        !cpus.is_empty(),
        "the operating system must report at least one logical CPU"
    );

    let cpu_type = cpus
        .iter()
        .map(|cpu| cpu.brand().trim())
        .find(|brand| !brand.is_empty())
        .unwrap_or(std::env::consts::ARCH)
        .to_owned();

    let kernel = System::kernel_version()
        .unwrap_or_else(|| "unreported".to_owned());

    let os_type = if std::env::consts::OS == "macos" {
        let kernel_major = kernel
            .split('.')
            .next()
            .expect("kernel version must have a first component");

        format!("darwin{kernel_major}")
    } else {
        format!("{}{}", std::env::consts::OS, kernel)
    };

    MachineMetadata {
        timestamp: utc_timestamp(),
        cpu_type,
        cpu_count: cpus.len(),
        os_type,
    }
}

fn utc_timestamp() -> String {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock must be after the Unix epoch");

    assert!(
        duration.as_secs() <= i64::MAX as u64,
        "system time must fit in the Gregorian conversion"
    );

    let total_seconds = duration.as_secs() as i64;
    let days = total_seconds.div_euclid(86_400);
    let seconds_in_day = total_seconds.rem_euclid(86_400);

    let hour = seconds_in_day / 3600;
    let minute = (seconds_in_day % 3600) / 60;
    let second = seconds_in_day % 60;

    let (year, month, day) =
        civil_date_from_unix_days(days);

    format!(
        "{year:04}-{month:02}-{day:02} \
         {hour:02}:{minute:02}:{second:02} UTC"
    )
}

fn civil_date_from_unix_days(unix_days: i64) -> (i64, i64, i64) {
    let shifted = unix_days + 719_468;

    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;

    let day_of_era = shifted - era * 146_097;

    let year_of_era = (
        day_of_era
            - day_of_era / 1460
            + day_of_era / 36_524
            - day_of_era / 146_096
    ) / 365;

    let mut year = year_of_era + era * 400;

    let day_of_year = day_of_era
        - (
            365 * year_of_era
                + year_of_era / 4
                - year_of_era / 100
        );

    let month_prime = (5 * day_of_year + 2) / 153;
    let day =
        day_of_year - (153 * month_prime + 2) / 5 + 1;

    let month =
        month_prime + if month_prime < 10 { 3 } else { -9 };

    if month <= 2 {
        year += 1;
    }

    (year, month, day)
}

fn x_fraction(bytes: usize) -> f64 {
    let smallest = (INPUT_SIZES[0].bytes as f64).log2();
    let largest =
        (INPUT_SIZES[INPUT_COUNT - 1].bytes as f64).log2();

    ((bytes as f64).log2() - smallest) / (largest - smallest)
}

fn gigabytes_per_second(ns_per_byte: f64) -> String {
    assert!(ns_per_byte > 0.0);

    let throughput = 1.0 / ns_per_byte;

    if throughput >= 10.0 {
        format!("{throughput:.0} GB/s")
    } else {
        format!("{throughput:.1} GB/s")
    }
}

/*
 * Speed of each contender relative to the baseline at each size: baseline
 * time divided by contender time. Above 1.0 means the contender is faster
 * than the baseline; the baseline's own ratio is exactly 1.0.
 */
/// ratios[contender][size] = baseline median / contender median.
fn median_ratios(roster: &Roster, results: &Results) -> Vec<[f64; INPUT_COUNT]> {
    (0..roster.len())
        .map(|algorithm_index| {
            std::array::from_fn(|size_index| {
                results[roster.baseline][size_index].median
                    / results[algorithm_index][size_index].median
            })
        })
        .collect()
}

fn generate_takeaway(roster: &Roster, results: &Results) -> String {
    let clauses: Vec<String> = (0..roster.len())
        .filter(|&algorithm_index| algorithm_index != roster.baseline)
        .map(|algorithm_index| takeaway_clause(roster, results, algorithm_index))
        .collect();

    format!("On this machine: {}", clauses.join("; "))
}

/*
 * Split the headline at clause boundaries into at most two lines that fit
 * the canvas width at the headline's font size. The script mirrors this.
 */
fn wrap_takeaway(takeaway: &str) -> Vec<String> {
    const MAX_CHARS: usize = 118;
    let mut lines: Vec<String> = Vec::new();
    for clause in takeaway.split("; ") {
        match lines.last_mut() {
            Some(last) if last.len() + 2 + clause.len() <= MAX_CHARS => {
                last.push_str("; ");
                last.push_str(clause);
            }
            _ => lines.push(clause.to_owned()),
        }
    }
    lines
}

/*
 * One contender's speed relative to the baseline, phrased for the headline.
 * Requires a non-baseline contender.
 */
fn takeaway_clause(roster: &Roster, results: &Results, algorithm_index: usize) -> String {
    assert_ne!(algorithm_index, roster.baseline, "the baseline has no clause of its own");

    let ratios = median_ratios(roster, results);
    let baseline = roster.algorithms[roster.baseline].name();

    {
        let name = roster.algorithms[algorithm_index].name();
        let column: Vec<f64> = ratios[algorithm_index].to_vec();
        let lowest = column.iter().copied().fold(f64::INFINITY, f64::min);
        let highest = column.iter().copied().fold(0.0_f64, f64::max);

        /*
         * ratio = baseline time / contender time. Above 1.0 the contender is
         * faster than the baseline; below 1.0 the baseline is faster.
         */
        /*
         * Within a few percent the two are indistinguishable at this
         * benchmark's precision; say so rather than pick a winner.
         */
        const TIE: f64 = 0.05;
        let tied_low = lowest > 1.0 - TIE;
        let tied_high = highest < 1.0 + TIE;

        let clause = if tied_low && tied_high {
            format!("{name} matches {baseline} at every size")
        } else if lowest > 1.0 + TIE {
            format!("{name} is {lowest:.2}× to {highest:.2}× faster than {baseline}")
        } else if highest < 1.0 - TIE {
            format!(
                "{baseline} is {:.2}× to {:.2}× faster than {name}",
                1.0 / highest,
                1.0 / lowest,
            )
        } else if tied_low {
            /* Never slower than the baseline; faster from some size up. */
            let first_faster = column.iter().position(|r| *r > 1.0 + TIE).expect("some ratio is above the tie band");
            format!(
                "{name} matches {baseline} below {} and is up to {highest:.2}× faster from there",
                INPUT_SIZES[first_faster].label,
            )
        } else if tied_high {
            let last_slower = column.iter().rposition(|r| *r < 1.0 - TIE).expect("some ratio is below the tie band");
            format!(
                "{baseline} is up to {:.2}× faster than {name} through {}, then they match",
                1.0 / lowest,
                INPUT_SIZES[last_slower].label,
            )
        } else {
            let first = if column[0] > 1.0 { name } else { baseline };
            let last = if column[INPUT_COUNT - 1] > 1.0 { name } else { baseline };
            format!(
                "{first} is faster at {}, {last} at {}",
                INPUT_SIZES[0].label,
                INPUT_SIZES[INPUT_COUNT - 1].label,
            )
        };

        clause
    }
}

fn sanitize_alphanumeric(input: &str) -> String {
    let sanitized: String = input
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect();

    assert!(
        !sanitized.is_empty(),
        "sanitized identifier must not be empty; input was {input:?}"
    );

    sanitized
}

fn output_directory(machine: &MachineMetadata) -> std::path::PathBuf {
    let cpu = sanitize_alphanumeric(&machine.cpu_type);
    let os = sanitize_alphanumeric(&machine.os_type);

    std::path::PathBuf::from("benchmark-results")
        .join(format!("{cpu}.{os}"))
}

/*
 * Layout constants shared by the static geometry (Rust) and the interactive
 * relayout (JavaScript inside the SVG). The script receives them through a
 * JSON block, so a single source of truth drives both.
 */
const SVG_WIDTH: f64 = 1200.0;
/// Canvas height: the provenance block ends where the lines end, with the
/// bottom margin that follows.
fn svg_height(provenance_lines: usize) -> f64 {
    provenance_line_y(provenance_lines.saturating_sub(1)) + PROVENANCE_LINE_HEIGHT + 8.0
}
const PLOT_LEFT: f64 = 110.0;
const PLOT_RIGHT: f64 = 1000.0;
const PLOT_TOP: f64 = 150.0;
const PLOT_BOTTOM: f64 = 455.0;
const X_INSET: f64 = 40.0;
const SERIES_LABEL_GAP: f64 = 44.0;
const PROVENANCE_TOP: f64 = PLOT_BOTTOM + 85.0;
const PROVENANCE_LINE_HEIGHT: f64 = 14.0;

/*
 * The y axis spans [axis_min, axis_max] on a log scale, with room below
 * the smallest minimum and above the largest maximum. The script applies
 * the same rule to whichever contenders are on, so the axis reflects the
 * visible data alone.
 */
fn log_axis_bounds(observed_min: f64, observed_max: f64) -> (f64, f64) {
    assert!(
        observed_min.is_finite() && observed_min > 0.0,
        "log axis requires positive measurements"
    );
    assert!(
        observed_max.is_finite() && observed_max >= observed_min,
        "graph maximum must be finite and at least the minimum"
    );

    (
        nice_log_bound_below(observed_min * 0.92),
        nice_log_bound_above(observed_max * 1.08),
    )
}

fn generate_svg(
    roster: &Roster,
    results: &Results,
    machine: &MachineMetadata,
    selection_note: &str,
) -> String {
    assert!(
        roster.len() >= 2,
        "the takeaway needs a baseline and at least one other contender"
    );

    let observed_max = results
        .iter()
        .flatten()
        .map(|statistics| statistics.maximum)
        .fold(0.0_f64, f64::max);

    let observed_min = results
        .iter()
        .flatten()
        .map(|statistics| statistics.minimum)
        .fold(f64::INFINITY, f64::min);

    let (axis_min, axis_max) = log_axis_bounds(observed_min, observed_max);

    let log_min = axis_min.ln();
    let log_max = axis_max.ln();

    let map_y = |value: f64| {
        assert!(value > 0.0);

        PLOT_BOTTOM
            - (value.ln() - log_min) / (log_max - log_min)
            * (PLOT_BOTTOM - PLOT_TOP)
    };

    let x_positions: [f64; INPUT_COUNT] =
        std::array::from_fn(|index| {
            PLOT_LEFT
                + X_INSET
                + x_fraction(INPUT_SIZES[index].bytes)
                * (PLOT_RIGHT - PLOT_LEFT - 2.0 * X_INSET)
        });

    let implementations: Vec<Implementation> =
        roster.algorithms.iter().map(|&algorithm| detect_implementation(algorithm)).collect();
    let takeaway = generate_takeaway(roster, results);

    let provenance_shared = shared_provenance_lines(machine, selection_note);
    let provenance_total = provenance_shared.len()
        + roster
            .algorithms
            .iter()
            .zip(&implementations)
            .map(|(&algorithm, implementation)| contender_provenance_lines(algorithm, implementation).len())
            .sum::<usize>();
    let svg_height = svg_height(provenance_total);

    let mut svg = String::new();

    writeln!(svg, r##"<?xml version="1.0" encoding="UTF-8"?>"##)
        .unwrap();

    writeln!(
        svg,
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {SVG_WIDTH:.0} {svg_height:.0}" width="{SVG_WIDTH:.0}" height="{svg_height:.0}">"##
    )
        .unwrap();

    writeln!(
        svg,
        r##"  <rect width="{SVG_WIDTH:.0}" height="{svg_height:.0}" fill="#fdfdfc"/>"##
    )
        .unwrap();

    svg.push_str(
        r##"  <style>
    text { font-family: -apple-system, "Segoe UI", "Helvetica Neue", Arial, sans-serif; }
    .title { font-size: 22px; font-weight: 700; fill: #1a1a1a; }
    .takeaway { font-size: 14px; font-weight: 600; fill: #3a3a3a; }
    .method { font-size: 11px; fill: #8a8a8a; }
    .axis-title { font-size: 12px; fill: #666666; }
    .tick-label { font-size: 11px; fill: #777777; }
    .size-label { font-size: 11px; font-weight: 600; fill: #333333; }
    .size-tick { stroke: #bbbbbb; stroke-width: 1; }
    .value-label { font-size: 10px; font-weight: 700; }
    .series-name { font-size: 13px; font-weight: 700; }
    .series-detail { font-size: 10px; fill: #777777; }
    .series-hint { font-size: 9px; fill: #b0b0b0; }
    .annotation { font-size: 10px; font-style: italic; fill: #8a8a8a; }
    .prov-head { font-size: 10px; font-weight: 700; fill: #aaaaaa; letter-spacing: 0.1em; }
    .prov { font-size: 9px; fill: #9a9a9a; }
    .grid { stroke: #e8e8e6; stroke-width: 1; }
    .grid-x { stroke: #f0f0ee; stroke-width: 1; }
    .axis { stroke: #55555a; stroke-width: 1; }
    .divider { stroke: #e0e0de; stroke-width: 1; }
    .series-label { cursor: pointer; transition: transform 0.3s ease; }
    .series-label:hover .series-name { text-decoration: underline; }
    .series-hint { display: none; }
    .marks, .dots { transition: opacity 0.3s ease; }
    .marks { pointer-events: none; }
    .series[data-on="false"] .marks, .dots[data-on="false"] { opacity: 0; pointer-events: none; }
    .series[data-on="false"] .series-name { fill: #9a9a9a; }
    .series[data-on="false"] .series-detail { display: none; }
    .series[data-on="false"] .series-hint { display: inline; }
    .series[data-on="false"] .series-prov { display: none; }
    .series[data-on="false"] .series-swatch { fill: #fdfdfc; }
    .series-swatch { stroke-width: 2; transition: fill 0.3s ease; }
    .dot { cursor: crosshair; }
    .dot-ring { stroke-width: 1.5; stroke-opacity: 0.55; }
    .dot:hover .dot-ring { stroke-opacity: 1; }
    .legend { font-size: 10px; fill: #8a8a8a; }
    #hover { pointer-events: none; }
    #hover-guide { stroke: #9a9a9a; stroke-width: 1; stroke-dasharray: 3,3; }
    #hover-box { fill: #ffffff; fill-opacity: 0.97; stroke: #c8c8c4; stroke-width: 1; }
    .hover-head { font-size: 12px; font-weight: 700; fill: #1a1a1a; }
    .hover-sub { font-size: 10px; fill: #777777; }
    .hover-row { font-size: 11px; fill: #333333; }
    .hover-row-focus { font-weight: 700; }
    .hover-ratio { font-size: 11px; font-weight: 600; }
    .hover-note { font-size: 9px; font-style: italic; fill: #9a9a9a; }
    .hover-path { font-weight: 700; fill: #333333; }
    .hover-why { font-size: 10px; fill: #555555; }
  </style>
"##,
    );

    writeln!(
        svg,
        r##"  <text x="{PLOT_LEFT:.0}" y="44" class="title">Cryptographic Hash Time per Byte</text>"##
    )
        .unwrap();

    /*
     * The headline wraps onto up to two lines at the semicolons; the script
     * re-wraps it the same way after each toggle.
     */
    writeln!(svg, r##"  <text id="takeaway" x="{PLOT_LEFT:.0}" y="70" class="takeaway">"##).unwrap();
    for (index, line) in wrap_takeaway(&takeaway).iter().enumerate() {
        writeln!(
            svg,
            r##"    <tspan x="{PLOT_LEFT:.0}" dy="{}">{}</tspan>"##,
            if index == 0 { 0 } else { 18 },
            xml_escape(line),
        )
            .unwrap();
    }
    writeln!(svg, "  </text>").unwrap();

    writeln!(
        svg,
        r##"  <text x="{PLOT_LEFT:.0}" y="108" class="method">Line and dot: median · shaded band: minimum–maximum across {SAMPLE_ROUNDS} interleaved samples; a deeper tint or dashed outline marks a wide spread, meaning lower precision · single-threaded · lower is better</text>"##
    )
        .unwrap();
    writeln!(
        svg,
        r##"  <text x="{PLOT_LEFT:.0}" y="123" class="method">Dot shape marks the code path a contender used at that size; a ringed dot is where a new path begins · hover any dot to compare contenders and see the path · click a name at right to hide or show that contender</text>"##
    )
        .unwrap();

    /* Horizontal grid and y-axis tick labels; the script rebuilds these. */
    writeln!(svg, r##"  <g id="y-axis">"##).unwrap();
    for value in log_ticks(axis_min, axis_max) {
        let y = map_y(value);
        writeln!(
            svg,
            r##"    <line x1="{PLOT_LEFT:.1}" y1="{y:.2}" x2="{PLOT_RIGHT:.1}" y2="{y:.2}" class="grid"/>"##
        )
            .unwrap();

        writeln!(
            svg,
            r##"    <text x="{:.1}" y="{:.2}" class="tick-label" text-anchor="end">{}</text>"##,
            PLOT_LEFT - 10.0,
            y + 3.5,
            format_tick(value),
        )
            .unwrap();
    }
    writeln!(svg, "  </g>").unwrap();

    writeln!(
        svg,
        r##"  <text x="30" y="{:.1}" class="axis-title" text-anchor="middle" transform="rotate(-90 30 {:.1})">Nanoseconds per byte</text>"##,
        (PLOT_TOP + PLOT_BOTTOM) / 2.0,
        (PLOT_TOP + PLOT_BOTTOM) / 2.0,
    )
        .unwrap();

    /*
     * Vertical guides and x-axis labels at each tested size. Where a label
     * would run into its left neighbour (3 KiB sits 24 px from 4 KiB on the
     * log axis), it drops to a second row with a short tick joining it to
     * its column.
     */
    const SIZE_LABEL_WIDTH: f64 = 34.0;
    let mut label_rows = [0u8; INPUT_COUNT];
    for size_index in 1..INPUT_COUNT {
        let gap = x_positions[size_index] - x_positions[size_index - 1];
        if gap < SIZE_LABEL_WIDTH && label_rows[size_index - 1] == 0 {
            label_rows[size_index] = 1;
        }
    }
    for size_index in 0..INPUT_COUNT {
        let x = x_positions[size_index];
        let row = label_rows[size_index];
        let label_y = PLOT_BOTTOM + 24.0 + 13.0 * f64::from(row);

        writeln!(
            svg,
            r##"  <line x1="{x:.2}" y1="{PLOT_TOP:.1}" x2="{x:.2}" y2="{PLOT_BOTTOM:.1}" class="grid-x"/>"##
        )
            .unwrap();

        if row == 1 {
            writeln!(
                svg,
                r##"  <line x1="{x:.2}" y1="{:.1}" x2="{x:.2}" y2="{:.1}" class="size-tick"/>"##,
                PLOT_BOTTOM + 4.0,
                label_y - 10.0,
            )
                .unwrap();
        }

        writeln!(
            svg,
            r##"  <text x="{x:.2}" y="{label_y:.1}" class="size-label" text-anchor="middle">{}</text>"##,
            xml_escape(INPUT_SIZES[size_index].label),
        )
            .unwrap();
    }

    writeln!(
        svg,
        r##"  <text x="{:.1}" y="{:.1}" class="axis-title" text-anchor="middle">Input size (logarithmic spacing)</text>"##,
        (PLOT_LEFT + PLOT_RIGHT) / 2.0,
        PLOT_BOTTOM + 52.0,
    )
        .unwrap();

    writeln!(
        svg,
        r##"  <line x1="{PLOT_LEFT:.1}" y1="{PLOT_TOP:.1}" x2="{PLOT_LEFT:.1}" y2="{PLOT_BOTTOM:.1}" class="axis"/>"##
    )
        .unwrap();

    writeln!(
        svg,
        r##"  <line x1="{PLOT_LEFT:.1}" y1="{PLOT_BOTTOM:.1}" x2="{PLOT_RIGHT:.1}" y2="{PLOT_BOTTOM:.1}" class="axis"/>"##
    )
        .unwrap();

    /*
     * Right-edge series labels double as toggles. Each sits level with its
     * line's last point, pushed apart when medians nearly coincide. The
     * script repeats this rule after each toggle; a hidden contender keeps
     * its slot, anchored where its line would end on the current axis and
     * clamped to the plot edge, so its grey label points toward its data.
     */
    let mut label_slots: Vec<(usize, f64)> = (0..roster.len())
        .map(|algorithm_index| {
            (
                algorithm_index,
                map_y(results[algorithm_index][INPUT_COUNT - 1].median),
            )
        })
        .collect();

    label_slots.sort_by(|a, b| a.1.total_cmp(&b.1));

    for index in 1..label_slots.len() {
        let minimum_y = label_slots[index - 1].1 + SERIES_LABEL_GAP;

        if label_slots[index].1 < minimum_y {
            label_slots[index].1 = minimum_y;
        }
    }

    let mut label_y_by_algorithm = vec![0.0_f64; roster.len()];
    for (algorithm_index, label_y) in &label_slots {
        label_y_by_algorithm[*algorithm_index] = *label_y;
    }

    /*
     * One group per contender holds everything that belongs to it: band,
     * line, dots, value labels, the clickable label at right, and its
     * provenance line. Toggling flips one attribute on the group.
     */
    let shared_count = provenance_shared.len();
    let mut provenance_slot = shared_count;

    let value_label_y = place_value_labels(roster, results, &map_y);

    /*
     * Dots are collected here and emitted after every series' band and
     * line, so no band can sit above another contender's dots and take
     * the hover. Each dot layer carries its series index; the script and
     * stylesheet treat it as part of that series.
     */
    let mut dot_layers: Vec<String> = (0..roster.len())
        .map(|algorithm_index| {
            format!("  <g class=\"dots\" id=\"dots-{algorithm_index}\" data-on=\"true\">\n")
        })
        .collect();

    for algorithm_index in 0..roster.len() {
        let algorithm = roster.algorithms[algorithm_index];
        let color = algorithm.color();
        let implementation = &implementations[algorithm_index];

        writeln!(
            svg,
            r##"  <g class="series" id="series-{algorithm_index}" data-on="true">"##
        )
            .unwrap();

        writeln!(svg, r##"    <g class="marks">"##).unwrap();

        let mut band = String::new();
        for size_index in 0..INPUT_COUNT {
            let x = x_positions[size_index];
            let y = map_y(results[algorithm_index][size_index].maximum);
            if size_index == 0 {
                write!(band, "M {x:.2} {y:.2}").unwrap();
            } else {
                write!(band, " L {x:.2} {y:.2}").unwrap();
            }
        }
        for size_index in (0..INPUT_COUNT).rev() {
            let x = x_positions[size_index];
            let y = map_y(results[algorithm_index][size_index].minimum);
            write!(band, " L {x:.2} {y:.2}").unwrap();
        }
        band.push_str(" Z");

        /*
         * The band's look reports the run's precision for this contender.
         * Spread is (max − min) / median at a size; the band takes the
         * worst spread across sizes. Tight runs stay a faint tint. As the
         * spread grows the tint deepens, and past the wide threshold a
         * dashed outline appears, so a broad band cannot pass as decor.
         */
        let worst_spread = (0..INPUT_COUNT)
            .map(|size_index| spread(results[algorithm_index][size_index]))
            .fold(0.0_f64, f64::max);
        let (opacity, outline) = band_style(worst_spread);

        writeln!(
            svg,
            r##"      <path class="band" d="{band}" fill="{color}" fill-opacity="{opacity:.2}" stroke="{color}" stroke-opacity="{}" stroke-width="1" stroke-dasharray="4,3"/>"##,
            if outline { "0.6" } else { "0" },
        )
            .unwrap();

        let mut path = String::new();
        for size_index in 0..INPUT_COUNT {
            let x = x_positions[size_index];
            let y = map_y(results[algorithm_index][size_index].median);
            if size_index == 0 {
                write!(path, "M {x:.2} {y:.2}").unwrap();
            } else {
                write!(path, " L {x:.2} {y:.2}").unwrap();
            }
        }

        writeln!(
            svg,
            r##"      <path class="median" d="{path}" fill="none" stroke="{color}" stroke-width="2.5" stroke-linejoin="round" stroke-linecap="round"/>"##
        )
            .unwrap();

        for size_index in 0..INPUT_COUNT {
            let x = x_positions[size_index];
            let statistics = results[algorithm_index][size_index];
            let median_y = map_y(statistics.median);

            /*
             * The dot's shape names the code path that produced this point;
             * the first dot of a new path is drawn larger, with a ring, as
             * the place to hover for the explanation.
             */
            let (regime_index, first_in_regime) = implementation.regime_index_for(size_index);
            let regime = &implementation.regimes[regime_index];
            let transition = first_in_regime && regime_index > 0;
            let dots = &mut dot_layers[algorithm_index];
            writeln!(
                dots,
                r##"    <g class="dot{}" data-size="{size_index}" transform="translate({x:.2} {median_y:.2})" onmouseenter="showHover({algorithm_index},{size_index})" onmouseleave="hideHover()">"##,
                if transition { " dot-transition" } else { "" },
            )
                .unwrap();
            if transition {
                writeln!(
                    dots,
                    r##"      <circle class="dot-ring" r="9.5" fill="none" stroke="{color}"/>"##
                )
                    .unwrap();
            }
            writeln!(dots, "      {}", mark_shape(regime.mark, color, if transition { 6.0 } else { 5.0 })).unwrap();
            dots.push_str("    </g>\n");

            /*
             * With sixteen columns, a value at every dot would overprint.
             * Label the ends and every fourth size; hovering a dot shows
             * the rest. Edge columns anchor inward so labels stay clear of
             * the y-axis gutter and the series labels at right.
             */
            let labeled = size_index == 0
                || size_index == INPUT_COUNT - 1
                || size_index % 4 == 0;

            if !labeled {
                continue;
            }

            let (label_x, anchor) = if size_index == 0 {
                (x + 9.0, "start")
            } else if size_index == INPUT_COUNT - 1 {
                (x - 9.0, "end")
            } else {
                (x, "middle")
            };

            writeln!(
                svg,
                r##"      <text class="value-label" data-size="{size_index}" x="{label_x:.2}" y="{:.2}" fill="{color}" text-anchor="{anchor}">{}</text>"##,
                value_label_y[algorithm_index][size_index],
                format_result_value(statistics.median),
            )
                .unwrap();
        }

        writeln!(svg, "    </g>").unwrap();

        /* Clickable label at right: swatch, name, detail, hint. */
        let statistics = results[algorithm_index][INPUT_COUNT - 1];
        let label_x = PLOT_RIGHT + 14.0;
        let label_y = label_y_by_algorithm[algorithm_index];

        writeln!(
            svg,
            r##"    <g class="series-label" transform="translate(0 {label_y:.2})" onclick="toggleSeries({algorithm_index})">"##
        )
            .unwrap();
        writeln!(
            svg,
            r##"      <title>Click to hide or show {}</title>"##,
            xml_escape(algorithm.name()),
        )
            .unwrap();
        writeln!(
            svg,
            r##"      <rect x="{:.1}" y="-14" width="{:.1}" height="{:.0}" fill="transparent"/>"##,
            label_x - 4.0,
            SVG_WIDTH - label_x - 6.0,
            SERIES_LABEL_GAP - 4.0,
        )
            .unwrap();
        writeln!(
            svg,
            r##"      <circle class="series-swatch" cx="{:.1}" cy="0" r="4.5" fill="{color}" stroke="{color}"/>"##,
            label_x + 4.5,
        )
            .unwrap();
        writeln!(
            svg,
            r##"      <text class="series-name" x="{:.1}" y="4" fill="{color}">{}</text>"##,
            label_x + 14.0,
            xml_escape(algorithm.name()),
        )
            .unwrap();
        writeln!(
            svg,
            r##"      <text class="series-detail" x="{:.1}" y="18">{} ns/B · {} at {}</text>"##,
            label_x + 14.0,
            format_result_value(statistics.median),
            gigabytes_per_second(statistics.median),
            xml_escape(INPUT_SIZES[INPUT_COUNT - 1].label),
        )
            .unwrap();
        writeln!(
            svg,
            r##"      <text class="series-hint" x="{:.1}" y="18">hidden · click to show</text>"##,
            label_x + 14.0,
        )
            .unwrap();
        writeln!(svg, "    </g>").unwrap();

        /* This contender's provenance lines, hidden along with it. */
        for line in contender_provenance_lines(algorithm, implementation) {
            writeln!(
                svg,
                r##"    <text class="prov series-prov" x="{PLOT_LEFT:.1}" y="{:.1}">{}</text>"##,
                provenance_line_y(provenance_slot),
                xml_escape(&line),
            )
                .unwrap();
            provenance_slot += 1;
        }

        writeln!(svg, "  </g>").unwrap();
    }

    for layer in &dot_layers {
        svg.push_str(layer);
        svg.push_str("  </g>\n");
    }

    /*
     * Shape legend under the plot's right end: one entry per mark in use,
     * in neutral grey, since colour belongs to contenders and shape to
     * code paths.
     */
    {
        let mut marks: Vec<Mark> = Vec::new();
        for implementation in &implementations {
            for regime in &implementation.regimes {
                if !marks.contains(&regime.mark) {
                    marks.push(regime.mark);
                }
            }
        }
        let legend_y = PLOT_BOTTOM + 68.0;
        let mut x = PLOT_RIGHT;
        let entries: Vec<(Mark, &str)> = marks
            .iter()
            .enumerate()
            .map(|(index, &mark)| {
                (mark, match index { 0 => "first code path", 1 => "second", _ => "third" })
            })
            .collect();
        /* Lay out right-to-left so the row ends flush with the plot edge. */
        for (mark, label) in entries.iter().rev() {
            let label_width = label.len() as f64 * 5.6;
            x -= label_width;
            writeln!(
                svg,
                r##"  <text x="{x:.1}" y="{:.1}" class="legend">{label}</text>"##,
                legend_y,
            )
                .unwrap();
            x -= 12.0;
            writeln!(
                svg,
                r##"  <g transform="translate({x:.1} {:.1}) scale(0.75)">{}</g>"##,
                legend_y - 3.5,
                mark_shape(*mark, "#8a8a8a", 5.0),
            )
                .unwrap();
            x -= 18.0;
        }
        writeln!(
            svg,
            r##"  <g transform="translate({:.1} {:.1}) scale(0.75)"><circle r="9.5" fill="none" stroke="#8a8a8a" stroke-width="1.5"/><circle r="5" fill="#8a8a8a"/></g>"##,
            x - 6.0,
            legend_y - 3.5,
        )
            .unwrap();
        writeln!(
            svg,
            r##"  <text x="{:.1}" y="{:.1}" class="legend" text-anchor="end">a new path begins</text>"##,
            x - 18.0,
            legend_y,
        )
            .unwrap();
    }

    /*
     * Hover panel, filled by the script when a dot is hovered. Last among
     * the drawn elements so it paints over every series.
     */
    writeln!(svg, r##"  <g id="hover" style="display:none">"##).unwrap();
    writeln!(
        svg,
        r##"    <line id="hover-guide" x1="0" y1="{PLOT_TOP:.1}" x2="0" y2="{PLOT_BOTTOM:.1}"/>"##
    )
        .unwrap();
    writeln!(svg, r##"    <rect id="hover-box" x="0" y="0" width="0" height="0" rx="4"/>"##).unwrap();
    writeln!(svg, r##"    <g id="hover-body"></g>"##).unwrap();
    writeln!(svg, "  </g>").unwrap();

    /* Machine-readable provenance, complete and untruncated. */
    writeln!(svg, "  <metadata>").unwrap();

    for (name, value) in [
        ("timestamp", machine.timestamp.as_str()),
        ("git source", GIT_SOURCE),
        ("git commit", GIT_COMMIT),
        ("git tag", GIT_TAG),
        ("git clean status", GIT_CLEAN_STATUS),
        ("bench-hashes version", BENCH_VERSION),
        ("CPU type", machine.cpu_type.as_str()),
        ("OS type", machine.os_type.as_str()),
        ("Rust compiler", RUSTC_VERSION),
        ("build target", BUILD_TARGET),
        ("target features", TARGET_FEATURES),
        ("BLAKE3 source", BLAKE3_SOURCE_INFO),
        ("SHA-256 source", SHA2_SOURCE_INFO),
        ("SHA-256 assembly source", SHA2_ASM_SOURCE_INFO),
        ("SHA-1DC source", SHA1_CHECKED_SOURCE_INFO),
        ("BLAKE3 SME2 source", BLAKE3_SME2_SOURCE_INFO),
    ] {
        writeln!(
            svg,
            "    {}: {}",
            xml_escape(name),
            xml_escape(value),
        )
            .unwrap();
    }

    writeln!(svg, "  </metadata>").unwrap();

    /*
     * Human-readable provenance: left-aligned, compact, de-emphasized.
     * Shared lines first; the per-contender lines emitted above follow and
     * close ranks when a contender is hidden.
     */
    writeln!(
        svg,
        r##"  <line x1="{PLOT_LEFT:.1}" y1="{PROVENANCE_TOP:.1}" x2="{:.1}" y2="{PROVENANCE_TOP:.1}" class="divider"/>"##,
        SVG_WIDTH - PLOT_LEFT,
    )
        .unwrap();

    writeln!(
        svg,
        r##"  <text x="{PLOT_LEFT:.1}" y="{:.1}" class="prov-head">PROVENANCE</text>"##,
        PROVENANCE_TOP + 20.0,
    )
        .unwrap();

    for (index, line) in provenance_shared.iter().enumerate() {
        writeln!(
            svg,
            r##"  <text class="prov" x="{PLOT_LEFT:.1}" y="{:.1}">{}</text>"##,
            provenance_line_y(index),
            xml_escape(line),
        )
            .unwrap();
    }

    assert_eq!(
        provenance_slot, provenance_total,
        "the provenance lines emitted must match the count the canvas was sized for"
    );

    /* Data and behaviour for the interactive toggles. */
    write_interaction_script(&mut svg, roster, results, &implementations, &x_positions, &label_y_by_algorithm, shared_count);

    svg.push_str("</svg>\n");
    svg
}

/*
 * Relative spread of one cell: (max − min) / median. Zero for a perfectly
 * repeatable measurement; 0.10 means the extremes differ by a tenth of the
 * median.
 */
fn spread(statistics: Statistics) -> f64 {
    (statistics.maximum - statistics.minimum) / statistics.median
}

/*
 * Band fill opacity and whether to outline it, from the worst spread. The
 * script applies the same thresholds. Spread under 10% is a routine run;
 * 10–25% earns a deeper tint; over 25% adds the dashed outline.
 */
const SPREAD_NOTICEABLE: f64 = 0.10;
const SPREAD_WIDE: f64 = 0.25;

fn band_style(worst_spread: f64) -> (f64, bool) {
    let opacity = if worst_spread < SPREAD_NOTICEABLE {
        0.16
    } else if worst_spread < SPREAD_WIDE {
        0.16 + 0.14 * (worst_spread - SPREAD_NOTICEABLE) / (SPREAD_WIDE - SPREAD_NOTICEABLE)
    } else {
        0.30
    };
    (opacity, worst_spread >= SPREAD_WIDE)
}

/*
 * Value labels sit above their dot by default. Within a column, labels
 * are processed top to bottom; one that would land within a label height
 * of the previous label moves below its dot instead, and if that also
 * collides it steps down until clear. The script repeats this rule.
 */
const VALUE_LABEL_ABOVE: f64 = -11.0;
const VALUE_LABEL_BELOW: f64 = 17.0;
const VALUE_LABEL_HEIGHT: f64 = 11.0;

fn place_value_labels(
    roster: &Roster,
    results: &Results,
    map_y: &dyn Fn(f64) -> f64,
) -> Vec<[f64; INPUT_COUNT]> {
    let mut placed = vec![[0.0_f64; INPUT_COUNT]; roster.len()];
    for size_index in 0..INPUT_COUNT {
        let mut order: Vec<usize> = (0..roster.len()).collect();
        order.sort_by(|&a, &b| {
            results[a][size_index].median.total_cmp(&results[b][size_index].median).reverse()
        });
        /* Smallest y (fastest, highest on the plot) first. */
        order.reverse();
        let mut taken: Vec<f64> = Vec::new();
        for algorithm_index in order {
            let dot_y = map_y(results[algorithm_index][size_index].median);
            let clear = |y: f64, taken: &[f64]| taken.iter().all(|t| (t - y).abs() >= VALUE_LABEL_HEIGHT);
            let mut y = dot_y + VALUE_LABEL_ABOVE;
            if !clear(y, &taken) {
                y = dot_y + VALUE_LABEL_BELOW;
                while !clear(y, &taken) {
                    y += VALUE_LABEL_HEIGHT;
                }
            }
            taken.push(y);
            placed[algorithm_index][size_index] = y;
        }
    }
    placed
}

fn json_string(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

/*
 * A mark centred on the origin. Diamonds and squares are sized to match a
 * circle's visual weight at the same radius.
 */
fn mark_shape(mark: Mark, color: &str, radius: f64) -> String {
    let stroke = r##"stroke="#fdfdfc" stroke-width="1.5""##;
    match mark {
        Mark::Circle => format!(r##"<circle r="{radius:.1}" fill="{color}" {stroke}/>"##),
        Mark::Diamond => {
            let r = radius * 1.25;
            format!(
                r##"<path d="M 0 {a:.2} L {r:.2} 0 L 0 {r:.2} L {a:.2} 0 Z" fill="{color}" {stroke}/>"##,
                a = -r,
            )
        }
        Mark::Square => {
            let h = radius * 0.9;
            format!(
                r##"<rect x="{a:.2}" y="{a:.2}" width="{w:.2}" height="{w:.2}" fill="{color}" {stroke}/>"##,
                a = -h,
                w = 2.0 * h,
            )
        }
    }
}

fn provenance_line_y(slot: usize) -> f64 {
    PROVENANCE_TOP + 40.0 + slot as f64 * PROVENANCE_LINE_HEIGHT
}

/* Provenance that describes the run as a whole. */
fn shared_provenance_lines(machine: &MachineMetadata, selection_note: &str) -> Vec<String> {
    vec![
        format!(
            "Run: {} · bench-hashes {BENCH_VERSION}",
            machine.timestamp,
        ),
        format!("Contenders: {selection_note}"),
        format!(
            "Machine: {} · {} logical CPUs · {}",
            machine.cpu_type, machine.cpu_count, machine.os_type,
        ),
        format!("Toolchain: {RUSTC_VERSION} · {BUILD_TARGET}"),
        format!("Source: {GIT_SOURCE} @ {GIT_COMMIT}"),
        format!("Tag: {GIT_TAG} · Working tree: {GIT_CLEAN_STATUS}"),
        "Full crate checksums are in this file's metadata element".to_owned(),
    ]
}

/* Provenance that belongs to one contender and hides with it. */
fn contender_provenance_lines(
    algorithm: Algorithm,
    implementation: &Implementation,
) -> Vec<String> {
    let name = algorithm.name();
    match algorithm {
        Algorithm::Blake3 => vec![format!(
            "{name}: {} · {} · platform {}",
            package_name_and_version(BLAKE3_SOURCE_INFO),
            algorithm.mode(),
            implementation.platform,
        )],
        Algorithm::Sha256 => vec![format!(
            "{name}: {} (+{}) · {}",
            package_name_and_version(SHA2_SOURCE_INFO),
            package_name_and_version(SHA2_ASM_SOURCE_INFO),
            algorithm.mode(),
        )],
        Algorithm::Sha1Dc => vec![format!(
            "{name}: {} · {}",
            package_name_and_version(SHA1_CHECKED_SOURCE_INFO),
            algorithm.mode(),
        )],
        Algorithm::Blake3Sme2 => vec![
            format!("{name}: {}", short_git_source(BLAKE3_SME2_SOURCE_INFO)),
            format!("{name}: {}", algorithm.mode()),
        ],
        Algorithm::Sha256CommonCrypto => vec![format!("{name}: {}", algorithm.mode())],
    }
}

/*
 * The script re-derives every y position from the visible contenders'
 * data, using the same rules as the Rust layout: nice log bounds with 8%
 * headroom, the same tick mantissas, the same label stacking gap. The
 * measurements and layout constants travel as JSON so the two stay in
 * lockstep.
 */
fn write_interaction_script(
    svg: &mut String,
    roster: &Roster,
    results: &Results,
    implementations: &[Implementation],
    x_positions: &[f64; INPUT_COUNT],
    label_y_by_algorithm: &[f64],
    shared_count: usize,
) {
    let mut data = String::from("{\"series\":[");
    for algorithm_index in 0..roster.len() {
        if algorithm_index > 0 {
            data.push(',');
        }
        write!(data, "{{\"name\":\"{}\",\"regimes\":[", roster.algorithms[algorithm_index].name()).unwrap();
        for (regime_index, regime) in implementations[algorithm_index].regimes.iter().enumerate() {
            if regime_index > 0 { data.push(','); }
            let first_size = INPUT_SIZES.iter().position(|size| size.bytes >= regime.first)
                .expect("every regime starts at or below the largest tested size");
            write!(
                data,
                "{{\"from\":{first_size},\"name\":{},\"why\":{},\"mark\":\"{}\"}}",
                json_string(regime.name),
                json_string(regime.why),
                regime.mark.name(),
            )
                .unwrap();
        }
        data.push_str("],\"min\":[");
        for size_index in 0..INPUT_COUNT {
            if size_index > 0 { data.push(','); }
            write!(data, "{}", results[algorithm_index][size_index].minimum).unwrap();
        }
        data.push_str("],\"med\":[");
        for size_index in 0..INPUT_COUNT {
            if size_index > 0 { data.push(','); }
            write!(data, "{}", results[algorithm_index][size_index].median).unwrap();
        }
        data.push_str("],\"max\":[");
        for size_index in 0..INPUT_COUNT {
            if size_index > 0 { data.push(','); }
            write!(data, "{}", results[algorithm_index][size_index].maximum).unwrap();
        }
        data.push_str("]}");
    }
    data.push_str("],\"x\":[");
    for (index, x) in x_positions.iter().enumerate() {
        if index > 0 { data.push(','); }
        write!(data, "{x:.2}").unwrap();
    }
    data.push_str("],\"labelY\":[");
    for (index, y) in label_y_by_algorithm.iter().enumerate() {
        if index > 0 { data.push(','); }
        write!(data, "{y:.2}").unwrap();
    }
    data.push_str("],\"colors\":[");
    for (index, algorithm) in roster.algorithms.iter().enumerate() {
        if index > 0 { data.push(','); }
        write!(data, "\"{}\"", algorithm.color()).unwrap();
    }
    data.push_str("],\"sizes\":[");
    for (index, size) in INPUT_SIZES.iter().enumerate() {
        if index > 0 { data.push(','); }
        write!(data, "\"{}\"", size.label).unwrap();
    }
    write!(
        data,
        "],\"baseline\":{},\"sharedProv\":{shared_count},\"plotLeft\":{PLOT_LEFT},\"plotRight\":{PLOT_RIGHT},\"plotTop\":{PLOT_TOP},\"plotBottom\":{PLOT_BOTTOM},\"labelGap\":{SERIES_LABEL_GAP},\"labelAbove\":{VALUE_LABEL_ABOVE},\"labelBelow\":{VALUE_LABEL_BELOW},\"labelHeight\":{VALUE_LABEL_HEIGHT},\"spreadNoticeable\":{SPREAD_NOTICEABLE},\"spreadWide\":{SPREAD_WIDE},\"provTop\":{PROVENANCE_TOP},\"provLine\":{PROVENANCE_LINE_HEIGHT},\"takeaway\":\"{}\"}}",
        roster.baseline,
        generate_takeaway(roster, results).replace('\\', "\\\\").replace('"', "\\\""),
    )
        .unwrap();

    svg.push_str("  <script><![CDATA[\n");
    writeln!(svg, "const DATA = {data};").unwrap();
    svg.push_str(INTERACTION_SCRIPT);
    svg.push_str("  ]]></script>\n");
}

const INTERACTION_SCRIPT: &str = r##"
const on = DATA.series.map(() => true);
const NS = "http://www.w3.org/2000/svg";

function niceBelow(v) {
  const mag = Math.pow(10, Math.floor(Math.log10(v)));
  const n = v / mag;
  return (n >= 5 ? 5 : n >= 2 ? 2 : 1) * mag;
}
function niceAbove(v) {
  const mag = Math.pow(10, Math.floor(Math.log10(v)));
  const n = v / mag;
  return (n <= 1 ? 1 : n <= 2 ? 2 : n <= 5 ? 5 : 10) * mag;
}
function fmtTick(v) {
  return v >= 10 ? v.toFixed(0) : v >= 1 ? v.toFixed(1) : v.toFixed(2);
}
function fmt2(v) { return v.toFixed(2); }
function label(i) { return DATA.sizes[i]; }

function ticks(lo, hi) {
  const out = [];
  const eLo = Math.floor(Math.log10(lo)) - 1, eHi = Math.ceil(Math.log10(hi)) + 1;
  for (let e = eLo; e <= eHi; e++) {
    for (const m of [1, 1.5, 2, 3, 5, 7]) {
      const v = m * Math.pow(10, e);
      if (v >= lo * 0.999999 && v <= hi * 1.000001) out.push(v);
    }
  }
  return out;
}

/* Mirror of takeaway_clause() in main.rs, for the visible contenders. */
function clause(i) {
  const b = DATA.series[DATA.baseline], s = DATA.series[i];
  const ratios = b.med.map((v, k) => v / s.med[k]);
  const lowest = Math.min(...ratios), highest = Math.max(...ratios);
  const TIE = 0.05, n = s.name, bn = b.name;
  const tiedLow = lowest > 1 - TIE, tiedHigh = highest < 1 + TIE;
  const x = v => v.toFixed(2) + "\u00d7";
  if (tiedLow && tiedHigh) return `${n} matches ${bn} at every size`;
  if (lowest > 1 + TIE) return `${n} is ${x(lowest)} to ${x(highest)} faster than ${bn}`;
  if (highest < 1 - TIE) return `${bn} is ${x(1 / highest)} to ${x(1 / lowest)} faster than ${n}`;
  if (tiedLow) {
    const first = ratios.findIndex(r => r > 1 + TIE);
    return `${n} matches ${bn} below ${label(first)} and is up to ${x(highest)} faster from there`;
  }
  if (tiedHigh) {
    let last = -1; ratios.forEach((r, k) => { if (r < 1 - TIE) last = k; });
    return `${bn} is up to ${x(1 / lowest)} faster than ${n} through ${label(last)}, then they match`;
  }
  const first = ratios[0] > 1 ? n : bn, lastN = ratios[ratios.length - 1] > 1 ? n : bn;
  return `${first} is faster at ${label(0)}, ${lastN} at ${label(ratios.length - 1)}`;
}

function takeaway() {
  const visible = DATA.series.map((_, i) => i).filter(i => on[i]);
  if (visible.length === 0) return "Every contender is hidden; click a name at right to show one";
  if (!on[DATA.baseline]) {
    return visible.length === 1
      ? `Showing ${DATA.series[visible[0]].name} alone`
      : `Showing ${visible.map(i => DATA.series[i].name).join(", ")}; show ${DATA.series[DATA.baseline].name} for speed ratios`;
  }
  const others = visible.filter(i => i !== DATA.baseline);
  if (others.length === 0) return `Showing ${DATA.series[DATA.baseline].name} alone`;
  return "On this machine: " + others.map(clause).join("; ");
}

function relayout() {
  const visible = DATA.series.map((_, i) => i).filter(i => on[i]);
  let lo = Infinity, hi = 0;
  for (const i of visible) {
    lo = Math.min(lo, ...DATA.series[i].min);
    hi = Math.max(hi, ...DATA.series[i].max);
  }
  if (visible.length === 0) { lo = 0.1; hi = 1; }
  const axMin = niceBelow(lo * 0.92), axMax = niceAbove(hi * 1.08);
  const lMin = Math.log(axMin), lMax = Math.log(axMax);
  const mapY = v => DATA.plotBottom - (Math.log(v) - lMin) / (lMax - lMin) * (DATA.plotBottom - DATA.plotTop);
  currentMapY = mapY;

  /* Y axis: grid lines and tick labels. */
  const axis = document.getElementById("y-axis");
  while (axis.firstChild) axis.removeChild(axis.firstChild);
  for (const v of ticks(axMin, axMax)) {
    const y = mapY(v);
    const line = document.createElementNS(NS, "line");
    line.setAttribute("x1", DATA.plotLeft); line.setAttribute("x2", DATA.plotRight);
    line.setAttribute("y1", y.toFixed(2)); line.setAttribute("y2", y.toFixed(2));
    line.setAttribute("class", "grid");
    axis.appendChild(line);
    const t = document.createElementNS(NS, "text");
    t.setAttribute("x", (DATA.plotLeft - 10).toFixed(1)); t.setAttribute("y", (y + 3.5).toFixed(2));
    t.setAttribute("class", "tick-label"); t.setAttribute("text-anchor", "end");
    t.textContent = fmtTick(v);
    axis.appendChild(t);
  }

  /* Each series: band, median line, dots, value labels. */
  DATA.series.forEach((s, i) => {
    const g = document.getElementById("series-" + i);
    const dots = document.getElementById("dots-" + i);
    g.setAttribute("data-on", on[i] ? "true" : "false");
    dots.setAttribute("data-on", on[i] ? "true" : "false");
    if (!on[i]) return;
    const X = DATA.x;
    let band = "";
    X.forEach((x, k) => { band += (k ? " L " : "M ") + x + " " + mapY(s.max[k]).toFixed(2); });
    for (let k = X.length - 1; k >= 0; k--) band += " L " + X[k] + " " + mapY(s.min[k]).toFixed(2);
    g.querySelector(".band").setAttribute("d", band + " Z");
    let med = "";
    X.forEach((x, k) => { med += (k ? " L " : "M ") + x + " " + mapY(s.med[k]).toFixed(2); });
    g.querySelector(".median").setAttribute("d", med);
    dots.querySelectorAll(".dot").forEach(dot => {
      const k = +dot.getAttribute("data-size");
      dot.setAttribute("transform", `translate(${X[k]} ${mapY(s.med[k]).toFixed(2)})`);
    });
  });

  /* Value labels: above the dot unless that collides within the column. */
  for (let k = 0; k < DATA.x.length; k++) {
    const order = visible.slice().sort((a, b) => DATA.series[a].med[k] - DATA.series[b].med[k]);
    const taken = [];
    const clear = y => taken.every(t => Math.abs(t - y) >= DATA.labelHeight);
    for (const i of order) {
      const dotY = mapY(DATA.series[i].med[k]);
      let y = dotY + DATA.labelAbove;
      if (!clear(y)) { y = dotY + DATA.labelBelow; while (!clear(y)) y += DATA.labelHeight; }
      taken.push(y);
      document.getElementById("series-" + i).querySelectorAll(".value-label").forEach(t => {
        if (+t.getAttribute("data-size") === k) t.setAttribute("y", y.toFixed(2));
      });
    }
  }
  /*
   * Right-edge labels, every contender in its slot. Each anchors level with
   * its line's last point on the current axis; a hidden contender's anchor
   * is clamped to the plot edge, so its grey label points toward where its
   * data lies. Then push overlapping labels apart and keep the stack inside
   * the plot.
   */
  const last = DATA.x.length - 1;
  const clamp = y => Math.min(DATA.plotBottom - 8, Math.max(DATA.plotTop + 8, y));
  const slots = DATA.series
    .map((s, i) => [i, clamp(mapY(s.med[last]))])
    .sort((a, b) => a[1] - b[1]);
  for (let k = 1; k < slots.length; k++) {
    slots[k][1] = Math.max(slots[k][1], slots[k - 1][1] + DATA.labelGap);
  }
  const overrun = Math.max(0, slots[slots.length - 1][1] + 20 - DATA.plotBottom);
  for (const [i, y] of slots) {
    const lab = document.getElementById("series-" + i).querySelector(".series-label");
    lab.setAttribute("transform", `translate(0 ${(y - overrun).toFixed(2)})`);
  }

  /* Provenance: visible contenders' lines close ranks after the shared lines. */
  let slot = DATA.sharedProv;
  DATA.series.forEach((s, i) => {
    document.getElementById("series-" + i).querySelectorAll(".series-prov").forEach(t => {
      if (on[i]) t.setAttribute("y", (DATA.provTop + 40 + slot++ * DATA.provLine).toFixed(1));
    });
  });

  const head = document.getElementById("takeaway");
  while (head.firstChild) head.removeChild(head.firstChild);
  const lines = [];
  for (const clause of takeaway().split("; ")) {
    const last = lines[lines.length - 1];
    if (last !== undefined && last.length + 2 + clause.length <= 118) lines[lines.length - 1] = last + "; " + clause;
    else lines.push(clause);
  }
  lines.forEach((line, i) => {
    const span = document.createElementNS(NS, "tspan");
    span.setAttribute("x", DATA.plotLeft); span.setAttribute("dy", i ? 18 : 0);
    span.textContent = line;
    head.appendChild(span);
  });
}

function toggleSeries(i) {
  on[i] = !on[i];
  relayout();
  if (hovered) showHover(hovered[0], hovered[1]);
}

/* Current y mapping, kept by relayout() so the hover panel places itself. */
let currentMapY = null;

/* The dot under the pointer, so a toggle can rebuild the panel in place. */
let hovered = null;

function gbps(nsPerByte) {
  const t = 1 / nsPerByte;
  return (t >= 10 ? t.toFixed(0) : t.toFixed(1)) + " GB/s";
}

function markGlyph(mark, color) {
  const stroke = ["stroke", "#fdfdfc"], sw = ["stroke-width", "1.5"];
  let el;
  if (mark === "diamond") { el = document.createElementNS(NS, "path"); el.setAttribute("d", "M 0 -6.25 L 6.25 0 L 0 6.25 L -6.25 0 Z"); }
  else if (mark === "square") { el = document.createElementNS(NS, "rect"); el.setAttribute("x", -4.5); el.setAttribute("y", -4.5); el.setAttribute("width", 9); el.setAttribute("height", 9); }
  else { el = document.createElementNS(NS, "circle"); el.setAttribute("r", 5); }
  el.setAttribute("fill", color); el.setAttribute(...stroke); el.setAttribute(...sw);
  return el;
}

function wrapText(text, maxChars) {
  const lines = []; let line = "";
  for (const word of text.split(" ")) {
    if (line && (line + " " + word).length > maxChars) { lines.push(line); line = word; }
    else line = line ? line + " " + word : word;
  }
  if (line) lines.push(line);
  return lines;
}

function textEl(x, y, cls, content, extra) {
  const t = document.createElementNS(NS, "text");
  t.setAttribute("x", x); t.setAttribute("y", y); t.setAttribute("class", cls);
  if (extra) for (const k in extra) t.setAttribute(k, extra[k]);
  t.textContent = content;
  return t;
}

/*
 * Hovering a dot: the hovered contender's median and range at that size,
 * then every visible contender ranked fastest first, each with its speed
 * relative to the hovered one. Hidden contenders stay out of the ranking.
 */
function showHover(focus, k) {
  hovered = [focus, k];
  if (!on[focus] || !currentMapY) { document.getElementById("hover").style.display = "none"; return; }
  const body = document.getElementById("hover-body");
  while (body.firstChild) body.removeChild(body.firstChild);

  const f = DATA.series[focus];
  const rows = DATA.series.map((s, i) => [i, s.med[k]]).filter(([i]) => on[i]).sort((a, b) => a[1] - b[1]);

  const PAD = 10, LINE = 16, W = 350;
  let y = PAD + 12;
  body.appendChild(textEl(PAD, y, "hover-head", `${f.name} at ${DATA.sizes[k]}`));
  y += 14;
  const spread = (f.max[k] - f.min[k]) / f.med[k];
  const spreadNote = spread >= DATA.spreadWide ? " · wide spread, low precision"
    : spread >= DATA.spreadNoticeable ? " · noticeable spread" : "";
  const rangeRow = textEl(PAD, y, "hover-sub",
    `median ${f.med[k].toFixed(3)} ns/B (${gbps(f.med[k])}) · range ${f.min[k].toFixed(3)}–${f.max[k].toFixed(3)} (±${(spread * 50).toFixed(0)}%)${spreadNote}`);
  if (spread >= DATA.spreadWide) rangeRow.setAttribute("fill", "#b45309");
  body.appendChild(rangeRow);

  /* Code path at this size; the first size of a new path explains why. */
  let ri = 0;
  f.regimes.forEach((r, j) => { if (k >= r.from) ri = j; });
  const regime = f.regimes[ri];
  const isTransition = ri > 0 && regime.from === k;
  y += 14;
  const pathRow = textEl(PAD, y, "hover-sub", "");
  const shape = markGlyph(regime.mark, DATA.colors[focus]);
  shape.setAttribute("transform", `translate(${PAD + 5} ${y - 3.5}) scale(0.8)`);
  body.appendChild(shape);
  pathRow.setAttribute("x", PAD + 14);
  pathRow.textContent = (isTransition ? "new path from here: " : "code path: ") + regime.name;
  if (isTransition) pathRow.setAttribute("class", "hover-sub hover-path");
  body.appendChild(pathRow);
  if (isTransition) {
    for (const line of wrapText(regime.why, 62)) {
      y += 13;
      body.appendChild(textEl(PAD + 14, y, "hover-why", line));
    }
  }
  y += 10;

  if (rows.length > 1) {
    y += LINE;
    body.appendChild(textEl(PAD, y, "hover-sub", "contender"));
    body.appendChild(textEl(PAD + 150, y, "hover-sub", "ns/B", { "text-anchor": "end" }));
    body.appendChild(textEl(PAD + 215, y, "hover-sub", "GB/s", { "text-anchor": "end" }));
    body.appendChild(textEl(W - PAD, y, "hover-sub", `relative to ${f.name}`, { "text-anchor": "end" }));
    y += 4;
    for (const [i, med] of rows) {
      y += LINE;
      const s = DATA.series[i];
      /* Swatch: this contender's mark at this size, in its own colour. */
      let rj = 0;
      s.regimes.forEach((r, j) => { if (k >= r.from) rj = j; });
      const sw = markGlyph(s.regimes[rj].mark, DATA.colors[i]);
      sw.setAttribute("class", "hover-swatch");
      sw.setAttribute("style", `fill: ${DATA.colors[i]}`);
      sw.setAttribute("transform", `translate(${PAD + 5} ${y - 4}) scale(0.85)`);
      body.appendChild(sw);
      const cls = "hover-row" + (i === focus ? " hover-row-focus" : "");
      body.appendChild(textEl(PAD + 15, y, cls, s.name));
      body.appendChild(textEl(PAD + 150, y, cls, med.toFixed(3), { "text-anchor": "end" }));
      body.appendChild(textEl(PAD + 215, y, cls, gbps(med).replace(" GB/s", ""), { "text-anchor": "end" }));
      let rel, color;
      if (i === focus) { rel = "—"; color = "#9a9a9a"; }
      else {
        const r = f.med[k] / med;
        if (Math.abs(r - 1) < 0.05) { rel = "about the same"; color = "#777777"; }
        else if (r > 1) { rel = "\u25b2 " + r.toFixed(2) + "\u00d7 faster"; color = "#15803d"; }
        else { rel = "\u25bc " + (1 / r).toFixed(2) + "\u00d7 slower"; color = "#b91c1c"; }
      }
      body.appendChild(textEl(W - PAD, y, "hover-ratio", rel, { "text-anchor": "end", fill: color }));
    }
    y += 12;
    body.appendChild(textEl(PAD, y, "hover-note", `each row's speed compared with ${f.name}; medians, ranked fastest first`));
    y += 4;
  }
  const H = y + PAD - 6;

  /* Place beside the column, flipping left near the right edge. */
  const x = DATA.x[k];
  const dotY = currentMapY(f.med[k]);
  let bx = x + 14;
  if (bx + W > DATA.plotRight + 10) bx = x - 14 - W;
  let by = Math.min(Math.max(dotY - H / 2, DATA.plotTop - 30), DATA.plotBottom + 30 - H);

  const box = document.getElementById("hover-box");
  box.setAttribute("x", bx); box.setAttribute("y", by);
  box.setAttribute("width", W); box.setAttribute("height", H);
  body.setAttribute("transform", `translate(${bx} ${by})`);
  const guide = document.getElementById("hover-guide");
  guide.setAttribute("x1", x); guide.setAttribute("x2", x);
  document.getElementById("hover").style.display = "";
}

function hideHover() {
  hovered = null;
  document.getElementById("hover").style.display = "none";
}

window.toggleSeries = toggleSeries;
window.showHover = showHover;
window.hideHover = hideHover;
relayout();
"##;

/// "source URL · branch B · commit C" for a git-dependency provenance line.
fn short_git_source(description: &str) -> String {
    let mut fields = description.split("; ");
    let _name = fields.next();
    let rest: Vec<&str> = fields.collect();
    rest.iter()
        .map(|f| if let Some(c) = f.strip_prefix("commit ") { format!("commit {}", &c[..12.min(c.len())]) } else { f.to_string() })
        .collect::<Vec<_>>()
        .join(" · ")
}

fn package_name_and_version(source_info: &str) -> &str {
    source_info
        .split(';')
        .next()
        .expect("package source information must not be empty")
}

fn nice_log_bound_below(value: f64) -> f64 {
    assert!(value.is_finite() && value > 0.0);

    let magnitude = 10.0_f64.powf(value.log10().floor());
    let normalized = value / magnitude;

    let nice = if normalized >= 5.0 {
        5.0
    } else if normalized >= 2.0 {
        2.0
    } else {
        1.0
    };

    nice * magnitude
}

fn nice_log_bound_above(value: f64) -> f64 {
    assert!(value.is_finite() && value > 0.0);

    let magnitude = 10.0_f64.powf(value.log10().floor());
    let normalized = value / magnitude;

    let nice = if normalized <= 1.0 {
        1.0
    } else if normalized <= 2.0 {
        2.0
    } else if normalized <= 5.0 {
        5.0
    } else {
        10.0
    };

    nice * magnitude
}

fn log_ticks(axis_min: f64, axis_max: f64) -> Vec<f64> {
    let lowest_exponent = axis_min.log10().floor() as i32 - 1;
    let highest_exponent = axis_max.log10().ceil() as i32 + 1;

    let mut ticks = Vec::new();

    for exponent in lowest_exponent..=highest_exponent {
        for mantissa in [1.0, 1.5, 2.0, 3.0, 5.0, 7.0] {
            let value = mantissa * 10.0_f64.powi(exponent);

            /*
             * Tolerate one part in a million of floating-point error at
             * the axis bounds themselves.
             */
            if value >= axis_min * 0.999_999
                && value <= axis_max * 1.000_001
            {
                ticks.push(value);
            }
        }
    }

    assert!(
        ticks.len() >= 2,
        "a log axis must have at least two ticks"
    );

    ticks
}

fn format_tick(value: f64) -> String {
    assert!(
        value > 0.0,
        "log-axis ticks must be positive"
    );

    if value >= 10.0 {
        format!("{value:.0}")
    } else if value >= 1.0 {
        format!("{value:.1}")
    } else {
        format!("{value:.2}")
    }
}

fn format_result_value(value: f64) -> String {
    format!("{value:.2}")
}

fn xml_escape(input: &str) -> String {
    let mut escaped = String::with_capacity(input.len());

    for character in input.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            _ => escaped.push(character),
        }
    }

    escaped
}
