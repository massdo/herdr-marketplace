# README demo

`demo.webp`, the animation at the top of the README, is drawn from real
marketplace screens. To make it again after the interface changes:

```sh
sh docs/demo/capture.sh     # real screens -> docs/demo/screens.json
node docs/demo/render.mjs   # docs/demo/demo.html -> docs/demo/demo.webp
```

- `capture.sh` runs the marketplace in a disposable Herdr profile, isolated
  like `scripts/e2e.sh`, with a 120×32 client read through the pyte terminal
  emulator. It searches `anotate`, opens Annotate's README and install
  preview, then cancels: nothing is installed. The installing and installed
  screens are derived from these by `demo.html`, as the plugin draws them.
- `render.mjs` plays `demo.html` in headless Chrome, one screenshot per
  frame, and `encode.py` turns the frames into a looping WebP, which the
  marketplace plays too. Its first frame is the poster: the marketplace
  shows it until the animation starts, and in a terminal that cannot play
  it.
- `node docs/demo/render.mjs --serve` plays the animation in a browser, and
  `--at=5.2,9` saves single instants to `.cache/instants` while you edit the
  timeline at the top of `demo.html`.

Requirements: Herdr 0.9.1, Rust and Python 3 for the capture; Node 22 or
later, Google Chrome (`CHROME` names another Chromium) and Python 3 for the
render. pyte and Pillow are installed into `docs/demo/.cache/venv` when
Python lacks them.
