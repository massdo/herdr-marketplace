#!/usr/bin/env python3
"""Marketplace journey in the disposable Herdr profile prepared by e2e.sh."""

import json
import os
import subprocess
import time
from pathlib import Path

SESSION = os.environ["HERDR_MARKETPLACE_E2E_SESSION"]
TMP = Path(os.environ["HERDR_MARKETPLACE_E2E_TMP"])
INDEX = TMP / "index.json"
SIDEBAR_TOKEN = "herdr_marketplace_sidebar"

# `herdr pane send-keys` has no names for these keys: send their sequences.
END = "\x1b[F"

FIXTURE = ("massdo", "herdr-marketplace-fixture")
SHA_A = "c8268d42a98d9140254f4bf4ca13c23a587faed8"
BROWSER_SHA = "ff8f17077e52a8b582a4659f3424cd8abbb5ce1d"


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


def keys(pane, *names):
    herdr("pane", "send-keys", pane, *names)


def type_text(pane, text):
    herdr("pane", "send-text", pane, text)


def toggle():
    herdr("plugin", "action", "invoke", "herdr-marketplace.toggle")


def with_token(key):
    return next((p for p in panes() if (p.get("tokens") or {}).get(key) == "v1"), None)


def others():
    """Every pane that is not the marketplace, with its tab."""
    return {
        p["pane_id"]: p["tab_id"]
        for p in panes()
        if not (p.get("tokens") or {}).get(SIDEBAR_TOKEN)
    }


def manifest(path, plugin_id, name, description, min_herdr="0.9.1"):
    return {
        "path": path, "id": plugin_id, "name": name, "version": "1.0.0",
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
    """Frozen test catalogue: the fixture at `fixture_sha`, terminal-browser,
    one incompatible plugin and fillers long enough to scroll."""
    repos = [
        repo(*FIXTURE, fixture_sha, 50, [
            manifest("herdr-plugin.toml", "herdr-marketplace-fixture",
                     "herdr-marketplace fixture", "Test fixture."),
            manifest("alt/herdr-plugin.toml", "herdr-marketplace-fixture",
                     "herdr-marketplace fixture (alt)", "Second source, same id."),
        ]),
        repo("zenbu-labs", "terminal-browser", BROWSER_SHA, 3403, [
            manifest("herdr-plugin/herdr-plugin.toml", "zenbu-labs.terminal-browser",
                     "Terminal Browser", "Open a browser inside herdr"),
        ], topics=["browser"]),
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
    shown = wait(lambda: "résultats" in (text := read(sidebar)) and text, "the catalogue did not load")
    assert f"{count - 1} résultats" in shown, shown
    assert "1 incompatible masqué" in shown, shown
    assert "Terminal Browser" in shown and "zenbu-labs/terminal-browser/herdr-plugin" in shown, shown
    assert others() == before, (before, others())
    layout = data("pane", "layout", "--pane", sidebar)["layout"]
    rects = {p["pane_id"]: p["rect"] for p in layout["panes"]}
    width = int(rects[sidebar]["width"])
    assert 28 <= width <= 40, f"sidebar width {width} is not near 32 columns: {layout}"
    assert all(rects[sidebar]["x"] < rect["x"] for pane, rect in rects.items() if pane != sidebar), layout
    print("sidebar_open_ok", flush=True)

    type_text(sidebar, "fixture")
    shown = wait(lambda: "2 résultats" in (text := read(sidebar)) and text, "the search did not filter")
    assert "Terminal Browser" not in shown, shown
    assert "massdo/herdr-marketplace-fixture/alt" in shown, shown
    type_text(sidebar, "jk")
    wait(lambda: "> fixturejk" in read(sidebar), "j and k did not reach the search")
    keys(sidebar, "backspace", "backspace")
    wait(lambda: "2 résultats" in read(sidebar), "backspace did not restore the search")
    print("search_ok", flush=True)

    # Typing never reloads: without the index file, the search still works.
    INDEX.unlink()
    type_text(sidebar, " (alt)")
    shown = wait(lambda: "1 résultat" in (text := read(sidebar)) and text, "the search stopped working")
    assert "Échec" not in shown, shown
    keys(sidebar, "esc")
    wait(lambda: f"{count - 1} résultats" in read(sidebar), "esc did not clear the search")
    print("typing_without_network_ok", flush=True)

    type_text(sidebar, END)
    wait(lambda: "Last Plugin" in read(sidebar), "the list did not reach its last entry")
    print("last_entry_ok", flush=True)

    toggle()
    wait(lambda: with_token(SIDEBAR_TOKEN) is None, "the action did not close the sidebar")
    assert others() == before, (before, others())
    print("sidebar_close_ok", flush=True)

    # INDEX is still missing: the sidebar reports it, then retries on Enter.
    sidebar = open_sidebar()
    shown = wait(lambda: "Échec du chargement" in (text := read(sidebar)) and text,
                 "the download failure was not shown")
    assert "Entrée : réessayer" in shown, shown
    write_catalog(SHA_A)
    keys(sidebar, "enter")
    wait(lambda: f"{count - 1} résultats" in read(sidebar), "the retry did not load the catalogue")
    toggle()
    wait(lambda: with_token(SIDEBAR_TOKEN) is None, "the action did not close the sidebar")
    print("retry_ok", flush=True)


def main():
    check_isolation()
    prove_sidebar()
    print("journey_ok", flush=True)


def diagnostics():
    print("== diagnostics ==", flush=True)
    try:
        print(herdr("plugin", "log", "list", "--plugin", "herdr-marketplace", "--limit", "5"))
        for pane in panes():
            print(pane, flush=True)
            print(read(pane["pane_id"]), flush=True)
    except Exception as error:
        print(f"diagnostics unavailable: {error}", flush=True)


if __name__ == "__main__":
    try:
        main()
    except Exception:
        diagnostics()
        raise
