#!/usr/bin/env python3
"""Marketplace journey in the disposable Herdr profile prepared by e2e.sh."""

import base64
import fcntl
import json
import os
import re
import pty
import struct
import subprocess
import termios
import threading
import time
from pathlib import Path

SESSION = os.environ["HERDR_MARKETPLACE_E2E_SESSION"]
TMP = Path(os.environ["HERDR_MARKETPLACE_E2E_TMP"])
CLIENT_LOG = TMP / "client.log"
INDEX = TMP / "index.json"
OPENED = TMP / "opened.log"
SIDEBAR_TOKEN = "herdr_marketplace_sidebar"
DETAILS_TOKEN = "herdr_marketplace_details"

# `herdr pane send-keys` has no names for these keys: send their sequences.
END = "\x1b[F"

FIXTURE = ("massdo", "herdr-marketplace-fixture")
SHA_A = "c8268d42a98d9140254f4bf4ca13c23a587faed8"  # 1.0.0, harmless build
SHA_B = "1be1b7bb9d9ad3d8733a66d132b87c908ff210c6"  # 1.1.0, harmless build
SHA_C = "bc4d8b84d062b8048b5d89647e7110c9f5a05561"  # 1.2.0, build fails on purpose
BROWSER_SHA = "ff8f17077e52a8b582a4659f3424cd8abbb5ce1d"
# The usage block of its README: the details pane cuts two of these lines.
BROWSER_USAGE = "\n".join([
    "terminal-browser # launches the browser",
    "terminal-browser open <url> # opens the browser at a url",
    "terminal-browser --split right # opens the browser in a split pane to the right",
    "terminal-browser ls # lists open browsers",
    "terminal-browser action # an agent-browser compatible cli for interacting with open terminal-browsers",
])
# herdr-sidebar's README shows a PNG next to it and links to the root README.
SIDEBAR_SHA = "1a5d37ef84edc91e5b3d3d4e39daa32952e6ecf2"
# This plugin's README opens on docs/demo/demo.webp, an animation.
MARKETPLACE_SHA = "f3ead2303f9554163ea30e853869c6cc4fc19e4d"
# Installs fetch the fixture from GitHub and build it.
OPERATION_TIMEOUT = 180
CLIENT_COLS, CLIENT_ROWS = 200, 50


class Client:
    """Attached 200x50 client, as in herdr-npm's journey: without one, a
    headless server does not size pane terminals to the layout."""

    def start(self):
        self.pid, self.master = pty.fork()
        if self.pid == 0:
            # As for a VS Code remote session, Herdr writes what it copies to
            # its terminal, here the client log, not to the system clipboard.
            os.execvpe("herdr", ["herdr", "--session", SESSION],
                       {**os.environ, "VSCODE_IPC_HOOK_CLI": str(TMP / "vscode.sock")})
        # Pixel size as a real terminal reports it: 8 × 17 pixel cells.
        fcntl.ioctl(self.master, termios.TIOCSWINSZ,
                    struct.pack("HHHH", CLIENT_ROWS, CLIENT_COLS, CLIENT_COLS * 8, CLIENT_ROWS * 17))
        self.reader = threading.Thread(target=self.record, daemon=True)
        self.reader.start()

    def record(self):
        with open(CLIENT_LOG, "ab", buffering=0) as log:
            while True:
                try:
                    chunk = os.read(self.master, 8192)
                except OSError:
                    break  # Linux reports PTY EOF as EIO.
                if not chunk:
                    break
                log.write(chunk)

    def click(self, column, row):
        """Left button pressed and released at 1-based screen cell
        `column`, `row`, in the SGR encoding a terminal sends."""
        os.write(self.master, f"\x1b[<0;{column};{row}M".encode())
        time.sleep(0.05)
        os.write(self.master, f"\x1b[<0;{column};{row}m".encode())

    def drag(self, start, end):
        """Left button pressed on 1-based screen cell `start`, moved to
        `end` and released there, as a terminal reports a drag."""
        (x1, y1), (x2, y2) = start, end
        os.write(self.master, f"\x1b[<0;{x1};{y1}M".encode())
        time.sleep(0.05)
        for step in range(1, 5):
            x, y = x1 + (x2 - x1) * step // 4, y1 + (y2 - y1) * step // 4
            os.write(self.master, f"\x1b[<32;{x};{y}M".encode())
            time.sleep(0.02)
        os.write(self.master, f"\x1b[<0;{x2};{y2}m".encode())

    def close(self):
        try:
            os.kill(self.pid, 15)
        except ProcessLookupError:
            pass
        os.waitpid(self.pid, 0)
        self.reader.join(timeout=2)
        os.close(self.master)


def herdr(*args):
    result = subprocess.run(
        ["herdr", "--session", SESSION, *args],
        text=True, capture_output=True, timeout=60,
    )
    assert result.returncode == 0, f"herdr {args}: {result.stdout}{result.stderr}"
    return result.stdout


def data(*args):
    return json.loads(herdr(*args))["result"]


def wait(predicate, message, timeout=30):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(0.2)
    raise AssertionError(message)


def panes():
    return data("pane", "list")["panes"]


def read(pane):
    return herdr("pane", "read", pane, "--source", "visible", "--format", "text")


def listed(text):
    """The filters, with their counts, show once the catalogue is loaded."""
    return re.search(r" All \d+ +Installed \d+", text) is not None


def keys(pane, *names):
    herdr("pane", "send-keys", pane, *names)


def type_text(pane, text):
    herdr("pane", "send-text", pane, text)


def toggle():
    herdr("plugin", "action", "invoke", "herdr-marketplace.toggle")


def find_text(pane, text, below=-1):
    """Row and column where `pane` shows `text`, on the first row after
    `below` that has it."""
    lines = read(pane).split("\n")
    row = next((i for i, line in enumerate(lines) if i > below and text in line), None)
    assert row is not None, f"{text!r} is not on {pane}: {lines}"
    return row, lines[row].index(text)


def screen_cell(pane, row, column):
    """1-based client cell of `pane`'s cell `row`, `column`. The tab area
    fills the client right of Herdr's sidebar and under its top bar; a pane
    in a split has a one-cell border."""
    layout = data("pane", "layout", "--pane", pane)["layout"]
    rect = next(p["rect"] for p in layout["panes"] if p["pane_id"] == pane)
    inset = 1 if len(layout["panes"]) > 1 else 0
    x = CLIENT_COLS - layout["area"]["width"] + rect["x"] + inset + column + 1
    y = CLIENT_ROWS - layout["area"]["height"] + rect["y"] + inset + row + 1
    return x, y


