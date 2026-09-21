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
 * 128 rounds: a multiple of the sixteen input sizes and the four cyclic
 * orders, and enough that the running medians settle well before the end
 * (they are stable to three decimals by round 30 on every machine measured
 * so far). Each sample runs a contender for about 2 ms; timing noise on
 * this scale is well under 1%.
 */
const SAMPLE_ROUNDS: usize = 128;
const CALIBRATION_PROBE_NS: u128 = 1_000_000;
const TARGET_SAMPLE_NS: u128 = 2_000_000;

const INPUT_COUNT: usize = 16;
const ALGORITHM_COUNT: usize = 4;

/*
 * Index of the contender every ratio is taken against. BLAKE3 stays the
 * baseline so ratios read "how much faster (or slower) is X than BLAKE3".
 */
const BASELINE: usize = 0;

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

const ALGORITHMS: [Algorithm; ALGORITHM_COUNT] = [
    Algorithm::Blake3,
    Algorithm::Sha256,
    Algorithm::Sha1Dc,
    Algorithm::Blake3Sme2,
];

/*
 * The interleaving spreads one contender's lingering effects (cache state,
 * clock, thermal drift) evenly over the others. These four orders do that
 * with the same balance as all 24 permutations: every contender takes
 * every position exactly once, and every ordered pair "Y runs right after
 * X" occurs exactly once across the set. SAMPLE_ROUNDS is a multiple of
 * four, so each order runs equally often.
 */
const ALGORITHM_ORDERS: [[usize; ALGORITHM_COUNT]; 4] = [
    [0, 1, 2, 3],
    [1, 3, 0, 2],
    [2, 0, 3, 1],
    [3, 2, 1, 0],
];

type Results = [[Statistics; ALGORITHM_COUNT]; INPUT_COUNT];

#[derive(Clone, Copy)]
struct InputSize {
    label: &'static str,
    bytes: usize,
}

/*
 * A contender is one hash implementation under test. Adding one means a
 * variant here, a name, a color, a provenance string, and an arm in
 * run_batch; the harness handles interleaving and reporting for any count.
 */
#[derive(Clone, Copy, PartialEq, Eq)]
enum Algorithm {
    Blake3,
    Sha256,
    Sha1Dc,
    Blake3Sme2,
}

