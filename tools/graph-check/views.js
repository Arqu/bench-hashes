// Visual states for review: zoomed with a hover open, and the code-path section open.
const fs = require("fs");
const { JSDOM } = require("jsdom");
const [,, path, outZoom, outProv] = process.argv;
const load = () => {
  const svg = fs.readFileSync(path, "utf8").replace(/^<\?xml[^>]*>/, "");
  const script = svg.match(/<script><!\[CDATA\[([\s\S]*)\]\]><\/script>/)[1];
  const markup = svg.replace(/<script><!\[CDATA\[[\s\S]*\]\]><\/script>/, "");
  const dom = new JSDOM(`<!DOCTYPE html><html><body>${markup}</body></html>`, { runScripts: "outside-only", pretendToBeVisual: true });
  dom.window.eval(script + "\n;window.__t = {DATA, ALLB, setZoom};");
  return dom.window;
};
const save = (w, out) => fs.writeFileSync(out, '<?xml version="1.0" encoding="UTF-8"?>\n' + w.document.querySelector("svg").outerHTML);
const a = load();
const T = a.__t;
T.setZoom(T.ALLB.findIndex(v => v >= 2048), T.ALLB.findIndex(v => v >= 8192));
setTimeout(() => {
  const servil = T.DATA.names.indexOf("BLAKE3 servil");
  const k = T.DATA.plots[0].sizes.indexOf("4 KiB");
  a.hoverDot({ pointerType: "mouse", stopPropagation() {} }, 0, servil, k);
  save(a, outZoom);
  const b = load();
  b.toggleProv("paths");
  save(b, outProv);
  process.exit(0);
}, 900);
