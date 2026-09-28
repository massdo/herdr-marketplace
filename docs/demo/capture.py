#!/usr/bin/env python3
"""Real marketplace screens for the README demo.

Runs in the disposable Herdr profile prepared by capture.sh. A 120x32 client
is attached through a pseudo-terminal that answers Herdr's terminal queries;
its output goes through the pyte emulator. The journey is a user's: open the
marketplace, search with a typo, open a plugin, preview its install, cancel.
Each screen is saved to the JSON file given as argument.
"""

import fcntl
import json
import os
import pty
import re
import struct
import subprocess
import sys
import termios
import threading
import time

import pyte

SESSION = os.environ["HERDR_MARKETPLACE_DEMO_SESSION"]
COLS, ROWS = 120, 32
# Typed one letter at a time, typo included: the first result is Annotate.
QUERY = "anotate"
# Annotate's README images, in the order its README shows them: the demo
# draws them where the kitty images are.
README_IMAGES = ["assets/herdr-annotate.webp", "assets/star-plannotator.svg"]
SIDEBAR_TOKEN = "herdr_marketplace_sidebar"
DETAILS_TOKEN = "herdr_marketplace_details"

# Colors the fake terminal reports: Herdr asks before drawing anything.
FG, BG = (0xCD, 0xD6, 0xF4), (0x1E, 0x1E, 0x2E)
QUERIES = re.compile(
    rb"\x1b\](?P<osc>1[01]);\?(?:\x1b\\|\x07)"
    rb"|\x1b\]4;(?P<palette>\d+);\?(?:\x1b\\|\x07)"
    rb"|\x1b\[(?P<csi>\?996n|0?c|>0?q|\?u|6n|\?\d+\$p)"
)
# APC, DCS, PM and SOS strings (kitty graphics among them): pyte would
# print their payload.
STRINGS = re.compile(rb"\x1b[_P^X].*?(?:\x1b\\|\x07)", re.S)
STRING_START = re.compile(rb"\x1b[_P^X]")


XTERM_16 = [
    (0, 0, 0), (205, 0, 0), (0, 205, 0), (205, 205, 0), (0, 0, 238), (205, 0, 205),
    (0, 205, 205), (229, 229, 229), (127, 127, 127), (255, 0, 0), (0, 255, 0),
    (255, 255, 0), (92, 92, 255), (255, 0, 255), (0, 255, 255), (255, 255, 255),
]