def click_text(pane, text):
    """Clicks the middle of `text` where `pane` shows it, through the attached
    client, as a user would."""
    row, column = find_text(pane, text)
    CLIENT.click(*screen_cell(pane, row, column + len(text) // 2))


def click_focused(pane, text):
    """Clicks `text` on `pane`: a pane without the focus takes the first
    click only as the focus, so a second click follows."""
    had_focus = focused() == pane
    click_text(pane, text)
    if not had_focus:
        wait(lambda: focused() == pane, "a click did not focus the pane")
        click_text(pane, text)


def drag_text(pane, first, last):
    """Drags through the attached client from the first cell of `first` to
    the last cell of `last`, where `pane` shows them, as a user selects."""
    row, column = find_text(pane, first)
    end_row, end_column = find_text(pane, last, row - 1)
    CLIENT.drag(screen_cell(pane, row, column),
                screen_cell(pane, end_row, end_column + len(last) - 1))


def copied():
    """The last text Herdr copied: the OSC 52 it wrote to the client."""
    copies = re.findall(rb"\x1b\]52;c;([A-Za-z0-9+/=]*)\x07", CLIENT_LOG.read_bytes())
    return base64.b64decode(copies[-1]).decode() if copies else None


def with_token(key):
    return next((p for p in panes() if (p.get("tokens") or {}).get(key) == "v1"), None)


def others():
    """Every pane that is not the marketplace, with its tab."""
    return {
        p["pane_id"]: p["tab_id"]
        for p in panes()
        if not (p.get("tokens") or {}).get(SIDEBAR_TOKEN)
    }


def manifest(path, plugin_id, name, description, min_herdr="0.9.1", version="1.0.0"):
    return {
        "path": path, "id": plugin_id, "name": name, "version": version,
        "description": description, "platforms": ["linux", "macos"],
        "minHerdrVersion": min_herdr,
    }


def repo(owner, name, sha, stars, manifests, topics=()):
    return {
        "owner": owner, "name": name, "fullName": f"{owner}/{name}",
        "headCommit": sha, "stars": stars, "topics": list(topics),
        "description": f"{name} repository", "manifests": manifests,
    }


def write_catalog(fixture_sha):
    """Frozen test catalogue: the fixture at `fixture_sha`, with the version
    its root manifest declares there, terminal-browser, one incompatible
    plugin and fillers long enough to scroll."""
    version = {SHA_A: "1.0.0", SHA_B: "1.1.0", SHA_C: "1.2.0"}[fixture_sha]
    repos = [
        repo(*FIXTURE, fixture_sha, 50, [
            manifest("herdr-plugin.toml", "herdr-marketplace-fixture",
                     "herdr-marketplace fixture", "Test fixture.", version=version),
            manifest("alt/herdr-plugin.toml", "herdr-marketplace-fixture",
                     "herdr-marketplace fixture (alt)", "Second source, same id."),
        ]),
        repo("zenbu-labs", "terminal-browser", BROWSER_SHA, 3403, [
            manifest("herdr-plugin/herdr-plugin.toml", "zenbu-labs.terminal-browser",
                     "Terminal Browser", "Open a browser inside herdr"),
        ], topics=["browser"]),
        repo("alexarthurs", "herdr-sidebar", SIDEBAR_SHA, 389, [
            manifest("plugins/herdr-sidebar/herdr-plugin.toml", "herdr-sidebar",
                     "herdr-sidebar", "File explorer and source control."),
        ]),
        repo("future", "needs-new-herdr", SHA_A, 900, [
            manifest("herdr-plugin.toml", "future.plugin", "Future Plugin",
                     "Needs a newer Herdr.", min_herdr="9.9.9"),
        ]),
    ]
    for index in range(20):
        repos.append(repo("acme", f"filler-{index:02}", SHA_A, 40 - index, [
            manifest("herdr-plugin.toml", f"acme.filler-{index:02}",
                     f"Filler {index:02}", "Scroll material."),
        ]))
    repos.append(repo("acme", "zz-last", SHA_A, 0, [
        manifest("herdr-plugin.toml", "acme.zz-last", "Last Plugin", "Bottom of the list."),
    ]))
    count = sum(len(r["manifests"]) for r in repos)
    INDEX.write_text(json.dumps({
        "schemaVersion": 1, "generatedAt": "2026-09-25T00:00:00Z",
        "pluginCount": count, "repositoryCount": len(repos), "plugins": repos,
    }))
    return count


def open_sidebar():
    toggle()
    pane = wait(lambda: with_token(SIDEBAR_TOKEN), "the action did not open the sidebar")["pane_id"]
    return pane


def check_isolation():
    xdg = Path(os.environ["XDG_CONFIG_HOME"]).resolve()
    assert SESSION.startswith("herdr-mkt-e2e-"), SESSION
    assert xdg.is_relative_to(TMP.resolve()), xdg
    assert Path(os.environ["HERDR_SOCKET_PATH"]).resolve().is_relative_to(xdg)
    assert Path(os.environ["HERDR_CONFIG_PATH"]).resolve().is_relative_to(xdg)
    assert xdg != Path.home() / ".config"
    print("isolation_ok", flush=True)


def prove_sidebar():
    count = write_catalog(SHA_A)
    before = others()
    sidebar = open_sidebar()
    shown = wait(lambda: listed(text := read(sidebar)) and text, "the catalogue did not load")
    assert f" All {count - 1} " in shown and " Installed 0" in shown, shown
    assert "Search name, topic, author" in shown, shown
    assert "1 incompatible plugin hidden" in shown, shown
    # 26 columns inside a card: a long owner/repo/subdir loses its middle,
    # never its ends.
    assert "Terminal Browser" in shown and "zenbu-labs/te…herdr-plugin" in shown, shown
    assert "╭ Terminal Browser" in shown and "★ 3403 ╮" in shown, "plugins are cards"
    assert others() == before, (before, others())
    layout = data("pane", "layout", "--pane", sidebar)["layout"]
    rects = {p["pane_id"]: p["rect"] for p in layout["panes"]}
    width = int(rects[sidebar]["width"])
    assert 28 <= width <= 40, f"sidebar width {width} is not near 32 columns: {layout}"
    assert all(rects[sidebar]["x"] < rect["x"] for pane, rect in rects.items() if pane != sidebar), layout
    print("sidebar_open_ok", flush=True)

    type_text(sidebar, "fixture")
    shown = wait(lambda: " All 2 " in (text := read(sidebar)) and text, "the search did not filter")
    assert "Terminal Browser" not in shown, shown
    assert "massdo/herdr-…lace-fixture" in shown, shown
    assert "massdo/herdr-…-fixture/alt" in shown, shown
    type_text(sidebar, "jk")
    wait(lambda: "│ fixturejk" in read(sidebar), "j and k did not reach the search")
    keys(sidebar, "backspace", "backspace")
    wait(lambda: " All 2 " in read(sidebar), "backspace did not restore the search")
    print("search_ok", flush=True)

    # Typing never reloads: without the index file, the search still works.
    INDEX.unlink()
    type_text(sidebar, " (alt)")
    shown = wait(lambda: " All 1 " in (text := read(sidebar)) and text, "the search stopped working")
    assert "Loading failed" not in shown, shown
    keys(sidebar, "esc")
    wait(lambda: f" All {count - 1} " in read(sidebar), "esc did not clear the search")
    print("typing_without_network_ok", flush=True)

    type_text(sidebar, END)
    wait(lambda: "Last Plugin" in read(sidebar), "the list did not reach its last entry")
    # The key previews the plugin it reaches; Esc in the preview closes it.
    preview = wait(lambda: with_token(DETAILS_TOKEN), "End did not preview the last plugin")["pane_id"]
    wait(lambda: "acme/zz-last" in read(preview), "the preview is not the last plugin")
    keys(preview, "q")
    wait(lambda: with_token(DETAILS_TOKEN) is None, "q did not close the preview")
    print("last_entry_ok", flush=True)

    toggle()
    wait(lambda: with_token(SIDEBAR_TOKEN) is None, "the action did not close the sidebar")
    assert others() == before, (before, others())
    print("sidebar_close_ok", flush=True)

    # INDEX is still missing: the sidebar reports it, then retries on Enter.
    sidebar = open_sidebar()
    shown = wait(lambda: "Loading failed" in (text := read(sidebar)) and text,
                 "the download failure was not shown")
    assert "Retry (Enter)" in shown, shown
    write_catalog(SHA_A)
    keys(sidebar, "enter")
    wait(lambda: f" All {count - 1} " in read(sidebar), "the retry did not load the catalogue")
    toggle()
    wait(lambda: with_token(SIDEBAR_TOKEN) is None, "the action did not close the sidebar")
    print("retry_ok", flush=True)


def details_panes(tab):
    return [p for p in panes()
            if p["tab_id"] == tab and (p.get("tokens") or {}).get(DETAILS_TOKEN) == "v1"]


def focused():
    return next((p["pane_id"] for p in panes() if p.get("focused")), None)


def open_details(sidebar, query, expected_results):
    """Search, press Enter, and wait for the existing Details pane's content."""
    keys(sidebar, *["backspace"] * 30)
    type_text(sidebar, query)
    wait(lambda: expected_results in read(sidebar), f"search {query!r} did not settle")
    keys(sidebar, "enter")
    details = wait(lambda: with_token(DETAILS_TOKEN),
                   f"Enter on {query!r} did not show Details")["pane_id"]
    source = {"fixture": "massdo/herdr-marketplace-fixture",
              "terminal browser": "zenbu-labs/terminal-browser/herdr-plugin",
              "herdr-sidebar": "alexarthurs/herdr-sidebar/plugins/herdr-sidebar",
              "marketplace": "massdo/herdr-marketplace"}[query]
    wait(lambda: source in read(details), f"Details did not show {query!r}")
    return details


def prove_details():
    write_catalog(SHA_A)
    sidebar = open_sidebar()
    wait(lambda: listed(read(sidebar)), "the catalogue did not load")
    tab = next(p["tab_id"] for p in panes() if p["pane_id"] == sidebar)

    details = open_details(sidebar, "terminal browser", " All 1 ")
    shown = wait(lambda: "open-split" in (text := read(details)) and text,
                 "the terminal-browser README was not rendered")
    assert "zenbu-labs/terminal-browser/herdr-plugin" in shown, shown
    assert f"commit {BROWSER_SHA[:7]}" in shown, shown
    assert "terminal-browser herdr plugin" in shown, shown
    assert "herdr plugin install zenbu-labs/terminal-browser/herdr-plugin" in shown, shown
    assert "```" not in shown and "### " not in shown, shown
    assert focused() == details, "the details pane did not take focus"
    print("details_terminal_browser_ok", flush=True)

    # Herdr leaves the mouse to a pane that asks for it: the details pane
    # selects on its own, and the release copies, even from another focus.
    herdr("pane", "focus", "--pane", details, "--direction", "left")
    wait(lambda: focused() != details, "the focus did not leave the details pane")
    command = "herdr plugin install zenbu-labs/terminal-browser/herdr-plugin"
    drag_text(details, command, command)
    wait(lambda: copied() == command, f"the drag did not copy the command: {copied()!r}")
    wait(lambda: focused() == details, "the drag did not focus the details pane")
    # Code comes without its frame, and a line the pane cuts comes back whole.
    drag_text(details, "terminal-browser # launches", "open terminal-browsers")
    wait(lambda: copied() == BROWSER_USAGE, f"the drag did not copy the usage: {copied()!r}")
    print("details_copy_ok", flush=True)

    keys(details, "esc")
    time.sleep(1)
    assert details_panes(tab), "escape closed the details pane"
    keys(details, "q")
    wait(lambda: not details_panes(tab), "q did not close the details pane")
    wait(lambda: focused() == sidebar, "focus did not return to the sidebar")
    print("details_quit_ok", flush=True)

    details = open_details(sidebar, "fixture", " All 2 ")
    shown = wait(lambda: "[image: fixture logo]" in (text := read(details)) and text,
                 "the fixture README was not rendered")
    assert "massdo/herdr-marketplace-fixture" in shown and f"commit {SHA_A[:7]}" in shown, shown
    assert "End of the fixture README" not in shown, shown
    type_text(details, END)
    wait(lambda: "End of the fixture README, version 1.0.0." in read(details),
         "the last README line was not reachable")
    print("details_last_line_ok", flush=True)

    wait(lambda: " All 2 " in read(sidebar), "search fixture did not stay")
    keys(sidebar, "down", "enter")
    alt = details
    shown = wait(lambda: "No README in alt/" in (text := read(alt)) and text,
                 "the root README fallback was not signalled")
    assert "massdo/herdr-marketplace-fixture/alt" in shown, shown
    assert "herdr-marketplace fixture" in shown, shown
    assert [p["pane_id"] for p in details_panes(tab)] == [alt], "two openings left more than one details pane"
    print("details_fallback_and_single_pane_ok", flush=True)

    toggle()
    wait(lambda: with_token(SIDEBAR_TOKEN) is None, "the action did not close the sidebar")
    assert "massdo/herdr-marketplace-fixture/alt" in read(alt), "the details pane lost its plugin"
    keys(alt, "q")
    wait(lambda: not details_panes(tab), "q did not close the details pane")
    wait(lambda: focused() is not None and focused() in others(), "focus did not go to a remaining pane")
    print("details_outlives_sidebar_ok", flush=True)

    # A click on the sidebar without the focus only focuses it, so that a
    # search can be typed; then one click on a plugin shows its details and
    # the sidebar keeps the focus, until Enter.
    sidebar = open_sidebar()
    wait(lambda: listed(read(sidebar)), "the catalogue did not load")
    herdr("pane", "focus", "--pane", sidebar, "--direction", "right")
    wait(lambda: focused() != sidebar, "the focus did not leave the sidebar")
    before = {p["pane_id"] for p in panes()}
    click_text(sidebar, "Terminal Browser")
    wait(lambda: focused() == sidebar, "a click did not focus the sidebar")
    time.sleep(1)
    assert not details_panes(tab), "the click that focused the sidebar opened a plugin"
    print("focus_click_ok", flush=True)
    click_text(sidebar, "Terminal Browser")
    details = wait(lambda: next((p["pane_id"] for p in details_panes(tab) if p["pane_id"] not in before), None),
                   "a click on a plugin did not open its details pane")
    wait(lambda: "zenbu-labs/terminal-browser/herdr-plugin" in read(details),
         "the clicked plugin is not the one shown")
    assert focused() == sidebar, "the click gave the details pane the focus"
    keys(sidebar, "enter")
    wait(lambda: focused() == details, "enter did not focus the clicked plugin's pane")
    assert [p["pane_id"] for p in details_panes(tab)] == [details], "enter opened another pane"
    print("details_click_ok", flush=True)
    keys(details, "q")
    wait(lambda: not details_panes(tab), "q did not close the details pane")
    toggle()
    wait(lambda: with_token(SIDEBAR_TOKEN) is None, "the action did not close the sidebar")


def rects(pane):
    """Every pane of the tab of `pane`, left to right, with its rectangle."""
    layout = data("pane", "layout", "--pane", pane)["layout"]
    return sorted(((p["pane_id"], p["rect"]) for p in layout["panes"]),
                  key=lambda item: (item[1]["x"], item[1]["y"]))


def prove_preview():
    """The arrows show the selected plugin right of the sidebar and leave
    it the focus; Enter then hands the focus to that pane."""
    write_catalog(SHA_A)
    sidebar = open_sidebar()
    wait(lambda: listed(read(sidebar)), "the catalogue did not load")
    tab = next(p["tab_id"] for p in panes() if p["pane_id"] == sidebar)
    type_text(sidebar, "fixture")
    wait(lambda: " All 2 " in read(sidebar), "search fixture did not settle")
    time.sleep(1)
    assert not details_panes(tab), "typing opened a preview"

    keys(sidebar, "down")
    alt = wait(lambda: next(iter(details_panes(tab)), None), "down did not preview the plugin")["pane_id"]
    wait(lambda: "No README in alt/" in read(alt), "the preview is not the selected plugin")
    assert focused() == sidebar, "the preview took the focus"
    order = rects(sidebar)
    assert [pane for pane, _ in order[:2]] == [sidebar, alt], order
    working = order[2]
    print("preview_beside_sidebar_ok", flush=True)

    keys(sidebar, "up")
    root = alt
    wait(lambda: "[image: fixture logo]" in read(root), "the preview did not follow the selection")
    assert [p["pane_id"] for p in details_panes(tab)] == [root], "the old preview stayed open"
    assert focused() == sidebar, "the new preview took the focus"
    assert [pane for pane, _ in rects(sidebar)[:2]] == [sidebar, root], rects(sidebar)
    assert working in rects(sidebar), "the working pane moved or changed width"
    print("preview_follows_arrows_ok", flush=True)

    # A search moves the open preview to its first result.
    type_text(sidebar, " (alt)")
    wait(lambda: " All 1 " in read(sidebar), "search fixture (alt) did not settle")
    alt = root
    wait(lambda: "No README in alt/" in read(alt), "the preview is not the first result")
    keys(sidebar, *["backspace"] * len(" (alt)"))
    root = alt
    wait(lambda: "[image: fixture logo]" in read(root), "the preview is not the first result")
    assert [p["pane_id"] for p in details_panes(tab)] == [root], "the old preview stayed open"
    assert focused() == sidebar, "the search moved the focus"
    assert working in rects(sidebar), "the working pane moved or changed width"
    print("preview_follows_search_ok", flush=True)

    keys(sidebar, "enter")
    wait(lambda: focused() == root, "enter did not focus the preview")
    assert [p["pane_id"] for p in details_panes(tab)] == [root], "enter opened another pane"
    print("enter_focuses_preview_ok", flush=True)

    keys(root, "q")
    wait(lambda: not details_panes(tab), "q did not close the preview")
    wait(lambda: focused() == sidebar, "focus did not return to the sidebar")
    toggle()
    wait(lambda: with_token(SIDEBAR_TOKEN) is None, "the action did not close the sidebar")




def prove_fast_sidebar_close():
    write_catalog(SHA_A)
    for mode in ("toggle", "esc"):
        sidebar = open_sidebar()
        wait(lambda: listed(read(sidebar)), "catalogue did not load")
        started = time.monotonic()
        if mode == "toggle":
            toggle()
        else:
            keys(sidebar, "esc")
        while with_token(SIDEBAR_TOKEN) is not None:
            assert time.monotonic() - started < 0.5, "closing stalled for half a second"
            time.sleep(0.01)
        elapsed = round((time.monotonic() - started) * 1000)
        assert elapsed < 500, elapsed
        print(f"sidebar_fast_close_ok mode={mode} milliseconds={elapsed}", flush=True)


def prove_single_details():
    """Observe every transition, rather than only the final pane count."""
    write_catalog(SHA_A)
    sidebar = open_sidebar()
    wait(lambda: listed(read(sidebar)), "the catalogue did not load")
    details = open_details(sidebar, "fixture", " All 2 ")
    wait(lambda: "[image: fixture logo]" in read(details), "fixture README did not load")
    tab = next(p["tab_id"] for p in panes() if p["pane_id"] == sidebar)
    layout = rects(sidebar)
    stop = threading.Event()
    samples, failures = [], []

    def observe():
        try:
            while not stop.is_set():
                current = [p["pane_id"] for p in details_panes(tab)]
                samples.append(current)
                if current != [details]:
                    failures.append(current)
                time.sleep(0.005)
        except Exception as error:
            failures.append(str(error))

    watcher = threading.Thread(target=observe, daemon=True)
    watcher.start()
    try:
        herdr("pane", "focus", "--pane", details, "--direction", "left")
        wait(lambda: focused() == sidebar, "sidebar did not regain focus")
        time.sleep(0.3)
        for _ in range(3):
            click_text(sidebar, "fixture/alt")
            wait(lambda: "No README in alt/" in read(details), "click did not select B")
            keys(sidebar, "up")
            wait(lambda: "[image: fixture logo]" in read(details), "up did not select A")
            assert rects(sidebar) == layout, "switching plugins changed pane geometry"
        # Rapid browsing must still converge to the final selection.
        keys(sidebar, "down", "up", "down", "up")
        time.sleep(0.5)
        assert "[image: fixture logo]" in read(details), read(details)
    finally:
        stop.set()
        watcher.join(timeout=5)
    assert not watcher.is_alive(), "Details monitor did not stop"
    assert len(samples) >= 10, len(samples)
    assert not failures, failures
    print(f"single_details_pane_ok samples={len(samples)}", flush=True)
    keys(details, "q")
    wait(lambda: not details_panes(tab), "q did not close Details")
    toggle()
    wait(lambda: with_token(SIDEBAR_TOKEN) is None, "toggle did not close Marketplace")


def registry():
    """Plugins installed from GitHub: source and commit, from `herdr plugin list --json`."""
    return {
        (p["source"]["owner"], p["source"]["repo"], p["source"].get("subdir", ""),
         p["source"]["resolved_commit"])
        for p in data("plugin", "list", "--json")["plugins"]
        if p["source"]["kind"] == "github"
    }


def fixture_log():
    path = Path(os.environ["HERDR_MARKETPLACE_FIXTURE_LOG"])
    return path.read_text() if path.exists() else ""


def prove_install_preview():
    write_catalog(SHA_A)
    sidebar = open_sidebar()
    wait(lambda: listed(read(sidebar)), "the catalogue did not load")
    tab = next(p["tab_id"] for p in panes() if p["pane_id"] == sidebar)
    details = open_details(sidebar, "fixture", " All 2 ")
    wait(lambda: "[image: fixture logo]" in read(details), "the fixture README was not rendered")
    before = registry()

    keys(details, "i")
    shown = wait(lambda: "Enter: confirm" in (text := read(details)) and text,
                 "i did not open the install preview")
    assert "source: massdo/herdr-marketplace-fixture" in shown, shown
    assert f"commit: {SHA_A}" in shown, shown
    assert "• /bin/sh build.sh" in shown, shown
    assert '• hello: /bin/echo "hello from herdr-marketplace-fixture"' in shown, shown
    assert "This plugin will run code with your permissions." in shown, shown
    print("install_preview_ok", flush=True)

    keys(details, "esc")
    wait(lambda: "Install (i)" in read(details), "escape did not cancel the preview")
    time.sleep(2)
    assert registry() == before, (before, registry())
    assert fixture_log() == "", fixture_log()
    assert details_panes(tab), "cancelling the preview closed the details pane"
    print("install_cancel_ok", flush=True)

    keys(details, "q")
    wait(lambda: not details_panes(tab), "q did not close the details pane")
    toggle()
    wait(lambda: with_token(SIDEBAR_TOKEN) is None, "the action did not close the sidebar")


def fixture_plugin():
    return next((p for p in data("plugin", "list", "--json")["plugins"]
                 if p["plugin_id"] == "herdr-marketplace-fixture"), None)


def fixture_details(sha):
    """Sidebar on a catalogue with the fixture at `sha`, and the root fixture's details pane."""
    write_catalog(sha)
    sidebar = open_sidebar()
    wait(lambda: listed(read(sidebar)), "the catalogue did not load")
    tab = next(p["tab_id"] for p in panes() if p["pane_id"] == sidebar)
    details = open_details(sidebar, "fixture", " All 2 ")
    wait(lambda: f"commit {sha[:7]}" in read(details), "the fixture details pane did not open")
    return sidebar, tab, details


def confirm_install(details, expected):
    keys(details, "i")
    shown = wait(lambda: "Enter: confirm" in (text := read(details)) and text,
                 "i did not open the install preview")
    assert expected in shown, shown
    keys(details, "enter")


def close_all(tab):
    for pane in details_panes(tab):
        keys(pane["pane_id"], "q")
    wait(lambda: not details_panes(tab), "q did not close the details pane")
    if with_token(SIDEBAR_TOKEN):
        toggle()
        wait(lambda: with_token(SIDEBAR_TOKEN) is None, "the action did not close the sidebar")


def prove_install():
    """Through the buttons, with the mouse."""
    sidebar, tab, details = fixture_details(SHA_A)
    wait(lambda: "Install (i)" in read(details), "the details pane has no Install button")
    click_text(details, "Install (i)")
    shown = wait(lambda: "Confirm install (Enter)" in (text := read(details)) and text,
                 "a click on Install did not open the preview")
    assert "Install this plugin?" in shown and "Cancel (Esc)" in shown, shown
    click_text(details, "Confirm install (Enter)")
    wait(lambda: "Install of c8268d4 succeeded" in read(details),
         "the install did not succeed", OPERATION_TIMEOUT)
    assert (*FIXTURE, "", SHA_A) in registry(), registry()
    marker = Path(fixture_plugin()["plugin_root"]) / "build-marker.txt"
    assert marker.read_text().strip() == "1.0.0", marker.read_text()
    wait(lambda: "· installed" in read(details), "the details pane does not show 'installed'")
    wait(lambda: "installed · Test fixture." in read(sidebar), "the sidebar does not show 'installed'")
    print("install_from_details_ok", flush=True)

    # The Installed filter, as in VS Code: the first click focuses the
    # sidebar, the second switches the filter.
    click_text(sidebar, "Installed 1")
    wait(lambda: focused() == sidebar, "a click did not focus the sidebar")
    click_text(sidebar, "Installed 1")
    # The search "fixture" stays: the alt source is the one not installed.
    shown = wait(lambda: "massdo/herdr-…-fixture/alt" not in (text := read(sidebar)) and text,
                 "the Installed filter did not apply")
    assert "massdo/herdr-…lace-fixture" in shown and "installed · Test fixture." in shown, shown
    type_text(sidebar, "\t")
    wait(lambda: "massdo/herdr-…-fixture/alt" in read(sidebar), "Tab did not return to all plugins")
    print("installed_filter_ok", flush=True)
    close_all(tab)


def prove_switch():
    herdr("plugin", "install", "/".join(FIXTURE), "--ref", SHA_A, "--yes")
    assert (*FIXTURE, "", SHA_A) in registry(), registry()
    sidebar, tab, details = fixture_details(SHA_B)
    confirm_install(details, f"installed commit: {SHA_A}")
    wait(lambda: "Install of 1be1b7b succeeded" in read(details),
         "the switch did not succeed", OPERATION_TIMEOUT)
    assert (*FIXTURE, "", SHA_B) in registry(), registry()
    print("switch_commit_ok", flush=True)
    close_all(tab)


def prove_failed_build():
    sidebar, tab, details = fixture_details(SHA_C)
    confirm_install(details, "another commit?")
    shown = wait(lambda: "Install of bc4d8b8 failed" in (text := read(details)) and text,
                 "the failed build was not reported", OPERATION_TIMEOUT)
    assert f"Registry: installed at {SHA_B}" in shown, shown
    assert "Plugin was not installed." in shown or "plugin build failed" in shown, shown
    assert (*FIXTURE, "", SHA_B) in registry(), registry()
    print("failed_build_ok", flush=True)
    close_all(tab)


def prove_details_closed_during_install():
    sidebar, tab, details = fixture_details(SHA_A)
    confirm_install(details, "another commit?")
    wait(lambda: "Installing" in read(details), "the details pane did not show the running install")
    keys(details, "q")
    wait(lambda: not details_panes(tab), "q did not close the details pane")
    wait(lambda: (*FIXTURE, "", SHA_A) in registry(),
         "the install stopped with its details pane", OPERATION_TIMEOUT)
    details = open_details(sidebar, "fixture", " All 2 ")
    wait(lambda: "Install of c8268d4 succeeded" in read(details),
         "the reopened details pane did not show the result")
    print("details_closed_during_install_ok", flush=True)
    close_all(tab)


def fixture_at_a():
    """The fixture installed at A, through Herdr's command line if needed."""
    if (*FIXTURE, "", SHA_A) not in registry():
        herdr("plugin", "install", "/".join(FIXTURE), "--ref", SHA_A, "--yes")
    assert (*FIXTURE, "", SHA_A) in registry(), registry()


def prove_update_shown():
    """A newer version in the catalogue: the card of the installed fixture
    offers it, the card of alt/, not installed, does not."""
    fixture_at_a()
    write_catalog(SHA_B)
    sidebar = open_sidebar()
    wait(lambda: listed(read(sidebar)), "the catalogue did not load")
    type_text(sidebar, "fixture")
    shown = wait(lambda: " All 2 " in (text := read(sidebar)) and "Update to 1.1.0" in text and text,
                 "the card did not offer the update")
    assert "massdo/herdr-…-fixture/alt" in shown, shown
    assert shown.count("Update to 1.1.0") == 1, shown
    toggle()
    wait(lambda: with_token(SIDEBAR_TOKEN) is None, "the action did not close the sidebar")

    write_catalog(SHA_A)
    sidebar = open_sidebar()
    wait(lambda: listed(read(sidebar)), "the catalogue did not load")
    type_text(sidebar, "fixture")
    shown = wait(lambda: " All 2 " in (text := read(sidebar)) and text, "search fixture did not settle")
    assert "Update to" not in shown, shown
    assert "installed · Test fixture." in shown, shown
    toggle()
    wait(lambda: with_token(SIDEBAR_TOKEN) is None, "the action did not close the sidebar")
    print("update_shown_ok", flush=True)


def prove_update_from_details():
    """`u` in the details pane updates the fixture from A to B, never
    showing a preview."""
    fixture_at_a()
    sidebar, tab, details = fixture_details(SHA_B)
    wait(lambda: "Update to 1.1.0 (u)" in read(details), "the details pane did not offer the update")
    keys(details, "u")

    def updated():
        text = read(details)
        assert "Enter: confirm" not in text, f"the update showed a preview: {text}"
        return "Update to 1.1.0 succeeded" in text and text

    shown = wait(updated, "the update did not succeed", OPERATION_TIMEOUT)
    assert "Reopen its panes to use it." in shown, shown
    assert (*FIXTURE, "", SHA_B) in registry(), registry()
    marker = Path(fixture_plugin()["plugin_root"]) / "build-marker.txt"
    assert marker.read_text().strip() == "1.1.0", marker.read_text()
    close_all(tab)
    fixture_at_a()
    print("update_from_details_ok", flush=True)


def flat(pane):
    """The text of `pane` on one line: a notice wraps anywhere."""
    return " ".join(read(pane).split())


def prove_update_from_sidebar():
    """A click on the card's Update button updates the fixture from A to B,
    opening nothing; a failed build keeps B and brings the button back."""
    fixture_at_a()
    write_catalog(SHA_B)
    sidebar = open_sidebar()
    wait(lambda: listed(read(sidebar)), "the catalogue did not load")
    tab = next(p["tab_id"] for p in panes() if p["pane_id"] == sidebar)
    type_text(sidebar, "fixture")
    wait(lambda: " All 2 " in (text := read(sidebar)) and "Update to 1.1.0" in text,
         "the card did not offer the update")
    click_focused(sidebar, "Update to 1.1.0")

    def running():
        assert not details_panes(tab), "the update opened a details pane"
        return "Updating…" in read(sidebar)

    wait(running, "the card did not show the running update")
    wait(lambda: (*FIXTURE, "", SHA_B) in registry(), "the update did not reach the registry",
         OPERATION_TIMEOUT)
    marker = Path(fixture_plugin()["plugin_root"]) / "build-marker.txt"
    assert marker.read_text().strip() == "1.1.0", marker.read_text()
    wait(lambda: "updated to 1.1.0" in (text := flat(sidebar)) and "Reopen its panes" in text,
         "the sidebar did not tell the update succeeded")
    wait(lambda: "installed · Test fixture." in read(sidebar), "the card did not return to 'installed'")
    assert not details_panes(tab), "the update opened a details pane"
    toggle()
    wait(lambda: with_token(SIDEBAR_TOKEN) is None, "the action did not close the sidebar")

    write_catalog(SHA_C)
    sidebar = open_sidebar()
    wait(lambda: listed(read(sidebar)), "the catalogue did not load")
    type_text(sidebar, "fixture")
    wait(lambda: " All 2 " in (text := read(sidebar)) and "Update to 1.2.0" in text,
         "the card did not offer the update")
    click_focused(sidebar, "Update to 1.2.0")
    wait(lambda: "failed" in flat(sidebar), "the failed build was not told", OPERATION_TIMEOUT)
    assert (*FIXTURE, "", SHA_B) in registry(), registry()
    wait(lambda: "Update to 1.2.0" in read(sidebar), "the button did not come back")
    toggle()
    wait(lambda: with_token(SIDEBAR_TOKEN) is None, "the action did not close the sidebar")
    fixture_at_a()
    print("update_from_sidebar_ok", flush=True)


def tabs_line(pane):
    """The line of the filters and their counts, empty while loading."""
    return next((line for line in read(pane).split("\n") if listed(line)), "")


def only_the_update(text):
    """The list shows the root fixture's card, nothing else."""
    return ("massdo/herdr-…lace-fixture" in text and "massdo/herdr-…-fixture/alt" not in text
            and "Filler" not in text)


def prove_updates_tab():
    """While an installed plugin has an update, the Updates tab lists it
    alone: a click or @outdated picks it, Esc leaves it."""
    fixture_at_a()
    write_catalog(SHA_B)
    sidebar = open_sidebar()
    # The full label when the three tabs fit, else the arrow.
    label = wait(lambda: next((label for label in ("Updates 1", "↑1") if label in tabs_line(sidebar)), None),
                 "the Updates tab did not show")
    click_focused(sidebar, label)
    wait(lambda: only_the_update(read(sidebar)), "the Updates tab did not list only the update")
    keys(sidebar, "esc")
    wait(lambda: "Filler" in read(sidebar), "Esc did not return to all plugins")
    type_text(sidebar, "@outdated")
    shown = wait(lambda: only_the_update(text := read(sidebar)) and text,
                 "@outdated did not pick the Updates tab")
    assert "@outdated" not in shown, shown
    keys(sidebar, "esc")
    wait(lambda: "Filler" in read(sidebar), "Esc did not return to all plugins")
    toggle()
    wait(lambda: with_token(SIDEBAR_TOKEN) is None, "the action did not close the sidebar")

    write_catalog(SHA_A)
    sidebar = open_sidebar()
    tabs = wait(lambda: tabs_line(sidebar), "the catalogue did not load")
    assert "Updates" not in tabs and "↑" not in tabs, tabs
    toggle()
    wait(lambda: with_token(SIDEBAR_TOKEN) is None, "the action did not close the sidebar")
    print("updates_tab_ok", flush=True)


def remove(details):
    keys(details, "r")
    shown = wait(lambda: "Enter: confirm" in (text := read(details)) and text,
                 "r did not ask for a confirmation")
    assert "source: massdo/herdr-marketplace-fixture" in shown, shown
    keys(details, "enter")
    wait(lambda: "Removal succeeded" in read(details), "the removal did not succeed", OPERATION_TIMEOUT)


def prove_uninstall():
    """Through the buttons, with the mouse."""
    sidebar, tab, details = fixture_details(SHA_A)
    wait(lambda: "· installed" in read(details), "the details pane does not show 'installed'")
    wait(lambda: "Remove (r)" in read(details), "the details pane has no Remove button")
    click_text(details, "Remove (r)")
    shown = wait(lambda: "Confirm removal (Enter)" in (text := read(details)) and text,
                 "a click on Remove did not ask for a confirmation")
    assert "Remove this plugin?" in shown, shown
    click_text(details, "Confirm removal (Enter)")
    wait(lambda: "Removal succeeded" in read(details), "the removal did not succeed", OPERATION_TIMEOUT)
    assert fixture_plugin() is None, data("plugin", "list", "--json")
    wait(lambda: "· not installed" in read(details), "the details pane does not show 'not installed'")
    wait(lambda: "installed · Test fixture." not in (text := read(sidebar)) and "Test fixture." in text,
         "the sidebar still shows 'installed'")
    print("uninstall_ok", flush=True)
    close_all(tab)


def prove_full_journey():
    """Two catalogues in turn: search, read, install A; read, switch to B;
    remove. The registry is checked at each step."""
    assert registry() == set(), registry()
    sidebar, tab, details = fixture_details(SHA_A)
    wait(lambda: "[image: fixture logo]" in read(details), "the README was not read")
    confirm_install(details, "Install")
    wait(lambda: "Install of c8268d4 succeeded" in read(details),
         "the install did not succeed", OPERATION_TIMEOUT)
    assert registry() == {(*FIXTURE, "", SHA_A)}, registry()
    close_all(tab)

    sidebar, tab, details = fixture_details(SHA_B)
    wait(lambda: "[image: fixture logo]" in read(details), "the README was not read")
    confirm_install(details, f"installed commit: {SHA_A}")
    wait(lambda: "Install of 1be1b7b succeeded" in read(details),
         "the switch did not succeed", OPERATION_TIMEOUT)
    assert registry() == {(*FIXTURE, "", SHA_B)}, registry()

    remove(details)
    assert registry() == set(), registry()
    print("full_journey_ok", flush=True)
    close_all(tab)


def opened():
    return OPENED.read_text().splitlines() if OPENED.exists() else []


def prove_readme():
    """herdr-sidebar's README: its image reaches the client through the
    kitty graphics protocol; a relative link and the GitHub button open."""
    write_catalog(SHA_A)
    sidebar = open_sidebar()
    wait(lambda: listed(read(sidebar)), "the catalogue did not load")
    tab = next(p["tab_id"] for p in panes() if p["pane_id"] == sidebar)
    sent = CLIENT_LOG.stat().st_size
    details = open_details(sidebar, "herdr-sidebar", " All 1 ")
    wait(lambda: "The sidebar your terminal was missing" in read(details),
         "the herdr-sidebar README was not rendered")
    wait(lambda: b"\x1b_Ga=t" in CLIENT_LOG.read_bytes()[sent:],
         "the README image did not reach the client", 60)
    assert "[image: The sidebar docked" not in read(details), read(details)
    print("readme_image_ok", flush=True)

    click_text(details, "repo README")
    wait(lambda: f"https://github.com/alexarthurs/herdr-sidebar/blob/{SIDEBAR_SHA}/README.md" in opened(),
         f"the relative link did not open: {opened()}")
    click_text(details, "Open on GitHub (o)")
    page = f"https://github.com/alexarthurs/herdr-sidebar/tree/{SIDEBAR_SHA}/plugins/herdr-sidebar"
    wait(lambda: page in opened(), f"the GitHub page did not open: {opened()}")
    print("readme_links_ok", flush=True)
    close_all(tab)


def layer_frames(since):
    """Images Herdr sent the client on a pane layer since byte `since` of
    the client log, each with its size and the cells it covers. The pane
    draws its own images as raw pixels; a layer sends PNG."""
    log = CLIENT_LOG.read_bytes()[since:]
    cells = {i: (int(c), int(r)) for i, c, r in
             re.findall(rb"\x1b_Ga=p,i=(\d+),p=\d+,c=(\d+),r=(\d+),z=1", log)}
    return [(int(w), int(h), cells.get(i))
            for w, h, i in re.findall(rb"\x1b_Ga=t,t=d,f=100,s=(\d+),v=(\d+),i=(\d+)", log)]


def prove_animation():
    """herdr-marketplace's README opens on an animated WebP: Herdr draws its
    frames in turn on a layer of the pane, each within the cells of the
    picture (8 × 17 pixels a cell here), cut to the rows the pane shows, and
    none once the picture is scrolled away."""
    INDEX.write_text(json.dumps({
        "schemaVersion": 1, "generatedAt": "2026-09-25T00:00:00Z",
        "pluginCount": 1, "repositoryCount": 1,
        "plugins": [repo("massdo", "herdr-marketplace", MARKETPLACE_SHA, 1, [
            manifest("herdr-plugin.toml", "herdr-marketplace", "herdr-marketplace",
                     "Search, read and install Herdr plugins from a sidebar."),
        ])],
    }))
    sidebar = open_sidebar()
    wait(lambda: listed(read(sidebar)), "the catalogue did not load")
    tab = next(p["tab_id"] for p in panes() if p["pane_id"] == sidebar)
    details = open_details(sidebar, "marketplace", " All 1 ")
    wait(lambda: "Real marketplace screens" in read(details),
         "the herdr-marketplace README was not rendered")
    since = CLIENT_LOG.stat().st_size
    frames = wait(lambda: len(found := layer_frames(since)) >= 10 and found,
                  "the demo did not play", 90)
    columns, rows = next(cells for _, _, cells in frames if cells)
    assert all(w <= columns * 8 and h <= rows * 17 for w, h, _ in frames), frames
    print("animation_plays_ok", flush=True)

    for _ in range(rows // 2 + 4):
        keys(details, "down")
    since = CLIENT_LOG.stat().st_size
    cut = wait(lambda: [f for f in layer_frames(since) if f[2] and f[2][1] < rows],
               "the demo was not cut to the rows shown", 30)
    assert all(h <= r * 17 for _, h, (_, r) in cut), cut
    type_text(details, END)
    time.sleep(1)
    since = CLIENT_LOG.stat().st_size
    time.sleep(2)
    assert not layer_frames(since), "the demo played once scrolled away"
    print("animation_follows_scroll_ok", flush=True)
    close_all(tab)


def ffmpeg_pids():
    """Processes of the private FFmpeg that e2e.sh built."""
    found = subprocess.run(["pgrep", "-f", os.environ["HERDR_MARKETPLACE_FFMPEG"]],
                           capture_output=True, text=True)
    return [int(pid) for pid in found.stdout.split()]


def video_files():
    """Videos the plugin downloaded into its state folder."""
    return list(Path(os.environ["XDG_STATE_HOME"]).rglob("videos/**/*.mp4"))


def resident_kb(pid):
    found = subprocess.run(["ps", "-o", "rss=", "-p", str(pid)], capture_output=True, text=True)
    return int(found.stdout.strip())


def play_video(details, since, automatic=False):
    """Waits for autoplay, or clicks once for replay. A failed transfer can
    be retried manually; a cached replay needs no Downloading screen."""
    for attempt in range(5):
        if not automatic or attempt:
            if "[Play]" in read(details):
                keys(details, "p")
            else:
                click_text(details, "Play video (9.8 MB)")
        outcome = wait(lambda: ("played" if layer_frames(since) else
                                "failed" if "Could not play the video" in read(details) else None),
                       "the video neither played nor failed", 60)
        if outcome == "played":
            return
    raise AssertionError("the video failed to play five times")


def prove_video():
    """Autoplay, progressive delivery, bounded frames, controls, A-B-A
    reuse, and cleanup owned by Marketplace rather than Details."""
    write_catalog(SHA_A)
    sidebar = open_sidebar()
    wait(lambda: listed(read(sidebar)), "the catalogue did not load")
    tab = next(p["tab_id"] for p in panes() if p["pane_id"] == sidebar)
    since = CLIENT_LOG.stat().st_size
    opened = time.monotonic()
    details = open_details(sidebar, "terminal browser", " All 1 ")
    play_video(details, since, automatic=True)
    first_ms = round((time.monotonic() - opened) * 1000)
    assert len(video_files()) == 1, video_files()
    cached = video_files()[0]
    complete_at_start = json.loads(cached.with_suffix(".json").read_text())["complete"]
    print(f"video_first_frame_ms={first_ms} complete_at_start={complete_at_start}", flush=True)
    started = time.monotonic()
    frames = wait(lambda: len(found := layer_frames(since)) >= 10 and found,
                  "the video did not play", 60)
    placed = [(w, h, cells) for w, h, cells in frames if cells]
    assert placed and all(w <= 800 and w <= columns * 8 and h <= rows * 17
                          for w, h, (columns, rows) in placed), frames
    [ffmpeg] = ffmpeg_pids()
    pane = int(subprocess.run(["ps", "-o", "ppid=", "-p", str(ffmpeg)],
                              capture_output=True, text=True).stdout.strip())
    time.sleep(max(0, started + 3 - time.monotonic()))
    early = resident_kb(pane)
    since = CLIENT_LOG.stat().st_size
    time.sleep(5)
    assert len(layer_frames(since)) <= 55, len(layer_frames(since))
    time.sleep(max(0, started + 15 - time.monotonic()))
    late = resident_kb(pane)
    assert late - early < 30 * 1024, (early, late)
    def complete():
        state = json.loads(cached.with_suffix(".json").read_text())
        assert state["error"] is None, state
        return state["complete"]

    wait(complete, "the background transfer did not finish", 280)
    cached_stat = cached.stat()
    print("video_plays_ok", flush=True)

    keys(details, "p")
    wait(lambda: "[Play]" in read(details), "p did not pause the video", 5)
    time.sleep(0.3)
    since = CLIENT_LOG.stat().st_size
    time.sleep(1)
    assert not layer_frames(since), "paused video kept presenting frames"
    assert ffmpeg_pids(), "pause restarted or killed the decoder"
    print("video_pauses_ok", flush=True)

    def position():
        found = re.search(r"(\d+):(\d{2})/\d+:\d{2}", read(details))
        return int(found[1]) * 60 + int(found[2]) if found else None

    before = position()
    assert before is not None, read(details)
    keys(details, "right")
    wait(lambda: position() is not None and position() >= before + 4,
         "seeking did not move the position", 10)
    assert "[Play]" in read(details), "seeking lost the paused state"
    keys(details, "p")
    since = CLIENT_LOG.stat().st_size
    wait(lambda: len(layer_frames(since)) >= 3, "resume did not present frames", 10)
    print("video_seeks_resumes_ok", flush=True)

    replacement = open_details(sidebar, "fixture", " All 2 ")
    assert replacement == details, "changing plugins replaced the Details pane"
    wait(lambda: not ffmpeg_pids(), "switching plugins left the previous decoder", 5)
    assert video_files() == [cached], "switching plugins evicted the cached video"
    since = CLIENT_LOG.stat().st_size
    details = open_details(sidebar, "terminal browser", " All 1 ")
    play_video(details, since, automatic=True)
    assert video_files() == [cached], "returning to A created a new video file"
    assert cached.stat().st_ino == cached_stat.st_ino
    assert cached.stat().st_mtime_ns == cached_stat.st_mtime_ns, "returning to A downloaded again"
    print("video_plugin_return_reuses_cache_ok", flush=True)

    keys(details, "q")
    wait(lambda: not details_panes(tab) and not ffmpeg_pids(), "q left a details pane or decoder", 5)
    assert video_files() == [cached], "Details closure deleted Marketplace's cache"
    print("video_details_close_keeps_cache_ok", flush=True)
    since = CLIENT_LOG.stat().st_size
    details = open_details(sidebar, "terminal browser", " All 1 ")
    play_video(details, since, automatic=True)
    herdr("pane", "close", details)
    wait(lambda: not ffmpeg_pids(), "external Details closure left FFmpeg", 5)
    assert video_files() == [cached]
    print("video_external_details_close_keeps_cache_ok", flush=True)

    since = CLIENT_LOG.stat().st_size
    details = open_details(sidebar, "terminal browser", " All 1 ")
    play_video(details, since, automatic=True)
    toggle()
    wait(lambda: with_token(SIDEBAR_TOKEN) is None, "the action did not close Marketplace")
    wait(lambda: not ffmpeg_pids() and not video_files(),
         "closing Marketplace left FFmpeg or cached videos", 5)
    assert details_panes(tab), "closing Marketplace unexpectedly closed Details"
    print("video_marketplace_close_cleans_cache_ok", flush=True)
    close_all(tab)


def main():
    check_isolation()
    if os.environ.get("HERDR_MARKETPLACE_E2E_ONLY") == "video":
        prove_video()
        print("video_journey_ok", flush=True)
        return
    prove_sidebar()
    prove_details()
    prove_preview()
    prove_single_details()
    prove_fast_sidebar_close()
    prove_readme()
    prove_animation()
    prove_video()
    prove_install_preview()
    prove_install()
    prove_switch()
    prove_failed_build()
    prove_details_closed_during_install()
    prove_update_shown()
    prove_update_from_details()
    prove_update_from_sidebar()
    prove_updates_tab()
    prove_uninstall()
    prove_full_journey()
    print("journey_ok", flush=True)


def diagnostics():
    print("== diagnostics ==", flush=True)
    print("video_files", video_files(), "ffmpeg_pids", ffmpeg_pids(), flush=True)
    for file in video_files():
        print("video_state", file, file.with_suffix(".json").read_text(), flush=True)
        owner = file.parent.name
        if owner.isdigit():
            info = subprocess.run(["ps", "-o", "pid=,ppid=,stat=,command=", "-p", owner], capture_output=True, text=True)
            print("video_owner", info.stdout, flush=True)
    try:
        print(herdr("plugin", "log", "list", "--plugin", "herdr-marketplace", "--limit", "5"))
        for pane in panes():
            print(pane, flush=True)
            print(data("pane", "layout", "--pane", pane["pane_id"])["layout"], flush=True)
            print(read(pane["pane_id"]), flush=True)
    except Exception as error:
        print(f"diagnostics unavailable: {error}", flush=True)


if __name__ == "__main__":
    CLIENT = Client()
    CLIENT.start()
    try:
        main()
    except Exception:
        diagnostics()
        raise
    finally:
        CLIENT.close()
