# herdr-marketplace

The Herdr plugin marketplace, in a sidebar: search the public plugin
catalog, read a plugin's README, install it, move it to the catalog's commit
or remove it, without leaving Herdr or typing `herdr plugin` commands. It
behaves like the VS Code extensions marketplace, within the limits of a
terminal.

## Requirements

- Herdr **0.9.1**
- Rust **1.89** (edition 2024) to build the binary. If Homebrew's Cargo comes
  before the toolchain installed by rustup, run
  `export PATH="$HOME/.cargo/bin:$PATH"` first.
- macOS or Linux, `git` (Herdr uses it to install plugins) and network access
  (the catalog, READMEs and manifests come from GitHub).

## Link the local checkout

```sh
git clone https://github.com/massdo/herdr-marketplace
cd herdr-marketplace
sh scripts/build.sh
herdr plugin link "$PWD" --enabled
```

`scripts/build.sh` builds `target/release/herdr-marketplace`, which the
`herdr-plugin.toml` manifest runs. Run it again after updating the checkout.

## Open the marketplace

From a Herdr pane:

```sh
herdr plugin action invoke herdr-marketplace.toggle
```

The sidebar opens on the left of the current tab, about 32 columns wide; the
same command closes it. For a shortcut, add this to your `config.toml`
yourself:

```toml
[[keys.command]]
key = "prefix+shift+m"
type = "plugin_action"
command = "herdr-marketplace.toggle"
description = "Marketplace"
```

## Full journey

Everything works with the mouse or the keyboard. A click on a pane without
the focus only gives it the focus: clicking the sidebar to type a search
never opens a plugin.

1. **Search.** The catalog loads when the sidebar opens. Type: the list
   filters on every key, over the name, id, description, owner/repo and
   topics of each plugin. The search is fuzzy, with
   [frizbee](https://github.com/saghen/frizbee), the matcher of skim, atuin
   and television: the letters of each word must appear in order and close
   together, so `reviw` finds "review"; a word that matches nothing that way
   is tried again with typos, so `reveiw` finds it too. Results come by
   relevance: a match in the name first, then in the id, a topic or
   owner/repo, then in the description; the most starred first among equals.
   The wheel scrolls the list; ↑↓, Page Up/Down, Home and End move the
   selection; Esc clears the search, then closes the sidebar. Each plugin
   shows its name, stars, owner/repo, the start of its description, and the
   marks "installed", "incompatible" or "not in catalog". Incompatible
   plugins that are not installed are hidden; the sidebar says how many match
   the search.
2. **Read.** A click on a plugin, or Enter, opens its details in a pane next
   to the sidebar: its README, rendered, at the catalog's commit. The wheel,
   ↑↓, Page Up/Down, Home and End scroll it; a click on the commit, or `s`,
   shows the full SHA; Esc closes the details pane.
3. **Install.** Under the header, buttons show what can be done, each with
   its key. **Install (i)** opens the preview: source, full SHA, build and
   startup commands, events, actions and panes. **Confirm install (Enter)**
   runs it, **Cancel (Esc)** runs nothing. The install goes on if you close
   the details pane; reopen it to see the result, confirmed by the Herdr
   registry.
4. **Switch commit.** For a plugin already installed at another commit,
   **Switch to <commit> (i)** moves it to the catalog's commit; the preview
   shows both SHAs.
5. **Remove.** For an installed plugin, **Remove (r)**, then **Confirm
   removal (Enter)**.

One install or removal at a time. Locally linked plugins, such as the
marketplace itself, are neither listed nor removed.

## Catalog

The source is the public index `https://assets.herdr.dev/plugins/index.json`,
loaded again every time the sidebar opens. `HERDR_MARKETPLACE_INDEX_URL`
replaces it with another http(s) or `file://` URL.

## Checks

- `sh scripts/check.sh`: format, lint and tests, offline.
- `cargo test -- --ignored`: loads the real index over the network.
- `sh scripts/e2e.sh`: the full journey in a disposable Herdr profile (server,
  configuration and state isolated under `/tmp`), never in your daily session.
  It installs the test plugin `massdo/herdr-marketplace-fixture` from GitHub.

## V1 limits

No Windows, a single sort (relevance, then stars), no version picker, no
catalog refresh while the sidebar is open. The marketplace is used linked
from a local checkout.

The Herdr socket client, the left dock and the launcher lock come from
herdr-npm and herdr-sidebar (MIT): see `NOTICE`.