def xterm_color(index):
    if index < 16:
        return XTERM_16[index]
    if index < 232:
        steps = (0, 95, 135, 175, 215, 255)
        index -= 16
        return steps[index // 36], steps[index // 6 % 6], steps[index % 6]
    level = 8 + (index - 232) * 10
    return level, level, level


def osc_rgb(color):
    return "rgb:" + "/".join(f"{c:02x}{c:02x}" for c in color)


class Screen(pyte.Screen):
    """Leaves the answers to Client.answer."""

    def report_device_status(self, mode, *args, **kwargs):
        if not kwargs.get("private"):
            super().report_device_status(mode)

    def report_device_attributes(self, *args, **kwargs):
        pass


class Client:
    def __init__(self):
        self.screen = Screen(COLS, ROWS)
        self.stream = pyte.ByteStream(self.screen)
        self.lock = threading.Lock()
        self.pending = b""
        self.tail = b""

    def start(self):
        self.pid, self.master = pty.fork()
        if self.pid == 0:
            os.execvp("herdr", ["herdr", "--session", SESSION])
        fcntl.ioctl(self.master, termios.TIOCSWINSZ,
                    struct.pack("HHHH", ROWS, COLS, COLS * 8, ROWS * 17))
        threading.Thread(target=self.record, daemon=True).start()

    def record(self):
        while True:
            try:
                chunk = os.read(self.master, 65536)
            except OSError:
                break  # Linux reports PTY EOF as EIO.
            if not chunk:
                break
            self.answer(chunk)
            with self.lock:
                self.feed(chunk)

    def answer(self, chunk):
        """Replies to the queries a real terminal answers."""
        data = self.tail + chunk
        end = 0
        for match in QUERIES.finditer(data):
            end = match.end()
            reply = ""
            if match["osc"]:
                reply = f"\x1b]{match['osc'].decode()};{osc_rgb(FG if match['osc'] == b'10' else BG)}\x1b\\"
            elif match["palette"]:
                index = int(match["palette"])
                reply = f"\x1b]4;{index};{osc_rgb(xterm_color(index))}\x1b\\"
            else:
                csi = match["csi"].decode()
                if csi == "?996n":
                    reply = "\x1b[?997;1n"  # dark
                elif csi.endswith("c"):
                    reply = "\x1b[?62;22c"
                elif csi.endswith("q"):
                    reply = "\x1bP>|pyte\x1b\\"
                elif csi == "?u":
                    reply = "\x1b[?0u"
                elif csi == "6n":
                    with self.lock:
                        reply = f"\x1b[{self.screen.cursor.y + 1};{self.screen.cursor.x + 1}R"
                else:
                    reply = f"\x1b[{csi[:-2]};0$y"
            os.write(self.master, reply.encode())
        rest = data[end:]
        cut = rest.rfind(b"\x1b")
        self.tail = rest[cut:] if cut >= 0 and len(rest) - cut < 32 else b""

    def feed(self, chunk):
        data = STRINGS.sub(b"", self.pending + chunk)
        self.pending = b""
        start = STRING_START.search(data)
        if start:
            data, self.pending = data[:start.start()], data[start.start():]
        elif data.endswith(b"\x1b"):
            data, self.pending = data[:-1], b"\x1b"
        self.stream.feed(data)

    def click(self, column, row):
        """Left click on 1-based client cell `column`, `row`."""
        os.write(self.master, f"\x1b[<0;{column};{row}M".encode())
        time.sleep(0.05)
        os.write(self.master, f"\x1b[<0;{column};{row}m".encode())

    def lines(self):
        with self.lock:
            return ["".join(self.screen.buffer[y][x].data for x in range(COLS))
                    for y in range(ROWS)]

    def cells(self):
        time.sleep(0.4)  # let Herdr finish drawing
        with self.lock:
            return [[self.screen.buffer[y][x] for x in range(COLS)] for y in range(ROWS)]

    def close(self):
        for signal in (15, 9):
            try:
                os.kill(self.pid, signal)
            except ProcessLookupError:
                return
            deadline = time.monotonic() + 3
            while time.monotonic() < deadline:
                if os.waitpid(self.pid, os.WNOHANG)[0]:
                    return
                time.sleep(0.1)


class Screens:
    """Screens as rows of [text, style] runs, with one style table."""

    def __init__(self):
        self.styles, self.index, self.screens = [], {}, {}

    def style(self, cell):
        flags = "".join(flag for flag, on in (("b", cell.bold), ("i", cell.italics),
                                              ("u", cell.underscore), ("r", cell.reverse)) if on)
        key = (cell.fg, cell.bg, flags)
        if key not in self.index:
            self.index[key] = len(self.styles)
            self.styles.append(list(key))
        return self.index[key]

    def add(self, name, cells):
        rows = []
        for line in cells:
            runs = []
            for cell in line:
                # A wide character's second cell is empty: keep its place. A
                # cell with combining marks is a run of its own, flagged.
                text, style = cell.data or "\0", self.style(cell)
                if len(text) > 1:
                    runs.append([text, style, 1])
                elif runs and runs[-1][1] == style and len(runs[-1]) == 2:
                    runs[-1][0] += text
                else:
                    runs.append([text, style])
            rows.append(runs)
        self.screens[name] = rows
        print("screen", name, flush=True)


def herdr(*args):
    result = subprocess.run(["herdr", "--session", SESSION, *args],
                            text=True, capture_output=True, timeout=60)
    assert result.returncode == 0, f"herdr {args}: {result.stdout}{result.stderr}"
    return result.stdout


def wait(predicate, message, timeout=40):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(0.2)
    raise AssertionError(message)


def pane_with(token):
    panes = json.loads(herdr("pane", "list"))["result"]["panes"]
    return next((p["pane_id"] for p in panes if (p.get("tokens") or {}).get(token) == "v1"), None)


def read(pane):
    return herdr("pane", "read", pane, "--source", "visible", "--format", "text")


def image_keys(cells):
    """Foreground colors of the blank cells that kitty images cover, from
    the top of the screen: Herdr keeps the placeholders' image ids there."""
    seen, used = [], set()
    for line in cells:
        for cell in line:
            if cell.data.strip():
                used.add(cell.fg)
    for line in cells:
        for cell in line:
            if not cell.data.strip() and cell.bg == "default" and cell.fg != "default" \
                    and cell.fg not in used and cell.fg not in seen:
                seen.append(cell.fg)
    return seen


def main(output):
    client, screens = Client(), Screens()
    client.start()
    wait(lambda: any("«" in line for line in client.lines()), "Herdr did not draw")
    # Herdr's own sidebar folds to three columns: more room for the panes.
    row = next(i for i, line in enumerate(client.lines()) if "«" in line)
    client.click(client.lines()[row].index("«") + 1, row + 1)
    wait(lambda: any("»" in line for line in client.lines()), "the sidebar did not fold")
    screens.add("start", client.cells())

    herdr("plugin", "action", "invoke", "herdr-marketplace.toggle")
    sidebar = wait(lambda: pane_with(SIDEBAR_TOKEN), "the sidebar did not open")
    screens.add("loading", client.cells())
    wait(lambda: re.search(r" All \d+ +Installed \d+", read(sidebar)), "the catalog did not load")
    screens.add("catalog", client.cells())

    for count in range(1, len(QUERY) + 1):
        herdr("pane", "send-text", sidebar, QUERY[count - 1])
        wait(lambda: f"│ {QUERY[:count]}" in read(sidebar), "the search was not typed")
        screens.add(f"query-{count}", client.cells())

    herdr("pane", "send-keys", sidebar, "enter")
    details = wait(lambda: pane_with(DETAILS_TOKEN), "the details pane did not open")
    wait(lambda: "Loading README" not in (text := read(details)) and "commit " in text,
         "the README did not load")
    screens.add("readme-loading", client.cells())
    keys = wait(lambda: image_keys(client.cells()), "the README images did not load")
    time.sleep(1)
    screens.add("readme", client.cells())

    herdr("pane", "send-text", details, "i")
    preview = wait(lambda: "Install this plugin?" in (text := read(details)) and text,
                   "the install preview did not open")
    screens.add("preview", client.cells())
    herdr("pane", "send-keys", details, "esc")
    wait(lambda: "Install this plugin?" not in read(details), "the preview was not cancelled")
    client.close()

    source = re.search(r"source:\s*([\w.-]+/[\w.-]+)", preview)[1]
    commit = re.search(r"commit:([0-9a-f]{40})", re.sub(r"\s+", "", preview))[1]
    with open(output, "w") as file:
        json.dump({
            "cols": COLS, "rows": ROWS, "query": QUERY,
            "source": source, "commit": commit, "imageKeys": keys,
            "readmeImages": [f"https://raw.githubusercontent.com/{source}/{commit}/{path}"
                             for path in README_IMAGES],
            "styles": screens.styles, "screens": screens.screens,
        }, file, ensure_ascii=False, separators=(",", ":"))
    print("wrote", output, flush=True)


if __name__ == "__main__":
    main(sys.argv[1])
