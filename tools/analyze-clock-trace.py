#!/usr/bin/env python3
"""Find windows where the thread CPU clock ran at a different rate from the
hardware counter, in a bench-hashes --trace-clocks CSV.

Each sample carries wall_ns (Instant), thread_cpu_ns, mach_ticks, and
process_cpu_ns read around the same work. For an uninterrupted single
thread, cpu/wall is 1.000 to within the clocks' read cost. The script
reports every sample outside a tolerance, groups consecutive ones into
windows, and for each window prints its length, the common ratio, and
what was running, so the shape of a disturbance is visible:

  one sample, cpu << wall          preemption; wall counted the gap
  one sample, cpu < wall by a bit  a slice billed short (under-billing)
  many consecutive, same ratio     the CPU clock ran at a different RATE
                                   for the window's duration
  many consecutive, both short     the work itself ran faster

Usage: analyze-clock-trace.py TRACE.csv [--tolerance 0.02]
"""

import csv
import sys
from collections import defaultdict


def report_frequency(rows, cell_wall_median):
    """The clock frequency each sample ran at, from per-perf-level cycles
    and time, and how it relates to the sample's wall time."""
    on_e = [r for r in rows if r["e_time"] > r["p_time"]]
    mixed = [r for r in rows if r["e_time"] and r["p_time"] and r["e_time"] <= r["p_time"]]
    print(f"\nCore placement: {len(rows) - len(on_e) - len(mixed)} samples wholly on P-cores, "
          f"{len(mixed)} split P/E (mostly P), {len(on_e)} mostly or wholly on E-cores")

    freqs = []
    for r in rows:
        cyc = r["p_cycles"] + r["e_cycles"]
        t = r["p_time"] + r["e_time"]
        r["ghz"] = cyc / t if t else 0.0          # cycles per ns = GHz
        r["ipc"] = (r["p_instr"] + r["e_instr"]) / cyc if cyc else 0.0
        if r["ghz"]:
            freqs.append(r["ghz"])
    if not freqs:
        return
    freqs.sort()
    n = len(freqs)
    print(f"Clock frequency over the run (cycles ÷ CPU time): min {freqs[0]:.3f} GHz, "
          f"p5 {freqs[n // 20]:.3f}, median {freqs[n // 2]:.3f}, p95 {freqs[n - 1 - n // 20]:.3f}, max {freqs[-1]:.3f}")
    med = freqs[n // 2]
    fast = [r for r in rows if r["ghz"] > med * 1.05]
    slow = [r for r in rows if r["ghz"] and r["ghz"] < med * 0.95]
    print(f"  {len(fast)} samples ran >5% above the median frequency, {len(slow)} ran >5% below")

    # Does frequency explain the wall-time extremes? For each cell's fastest
    # and slowest sample, show its frequency next to the cell median.
    print("\nPer-cell wall extremes with the frequency they ran at (cell median frequency in brackets):")
    cells = {}
    for r in rows:
        cells.setdefault((r["contender"], r["size"]), []).append(r)
    print(f"  {'contender':<12} {'size':>8}  {'fastest wall/med':>16} {'@GHz':>6}   {'slowest wall/med':>16} {'@GHz':>6}   [cell GHz]")
    for (c, sz), rs in sorted(cells.items(), key=lambda kv: (kv[0][1], kv[0][0])):
        rs_f = sorted(r["ghz"] for r in rs if r["ghz"])
        cell_ghz = rs_f[len(rs_f) // 2] if rs_f else 0
        med_w = cell_wall_median[(c, sz)]
        fmin = min(rs, key=lambda r: r["wall"])
        fmax = max(rs, key=lambda r: r["wall"])
        if fmin["wall"] / med_w < 0.95 or fmax["wall"] / med_w > 1.08:
            print(f"  {c:<12} {sz:>8}  {fmin['wall'] / med_w:16.3f} {fmin['ghz']:6.2f}   "
                  f"{fmax['wall'] / med_w:16.3f} {fmax['ghz']:6.2f}   [{cell_ghz:.2f}]")

    # Consecutive windows of off-median frequency: the shape of a boost or throttle.
    off = [r for r in rows if r["ghz"] and abs(r["ghz"] / med - 1) > 0.05]
    if off:
        windows = []
        cur = [off[0]]
        for a, b in zip(off, off[1:]):
            if b["round"] == a["round"] and b["position"] == a["position"] + 1:
                cur.append(b)
            else:
                windows.append(cur)
                cur = [b]
        windows.append(cur)
        multi = [w for w in windows if len(w) >= 2]
        print(f"\nWindows of ≥2 consecutive samples at a frequency >5% off the median: {len(multi)}")
        for w in multi[:8]:
            g = [r["ghz"] for r in w]
            print(f"  round {w[0]['round']}, positions {w[0]['position']}–{w[-1]['position']}, {len(w)} samples, "
                  f"{sum(r['wall'] for r in w) / 1e6:.1f} ms: {min(g):.2f}–{max(g):.2f} GHz "
                  f"({'boost' if g[0] > med else 'throttle'}); contenders {', '.join(sorted(set(r['contender'] for r in w)))}; "
                  f"sizes {min(r['size'] for r in w)}–{max(r['size'] for r in w)} B")
        if len(multi) > 8:
            print(f"  … and {len(multi) - 8} more")
    print()


def main():
    if len(sys.argv) < 2:
        print(__doc__)
        sys.exit(2)
    path = sys.argv[1]
    tolerance = 0.02
    if "--tolerance" in sys.argv:
        tolerance = float(sys.argv[sys.argv.index("--tolerance") + 1])

    rows = []
    with open(path) as handle:
        for row in csv.DictReader(handle):
            rows.append({
                "round": int(row["round"]),
                "position": int(row["position"]),
                "contender": row["contender"],
                "size": int(row["size_bytes"]),
                "iterations": int(row["iterations"]),
                "wall": int(row["wall_ns"]),
                "cpu": int(row["thread_cpu_ns"]),
                "mach": int(row["mach_ticks"]),
                "proc": int(row["process_cpu_ns"]),
                "p_cycles": int(row.get("p_cycles", 0) or 0),
                "p_time": int(row.get("p_time_ns", 0) or 0),
                "e_cycles": int(row.get("e_cycles", 0) or 0),
                "e_time": int(row.get("e_time_ns", 0) or 0),
                "p_instr": int(row.get("p_instructions", 0) or 0),
                "e_instr": int(row.get("e_instructions", 0) or 0),
            })
    rows.sort(key=lambda r: (r["round"], r["position"]))
    n = len(rows)
    print(f"{n} samples from {path}")

    # Per-cell wall medians, so a sample's wall time can be judged "normal".
    cell_wall = defaultdict(list)
    for r in rows:
        cell_wall[(r["contender"], r["size"])].append(r["wall"])
    cell_wall_median = {k: sorted(v)[len(v) // 2] for k, v in cell_wall.items()}

    # mach tick → ns ratio, from the whole run (should be a constant, 125/3 on Apple silicon).
    mach_total = sum(r["mach"] for r in rows)
    wall_total = sum(r["wall"] for r in rows)
    if mach_total:
        print(f"mach_absolute_time ticks per wall ns over the run: {mach_total / wall_total:.6f} "
              f"(Apple silicon nominal 0.024 = 3/125)")

    # Frequency: cycles over CPU time, per core kind, when the trace has them.
    have_cycles = any(r["p_cycles"] or r["e_cycles"] for r in rows)
    if have_cycles:
        report_frequency(rows, cell_wall_median)

    # Classify each sample.
    for r in rows:
        r["ratio"] = r["cpu"] / r["wall"]
        r["wall_vs_cell"] = r["wall"] / cell_wall_median[(r["contender"], r["size"])]
        r["odd"] = abs(r["ratio"] - 1.0) > tolerance

    odd = [r for r in rows if r["odd"]]
    print(f"{len(odd)} samples with |cpu/wall − 1| > {tolerance:.0%}")
    if not odd:
        print("The thread CPU clock tracked the hardware counter on every sample.")
        return

    # Group consecutive odd samples into windows.
    windows = []
    current = [odd[0]]
    for prev, cur in zip(odd, odd[1:]):
        consecutive = (cur["round"] == prev["round"] and cur["position"] == prev["position"] + 1) or (
            cur["round"] == prev["round"] + 1 and prev["position"] == max(x["position"] for x in rows) and cur["position"] == 0
        )
        if consecutive:
            current.append(cur)
        else:
            windows.append(current)
            current = [cur]
    windows.append(current)

    singles = [w for w in windows if len(w) == 1]
    multi = [w for w in windows if len(w) > 1]
    print(f"{len(windows)} window(s) of consecutive odd samples: {len(singles)} single-sample, {len(multi)} multi-sample\n")

    if singles:
        far = sum(1 for w in singles if w[0]["ratio"] < 0.5)
        near = len(singles) - far
        print(f"single-sample windows: {far} with cpu < 50% of wall (preemption; wall counted the gap, harmless),")
        print(f"                       {near} with cpu between 50% and {1 - tolerance:.0%} of wall (a slice billed short)")
        if near:
            print("  the billed-short ones:")
            for w in singles:
                r = w[0]
                if r["ratio"] >= 0.5:
                    print(f"    r{r['round']:>3} p{r['position']:>2}  {r['contender']:<12} {r['size']:>8} B  "
                          f"wall {r['wall']/1e6:7.3f} ms  cpu {r['cpu']/1e6:7.3f} ms  ratio {r['ratio']:.3f}  wall/cell {r['wall_vs_cell']:.3f}")
        print()

    for i, w in enumerate(multi, 1):
        ratios = [r["ratio"] for r in w]
        wall_dur = sum(r["wall"] for r in w) / 1e6
        cpu_dur = sum(r["cpu"] for r in w) / 1e6
        walls_normal = all(0.95 <= r["wall_vs_cell"] <= 1.05 for r in w)
        spread = max(ratios) - min(ratios)
        print(f"window {i}: round {w[0]['round']}, positions {w[0]['position']}–{w[-1]['position']}, "
              f"{len(w)} samples, {wall_dur:.1f} ms wall / {cpu_dur:.1f} ms cpu")
        print(f"  cpu/wall: min {min(ratios):.3f} median {sorted(ratios)[len(ratios)//2]:.3f} max {max(ratios):.3f}"
              f"  (spread {spread:.3f}) · wall vs cell median: "
              f"{min(r['wall_vs_cell'] for r in w):.3f}–{max(r['wall_vs_cell'] for r in w):.3f}")
        if len(w) == 1 and w[0]["ratio"] < 0.5:
            verdict = "single sample, cpu far below wall → preemption (wall counted the gap; expected, harmless)"
        elif len(w) == 1:
            verdict = "single sample, cpu modestly below wall → one slice billed short"
        elif spread < 0.03 and walls_normal:
            verdict = ("CONSECUTIVE SAMPLES AT ONE RATIO WITH NORMAL WALL TIMES → the CPU clock ran at "
                       f"{sorted(ratios)[len(ratios)//2]:.1%} of wall rate for this window; the work did not change speed")
        elif spread < 0.03 and not walls_normal:
            verdict = "consecutive samples at one ratio AND wall times shifted → the work itself changed speed (both clocks agree)"
        else:
            verdict = "consecutive samples with varying ratios → repeated interruptions"
        print(f"  → {verdict}")
        print("  samples:")
        for r in w:
            print(f"    r{r['round']:>3} p{r['position']:>2}  {r['contender']:<12} {r['size']:>8} B  "
                  f"wall {r['wall']/1e6:7.3f} ms  cpu {r['cpu']/1e6:7.3f} ms  ratio {r['ratio']:.3f}  "
                  f"wall/cell {r['wall_vs_cell']:.3f}"
                  + (f"  mach/wall {r['mach']/r['wall']:.4f}" if r["mach"] else ""))
        print()

    # Neighbours of the largest window, for context.
    big = max(windows, key=len)
    if len(big) >= 2:
        first = rows.index(big[0])
        last = rows.index(big[-1])
        print(f"context around the largest window (2 samples either side):")
        for r in rows[max(0, first - 2):min(n, last + 3)]:
            mark = "*" if r["odd"] else " "
            print(f"  {mark} r{r['round']:>3} p{r['position']:>2}  {r['contender']:<12} {r['size']:>8} B  "
                  f"wall {r['wall']/1e6:7.3f} ms  cpu {r['cpu']/1e6:7.3f} ms  ratio {r['ratio']:.3f}")


if __name__ == "__main__":
    main()
