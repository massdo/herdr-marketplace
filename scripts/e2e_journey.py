#!/usr/bin/env python3
"""Marketplace journey in the disposable Herdr profile prepared by e2e.sh."""

import fcntl
import json
import os
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
SIDEBAR_TOKEN = "herdr_marketplace_sidebar"
DETAILS_TOKEN = "herdr_marketplace_details"

# `herdr pane send-keys` has no names for these keys: send their sequences.
END = "\x1b[F"

FIXTURE = ("massdo", "herdr-marketplace-fixture")
SHA_A = "c8268d42a98d9140254f4bf4ca13c23a587faed8"  # 1.0.0, harmless build
SHA_B = "1be1b7bb9d9ad3d8733a66d132b87c908ff210c6"  # 1.1.0, harmless build
SHA_C = "bc4d8b84d062b8048b5d89647e7110c9f5a05561"  # 1.2.0, build fails on purpose
BROWSER_SHA = "ff8f17077e52a8b582a4659f3424cd8abbb5ce1d"
# Installs fetch the fixture from GitHub and build it.
OPERATION_TIMEOUT = 180


class Client:
    """Attached 200x50 client, as in herdr-npm's journey: without one, a
    headless server does not size pane terminals to the layout."""

    def start(self):
        self.pid, self.master = pty.fork()
        if self.pid == 0:
            os.execvp("herdr", ["herdr", "--session", SESSION])
        fcntl.ioctl(self.master, termios.TIOCSWINSZ, struct.pack("HHHH", 50, 200, 0, 0))
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
    # 30 inner columns: a long owner/repo/subdir loses its middle, never its ends.
    assert "Terminal Browser" in shown and "zenbu-labs/term…r/herdr-plugin" in shown, shown
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
    assert "massdo/herdr-ma…tplace-fixture" in shown, shown
    assert "massdo/herdr-ma…ce-fixture/alt" in shown, shown
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


def details_panes(tab):
    return [p for p in panes()
            if p["tab_id"] == tab and (p.get("tokens") or {}).get(DETAILS_TOKEN) == "v1"]


def focused():
    return next((p["pane_id"] for p in panes() if p.get("focused")), None)


def open_details(sidebar, query, expected_results):
    """Search in the sidebar, press Enter on the first result, return its details pane."""
    type_text(sidebar, query)
    wait(lambda: expected_results in read(sidebar), f"search {query!r} did not settle")
    before = {p["pane_id"] for p in panes()}
    keys(sidebar, "enter")
    details = wait(lambda: next((p for p in panes() if p["pane_id"] not in before
                               and (p.get("tokens") or {}).get(DETAILS_TOKEN) == "v1"), None),
                 f"Enter on {query!r} did not open a details pane")["pane_id"]
    keys(sidebar, *["backspace"] * len(query))
    return details


