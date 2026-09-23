mod test_vectors;

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
 * Every (contender, size) cell collects about SAMPLE_ROUNDS_TARGET samples
 * of about TARGET_SAMPLE_NS each; Roster::rounds() settles the exact count
 * as the smallest multiple of both the size count and the order count at
 * or above the target. The two knobs trade off differently:
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
 * Rounds are a multiple of the size count and of the order count.
 */
const SAMPLE_ROUNDS_TARGET: usize = 80;
/// --thorough multiplies the rounds; the median's interval narrows as 1/√n.
const THOROUGH_MULTIPLIER: usize = 3;
const CALIBRATION_PROBE_NS: u128 = 250_000;
/// Measured: 0.5 ms samples ran a full --all in 62 s instead of 80 s but
/// read 1.6% slower across the board (each sample's fixed cost, the duo
/// release and the clock reads, weighs twice as much), so 1 ms stays.
const TARGET_SAMPLE_NS: u128 = 1_000_000;

/*
 * Cells with a long hash get a time budget. A cell whose single hash
 * takes LONG_HASH_NS or more (every sample is then one hash, tens of
 * milliseconds for the plateau sizes) is sampled in every LONG_EVERY-th
 * round, at an offset of its own so its samples still span the run, and
 * in every LONG_EVERY_UNSURE-th round while the 95% interval of its median
 * is wider than LONG_PRECISION_PERMILLE of the median (or it has fewer
 * than LONG_MIN_SAMPLES). Steady cells take a quarter of the samples,
 * noisy ones half: past that, two runs of the same code differ by more
 * than further samples would narrow. Measured on the VM: a full --all
 * run from 150 s to 80 s; its medians against two full-sample runs at
 * x0.9955 and x1.0072, inside the x0.9886 those two runs differ by.
 */
const LONG_HASH_NS: u128 = 4_000_000;
const LONG_EVERY: usize = 4;
const LONG_PRECISION_PERMILLE: u64 = 20;
const LONG_EVERY_UNSURE: usize = 2;
const LONG_MIN_SAMPLES: usize = 8;

/// Points on the one-message axis, and on the many-messages axis.
const INPUT_COUNT: usize = 24;
const BATCH_COUNT: usize = 24;
/// Every measured (contender, x) cell lies on one of the two axes.
const POINT_COUNT: usize = INPUT_COUNT + BATCH_COUNT;
/// Every message in the many-messages use case is one BLAKE3 block, the
/// one size ab-blake3's batch entry point accepts.
const MESSAGE_LEN: usize = 64;

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
const RING_SOURCE_INFO: &str = env!("RING_SOURCE_INFO");
const SHA1_CHECKED_SOURCE_INFO: &str = env!("SHA1_CHECKED_SOURCE_INFO");
const BLAKE3_SERVIL_SOURCE_INFO: &str = env!("BLAKE3_SERVIL_SOURCE_INFO");
const AB_BLAKE3_SOURCE_INFO: &str = env!("AB_BLAKE3_SOURCE_INFO");

/*
 * Every power of two from 64 B to 128 MiB, plus 3 KiB and 3 MiB. Between
 * 64 B and 1 KiB BLAKE3 is inside one chunk; from 2 KiB to 16 KiB its SIMD
 * paths fill up (4-way NEON at 4 KiB, a sixteen-lane SME2 group at 16
 * KiB); above that the bulk rate settles. 3 KiB is where the fork's integer
 * + NEON hybrid kernels first overtake hardware SHA-256: one chunk on the
 * integer ALUs beside a NEON pair costs the same as the pair alone. SHA-1DC
 * and SHA-256 are block-serial and have only the per-message overhead to
 * show.
 *
 * The sizes past 1 MiB are there to show the plateau: a contender whose
 * 32, 64, and 128 MiB medians agree has levelled out. They matter most
 * for the multithreaded contenders, whose per-call overhead (a pool
 * hand-off, a subtree merge) takes longest to amortise: at 8 MiB the
 * fork's multithreaded rate was still climbing. Everything from 8 MiB up
 * is past the last-level cache on every machine this benchmark targets,
 * so the plateau is the memory-resident one. 3 MiB is to the plateau what
 * 3 KiB is to the SIMD ramp: a tree that is no power of two, whose left
 * subtree is 2 MiB and right 1 MiB, so a splitter that cuts at subtree
 * boundaries hands its threads unequal work there. Twenty-four points on
 * each axis keep the round count small: rounds are a common multiple of
 * the point count and the order count, and forty-eight shares factors
 * with every even order count from two to eight.
 *
 * The many-messages axis counts 64-byte messages per batch, from one to
 * 262144 (16 MiB of input). Powers of two from 1 to 16 show a SIMD batch
 * filling up (the blake3 crate's hash_many takes four blocks at a time on
 * NEON, sixteen with AVX-512); 3, 6, 12, 24, and 48 leave a group
 * partly filled or leave a remainder past the sixteen-message groups
 * ab-blake3 forms; from 64 up the per-batch overhead amortises, and the
 * batches past 16384 show the multithreaded batch calls levelling out.
 */
const POINTS: [Point; POINT_COUNT] = [
    Point::one("64 B", 64),
    Point::one("128 B", 128),
    Point::one("256 B", 256),
    Point::one("512 B", 512),
    Point::one("1 KiB", 1024),
    Point::one("2 KiB", 2 * 1024),
    Point::one("3 KiB", 3 * 1024),
    Point::one("4 KiB", 4 * 1024),
    Point::one("8 KiB", 8 * 1024),
    Point::one("16 KiB", 16 * 1024),
    Point::one("32 KiB", 32 * 1024),
    Point::one("64 KiB", 64 * 1024),
    Point::one("128 KiB", 128 * 1024),
    Point::one("256 KiB", 256 * 1024),
    Point::one("512 KiB", 512 * 1024),
    Point::one("1 MiB", 1024 * 1024),
    Point::one("2 MiB", 2 * 1024 * 1024),
    Point::one("3 MiB", 3 * 1024 * 1024),
    Point::one("4 MiB", 4 * 1024 * 1024),
    Point::one("8 MiB", 8 * 1024 * 1024),
    Point::one("16 MiB", 16 * 1024 * 1024),
    Point::one("32 MiB", 32 * 1024 * 1024),
    Point::one("64 MiB", 64 * 1024 * 1024),
    Point::one("128 MiB", 128 * 1024 * 1024),
    Point::many("1", 1),
    Point::many("2", 2),
    Point::many("3", 3),
    Point::many("4", 4),
    Point::many("6", 6),
    Point::many("8", 8),
    Point::many("12", 12),
    Point::many("16", 16),
    Point::many("24", 24),
    Point::many("32", 32),
    Point::many("48", 48),
    Point::many("64", 64),
    Point::many("128", 128),
    Point::many("256", 256),
    Point::many("512", 512),
    Point::many("1024", 1024),
    Point::many("2048", 2048),
    Point::many("4096", 4096),
    Point::many("8192", 8192),
    Point::many("16384", 16384),
    Point::many("32768", 32768),
    Point::many("65536", 65536),
    Point::many("131072", 131072),
    Point::many("262144", 262144),
];

/// results[contender_index][point_index], contenders in the roster's
/// order; None where the contender takes no part in the point's use case.
type Results = Vec<Vec<Option<Cell>>>;
/// Solo samples; empty vectors without --solo.
type Samples = Vec<Vec<Vec<Sample>>>;
/// Duo samples, in the same shape, one per round.
type DuoSamples = Vec<Vec<Vec<u64>>>;

/*
 * The two use cases. One message: a call hashes one input of the size,
 * as every contender's plain entry point does. Many messages: a call
 * hashes a batch of 64-byte messages; every contender loops its plain
 * entry point over the batch, and ab-blake3 hands the whole batch to
 * single_block_hash_many_exact. The multithreaded contenders sit this one
 * out: a pool is no answer to a 64-byte message.
 */
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum UseCase {
    OneMessage,
    ManyMessages,
}

impl UseCase {
    const ALL: [UseCase; 2] = [UseCase::OneMessage, UseCase::ManyMessages];

    /// The contiguous run of POINTS on this use case's axis.
    fn points(self) -> std::ops::Range<usize> {
        let start = POINTS.iter().position(|point| point.use_case == self).expect("each use case has points");
        let end = POINTS.iter().rposition(|point| point.use_case == self).unwrap() + 1;
        assert!(POINTS[start..end].iter().all(|point| point.use_case == self), "a use case's points are contiguous");
        start..end
    }

    /// What the x axis counts.
    fn x_axis(self) -> &'static str {
        match self {
            Self::OneMessage => "Input size (logarithmic spacing)",
            Self::ManyMessages => "Messages per batch, 64 B each (logarithmic spacing)",
        }
    }

    fn heading(self) -> &'static str {
        match self {
            Self::OneMessage => "One message per call",
            Self::ManyMessages => "Many 64-byte messages per call",
        }
    }

    /// The x column's header in the text report.
    fn column(self) -> &'static str {
        match self {
            Self::OneMessage => "size",
            Self::ManyMessages => "messages",
        }
    }

    /*
     * What a sample is divided by, and the units that follow. One message:
     * bytes, so time is ns/B and rate GB/s. Many messages: messages, so
     * time is ns per message and rate million messages per second. In
     * both, rate = rate_scale / time.
     */
    fn units(self, point: Point, iterations: usize) -> u64 {
        match self {
            Self::OneMessage => point.bytes as u64 * iterations as u64,
            Self::ManyMessages => point.messages as u64 * iterations as u64,
        }
    }

    fn time_unit(self) -> &'static str {
        match self {
            Self::OneMessage => "ns/B",
            Self::ManyMessages => "ns/msg",
        }
    }

    fn rate_unit(self) -> &'static str {
        match self {
            Self::OneMessage => "GB/s",
            Self::ManyMessages => "Mmsg/s",
        }
    }

    fn rate_unit_long(self) -> &'static str {
        match self {
            Self::OneMessage => "Gigabytes per second",
            Self::ManyMessages => "Million messages per second",
        }
    }

    /// rate = rate_scale / (ns per unit): 1 ns/B is 1 GB/s; 1 ns/msg is 1000 Mmsg/s.
    fn rate_scale(self) -> u64 {
        match self {
            Self::OneMessage => 1,
            Self::ManyMessages => 1000,
        }
    }
}

/// One x-axis point: an input size on the one-message axis, or a batch of
/// `messages` 64-byte messages (`bytes` in all) on the many-messages axis.
#[derive(Clone, Copy)]
struct Point {
    label: &'static str,
    bytes: usize,
    messages: usize,
    use_case: UseCase,
}

impl Point {
    const fn one(label: &'static str, bytes: usize) -> Self {
        Self { label, bytes, messages: 1, use_case: UseCase::OneMessage }
    }

    const fn many(label: &'static str, messages: usize) -> Self {
        Self { label, bytes: messages * MESSAGE_LEN, messages, use_case: UseCase::ManyMessages }
    }
}

/*
 * A contender is one hash implementation under test: which crate (the
 * crates.io blake3, the servil fork, sha2, ...) in which mode
 * (single-threaded or multithreaded). Which kernel that implementation
 * runs at each input size is chosen at run time and reported by
 * detect_kernels. Adding a contender means a variant here, an entry in
 * ALL, a key, a name, a color, a provenance string, a kernel description,
 * and an arm in run_batch; the harness handles selection, interleaving,
 * and reporting for any count.
 */
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Algorithm {
    Blake3,
    Sha256,
    Sha1Dc,
    /// The servil fork's single-threaded hash; its kernels are chosen at
    /// run time (SME2 where the CPU has it, integer + NEON hybrids elsewhere).
    Blake3Servil,
    /// Apple's CommonCrypto SHA-256 through CC_SHA256_Init/Update/Final.
    Sha256CommonCrypto,
    /// ring's SHA-256: BoringSSL's assembly, with runtime CPU detection.
    Sha256Ring,
    /// crates.io blake3 through Hasher::update_rayon, the crate's own
    /// multithreading, on Rayon's global pool as Rayon sizes it.
    Blake3Rayon,
    /// The fork's hash_multithreaded: the caller's thread plus the fork's
    /// own resident workers, shared fairly between concurrent callers in
    /// one process.
    Blake3ServilMt,
    /// The fork's hash_multithreaded_with_budget with a cap of one thread:
    /// a sanity check that the capped call is the single-threaded call, so
    /// its column should lie on BLAKE3 servil's. Runs only when named.
    Blake3ServilMt1,
    /// The ab-blake3 crate: const_hash for one message (a const fn copy of
    /// the reference tree), and single_block_hash_many_exact for a batch of
    /// 64-byte messages.
    AbBlake3,
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
    const ALL: [Algorithm; 10] = [
        Algorithm::Blake3,
        Algorithm::Sha256,
        Algorithm::Sha1Dc,
        Algorithm::Blake3Servil,
        Algorithm::Sha256CommonCrypto,
        Algorithm::Sha256Ring,
        Algorithm::Blake3Rayon,
        Algorithm::Blake3ServilMt,
        Algorithm::Blake3ServilMt1,
        Algorithm::AbBlake3,
    ];

    /// Command-line key, as in `--contenders blake3,sha256-cc`.
    fn key(self) -> &'static str {
        match self {
            Self::Blake3 => "blake3",
            Self::Sha256 => "sha256",
            Self::Sha1Dc => "sha1dc",
            Self::Blake3Servil => "blake3-servil",
            Self::Sha256CommonCrypto => "sha256-cc",
            Self::Sha256Ring => "sha256-ring",
            Self::Blake3Rayon => "blake3-mt",
            Self::Blake3ServilMt => "blake3-servil-mt",
            Self::Blake3ServilMt1 => "blake3-servil-mt1",
            Self::AbBlake3 => "ab-blake3",
        }
    }

    fn family(self) -> Family {
        match self {
            Self::Blake3 | Self::Blake3Servil | Self::Blake3Rayon | Self::Blake3ServilMt | Self::Blake3ServilMt1 | Self::AbBlake3 => Family::Blake3,
            Self::Sha256 | Self::Sha256CommonCrypto | Self::Sha256Ring => Family::Sha256,
            Self::Sha1Dc => Family::Sha1Dc,
        }
    }

    /// Whether this contender may use more than the calling thread.
    fn multithreaded(self) -> bool {
        matches!(self, Self::Blake3Rayon | Self::Blake3ServilMt)
    }

    /// Whether this contender is measured in a use case. BLAKE3 mt stays
    /// out of the many-messages use case: `update_rayon` exists for large
    /// inputs, and a 64-byte message is a call to it that no program would
    /// make. The fork's multithreaded contenders take part through its
    /// batch entry point, `hash_many_multithreaded`.
    fn takes_part(self, use_case: UseCase) -> bool {
        match use_case {
            UseCase::OneMessage => true,
            UseCase::ManyMessages => !matches!(self, Self::Blake3Rayon),
        }
    }

    /*
     * Contenders that run only when named on the command line. They are
     * kept for direct comparison; measured on the machines this benchmark
     * targets, another member of the same family beats them at every size,
     * so a default or --all run gains nothing from them.
     */
    fn on_request_only(self) -> bool {
        matches!(self, Self::Sha256CommonCrypto | Self::Blake3ServilMt1)
    }

    /// Whether this contender can run in this build, or why not. A
    /// property of the target platform alone, so --list reads the same on
    /// every machine of one platform; machine capacity (CPU count, which
    /// kernel a CPU selects) shows up in the results and the kernel
    /// report, never here.
    fn availability(self) -> Result<(), String> {
        match self {
            Self::Blake3
            | Self::Sha256
            | Self::Sha1Dc
            | Self::Sha256Ring
            | Self::Blake3Servil
            | Self::Blake3Rayon
            | Self::Blake3ServilMt
            | Self::Blake3ServilMt1
            | Self::AbBlake3 => Ok(()),
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
            Self::Blake3Servil => "BLAKE3 servil",
            Self::Sha256CommonCrypto => "SHA-256 CommonCrypto",
            Self::Sha256Ring => "SHA-256 ring",
            Self::Blake3Rayon => "BLAKE3 mt",
            Self::Blake3ServilMt => "BLAKE3 servil mt",
            Self::Blake3ServilMt1 => "BLAKE3 servil mt·1",
            Self::AbBlake3 => "ab-blake3",
        }
    }

    /*
     * Contender colours stay off pure green and pure red, which the hover
     * panel reserves for "faster" and "slower". A multithreaded contender
     * wears a darker shade of its single-threaded sibling's hue.
     */
    fn color(self) -> &'static str {
        match self {
            Self::Blake3 => "#3b82f6",
            Self::Sha256 => "#e07a45",
            Self::Sha1Dc => "#8a7a1e",
            Self::Blake3Servil => "#7c3aed",
            Self::Sha256CommonCrypto => "#0e9aa7",
            Self::Sha256Ring => "#c2410c",
            Self::Blake3Rayon => "#1e3a8a",
            Self::Blake3ServilMt => "#4c1d95",
            Self::Blake3ServilMt1 => "#a78bfa",
            Self::AbBlake3 => "#c026d3",
        }
    }

    /// The Cargo.lock description of the crate that implements this contender.
    fn source(self) -> &'static str {
        match self {
            Self::Blake3 => BLAKE3_SOURCE_INFO,
            Self::Sha256 => SHA2_SOURCE_INFO,
            Self::Sha1Dc => SHA1_CHECKED_SOURCE_INFO,
            Self::Blake3Servil => BLAKE3_SERVIL_SOURCE_INFO,
            Self::Sha256CommonCrypto => "CommonCrypto CC_SHA256_Init/Update/Final from the running macOS (libSystem); version follows the OS",
            Self::Sha256Ring => RING_SOURCE_INFO,
            Self::Blake3Rayon => BLAKE3_SOURCE_INFO,
            Self::Blake3ServilMt | Self::Blake3ServilMt1 => BLAKE3_SERVIL_SOURCE_INFO,
            Self::AbBlake3 => AB_BLAKE3_SOURCE_INFO,
        }
    }

    /// The contender's mode: how many threads it may use and how, for the
    /// report header. The kernels it runs are a separate matter (see
    /// detect_kernels), chosen at run time.
    fn mode(self) -> &'static str {
        match self {
            Self::Blake3
            | Self::Sha256
            | Self::Sha1Dc
            | Self::Sha256CommonCrypto
            | Self::Sha256Ring => "single-threaded",
            Self::Blake3Servil => "single-threaded; blake3_servil::hash for one message, blake3_servil::hash_many for a batch",
            Self::AbBlake3 => "single-threaded; ab_blake3::const_hash for one message, ab_blake3::single_block_hash_many_exact::<N> for a batch of N 64-byte messages",
            Self::Blake3Rayon => "multithreaded; Hasher::update_rayon on Rayon's global pool, the crate's own multithreading as a program gets it by default: the tree splits recursively over the pool, and inputs under a few chunks stay on the caller's thread",
            Self::Blake3ServilMt => "multithreaded; blake3_servil::hash_multithreaded for one message and hash_many_multithreaded for a batch: the fork chooses whether to use its shared resident workers; the kernel tables below show the thresholds",
            Self::Blake3ServilMt1 => "capped at one thread; blake3_servil::hash_multithreaded_with_budget(input, 1) and hash_many_multithreaded_with_budget(batch, digests, 1): the single-threaded path through the multithreaded entry points, a check that it costs what hash() and hash_many() cost",
        }
    }

    /// The threads a multithreaded contender runs on, as far as the
    /// benchmarker can say without asking the implementation for machine
    /// capacity: Rayon's global pool is documented to take one thread per
    /// logical CPU by default; the fork sizes its own workers.
    fn thread_resources(self) -> Option<String> {
        match self {
            Self::Blake3Rayon => Some("Rayon's global pool, at its default size (one thread per logical CPU)".to_owned()),
            _ => None,
        }
    }
}

/*
 * Time per unit in integer picoseconds: per byte on the one-message axis,
 * per message on the many-messages axis. A sample of 1 ms over 64 bytes of
 * input repeated ~20 000 times resolves to better than 1 ps/B, and 1 MiB
 * at 0.17 ns/B is 170 000 ps/B, so u64 has room to spare. Integers keep
 * every median, ratio, and spread exact and reproducible.
 */
type PsPerByte = u64;
const PS_PER_NS: u64 = 1_000;

/*
 * Cycles per byte in integer millicycles. The thread's cycle count over a
 * sample divided by bytes hashed: frequency-independent, so a core boost
 * or throttle mid-run leaves it unmoved, and comparable across runs on the
 * same microarchitecture. Read on Apple silicon from thread_selfcounts;
 * zero where no per-thread cycle counter is available.
 */
type MilliCyclesPerByte = u64;

/*
 * Summary of one cell's samples. `low` and `high` bound the band the graph
 * draws: a 95% bootstrap confidence interval of the median. That interval
 * says how well the median is known; it narrows as 1/√n with more rounds,
 * and outliers barely move it. `minimum` and `maximum` are the extremes
 * seen, for the text report.
 *
 * `modes` is set when the samples split into two clusters at least 4%
 * apart with a tenth or more of the samples on each side. That is a real
 * two-speed behaviour of the code in this context (the interleaving's
 * neighbours, branch predictor state), which a single median cannot
 * express; the hover panel reports both clusters.
 */
#[derive(Clone, Copy)]
struct Statistics {
    /// Samples behind these figures.
    count: usize,
    minimum: u64,
    low: u64,
    median: u64,
    high: u64,
    maximum: u64,
    modes: Option<Modes>,
}

/// Two clusters within one cell: each cluster's median and sample count.
#[derive(Clone, Copy)]
struct Modes {
    lower_median: u64,
    lower_count: usize,
    upper_median: u64,
    upper_count: usize,
}

/// Bootstrap resamples per cell. 400 gives the 2.5th and 97.5th percentiles
/// to within about one rank; the cost is microseconds per cell.
const BOOTSTRAP_RESAMPLES: usize = 400;
/// Consecutive sorted samples this far apart (permille of the median) split
/// the cell into modes, when both sides hold at least MODE_MIN_SHARE.
const MODE_GAP_PERMILLE: u64 = 40;
const MODE_MIN_SHARE_PERMILLE: usize = 100;

/// One (contender, size) cell: the reported time per byte (see TimeBasis),
/// and in a duo run the duo time per byte beside it.
#[derive(Clone, Copy)]
struct Cell {
    time: Statistics,
    /// Time to the later finish of two copies, per byte of one copy;
    /// present in duo runs.
    duo: Option<Statistics>,
}

/// One timed run of a contender over an input.
#[derive(Clone, Copy)]
struct Sample {
    /// Measured on the hardware counter.
    ps_per_byte: PsPerByte,
    /// From the thread cycle counter; zero without one.
    millicycles_per_byte: MilliCyclesPerByte,
    /// The raw readings the two above came from, for the run's clock rate.
    elapsed_ns: u64,
    cycles: u64,
}

