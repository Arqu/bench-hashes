//! Temporary diagnostic: why does CommonCrypto's `CC_SHA256` cost so much
//! more than the sha2 crate at small inputs on Apple silicon?
//!
//! The main benchmark measured (M4 Max, 2026-09-21) a fixed cost of about
//! 150 ns per CC_SHA256 call against about 15 ns for sha2, with CommonCrypto
//! the faster of the two per byte in bulk. This binary separates the
//! candidate explanations in one run:
//!
//!   A. Per-call fixed cost, measured directly on empty and tiny inputs.
//!   B. Dynamic-library call overhead: the cost of calling a trivial libSystem
//!      function through the same kind of boundary (dlsym'd pointer, PLT).
//!   C. Data Independent Timing: whether CC_SHA256 changes the DIT bit, what
//!      a DIT toggle costs on this core, and whether the gap shrinks when
//!      DIT is already on before the call (a DIT-aware implementation skips
//!      the toggle when it finds the bit set).
//!   D. Streaming API: CC_SHA256_Init/Update/Final on the same input. If the
//!      fixed cost is per public entry point, three calls pay it three times.
//!   E. Digest context lifecycle: the streaming API split into Init, Update,
//!      Final, timed separately, shows where within a digest the time goes.
//!   F. Alternative entry points that reach the same corecrypto code by a
//!      different route: CCDigest() with kCCDigestSHA256, and Security
//!      framework's SecDigestTransform is out of scope, but CNG-style
//!      CCDigestCreate/Update/Final shows whether the CC_SHA256 wrapper
//!      specifically is at fault.
//!   G. Warm vs cold: the first call after a pause, to see whether lazy
//!      binding or a one-time implementation lookup is being re-paid.
//!
//! Every measurement reports median and min–max over many samples, in
//! ns per call, and the report ends with the explanation the numbers support.
//!
//! Apple only. Build and run with `cargo run --release --bin cc-diagnose`.

#[cfg(not(target_vendor = "apple"))]
fn main() {
    eprintln!("cc-diagnose measures Apple's CommonCrypto and runs on Apple platforms only.");
    std::process::exit(2);
}

#[cfg(target_vendor = "apple")]
fn main() {
    apple::main();
}

