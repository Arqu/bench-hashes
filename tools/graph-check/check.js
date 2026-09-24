// Drive the graph's script in jsdom: zoom steps, in/out/all, unit switch
// during a zoom, toggles, hover; check positions, labels, and that no
// attribute holds NaN.
const fs = require("fs");
const { JSDOM } = require("jsdom");
const path = process.argv[2];
const svg = fs.readFileSync(path, "utf8").replace(/^<\?xml[^>]*>/, "");
const script = svg.match(/<script><!\[CDATA\[([\s\S]*)\]\]><\/script>/)[1];
const markup = svg.replace(/<script><!\[CDATA\[[\s\S]*\]\]><\/script>/, "");
const dom = new JSDOM(`<!DOCTYPE html><html><body>${markup}</body></html>`, { runScripts: "outside-only", pretendToBeVisual: true });
const w = dom.window;
w.eval(script + "\n;window.__on = on; window.__t = {DATA, ALLB, win: () => win, currentX, get zFrom() { return zFrom; }, get zTo() { return zTo; }};");
const T = w.__t, D = T.DATA;
const sleep = ms => new Promise(r => setTimeout(r, ms));
let failures = 0;
const check = (cond, what) => { if (!cond) { failures++; console.log("FAIL", what); } };
const L = D.plotLeft + D.xInset, R = D.plotRight - D.xInset;

function noNaN(tag) {
  const bad = [...w.document.querySelectorAll("*")].filter(el => [...el.attributes].some(a => /NaN|Infinity|undefined/.test(a.value)));
  check(bad.length === 0, `${tag}: ${bad.length} elements with NaN/Infinity/undefined, e.g. ${bad[0] && bad[0].outerHTML.slice(0, 160)}`);
}
function checkLayout(tag) {
  D.plots.forEach((plot, p) => {
    const x = T.currentX[p], wd = T.win()[p];
    check(Math.abs(x[wd.k0] - L) < 0.05 && Math.abs(x[wd.k1] - R) < 0.05, `${tag}: plot ${p} window ends at ${x[wd.k0]}, ${x[wd.k1]}`);
    for (let k = 0; k < x.length; k++) {
      const inside = k >= wd.k0 && k <= wd.k1;
      check(inside === (x[k] >= L - 0.05 && x[k] <= R + 0.05), `${tag}: plot ${p} point ${k} at ${x[k]} inside=${inside}`);
      const label = w.document.querySelector(`.size-label[data-plot="${p}"][data-size="${k}"]`);
      check(Math.abs(+label.getAttribute("x") - x[k]) < 0.05, `${tag}: plot ${p} label ${k} x`);
      if (!(x[k] >= D.plotLeft - 12 && x[k] <= D.plotRight + 12)) check(+label.getAttribute("opacity") === 0, `${tag}: plot ${p} label ${k} outside the plot is hidden`);
    }
    // Shown labels never overlap within a row; a hidden label inside the
    // plot fits on neither row beside the labels shown before it.
    const ends = [-Infinity, -Infinity];
    for (let k = 0; k < x.length; k++) {
      const label = w.document.querySelector(`.size-label[data-plot="${p}"][data-size="${k}"]`);
      const half = (label.textContent.length * 7.2 + 6) / 2, shown = +label.getAttribute("opacity") > 0;
      const row = Math.round((+label.getAttribute("y") - plot.bottom - 24) / 13);
      if (shown) {
        check(x[k] - half >= ends[row] - 0.01, `${tag}: plot ${p} label ${k} overlaps row ${row}`);
        ends[row] = x[k] + half;
      } else if (x[k] >= D.plotLeft && x[k] <= D.plotRight) {
        check(ends.every(e => x[k] - half < e), `${tag}: plot ${p} label ${k} hidden though a row has room`);
      }
    }
    plot.series.forEach((s, i) => {
      if (!s || !w.__on[i]) return;
      const labels = [...w.document.getElementById(`series-${p}-${i}`).querySelectorAll(".value-label")];
      check(labels.length === x.length, `${tag}: plot ${p} series ${i} has ${labels.length} value labels`);
      const shown = labels.filter(t => t.getAttribute("display") !== "none").map(t => +t.getAttribute("data-size")).sort((a, b) => a - b);
      check(shown[0] === wd.k0 && shown[shown.length - 1] === wd.k1, `${tag}: plot ${p} series ${i} labels at ${shown}`);
      const detail = w.document.getElementById(`series-${p}-${i}`).querySelector(".series-detail").textContent;
      check(detail.endsWith("at " + plot.sizes[wd.k1]), `${tag}: plot ${p} series ${i} detail "${detail}"`);
    });
  });
  noNaN(tag);
}