/*
 * How each cell's reported time was arrived at.
 *
 * With a per-thread cycle counter, the reported time is cycles per byte
 * at the run's sustained clock: the median over every sample of cycles ÷
 * elapsed time. A core boost or throttle during a sample changes its
 * elapsed time and leaves its cycles alone, so the normalised time is
 * unmoved; the median rate is unmoved too, since excursions are brief.
 * The result reads in the same unit a stopwatch gives, with the machine's
 * frequency excursions taken out.
 *
 * Without a counter the reported time is the measured time.
 */
#[derive(Clone, Copy)]
enum TimeBasis {
    Measured,
    /// Cycles per byte divided by this rate, in kHz, for samples whose own
    /// cycles ÷ elapsed matches the rate; measured time for the rest.
    NormalisedToKhz {
        khz: u64,
        /// Samples whose cycle count fell short of the rate and were
        /// reported as measured time instead.
        measured_samples: usize,
        total_samples: usize,
    },
}

/*
 * A sample's cycles ÷ elapsed is the core clock when the counter saw every
 * cycle. Work on an execution engine the counter misses (Apple's SME2
 * streaming mode counts at 70–75% of the core rate) reads low. A sample
 * more than this far below the run's rate keeps its measured time.
 */
const CYCLE_COUNTER_TOLERANCE_PERMILLE: u64 = 60;

impl TimeBasis {
    fn describe(self) -> String {
        match self {
            Self::Measured => "measured elapsed time on the hardware counter".to_owned(),
            Self::NormalisedToKhz { khz, measured_samples, total_samples } => {
                let mut text = format!(
                    "per-thread cycles at the run's sustained clock, {} GHz (median of cycles ÷ elapsed time over every sample)",
                    format_khz_as_ghz(khz),
                );
                if measured_samples > 0 {
                    write!(
                        text,
                        "; {measured_samples} of {total_samples} samples whose cycle count fell short of that rate (work the counter does not see, such as SME2 streaming mode) keep their measured time",
                    )
                    .unwrap();
                }
                text
            }
        }
    }
}

/// kHz as GHz with three decimals: 3_996_000 → "3.996".
fn format_khz_as_ghz(khz: u64) -> String {
    let mhz = (khz + 500) / 1_000;
    format!("{}.{:03}", mhz / 1_000, mhz % 1_000)
}

struct MachineMetadata {
    timestamp: String,
    cpu_type: String,
    cpu_count: usize,
    os_type: String,
    /// What the OS says about the CPU beyond its brand, so two machines
    /// that report the same brand (every Linux VM on Apple silicon says
    /// "aarch64") can be told apart: see cpu_identity.
    cpu_identity: String,
}

/*
 * The contenders selected for this run, in column order, with the
 * interleaving orders that balance them.
 *
 * Contract: two to six contenders, each available on this machine, no
 * duplicates.
 */
struct Roster {
    algorithms: Vec<Algorithm>,
    orders: Vec<Vec<usize>>,
    /// The points measured, as ascending indices into POINTS: every point,
    /// or the subset --points names.
    points: Vec<usize>,
    /// Sample rounds: a multiple of points.len() and of orders.len().
    rounds: usize,
    /// Every sample runs two independent copies of the contender at once
    /// and times the later finish (see Duo). With `solo`, a solo sample
    /// (one copy, one thread) is taken beside each duo sample and the
    /// report shows both columns. Off by default; --solo enables it.
    solo: bool,
}