def prove_details():
    write_catalog(SHA_A)
    sidebar = open_sidebar()
    wait(lambda: "résultats" in read(sidebar), "the catalogue did not load")
    tab = next(p["tab_id"] for p in panes() if p["pane_id"] == sidebar)

    details = open_details(sidebar, "terminal browser", "1 résultat")
    shown = wait(lambda: "open-split" in (text := read(details)) and text,
                 "the terminal-browser README was not rendered")
    assert "zenbu-labs/terminal-browser/herdr-plugin" in shown, shown
    assert f"commit {BROWSER_SHA[:7]}" in shown, shown
    assert "terminal-browser herdr plugin" in shown, shown
    assert "herdr plugin install zenbu-labs/terminal-browser/herdr-plugin" in shown, shown
    assert "```" not in shown and "### " not in shown, shown
    assert focused() == details, "the details pane did not take focus"
    print("details_terminal_browser_ok", flush=True)

    keys(details, "esc")
    wait(lambda: not details_panes(tab), "escape did not close the details pane")
    wait(lambda: focused() == sidebar, "focus did not return to the sidebar")
    print("details_escape_ok", flush=True)

    details = open_details(sidebar, "fixture", "2 résultats")
    shown = wait(lambda: "[image : fixture logo]" in (text := read(details)) and text,
                 "the fixture README was not rendered")
    assert "massdo/herdr-marketplace-fixture" in shown and f"commit {SHA_A[:7]}" in shown, shown
    assert "End of the fixture README" not in shown, shown
    type_text(details, END)
    wait(lambda: "End of the fixture README, version 1.0.0." in read(details),
         "the last README line was not reachable")
    print("details_last_line_ok", flush=True)

    type_text(sidebar, "fixture")
    wait(lambda: "2 résultats" in read(sidebar), "search fixture did not settle")
    keys(sidebar, "down", "enter")
    alt = wait(lambda: next((p["pane_id"] for p in details_panes(tab) if p["pane_id"] != details), None),
               "Enter on the alt source did not open its details pane")
    shown = wait(lambda: "Pas de README.md dans alt/" in (text := read(alt)) and text,
                 "the root README fallback was not signalled")
    assert "massdo/herdr-marketplace-fixture/alt" in shown, shown
    assert "herdr-marketplace fixture" in shown, shown
    assert [p["pane_id"] for p in details_panes(tab)] == [alt], "two openings left more than one details pane"
    print("details_fallback_and_single_pane_ok", flush=True)

    toggle()
    wait(lambda: with_token(SIDEBAR_TOKEN) is None, "the action did not close the sidebar")
    assert "massdo/herdr-marketplace-fixture/alt" in read(alt), "the details pane lost its plugin"
    keys(alt, "esc")
    wait(lambda: not details_panes(tab), "escape did not close the details pane")
    wait(lambda: focused() is not None and focused() in others(), "focus did not go to a remaining pane")
    print("details_outlives_sidebar_ok", flush=True)


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
    wait(lambda: "résultats" in read(sidebar), "the catalogue did not load")
    tab = next(p["tab_id"] for p in panes() if p["pane_id"] == sidebar)
    details = open_details(sidebar, "fixture", "2 résultats")
    wait(lambda: "[image : fixture logo]" in read(details), "the fixture README was not rendered")
    before = registry()

    keys(details, "i")
    shown = wait(lambda: "Entrée : confirmer" in (text := read(details)) and text,
                 "i did not open the install preview")
    assert "source : massdo/herdr-marketplace-fixture" in shown, shown
    assert f"commit : {SHA_A}" in shown, shown
    assert "• /bin/sh build.sh" in shown, shown
    assert "• hello : /bin/echo hello from herdr-marketplace-fixture" in shown, shown
    assert "Ce plugin exécutera du code avec vos droits." in shown, shown
    print("install_preview_ok", flush=True)

    keys(details, "esc")
    wait(lambda: "i : installer" in read(details), "escape did not cancel the preview")
    time.sleep(2)
    assert registry() == before, (before, registry())
    assert fixture_log() == "", fixture_log()
    assert details_panes(tab), "cancelling the preview closed the details pane"
    print("install_cancel_ok", flush=True)

    keys(details, "esc")
    wait(lambda: not details_panes(tab), "escape did not close the details pane")
    toggle()
    wait(lambda: with_token(SIDEBAR_TOKEN) is None, "the action did not close the sidebar")


def fixture_plugin():
    return next((p for p in data("plugin", "list", "--json")["plugins"]
                 if p["plugin_id"] == "herdr-marketplace-fixture"), None)


def fixture_details(sha):
    """Sidebar on a catalogue with the fixture at `sha`, and the root fixture's details pane."""
    write_catalog(sha)
    sidebar = open_sidebar()
    wait(lambda: "résultats" in read(sidebar), "the catalogue did not load")
    tab = next(p["tab_id"] for p in panes() if p["pane_id"] == sidebar)
    details = open_details(sidebar, "fixture", "2 résultats")
    wait(lambda: f"commit {sha[:7]}" in read(details), "the fixture details pane did not open")
    return sidebar, tab, details


def confirm_install(details, expected):
    keys(details, "i")
    shown = wait(lambda: "Entrée : confirmer" in (text := read(details)) and text,
                 "i did not open the install preview")
    assert expected in shown, shown
    keys(details, "enter")


def close_all(tab):
    for pane in details_panes(tab):
        keys(pane["pane_id"], "esc")
    wait(lambda: not details_panes(tab), "escape did not close the details pane")
    if with_token(SIDEBAR_TOKEN):
        toggle()
        wait(lambda: with_token(SIDEBAR_TOKEN) is None, "the action did not close the sidebar")


def prove_install():
    sidebar, tab, details = fixture_details(SHA_A)
    confirm_install(details, "Installation")
    wait(lambda: "Installation de c8268d4 réussie" in read(details),
         "the install did not succeed", OPERATION_TIMEOUT)
    assert (*FIXTURE, "", SHA_A) in registry(), registry()
    marker = Path(fixture_plugin()["plugin_root"]) / "build-marker.txt"
    assert marker.read_text().strip() == "1.0.0", marker.read_text()
    wait(lambda: "· installé" in read(details), "the details pane does not show « installé »")
    wait(lambda: "installé · Test fixture." in read(sidebar), "the sidebar does not show « installé »")
    print("install_from_details_ok", flush=True)
    close_all(tab)