#[cfg(target_vendor = "apple")]
mod apple {
use sha2::{Digest, Sha256};
use std::hint::black_box;
use std::time::Instant;

const DIGEST_LEN: usize = 32;
const SAMPLES: usize = 200;
const TARGET_SAMPLE_NS: f64 = 500_000.0;

#[repr(C)]
struct CcSha256Ctx {
    // CC_SHA256_CTX: uint32 count[2]; uint32 hash[8]; uint32 wbuf[16];
    count: [u32; 2],
    hash: [u32; 8],
    wbuf: [u32; 16],
}

#[allow(non_camel_case_types)]
type CCDigestRef = *mut std::ffi::c_void;

unsafe extern "C" {
    fn CC_SHA256(data: *const u8, len: u32, md: *mut u8) -> *mut u8;
    fn CC_SHA256_Init(ctx: *mut CcSha256Ctx) -> i32;
    fn CC_SHA256_Update(ctx: *mut CcSha256Ctx, data: *const u8, len: u32) -> i32;
    fn CC_SHA256_Final(md: *mut u8, ctx: *mut CcSha256Ctx) -> i32;

    // CommonDigestSPI.h (private but exported; present since 10.7).
    fn CCDigest(algorithm: u32, data: *const u8, length: usize, output: *mut u8) -> i32;
    fn CCDigestCreate(algorithm: u32) -> CCDigestRef;
    fn CCDigestUpdate(ctx: CCDigestRef, data: *const u8, length: usize) -> i32;
    fn CCDigestFinal(ctx: CCDigestRef, output: *mut u8) -> i32;
    fn CCDigestDestroy(ctx: CCDigestRef);

    // Trivial libSystem calls with no work of their own, for boundary cost.
    // (getpid is a real syscall on current macOS, so it is avoided.)
    fn abs(value: i32) -> i32;
    fn strlen(s: *const std::ffi::c_char) -> usize;
    // A real corecrypto-backed CommonCrypto call whose work is tiny: the
    // random-generator accessor path is not suitable, so use CC_SHA256_Init,
    // which only writes eight constants and two counters.
}

const K_CC_DIGEST_SHA256: u32 = 10;

// ── DIT ───────────────────────────────────────────────────────────────────

/// Read the DIT bit (PSTATE.DIT). Requires FEAT_DIT, present on every
/// Apple silicon core.
#[inline(always)]
fn read_dit() -> bool {
    let value: u64;
    unsafe { std::arch::asm!("mrs {}, DIT", out(reg) value, options(nomem, nostack)) };
    (value >> 24) & 1 == 1
}

#[inline(always)]
fn set_dit(on: bool) {
    unsafe {
        if on {
            std::arch::asm!("msr DIT, #1", options(nomem, nostack));
        } else {
            std::arch::asm!("msr DIT, #0", options(nomem, nostack));
        }
    }
}

// ── timing ────────────────────────────────────────────────────────────────

struct Stat {
    median: f64,
    min: f64,
    max: f64,
}

/// Time `work` per call: calibrate an iteration count for ~0.5 ms samples,
/// then take SAMPLES samples and report ns per call.
fn measure<F: FnMut()>(mut work: F) -> Stat {
    let mut iterations = 1usize;
    loop {
        let start = Instant::now();
        for _ in 0..iterations {
            work();
        }
        let ns = start.elapsed().as_nanos() as f64;
        if ns >= TARGET_SAMPLE_NS / 4.0 {
            iterations = ((iterations as f64) * TARGET_SAMPLE_NS / ns).ceil().max(1.0) as usize;
            break;
        }
        iterations *= 4;
    }
    // warm
    for _ in 0..iterations {
        work();
    }
    let mut samples = Vec::with_capacity(SAMPLES);
    for _ in 0..SAMPLES {
        let start = Instant::now();
        for _ in 0..iterations {
            work();
        }
        samples.push(start.elapsed().as_nanos() as f64 / iterations as f64);
    }
    samples.sort_by(f64::total_cmp);
    Stat {
        median: samples[samples.len() / 2],
        min: samples[0],
        max: samples[samples.len() - 1],
    }
}

fn row(label: &str, stat: &Stat) {
    println!(
        "  {label:<58} {:>9.1} ns   ({:.1}–{:.1})",
        stat.median, stat.min, stat.max
    );
}

fn note(text: &str) {
    println!("  {text}");
}

// ── hash wrappers ─────────────────────────────────────────────────────────

fn cc_oneshot(input: &[u8]) -> [u8; DIGEST_LEN] {
    let mut out = [0u8; DIGEST_LEN];
    unsafe { CC_SHA256(input.as_ptr(), input.len() as u32, out.as_mut_ptr()) };
    out
}

fn cc_streaming(input: &[u8]) -> [u8; DIGEST_LEN] {
    let mut ctx = CcSha256Ctx { count: [0; 2], hash: [0; 8], wbuf: [0; 16] };
    let mut out = [0u8; DIGEST_LEN];
    unsafe {
        CC_SHA256_Init(&mut ctx);
        CC_SHA256_Update(&mut ctx, input.as_ptr(), input.len() as u32);
        CC_SHA256_Final(out.as_mut_ptr(), &mut ctx);
    }
    out
}

fn cc_digest_spi(input: &[u8]) -> [u8; DIGEST_LEN] {
    let mut out = [0u8; DIGEST_LEN];
    unsafe { CCDigest(K_CC_DIGEST_SHA256, input.as_ptr(), input.len(), out.as_mut_ptr()) };
    out
}

fn cc_digest_spi_streaming(input: &[u8]) -> [u8; DIGEST_LEN] {
    let mut out = [0u8; DIGEST_LEN];
    unsafe {
        let ctx = CCDigestCreate(K_CC_DIGEST_SHA256);
        CCDigestUpdate(ctx, input.as_ptr(), input.len());
        CCDigestFinal(ctx, out.as_mut_ptr());
        CCDigestDestroy(ctx);
    }
    out
}

fn sha2_oneshot(input: &[u8]) -> [u8; DIGEST_LEN] {
    Sha256::digest(input).into()
}

pub fn main() {
    let sizes: [(usize, &str); 7] = [
        (0, "0 B"),
        (1, "1 B"),
        (55, "55 B (one block, longest fit)"),
        (56, "56 B (two blocks from padding)"),
        (64, "64 B"),
        (1024, "1 KiB"),
        (65536, "64 KiB"),
    ];
    let inputs: Vec<Vec<u8>> = sizes.iter().map(|(n, _)| (0..*n).map(|i| (i * 7 + 3) as u8).collect()).collect();

    println!("CommonCrypto SHA-256 latency diagnosis");
    println!("all figures: ns per call, median (min–max) over {SAMPLES} samples of ~0.5 ms\n");

    // correctness first
    for input in &inputs {
        let reference = sha2_oneshot(input);
        assert_eq!(cc_oneshot(input), reference, "CC_SHA256 disagrees at {} B", input.len());
        assert_eq!(cc_streaming(input), reference, "CC_SHA256 streaming disagrees at {} B", input.len());
        assert_eq!(cc_digest_spi(input), reference, "CCDigest disagrees at {} B", input.len());
        assert_eq!(cc_digest_spi_streaming(input), reference, "CCDigest streaming disagrees at {} B", input.len());
    }
    println!("All four CommonCrypto paths agree with the sha2 crate on every input.\n");

    // ── A. fixed cost ────────────────────────────────────────────────────
    println!("A. Per-call cost by input size");
    println!("  {:<58} {:>12}   {}", "", "median", "min–max");
    let mut cc_by_size = Vec::new();
    let mut sha2_by_size = Vec::new();
    for (input, (_, label)) in inputs.iter().zip(sizes.iter()) {
        let cc = measure(|| { black_box(cc_oneshot(black_box(input))); });
        let s2 = measure(|| { black_box(sha2_oneshot(black_box(input))); });
        row(&format!("CC_SHA256          {label}"), &cc);
        row(&format!("sha2 crate         {label}"), &s2);
        cc_by_size.push(cc.median);
        sha2_by_size.push(s2.median);
    }
    let cc_fixed = cc_by_size[0];
    let sha2_fixed = sha2_by_size[0];
    let cc_slope = (cc_by_size[6] - cc_by_size[5]) / (65536.0 - 1024.0);
    let sha2_slope = (sha2_by_size[6] - sha2_by_size[5]) / (65536.0 - 1024.0);
    println!();
    note(&format!("Fixed cost (0 B input):  CC_SHA256 {cc_fixed:.1} ns   sha2 {sha2_fixed:.1} ns   gap {:.1} ns", cc_fixed - sha2_fixed));
    note(&format!("Bulk rate (1 KiB→64 KiB): CC_SHA256 {cc_slope:.3} ns/B   sha2 {sha2_slope:.3} ns/B"));
    note(&format!("55 B→56 B step (one extra compression): CC {:.1} ns, sha2 {:.1} ns",
        cc_by_size[3] - cc_by_size[2], sha2_by_size[3] - sha2_by_size[2]));
    println!();

    // ── B. dylib boundary ────────────────────────────────────────────────
    println!("B. Cost of a libSystem call boundary with no work behind it");
    let ab = measure(|| { black_box(unsafe { abs(black_box(-7)) }); });
    row("abs() through libSystem", &ab);
    let s = black_box(c"twelve chars".as_ptr());
    let sl = measure(|| { black_box(unsafe { strlen(black_box(s)) }); });
    row("strlen(12 chars) through libSystem", &sl);
    // Function pointer call inside this binary, for the indirect-call floor.
    let f: fn(&[u8]) -> [u8; DIGEST_LEN] = black_box(sha2_oneshot);
    let indirect = measure(|| { black_box(f(black_box(&inputs[0]))); });
    row("sha2 0 B through a function pointer (indirect call floor)", &indirect);
    println!();

    // ── C. DIT ───────────────────────────────────────────────────────────
    println!("C. Data Independent Timing (PSTATE.DIT)");
    let dit_before = read_dit();
    note(&format!("DIT before any CommonCrypto call: {}", dit_before));
    set_dit(false);
    unsafe { CC_SHA256(inputs[4].as_ptr(), 64, [0u8; 32].as_mut_ptr()) };
    let dit_after = read_dit();
    note(&format!("DIT after CC_SHA256 returns (started clear): {dit_after}  → {}",
        if dit_after { "CC_SHA256 LEAVES DIT SET" } else { "restored or never set" }));

    let toggle = measure(|| { set_dit(true); black_box(read_dit()); set_dit(false); });
    row("set DIT, read, clear (one round trip)", &toggle);
    let msr_only = measure(|| { set_dit(true); set_dit(false); });
    row("msr DIT #1; msr DIT #0 (two writes, no read)", &msr_only);

    set_dit(false);
    let cc_dit_off = measure(|| { black_box(cc_oneshot(black_box(&inputs[4]))); });
    row("CC_SHA256 64 B, DIT clear on entry", &cc_dit_off);
    set_dit(true);
    let cc_dit_on = measure(|| { black_box(cc_oneshot(black_box(&inputs[4]))); });
    row("CC_SHA256 64 B, DIT already set on entry", &cc_dit_on);
    let sha2_dit_on = measure(|| { black_box(sha2_oneshot(black_box(&inputs[4]))); });
    row("sha2 64 B, DIT set (does the bit itself slow the kernel?)", &sha2_dit_on);
    set_dit(false);
    let sha2_dit_off = measure(|| { black_box(sha2_oneshot(black_box(&inputs[4]))); });
    row("sha2 64 B, DIT clear", &sha2_dit_off);
    println!();
    let dit_saving = cc_dit_off.median - cc_dit_on.median;
    note(&format!("CC_SHA256 saves {dit_saving:.1} ns when DIT is already set → {}",
        if dit_saving > 20.0 { "it toggles DIT and skips the toggle when set: DIT is a real part of the gap" }
        else if dit_after { "it sets DIT unconditionally (no skip); toggle cost is in the round-trip row" }
        else { "DIT handling is not a measurable part of the gap" }));
    println!();

    // ── D/E. streaming and its phases ────────────────────────────────────
    println!("D. Streaming API on 64 B (three public calls)");
    let stream = measure(|| { black_box(cc_streaming(black_box(&inputs[4]))); });
    row("CC_SHA256_Init + Update + Final, 64 B", &stream);
    let one = measure(|| { black_box(cc_oneshot(black_box(&inputs[4]))); });
    row("CC_SHA256 one-shot, 64 B (for comparison)", &one);
    println!();
    println!("E. Streaming phases in isolation");
    let mut ctx = CcSha256Ctx { count: [0; 2], hash: [0; 8], wbuf: [0; 16] };
    let init = measure(|| { unsafe { CC_SHA256_Init(black_box(&mut ctx)) }; });
    row("CC_SHA256_Init alone", &init);
    unsafe { CC_SHA256_Init(&mut ctx) };
    let upd_small = measure(|| { unsafe { CC_SHA256_Update(black_box(&mut ctx), inputs[1].as_ptr(), 1) }; });
    row("CC_SHA256_Update 1 B (buffered, no compression most calls)", &upd_small);
    let upd_block = measure(|| {
        unsafe { CC_SHA256_Init(&mut ctx); CC_SHA256_Update(black_box(&mut ctx), inputs[4].as_ptr(), 64) };
    });
    row("Init + Update 64 B (one compression)", &upd_block);
    let mut out = [0u8; 32];
    let fin = measure(|| {
        unsafe { CC_SHA256_Init(&mut ctx); CC_SHA256_Final(out.as_mut_ptr(), black_box(&mut ctx)) };
    });
    row("Init + Final (pads and compresses once, zeroes context)", &fin);
    println!();
    let per_entry = (stream.median - one.median) / 2.0;
    note(&format!("Streaming costs {:.1} ns more than one-shot for two extra entry points → ≈{per_entry:.1} ns per public call",
        stream.median - one.median));
    println!();

    // ── F. other routes to the same corecrypto code ──────────────────────
    println!("F. Other CommonCrypto entry points, 64 B");
    let spi = measure(|| { black_box(cc_digest_spi(black_box(&inputs[4]))); });
    row("CCDigest(kCCDigestSHA256) one-shot SPI", &spi);
    let spi_stream = measure(|| { black_box(cc_digest_spi_streaming(black_box(&inputs[4]))); });
    row("CCDigestCreate/Update/Final/Destroy (heap context)", &spi_stream);
    println!();

    // ── G. cold call ─────────────────────────────────────────────────────
    println!("G. First call after a pause (lazy binding / cold caches)");
    let mut colds = Vec::new();
    for _ in 0..5 {
        std::thread::sleep(std::time::Duration::from_millis(50));
        let start = Instant::now();
        black_box(cc_oneshot(black_box(&inputs[4])));
        colds.push(start.elapsed().as_nanos() as f64);
    }
    colds.sort_by(f64::total_cmp);
    note(&format!("CC_SHA256 64 B after 50 ms idle, 5 trials: min {:.0} ns, median {:.0} ns, max {:.0} ns (warm median {:.1} ns)",
        colds[0], colds[2], colds[4], one.median));
    println!();

    // ── verdict ──────────────────────────────────────────────────────────
    println!("VERDICT");
    let gap = cc_fixed - sha2_fixed;
    let boundary = ab.median.max(sl.median);
    let dit_part = if dit_saving > 20.0 { dit_saving } else if dit_after { toggle.median } else { 0.0 };
    let unexplained = gap - boundary - dit_part;
    note(&format!("Fixed-cost gap to explain: {gap:.1} ns per call."));
    note(&format!("  dylib call boundary:     ≈{boundary:.1} ns   ({:.0}%)", boundary / gap * 100.0));
    note(&format!("  DIT toggle:              ≈{dit_part:.1} ns   ({:.0}%)", dit_part / gap * 100.0));
    note(&format!("  remaining (context init/final, dispatch, zeroing, extra copies): ≈{unexplained:.1} ns   ({:.0}%)",
        unexplained / gap * 100.0));
    note(&format!("Per public entry point (from streaming vs one-shot): ≈{per_entry:.1} ns."));
    if cc_slope < sha2_slope {
        note(&format!("Per byte, CommonCrypto is the faster kernel ({cc_slope:.3} vs {sha2_slope:.3} ns/B); the gap is entirely fixed per-call overhead."));
    } else {
        note("Per byte the two are close; the gap is fixed per-call overhead.");
    }
    let crossover = if cc_slope < sha2_slope { gap / (sha2_slope - cc_slope) } else { f64::INFINITY };
    note(&format!("Break-even input size: ≈{crossover:.0} B; above this CommonCrypto wins."));
}
}