impl Roster {
    /*
     * `points` restricts the run to those POINTS indices (every point when
     * None); `rounds` fixes the round count, which must then be a positive
     * multiple of lcm(point count, order count), so every order and every
     * point position recurs equally often.
     */
    fn new(algorithms: Vec<Algorithm>, thorough: bool, solo: bool, points: Option<Vec<usize>>, rounds: Option<usize>) -> Self {
        assert!(
            (2..=8).contains(&algorithms.len()),
            "a run compares two to eight contenders; {} were selected",
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
        let points = points.unwrap_or_else(|| (0..POINT_COUNT).collect());
        assert!(!points.is_empty() && points.windows(2).all(|w| w[0] < w[1]), "points ascend, without repeats");
        let step = lcm(points.len(), orders.len());
        let rounds = match rounds {
            Some(rounds) => {
                assert!(
                    rounds > 0 && rounds % step == 0,
                    "--rounds must be a positive multiple of {step} for these contenders and points"
                );
                rounds
            }
            None => {
                let target = SAMPLE_ROUNDS_TARGET * if thorough { THOROUGH_MULTIPLIER } else { 1 };
                target.div_ceil(step) * step
            }
        };
        Self { algorithms, orders, points, rounds, solo }
    }

    /// Whether this run measures POINTS[point_index].
    fn measures(&self, point_index: usize) -> bool {
        self.points.binary_search(&point_index).is_ok()
    }

    /// The point the progress line follows: the largest one-message
    /// point measured, else the largest point.
    fn progress_point(&self) -> usize {
        *self
            .points
            .iter()
            .rev()
            .find(|&&index| POINTS[index].use_case == UseCase::OneMessage)
            .unwrap_or_else(|| self.points.last().unwrap())
    }

    fn len(&self) -> usize {
        self.algorithms.len()
    }
}

fn gcd(a: usize, b: usize) -> usize {
    if b == 0 { a } else { gcd(b, a % b) }
}

fn lcm(a: usize, b: usize) -> usize {
    a / gcd(a, b) * b
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
bench-hashes: hash throughput by input size, and by messages per batch

  bench-hashes                     SHA-1DC plus the best available BLAKE3 and
                                   SHA-256 on this machine (best = Pareto-better
                                   at every size; the run says so if none is)
  bench-hashes --all               every contender this machine can run,
                                   apart from those marked on-request in --list
  bench-hashes --contenders K,...  exactly these, in this column order
  bench-hashes --list              contenders and their availability here

Keys: blake3, ab-blake3, blake3-servil, sha256, sha256-ring, sha1dc; sha256-cc on
      request; blake3-mt and blake3-servil-mt (multithreaded) in --all and
      default runs, or when named; blake3-servil-mt1 (the multithreaded call
      capped at one thread, a check that it matches blake3-servil) on request

Every run measures two use cases: one message per call at twenty-four input
sizes from 64 B to 128 MiB, and a batch of 64-byte messages per call at
twenty-four batch sizes from 1 to 262144 messages (BLAKE3 mt sits that one out).

  --points LABEL,...               measure only these points (labels as in the
                                   report: \"64 B\", \"8 MiB\", \"1024\" messages);
                                   with --contenders only; no graph
  --rounds N                       exactly N sample rounds (a multiple of the
                                   point count and the order count)
  --solo                           also take a solo sample (one copy, one
                                   thread) beside each duo sample and report
                                   solo and duo side by side
  --thorough                       three times the sample rounds, for narrower
                                   bands; about three times the run time
  --trace-clocks PATH              also write one CSV line per sample with
                                   wall, thread-CPU, mach_absolute_time, and
                                   (on Apple) per-core-kind cycles and
                                   instructions, for clock and frequency
                                   diagnosis
";

struct Options {
    selection: Selection,
    explicit: Vec<Algorithm>,
    points: Option<Vec<usize>>,
    rounds: Option<usize>,
    trace_path: Option<std::path::PathBuf>,
    thorough: bool,
    /// Also take a solo sample beside each duo sample.
    solo: bool,
}

fn parse_arguments() -> Options {
    let mut arguments: Vec<String> = std::env::args().skip(1).collect();

    /* --thorough and --solo may accompany any selection. Every run is a duo run. */
    let mut take_flag = |flag: &str| {
        arguments
            .iter()
            .position(|argument| argument == flag)
            .map(|index| {
                arguments.remove(index);
            })
            .is_some()
    };
    let thorough = take_flag("--thorough");
    let solo = take_flag("--solo");

    /* --trace-clocks PATH may accompany any selection. */
    let trace_path = arguments
        .iter()
        .position(|argument| argument == "--trace-clocks")
        .map(|index| {
            assert!(index + 1 < arguments.len(), "--trace-clocks needs a file path\n\n{USAGE}");
            let path = std::path::PathBuf::from(&arguments[index + 1]);
            arguments.drain(index..=index + 1);
            path
        });

    /* --points LIST and --rounds N narrow a --contenders run. */
    let mut take_value = |flag: &str| {
        arguments.iter().position(|argument| argument == flag).map(|index| {
            assert!(index + 1 < arguments.len(), "{flag} needs a value\n\n{USAGE}");
            let value = arguments[index + 1].clone();
            arguments.drain(index..=index + 1);
            value
        })
    };
    let points = take_value("--points").map(|list| {
        let mut indices: Vec<usize> = list
            .split(',')
            .map(|label| {
                POINTS.iter().position(|point| point.label == label.trim()).unwrap_or_else(|| {
                    let labels: Vec<&str> = POINTS.iter().map(|point| point.label).collect();
                    panic!("--points: no point labelled {label:?}; the labels are {}", labels.join(", "))
                })
            })
            .collect();
        indices.sort_unstable();
        indices.dedup();
        indices
    });
    let rounds = take_value("--rounds").map(|n| n.parse::<usize>().expect("--rounds takes a whole number"));

    let (selection, explicit) = parse_selection(&arguments);
    assert!(
        (points.is_none() && rounds.is_none()) || selection == Selection::Explicit,
        "--points and --rounds narrow a --contenders run\n\n{USAGE}"
    );
    Options { selection, explicit, points, rounds, trace_path, thorough, solo }
}

fn parse_selection(arguments: &[String]) -> (Selection, Vec<Algorithm>) {
    match arguments {
        [] => (Selection::Best, Vec::new()),
        [flag] if flag == "--all" => (Selection::All, Vec::new()),
        [flag] if flag == "--list" => {
            for algorithm in Algorithm::ALL {
                let status = match algorithm.availability() {
                    Ok(()) if algorithm.on_request_only() => "available; runs only when named with --contenders".to_owned(),
                    Ok(()) => "available".to_owned(),
                    Err(reason) => format!("unavailable: {reason}"),
                };
                println!("  {:<18} {:<22} {status}", algorithm.key(), algorithm.name());
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
    let Options { selection, explicit, points, rounds, trace_path, thorough, solo } = parse_arguments();
    let mut trace = trace_path.map(ClockTrace::new);
    assert!(
        trace.is_none() || solo,
        "--trace-clocks reads one thread's clocks around the solo sample, so it needs --solo"
    );
    let available: Vec<Algorithm> = Algorithm::ALL
        .into_iter()
        .filter(|algorithm| algorithm.availability().is_ok() && !algorithm.on_request_only())
        .collect();

    let machine = machine_metadata();

    /*
     * The default run picks the best available member of each family. That
     * needs measurements, so it measures every available contender, then
     * keeps the Pareto-best per family and reports on those alone. Timing
     * cost is the same as --all; only the report narrows.
     */
    let (roster, results, basis, duo_samples, selection_note) = match selection {
        Selection::Explicit => {
            let keys = explicit.iter().map(|algorithm| algorithm.key()).collect::<Vec<_>>().join(",");
            let roster = Roster::new(explicit, thorough, solo, points, rounds);
            let (results, basis, duo_samples) = measure_all(&roster, trace.as_mut());
            (roster, results, basis, duo_samples, format!("--contenders {keys}"))
        }
        Selection::All => {
            let roster = Roster::new(available, thorough, solo, None, None);
            let (results, basis, duo_samples) = measure_all(&roster, trace.as_mut());
            (roster, results, basis, duo_samples, String::from("every contender available on this machine"))
        }
        Selection::Best => {
            let full = Roster::new(available, thorough, solo, None, None);
            let (full_results, basis, full_samples) = measure_all(&full, trace.as_mut());
            let (keep, note) = choose_best_per_family(&full, &full_results);
            let roster = Roster::new(keep.iter().map(|&index| full.algorithms[index]).collect(), thorough, solo, None, None);
            let results: Results = full_results
                .iter()
                .enumerate()
                .filter(|(index, _)| keep.contains(index))
                .map(|(_, row)| row.clone())
                .collect();
            let duo_samples: DuoSamples = full_samples
                .into_iter()
                .enumerate()
                .filter(|(index, _)| keep.contains(index))
                .map(|(_, row)| row)
                .collect();
            (roster, results, basis, duo_samples, note)
        }
    };

    if let Some(trace) = &trace {
        trace.write();
    }

    /* The Measurement section explains duo; the note names the selection only. */
    let text = generate_text(&roster, &results, &machine, &selection_note, basis);
    /* The graph draws whole axes, so a run of a subset of points has none. */
    let svg = (roster.points.len() == POINT_COUNT).then(|| generate_svg(&roster, &results, &machine, &selection_note, basis));

    print!("{text}");

    let directory = output_directory(&machine);

    fs::create_dir_all(&directory).unwrap_or_else(|error| {
        panic!(
            "failed to create output directory {}: {error}",
            directory.display(),
        )
    });

    /* Every run measures duo; --solo adds solo beside it under the same names. */
    let stem = "bench-hashes.duo";
    let text_path = directory.join(format!("{stem}.result.txt"));
    let svg_path = directory.join(format!("{stem}.graph.svg"));
    let samples_path = directory.join(format!("{stem}.samples.tsv"));
    let samples = generate_samples_tsv(&roster, &duo_samples, &machine, &selection_note);
    fs::write(&samples_path, &samples).unwrap_or_else(|error| {
        panic!("failed to write {}: {error}", samples_path.display())
    });

    fs::write(&text_path, &text).unwrap_or_else(|error| {
        panic!("failed to write {}: {error}", text_path.display())
    });

    if let Some(svg) = &svg {
        fs::write(&svg_path, svg).unwrap_or_else(|error| {
            panic!("failed to write {}: {error}", svg_path.display())
        });
    }

    println!(
        "# Data results (text) are in \"{}\" .",
        text_path.display(),
    );
    match svg {
        Some(_) => println!("# Graph results (SVG) are in \"{}\" .", svg_path.display()),
        None => println!("# No graph: --points measured a subset of the points."),
    }
    println!("# Samples (TSV) are in \"{}\" .", samples_path.display());
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
 * is at least as fast (by median) at every point both take part in, and
 * strictly faster at one, is the best. Without such a member the family
 * has no best: both are kept and the note says so. Returns the kept
 * indices in roster order.
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
            for point_index in 0..POINT_COUNT {
                let (Some(ca), Some(cb)) = (results[a][point_index], results[b][point_index]) else {
                    continue;
                };
                let (ma, mb) = (ca.time.median, cb.time.median);
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
                    "no best {} here: {} each win at some points{}; all are shown",
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

/// The first one-message size at which the faster of two contenders changes.
fn first_crossover(results: &Results, a: usize, b: usize) -> Option<&'static str> {
    let leader = |size_index: usize| cell(results, a, size_index).time.median < cell(results, b, size_index).time.median;
    (1..INPUT_COUNT)
        .find(|&size_index| leader(size_index) != leader(size_index - 1))
        .map(|size_index| POINTS[size_index].label)
}

/// The measured cell of a contender that takes part at this point.
fn cell(results: &Results, algorithm_index: usize, point_index: usize) -> &Cell {
    results[algorithm_index][point_index]
        .as_ref()
        .expect("the contender takes part in this point's use case")
}

fn measure_all(roster: &Roster, mut trace: Option<&mut ClockTrace>) -> (Results, TimeBasis, DuoSamples) {
    /* Inputs for the points measured; an empty buffer stands in for the rest. */
    let inputs: Vec<Vec<u8>> = (0..POINT_COUNT)
        .map(|index| if roster.measures(index) { make_input(POINTS[index].bytes) } else { Vec::new() })
        .collect();
    /*
     * The second copy in a duo sample hashes its own buffer of the same
     * size and different contents, as two independent programs would;
     * sharing one buffer would let the copies share cache lines.
     */
    let duo_inputs: Vec<Vec<u8>> = (0..POINT_COUNT)
        .map(|index| if roster.measures(index) { make_input_seeded(POINTS[index].bytes, 1) } else { Vec::new() })
        .collect();
    let mut progress = Progress::new(roster);
    progress.phase("checking digests");
    // Both timed input sets visit every implementation's selected kernels.
    // Empty and short tails cover boundaries absent from the timing grid.
    for (seed, buffers) in [(0, &inputs), (1, &duo_inputs)] {
        for (index, (point, input)) in POINTS.iter().zip(buffers).enumerate() {
            if roster.measures(index) {
                check_input(&roster.algorithms, input, point.messages, seed);
            }
        }
    }
    for &(len, seed, _) in test_vectors::VECTORS {
        if !POINTS.iter().any(|point| point.messages == 1 && point.bytes == len) {
            check_input(&roster.algorithms, &make_input_seeded(len, seed), 1, seed);
        }
    }
    let duo = Duo::new();

    /*
     * Each algorithm/point combination gets its own calibrated iteration
     * count so that timed blocks have approximately equal durations.
     * The digest checks have already called each contender at every point.
     * Startup and calibration both happen before the timed samples.
     */
    progress.phase("calibrating");

    let mut batch_iterations: Vec<Vec<usize>> = vec![vec![1usize; POINT_COUNT]; roster.len()];
    /* Cells under the time budget (see LONG_HASH_NS). */
    let mut budgeted: Vec<Vec<bool>> = vec![vec![false; POINT_COUNT]; roster.len()];

    for (point_index, point) in POINTS.iter().enumerate() {
        for algorithm_index in 0..roster.len() {
            let algorithm = roster.algorithms[algorithm_index];
            if algorithm.takes_part(point.use_case) && roster.measures(point_index) {
                let (iterations, per_iteration_ns) = calibrate_batch(algorithm, &inputs[point_index], point.messages);
                batch_iterations[algorithm_index][point_index] = iterations;
                budgeted[algorithm_index][point_index] = iterations == 1 && per_iteration_ns >= LONG_HASH_NS;
            }
        }
    }

    /*
     * No warm-up phase. Calibration has just run every contender at every
     * size, and a cold-start cost that survived it would be one sample
     * among the rounds, which the median drops. A separate warm-up would
     * change nothing measurable and cost a minute of run time.
     */

    let mut samples: Samples = (0..roster.len())
        .map(|_| (0..POINT_COUNT).map(|_| Vec::with_capacity(if roster.solo { roster.rounds } else { 0 })).collect())
        .collect();
    let mut duo_samples: DuoSamples = (0..roster.len())
        .map(|_| (0..POINT_COUNT).map(|_| Vec::with_capacity(roster.rounds)).collect())
        .collect();

    /*
     * The algorithm order cycles through the Williams orders. Point order
     * rotates independently. This distributes ordering, thermal, and
     * system-load effects across the algorithms.
     */
    progress.phase("measuring");

    for round in 0..roster.rounds {
        if roster.solo {
            progress.round(round, &samples);
        } else {
            progress.round_duo(round, &duo_samples);
        }

        let algorithm_order = &roster.orders[round % roster.orders.len()];

        for point_offset in 0..roster.points.len() {
            let size_index = roster.points[(point_offset + round) % roster.points.len()];
            let point = POINTS[size_index];

            let input = &inputs[size_index];

            for (position, &algorithm_index) in algorithm_order.iter().enumerate() {
                let algorithm = roster.algorithms[algorithm_index];
                if !algorithm.takes_part(point.use_case) {
                    continue;
                }
                let iterations =
                    batch_iterations[algorithm_index][size_index];
                if budgeted[algorithm_index][size_index]
                    && !long_cell_wants_sample(&duo_samples[algorithm_index][size_index], round + size_index)
                {
                    continue;
                }

                /* Trace reads bracket the sample; the sample clock sits innermost. */
                let (trace_cpu0, trace_proc0, trace_mach0, trace_perf0) = if trace.is_some() {
                    (
                        trace_clocks::thread_cpu_ns(),
                        trace_clocks::process_cpu_ns(),
                        trace_clocks::mach_ticks(),
                        trace_clocks::perf_counters(),
                    )
                } else {
                    (0, 0, 0, PerfCounters::default())
                };

                /*
                 * The solo sample (--solo only): this thread runs the batch
                 * and its own cycle counter describes the work.
                 */
                let (elapsed_ns, cycles) = if roster.solo {
                    let cycles0 = trace_clocks::thread_cycles();
                    let started = sample_clock::now();
                    run_batch(algorithm, input, point.messages, iterations);
                    let elapsed_ns = sample_clock::since_ns(started);
                    (elapsed_ns, trace_clocks::thread_cycles() - cycles0)
                } else {
                    (0, 0)
                };

                /*
                 * The duo sample: two threads run a batch each and the
                 * sample is the time to the later finish. With --solo it is
                 * taken beside the solo sample, under the same conditions,
                 * so the two columns compare. The copies' cycle counts
                 * describe two threads, so duo times are measured time.
                 */
                let later_ns = duo.run(algorithm, input, &duo_inputs[size_index], point.messages, iterations);
                let total_units = point.use_case.units(point, iterations);
                let later_ps = later_ns.checked_mul(PS_PER_NS).expect("a sample of under a second fits in picoseconds");
                duo_samples[algorithm_index][size_index].push((later_ps + total_units / 2) / total_units);

                if let Some(trace) = trace.as_deref_mut() {
                    let perf1 = trace_clocks::perf_counters();
                    let mach1 = trace_clocks::mach_ticks();
                    let proc1 = trace_clocks::process_cpu_ns();
                    let cpu1 = trace_clocks::thread_cpu_ns();
                    let perf = perf1.since(trace_perf0);
                    trace.lines.push(format!(
                        "{round},{},{},{},{iterations},{elapsed_ns},{},{},{},{},{},{},{},{},{}",
                        point_offset * algorithm_order.len() + position,
                        algorithm.key(),
                        input.len(),
                        cpu1 - trace_cpu0,
                        mach1 - trace_mach0,
                        proc1 - trace_proc0,
                        perf.p_cycles,
                        perf.p_instructions,
                        perf.p_time_ns,
                        perf.e_cycles,
                        perf.e_instructions,
                        perf.e_time_ns,
                    ));
                }

                if roster.solo {
                    let total_units = point.use_case.units(point, iterations);

                    /* Rounded to the nearest picosecond per unit. */
                    let elapsed_ps = elapsed_ns
                        .checked_mul(PS_PER_NS)
                        .expect("a sample of under a second fits in picoseconds");
                    let ps_per_byte = (elapsed_ps + total_units / 2) / total_units;

                    assert!(ps_per_byte > 0, "every timing sample must be positive");

                    /* Rounded to the nearest millicycle per unit; zero without a counter. */
                    let millicycles_per_byte = (cycles * 1_000 + total_units / 2) / total_units;

                    samples[algorithm_index][size_index]
                        .push(Sample { ps_per_byte, millicycles_per_byte, elapsed_ns, cycles });
                }
            }
        }
    }

    if roster.solo {
        progress.finish(&samples);
    } else {
        progress.finish_duo(&duo_samples);
    }

    /* Duo-only runs report measured time; --solo runs keep the cycle-normalised basis. */
    let basis = if roster.solo { time_basis(&samples) } else { TimeBasis::Measured };

    let mut results: Results = vec![vec![None; POINT_COUNT]; roster.len()];

    for algorithm_index in 0..roster.len() {
        for (size_index, point) in POINTS.iter().enumerate() {
            if !roster.algorithms[algorithm_index].takes_part(point.use_case) || !roster.measures(size_index) {
                continue;
            }
            let duo = &mut duo_samples[algorithm_index][size_index];
            assert!(!duo.is_empty() && duo.len() <= roster.rounds, "one duo sample per round at most, and one at least");
            let cell = if roster.solo {
                let mut cell = summarize_cell(&samples[algorithm_index][size_index], basis);
                cell.duo = Some(summarize(&mut duo.clone()));
                cell
            } else {
                /* Duo-only: the reported column is the duo measurement. */
                Cell { time: summarize(&mut duo.clone()), duo: None }
            };
            results[algorithm_index][size_index] = Some(cell);
        }
    }

    (results, basis, duo_samples)
}

/*
 * The run's sustained clock: the median over all samples of cycles per
 * elapsed nanosecond, in kHz. Measured when any sample lacks cycles.
 */
fn time_basis(samples: &Samples) -> TimeBasis {
    let mut rates_khz: Vec<u64> = Vec::new();
    for cell in samples.iter().flatten() {
        for sample in cell {
            if sample.cycles == 0 {
                return TimeBasis::Measured;
            }
            /* cycles / ns = GHz; × 10⁶ = kHz. Rounded. */
            rates_khz.push((sample.cycles * 1_000_000 + sample.elapsed_ns / 2) / sample.elapsed_ns);
        }
    }
    if rates_khz.is_empty() {
        return TimeBasis::Measured;
    }
    let total_samples = rates_khz.len();
    rates_khz.sort_unstable();
    let khz = median_of_sorted(&rates_khz);
    let measured_samples = rates_khz
        .iter()
        .filter(|&&rate| !cycle_count_trustworthy(rate, khz))
        .count();
    TimeBasis::NormalisedToKhz { khz, measured_samples, total_samples }
}

/// Whether a sample's own cycles-per-nanosecond rate is close enough to the
/// run's sustained clock for its cycle count to have seen all the work.
fn cycle_count_trustworthy(sample_khz: u64, sustained_khz: u64) -> bool {
    sample_khz * 1000 >= sustained_khz * (1000 - CYCLE_COUNTER_TOLERANCE_PERMILLE)
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
        self.round_inner(round, &running_medians(self.roster, samples, self.roster.progress_point()));
    }

    /* Duo-only runs track the duo samples instead. */
    fn round_duo(&mut self, round: usize, samples: &DuoSamples) {
        self.round_inner(round, &running_duo_medians(self.roster, samples, self.roster.progress_point()));
    }

    fn round_inner(&mut self, round: usize, medians: &str) {
        let measuring_started = self
            .measuring_started
            .expect("round() runs inside the measuring phase");

        let rounds = self.roster.rounds;
        let done = round as f64 / rounds as f64;
        let filled = (done * Self::BAR_WIDTH as f64).round() as usize;
        let bar: String = "█".repeat(filled) + &"░".repeat(Self::BAR_WIDTH - filled);

        let elapsed = measuring_started.elapsed().as_secs_f64();
        let remaining = if round > 0 {
            format!("{:>3.0}s left", elapsed / done * (1.0 - done))
        } else {
            " estimating".to_owned()
        };

        self.draw(&format!(
            "[{:>5.1}s] measuring {bar} {:>3}/{rounds} rounds · {remaining} · {medians}",
            self.started.elapsed().as_secs_f64(),
            round,
        ));
    }

    fn finish(&mut self, samples: &Samples) {
        self.finish_inner(&running_medians(self.roster, samples, self.roster.progress_point()));
    }

    fn finish_duo(&mut self, samples: &DuoSamples) {
        self.finish_inner(&running_duo_medians(self.roster, samples, self.roster.progress_point()));
    }

    fn finish_inner(&mut self, medians: &str) {
        let bar = "█".repeat(Self::BAR_WIDTH);
        let rounds = self.roster.rounds;
        self.draw(&format!(
            "[{:>5.1}s] measured  {bar} {rounds}/{rounds} rounds · {medians}",
            self.started.elapsed().as_secs_f64(),
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
        return format!("medians at {} pending", POINTS[size_index].label);
    }

    let parts: Vec<String> = (0..roster.len())
        .map(|algorithm_index| {
            let mut sorted: Vec<u64> = samples[algorithm_index][size_index].iter().map(|s| s.ps_per_byte).collect();
            sorted.sort_unstable();
            format!("{} {}", roster.algorithms[algorithm_index].name(), format_ps(median_of_sorted(&sorted)))
        })
        .collect();

    format!("{} ns/B at {}", parts.join(" · "), POINTS[size_index].label)
}

/* Duo-only progress: medians over the duo samples collected so far. */
fn running_duo_medians(roster: &Roster, samples: &DuoSamples, size_index: usize) -> String {
    if samples[0][size_index].is_empty() {
        return format!("medians at {} pending", POINTS[size_index].label);
    }
    let parts: Vec<String> = (0..roster.len())
        .map(|algorithm_index| {
            let mut sorted = samples[algorithm_index][size_index].clone();
            sorted.sort_unstable();
            format!("{} {}", roster.algorithms[algorithm_index].name(), format_ps(median_of_sorted(&sorted)))
        })
        .collect();
    format!("{} ns/B at {}", parts.join(" · "), POINTS[size_index].label)
}

fn make_input(size: usize) -> Vec<u8> {
    make_input_seeded(size, 0)
}

/// The input of `size` bytes for `seed`: `seed` 0 is make_input's buffer,
/// seed 1 the duo copy's. This stream is frozen by test_vectors.rs.
fn make_input_seeded(size: usize, seed: u64) -> Vec<u8> {
    /*
     * Little-endian 64-bit words `seed << 48 | index`: every block of every
     * input differs, so a kernel that mixed up its lanes would fail the
     * golden digests, and the two seeds give the duo copies different
     * contents. Hash speed does not depend on the bytes. Generated
     * outside timed intervals.
     */
    let mut input: Vec<u8> = (0..size.div_ceil(8) as u64).flat_map(|index| (seed << 48 | index).to_le_bytes()).collect();
    input.truncate(size);
    input
}

/// Check exactly the entry points used by timed batches, on identical
/// bytes against checked-in golden digests: the digest itself for one
/// message, SHA-256 over the concatenated digests for a batch of
/// `messages`. Multithreaded entries also run two simultaneous calls over
/// the same vectors, exercising shared pools.
fn check_input(algorithms: &[Algorithm], input: &[u8], messages: usize, seed: u64) {
    assert!(!algorithms.is_empty(), "correctness checks need a contender");
    let use_case = if messages == 1 { UseCase::OneMessage } else { UseCase::ManyMessages };
    for &algorithm in algorithms.iter().filter(|algorithm| algorithm.takes_part(use_case)) {
        assert!(algorithm.availability().is_ok(), "{} must be available", algorithm.key());
        let expected = expected_digest(algorithm.family(), input.len(), messages, seed);
        let check = || {
            let mut digests = Vec::new();
            hash_batch(algorithm, input, messages, 1, |digest| digests.extend_from_slice(digest));
            let actual = if messages == 1 { digests } else { Sha256::digest(&digests).to_vec() };
            assert_digest_matches(algorithm, input.len(), messages, seed, &expected, &actual);
        };
        check();
        if algorithm.multithreaded() {
            let barrier = std::sync::Barrier::new(2);
            std::thread::scope(|scope| {
                for _ in 0..2 {
                    scope.spawn(|| {
                        barrier.wait();
                        check();
                    });
                }
            });
        }
    }
}

/// Every checked input has a frozen vector: (length, seed) for one
/// message, (messages, seed) for a batch. Hex decoding and lookup happen
/// outside timed batches.
fn expected_digest(family: Family, len: usize, messages: usize, seed: u64) -> Vec<u8> {
    let (_, _, digests) = if messages == 1 {
        test_vectors::VECTORS.iter()
            .find(|&&(n, s, _)| n == len && s == seed)
            .unwrap_or_else(|| panic!("missing golden vector for length {len}, seed {seed}"))
    } else {
        assert_eq!(len, messages * MESSAGE_LEN, "a batch is {messages} messages of {MESSAGE_LEN} bytes");
        test_vectors::MANY_VECTORS.iter()
            .find(|&&(n, s, _)| n == messages && s == seed)
            .unwrap_or_else(|| panic!("missing golden vector for {messages} messages, seed {seed}"))
    };
    let index = match family { Family::Blake3 => 0, Family::Sha256 => 1, Family::Sha1Dc => 2 };
    let hex = digests[index];
    assert_eq!(hex.len(), if family == Family::Sha1Dc && messages == 1 { 40 } else { 64 });
    (0..hex.len()).step_by(2).map(|i|
        u8::from_str_radix(&hex[i..i + 2], 16).expect("golden digests are hexadecimal")
    ).collect()
}

fn assert_digest_matches(algorithm: Algorithm, len: usize, messages: usize, seed: u64, expected: &[u8], actual: &[u8]) {
    if messages == 1 {
        assert_eq!(actual, expected, "{} ({}) disagrees with golden vector on {} input bytes, seed {}",
            algorithm.key(), algorithm.family().name(), len, seed);
    } else {
        assert_eq!(actual, expected, "{} ({}) disagrees with golden vector on a batch of {} messages of {} bytes, seed {}",
            algorithm.key(), algorithm.family().name(), messages, MESSAGE_LEN, seed);
    }
}

fn run_batch(algorithm: Algorithm, input: &[u8], messages: usize, iterations: usize) {
    hash_batch(algorithm, input, messages, iterations, |digest| { black_box(digest); });
}

/*
 * One dispatch for both timing and correctness checks. The callback is
 * monomorphized: timed batches black-box each digest, and checking batches
 * collect it. Selection and allocation stay outside the per-hash loop.
 *
 * `input` holds `messages` messages: the whole slice when `messages` is
 * one, else `messages` × MESSAGE_LEN bytes. Every contender hashes the
 * messages one call each through its plain entry point, except ab-blake3,
 * whose single_block_hash_many_exact takes the batch as one array; its
 * message count is a const generic, so each batch size on the axis is its
 * own call. The contender must take part in the point's use case.
 */
fn hash_batch(
    algorithm: Algorithm,
    input: &[u8],
    messages: usize,
    iterations: usize,
    mut consume: impl FnMut(&[u8]),
) {
    assert!(iterations > 0, "batch size must be positive");
    assert!(messages == 1 || input.len() == messages * MESSAGE_LEN, "a batch is {messages} messages of {MESSAGE_LEN} bytes");

    match algorithm {
        Algorithm::Blake3 => each_message(input, messages, iterations, |m| *blake3::hash(m).as_bytes(), consume),
        Algorithm::Sha256 => each_message(input, messages, iterations, |m| Sha256::digest(m), consume),
        Algorithm::Sha1Dc => each_message(input, messages, iterations, |m| {
            let result = sha1_checked::Sha1::try_digest(m);
            let mut digest = [0u8; 20];
            digest.copy_from_slice(result.hash());
            digest
        }, consume),
        Algorithm::Blake3Servil => {
            if messages == 1 {
                each_message(input, messages, iterations, |m| *blake3_servil::hash(m).as_bytes(), consume)
            } else {
                servil_batch(input, messages, iterations, blake3_servil::hash_many, consume)
            }
        }
        Algorithm::Sha256CommonCrypto => each_message(input, messages, iterations, |m| common_crypto::sha256(m), consume),
        Algorithm::Sha256Ring => each_message(input, messages, iterations, |m| ring::digest::digest(&ring::digest::SHA256, m), consume),
        Algorithm::Blake3Rayon => {
            assert_eq!(messages, 1, "BLAKE3 mt takes no part in the many-messages use case");
            each_message(input, messages, iterations, |m| *blake3::Hasher::new().update_rayon(m).finalize().as_bytes(), consume)
        }
        Algorithm::Blake3ServilMt => {
            if messages == 1 {
                each_message(input, messages, iterations, |m| *blake3_servil::hash_multithreaded(m).as_bytes(), consume)
            } else {
                servil_batch(input, messages, iterations, blake3_servil::hash_many_multithreaded, consume)
            }
        }
        Algorithm::Blake3ServilMt1 => {
            if messages == 1 {
                each_message(input, messages, iterations, |m| *blake3_servil::hash_multithreaded_with_budget(m, 1).as_bytes(), consume)
            } else {
                servil_batch(input, messages, iterations, |b, d| blake3_servil::hash_many_multithreaded_with_budget(b, d, 1), consume)
            }
        }
        Algorithm::AbBlake3 => {
            if messages == 1 {
                each_message(input, messages, iterations, |m| ab_blake3::const_hash(m), consume)
            } else {
                let blocks: &[[u8; MESSAGE_LEN]] = input.as_chunks::<MESSAGE_LEN>().0;
                let mut outputs = vec![[0u8; 32]; messages];
                for _ in 0..iterations {
                    ab_blake3_hash_many(black_box(blocks), &mut outputs);
                    consume(outputs.as_flattened());
                }
            }
        }
    }
}

/// `iterations` passes over the batch through one of the fork's batch entry
/// points, which take the messages as a slice of slices and fill a slice
/// of digests; the whole batch's digests go to `consume` per pass.
#[inline(always)]
fn servil_batch(
    input: &[u8],
    messages: usize,
    iterations: usize,
    hash_many: impl Fn(&[&[u8]], &mut [blake3_servil::Hash]),
    mut consume: impl FnMut(&[u8]),
) {
    let batch: Vec<&[u8]> = input.chunks_exact(MESSAGE_LEN).collect();
    assert_eq!(batch.len(), messages);
    let mut digests = vec![blake3_servil::Hash::from_bytes([0; 32]); messages];
    for _ in 0..iterations {
        hash_many(black_box(&batch), &mut digests);
        for digest in &digests {
            consume(digest.as_bytes());
        }
    }
}

/// `iterations` passes over the input, each hashing every message with one
/// call of `hash`, digest by digest into `consume`.
#[inline(always)]
fn each_message<D: AsRef<[u8]>>(
    input: &[u8],
    messages: usize,
    iterations: usize,
    hash: impl Fn(&[u8]) -> D,
    mut consume: impl FnMut(&[u8]),
) {
    for _ in 0..iterations {
        if messages == 1 {
            consume(hash(black_box(input)).as_ref());
        } else {
            for message in black_box(input).chunks_exact(MESSAGE_LEN) {
                consume(hash(message).as_ref());
            }
        }
    }
}

/// ab_blake3::single_block_hash_many_exact::<N> over a batch of N blocks.
/// N is a const generic, so the batch sizes of the many-messages axis are
/// the ones this function can be called with.
fn ab_blake3_hash_many(blocks: &[[u8; MESSAGE_LEN]], outputs: &mut [[u8; 32]]) {
    macro_rules! exact {
        ($($n:literal),*) => {
            match blocks.len() {
                $($n => ab_blake3::single_block_hash_many_exact::<$n>(
                    blocks.try_into().unwrap(),
                    outputs.try_into().unwrap(),
                ),)*
                other => panic!("a batch of {other} messages is off the many-messages axis"),
            }
        };
    }
    exact!(1, 2, 3, 4, 6, 8, 12, 16, 24, 32, 48, 64, 128, 256, 512, 1024, 2048, 4096, 8192, 16384, 32768, 65536, 131072, 262144)
}

/*
 * The duo measurement: two independent copies of a contender run at once,
 * each on its own thread over its own input, and the sample is the time
 * from a shared release to the later finish. A hash that takes the whole
 * machine to go faster alone runs beside a copy of itself here and shows
 * what that costs; a hash that leaves room finishes at its solo speed.
 * Every run measures duo; with --solo every sample interval takes a solo
 * sample and then a duo sample of the same batch, so each cell reports
 * both, side by side, from the same moment of the run.
 *
 * The two copy threads persist for the run. The caller posts a job, and
 * each copy takes it and polls a generation counter (yielding between
 * polls) until the caller has seen both arrive and flips it. Both copies
 * are then on a CPU, in the instruction stream, when the release happens,
 * and each reads the sample clock as its own first act; the later finish
 * is the later of the two finish times, each measured from that copy's
 * own start. A release through a barrier or condition variable would
 * instead need the operating system to wake a sleeping thread, which on a
 * busy machine can take hundreds of microseconds (a two-CPU virtual
 * machine measured 300 µs when the caller's own thread had just finished
 * a batch on that CPU); a copy that starts late finishes late through no
 * fault of the contender, and the pair's later finish would carry the
 * wake, not the hash. A polling copy needs no wake.
 *
 * The caller then sleeps on a condition variable until both copies have
 * finished. The finishes are sample-clock reads the copies take
 * themselves, so the caller's own wake, however slow, enters no sample;
 * and a sleeping caller holds no CPU, which matters on a machine with as
 * many CPUs as copies. Between jobs the copies sleep the same way, so an
 * idle run holds no CPU either.
 *
 * Reported per byte per copy: a duo sample over N bytes per copy is
 * divided by N, so the number reads as "the time one hash costs when
 * another runs beside it".
 */
struct Duo {
    /// A job for both threads, or None between jobs.
    job: std::sync::Mutex<Option<DuoJob>>,
    posted: std::sync::Condvar,
    /// Copies that hold the job and are spinning, ready to start.
    ready: std::sync::atomic::AtomicUsize,
    /// Advances once both copies are ready: the release.
    generation: std::sync::atomic::AtomicU64,
    /// Each copy's elapsed nanoseconds from its own start to its finish;
    /// None while a copy is still running.
    finished: std::sync::Mutex<[Option<u64>; 2]>,
    done: std::sync::Condvar,
}

#[derive(Clone, Copy)]
struct DuoJob {
    algorithm: Algorithm,
    inputs: [*const [u8]; 2],
    messages: usize,
    iterations: usize,
    /// Which workers have taken this job (a bit each).
    taken: u8,
}

// The input pointers are borrows of the caller's buffers, which outlive the
// job: run() returns only after both finishes are read.
unsafe impl Send for DuoJob {}

impl Duo {
    fn new() -> &'static Self {
        use std::sync::atomic::{AtomicU64, AtomicUsize};
        let duo: &'static Self = Box::leak(Box::new(Self {
            job: std::sync::Mutex::new(None),
            posted: std::sync::Condvar::new(),
            ready: AtomicUsize::new(0),
            generation: AtomicU64::new(0),
            finished: std::sync::Mutex::new([None, None]),
            done: std::sync::Condvar::new(),
        }));
        for copy in 0..2 {
            std::thread::Builder::new()
                .name(format!("duo-copy-{copy}"))
                .spawn(move || duo.worker(copy))
                .expect("spawning a duo copy thread");
        }
        duo
    }

    /// Run `iterations` of `algorithm` on both threads at once, copy 0 over
    /// `input` and copy 1 over `other`; returns the later finish in
    /// nanoseconds, each copy timed from its own start.
    fn run(&self, algorithm: Algorithm, input: &[u8], other: &[u8], messages: usize, iterations: usize) -> u64 {
        use std::sync::atomic::Ordering;
        assert_eq!(input.len(), other.len(), "the two copies hash inputs of one size");
        {
            let mut job = self.job.lock().unwrap();
            assert!(job.is_none(), "one duo job at a time");
            *job = Some(DuoJob { algorithm, inputs: [input, other], messages, iterations, taken: 0 });
            self.posted.notify_all();
        }
        /*
         * Both copies spinning: release. The caller yields between polls
         * so that on a machine with as many CPUs as copies the copies get
         * theirs; a copy is spinning within microseconds of the post.
         */
        while self.ready.load(Ordering::Acquire) < 2 {
            std::thread::yield_now();
        }
        self.ready.store(0, Ordering::Release);
        self.generation.fetch_add(1, Ordering::AcqRel);
        /* Sleep until both copies have finished. */
        let mut finished = self.finished.lock().unwrap();
        while finished.iter().any(Option::is_none) {
            finished = self.done.wait(finished).unwrap();
        }
        let later = finished.iter().map(|f| f.unwrap()).max().unwrap();
        *finished = [None, None];
        later
    }

    fn worker(&self, copy: usize) {
        use std::sync::atomic::Ordering;
        let mut seen = self.generation.load(Ordering::Acquire);
        loop {
            let job = {
                let mut job = self.job.lock().unwrap();
                loop {
                    if let Some(current) = job.as_mut() {
                        if current.taken & (1 << copy) == 0 {
                            current.taken |= 1 << copy;
                            let taken = *current;
                            if taken.taken == 0b11 {
                                *job = None;
                            }
                            break taken;
                        }
                    }
                    job = self.posted.wait(job).unwrap();
                }
            };
            /*
             * Arrive, then poll until the caller releases, yielding between
             * polls. A copy that spun without yielding would hold its CPU
             * for a scheduler quantum, and on a machine with as many CPUs
             * as copies the other copy's worker threads (a multithreaded
             * contender's) would wait that quantum to start: measured, a
             * 128 KiB multithreaded hash beside two hard spinners took 2 ms in
             * place of 29 µs. A yielding poll still has the copy in the
             * instruction stream when the release comes, within a
             * microsecond of it.
             */
            self.ready.fetch_add(1, Ordering::AcqRel);
            while self.generation.load(Ordering::Acquire) == seen {
                std::thread::yield_now();
            }
            seen = self.generation.load(Ordering::Acquire);
            let started = sample_clock::now();
            // Sound: run() holds the borrows until both finishes are read.
            run_batch(job.algorithm, unsafe { &*job.inputs[copy] }, job.messages, job.iterations);
            let elapsed_ns = sample_clock::since_ns(started);
            let mut finished = self.finished.lock().unwrap();
            finished[copy] = Some(elapsed_ns);
            self.done.notify_all();
        }
    }
}

/*
 * Apple's CommonCrypto SHA-256, linked from libSystem, through the
 * streaming Init/Update/Final calls: the fastest route into corecrypto.
 * Measured on an M4 Max, a 64-byte digest takes 51 ns this way and 182 ns
 * through the one-shot CC_SHA256(), whose finalisation spends about
 * 110 ns per compression; bulk throughput is identical on both. Keep the
 * three-call form.
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
        /// input here is at most 128 MiB.
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

/*
 * Optional per-sample trace for clock diagnosis: every sample's wall
 * nanoseconds, thread CPU nanoseconds, and (on Apple) mach_absolute_time
 * ticks, with the round, its position in the round, and the contender and
 * size. One CSV line per sample. Off unless --trace-clocks PATH is given;
 * the extra clock reads add about 100 ns to each 1 ms sample.
 */
struct ClockTrace {
    lines: Vec<String>,
    path: std::path::PathBuf,
}

impl ClockTrace {
    fn new(path: std::path::PathBuf) -> Self {
        let mut lines = Vec::with_capacity(8192);
        lines.push("round,position,contender,size_bytes,iterations,wall_ns,thread_cpu_ns,mach_ticks,process_cpu_ns,p_cycles,p_instructions,p_time_ns,e_cycles,e_instructions,e_time_ns".to_owned());
        Self { lines, path }
    }

    fn write(&self) {
        let body = self.lines.join("\n") + "\n";
        fs::write(&self.path, body).unwrap_or_else(|error| {
            panic!("failed to write {}: {error}", self.path.display())
        });
        eprintln!("clock trace: {} samples in {}", self.lines.len() - 1, self.path.display());
    }
}

/*
 * Per-thread cycle and instruction counts, split by CPU performance level,
 * from Apple's thread_selfcounts(THSC_TIME_CPI_PER_PERF_LEVEL). Cycles
 * over time is the clock frequency the thread actually ran at; the split
 * says which cluster ran it. Zero everywhere off Apple, or when the call is
 * unsupported.
 */
#[derive(Clone, Copy, Default)]
struct PerfCounters {
    p_cycles: u64,
    p_instructions: u64,
    p_time_ns: u64,
    e_cycles: u64,
    e_instructions: u64,
    e_time_ns: u64,
}

impl PerfCounters {
    fn since(self, earlier: PerfCounters) -> PerfCounters {
        PerfCounters {
            p_cycles: self.p_cycles - earlier.p_cycles,
            p_instructions: self.p_instructions - earlier.p_instructions,
            p_time_ns: self.p_time_ns - earlier.p_time_ns,
            e_cycles: self.e_cycles - earlier.e_cycles,
            e_instructions: self.e_instructions - earlier.e_instructions,
            e_time_ns: self.e_time_ns - earlier.e_time_ns,
        }
    }
}

/*
 * Reads around each sample. thread_cycles() feeds the cycles-per-byte
 * result on every sample; the rest serve --trace-clocks.
 */
mod trace_clocks {
    use super::PerfCounters;

    /// The calling thread's cycles so far, all core kinds together; zero
    /// where no per-thread counter exists. About 200 ns per call on Apple
    /// silicon, against a 1 ms sample.
    #[inline]
    pub fn thread_cycles() -> u64 {
        let counters = perf_counters();
        counters.p_cycles + counters.e_cycles
    }

    #[cfg(target_vendor = "apple")]
    pub fn perf_counters() -> PerfCounters {
        use std::sync::OnceLock;

        #[repr(C)]
        #[derive(Clone, Copy, Default)]
        struct ThscTimeCpi {
            instructions: u64,
            cycles: u64,
            user_time_mach: u64,
            system_time_mach: u64,
        }
        #[repr(C)]
        struct MachTimebaseInfo {
            numer: u32,
            denom: u32,
        }
        unsafe extern "C" {
            fn thread_selfcounts(kind: u32, dst: *mut std::ffi::c_void, size: usize) -> i32;
            fn mach_timebase_info(info: *mut MachTimebaseInfo) -> i32;
        }
        const THSC_TIME_CPI_PER_PERF_LEVEL: u32 = 4;

        /* (numer, denom) for mach ticks → ns, and whether the call works here. */
        static SETUP: OnceLock<Option<(u64, u64)>> = OnceLock::new();
        let Some((numer, denom)) = *SETUP.get_or_init(|| {
            let mut info = MachTimebaseInfo { numer: 0, denom: 0 };
            if unsafe { mach_timebase_info(&mut info) } != 0 || info.denom == 0 {
                return None;
            }
            let mut probe = [ThscTimeCpi::default(); 2];
            let rc = unsafe {
                thread_selfcounts(
                    THSC_TIME_CPI_PER_PERF_LEVEL,
                    probe.as_mut_ptr().cast(),
                    std::mem::size_of_val(&probe),
                )
            };
            if rc != 0 {
                eprintln!("thread_selfcounts(THSC_TIME_CPI_PER_PERF_LEVEL) unavailable (rc {rc}); cycle columns stay 0");
                return None;
            }
            Some((u64::from(info.numer), u64::from(info.denom)))
        }) else {
            return PerfCounters::default();
        };

        /* hw.nperflevels is 2 on every Apple silicon Mac: index 0 = P, 1 = E. */
        let mut levels = [ThscTimeCpi::default(); 2];
        let rc = unsafe {
            thread_selfcounts(
                THSC_TIME_CPI_PER_PERF_LEVEL,
                levels.as_mut_ptr().cast(),
                std::mem::size_of_val(&levels),
            )
        };
        assert_eq!(rc, 0, "thread_selfcounts failed after succeeding at setup");
        let to_ns = |mach: u64| mach * numer / denom;
        PerfCounters {
            p_cycles: levels[0].cycles,
            p_instructions: levels[0].instructions,
            p_time_ns: to_ns(levels[0].user_time_mach + levels[0].system_time_mach),
            e_cycles: levels[1].cycles,
            e_instructions: levels[1].instructions,
            e_time_ns: to_ns(levels[1].user_time_mach + levels[1].system_time_mach),
        }
    }

    #[cfg(not(target_vendor = "apple"))]
    pub fn perf_counters() -> PerfCounters {
        PerfCounters::default()
    }

    #[cfg(unix)]
    fn clock_ns(clock_id: i32) -> u64 {
        #[repr(C)]
        struct Timespec {
            tv_sec: i64,
            tv_nsec: i64,
        }
        unsafe extern "C" {
            fn clock_gettime(clock_id: i32, tp: *mut Timespec) -> i32;
        }
        let mut ts = Timespec { tv_sec: 0, tv_nsec: 0 };
        let rc = unsafe { clock_gettime(clock_id, &mut ts) };
        assert_eq!(rc, 0, "clock_gettime({clock_id}) failed");
        ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
    }

    #[cfg(target_vendor = "apple")]
    pub fn thread_cpu_ns() -> u64 { clock_ns(16) }
    #[cfg(target_vendor = "apple")]
    pub fn process_cpu_ns() -> u64 { clock_ns(12) }
    #[cfg(all(unix, not(target_vendor = "apple")))]
    pub fn thread_cpu_ns() -> u64 { clock_ns(3) }
    #[cfg(all(unix, not(target_vendor = "apple")))]
    pub fn process_cpu_ns() -> u64 { clock_ns(2) }
    #[cfg(not(unix))]
    pub fn thread_cpu_ns() -> u64 { 0 }
    #[cfg(not(unix))]
    pub fn process_cpu_ns() -> u64 { 0 }

    #[cfg(target_vendor = "apple")]
    pub fn mach_ticks() -> u64 {
        unsafe extern "C" {
            fn mach_absolute_time() -> u64;
        }
        unsafe { mach_absolute_time() }
    }
    #[cfg(not(target_vendor = "apple"))]
    pub fn mach_ticks() -> u64 { 0 }
}

/*
 * The clock for timed samples: the platform's raw hardware counter, read
 * through std::time::Instant. On Darwin that is CLOCK_UPTIME_RAW
 * (mach_absolute_time in nanoseconds); on Linux, CLOCK_MONOTONIC; on
 * Windows, QueryPerformanceCounter. Each is a counter read with no NTP
 * slew, and the Darwin and Linux clocks stop while the machine sleeps, so
 * a suspend mid-run stays out of the samples.
 *
 * A counter read can only over-count when the thread is interrupted, and
 * the median absorbs that while the band reports it. Thread CPU time
 * (CLOCK_THREAD_CPUTIME_ID) is scheduler accounting instead: an
 * interruption mid-sample can leave the slice under-counted, so the sample
 * reports a hash faster than the hardware allows. On an M4 Max three
 * unrelated SHA-256 contenders shared one minimum 12% under their
 * own steady medians. The measure-clocks3 repository demonstrates this.
 */
mod sample_clock {
    use std::time::Instant;

    #[cfg(target_vendor = "apple")]
    pub const NAME: &str = "std::time::Instant → CLOCK_UPTIME_RAW (mach_absolute_time; stops during sleep, no NTP slew)";
    #[cfg(all(unix, not(target_vendor = "apple")))]
    pub const NAME: &str = "std::time::Instant → CLOCK_MONOTONIC (stops during suspend, NTP slew only)";
    #[cfg(windows)]
    pub const NAME: &str = "std::time::Instant → QueryPerformanceCounter";
    #[cfg(not(any(unix, windows)))]
    pub const NAME: &str = "std::time::Instant";

    /// A point on the sample clock. Only differences are meaningful.
    #[inline(always)]
    pub fn now() -> Instant {
        Instant::now()
    }

    /// Elapsed nanoseconds since `start`.
    #[inline(always)]
    pub fn since_ns(start: Instant) -> u64 {
        u64::try_from(start.elapsed().as_nanos()).expect("a sample lasts well under 584 years")
    }
}

/// Whether a cell under the time budget takes a sample this round:
/// in every LONG_EVERY-th round (`slot` is the round plus the cell's own
/// offset), and in every round while its median is not yet known to within
/// LONG_PRECISION_PERMILLE.
fn long_cell_wants_sample(taken: &[u64], slot: usize) -> bool {
    if slot % LONG_EVERY == 0 || taken.len() < LONG_MIN_SAMPLES {
        return true;
    }
    if slot % LONG_EVERY_UNSURE != 0 {
        return false;
    }
    let mut sorted = taken.to_vec();
    sorted.sort_unstable();
    let median = median_of_sorted(&sorted);
    let (low, high) = bootstrap_median_interval(&sorted);
    (high - low) * 1000 > LONG_PRECISION_PERMILLE * median
}

/// (iterations per sample, nanoseconds per iteration measured).
fn calibrate_batch(
    algorithm: Algorithm,
    input: &[u8],
    messages: usize,
) -> (usize, u128) {
    let mut iterations = 1usize;

    loop {
        let started = sample_clock::now();
        run_batch(algorithm, input, messages, iterations);
        let elapsed_ns = u128::from(sample_clock::since_ns(started));

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

            return (scaled as usize, elapsed_ns / iterations as u128);
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

/*
 * Requires a non-empty, ascending slice. For an even count the median is
 * the mean of the two middle values, rounded half up.
 */
fn median_of_sorted(sorted: &[u64]) -> u64 {
    assert!(!sorted.is_empty(), "median requires at least one sample");
    debug_assert!(sorted.windows(2).all(|pair| pair[0] <= pair[1]));

    let middle = sorted.len() / 2;
    if sorted.len() % 2 == 0 {
        (sorted[middle - 1] + sorted[middle] + 1) / 2
    } else {
        sorted[middle]
    }
}

fn summarize_cell(samples: &[Sample], basis: TimeBasis) -> Cell {
    assert!(!samples.is_empty(), "a cell has at least one sample");

    /*
     * The reported time per sample: measured, or cycles per byte at the
     * sustained rate. (millicycles/B × 10⁻³ cycles/millicycle) ÷ (kHz ×
     * 10³ cycles/s) = s/B; × 10¹² ps/s gives ps/B = millicycles/B × 10⁶ / kHz.
     */
    let mut times: Vec<u64> = samples
        .iter()
        .map(|sample| match basis {
            TimeBasis::Measured => sample.ps_per_byte,
            TimeBasis::NormalisedToKhz { khz, .. } => {
                let sample_khz = (sample.cycles * 1_000_000 + sample.elapsed_ns / 2) / sample.elapsed_ns;
                if cycle_count_trustworthy(sample_khz, khz) {
                    (sample.millicycles_per_byte * 1_000_000 + khz / 2) / khz
                } else {
                    sample.ps_per_byte
                }
            }
        })
        .collect();
    assert!(times.iter().all(|&t| t > 0), "all timing samples must be positive");

    Cell { time: summarize(&mut times), duo: None }
}

/// Requires a non-empty slice; sorts it. Zeros summarise to zeros.
fn summarize(samples: &mut [u64]) -> Statistics {
    assert!(!samples.is_empty(), "a cell has at least one sample");
    samples.sort_unstable();

    let median = median_of_sorted(samples);
    let (low, high) = bootstrap_median_interval(samples);

    Statistics {
        count: samples.len(),
        minimum: samples[0],
        low,
        median,
        high,
        maximum: samples[samples.len() - 1],
        modes: find_modes(samples, median),
    }
}

/*
 * 95% percentile-bootstrap interval of the median: resample with
 * replacement BOOTSTRAP_RESAMPLES times, take each resample's median, and
 * report the 2.5th and 97.5th percentiles of those. A fixed-seed
 * SplitMix64 makes the result reproducible run to run for the same samples.
 * Requires a sorted, non-empty slice.
 */
fn bootstrap_median_interval(sorted: &[u64]) -> (u64, u64) {
    let n = sorted.len();
    /* SplitMix64 (Steele, Lea & Flood 2014), seeded by the sample count. */
    let mut state: u64 = n as u64;
    let mut next = || {
        state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    };
    let mut medians = Vec::with_capacity(BOOTSTRAP_RESAMPLES);
    let mut resample = vec![0u64; n];
    for _ in 0..BOOTSTRAP_RESAMPLES {
        for slot in resample.iter_mut() {
            /* The high half of a 64 × 64-bit product: an index in 0..n with
               a bias below n / 2⁶⁴ (Lemire's multiply-shift). */
            *slot = sorted[((u128::from(next()) * n as u128) >> 64) as usize];
        }
        resample.sort_unstable();
        medians.push(median_of_sorted(&resample));
    }
    medians.sort_unstable();
    (
        medians[BOOTSTRAP_RESAMPLES * 25 / 1000],
        medians[BOOTSTRAP_RESAMPLES * 975 / 1000],
    )
}

/*
 * Two clusters, if the sorted samples have a gap of MODE_GAP_PERMILLE of
 * the median between consecutive values with at least MODE_MIN_SHARE on
 * each side. The widest such gap splits them. Requires a sorted slice.
 */
fn find_modes(sorted: &[u64], median: u64) -> Option<Modes> {
    let n = sorted.len();
    let min_side = (n * MODE_MIN_SHARE_PERMILLE).div_ceil(1000).max(1);
    let threshold = median * MODE_GAP_PERMILLE / 1000;
    let mut best: Option<(usize, u64)> = None;
    for split in min_side..=n - min_side {
        let gap = sorted[split] - sorted[split - 1];
        if gap >= threshold && best.is_none_or(|(_, g)| gap > g) {
            best = Some((split, gap));
        }
    }
    best.map(|(split, _)| Modes {
        lower_median: median_of_sorted(&sorted[..split]),
        lower_count: split,
        upper_median: median_of_sorted(&sorted[split..]),
        upper_count: n - split,
    })
}

/*
 * One code path a contender uses for a range of input sizes. `first` is the
 * smallest input in bytes that takes this path; a contender's kernels are
 * listed in ascending order of `first`, the first starting at 0.
 */
#[derive(Clone)]
struct Kernel {
    first: usize,
    /// Short name for the report and the hover panel, e.g. "NEON hash_many".
    name: String,
    /// One sentence on why the path changes here, for the first dot of the kernel.
    why: String,
    /// Mark drawn at every dot in this kernel.
    mark: Mark,
}

/// Dot shapes: the same colour keeps the contender's identity, the shape
/// says which code path produced the point.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mark {
    Circle,
    Diamond,
    Square,
    /// A fourth kernel, which only the multithreaded servil contender has.
    Triangle,
}

impl Mark {
    fn name(self) -> &'static str {
        match self {
            Self::Circle => "circle",
            Self::Diamond => "diamond",
            Self::Square => "square",
            Self::Triangle => "triangle",
        }
    }
}

/*
 * A contender's code paths by input size, with the platform name for the
 * report header. `kernels` is non-empty, ascending in `first`, and starts
 * at 0.
 */
struct Kernels {
    platform: &'static str,
    kernels: Vec<Kernel>,
}

impl Kernels {
    fn new(platform: &'static str, kernels: Vec<Kernel>) -> Self {
        assert!(!kernels.is_empty(), "a contender has at least one kernel");
        assert_eq!(kernels[0].first, 0, "the first kernel covers the smallest inputs");
        assert!(
            kernels.windows(2).all(|pair| pair[0].first < pair[1].first),
            "kernels ascend in their first input size"
        );
        Self { platform, kernels }
    }

    /// Index of the kernel for an input of `bytes` (a batch's bytes in all).
    fn kernel_index_for(&self, bytes: usize) -> usize {
        self.kernels
            .iter()
            .rposition(|kernel| bytes >= kernel.first)
            .expect("the first kernel starts at 0")
    }
}

/*
 * Code paths of the crates.io blake3 crate, from BLAKE3 v1.8.7's
 * src/platform.rs and the SIMD hash_many fallback chains. One chunk is
 * 1024 bytes; hash_many batches whole chunks and hands leftovers below the
 * SIMD degree to the single-chunk path.
 */
fn detect_blake3_kernels() -> Kernels {
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
        let mut kernels = vec![Kernel {
            first: 0,
            name: one.to_owned(),
            why: "Up to one chunk, so a single compression handles the whole input.".to_owned(),
            mark: Mark::Circle,
        }];
        if degree > 4 {
            kernels.push(Kernel {
                first: 4 * 1024,
                name: "SSE4.1 hash_many (4-way fallback)".to_owned(),
                why: "Four whole chunks fill the narrowest SIMD batch; wider batches wait for more chunks.".to_owned(),
                mark: Mark::Diamond,
            });
        }
        if degree > 1 {
            kernels.push(Kernel {
                first: degree * 1024,
                name: wide.to_owned(),
                why: "Enough whole chunks to fill the widest SIMD batch on this CPU.".to_owned(),
                mark: if degree > 4 { Mark::Square } else { Mark::Diamond },
            });
        }
        return Kernels::new(platform, kernels);
    }

    #[cfg(target_arch = "aarch64")]
    {
        return Kernels::new(
            "NEON",
            vec![
                Kernel {
                    first: 0,
                    name: "portable compression".to_owned(),
                    why: "Fewer than four whole chunks: each runs through the portable single-chunk compressor, so 2 KiB and 3 KiB take this path too.".to_owned(),
                    mark: Mark::Circle,
                },
                Kernel {
                    first: 4 * 1024,
                    name: "NEON hash_many (4-way)".to_owned(),
                    why: "Four whole chunks fill a NEON batch; from here the bulk of the input runs four chunks at a time.".to_owned(),
                    mark: Mark::Diamond,
                },
            ],
        );
    }

    #[allow(unreachable_code)]
    Kernels::new(
        "portable",
        vec![Kernel {
            first: 0,
            name: "portable compression".to_owned(),
            why: "This build has no SIMD path; every size runs the portable compressor.".to_owned(),
            mark: Mark::Circle,
        }],
    )
}

/*
 * SHA-256 and SHA-1DC each run one code path at every size. Their kernels
 * still carry a name so the hover panel can say what produced the point.
 */
fn detect_sha256_kernels() -> Kernels {
    let name = if cfg!(target_arch = "aarch64") {
        "sha2 aarch64_sha2 backend (ARMv8 SHA-256 instructions)"
    } else if cfg!(any(target_arch = "x86", target_arch = "x86_64")) {
        "sha2 x86_sha backend (SHA-NI where present)"
    } else {
        "sha2 portable"
    };
    Kernels::new(
        "sha2",
        vec![Kernel {
            first: 0,
            name: name.to_owned(),
            why: "One kernel at every size.".to_owned(),
            mark: Mark::Circle,
        }],
    )
}

fn detect_sha1dc_kernels() -> Kernels {
    Kernels::new(
        "sha1-checked",
        vec![Kernel {
            first: 0,
            name: "SHA-1 with collision detection, pure Rust".to_owned(),
            why: "One kernel at every size.".to_owned(),
            mark: Mark::Circle,
        }],
    )
}

fn detect_common_crypto_kernels() -> Kernels {
    Kernels::new(
        "CommonCrypto",
        vec![Kernel {
            first: 0,
            name: "CC_SHA256_Init/Update/Final (corecrypto, ARMv8 SHA-256 instructions)".to_owned(),
            why: "One kernel at every size.".to_owned(),
            mark: Mark::Circle,
        }],
    )
}

fn detect_ring_kernels() -> Kernels {
    let name = if cfg!(target_arch = "aarch64") {
        "sha256_block_data_order_hw (ARMv8 SHA-256 instructions, pipelined schedule)"
    } else if cfg!(any(target_arch = "x86", target_arch = "x86_64")) {
        "sha256_block_data_order_hw (SHA-NI) or _avx / _ssse3"
    } else {
        "sha256_block_data_order_nohw"
    };
    Kernels::new(
        "ring",
        vec![Kernel {
            first: 0,
            name: name.to_owned(),
            why: "One kernel at every size.".to_owned(),
            mark: Mark::Circle,
        }],
    )
}

/*
 * The servil fork describes its own kernels: kernel_report() and
 * kernel_report_multithreaded() come from the same run-time detection
 * its hash functions use, so the report describes what was measured. The
 * bencher adds only the dot shapes, in order.
 */
fn servil_kernels(report: blake3_servil::KernelReport) -> Kernels {
    const MARKS: [Mark; 4] = [Mark::Circle, Mark::Diamond, Mark::Square, Mark::Triangle];
    assert!(
        report.kernels.len() <= MARKS.len(),
        "the graph has {} dot shapes; the fork reports {} kernels",
        MARKS.len(),
        report.kernels.len(),
    );
    let kernels = report
        .kernels
        .iter()
        .zip(MARKS)
        .map(|(kernel, mark)| Kernel { first: kernel.from_len, name: kernel.name.to_owned(), why: kernel.why.to_owned(), mark })
        .collect();
    Kernels::new(report.platform, kernels)
}

/// The code paths a contender runs in a use case, by the point's bytes.
/// The contender must take part in the use case.
fn detect_kernels(algorithm: Algorithm, use_case: UseCase) -> Kernels {
    assert!(algorithm.takes_part(use_case), "{} takes no part in {use_case:?}", algorithm.name());
    let one_message = match algorithm {
        Algorithm::Blake3 => detect_blake3_kernels(),
        Algorithm::Sha256 => detect_sha256_kernels(),
        Algorithm::Sha1Dc => detect_sha1dc_kernels(),
        Algorithm::Blake3Servil => servil_kernels(blake3_servil::kernel_report()),
        Algorithm::Sha256CommonCrypto => detect_common_crypto_kernels(),
        Algorithm::Sha256Ring => detect_ring_kernels(),
        Algorithm::Blake3Rayon => detect_blake3_rayon_kernels(),
        Algorithm::Blake3ServilMt => servil_kernels(blake3_servil::kernel_report_multithreaded()),
        Algorithm::Blake3ServilMt1 => servil_kernels(blake3_servil::kernel_report()),
        Algorithm::AbBlake3 => detect_ab_blake3_kernels(),
    };
    match use_case {
        UseCase::OneMessage => one_message,
        UseCase::ManyMessages if algorithm == Algorithm::AbBlake3 => detect_ab_blake3_many_kernels(),
        UseCase::ManyMessages if matches!(algorithm, Algorithm::Blake3Servil | Algorithm::Blake3ServilMt1) => {
            servil_kernels(blake3_servil::kernel_report_many())
        }
        UseCase::ManyMessages if algorithm == Algorithm::Blake3ServilMt => {
            servil_kernels(blake3_servil::kernel_report_many_multithreaded())
        }
        UseCase::ManyMessages => {
            /* One call per 64-byte message: the 64 B kernel, whatever the batch size. */
            let kernel = &one_message.kernels[one_message.kernel_index_for(MESSAGE_LEN)];
            Kernels::new(
                one_message.platform,
                vec![Kernel {
                    first: 0,
                    name: format!("{} · one 64 B message per call", kernel.name),
                    why: "Every message is its own call of the one-message entry point; the batch size changes nothing in the code path.".to_owned(),
                    mark: Mark::Circle,
                }],
            )
        }
    }
}

/*
 * ab-blake3's const_hash is a const fn copy of the reference tree, from
 * the crate's const_fn module: portable compression at every size, with
 * no run-time platform detection.
 */
fn detect_ab_blake3_kernels() -> Kernels {
    Kernels::new(
        "portable",
        vec![Kernel {
            first: 0,
            name: "const_hash (const fn reference tree, portable compression)".to_owned(),
            why: "One kernel at every size: a const fn has no run-time SIMD dispatch.".to_owned(),
            mark: Mark::Circle,
        }],
    )
}

/*
 * single_block_hash_many_exact::<N> hands each group of sixteen blocks to
 * the blake3 crate's platform hash_many (the SIMD path detect_blake3_kernels
 * names) and compresses the blocks past the last full group one at a time.
 */
fn detect_ab_blake3_many_kernels() -> Kernels {
    let blake3 = detect_blake3_kernels();
    let wide = &blake3.kernels[blake3.kernels.len() - 1].name;
    Kernels::new(
        blake3.platform,
        vec![
            Kernel {
                first: 0,
                name: "single_block_hash_many_exact::<N>, one compression per block".to_owned(),
                why: "Below sixteen messages the batch entry point compresses each block on its own; the messages queue through one compression function.".to_owned(),
                mark: Mark::Circle,
            },
            Kernel {
                first: 16 * MESSAGE_LEN,
                name: format!("single_block_hash_many_exact::<N>, {wide} per sixteen blocks"),
                why: "From sixteen messages each full group of sixteen blocks goes through the blake3 crate's SIMD hash_many, several blocks per instruction; blocks past the last full group are compressed one at a time.".to_owned(),
                mark: Mark::Diamond,
            },
        ],
    )
}

/*
 * update_rayon splits the tree with rayon::join down to the SIMD degree,
 * so any input above one SIMD width of chunks may cross threads; the
 * kernel boundary is where the crate's serial path ends.
 */
fn detect_blake3_rayon_kernels() -> Kernels {
    let single = detect_blake3_kernels();
    let degree_bytes = single.kernels.last().map(|r| r.first).unwrap_or(0).max(2 * 1024);
    Kernels::new(
        single.platform,
        vec![
            Kernel {
                first: 0,
                name: "caller's thread (below one SIMD width of chunks)".to_owned(),
                why: "One SIMD width of chunks or less is one hash_many call; update_rayon has nothing to split.".to_owned(),
                mark: Mark::Circle,
            },
            Kernel {
                first: 2 * degree_bytes,
                name: "rayon::join over the pool".to_owned(),
                why: "Above one SIMD width of chunks the tree splits recursively with rayon::join, and idle pool threads steal the halves.".to_owned(),
                mark: Mark::Diamond,
            },
        ],
    )
}

/// The kernels a contender ran in a use case, by point: one line for a
/// contender with a single kernel, a table for one that changes kernel
/// along the axis.
fn append_kernel_report(output: &mut String, algorithm: Algorithm, use_case: UseCase) {
    let kernels = detect_kernels(algorithm, use_case);

    if kernels.kernels.len() == 1 {
        writeln!(
            output,
            "{} kernel: {} (platform {})",
            algorithm.name(),
            kernels.kernels[0].name,
            kernels.platform,
        )
        .unwrap();
        return;
    }

    writeln!(output, "{} kernels by {} (platform {}):", algorithm.name(), use_case.column(), kernels.platform).unwrap();
    let mut previous = None;
    for point in &POINTS[use_case.points()] {
        let kernel_index = kernels.kernel_index_for(point.bytes);
        let kernel = &kernels.kernels[kernel_index];
        let new_here = previous.is_some_and(|previous| previous != kernel_index);
        previous = Some(kernel_index);
        writeln!(
            output,
            "  {:>8}: {}{}",
            point.label,
            kernel.name,
            if new_here { "  ← new kernel from here" } else { "" },
        )
        .unwrap();
    }
}

/*
 * Every duo sample, for tools that do their own statistics (the fork's
 * performance-regression check reads this file). Lines starting with '#'
 * carry the provenance and machine identity as `key: value`; then one row
 * per measured cell: contender key, use case, point label, the unit a
 * sample is per (`B` or `msg`), and the samples in picoseconds per unit,
 * comma-separated, in the order taken.
 */
fn generate_samples_tsv(roster: &Roster, duo_samples: &DuoSamples, machine: &MachineMetadata, selection_note: &str) -> String {
    let mut out = String::new();
    writeln!(out, "# bench-hashes samples v1").unwrap();
    for (key, value) in [
        ("timestamp", machine.timestamp.as_str()),
        ("bench-hashes version", BENCH_VERSION),
        ("git commit", GIT_COMMIT),
        ("git clean status", GIT_CLEAN_STATUS),
        ("blake3-servil source", BLAKE3_SERVIL_SOURCE_INFO),
        ("blake3 source", BLAKE3_SOURCE_INFO),
        ("sha256 source", SHA2_SOURCE_INFO),
        ("cpu type", machine.cpu_type.as_str()),
        ("cpu count", machine.cpu_count.to_string().as_str()),
        ("os type", machine.os_type.as_str()),
        ("cpu identity", machine.cpu_identity.as_str()),
        ("rust compiler", RUSTC_VERSION),
        ("build target", BUILD_TARGET),
        ("target features", TARGET_FEATURES),
        ("sample clock", sample_clock::NAME),
        ("contenders", selection_note),
        ("rounds", roster.rounds.to_string().as_str()),
        ("points", roster.points.iter().map(|&index| POINTS[index].label).collect::<Vec<_>>().join(",").as_str()),
    ] {
        writeln!(out, "# {key}: {value}").unwrap();
    }
    for &algorithm in &roster.algorithms {
        for use_case in UseCase::ALL.iter().filter(|&&use_case| algorithm.takes_part(use_case)) {
            writeln!(out, "# kernel platform {} {:?}: {}", algorithm.key(), use_case, detect_kernels(algorithm, *use_case).platform).unwrap();
        }
    }
    writeln!(out, "contender\tuse_case\tpoint\tunit\tps_per_unit").unwrap();
    for (algorithm_index, &algorithm) in roster.algorithms.iter().enumerate() {
        for (point_index, point) in POINTS.iter().enumerate() {
            let samples = &duo_samples[algorithm_index][point_index];
            if samples.is_empty() {
                continue;
            }
            let unit = match point.use_case {
                UseCase::OneMessage => "B",
                UseCase::ManyMessages => "msg",
            };
            let values: Vec<String> = samples.iter().map(u64::to_string).collect();
            writeln!(out, "{}\t{:?}\t{}\t{unit}\t{}", algorithm.key(), point.use_case, point.label, values.join(",")).unwrap();
        }
    }
    out
}

fn generate_text(
    roster: &Roster,
    results: &Results,
    machine: &MachineMetadata,
    selection_note: &str,
    basis: TimeBasis,
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
    writeln!(output, "CPU identity: {}", machine.cpu_identity).unwrap();
    writeln!(output, "Rust compiler: {RUSTC_VERSION}").unwrap();
    writeln!(output, "Build target: {BUILD_TARGET}").unwrap();
    writeln!(output, "Target features: {TARGET_FEATURES}").unwrap();
    writeln!(output, "Sample clock: {}", sample_clock::NAME).unwrap();
    writeln!(output, "Reported time: {}", basis.describe()).unwrap();
    writeln!(output, "Correctness: selected implementations match checked-in golden digests on identical deterministic inputs before calibration; checks cover both timed input sets, boundary lengths, and concurrent multithreaded calls.").unwrap();
    writeln!(output, "Contenders: {}", selection_note).unwrap();
    for algorithm in &roster.algorithms {
        writeln!(output, "{} source: {}", algorithm.name(), algorithm.source()).unwrap();
    }
    for algorithm in &roster.algorithms {
        writeln!(output, "{} mode: {}", algorithm.name(), algorithm.mode()).unwrap();
    }
    for algorithm in &roster.algorithms {
        if let Some(resources) = algorithm.thread_resources() {
            writeln!(output, "{} threads: {}", algorithm.name(), resources).unwrap();
        }
    }
    if roster.solo {
        writeln!(
            output,
            "Measurement: solo and duo. Every sample interval took a solo sample (one copy, one thread) and then a duo sample: two independent copies of the contender at once, on two threads over two inputs of the size, released together, timed to the later finish, per byte of one copy. A contender that takes the whole machine to go faster alone runs beside a copy of itself in the duo sample and shows what that costs. Duo times are measured time; solo times follow the reported-time rule above."
        )
        .unwrap();
    } else {
        writeln!(
            output,
            "Measurement: duo. Every sample ran two independent copies of the contender at once, on two threads over two inputs of the size, released together, timed to the later finish, per byte of one copy. A contender that takes the whole machine to go faster alone runs beside a copy of itself and shows what that costs. Duo times are measured time. Pass --solo to also take a solo sample beside each duo sample."
        )
        .unwrap();
    }
    writeln!(
        output,
        "Use cases: one message per call, at twenty-four input sizes from 64 B to 128 MiB; and many messages per call, a batch of {MESSAGE_LEN}-byte messages at twenty-four batch sizes from 1 to 262144. In the second, a contender with a batch entry point takes the batch as one call (ab-blake3's single_block_hash_many_exact::<N>, BLAKE3 servil's hash_many, BLAKE3 servil mt's hash_many_multithreaded); every other contender hashes the batch one message per call of its plain entry point; BLAKE3 mt takes no part in it."
    )
    .unwrap();
    writeln!(
        output,
        "Samples: {} rounds. A cell whose single hash takes {} ms or more is sampled in every {}th round, and in every {}nd while the 95% interval of its median is wider than {}% of it; a third row gives the sample counts, in brackets, wherever a cell took fewer.",
        roster.rounds,
        LONG_HASH_NS / 1_000_000,
        LONG_EVERY,
        LONG_EVERY_UNSURE,
        LONG_PRECISION_PERMILLE / 10,
    )
    .unwrap();
    writeln!(output).unwrap();

    for use_case in UseCase::ALL {
        writeln!(output, "{} — kernels:", use_case.heading()).unwrap();
        for &algorithm in roster.algorithms.iter().filter(|algorithm| algorithm.takes_part(use_case)) {
            append_kernel_report(&mut output, algorithm, use_case);
        }
        writeln!(output).unwrap();
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
        "Time per byte in ns/B for one message per call, time per message in ns for many messages per call: median, with minimum–maximum beneath; lower is better. Bands in the graph are the 95% interval of each median."
    )
        .unwrap();
    if roster.solo {
        writeln!(
            output,
            "Each contender has two columns. solo: one copy on one thread, the machine otherwise idle. duo: two independent copies at once on two threads, the time to the later finish, per byte of one copy. A contender that takes the whole machine to go faster alone shows the difference between the two."
        )
        .unwrap();
    } else {
        writeln!(
            output,
            "Each contender has one column: duo, two independent copies at once on two threads, the time to the later finish, per byte of one copy."
        )
        .unwrap();
    }
    writeln!(output).unwrap();

    let columns = |cell: &Cell| -> Vec<Statistics> {
        let mut columns = vec![cell.time];
        if let Some(duo) = cell.duo {
            columns.push(duo);
        }
        columns
    };

    for use_case in UseCase::ALL {
        let contenders: Vec<usize> = (0..roster.len())
            .filter(|&index| roster.algorithms[index].takes_part(use_case))
            .collect();
        writeln!(output, "{} ({}):", use_case.heading(), use_case.time_unit()).unwrap();
        writeln!(output).unwrap();

        /*
         * Header rows: one column per contender, or a solo and a duo column
         * per contender in a duo run; wide names get a short form.
         */
        write!(output, "  {:<8}", use_case.column()).unwrap();
        for &algorithm_index in &contenders {
            let algorithm = roster.algorithms[algorithm_index];
            if roster.solo {
                write!(output, "  {:>27}", column_heading(algorithm)).unwrap();
            } else {
                write!(output, "  {:>13}", column_heading(algorithm)).unwrap();
            }
        }
        writeln!(output).unwrap();
        if roster.solo {
            write!(output, "  {:<8}", "").unwrap();
            for _ in &contenders {
                write!(output, "  {:>13}{:>14}", "solo", "duo").unwrap();
            }
            writeln!(output).unwrap();
        }

        for point_index in use_case.points().filter(|&index| roster.measures(index)) {
            write!(output, "  {:<8}", POINTS[point_index].label).unwrap();
            for &algorithm_index in &contenders {
                for statistics in columns(cell(results, algorithm_index, point_index)) {
                    write!(output, "  {:>13}", format_ps(statistics.median)).unwrap();
                }
            }
            writeln!(output).unwrap();

            write!(output, "  {:<8}", "").unwrap();
            for &algorithm_index in &contenders {
                for statistics in columns(cell(results, algorithm_index, point_index)) {
                    /* A trailing mark flags a wide spread; the legend below explains it. */
                    let flag = if spread_permille(statistics) >= SPREAD_WIDE_PERMILLE { "!" } else { " " };
                    write!(
                        output,
                        "  {:>12}{flag}",
                        format!("{}–{}", format_ps(statistics.minimum), format_ps(statistics.maximum)),
                    )
                        .unwrap();
                }
            }
            writeln!(output).unwrap();

            /* Cells the time budget sampled less often say how many samples they took. */
            let counts: Vec<usize> = contenders
                .iter()
                .flat_map(|&algorithm_index| columns(cell(results, algorithm_index, point_index)))
                .map(|statistics| statistics.count)
                .collect();
            if counts.iter().any(|&count| count < roster.rounds) {
                write!(output, "  {:<8}", "").unwrap();
                for count in counts {
                    write!(output, "  {:>13}", format!("[{count}]")).unwrap();
                }
                writeln!(output).unwrap();
            }
        }
        writeln!(output).unwrap();
    }

    let all_statistics: Vec<Statistics> = results.iter().flatten().flatten().flat_map(|cell| columns(cell)).collect();
    let wide_cells = all_statistics
        .iter()
        .filter(|statistics| spread_permille(**statistics) >= SPREAD_WIDE_PERMILLE)
        .count();
    writeln!(output).unwrap();
    if wide_cells > 0 {
        writeln!(
            output,
            "!  marks a cell whose 95% median interval is at least {}% of its median: {wide_cells} of {} cells; those medians are poorly determined.",
            SPREAD_WIDE_PERMILLE / 10,
            all_statistics.len(),
        )
            .unwrap();
    } else {
        writeln!(
            output,
            "Every cell's median is known to within {}% (95% bootstrap interval).",
            SPREAD_WIDE_PERMILLE / 10,
        )
            .unwrap();
    }
    writeln!(output).unwrap();

    output
}

/// Column heading that fits the 13-character summary columns.
fn column_heading(algorithm: Algorithm) -> &'static str {
    match algorithm {
        Algorithm::Sha256CommonCrypto => "SHA-256 CC",
        Algorithm::Sha256Ring => "SHA-256 ring",
        Algorithm::Blake3ServilMt => "B3 servil mt",
        Algorithm::Blake3ServilMt1 => "B3 servil mt·1",
        other => other.name(),
    }
}

/// A point's x position on its use case's axis, as a fraction of the
/// axis width; both axes are logarithmic in bytes.
fn x_fraction(point_index: usize) -> f64 {
    let range = POINTS[point_index].use_case.points();
    let smallest = (POINTS[range.start].bytes as f64).log2();
    let largest = (POINTS[range.end - 1].bytes as f64).log2();
    ((POINTS[point_index].bytes as f64).log2() - smallest) / (largest - smallest)
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
        cpu_identity: cpu_identity(),
    }
}

/*
 * The CPU's identity for comparing runs across machines. Linux: the first
 * processor's implementer, variant, part, and revision (a hypervisor may
 * hide the part), its feature list (which pins an ARM core's generation),
 * and the machine model the device tree or DMI names (a VM says so). macOS:
 * the brand string and the core count of each performance level. Empty
 * where neither source exists.
 */
fn cpu_identity() -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Ok(cpuinfo) = fs::read_to_string("/proc/cpuinfo") {
        let first = cpuinfo.split("\n\n").next().unwrap_or("");
        for key in ["CPU implementer", "CPU variant", "CPU part", "CPU revision", "model name", "Features", "flags"] {
            if let Some(line) = first.lines().find(|line| line.split(':').next().is_some_and(|k| k.trim() == key)) {
                parts.push(format!("{key}: {}", line.split_once(':').unwrap().1.trim()));
            }
        }
        for path in ["/sys/firmware/devicetree/base/compatible", "/sys/class/dmi/id/product_name"] {
            if let Ok(model) = fs::read(path) {
                let model = String::from_utf8_lossy(&model).replace('\0', " ").trim().to_owned();
                if !model.is_empty() {
                    parts.push(format!("machine: {model}"));
                }
            }
        }
    }
    if cfg!(target_os = "macos") {
        for key in ["machdep.cpu.brand_string", "hw.perflevel0.physicalcpu", "hw.perflevel1.physicalcpu"] {
            if let Ok(out) = std::process::Command::new("sysctl").args(["-n", key]).output() {
                let value = String::from_utf8_lossy(&out.stdout).trim().to_owned();
                if out.status.success() && !value.is_empty() {
                    parts.push(format!("{key}: {value}"));
                }
            }
        }
    }
    parts.join(" · ")
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

/// Picoseconds per byte as an f64 of nanoseconds, for the SVG's log axis only.
fn ps_to_ns(ps: PsPerByte) -> f64 {
    ps as f64 / PS_PER_NS as f64
}

/// Picoseconds per byte as nanoseconds with three decimals: 437 → "0.437".
fn format_ps(ps: PsPerByte) -> String {
    format!("{}.{:03}", ps / PS_PER_NS, ps % PS_PER_NS)
}

/*
 * Rate from picoseconds per unit, with the use case's unit: 1 B/ps =
 * 1000 GB/s, so GB/s = 1000 / ps; 1 msg/ps = 10⁶ Mmsg/s, so Mmsg/s =
 * 10⁶ / ps. Shown to one decimal below 10, whole numbers above.
 */
fn format_rate(ps: PsPerByte, use_case: UseCase) -> String {
    assert!(ps > 0);
    /* tenths of the rate unit, rounded */
    let tenths = (10_000 * use_case.rate_scale() + ps / 2) / ps;
    if tenths >= 100 {
        format!("{} {}", (tenths + 5) / 10, use_case.rate_unit())
    } else {
        format!("{}.{} {}", tenths / 10, tenths % 10, use_case.rate_unit())
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
 *
 * Two plots stack down the canvas, one per use case, sharing the x extent,
 * the unit switch, and the contender toggles at right.
 */
const SVG_WIDTH: f64 = 1300.0;
/// Canvas height: the provenance block ends where the lines end, with the
/// bottom margin that follows.
fn svg_height(provenance_lines: usize) -> f64 {
    provenance_line_y(provenance_lines.saturating_sub(1)) + PROVENANCE_LINE_HEIGHT + 8.0
}
const PLOT_LEFT: f64 = 110.0;
const PLOT_RIGHT: f64 = 1000.0;
/// The first plot's top, below the title, two method lines, and its own
/// heading; each further plot sits PLOT_PITCH lower.
const PLOT_TOP: f64 = 150.0;
const PLOT_HEIGHT: f64 = 340.0;
/// Room under a plot for its x labels, axis title, and shape legend, and
/// above the next for its heading.
const PLOT_PITCH: f64 = 470.0;
const X_INSET: f64 = 40.0;
const SERIES_LABEL_GAP: f64 = 44.0;
/// Fixed shape slots before each right-hand name, so names align across
/// contenders with different shape counts. Four covers every contender.
const SWATCH_SLOTS: usize = 4;
const PROVENANCE_TOP: f64 = PLOT_TOP + (UseCase::ALL.len() - 1) as f64 * PLOT_PITCH + PLOT_HEIGHT + 85.0;
const PROVENANCE_LINE_HEIGHT: f64 = 14.0;

fn plot_top(plot_index: usize) -> f64 {
    PLOT_TOP + plot_index as f64 * PLOT_PITCH
}

fn plot_bottom(plot_index: usize) -> f64 {
    plot_top(plot_index) + PLOT_HEIGHT
}

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

/*
 * One plot's geometry: its use case, the points along its x axis, the
 * contenders that take part, its y axis in GB/s, and where each
 * contender's right-hand label sits.
 */
struct Plot {
    index: usize,
    use_case: UseCase,
    points: std::ops::Range<usize>,
    /// Roster indices of the contenders measured in this use case.
    contenders: Vec<usize>,
    top: f64,
    bottom: f64,
    log_min: f64,
    log_max: f64,
    axis_min: f64,
    axis_max: f64,
    /// Rate per unit time: rate = scale / (ns per unit). 1 for GB/s from
    /// ns/B, 1000 for million messages per second from ns/message.
    scale: f64,
    /// Pixel x per point of this plot, in axis order.
    x_positions: Vec<f64>,
    /// Pixel y of each contender's label at right; None for a contender
    /// that takes no part here.
    label_y: Vec<Option<f64>>,
    /// Each contender's kernels in this use case; None for a non-participant.
    kernels: Vec<Option<Kernels>>,
}

impl Plot {
    fn new(index: usize, use_case: UseCase, roster: &Roster, results: &Results) -> Self {
        let points = use_case.points();
        let contenders: Vec<usize> = (0..roster.len())
            .filter(|&algorithm_index| roster.algorithms[algorithm_index].takes_part(use_case))
            .collect();
        assert!(!contenders.is_empty(), "a plot has at least one contender");

        /*
         * From here down the SVG needs pixel positions on a log axis, which
         * is where floating point earns its place: the values are drawn,
         * never compared or reported. Everything above this point is integer.
         */
        let cells = || contenders.iter().flat_map(|&a| points.clone().map(move |s| (a, s))).map(|(a, s)| cell(results, a, s));
        let observed_max = cells()
            .map(|cell| cell.time.high.max(cell.duo.map_or(0, |duo| duo.high)))
            .max()
            .expect("there are results");
        let observed_min = cells()
            .map(|cell| cell.time.low.min(cell.duo.map_or(u64::MAX, |duo| duo.low)))
            .min()
            .expect("there are results");

        /*
         * The static render shows gigabytes per second, the default unit:
         * the axis spans the reciprocals of the observed times, so on the
         * log axis the plot mirrors a time-per-byte one, fastest at the
         * top. The script rebuilds all of this when the unit flips.
         */
        let scale = use_case.rate_scale() as f64;
        let observed_lo_rate = scale / ps_to_ns(observed_max);
        let observed_hi_rate = scale / ps_to_ns(observed_min);
        let (axis_min, axis_max) = log_axis_bounds(observed_lo_rate, observed_hi_rate);

        let x_positions: Vec<f64> = points
            .clone()
            .map(|point_index| PLOT_LEFT + X_INSET + x_fraction(point_index) * (PLOT_RIGHT - PLOT_LEFT - 2.0 * X_INSET))
            .collect();

        let kernels: Vec<Option<Kernels>> = roster
            .algorithms
            .iter()
            .map(|&algorithm| algorithm.takes_part(use_case).then(|| detect_kernels(algorithm, use_case)))
            .collect();

        let mut plot = Self {
            index,
            use_case,
            points: points.clone(),
            contenders: contenders.clone(),
            top: plot_top(index),
            bottom: plot_bottom(index),
            log_min: axis_min.ln(),
            log_max: axis_max.ln(),
            axis_min,
            axis_max,
            scale,
            x_positions,
            label_y: vec![None; roster.len()],
            kernels,
        };

        /*
         * Right-edge series labels double as toggles. Each sits level with
         * its line's last point, pushed apart when medians nearly coincide.
         * The script repeats this rule after each toggle; a hidden
         * contender keeps its slot, anchored where its line would end on
         * the current axis and clamped to the plot edge, so its grey label
         * points toward its data.
         */
        let last = points.end - 1;
        let mut label_slots: Vec<(usize, f64)> = contenders
            .iter()
            .map(|&algorithm_index| (algorithm_index, plot.map_y(cell(results, algorithm_index, last).time.median)))
            .collect();
        label_slots.sort_by(|a, b| a.1.total_cmp(&b.1));
        for index in 1..label_slots.len() {
            let minimum_y = label_slots[index - 1].1 + SERIES_LABEL_GAP;
            if label_slots[index].1 < minimum_y {
                label_slots[index].1 = minimum_y;
            }
        }
        /*
         * Keep the stack inside the plot: with many contenders the
         * pushed-apart labels can run past the bottom axis, so the whole
         * stack shifts up by the overrun. The script applies the same rule.
         */
        let overrun = (label_slots.last().map(|slot| slot.1).unwrap_or(0.0) + 20.0 - plot.bottom).max(0.0);
        for (algorithm_index, label_y) in label_slots {
            plot.label_y[algorithm_index] = Some(label_y - overrun);
        }
        plot
    }

    /// Pixel y for a rate (GB/s, or million messages per second) on the log axis.
    fn map_rate(&self, value: f64) -> f64 {
        assert!(value > 0.0);
        self.bottom - (value.ln() - self.log_min) / (self.log_max - self.log_min) * (self.bottom - self.top)
    }

    /// Pixel y for a measured value.
    fn map_y(&self, ps: PsPerByte) -> f64 {
        assert!(ps > 0);
        self.map_rate(self.scale / ps_to_ns(ps))
    }

    fn kernels(&self, algorithm_index: usize) -> &Kernels {
        self.kernels[algorithm_index].as_ref().expect("the contender takes part in this plot")
    }

    /// The point count along this plot's axis.
    fn len(&self) -> usize {
        self.points.len()
    }
}

fn generate_svg(
    roster: &Roster,
    results: &Results,
    machine: &MachineMetadata,
    selection_note: &str,
    basis: TimeBasis,
) -> String {
    assert!(roster.len() >= 2, "a graph compares at least two contenders");

    let plots: Vec<Plot> = UseCase::ALL
        .iter()
        .enumerate()
        .map(|(index, &use_case)| Plot::new(index, use_case, roster, results))
        .collect();

    let provenance_cats = shared_provenance_cats(machine, selection_note, basis);
    let provenance_total = provenance_cats.len()
        + provenance_cats.iter().map(|cat| cat.lines.len()).sum::<usize>()
        + roster
            .algorithms
            .iter()
            .enumerate()
            .map(|(algorithm_index, &algorithm)| contender_provenance_lines(algorithm, plots[0].kernels(algorithm_index)).len())
            .sum::<usize>();
    let svg_height = svg_height(provenance_total);

    let mut svg = String::new();

    writeln!(svg, r##"<?xml version="1.0" encoding="UTF-8"?>"##)
        .unwrap();

    writeln!(
        svg,
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {SVG_WIDTH:.0} {svg_height:.0}" width="{SVG_WIDTH:.0}" height="{svg_height:.0}" onclick="tapAway()">"##
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
    .method { font-size: 11px; fill: #8a8a8a; }
    .plot-title { font-size: 14px; font-weight: 700; fill: #333333; }
    .plot-sub { font-size: 11px; fill: #8a8a8a; }
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
    .prov-head-row { cursor: pointer; }
    .prov { font-size: 9px; fill: #9a9a9a; }
    .grid { stroke: #e8e8e6; stroke-width: 1; }
    .grid-x { stroke: #f0f0ee; stroke-width: 1; }
    .axis { stroke: #55555a; stroke-width: 1; }
    .divider { stroke: #e0e0de; stroke-width: 1; }
    .series-label { cursor: pointer; transition: transform 0.3s ease; }
    .series-hint { display: none; }
    .marks, .dots { transition: opacity 0.3s ease; }
    .marks { pointer-events: none; }
    .series[data-on="false"] .marks, .dots[data-on="false"] { opacity: 0; pointer-events: none; }
    .series[data-on="false"] .series-name { fill: #9a9a9a; }
    .series[data-on="false"] .series-detail { display: none; }
    .series[data-on="false"] .series-hint { display: inline; }
    .series[data-on="false"] .series-prov { display: none; }
    .series[data-on="false"] .series-swatch * { fill: #fdfdfc; }
    .series { transition: opacity 0.15s ease; }
    .series[data-dim="true"], .dots[data-dim="true"] { opacity: 0.25; }
    .series[data-dim="true"] .series-name { fill: #b5b5b5; }
    .series[data-hl="true"] .series-name { text-decoration: underline; }
    .series-swatch { stroke-width: 2; transition: fill 0.3s ease; }
    #unit-switch { cursor: pointer; }
    .unit-track { fill: #e8e8e6; stroke: #c8c8c4; stroke-width: 1; }
    @media (hover: hover) {
      .prov-head-row:hover .prov-head { text-decoration: underline; }
      .series-label:hover .series-name { text-decoration: underline; }
      #unit-switch:hover .unit-track { stroke: #55555a; }
    }
    .unit-knob { fill: #55555a; transition: cy 0.35s ease; }
    .unit-label { font-size: 10px; font-weight: 600; fill: #b0b0b0; transition: fill 0.35s ease; }
    .unit-label.unit-on { fill: #333333; }
    .dot { cursor: crosshair; }
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
        r##"  <text x="{PLOT_LEFT:.0}" y="44" class="title">Cryptographic Hash Performance</text>"##
    )
        .unwrap();

    if roster.solo {
        writeln!(
            svg,
            r##"  <text x="{PLOT_LEFT:.0}" y="72" class="method">Solid line and dot: solo median of up to {} interleaved samples · dashed line: duo median (two copies at once, later finish) · shaded band: 95% confidence interval of that median; a deeper tint marks a median that is less certain</text>"##,
            roster.rounds,
        )
        .unwrap();
    } else {
        writeln!(
            svg,
            r##"  <text x="{PLOT_LEFT:.0}" y="72" class="method">Line and dot: median of up to {} interleaved duo samples (two copies at once, later finish) · shaded band: 95% confidence interval of that median; a deeper tint marks a median that is less certain</text>"##,
            roster.rounds,
        )
        .unwrap();
    }
    writeln!(
        svg,
        r##"  <text x="{PLOT_LEFT:.0}" y="88" class="method">Dot shape marks the code path a contender used at that point · hover or tap a dot to compare at that point · hover a name to highlight its contender · click a name at right to hide or show it in both plots</text>"##
    )
        .unwrap();

    /*
     * Unit switch, above the first y axis: a vertical track with a knob
     * that slides between GB/s (top) and ns/B (bottom). Clicking anywhere
     * on the switch flips both plots. The knob's position is the state;
     * the label beside it reads darker. Without script the graph stays in
     * GB/s and the switch is inert.
     */
    writeln!(
        svg,
        r##"  <g id="unit-switch" transform="translate({:.1} {:.1})" onclick="event.stopPropagation(); flipUnit()">"##,
        PLOT_LEFT - 60.0,
        PLOT_TOP - 50.0,
    )
        .unwrap();
    writeln!(svg, r##"    <title>Switch both plots between rate (GB/s, million messages per second) and time (ns per byte, ns per message)</title>"##).unwrap();
    writeln!(svg, r##"    <rect class="unit-hit" x="-4" y="-4" width="60" height="42" fill="transparent"/>"##).unwrap();
    writeln!(svg, r##"    <rect class="unit-track" x="0" y="0" width="14" height="34" rx="7"/>"##).unwrap();
    writeln!(svg, r##"    <circle id="unit-knob" class="unit-knob" cx="7" cy="7" r="5"/>"##).unwrap();
    writeln!(svg, r##"    <text class="unit-label unit-on" data-unit="gbps" x="20" y="11">rate</text>"##).unwrap();
    writeln!(svg, r##"    <text class="unit-label" data-unit="ns" x="20" y="31">time</text>"##).unwrap();
    writeln!(svg, "  </g>").unwrap();

    /*
     * Headers occupy the first provenance slots, then every category's
     * detail lines; the per-contender lines emitted inside the first plot's
     * series groups start after them. The script flows detail lines from
     * the header count when categories collapse.
     */
    let shared_count = provenance_cats.len();
    let mut provenance_slot = shared_count
        + provenance_cats.iter().map(|cat| cat.lines.len()).sum::<usize>();

    for plot in &plots {
        write_plot(&mut svg, plot, roster, results, &mut provenance_slot);
    }

    /*
     * Hover panel, filled by the script when a dot is hovered. Last among
     * the drawn elements so it paints over every series.
     */
    writeln!(svg, r##"  <g id="hover" style="display:none">"##).unwrap();
    writeln!(
        svg,
        r##"    <line id="hover-guide" x1="0" y1="{:.1}" x2="0" y2="{:.1}"/>"##,
        plots[0].top,
        plots[0].bottom,
    )
        .unwrap();
    writeln!(svg, r##"    <rect id="hover-box" x="0" y="0" width="0" height="0" rx="4"/>"##).unwrap();
    writeln!(svg, r##"    <g id="hover-body"></g>"##).unwrap();
    writeln!(svg, "  </g>").unwrap();

    /* Machine-readable provenance, complete and untruncated. */
    writeln!(svg, "  <metadata>").unwrap();

    let basis_description = basis.describe();
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
        ("sample clock", sample_clock::NAME),
        ("reported time", basis_description.as_str()),
        ("BLAKE3 source", BLAKE3_SOURCE_INFO),
        ("SHA-256 source", SHA2_SOURCE_INFO),
        ("SHA-1DC source", SHA1_CHECKED_SOURCE_INFO),
        ("SHA-256 ring source", RING_SOURCE_INFO),
        ("BLAKE3 servil source", BLAKE3_SERVIL_SOURCE_INFO),
        ("ab-blake3 source", AB_BLAKE3_SOURCE_INFO),
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

    /* Collapsible categories: a header row per category, then its detail
       lines. Without script everything shows, fully laid out. */
    let mut header_slot = 0;
    for cat in &provenance_cats {
        writeln!(
            svg,
            r##"  <g class="prov-head-row" data-cat="{}" data-name="{}" data-summary="{}" onclick="event.stopPropagation(); toggleProv('{}')">"##,
            cat.key,
            xml_escape(cat.name),
            xml_escape(&cat.summary),
            cat.key,
        )
        .unwrap();
        writeln!(
            svg,
            r##"    <text class="prov-head" x="{PLOT_LEFT:.1}" y="{:.1}">▾ {} — {}</text>"##,
            provenance_line_y(header_slot),
            xml_escape(cat.name),
            xml_escape(&cat.summary),
        )
        .unwrap();
        writeln!(svg, r##"  </g>"##).unwrap();
        header_slot += 1;
        for line in &cat.lines {
            writeln!(
                svg,
                r##"  <text class="prov prov-shared" data-cat="{}" x="{PLOT_LEFT:.1}" y="{:.1}">{}</text>"##,
                cat.key,
                provenance_line_y(header_slot),
                xml_escape(line),
            )
            .unwrap();
            header_slot += 1;
        }
    }

    assert_eq!(
        provenance_slot, provenance_total,
        "the provenance lines emitted must match the count the canvas was sized for"
    );

    /* Data and behaviour for the interactive toggles. */
    write_interaction_script(&mut svg, roster, results, &plots, shared_count);

    svg.push_str("</svg>\n");
    svg
}

/*
 * One plot: heading, axes, every participating contender's band, line,
 * dots, value labels, and clickable label at right, and the shape legend
 * beneath. The first plot's series groups also carry each contender's
 * provenance lines, which hide with the contender.
 */
fn write_plot(svg: &mut String, plot: &Plot, roster: &Roster, results: &Results, provenance_slot: &mut usize) {
    let p = plot.index;
    let top = plot.top;
    let bottom = plot.bottom;

    let heading_note = match plot.use_case {
        UseCase::OneMessage => "each call hashes one input of the size".to_owned(),
        UseCase::ManyMessages => format!(
            "each call hashes a batch of {MESSAGE_LEN}-byte messages: a call per message, or one call per batch where a crate offers it (ab-blake3, BLAKE3 servil, servil mt) · BLAKE3 mt sits out",
        ),
    };
    writeln!(
        svg,
        r##"  <text x="{PLOT_LEFT:.0}" y="{:.1}" class="plot-title">{}</text>"##,
        top - 26.0,
        xml_escape(plot.use_case.heading()),
    )
    .unwrap();
    writeln!(
        svg,
        r##"  <text x="{PLOT_LEFT:.0}" y="{:.1}" class="plot-sub">{}</text>"##,
        top - 11.0,
        xml_escape(&heading_note),
    )
    .unwrap();

    /* Horizontal grid and y-axis tick labels; the script rebuilds these. */
    writeln!(svg, r##"  <g id="y-axis-{p}">"##).unwrap();
    for value in log_ticks(plot.axis_min, plot.axis_max) {
        let y = plot.map_rate(value);
        let ns = plot.scale / value;
        writeln!(
            svg,
            r##"    <line x1="{PLOT_LEFT:.1}" y1="{y:.2}" x2="{PLOT_RIGHT:.1}" y2="{y:.2}" class="grid" data-ns="{ns}"/>"##
        )
            .unwrap();

        writeln!(
            svg,
            r##"    <text x="{:.1}" y="{:.2}" class="tick-label" text-anchor="end" data-ns="{ns}">{}</text>"##,
            PLOT_LEFT - 10.0,
            y + 3.5,
            format_gbps_tick(value),
        )
            .unwrap();
    }
    writeln!(svg, "  </g>").unwrap();

    writeln!(
        svg,
        r##"  <text id="y-title-{p}" x="30" y="{:.1}" class="axis-title" text-anchor="middle" transform="rotate(-90 30 {:.1})">{} (log scale) · higher is better</text>"##,
        (top + bottom) / 2.0,
        (top + bottom) / 2.0,
        plot.use_case.rate_unit_long(),
    )
        .unwrap();

    /*
     * Vertical guides and x-axis labels at each tested point. Where a label
     * would run into its left neighbour (3 KiB sits 24 px from 4 KiB on the
     * log axis), it drops to a second row with a short tick joining it to
     * its column.
     */
    /* About 7.2 px per character at the bold label font, plus a gutter. */
    let label_width = |k: usize| POINTS[plot.points.start + k].label.chars().count() as f64 * 7.2 + 6.0;
    let mut label_rows = vec![0u8; plot.len()];
    for k in 1..plot.len() {
        let gap = plot.x_positions[k] - plot.x_positions[k - 1];
        if gap < (label_width(k - 1) + label_width(k)) / 2.0 && label_rows[k - 1] == 0 {
            label_rows[k] = 1;
        }
    }
    for (k, point_index) in plot.points.clone().enumerate() {
        let x = plot.x_positions[k];
        let row = label_rows[k];
        let label_y = bottom + 24.0 + 13.0 * f64::from(row);

        writeln!(
            svg,
            r##"  <line x1="{x:.2}" y1="{top:.1}" x2="{x:.2}" y2="{bottom:.1}" class="grid-x"/>"##
        )
            .unwrap();

        if row == 1 {
            writeln!(
                svg,
                r##"  <line x1="{x:.2}" y1="{:.1}" x2="{x:.2}" y2="{:.1}" class="size-tick"/>"##,
                bottom + 4.0,
                label_y - 10.0,
            )
                .unwrap();
        }

        writeln!(
            svg,
            r##"  <text x="{x:.2}" y="{label_y:.1}" class="size-label" text-anchor="middle">{}</text>"##,
            xml_escape(POINTS[point_index].label),
        )
            .unwrap();
    }

    writeln!(
        svg,
        r##"  <text x="{:.1}" y="{:.1}" class="axis-title" text-anchor="middle">{}</text>"##,
        (PLOT_LEFT + PLOT_RIGHT) / 2.0,
        bottom + 52.0,
        xml_escape(plot.use_case.x_axis()),
    )
        .unwrap();

    writeln!(
        svg,
        r##"  <line x1="{PLOT_LEFT:.1}" y1="{top:.1}" x2="{PLOT_LEFT:.1}" y2="{bottom:.1}" class="axis"/>"##
    )
        .unwrap();

    writeln!(
        svg,
        r##"  <line x1="{PLOT_LEFT:.1}" y1="{bottom:.1}" x2="{PLOT_RIGHT:.1}" y2="{bottom:.1}" class="axis"/>"##
    )
        .unwrap();

    let value_label_y = place_value_labels(plot, results);

    /*
     * Dots are collected here and emitted after every series' band and
     * line, so no band can sit above another contender's dots and take
     * the hover. Each dot layer carries its plot and series index; the
     * script and stylesheet treat it as part of that series.
     */
    let mut dot_layers: Vec<String> = Vec::new();

    /*
     * One group per contender holds everything that belongs to it: band,
     * line, dots, value labels, the clickable label at right, and (in the
     * first plot) its provenance lines. Toggling flips one attribute on
     * the group.
     */
    for &algorithm_index in &plot.contenders {
        let algorithm = roster.algorithms[algorithm_index];
        let color = algorithm.color();
        let kernels = plot.kernels(algorithm_index);
        let cell_at = |k: usize| cell(results, algorithm_index, plot.points.start + k);
        let last = cell_at(plot.len() - 1);

        writeln!(
            svg,
            r##"  <g class="series" id="series-{p}-{algorithm_index}" data-on="true">"##
        )
            .unwrap();

        writeln!(svg, r##"    <g class="marks">"##).unwrap();

        let mut band = String::new();
        for k in 0..plot.len() {
            let x = plot.x_positions[k];
            let y = plot.map_y(cell_at(k).time.high);
            if k == 0 {
                write!(band, "M {x:.2} {y:.2}").unwrap();
            } else {
                write!(band, " L {x:.2} {y:.2}").unwrap();
            }
        }
        for k in (0..plot.len()).rev() {
            let x = plot.x_positions[k];
            let y = plot.map_y(cell_at(k).time.low);
            write!(band, " L {x:.2} {y:.2}").unwrap();
        }
        band.push_str(" Z");

        /*
         * The band's tint reports the run's precision for this contender.
         * Spread is (max − min) / median at a point; the band takes the
         * worst spread across the axis. Tight runs stay a faint tint;
         * wider runs deepen it. No outline: the tint alone carries the
         * precision, and the plot stays quiet.
         */
        let worst_spread = (0..plot.len())
            .map(|k| spread_permille(cell_at(k).time))
            .max()
            .expect("there is at least one point");
        let (opacity_hundredths, _) = band_style(worst_spread);

        writeln!(
            svg,
            r##"      <path class="band" d="{band}" fill="{color}" fill-opacity="0.{opacity_hundredths:02}" stroke="none"/>"##,
        )
            .unwrap();

        let mut path = String::new();
        for k in 0..plot.len() {
            let x = plot.x_positions[k];
            let y = plot.map_y(cell_at(k).time.median);
            if k == 0 {
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

        let mut dots = format!("  <g class=\"dots\" id=\"dots-{p}-{algorithm_index}\" data-on=\"true\">\n");

        /*
         * In a solo run the contender's duo medians draw as a dashed line
         * of the same colour with hollow dots, inside the same group so
         * the toggle hides both. Where duo and solo agree the dashed line
         * lies on the solid one; where a contender pays for sharing the
         * machine the dashed line rises above it, and the gap is the price.
         */
        if roster.solo {
            let mut duo_path = String::new();
            for k in 0..plot.len() {
                let x = plot.x_positions[k];
                let y = plot.map_y(cell_at(k).duo.expect("solo run has duo statistics").median);
                if k == 0 {
                    write!(duo_path, "M {x:.2} {y:.2}").unwrap();
                } else {
                    write!(duo_path, " L {x:.2} {y:.2}").unwrap();
                }
            }
            writeln!(
                svg,
                r##"      <path class="median duo" d="{duo_path}" fill="none" stroke="{color}" stroke-width="2" stroke-dasharray="6,4" stroke-linejoin="round" stroke-linecap="round"/>"##
            )
                .unwrap();
            for k in 0..plot.len() {
                let x = plot.x_positions[k];
                let y = plot.map_y(cell_at(k).duo.unwrap().median);
                writeln!(
                    dots,
                    r##"    <g class="dot dot-duo" data-size="{k}" transform="translate({x:.2} {y:.2})" onpointerenter="hoverDot(event,{p},{algorithm_index},{k})" onpointerleave="leaveDot(event)" onclick="tapDot(event,{p},{algorithm_index},{k})"><circle r="4" fill="#fdfdfc" stroke="{color}" stroke-width="2"/></g>"##
                )
                    .unwrap();
            }
        }

        for k in 0..plot.len() {
            let x = plot.x_positions[k];
            let statistics = cell_at(k).time;
            let median_y = plot.map_y(statistics.median);

            /*
             * The dot's shape names the code path that produced this point;
             * the shape alone marks a new path, so every dot draws the same
             * size with no ring. Hovering shows the path's explanation.
             */
            let kernel = &kernels.kernels[kernels.kernel_index_for(POINTS[plot.points.start + k].bytes)];
            writeln!(
                dots,
                r##"    <g class="dot" data-size="{k}" transform="translate({x:.2} {median_y:.2})" onpointerenter="hoverDot(event,{p},{algorithm_index},{k})" onpointerleave="leaveDot(event)" onclick="tapDot(event,{p},{algorithm_index},{k})">"##,
            )
                .unwrap();
            writeln!(dots, "      {}", mark_shape(kernel.mark, color, 5.0)).unwrap();
            dots.push_str("    </g>\n");

            /*
             * With twenty-four columns, a value at every dot would overprint.
             * Label the ends and every fourth point counted from the last,
             * so the axis's far end and the points four apart below it
             * carry values; hovering a dot shows the rest. Edge columns
             * anchor inward so labels stay clear of the y-axis gutter and
             * the series labels at right.
             */
            let labeled = k == 0 || (plot.len() - 1 - k) % 4 == 0;

            if !labeled {
                continue;
            }

            let (label_x, anchor) = if k == 0 {
                (x + 9.0, "start")
            } else if k == plot.len() - 1 {
                (x - 9.0, "end")
            } else {
                (x, "middle")
            };

            writeln!(
                svg,
                r##"      <text class="value-label" data-size="{k}" x="{label_x:.2}" y="{:.2}" fill="{color}" text-anchor="{anchor}">{}</text>"##,
                value_label_y[algorithm_index][k],
                format_rate_value(statistics.median, plot.use_case),
            )
                .unwrap();
        }

        writeln!(svg, "    </g>").unwrap();
        dots.push_str("  </g>\n");
        dot_layers.push(dots);

        /* Clickable label at right: swatch, name, detail, hint. */
        let statistics = last.time;
        let label_x = PLOT_RIGHT + 14.0;
        let label_y = plot.label_y[algorithm_index].expect("a participant has a label slot");

        writeln!(
            svg,
            r##"    <g class="series-label" transform="translate(0 {label_y:.2})" onclick="event.stopPropagation(); toggleSeries({algorithm_index})" onpointerenter="hoverLabel(event,{algorithm_index},true)" onpointerleave="hoverLabel(event,{algorithm_index},false)">"##
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
        /* Swatch: every dot shape this contender uses, in its colour, so the
           right-hand names match the marks in the plot. Names share one x
           across contenders; shapes fill fixed slots, so rows align. Each
           shape carries a tooltip naming its code path. */
        let mut swatch_marks: Vec<(Mark, &str, &str)> = Vec::new();
        for kernel in &kernels.kernels {
            if !swatch_marks.iter().any(|slot| slot.0 == kernel.mark) {
                swatch_marks.push((kernel.mark, &kernel.name, &kernel.why));
            }
        }
        let name_x = label_x + 14.0 + (SWATCH_SLOTS as f64) * 13.0;
        writeln!(svg, r##"      <g class="series-swatch" transform="translate(0 0)">"##).unwrap();
        for (mark_index, (mark, name, why)) in swatch_marks.iter().enumerate() {
            writeln!(
                svg,
                r##"        <g transform="translate({:.1} 0)"><title>{}: {}</title>{}</g>"##,
                label_x + 4.5 + mark_index as f64 * 13.0,
                xml_escape(name),
                xml_escape(why),
                mark_shape(*mark, color, 4.5),
            )
            .unwrap();
        }
        writeln!(svg, r##"      </g>"##).unwrap();
        writeln!(
            svg,
            r##"      <text class="series-name" x="{:.1}" y="4" fill="{color}">{}</text>"##,
            name_x,
            xml_escape(algorithm.name()),
        )
            .unwrap();
        writeln!(
            svg,
            r##"      <text class="series-detail" x="{:.1}" y="18">{} · {} {} at {}</text>"##,
            name_x,
            format_rate(statistics.median, plot.use_case),
            format_ps(statistics.median),
            plot.use_case.time_unit(),
            xml_escape(POINTS[plot.points.end - 1].label),
        )
            .unwrap();
        writeln!(
            svg,
            r##"      <text class="series-hint" x="{:.1}" y="18">hidden · click to show</text>"##,
            name_x,
        )
            .unwrap();
        writeln!(svg, "    </g>").unwrap();

        /* This contender's provenance lines, hidden along with it. */
        if p == 0 {
            for line in contender_provenance_lines(algorithm, kernels) {
                writeln!(
                    svg,
                    r##"    <text class="prov series-prov" x="{PLOT_LEFT:.1}" y="{:.1}">{}</text>"##,
                    provenance_line_y(*provenance_slot),
                    xml_escape(&line),
                )
                    .unwrap();
                *provenance_slot += 1;
            }
        }

        writeln!(svg, "  </g>").unwrap();
    }

    for layer in &dot_layers {
        svg.push_str(layer);
    }

    /*
     * Shape legend under the plot's right end: one entry per mark in use,
     * in neutral grey, since colour belongs to contenders and shape to
     * code paths.
     */
    let mut marks: Vec<Mark> = Vec::new();
    for &algorithm_index in &plot.contenders {
        for kernel in &plot.kernels(algorithm_index).kernels {
            if !marks.contains(&kernel.mark) {
                marks.push(kernel.mark);
            }
        }
    }
    let legend_y = bottom + 68.0;
    let mut x = PLOT_RIGHT;
    let entries: Vec<(Mark, &str)> = marks
        .iter()
        .enumerate()
        .map(|(index, &mark)| {
            (mark, match index { 0 => "first code path", 1 => "second", 2 => "third", _ => "fourth" })
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
    /* In a solo run: the dashed line and hollow dot, on the left of the row. */
    if roster.solo {
        let x = PLOT_LEFT;
        writeln!(
            svg,
            r##"  <path d="M {x:.1} {y:.1} L {:.1} {y:.1}" stroke="#8a8a8a" stroke-width="2" stroke-dasharray="6,4"/><circle cx="{:.1}" cy="{y:.1}" r="3" fill="#fdfdfc" stroke="#8a8a8a" stroke-width="1.5"/>"##,
            x + 30.0,
            x + 15.0,
            y = legend_y - 3.5,
        )
            .unwrap();
        writeln!(
            svg,
            r##"  <text x="{:.1}" y="{:.1}" class="legend">duo: two copies at once, later finish, per byte of one copy · solid line: solo</text>"##,
            x + 38.0,
            legend_y,
        )
            .unwrap();
    }
}

/*
 * Relative width of one cell's median interval in permille: 1000 × (high −
 * low) / median, rounded. Zero when every resample agrees on the median;
 * 20 means the median is known to within 2%.
 */
fn spread_permille(statistics: Statistics) -> u64 {
    let range = statistics.high - statistics.low;
    (range * 1000 + statistics.median / 2) / statistics.median
}

/*
 * Band fill opacity and whether to outline it, from the worst interval
 * width. The script applies the same thresholds. Under 2% is a well-known
 * median; 2–5% earns a deeper tint; 5% and over adds the dashed outline,
 * which with 80 rounds means the samples disagree with each other well
 * beyond ordinary noise (a two-mode cell, or heavy interference).
 */
const SPREAD_NOTICEABLE_PERMILLE: u64 = 20;
const SPREAD_WIDE_PERMILLE: u64 = 50;

/// Fill opacity in hundredths (16 → 0.16) and whether to outline.
fn band_style(worst_spread_permille: u64) -> (u64, bool) {
    let opacity_hundredths = if worst_spread_permille < SPREAD_NOTICEABLE_PERMILLE {
        16
    } else if worst_spread_permille < SPREAD_WIDE_PERMILLE {
        16 + 14 * (worst_spread_permille - SPREAD_NOTICEABLE_PERMILLE)
            / (SPREAD_WIDE_PERMILLE - SPREAD_NOTICEABLE_PERMILLE)
    } else {
        30
    };
    (opacity_hundredths, worst_spread_permille >= SPREAD_WIDE_PERMILLE)
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

fn place_value_labels(plot: &Plot, results: &Results) -> Vec<Vec<f64>> {
    let mut placed = vec![vec![0.0_f64; plot.len()]; results.len()];
    for k in 0..plot.len() {
        let point_index = plot.points.start + k;
        let mut order: Vec<usize> = plot.contenders.clone();
        order.sort_by(|&a, &b| {
            cell(results, b, point_index).time.median.cmp(&cell(results, a, point_index).time.median)
        });
        /* Smallest y (fastest, highest on the plot) first. */
        order.reverse();
        let mut taken: Vec<f64> = Vec::new();
        for algorithm_index in order {
            let dot_y = plot.map_y(cell(results, algorithm_index, point_index).time.median);
            let clear = |y: f64, taken: &[f64]| taken.iter().all(|t| (t - y).abs() >= VALUE_LABEL_HEIGHT);
            let mut y = dot_y + VALUE_LABEL_ABOVE;
            if !clear(y, &taken) {
                y = dot_y + VALUE_LABEL_BELOW;
                while !clear(y, &taken) {
                    y += VALUE_LABEL_HEIGHT;
                }
            }
            taken.push(y);
            placed[algorithm_index][k] = y;
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
        Mark::Triangle => {
            /* Point up; the centroid sits at the origin. */
            let r = radius * 1.3;
            format!(
                r##"<path d="M 0 {top:.2} L {r:.2} {base:.2} L {left:.2} {base:.2} Z" fill="{color}" {stroke}/>"##,
                top = -r,
                base = r * 0.5,
                left = -r,
            )
        }
    }
}

fn provenance_line_y(slot: usize) -> f64 {
    PROVENANCE_TOP + 40.0 + slot as f64 * PROVENANCE_LINE_HEIGHT
}

/* Provenance that describes the run as a whole. */
/// One collapsible provenance category: a header row plus its detail lines.
struct ProvCat {
    key: &'static str,
    name: &'static str,
    summary: String,
    lines: Vec<String>,
}

fn shared_provenance_cats(machine: &MachineMetadata, selection_note: &str, basis: TimeBasis) -> Vec<ProvCat> {
    vec![
        ProvCat {
            key: "run",
            name: "Run",
            summary: format!("{} · bench-hashes {BENCH_VERSION}", machine.timestamp),
            lines: vec![
                format!(
                    "Run: {} · bench-hashes {BENCH_VERSION}",
                    machine.timestamp,
                ),
                format!("Contenders: {selection_note}"),
                format!("Tag: {GIT_TAG} · Working tree: {GIT_CLEAN_STATUS}"),
            ],
        },
        ProvCat {
            key: "machine",
            name: "Machine",
            summary: format!("{} · {} CPUs · {}", machine.cpu_type, machine.cpu_count, machine.os_type),
            lines: vec![
                format!(
                    "Machine: {} · {} logical CPUs · {}",
                    machine.cpu_type, machine.cpu_count, machine.os_type,
                ),
                format!("Toolchain: {RUSTC_VERSION} · {BUILD_TARGET}"),
                format!("Sample clock: {}", sample_clock::NAME),
                format!("Reported time: {}", basis.describe()),
            ],
        },
        ProvCat {
            key: "sources",
            name: "Sources",
            summary: format!("bench-hashes @ {}", &GIT_COMMIT[..12.min(GIT_COMMIT.len())]),
            lines: vec![
                format!("Source: {GIT_SOURCE} @ {GIT_COMMIT}"),
                "Full crate checksums are in this file's metadata element".to_owned(),
            ],
        },
    ]
}

/* Provenance that belongs to one contender and hides with it: the
   implementation, its mode, and the platform its kernels ran on. */
fn contender_provenance_lines(
    algorithm: Algorithm,
    kernels: &Kernels,
) -> Vec<String> {
    let name = algorithm.name();
    let platform = kernels.platform;
    match algorithm {
        Algorithm::Blake3 => vec![format!(
            "{name}: {} · {} · platform {platform}",
            package_name_and_version(BLAKE3_SOURCE_INFO),
            algorithm.mode(),
        )],
        Algorithm::Sha256 => vec![format!(
            "{name}: {} · {} · {}",
            package_name_and_version(SHA2_SOURCE_INFO),
            algorithm.mode(),
            kernels.kernels[0].name,
        )],
        Algorithm::Sha1Dc => vec![format!(
            "{name}: {} · {}",
            package_name_and_version(SHA1_CHECKED_SOURCE_INFO),
            algorithm.mode(),
        )],
        Algorithm::Blake3Servil => vec![
            format!("{name}: {} · hash, hash_many for a batch", short_git_source(BLAKE3_SERVIL_SOURCE_INFO)),
            format!("{name}: single-threaded · platform {platform}"),
        ],
        Algorithm::Sha256CommonCrypto => vec![format!("{name}: {} · {}", algorithm.mode(), kernels.kernels[0].name)],
        Algorithm::Sha256Ring => vec![format!(
            "{name}: {} · {} · {}",
            package_name_and_version(RING_SOURCE_INFO),
            algorithm.mode(),
            kernels.kernels[0].name,
        )],
        Algorithm::Blake3Rayon => vec![
            format!(
                "{name}: {} · Hasher::update_rayon · platform {platform}",
                package_name_and_version(BLAKE3_SOURCE_INFO),
            ),
            format!("{name}: {}", algorithm.thread_resources().expect("BLAKE3 mt runs on Rayon's pool")),
        ],
        Algorithm::Blake3ServilMt => vec![
            format!("{name}: {} · hash_multithreaded, hash_many_multithreaded for a batch", short_git_source(BLAKE3_SERVIL_SOURCE_INFO)),
            format!("{name}: multithreaded on the fork's own threads · platform {platform}"),
        ],
        Algorithm::Blake3ServilMt1 => vec![
            format!("{name}: {} · hash_multithreaded_with_budget(_, 1)", short_git_source(BLAKE3_SERVIL_SOURCE_INFO)),
            format!("{name}: capped at one thread · platform {platform}"),
        ],
        Algorithm::AbBlake3 => vec![format!(
            "{name}: {} · const_hash for one message, single_block_hash_many_exact::<N> for a batch · {}",
            package_name_and_version(AB_BLAKE3_SOURCE_INFO),
            algorithm.mode().split(';').next().unwrap(),
        )],
    }
}

/*
 * The script re-derives every y position from the visible contenders'
 * data, using the same rules as the Rust layout: nice log bounds with 8%
 * headroom, the same tick mantissas, the same label stacking gap. The
 * measurements and layout constants travel as JSON so the two stay in
 * lockstep. Each plot carries its own points, series, and units; the
 * contender names, colours, and on/off state are shared.
 */
fn write_interaction_script(
    svg: &mut String,
    roster: &Roster,
    results: &Results,
    plots: &[Plot],
    shared_count: usize,
) {
    let mut data = String::from("{\"names\":[");
    for (index, algorithm) in roster.algorithms.iter().enumerate() {
        if index > 0 { data.push(','); }
        data.push_str(&json_string(algorithm.name()));
    }
    data.push_str("],\"colors\":[");
    for (index, algorithm) in roster.algorithms.iter().enumerate() {
        if index > 0 { data.push(','); }
        write!(data, "\"{}\"", algorithm.color()).unwrap();
    }
    data.push_str("],\"plots\":[");
    for (plot_index, plot) in plots.iter().enumerate() {
        if plot_index > 0 { data.push(','); }
        write!(
            data,
            "{{\"top\":{:.1},\"bottom\":{:.1},\"scale\":{},\"timeUnit\":{},\"rateUnit\":{},\"rateLong\":{},\"timeLong\":{},\"x\":[",
            plot.top,
            plot.bottom,
            plot.use_case.rate_scale(),
            json_string(plot.use_case.time_unit()),
            json_string(plot.use_case.rate_unit()),
            json_string(plot.use_case.rate_unit_long()),
            json_string(match plot.use_case {
                UseCase::OneMessage => "Nanoseconds per byte",
                UseCase::ManyMessages => "Nanoseconds per message",
            }),
        )
        .unwrap();
        for (index, x) in plot.x_positions.iter().enumerate() {
            if index > 0 { data.push(','); }
            write!(data, "{x:.2}").unwrap();
        }
        data.push_str("],\"sizes\":[");
        for (index, point_index) in plot.points.clone().enumerate() {
            if index > 0 { data.push(','); }
            data.push_str(&json_string(POINTS[point_index].label));
        }
        data.push_str("],\"labelY\":[");
        for (index, label_y) in plot.label_y.iter().enumerate() {
            if index > 0 { data.push(','); }
            match label_y {
                Some(y) => write!(data, "{y:.2}").unwrap(),
                None => data.push_str("null"),
            }
        }
        data.push_str("],\"series\":[");
        for algorithm_index in 0..roster.len() {
            if algorithm_index > 0 { data.push(','); }
            let Some(kernels) = &plot.kernels[algorithm_index] else {
                data.push_str("null");
                continue;
            };
            data.push_str("{\"kernels\":[");
            for (kernel_index, kernel) in kernels.kernels.iter().enumerate() {
                if kernel_index > 0 { data.push(','); }
                let first = plot
                    .points
                    .clone()
                    .position(|point_index| POINTS[point_index].bytes >= kernel.first)
                    .expect("every kernel starts at or below the axis's largest point");
                write!(
                    data,
                    "{{\"from\":{first},\"name\":{},\"why\":{},\"mark\":\"{}\"}}",
                    json_string(&kernel.name),
                    json_string(&kernel.why),
                    kernel.mark.name(),
                )
                .unwrap();
            }
            let cell_at = |k: usize| cell(results, algorithm_index, plot.points.start + k);
            for (key, pick) in [
                ("min", (|t: Statistics| t.minimum) as fn(Statistics) -> u64),
                ("low", |t| t.low),
                ("med", |t| t.median),
                ("high", |t| t.high),
                ("max", |t| t.maximum),
            ] {
                write!(data, "],\"{key}\":[").unwrap();
                for k in 0..plot.len() {
                    if k > 0 { data.push(','); }
                    write!(data, "{}", format_ps(pick(cell_at(k).time))).unwrap();
                }
            }
            if roster.solo {
                for (key, pick) in [
                    ("duoLow", (|t: Statistics| t.low) as fn(Statistics) -> u64),
                    ("duoMed", |t| t.median),
                    ("duoHigh", |t| t.high),
                ] {
                    write!(data, "],\"{key}\":[").unwrap();
                    for k in 0..plot.len() {
                        if k > 0 { data.push(','); }
                        write!(data, "{}", format_ps(pick(cell_at(k).duo.unwrap()))).unwrap();
                    }
                }
            }
            data.push_str("],\"n\":[");
            for k in 0..plot.len() {
                if k > 0 { data.push(','); }
                write!(data, "{}", cell_at(k).time.count).unwrap();
            }
            data.push_str("],\"modes\":[");
            for k in 0..plot.len() {
                if k > 0 { data.push(','); }
                match cell_at(k).time.modes {
                    Some(m) => write!(
                        data,
                        "[{},{},{},{}]",
                        format_ps(m.lower_median), m.lower_count, format_ps(m.upper_median), m.upper_count
                    )
                    .unwrap(),
                    None => data.push_str("null"),
                }
            }
            data.push_str("]}");
        }
        data.push_str("]}");
    }
    write!(
        data,
        "],\"sharedProv\":{shared_count},\"svgWidth\":{SVG_WIDTH:.0},\"plotLeft\":{PLOT_LEFT},\"plotRight\":{PLOT_RIGHT},\"labelGap\":{SERIES_LABEL_GAP},\"rounds\":{},\"labelAbove\":{VALUE_LABEL_ABOVE},\"labelBelow\":{VALUE_LABEL_BELOW},\"labelHeight\":{VALUE_LABEL_HEIGHT},\"spreadNoticeable\":0.{SPREAD_NOTICEABLE_PERMILLE:03},\"spreadWide\":0.{SPREAD_WIDE_PERMILLE:03},\"provTop\":{PROVENANCE_TOP},\"provLine\":{PROVENANCE_LINE_HEIGHT}}}",
        roster.rounds,
    )
        .unwrap();

    svg.push_str("  <script><![CDATA[\n");
    writeln!(svg, "const DATA = {data};").unwrap();
    svg.push_str(INTERACTION_SCRIPT);
    svg.push_str("  ]]></script>\n");
}

const INTERACTION_SCRIPT: &str = r##"
/* One on/off state per contender, shared by both plots. */
const on = DATA.names.map(() => true);

/*
 * Display unit. Data is stored as ns per unit (byte or message, by plot);
 * the rate is the plot's scale over it, so on the log axis switching units
 * mirrors each plot: the fastest contender moves from the bottom to the
 * top. Every drawn or printed value goes through val() and fmt(); ratios
 * between contenders are unitless and stay put.
 */
let unit = "gbps";

/*
 * Blend between the units. `blend` runs from 0 (time) to 1 (rate), and the
 * plotted value is the log-space interpolation of the two readings, so
 * during a switch every point travels a straight line on the log axis and
 * each plot mirrors through its middle. Text follows the unit from the
 * midpoint; the two axes cross-fade.
 */
let blend = 1;
const valAt = (ns, b, scale) => Math.exp((1 - b) * Math.log(ns) + b * Math.log(scale / ns));
const val = (ns, p) => valAt(ns, blend, DATA.plots[p].scale);
/* Values in the settled unit, for text. */
const settled = (ns, p) => unit === "ns" ? ns : DATA.plots[p].scale / ns;
function fmt(ns, p, digits) {
  const v = settled(ns, p);
  if (unit === "ns") return v.toFixed(digits === undefined ? 3 : digits);
  return v >= 10 ? v.toFixed(digits === undefined ? 0 : Math.max(0, digits - 2)) : v.toFixed(digits === undefined ? 1 : Math.max(1, digits - 1));
}
const unitLabel = p => unit === "ns" ? DATA.plots[p].timeUnit : DATA.plots[p].rateUnit;
const otherUnitLabel = p => unit === "ns" ? DATA.plots[p].rateUnit : DATA.plots[p].timeUnit;
const fmtOther = (ns, p) => unit === "ns" ? rate(ns, p) : ns.toFixed(3) + " " + DATA.plots[p].timeUnit;

let animation = null;
let chosen = "gbps";
function flipUnit() { setUnit(chosen === "ns" ? "gbps" : "ns"); }
function setUnit(u) {
  chosen = u;
  const target = u === "ns" ? 0 : 1;
  if (animation) cancelAnimationFrame(animation);
  document.getElementById("unit-knob").setAttribute("cy", u === "ns" ? 27 : 7);
  document.getElementById("unit-switch").querySelectorAll(".unit-label")
    .forEach(l => l.setAttribute("class", "unit-label" + (l.getAttribute("data-unit") === u ? " unit-on" : "")));

  const start = blend, startTime = performance.now(), DURATION = 700;
  const ease = x => x < 0.5 ? 4 * x * x * x : 1 - Math.pow(-2 * x + 2, 3) / 2;
  /* Prepare each incoming axis so it can fade in while the old one fades out. */
  const outgoing = [], incoming = [];
  DATA.plots.forEach((_, p) => {
    const out = document.getElementById("y-axis-" + p);
    out.setAttribute("id", "y-axis-old-" + p);
    const inc = document.createElementNS(NS, "g");
    inc.setAttribute("id", "y-axis-" + p);
    inc.setAttribute("opacity", "0");
    out.parentNode.insertBefore(inc, out.nextSibling);
    outgoing.push(out); incoming.push(inc);
  });

  const step = now => {
    const raw = Math.min(1, (now - startTime) / DURATION);
    const e = ease(raw);
    blend = start + (target - start) * e;
    /* Text follows the unit once the plot is past halfway. */
    unit = blend >= 0.5 ? "gbps" : "ns";
    DATA.plots.forEach((plot, p) => {
      document.getElementById("y-title-" + p).textContent =
        unit === "ns" ? plot.timeLong + " (log scale) · lower is better" : plot.rateLong + " (log scale) · higher is better";
      outgoing[p].setAttribute("opacity", (1 - e).toFixed(3));
      incoming[p].setAttribute("opacity", e.toFixed(3));
    });
    relayout();
    if (hovered) showHover(hovered[0], hovered[1], hovered[2]);
    if (raw < 1) {
      animation = requestAnimationFrame(step);
    } else {
      outgoing.forEach(out => out.parentNode.removeChild(out));
      animation = null;
    }
  };
  animation = requestAnimationFrame(step);
}
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

/* Current y mapping per plot, kept by relayout() so the hover panel places itself. */
const currentMapY = DATA.plots.map(() => null);

function relayout() {
  DATA.plots.forEach((_, p) => relayoutPlot(p));
  layoutProv();
}

function relayoutPlot(p) {
  const plot = DATA.plots[p];
  const visible = plot.series.map((s, i) => i).filter(i => on[i] && plot.series[i]);
  let lo = Infinity, hi = 0;
  for (const i of visible) {
    const s = plot.series[i];
    lo = Math.min(lo, ...s.low);
    hi = Math.max(hi, ...s.high);
    if (s.duoMed) {
      lo = Math.min(lo, ...s.duoLow);
      hi = Math.max(hi, ...s.duoHigh);
    }
  }
  if (visible.length === 0) { lo = 0.1; hi = 1; }
  /*
   * Axis bounds at both ends of the blend, then interpolated in log space
   * alongside the data, so the axis and the points move together.
   */
  const boundsAt = b => {
    const a = valAt(lo, b, plot.scale), c = valAt(hi, b, plot.scale);
    const dLo = Math.min(a, c), dHi = Math.max(a, c);
    return [Math.log(niceBelow(dLo * 0.92)), Math.log(niceAbove(dHi * 1.08))];
  };
  const [n0, n1] = boundsAt(0), [g0, g1] = boundsAt(1);
  const lMin = (1 - blend) * n0 + blend * g0, lMax = (1 - blend) * n1 + blend * g1;
  /* mapY takes ns per unit, as stored; the unit transform happens inside. */
  const mapY = ns => plot.bottom - (Math.log(val(ns, p)) - lMin) / (lMax - lMin) * (plot.bottom - plot.top);
  currentMapY[p] = mapY;
  /*
   * Y axis for the settled unit. Each tick is a value in that unit; its
   * stored equivalent is placed with the blended mapY, so mid-animation
   * the ticks ride the same mirroring motion as the data, while the
   * outgoing axis (still in the old unit, in its own group) fades out and
   * this one fades in.
   */
  const axis = document.getElementById("y-axis-" + p);
  while (axis.firstChild) axis.removeChild(axis.firstChild);
  const [tMin, tMax] = unit === "ns" ? [n0, n1] : [g0, g1];
  const tickToNs = v => unit === "ns" ? v : plot.scale / v;
  for (const v of ticks(Math.exp(tMin), Math.exp(tMax))) {
    const y = mapY(tickToNs(v));
    const line = document.createElementNS(NS, "line");
    line.setAttribute("x1", DATA.plotLeft); line.setAttribute("x2", DATA.plotRight);
    line.setAttribute("y1", y.toFixed(2)); line.setAttribute("y2", y.toFixed(2));
    line.setAttribute("class", "grid");
    axis.appendChild(line);
    line.setAttribute("data-ns", tickToNs(v));
    const t = document.createElementNS(NS, "text");
    t.setAttribute("x", (DATA.plotLeft - 10).toFixed(1)); t.setAttribute("y", (y + 3.5).toFixed(2));
    t.setAttribute("class", "tick-label"); t.setAttribute("text-anchor", "end");
    t.setAttribute("data-ns", tickToNs(v));
    t.textContent = unit === "ns" ? fmtTick(v) : (v >= 10 ? v.toFixed(0) : v.toFixed(1));
    axis.appendChild(t);
  }
  /* The outgoing axis, if one is fading, rides the same motion. */
  const old = document.getElementById("y-axis-old-" + p);
  if (old) {
    old.querySelectorAll("line").forEach(l => { const y = mapY(+l.getAttribute("data-ns")); l.setAttribute("y1", y.toFixed(2)); l.setAttribute("y2", y.toFixed(2)); });
    old.querySelectorAll("text").forEach(t => { t.setAttribute("y", (mapY(+t.getAttribute("data-ns")) + 3.5).toFixed(2)); });
  }

  /* Each series: band, median line, dots, value labels. */
  plot.series.forEach((s, i) => {
    if (!s) return;
    const g = document.getElementById("series-" + p + "-" + i);
    const dots = document.getElementById("dots-" + p + "-" + i);
    g.setAttribute("data-on", on[i] ? "true" : "false");
    dots.setAttribute("data-on", on[i] ? "true" : "false");
    if (!on[i]) return;
    const X = plot.x;
    let band = "";
    X.forEach((x, k) => { band += (k ? " L " : "M ") + x + " " + mapY(s.high[k]).toFixed(2); });
    for (let k = X.length - 1; k >= 0; k--) band += " L " + X[k] + " " + mapY(s.low[k]).toFixed(2);
    g.querySelector(".band").setAttribute("d", band + " Z");
    let med = "";
    X.forEach((x, k) => { med += (k ? " L " : "M ") + x + " " + mapY(s.med[k]).toFixed(2); });
    g.querySelector(".median:not(.duo)").setAttribute("d", med);
    if (s.duoMed) {
      let duo = "";
      X.forEach((x, k) => { duo += (k ? " L " : "M ") + x + " " + mapY(s.duoMed[k]).toFixed(2); });
      g.querySelector(".median.duo").setAttribute("d", duo);
    }
    dots.querySelectorAll(".dot").forEach(dot => {
      const k = +dot.getAttribute("data-size");
      const series = dot.classList.contains("dot-duo") ? s.duoMed : s.med;
      dot.setAttribute("transform", `translate(${X[k]} ${mapY(series[k]).toFixed(2)})`);
    });
  });

  /* Value labels: above the dot unless that collides within the column. */
  for (let k = 0; k < plot.x.length; k++) {
    const order = visible.slice().sort((a, b) => mapY(plot.series[a].med[k]) - mapY(plot.series[b].med[k]));
    const taken = [];
    const clear = y => taken.every(t => Math.abs(t - y) >= DATA.labelHeight);
    for (const i of order) {
      const dotY = mapY(plot.series[i].med[k]);
      let y = dotY + DATA.labelAbove;
      if (!clear(y)) { y = dotY + DATA.labelBelow; while (!clear(y)) y += DATA.labelHeight; }
      taken.push(y);
      document.getElementById("series-" + p + "-" + i).querySelectorAll(".value-label").forEach(t => {
        if (+t.getAttribute("data-size") === k) {
          t.setAttribute("y", y.toFixed(2));
          t.textContent = fmt(plot.series[i].med[k], p, 2);
        }
      });
    }
  }
  /*
   * Right-edge labels, every participating contender in its slot. Each
   * anchors level with its line's last point on the current axis; a hidden
   * contender's anchor is clamped to the plot edge, so its grey label
   * points toward where its data lies. Then push overlapping labels apart
   * and keep the stack inside the plot.
   */
  const last = plot.x.length - 1;
  const clamp = y => Math.min(plot.bottom - 8, Math.max(plot.top + 8, y));
  const slots = plot.series
    .map((s, i) => s ? [i, clamp(mapY(s.med[last]))] : null)
    .filter(slot => slot)
    .sort((a, b) => a[1] - b[1]);
  for (let k = 1; k < slots.length; k++) {
    slots[k][1] = Math.max(slots[k][1], slots[k - 1][1] + DATA.labelGap);
  }
  const overrun = Math.max(0, slots[slots.length - 1][1] + 20 - plot.bottom);
  for (const [i, y] of slots) {
    const lab = document.getElementById("series-" + p + "-" + i).querySelector(".series-label");
    lab.setAttribute("transform", `translate(0 ${(y - overrun).toFixed(2)})`);
    const detail = lab.querySelector(".series-detail");
    const m = plot.series[i].med[last];
    detail.textContent = `${fmt(m, p, 2)} ${unitLabel(p)} · ${fmtOther(m, p)} at ${plot.sizes[last]}`;
  }
}

const provOpen = {run: false, machine: false, sources: false};

function toggleProv(cat) {
  provOpen[cat] = !provOpen[cat];
  layoutProv();
}

function layoutProv() {
  /* Headers and details flow in document order: each header stays
     visible at its slot, details show only when their category is open. */
  let slot = 0;
  const y = s => (DATA.provTop + 40 + s * DATA.provLine).toFixed(1);
  document.querySelectorAll(".prov-head-row, .prov-shared").forEach(el => {
    if (el.classList.contains("prov-head-row")) {
      const cat = el.getAttribute("data-cat");
      el.querySelector("text").textContent =
        (provOpen[cat] ? "▾ " : "▸ ") + el.getAttribute("data-name") + " — " + el.getAttribute("data-summary");
      el.querySelector("text").setAttribute("y", y(slot++));
    } else if (provOpen[el.getAttribute("data-cat")]) {
      el.style.display = "";
      el.setAttribute("y", y(slot++));
    } else {
      el.style.display = "none";
    }
  });
  /* Contender lines live in the first plot's series groups. */
  DATA.names.forEach((_, i) => {
    document.getElementById("series-0-" + i).querySelectorAll(".series-prov").forEach(t => {
      if (on[i]) t.setAttribute("y", (DATA.provTop + 40 + slot++ * DATA.provLine).toFixed(1));
    });
  });
  const h = DATA.provTop + 40 + slot * DATA.provLine + 8;
  const svgEl = document.querySelector("svg");
  svgEl.setAttribute("height", h.toFixed(0));
  svgEl.setAttribute("viewBox", `0 0 ${DATA.svgWidth} ${h.toFixed(0)}`);
}

function highlightSeries(i, active) {
  DATA.plots.forEach((plot, p) => {
    for (let j = 0; j < DATA.names.length; j++) {
      if (!plot.series[j]) continue;
      const series = document.getElementById("series-" + p + "-" + j);
      const dots = document.getElementById("dots-" + p + "-" + j);
      const dim = active && j !== i && on[j];
      series.setAttribute("data-dim", dim ? "true" : "false");
      series.setAttribute("data-hl", active && j === i ? "true" : "false");
      if (dots) dots.setAttribute("data-dim", dim ? "true" : "false");
    }
  });
}

function toggleSeries(i) {
  on[i] = !on[i];
  relayout();
  if (hovered) showHover(hovered[0], hovered[1], hovered[2]);
}

/* The dot the panel describes ([plot, series, point]), so a toggle can rebuild the panel in place. */
let hovered = null;
/* The dot a tap pinned the panel to; a mouse leaving a dot then leaves the panel up. */
let pinned = null;

function rate(ns, p) {
  const t = DATA.plots[p].scale / ns;
  return (t >= 10 ? t.toFixed(0) : t.toFixed(1)) + " " + DATA.plots[p].rateUnit;
}

function markGlyph(mark, color) {
  const stroke = ["stroke", "#fdfdfc"], sw = ["stroke-width", "1.5"];
  let el;
  if (mark === "diamond") { el = document.createElementNS(NS, "path"); el.setAttribute("d", "M 0 -6.25 L 6.25 0 L 0 6.25 L -6.25 0 Z"); }
  else if (mark === "square") { el = document.createElementNS(NS, "rect"); el.setAttribute("x", -4.5); el.setAttribute("y", -4.5); el.setAttribute("width", 9); el.setAttribute("height", 9); }
  else if (mark === "triangle") { el = document.createElementNS(NS, "path"); el.setAttribute("d", "M 0 -6.5 L 6.5 3.25 L -6.5 3.25 Z"); }
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
 * Hovering a dot: the hovered contender's median and range at that point,
 * then every visible contender of that plot ranked fastest first, each
 * with its speed relative to the hovered one. Hidden contenders stay out
 * of the ranking.
 */
function showHover(p, focus, k) {
  hovered = [p, focus, k];
  highlightSeries(focus, true);
  const plot = DATA.plots[p];
  const mapY = currentMapY[p];
  if (!on[focus] || !mapY || !plot.series[focus]) { document.getElementById("hover").style.display = "none"; return; }
  const body = document.getElementById("hover-body");
  while (body.firstChild) body.removeChild(body.firstChild);

  const f = plot.series[focus];
  const name = i => DATA.names[i];
  const rows = plot.series.map((s, i) => s ? [i, s.med[k]] : null).filter(r => r && on[r[0]]).sort((a, b) => a[1] - b[1]);

  const PAD = 10, LINE = 16, W = 350;
  let y = PAD + 12;
  body.appendChild(textEl(PAD, y, "hover-head", `${name(focus)} at ${plot.sizes[k]}`));
  y += 14;
  const spread = (f.high[k] - f.low[k]) / f.med[k];
  const spreadNote = spread >= DATA.spreadWide ? " · median poorly determined"
    : spread >= DATA.spreadNoticeable ? " · median less certain" : "";
  /* In the rate unit the fastest sample (min time) is the top of the range. */
  const asc = (a, b) => unit === "ns" ? [a, b] : [b, a];
  const [cLo, cHi] = asc(f.low[k], f.high[k]);
  const [rLo, rHi] = asc(f.min[k], f.max[k]);
  const rangeRow = textEl(PAD, y, "hover-sub",
    `median ${fmt(f.med[k], p)} ${unitLabel(p)} (${fmtOther(f.med[k], p)}) · 95% interval ${fmt(cLo, p)}–${fmt(cHi, p)} (±${(spread * 50).toFixed(1)}%)${spreadNote}`);
  if (spread >= DATA.spreadWide) rangeRow.setAttribute("fill", "#b45309");
  body.appendChild(rangeRow);
  y += 13;
  if (f.duoMed) {
    const [dLo, dHi] = asc(f.duoLow[k], f.duoHigh[k]);
    const ratio = f.duoMed[k] / f.med[k];
    const note = ratio > 1.05 ? ` · ${((ratio - 1) * 100).toFixed(0)}% slower beside a copy of itself`
      : ratio < 0.95 ? ` · ${((1 - ratio) * 100).toFixed(0)}% faster beside a copy of itself` : " · unchanged beside a copy of itself";
    const duoRow = textEl(PAD, y, "hover-sub",
      `duo ${fmt(f.duoMed[k], p)} ${unitLabel(p)} (${fmtOther(f.duoMed[k], p)}) · 95% interval ${fmt(dLo, p)}–${fmt(dHi, p)}${note}`);
    if (ratio > 1.05) duoRow.setAttribute("fill", "#b45309");
    body.appendChild(duoRow);
    y += 13;
  }
  const modes = f.modes[k];
  if (modes) {
    const [mLo, mHi] = asc(modes[0], modes[2]);
    const [nLo, nHi] = asc(modes[1], modes[3]);
    const modeRow = textEl(PAD, y, "hover-sub",
      `two speeds: ${nLo} samples near ${fmt(mLo, p)}, ${nHi} near ${fmt(mHi, p)} ${unitLabel(p)} · extremes ${fmt(rLo, p)}–${fmt(rHi, p)}`);
    modeRow.setAttribute("fill", "#b45309");
    body.appendChild(modeRow);
  } else {
    body.appendChild(textEl(PAD, y, "hover-sub", `extremes ${fmt(rLo, p)}–${fmt(rHi, p)} ${unitLabel(p)} over ${f.n[k]} samples`));
  }

  /* Code path at this point; the first point of a new path explains why. */
  let ri = 0;
  f.kernels.forEach((r, j) => { if (k >= r.from) ri = j; });
  const kernel = f.kernels[ri];
  const isTransition = ri > 0 && kernel.from === k;
  y += 14;
  const pathRow = textEl(PAD, y, "hover-sub", "");
  const shape = markGlyph(kernel.mark, DATA.colors[focus]);
  shape.setAttribute("transform", `translate(${PAD + 5} ${y - 3.5}) scale(0.8)`);
  body.appendChild(shape);
  pathRow.setAttribute("x", PAD + 14);
  pathRow.textContent = (isTransition ? "new path from here: " : "code path: ") + kernel.name;
  if (isTransition) pathRow.setAttribute("class", "hover-sub hover-path");
  body.appendChild(pathRow);
  if (isTransition) {
    for (const line of wrapText(kernel.why, 62)) {
      y += 13;
      body.appendChild(textEl(PAD + 14, y, "hover-why", line));
    }
  }
  y += 10;

  if (rows.length > 1) {
    y += LINE;
    body.appendChild(textEl(PAD, y, "hover-sub", "contender"));
    body.appendChild(textEl(PAD + 150, y, "hover-sub", unitLabel(p), { "text-anchor": "end" }));
    body.appendChild(textEl(PAD + 215, y, "hover-sub", otherUnitLabel(p), { "text-anchor": "end" }));
    body.appendChild(textEl(W - PAD, y, "hover-sub", `relative to ${name(focus)}`, { "text-anchor": "end" }));
    y += 4;
    for (const [i, med] of rows) {
      y += LINE;
      const s = plot.series[i];
      /* Swatch: this contender's mark at this point, in its own colour. */
      let rj = 0;
      s.kernels.forEach((r, j) => { if (k >= r.from) rj = j; });
      const sw = markGlyph(s.kernels[rj].mark, DATA.colors[i]);
      sw.setAttribute("class", "hover-swatch");
      sw.setAttribute("style", `fill: ${DATA.colors[i]}`);
      sw.setAttribute("transform", `translate(${PAD + 5} ${y - 4}) scale(0.85)`);
      body.appendChild(sw);
      const cls = "hover-row" + (i === focus ? " hover-row-focus" : "");
      body.appendChild(textEl(PAD + 15, y, cls, name(i)));
      body.appendChild(textEl(PAD + 150, y, cls, fmt(med, p), { "text-anchor": "end" }));
      body.appendChild(textEl(PAD + 215, y, cls, fmtOther(med, p).replace(" " + otherUnitLabel(p), ""), { "text-anchor": "end" }));
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
    body.appendChild(textEl(PAD, y, "hover-note", `each row's speed compared with ${name(focus)}; medians, ranked fastest first`));
    y += 4;
  }
  const H = y + PAD - 6;

  /* Place beside the column, flipping left near the right edge. */
  const x = plot.x[k];
  const dotY = mapY(f.med[k]);
  let bx = x + 14;
  if (bx + W > DATA.plotRight + 10) bx = x - 14 - W;
  let by = Math.min(Math.max(dotY - H / 2, plot.top - 30), plot.bottom + 30 - H);

  const box = document.getElementById("hover-box");
  box.setAttribute("x", bx); box.setAttribute("y", by);
  box.setAttribute("width", W); box.setAttribute("height", H);
  body.setAttribute("transform", `translate(${bx} ${by})`);
  const guide = document.getElementById("hover-guide");
  guide.setAttribute("x1", x); guide.setAttribute("x2", x);
  guide.setAttribute("y1", plot.top); guide.setAttribute("y2", plot.bottom);
  document.getElementById("hover").style.display = "";
}

function hideHover() {
  hovered = null;
  highlightSeries(0, false);
  document.getElementById("hover").style.display = "none";
}

/*
 * Two inputs, one panel. A mouse hovers: entering a dot shows the panel,
 * leaving hides it, unless a click pinned it. A finger taps: pointerenter
 * fires too, without a matching leave, so touch is handled by tap alone.
 * Tapping a dot pins the panel to it; tapping it again, or the background,
 * clears it. Name highlighting follows the mouse only, since a finger
 * has no way to leave.
 */
function hoverDot(event, p, i, k) { if (event.pointerType === "mouse") showHover(p, i, k); }
function leaveDot(event) { if (event.pointerType === "mouse" && !pinned) hideHover(); }
function tapDot(event, p, i, k) {
  event.stopPropagation();
  if (pinned && pinned[0] === p && pinned[1] === i && pinned[2] === k) { pinned = null; hideHover(); return; }
  pinned = [p, i, k];
  showHover(p, i, k);
}
function tapAway() { pinned = null; hideHover(); }
function hoverLabel(event, i, active) { if (event.pointerType === "mouse") highlightSeries(i, active); }

window.toggleSeries = toggleSeries;
window.toggleProv = toggleProv;
window.hoverDot = hoverDot;
window.leaveDot = leaveDot;
window.tapDot = tapDot;
window.tapAway = tapAway;
window.hoverLabel = hoverLabel;
window.setUnit = setUnit;
window.flipUnit = flipUnit;
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

fn format_gbps_tick(value: f64) -> String {
    assert!(
        value > 0.0,
        "log-axis ticks must be positive"
    );

    if value >= 10.0 {
        format!("{value:.0}")
    } else {
        format!("{value:.1}")
    }
}

/// A measured value as a bare rate number, as the graph's value labels
/// show it in the default unit: whole numbers at 10 and above, one
/// decimal below.
fn format_rate_value(ps: PsPerByte, use_case: UseCase) -> String {
    assert!(ps > 0);
    let rate = (1000 * use_case.rate_scale()) as f64 / ps as f64;
    if rate >= 10.0 {
        format!("{rate:.0}")
    } else {
        format!("{rate:.1}")
    }
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

#[cfg(test)]
mod correctness_tests {
    use super::*;

    #[test]
    fn same_input_agrees_across_available_implementations() {
        let algorithms: Vec<_> = Algorithm::ALL.into_iter()
            .filter(|a| a.availability().is_ok()).collect();
        for &(len, seed, _) in test_vectors::VECTORS {
            check_input(&algorithms, &make_input_seeded(len, seed), 1, seed);
        }
    }

    #[test]
    fn every_batch_size_has_a_golden_vector_and_a_dispatch_arm() {
        let algorithms: Vec<_> = Algorithm::ALL.into_iter()
            .filter(|a| a.availability().is_ok()).collect();
        for point in &POINTS[UseCase::ManyMessages.points()] {
            for seed in [0, 1] {
                check_input(&algorithms, &make_input_seeded(point.bytes, seed), point.messages, seed);
            }
        }
        assert_eq!(test_vectors::MANY_VECTORS.len(), 2 * BATCH_COUNT, "two seeds per batch size");
    }

    #[test]
    fn use_case_axes_are_contiguous_and_cover_every_point() {
        assert_eq!(UseCase::OneMessage.points(), 0..INPUT_COUNT);
        assert_eq!(UseCase::ManyMessages.points(), INPUT_COUNT..POINT_COUNT);
        assert!(POINTS[UseCase::ManyMessages.points()].iter().all(|p| p.bytes == p.messages * MESSAGE_LEN));
    }

    #[test]
    fn batch_observes_every_digest_and_matches_empty_vectors() {
        for (algorithm, expected) in [
            (Algorithm::Blake3, "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262"),
            (Algorithm::Sha256, "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"),
            (Algorithm::Sha1Dc, "da39a3ee5e6b4b0d3255bfef95601890afd80709"),
        ] {
            let mut calls = 0;
            hash_batch(algorithm, &[], 1, 3, |digest| {
                let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
                assert_eq!(hex, expected);
                calls += 1;
            });
            assert_eq!(calls, 3);
        }
    }

    #[test]
    #[should_panic(expected = "blake3-servil (BLAKE3) disagrees with golden vector on 65 input bytes, seed 0")]
    fn digest_mismatch_fails_stop_with_context() {
        assert_digest_matches(Algorithm::Blake3Servil, 65, 1, 0, &[0; 32], &[1; 32]);
    }
}