def prove_switch():
    herdr("plugin", "install", "/".join(FIXTURE), "--ref", SHA_A, "--yes")
    assert (*FIXTURE, "", SHA_A) in registry(), registry()
    sidebar, tab, details = fixture_details(SHA_B)
    confirm_install(details, f"commit installé : {SHA_A}")
    wait(lambda: "Installation de 1be1b7b réussie" in read(details),
         "the switch did not succeed", OPERATION_TIMEOUT)
    assert (*FIXTURE, "", SHA_B) in registry(), registry()
    print("switch_commit_ok", flush=True)
    close_all(tab)


def prove_failed_build():
    sidebar, tab, details = fixture_details(SHA_C)
    confirm_install(details, "Changement de commit")
    shown = wait(lambda: "Échec de l'installation de bc4d8b8" in (text := read(details)) and text,
                 "the failed build was not reported", OPERATION_TIMEOUT)
    assert f"Registre : installé à {SHA_B}" in shown, shown
    assert "Plugin was not installed." in shown or "plugin build failed" in shown, shown
    assert (*FIXTURE, "", SHA_B) in registry(), registry()
    print("failed_build_ok", flush=True)
    close_all(tab)


def prove_details_closed_during_install():
    sidebar, tab, details = fixture_details(SHA_A)
    confirm_install(details, "Changement de commit")
    wait(lambda: "en cours" in read(details), "the details pane did not show the running install")
    keys(details, "esc")
    wait(lambda: not details_panes(tab), "escape did not close the details pane")
    wait(lambda: (*FIXTURE, "", SHA_A) in registry(),
         "the install stopped with its details pane", OPERATION_TIMEOUT)
    details = open_details(sidebar, "fixture", "2 résultats")
    wait(lambda: "Installation de c8268d4 réussie" in read(details),
         "the reopened details pane did not show the result")
    print("details_closed_during_install_ok", flush=True)
    close_all(tab)


def remove(details):
    keys(details, "r")
    shown = wait(lambda: "Entrée : confirmer" in (text := read(details)) and text,
                 "r did not ask for a confirmation")
    assert "source : massdo/herdr-marketplace-fixture" in shown, shown
    keys(details, "enter")
    wait(lambda: "Retrait réussi" in read(details), "the removal did not succeed", OPERATION_TIMEOUT)


def prove_uninstall():
    sidebar, tab, details = fixture_details(SHA_A)
    wait(lambda: "· installé" in read(details), "the details pane does not show « installé »")
    remove(details)
    assert fixture_plugin() is None, data("plugin", "list", "--json")
    wait(lambda: "· non installé" in read(details), "the details pane does not show « non installé »")
    wait(lambda: "installé · Test fixture." not in (text := read(sidebar)) and "Test fixture." in text,
         "the sidebar still shows « installé »")
    print("uninstall_ok", flush=True)
    close_all(tab)


def prove_full_journey():
    """Two catalogues in turn: search, read, install A; read, switch to B;
    remove. The registry is checked at each step."""
    assert registry() == set(), registry()
    sidebar, tab, details = fixture_details(SHA_A)
    wait(lambda: "[image : fixture logo]" in read(details), "the README was not read")
    confirm_install(details, "Installation")
    wait(lambda: "Installation de c8268d4 réussie" in read(details),
         "the install did not succeed", OPERATION_TIMEOUT)
    assert registry() == {(*FIXTURE, "", SHA_A)}, registry()
    close_all(tab)

    sidebar, tab, details = fixture_details(SHA_B)
    wait(lambda: "[image : fixture logo]" in read(details), "the README was not read")
    confirm_install(details, f"commit installé : {SHA_A}")
    wait(lambda: "Installation de 1be1b7b réussie" in read(details),
         "the switch did not succeed", OPERATION_TIMEOUT)
    assert registry() == {(*FIXTURE, "", SHA_B)}, registry()

    remove(details)
    assert registry() == set(), registry()
    print("full_journey_ok", flush=True)
    close_all(tab)


def main():
    check_isolation()
    prove_sidebar()
    prove_details()
    prove_install_preview()
    prove_install()
    prove_switch()
    prove_failed_build()
    prove_details_closed_during_install()
    prove_uninstall()
    prove_full_journey()
    print("journey_ok", flush=True)


def diagnostics():
    print("== diagnostics ==", flush=True)
    try:
        print(herdr("plugin", "log", "list", "--plugin", "herdr-marketplace", "--limit", "5"))
        for pane in panes():
            print(pane, flush=True)
            print(data("pane", "layout", "--pane", pane["pane_id"])["layout"], flush=True)
            print(read(pane["pane_id"]), flush=True)
    except Exception as error:
        print(f"diagnostics unavailable: {error}", flush=True)


if __name__ == "__main__":
    client = Client()
    client.start()
    try:
        main()
    except Exception:
        diagnostics()
        raise
    finally:
        client.close()
