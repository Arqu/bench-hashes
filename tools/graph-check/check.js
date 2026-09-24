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
      check((+label.getAttribute("opacity") > 0) === (x[k] >= D.plotLeft - 12 && x[k] <= D.plotRight + 12), `${tag}: plot ${p} label ${k} opacity`);
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
  // Zoom in and out, and switch the unit mid-zoom.
  w.zoomAll(); await sleep(700);
  w.zoomIn(); await sleep(100); w.flipUnit(); await sleep(900);
  checkLayout("in + unit");
  console.log("zoom in:", text("zoom-from"), "to", text("zoom-to"));
  w.zoomIn(); w.zoomIn(); w.zoomIn(); w.zoomIn(); await sleep(700);
  checkLayout("in x5");
  console.log("zoom in x5:", text("zoom-from"), "to", text("zoom-to"), "off:", ["zoom-in", "zoom-from-inc", "zoom-to-dec"].map(id => w.document.getElementById(id).getAttribute("data-off")).join(","));
  w.zoomOut(); await sleep(700); checkLayout("out");
  console.log("zoom out:", text("zoom-from"), "to", text("zoom-to"));
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
