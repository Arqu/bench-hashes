# Checking the graph's script

`check.js` runs the script of a generated `bench-hashes.graph.svg` in jsdom
and drives it: zoom steps, zoom in and out, "all", a unit switch during a
zoom, series toggles, hover. It checks that each plot's window ends on the
plot's inner edges, that labels, value labels, and right-hand details
follow the window, that points outside it are out of view, and that no
attribute holds NaN. `snapshot.js` zooms to a byte range and writes the
resulting SVG, for `rsvg-convert` to render and a person to look at.

    npm install jsdom@22
    node tools/graph-check/check.js benchmark-results/<machine>/bench-hashes.graph.svg
    node tools/graph-check/snapshot.js GRAPH.svg /tmp/zoomed.svg 2048 8192
    rsvg-convert -w 1300 /tmp/zoomed.svg -o /tmp/zoomed.png

In the VM: `apt-get install -y nodejs npm` first (lost on restart).
