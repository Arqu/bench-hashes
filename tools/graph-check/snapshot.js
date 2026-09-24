// Render states: serialize the SVG after zooming to a range, for rsvg-convert.
const fs = require("fs");
const { JSDOM } = require("jsdom");
const [,, path, out, fromBytes, toBytes] = process.argv;
const svg = fs.readFileSync(path, "utf8").replace(/^<\?xml[^>]*>/, "");
const script = svg.match(/<script><!\[CDATA\[([\s\S]*)\]\]><\/script>/)[1];
const markup = svg.replace(/<script><!\[CDATA\[[\s\S]*\]\]><\/script>/, "");
const dom = new JSDOM(`<!DOCTYPE html><html><body>${markup}</body></html>`, { runScripts: "outside-only", pretendToBeVisual: true });
const w = dom.window;
w.eval(script + "\n;window.__z = b => { let f = ALLB.findIndex(v => v >= b[0]), t = ALLB.length - 1 - [...ALLB].reverse().findIndex(v => v <= b[1]); setZoom(f, t); };");
w.__z([+fromBytes, +toBytes]);
setTimeout(() => {
  const el = w.document.querySelector("svg");
  fs.writeFileSync(out, '<?xml version="1.0" encoding="UTF-8"?>\n' + el.outerHTML.replace("<svg ", '<svg xmlns:xlink="http://www.w3.org/1999/xlink" '));
  process.exit(0);
}, 900);