impl Algorithm {
    fn name(self) -> &'static str {
        match self {
            Self::Blake3 => "BLAKE3",
            Self::Sha256 => "SHA-256",
            Self::Sha1Dc => "SHA-1DC",
            Self::Blake3Sme2 => "BLAKE3 SME2",
        }
    }

    fn color(self) -> &'static str {
        match self {
            Self::Blake3 => "#3b82f6",
            Self::Sha256 => "#e07a45",
            Self::Sha1Dc => "#6b9e3a",
            Self::Blake3Sme2 => "#7c3aed",
        }
    }

    /// The Cargo.lock description of the crate that implements this contender.
    fn source(self) -> &'static str {
        match self {
            Self::Blake3 => BLAKE3_SOURCE_INFO,
            Self::Sha256 => SHA2_SOURCE_INFO,
            Self::Sha1Dc => SHA1_CHECKED_SOURCE_INFO,
            Self::Blake3Sme2 => BLAKE3_SME2_SOURCE_INFO,
        }
    }

    /// One line on how this contender runs, for the report header.
    fn mode(self) -> &'static str {
        match self {
            Self::Blake3 => "single-threaded; Rayon not enabled",
            Self::Sha256 => "assembly backends where available (ARMv8 SHA-256 instructions on AArch64)",
            Self::Sha1Dc => "SHA-1 with collision detection, pure Rust (the construction git uses)",
            Self::Blake3Sme2 => "single-threaded; SME2 kernel for groups of sixteen chunks, integer + NEON hybrid kernels below that; the benchmark stops on a CPU without SME2",
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

fn main() {
    assert_eq!(
        SAMPLE_ROUNDS % ALGORITHM_ORDERS.len(),
        0,
        "SAMPLE_ROUNDS must use every algorithm order equally"
    );

    assert_eq!(
        SAMPLE_ROUNDS % INPUT_SIZES.len(),
        0,
        "SAMPLE_ROUNDS must use every input-size position equally"
    );

    assert_orders_balanced();

    /*
     * The BLAKE3 SME2 column measures the fork with its SME2 kernel
     * selected. The fork falls back to its NEON backend on a CPU without
     * SME2, which is a fine library behaviour and a wrong benchmark
     * heading; the check belongs here, where the heading is.
     */
    let sme2_platform = blake3_sme2::platform::Platform::detect();
    assert_eq!(
        format!("{sme2_platform:?}"),
        "SME2",
        "the blake3_sme2 crate selected {sme2_platform:?} on this machine; \
         the BLAKE3 SME2 column needs a CPU with SME2 and 512-bit streaming \
         vectors (Apple M4 and later)",
    );

    let machine = machine_metadata();
    let results = measure_all();

    let text = generate_text(&results, &machine);
    let svg = generate_svg(&results, &machine);

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
 * ALGORITHM_ORDERS must place every contender in every position exactly
 * once and realise every ordered adjacency exactly once. This is what
 * lets four orders stand in for all permutations.
 */
fn assert_orders_balanced() {
    let mut positions = [[0usize; ALGORITHM_COUNT]; ALGORITHM_COUNT];
    let mut adjacencies = [[0usize; ALGORITHM_COUNT]; ALGORITHM_COUNT];

    for order in ALGORITHM_ORDERS {
        for (position, &algorithm) in order.iter().enumerate() {
            positions[algorithm][position] += 1;
            if position > 0 {
                adjacencies[order[position - 1]][algorithm] += 1;
            }
        }
    }

    for algorithm in 0..ALGORITHM_COUNT {
        for position in 0..ALGORITHM_COUNT {
            assert_eq!(
                positions[algorithm][position], 1,
                "contender {algorithm} must take position {position} exactly once across ALGORITHM_ORDERS"
            );
        }
        for follower in 0..ALGORITHM_COUNT {
            let expected = usize::from(follower != algorithm);
            assert_eq!(
                adjacencies[algorithm][follower], expected,
                "contender {follower} must run right after {algorithm} exactly {expected} time(s) across ALGORITHM_ORDERS"
            );
        }
    }
}

fn measure_all() -> Results {
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
    }

    /*
     * Each algorithm/input combination gets its own calibrated iteration
     * count so that timed blocks have approximately equal durations.
     */
    let mut progress = Progress::new();
    progress.phase("calibrating");

    let mut batch_iterations =
        [[1usize; ALGORITHM_COUNT]; INPUT_COUNT];

    for size_index in 0..INPUT_COUNT {
        for algorithm_index in 0..ALGORITHM_COUNT {
            batch_iterations[size_index][algorithm_index] =
                calibrate_batch(
                    ALGORITHMS[algorithm_index],
                    &inputs[size_index],
                );
        }
    }

    progress.phase("warming up");

    /*
     * Warm every algorithm in every ordering position, on every input size.
     * The input size that runs first is rotated as well.
     */
    for warmup_round in 0..ALGORITHM_ORDERS.len() {
        let order = ALGORITHM_ORDERS[warmup_round];

        for size_offset in 0..INPUT_COUNT {
            let size_index =
                (size_offset + warmup_round) % INPUT_COUNT;

            for algorithm_index in order {
                run_batch(
                    ALGORITHMS[algorithm_index],
                    &inputs[size_index],
                    batch_iterations[size_index][algorithm_index],
                );
            }
        }
    }

    let mut samples: [[Vec<f64>; ALGORITHM_COUNT]; INPUT_COUNT] =
        std::array::from_fn(|_| {
            std::array::from_fn(|_| Vec::with_capacity(SAMPLE_ROUNDS))
        });

    /*
     * The algorithm order cycles through all six permutations. Input-size
     * order rotates independently. This distributes ordering, thermal, and
     * system-load effects across the algorithms.
     */
    progress.phase("measuring");

    for round in 0..SAMPLE_ROUNDS {
        progress.round(round, &samples);

        let algorithm_order =
            ALGORITHM_ORDERS[round % ALGORITHM_ORDERS.len()];

        for size_offset in 0..INPUT_COUNT {
            let size_index =
                (size_offset + round) % INPUT_COUNT;

            let input = &inputs[size_index];

            for algorithm_index in algorithm_order {
                let algorithm = ALGORITHMS[algorithm_index];
                let iterations =
                    batch_iterations[size_index][algorithm_index];

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

                samples[size_index][algorithm_index]
                    .push(nanoseconds_per_byte);
            }
        }
    }

    progress.finish(&samples);

    let mut results =
        [[Statistics::ZERO; ALGORITHM_COUNT]; INPUT_COUNT];

    for size_index in 0..INPUT_COUNT {
        for algorithm_index in 0..ALGORITHM_COUNT {
            results[size_index][algorithm_index] =
                summarize(&mut samples[size_index][algorithm_index]);
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
struct Progress {
    started: Instant,
    measuring_started: Option<Instant>,
    interactive: bool,
    last_width: usize,
}

impl Progress {
    const BAR_WIDTH: usize = 30;

    fn new() -> Self {
        let interactive = std::io::stderr().is_terminal();
        Self {
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
    fn round(&mut self, round: usize, samples: &[[Vec<f64>; ALGORITHM_COUNT]; INPUT_COUNT]) {
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
            running_medians(samples, INPUT_COUNT - 1),
        ));
    }

    fn finish(&mut self, samples: &[[Vec<f64>; ALGORITHM_COUNT]; INPUT_COUNT]) {
        let bar = "█".repeat(Self::BAR_WIDTH);
        self.draw(&format!(
            "[{:>5.1}s] measured  {bar} {SAMPLE_ROUNDS}/{SAMPLE_ROUNDS} rounds · {}",
            self.started.elapsed().as_secs_f64(),
            running_medians(samples, INPUT_COUNT - 1),
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
fn running_medians(samples: &[[Vec<f64>; ALGORITHM_COUNT]; INPUT_COUNT], size_index: usize) -> String {
    if samples[size_index][0].is_empty() {
        return format!("medians at {} pending", INPUT_SIZES[size_index].label);
    }

    let parts: Vec<String> = (0..ALGORITHM_COUNT)
        .map(|algorithm_index| {
            let mut sorted = samples[size_index][algorithm_index].clone();
            sorted.sort_by(f64::total_cmp);
            format!("{} {:.3}", ALGORITHMS[algorithm_index].name(), median_of_sorted(&sorted))
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

#[derive(Clone, Copy)]
struct Blake3Implementation {
    platform: &'static str,
    one_chunk: &'static str,
    four_chunks: &'static str,
    bulk: &'static str,
}

/*
 * These backend inferences are based on BLAKE3 v1.8.7, particularly
 * src/platform.rs and the SIMD hash_many fallback chains.
 */
fn detect_blake3_implementation() -> Blake3Implementation {
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        if std::arch::is_x86_feature_detected!("avx512f")
            && std::arch::is_x86_feature_detected!("avx512vl")
        {
            return Blake3Implementation {
                platform: "AVX-512",
                one_chunk: "AVX-512 compression",
                four_chunks:
                "SSE4.1 hash_many (4-way SIMD fallback)",
                bulk: "AVX-512 hash_many (16-way SIMD)",
            };
        }

        if std::arch::is_x86_feature_detected!("avx2") {
            return Blake3Implementation {
                platform: "AVX2",
                one_chunk: "SSE4.1 compression",
                four_chunks:
                "SSE4.1 hash_many (4-way SIMD fallback)",
                bulk: "AVX2 hash_many (8-way SIMD)",
            };
        }

        if std::arch::is_x86_feature_detected!("sse4.1") {
            return Blake3Implementation {
                platform: "SSE4.1",
                one_chunk: "SSE4.1 compression",
                four_chunks: "SSE4.1 hash_many (4-way SIMD)",
                bulk: "SSE4.1 hash_many (4-way SIMD)",
            };
        }

        if std::arch::is_x86_feature_detected!("sse2") {
            return Blake3Implementation {
                platform: "SSE2",
                one_chunk: "SSE2 compression",
                four_chunks: "SSE2 hash_many (4-way SIMD)",
                bulk: "SSE2 hash_many (4-way SIMD)",
            };
        }

        return Blake3Implementation {
            platform: "portable",
            one_chunk: "portable compression",
            four_chunks: "portable hash_many",
            bulk: "portable hash_many",
        };
    }

    #[cfg(target_arch = "aarch64")]
    {
        return Blake3Implementation {
            platform: "NEON",
            one_chunk:
            "portable compression (one chunk; NEON bulk path not used)",
            four_chunks:
            "NEON hash_many (4-way SIMD; leftover chunks below four use portable compression, so 2 KiB and 3 KiB are all portable)",
            bulk: "NEON hash_many (4-way SIMD)",
        };
    }

    #[allow(unreachable_code)]
    Blake3Implementation {
        platform: "portable",
        one_chunk: "portable compression",
        four_chunks: "portable hash_many",
        bulk: "portable hash_many",
    }
}

fn blake3_backend_for_input(
    implementation: Blake3Implementation,
    input_bytes: usize,
) -> &'static str {
    if input_bytes <= 1024 {
        implementation.one_chunk
    } else if input_bytes < 16 * 1024 {
        implementation.four_chunks
    } else {
        implementation.bulk
    }
}

fn append_blake3_backend_report(output: &mut String) {
    let implementation = detect_blake3_implementation();

    writeln!(output, "BLAKE3 implementation selection:").unwrap();
    writeln!(
        output,
        "  selected platform: {}",
        implementation.platform,
    )
        .unwrap();

    for input_size in INPUT_SIZES {
        writeln!(
            output,
            "  {:>7}: {}",
            input_size.label,
            blake3_backend_for_input(
                implementation,
                input_size.bytes,
            ),
        )
            .unwrap();
    }

    writeln!(output).unwrap();
}

/*
 * The SME2 fork exposes its runtime platform choice directly, so this
 * report asks the crate rather than inferring from CPU features. main()
 * has already asserted that the platform is SME2, so this section
 * describes the fork's per-size behaviour with that selection.
 *
 * Backend inferences follow the fork's src/ffi_sme2.rs and
 * src/ffi_neon_hybrid.rs: a single chunk runs on the integer-only scalar
 * kernel (k1); two to fifteen whole chunks run on the integer + NEON
 * hybrid kernels (one or two chunks on the integer ALUs beside NEON pairs
 * or quads, xar from the SHA-3 extension); groups of sixteen whole chunks
 * go to the SME2 kernel, with any remainder on the hybrid kernels.
 */
fn append_blake3_sme2_backend_report(output: &mut String) {
    let platform = blake3_sme2::platform::Platform::detect();
    let degree = platform.simd_degree();

    writeln!(output, "BLAKE3 SME2 implementation selection:").unwrap();
    writeln!(
        output,
        "  selected platform: {platform:?} (512-bit streaming vectors, \
         sixteen-lane groups, hash_many degree {degree})",
    )
        .unwrap();

    let implementation = Blake3Implementation {
        platform: "SME2",
        one_chunk:
        "scalar kernel k1 (one chunk on the integer ALUs)",
        four_chunks:
        "integer + NEON hybrid kernels (fewer than sixteen chunks; SME2 group not filled)",
        bulk: "SME2 hash16_chunks kernel (16-way, 512-bit streaming vectors); remainder on hybrid kernels",
    };

    for input_size in INPUT_SIZES {
        writeln!(
            output,
            "  {:>7}: {}",
            input_size.label,
            blake3_backend_for_input(
                implementation,
                input_size.bytes,
            ),
        )
            .unwrap();
    }

    writeln!(output).unwrap();
}

fn generate_text(
    results: &Results,
    machine: &MachineMetadata,
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
    for algorithm in ALGORITHMS {
        writeln!(output, "{} source: {}", algorithm.name(), algorithm.source()).unwrap();
    }
    writeln!(
        output,
        "SHA-256 assembly source: {SHA2_ASM_SOURCE_INFO}"
    )
        .unwrap();
    for algorithm in ALGORITHMS {
        writeln!(output, "{} mode: {}", algorithm.name(), algorithm.mode()).unwrap();
    }
    writeln!(output).unwrap();

    append_blake3_backend_report(&mut output);
    append_blake3_sme2_backend_report(&mut output);

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

    /* Header row: one column per contender. */
    write!(output, "  {:<8}", "size").unwrap();
    for algorithm in ALGORITHMS {
        write!(output, "  {:>13}", algorithm.name()).unwrap();
    }
    writeln!(output).unwrap();

    for size_index in 0..INPUT_COUNT {
        write!(output, "  {:<8}", INPUT_SIZES[size_index].label).unwrap();
        for algorithm_index in 0..ALGORITHM_COUNT {
            write!(
                output,
                "  {:>13.3}",
                results[size_index][algorithm_index].median,
            )
                .unwrap();
        }
        writeln!(output).unwrap();

        write!(output, "  {:<8}", "").unwrap();
        for algorithm_index in 0..ALGORITHM_COUNT {
            let statistics = results[size_index][algorithm_index];
            write!(
                output,
                "  {:>13}",
                format!("{:.3}–{:.3}", statistics.minimum, statistics.maximum),
            )
                .unwrap();
        }
        writeln!(output).unwrap();
    }

    writeln!(output).unwrap();
    writeln!(output, "{}", generate_takeaway(results)).unwrap();
    writeln!(output).unwrap();

    output
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
fn median_ratios(results: &Results) -> [[f64; ALGORITHM_COUNT]; INPUT_COUNT] {
    std::array::from_fn(|size_index| {
        std::array::from_fn(|algorithm_index| {
            results[size_index][BASELINE].median
                / results[size_index][algorithm_index].median
        })
    })
}

fn generate_takeaway(results: &Results) -> String {
    let clauses: Vec<String> = (0..ALGORITHM_COUNT)
        .filter(|&algorithm_index| algorithm_index != BASELINE)
        .map(|algorithm_index| takeaway_clause(results, algorithm_index))
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
fn takeaway_clause(results: &Results, algorithm_index: usize) -> String {
    assert_ne!(algorithm_index, BASELINE, "the baseline has no clause of its own");

    let ratios = median_ratios(results);
    let baseline = ALGORITHMS[BASELINE].name();

    {
        let name = ALGORITHMS[algorithm_index].name();
        let column: Vec<f64> =
            ratios.iter().map(|row| row[algorithm_index]).collect();
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
const SVG_HEIGHT: f64 = 755.0;
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
    results: &Results,
    machine: &MachineMetadata,
) -> String {
    assert!(
        ALGORITHM_COUNT >= 2,
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

    let implementation = detect_blake3_implementation();
    let takeaway = generate_takeaway(results);

    let mut svg = String::new();

    writeln!(svg, r##"<?xml version="1.0" encoding="UTF-8"?>"##)
        .unwrap();

    writeln!(
        svg,
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {SVG_WIDTH:.0} {SVG_HEIGHT:.0}" width="{SVG_WIDTH:.0}" height="{SVG_HEIGHT:.0}">"##
    )
        .unwrap();

    writeln!(
        svg,
        r##"  <rect width="{SVG_WIDTH:.0}" height="{SVG_HEIGHT:.0}" fill="#fdfdfc"/>"##
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
    .marks { transition: opacity 0.3s ease; }
    .series[data-on="false"] .marks { opacity: 0; pointer-events: none; }
    .series[data-on="false"] .series-name { fill: #9a9a9a; }
    .series[data-on="false"] .series-detail { display: none; }
    .series[data-on="false"] .series-hint { display: inline; }
    .series[data-on="false"] .series-prov { display: none; }
    .series[data-on="false"] .series-swatch { fill: #fdfdfc; }
    .series-swatch { stroke-width: 2; transition: fill 0.3s ease; }
    .dot { cursor: crosshair; }
    #hover { pointer-events: none; }
    #hover-guide { stroke: #9a9a9a; stroke-width: 1; stroke-dasharray: 3,3; }
    #hover-box { fill: #ffffff; fill-opacity: 0.97; stroke: #c8c8c4; stroke-width: 1; }
    .hover-head { font-size: 12px; font-weight: 700; fill: #1a1a1a; }
    .hover-sub { font-size: 10px; fill: #777777; }
    .hover-row { font-size: 11px; fill: #333333; }
    .hover-row-focus { font-weight: 700; }
    .hover-ratio { font-size: 11px; font-weight: 600; }
    .hover-note { font-size: 9px; font-style: italic; fill: #9a9a9a; }
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
        r##"  <text x="{PLOT_LEFT:.0}" y="108" class="method">Line and dot: median · shaded band: minimum–maximum across {SAMPLE_ROUNDS} interleaved samples · single-threaded · lower is better</text>"##
    )
        .unwrap();
    writeln!(
        svg,
        r##"  <text x="{PLOT_LEFT:.0}" y="123" class="method">Hover a dot to compare contenders at that size · click a name at right to hide or show that contender</text>"##
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

    /* Vertical guides and x-axis labels at each tested size. */
    for size_index in 0..INPUT_COUNT {
        let x = x_positions[size_index];

        writeln!(
            svg,
            r##"  <line x1="{x:.2}" y1="{PLOT_TOP:.1}" x2="{x:.2}" y2="{PLOT_BOTTOM:.1}" class="grid-x"/>"##
        )
            .unwrap();

        writeln!(
            svg,
            r##"  <text x="{x:.2}" y="{:.1}" class="size-label" text-anchor="middle">{}</text>"##,
            PLOT_BOTTOM + 24.0,
            xml_escape(INPUT_SIZES[size_index].label),
        )
            .unwrap();
    }

    writeln!(
        svg,
        r##"  <text x="{:.1}" y="{:.1}" class="axis-title" text-anchor="middle">Input size (logarithmic spacing)</text>"##,
        (PLOT_LEFT + PLOT_RIGHT) / 2.0,
        PLOT_BOTTOM + 46.0,
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
    let mut label_slots: Vec<(usize, f64)> = (0..ALGORITHM_COUNT)
        .map(|algorithm_index| {
            (
                algorithm_index,
                map_y(results[INPUT_COUNT - 1][algorithm_index].median),
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

    let mut label_y_by_algorithm = [0.0_f64; ALGORITHM_COUNT];
    for (algorithm_index, label_y) in &label_slots {
        label_y_by_algorithm[*algorithm_index] = *label_y;
    }

    /*
     * One group per contender holds everything that belongs to it: band,
     * line, dots, value labels, the clickable label at right, and its
     * provenance line. Toggling flips one attribute on the group.
     */
    let provenance_shared = shared_provenance_lines(machine);
    let shared_count = provenance_shared.len();
    let mut provenance_slot = shared_count;

    for algorithm_index in 0..ALGORITHM_COUNT {
        let algorithm = ALGORITHMS[algorithm_index];
        let color = algorithm.color();

        writeln!(
            svg,
            r##"  <g class="series" id="series-{algorithm_index}" data-on="true">"##
        )
            .unwrap();

        writeln!(svg, r##"    <g class="marks">"##).unwrap();

        let mut band = String::new();
        for size_index in 0..INPUT_COUNT {
            let x = x_positions[size_index];
            let y = map_y(results[size_index][algorithm_index].maximum);
            if size_index == 0 {
                write!(band, "M {x:.2} {y:.2}").unwrap();
            } else {
                write!(band, " L {x:.2} {y:.2}").unwrap();
            }
        }
        for size_index in (0..INPUT_COUNT).rev() {
            let x = x_positions[size_index];
            let y = map_y(results[size_index][algorithm_index].minimum);
            write!(band, " L {x:.2} {y:.2}").unwrap();
        }
        band.push_str(" Z");

        writeln!(
            svg,
            r##"      <path class="band" d="{band}" fill="{color}" fill-opacity="0.16" stroke="none"/>"##
        )
            .unwrap();

        let mut path = String::new();
        for size_index in 0..INPUT_COUNT {
            let x = x_positions[size_index];
            let y = map_y(results[size_index][algorithm_index].median);
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

        /*
         * Stagger value labels per algorithm so nearly-coincident series
         * never collide: baseline above its dot, the others below at
         * increasing offsets.
         */
        let label_offset = match algorithm_index {
            BASELINE => -11.0,
            1 => 17.0,
            2 => -11.0,
            _ => 27.0,
        };

        for size_index in 0..INPUT_COUNT {
            let x = x_positions[size_index];
            let statistics = results[size_index][algorithm_index];
            let median_y = map_y(statistics.median);

            writeln!(
                svg,
                r##"      <circle class="dot" data-size="{size_index}" cx="{x:.2}" cy="{median_y:.2}" r="5" fill="{color}" stroke="#fdfdfc" stroke-width="1.5" onmouseenter="showHover({algorithm_index},{size_index})" onmouseleave="hideHover()">"##
            )
                .unwrap();

            /*
             * A native <title> would pop over the hover panel, so the panel
             * alone carries the numbers. The value labels below stay for
             * viewers without script.
             */
            svg.push_str("      </circle>\n");

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
                r##"      <text class="value-label" data-size="{size_index}" data-offset="{label_offset:.1}" x="{label_x:.2}" y="{:.2}" fill="{color}" text-anchor="{anchor}">{}</text>"##,
                median_y + label_offset,
                format_result_value(statistics.median),
            )
                .unwrap();
        }

        writeln!(svg, "    </g>").unwrap();

        /* Clickable label at right: swatch, name, detail, hint. */
        let statistics = results[INPUT_COUNT - 1][algorithm_index];
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

    /*
     * Annotate the BLAKE3 single-chunk elbow: the 64 B point uses a
     * different code path than the bulk sizes, and that is the whole story
     * of its shape. It follows the BLAKE3 dot, so it lives in a group the
     * script moves and hides with that series.
     */
    {
        let blake3_index = 0;
        let x = x_positions[0];
        let y = map_y(results[0][blake3_index].median);

        let short_backend = implementation
            .one_chunk
            .split(" (")
            .next()
            .expect("backend description is not empty");

        writeln!(
            svg,
            r##"  <g id="elbow" transform="translate({x:.2} {y:.2})">"##
        )
            .unwrap();

        /*
         * Above and to the right of the dot. The other BLAKE3 flavour and
         * SHA-256 sit at or below this dot at 64 B, so the space above it is
         * the clear side; the leader starts past the 64 B value labels.
         */
        writeln!(
            svg,
            r##"    <line x1="10" y1="-8" x2="70" y2="-40" stroke="#bbbbbb" stroke-width="1"/>"##
        )
            .unwrap();

        writeln!(
            svg,
            r##"    <text x="74" y="-44" class="annotation">BLAKE3 at 64 B: one chunk → {}</text>"##,
            xml_escape(short_backend),
        )
            .unwrap();

        writeln!(
            svg,
            r##"    <text x="74" y="-32" class="annotation">(bulk sizes use {})</text>"##,
            xml_escape(
                implementation
                    .bulk
                    .split(" (")
                    .next()
                    .expect("backend description is not empty"),
            ),
        )
            .unwrap();

        writeln!(svg, "  </g>").unwrap();
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

    let last_line_y = provenance_line_y(provenance_slot - 1);
    assert!(
        last_line_y + PROVENANCE_LINE_HEIGHT <= SVG_HEIGHT,
        "provenance must fit inside the canvas: last line at {last_line_y}, height {SVG_HEIGHT}"
    );

    /* Data and behaviour for the interactive toggles. */
    write_interaction_script(&mut svg, results, &x_positions, &label_y_by_algorithm, shared_count);

    svg.push_str("</svg>\n");
    svg
}

fn provenance_line_y(slot: usize) -> f64 {
    PROVENANCE_TOP + 40.0 + slot as f64 * PROVENANCE_LINE_HEIGHT
}

/* Provenance that describes the run as a whole. */
fn shared_provenance_lines(machine: &MachineMetadata) -> Vec<String> {
    vec![
        format!(
            "Run: {} · bench-hashes {BENCH_VERSION}",
            machine.timestamp,
        ),
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
    implementation: Blake3Implementation,
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
    results: &Results,
    x_positions: &[f64; INPUT_COUNT],
    label_y_by_algorithm: &[f64; ALGORITHM_COUNT],
    shared_count: usize,
) {
    let mut data = String::from("{\"series\":[");
    for algorithm_index in 0..ALGORITHM_COUNT {
        if algorithm_index > 0 {
            data.push(',');
        }
        write!(data, "{{\"name\":\"{}\",\"min\":[", ALGORITHMS[algorithm_index].name()).unwrap();
        for size_index in 0..INPUT_COUNT {
            if size_index > 0 { data.push(','); }
            write!(data, "{}", results[size_index][algorithm_index].minimum).unwrap();
        }
        data.push_str("],\"med\":[");
        for size_index in 0..INPUT_COUNT {
            if size_index > 0 { data.push(','); }
            write!(data, "{}", results[size_index][algorithm_index].median).unwrap();
        }
        data.push_str("],\"max\":[");
        for size_index in 0..INPUT_COUNT {
            if size_index > 0 { data.push(','); }
            write!(data, "{}", results[size_index][algorithm_index].maximum).unwrap();
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
    for (index, algorithm) in ALGORITHMS.iter().enumerate() {
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
        "],\"baseline\":{BASELINE},\"sharedProv\":{shared_count},\"plotLeft\":{PLOT_LEFT},\"plotRight\":{PLOT_RIGHT},\"plotTop\":{PLOT_TOP},\"plotBottom\":{PLOT_BOTTOM},\"labelGap\":{SERIES_LABEL_GAP},\"provTop\":{PROVENANCE_TOP},\"provLine\":{PROVENANCE_LINE_HEIGHT},\"takeaway\":\"{}\"}}",
        generate_takeaway(results).replace('\\', "\\\\").replace('"', "\\\""),
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
    g.setAttribute("data-on", on[i] ? "true" : "false");
    if (!on[i]) return;
    const X = DATA.x;
    let band = "";
    X.forEach((x, k) => { band += (k ? " L " : "M ") + x + " " + mapY(s.max[k]).toFixed(2); });
    for (let k = X.length - 1; k >= 0; k--) band += " L " + X[k] + " " + mapY(s.min[k]).toFixed(2);
    g.querySelector(".band").setAttribute("d", band + " Z");
    let med = "";
    X.forEach((x, k) => { med += (k ? " L " : "M ") + x + " " + mapY(s.med[k]).toFixed(2); });
    g.querySelector(".median").setAttribute("d", med);
    g.querySelectorAll(".dot").forEach(dot => {
      const k = +dot.getAttribute("data-size");
      dot.setAttribute("cy", mapY(s.med[k]).toFixed(2));
    });
    g.querySelectorAll(".value-label").forEach(t => {
      const k = +t.getAttribute("data-size");
      t.setAttribute("y", (mapY(s.med[k]) + +t.getAttribute("data-offset")).toFixed(2));
    });
  });

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

  /* Elbow annotation follows the baseline's first dot. */
  const elbow = document.getElementById("elbow");
  const b = DATA.series[DATA.baseline];
  elbow.setAttribute("transform", `translate(${DATA.x[0]} ${mapY(b.med[0]).toFixed(2)})`);
  elbow.style.display = on[DATA.baseline] ? "" : "none";

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
  hideHover();
  relayout();
}

/* Current y mapping, kept by relayout() so the hover panel places itself. */
let currentMapY = null;

function gbps(nsPerByte) {
  const t = 1 / nsPerByte;
  return (t >= 10 ? t.toFixed(0) : t.toFixed(1)) + " GB/s";
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
  if (!on[focus] || !currentMapY) return;
  const body = document.getElementById("hover-body");
  while (body.firstChild) body.removeChild(body.firstChild);

  const f = DATA.series[focus];
  const rows = DATA.series.map((s, i) => [i, s.med[k]]).filter(([i]) => on[i]).sort((a, b) => a[1] - b[1]);

  const PAD = 10, LINE = 16, W = 330;
  let y = PAD + 12;
  body.appendChild(textEl(PAD, y, "hover-head", `${f.name} at ${DATA.sizes[k]}`));
  y += 14;
  body.appendChild(textEl(PAD, y, "hover-sub",
    `median ${f.med[k].toFixed(3)} ns/B (${gbps(f.med[k])}) · range ${f.min[k].toFixed(3)}–${f.max[k].toFixed(3)}`));
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
      const sw = document.createElementNS(NS, "circle");
      sw.setAttribute("cx", PAD + 5); sw.setAttribute("cy", y - 4); sw.setAttribute("r", 4.5);
      sw.setAttribute("fill", DATA.colors[i]);
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
        else if (r > 1) { rel = r.toFixed(2) + "\u00d7 faster"; color = "#2f7d32"; }
        else { rel = (1 / r).toFixed(2) + "\u00d7 slower"; color = "#b3261e"; }
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
