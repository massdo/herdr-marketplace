#!/usr/bin/env node
// Renders demo.html frame by frame in headless Chrome, through the DevTools
// protocol (no npm dependency), then encodes the README animation with
// encode.py (Pillow).
//
//   node docs/demo/render.mjs              writes docs/demo/demo.webp
//   node docs/demo/render.mjs --serve      plays the animation in your browser
//   node docs/demo/render.mjs --at=0,5.2   saves those instants as PNG only
//
// CHROME overrides the browser binary. The captured plugin's README images
// are downloaded once into docs/demo/.cache; Pillow is installed there too,
// in a virtual environment, when python3 lacks it.
import { execFileSync, spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { dirname, extname, join, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const CACHE = join(HERE, ".cache");
const FRAMES = join(CACHE, "frames");
const OUTPUT = join(HERE, "demo.webp");
const WIDTH = 1280, HEIGHT = 720;
const CHROME = process.env.CHROME || (process.platform === "darwin"
  ? "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
  : "google-chrome");
const TYPES = {
  ".html": "text/html", ".json": "application/json", ".png": "image/png",
  ".jpg": "image/jpeg", ".jpeg": "image/jpeg", ".gif": "image/gif",
  ".webp": "image/webp", ".svg": "image/svg+xml",
};

async function readmeImages() {
  const { readmeImages: urls } = JSON.parse(readFileSync(join(HERE, "screens.json"), "utf8"));
  mkdirSync(join(CACHE, "images"), { recursive: true });
  return Promise.all(urls.map(async (url, index) => {
    const hash = createHash("sha256").update(url).digest("hex").slice(0, 12);
    const file = join(CACHE, "images", `${index}-${hash}${extname(new URL(url).pathname)}`);
    if (!existsSync(file)) {
      const response = await fetch(url);
      if (!response.ok) throw new Error(`${url}: HTTP ${response.status}`);
      writeFileSync(file, Buffer.from(await response.arrayBuffer()));
    }
    return file;
  }));
}

// demo.html, screens.json and the README images, on localhost only.
function serve(images) {
  const server = createServer((request, response) => {
    const path = decodeURIComponent(new URL(request.url, "http://localhost").pathname);
    const image = /^\/images\/(\d+)$/.exec(path);
    const file = image ? images[Number(image[1])] : resolve(HERE, `.${path === "/" ? "/demo.html" : path}`);
    if (!file || !file.startsWith(HERE + sep) || !existsSync(file)) {
      response.writeHead(404).end();
      return;
    }
    response.writeHead(200, { "content-type": TYPES[extname(file)] || "application/octet-stream" });
    response.end(readFileSync(file));
  });
  return new Promise((done) => server.listen(0, "127.0.0.1", () => done(server)));
}

// A DevTools protocol client over the WebSocket built into Node.
class DevTools {
  constructor(socket) {
    this.socket = socket;
    this.next = 0;
    this.calls = new Map();
    this.waiters = [];
    socket.addEventListener("message", ({ data }) => {
      const message = JSON.parse(data);
      const call = this.calls.get(message.id);
      if (call) {
        this.calls.delete(message.id);
        message.error ? call.reject(new Error(message.error.message)) : call.resolve(message.result);
      } else {
        this.waiters = this.waiters.filter(([method, done]) => method !== message.method || done(message.params));
      }
    });
  }

  static open(url) {
    const socket = new WebSocket(url);
    return new Promise((done, fail) => {
      socket.addEventListener("open", () => done(new DevTools(socket)));
      socket.addEventListener("error", () => fail(new Error(`cannot reach ${url}`)));
    });
  }

  send(method, params = {}) {
    const id = ++this.next;
    this.socket.send(JSON.stringify({ id, method, params }));
    return new Promise((resolve, reject) => this.calls.set(id, { resolve, reject }));
  }

  once(method) {
    return new Promise((done) => this.waiters.push([method, done]));
  }

  async evaluate(expression) {
    const { result, exceptionDetails } = await this.send("Runtime.evaluate", {
      expression, awaitPromise: true, returnByValue: true,
    });
    if (exceptionDetails) throw new Error(exceptionDetails.exception?.description || expression);
    return result.value;
  }
}

async function launchChrome() {
  const profile = mkdtempSync(join(tmpdir(), "herdr-demo-chrome-"));
  const chrome = spawn(CHROME, [
    "--headless=new", "--remote-debugging-port=0", `--user-data-dir=${profile}`,
    "--hide-scrollbars", "--mute-audio", "--no-first-run", "--no-default-browser-check",
    `--window-size=${WIDTH},${HEIGHT}`, "about:blank",
  ], { stdio: ["ignore", "ignore", "pipe"] });
  const endpoint = await new Promise((done, fail) => {
    let log = "";
    chrome.stderr.on("data", (chunk) => {
      log += chunk;
      const match = /DevTools listening on ws:\/\/([^/]+)\//.exec(log);
      if (match) done(match[1]);
    });
    chrome.on("exit", (code) => fail(new Error(`Chrome exited (${code}): ${log}`)));
  });
  const targets = await (await fetch(`http://${endpoint}/json/list`)).json();
  const page = await DevTools.open(targets.find((target) => target.type === "page").webSocketDebuggerUrl);
  const close = async () => {
    const exited = new Promise((done) => chrome.once("exit", done));
    chrome.kill();
    await exited;
    rmSync(profile, { recursive: true, force: true, maxRetries: 5 });
  };
  return { page, close };
}

async function render(base, instants) {
  const { page, close } = await launchChrome();
  try {
    await page.send("Emulation.setDeviceMetricsOverride", {
      width: WIDTH, height: HEIGHT, deviceScaleFactor: 1, mobile: false,
    });
    await page.send("Page.enable");
    const loaded = page.once("Page.loadEventFired");
    await page.send("Page.navigate", { url: `${base}/demo.html?render` });
    await loaded;
    const deadline = Date.now() + 30_000;
    while (!(await page.evaluate('document.documentElement.dataset.ready === "1"'))) {
      if (Date.now() > deadline) throw new Error("demo.html did not get ready");
      await new Promise((done) => setTimeout(done, 100));
    }
    const { fps, duration } = await page.evaluate("({ fps: demo.fps, duration: demo.duration })");
    const times = instants || Array.from({ length: Math.round(fps * duration) }, (_, frame) => frame / fps);
    const folder = instants ? join(CACHE, "instants") : FRAMES;
    rmSync(folder, { recursive: true, force: true });
    mkdirSync(folder, { recursive: true });
    for (const [index, time] of times.entries()) {
      await page.evaluate(`demo.render(${time})`);
      const { data } = await page.send("Page.captureScreenshot", {
        format: "png", clip: { x: 0, y: 0, width: WIDTH, height: HEIGHT, scale: 1 },
      });
      const name = instants ? `t${time.toFixed(2)}` : String(index).padStart(4, "0");
      writeFileSync(join(folder, `${name}.png`), Buffer.from(data, "base64"));
      if (!instants && index % 50 === 0) console.log(`frame ${index}/${times.length}`);
    }
    if (instants) console.log(`wrote ${folder}`);
    return fps;
  } finally {
    await close();
  }
}

// python3 with Pillow: the system's, or a virtual environment in the cache.
function python() {
  const venv = join(CACHE, "venv", "bin", "python");
  for (const candidate of [venv, "python3"]) {
    try {
      execFileSync(candidate, ["-c", "import PIL"], { stdio: "ignore" });
      return candidate;
    } catch {
      // not this one
    }
  }
  if (!existsSync(venv)) execFileSync("python3", ["-m", "venv", join(CACHE, "venv")], { stdio: "inherit" });
  execFileSync(venv, ["-m", "pip", "install", "--quiet", "pillow"], { stdio: "inherit" });
  return venv;
}

function encode(fps) {
  execFileSync(python(), [join(HERE, "encode.py"), FRAMES, OUTPUT, String(fps)], { stdio: "inherit" });
}

const images = await readmeImages();
const server = await serve(images);
const base = `http://127.0.0.1:${server.address().port}`;
const at = process.argv.find((arg) => arg.startsWith("--at="));
if (process.argv.includes("--serve")) {
  console.log(`${base}/demo.html`);
} else {
  try {
    if (at) await render(base, at.slice(5).split(",").map(Number));
    else encode(await render(base));
  } finally {
    server.close();
  }
}