(async () => {
  // The static render hid the same labels the script hides.
  {
    const staticDom = new JSDOM(`<!DOCTYPE html><html><body>${markup}</body></html>`);
    D.plots.forEach((plot, p) => plot.x.forEach((_, k) => {
      const sel = `.size-label[data-plot="${p}"][data-size="${k}"]`;
      const before = staticDom.window.document.querySelector(sel).getAttribute("opacity") === "0";
      const after = +w.document.querySelector(sel).getAttribute("opacity") === 0;
      check(before === after, `static and script disagree on label ${p}/${k}`);
    }));
  }
  // Initial layout matches the static render's x positions.
  D.plots.forEach((plot, p) => plot.x.forEach((x, k) => check(Math.abs(T.currentX[p][k] - x) < 0.02, `initial x plot ${p} point ${k}: ${T.currentX[p][k]} vs ${x}`)));
  checkLayout("initial");
  const text = id => w.document.getElementById(id).textContent;
  console.log("ALLB", T.ALLB.length, "values;", text("zoom-from"), "to", text("zoom-to"));
  // Step the lower end up five points.
  for (let i = 0; i < 5; i++) w.zoomStep("from", 1);
  await sleep(700);
  checkLayout("from+5");
  console.log("after from+5:", text("zoom-from"), "to", text("zoom-to"), "windows", T.win().map(x => `${x.k0}-${x.k1}`).join(" "));
  // Zoom to 2 KiB .. 4 KiB exactly by steps.
  w.zoomAll(); await sleep(700);
  while (T.ALLB[T.zFrom] < 2048) w.zoomStep("from", 1);
  while (T.ALLB[T.zTo] > 4096) w.zoomStep("to", -1);
  await sleep(700);
  checkLayout("2-4 KiB");
  console.log("2-4 KiB:", text("zoom-from"), "to", text("zoom-to"), "windows", T.win().map(x => `${x.k0}-${x.k1}`).join(" "),
    "sizes", D.plots.map((pl, p) => pl.sizes.slice(T.win()[p].k0, T.win()[p].k1 + 1).join(",")).join(" | "));
  // Step both ends, switching the unit mid-transition.
  w.zoomAll(); await sleep(700);
  w.zoomStep("from", 3); w.zoomStep("to", -3); await sleep(100); w.flipUnit(); await sleep(900);
  checkLayout("steps + unit");
  console.log("steps + unit:", text("zoom-from"), "to", text("zoom-to"));
  // The arrows hug their labels.
  const tx = id => +((w.document.getElementById(id).getAttribute("transform") || "").match(/translate\(([-\d.]+)/) || [0, 0])[1];
  const fromEnd = +w.document.getElementById("zoom-from").getAttribute("x") + text("zoom-from").length * 7.6;
  check(Math.abs(tx("zoom-from-inc") - (fromEnd + 4)) < 0.2, "the first range's right arrow follows its label");
  const toStart = +w.document.getElementById("zoom-to").getAttribute("x") - text("zoom-to").length * 7.6;
  check(Math.abs(tx("zoom-to-dec") + 16 + 4 - toStart) < 0.2, "the last range's left arrow precedes its label");
  // Hover every point of every plot: the panel holds its widest line.
  w.zoomAll(); await sleep(700);
  const ev0 = { pointerType: "mouse", stopPropagation() {} };
  let widest = 0;
  D.plots.forEach((plot, p) => plot.series.forEach((s, i) => { if (!s) return; plot.x.forEach((_, k) => {
    w.hoverDot(ev0, p, i, k);
    const box = w.document.getElementById("hover-box"), W = +box.getAttribute("width");
    widest = Math.max(widest, W);
    const bx = +box.getAttribute("x");
    [...w.document.getElementById("hover-body").querySelectorAll("text")].forEach(t => {
      const x = +t.getAttribute("x"), n = t.textContent.length, cls = t.getAttribute("class").split(" ")[0];
      const cw = { "hover-head": 7.2, "hover-row": 6.4, "hover-ratio": 6.8, "hover-sub": 5.8, "hover-note": 5.0 }[cls] || 6.4;
      const anchor = t.getAttribute("text-anchor");
      const [lo, hi] = anchor === "end" ? [x - n * cw, x] : [x, x + n * cw];
      check(lo >= -0.5 && hi <= W + 0.5 || W >= 640, `hover ${p}/${i}/${k}: "${t.textContent}" spans ${lo.toFixed(0)}-${hi.toFixed(0)} in a ${W}-wide panel`);
    });
  }); }));
  console.log("widest hover panel", widest);
  noNaN("hover all");
  // Beyond the batch axis: the batch plots keep their two nearest points.
  w.zoomAll(); await sleep(700);
  while (T.zTo - T.zFrom > 1) w.zoomStep("from", 1);
  await sleep(700); checkLayout("last two");
  console.log("last two:", text("zoom-from"), "to", text("zoom-to"), "windows", T.win().map(x => `${x.k0}-${x.k1}`).join(" "));
  // Toggle a series, hover a visible dot and a hidden one.
  w.toggleSeries(0); await sleep(50); checkLayout("toggle");
  const ev = { pointerType: "mouse", stopPropagation() {} };
  const wd = T.win()[0];
  w.hoverDot(ev, 0, 1, wd.k1);
  check(w.document.getElementById("hover").style.display === "", "hover on a shown point shows the panel");
  w.hoverDot(ev, 0, 1, 0);
  check(wd.k0 === 0 || w.document.getElementById("hover").style.display === "none", "hover on a point outside the window hides the panel");
  w.zoomAll(); await sleep(700); checkLayout("all again");
  w.toggleSeries(0); await sleep(50); checkLayout("shown again");
  console.log(failures ? `${failures} failures` : "all checks pass");
  process.exit(failures ? 1 : 0);
})();
