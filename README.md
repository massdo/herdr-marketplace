<div align="center">

# herdr-marketplace

**Search, read and install [Herdr](https://herdr.dev) plugins without leaving your terminal.**

[![CI](https://github.com/massdo/herdr-marketplace/actions/workflows/ci.yml/badge.svg)](https://github.com/massdo/herdr-marketplace/actions/workflows/ci.yml)
[![Herdr 0.9.1+](https://img.shields.io/badge/Herdr-0.9.1%2B-89b4fa)](https://herdr.dev)
[![macOS | Linux](https://img.shields.io/badge/platform-macOS%20%7C%20Linux-a6e3a1)](#requirements)
[![License: MIT](https://img.shields.io/badge/license-MIT-f2b82d)](LICENSE)

![The marketplace sidebar in Herdr: a search with a typo finds Annotate, its README opens beside the list, the install preview shows what will run, and Enter installs it.](docs/demo/demo.webp)

<sub>Real marketplace screens. The plugin shown is <a href="https://github.com/plannotator/herdr-annotate">Annotate</a> by plannotator.</sub>

</div>

## Install

```sh
herdr plugin install massdo/herdr-marketplace
```

Herdr builds the plugin first, which takes a few minutes the first time. Then,
from any Herdr pane, open the marketplace: a sidebar on the left of the
current tab. The same command closes it.

```sh
herdr plugin action invoke herdr-marketplace.toggle
```

To open it with <kbd>prefix</kbd> <kbd>shift</kbd>+<kbd>M</kbd> instead, add
this to Herdr's `config.toml`:

```toml
[[keys.command]]
key = "prefix+shift+m"
type = "plugin_action"
command = "herdr-marketplace.toggle"
description = "Marketplace"
```

## Features

It works like the extensions view of VS Code, in a sidebar next to your panes.

- **Search the whole catalog.** 1,300+ plugins, filtered as you type by name,
  topic, author or description. Typos are forgiven: `anotate` finds Annotate.
- **Read before you install.** Every README is drawn like on GitHub, with
  headings, code, tables, alerts, images and links, at the exact commit you
  would install.
- **Install with one key.** `i` previews the source, the commit and the
  commands the plugin declares; `Enter` confirms, `Esc` cancels. The same key
  moves an installed plugin to the catalog's commit, and `r` removes it.
- **Keyboard or mouse.** Click, scroll and drag to copy text, or never leave
  the keyboard.

## Keys

| Key | Where | Action |
| --- | --- | --- |
| Type | Sidebar | Search |
| `↑` `↓` `PgUp` `PgDn` `Home` `End` | Sidebar, details | Move, scroll |
| `Enter` | Sidebar | Open the selected plugin |
| `Tab` | Sidebar | Switch between **All** and **Installed** |
| `i` | Details | Install, or switch to the catalog's commit |
| `r` | Details | Remove |
| `o` | Details | Open on GitHub |
| `s` | Details | Show the full commit SHA |
| `Enter` / `Esc` | Install or removal | Confirm / cancel |
| `Esc` | Sidebar | Clear the search, show All, then close |
| `Esc` | Details | Close the pane |

> [!IMPORTANT]
> A community plugin, not an official Herdr product. The catalog is indexed
> automatically: its plugins are not reviewed or endorsed by Herdr or by this
> plugin. Installing one runs its code with your permissions, so read its
> source and the install preview before you confirm.

## Requirements

- Herdr 0.9.1 or later (tested with 0.9.1), on macOS or Linux.
- `git`, which Herdr installs plugins with, and network access: the catalog,
  READMEs and manifests come from GitHub.
- Rust 1.89 or later, to build the plugin. If Homebrew's Cargo comes before
  rustup's on your `PATH`, run `export PATH="$HOME/.cargo/bin:$PATH"` first.

To update, run the install command again, then close and reopen the
marketplace.

## Learn more

<details>
<summary>Search, details and installs</summary>

- **Search.** The catalog loads when the sidebar opens. Matching is fuzzy, with
  [frizbee](https://github.com/saghen/frizbee): the letters of each word must
  appear in order and close together, so `reviw` finds "review"; a word that
  matches nothing that way is tried again with typos, so `reveiw` finds it
  too. A match in the name comes first, then in the id, a topic or
  owner/repo, then in the description; the most starred first among equals.
- **List.** Each plugin shows its name, stars, owner/repo, the start of its
  description and the marks "installed", "incompatible" or "not in catalog".
  Incompatible plugins that are not installed are hidden, and the sidebar
  says how many. `@installed` typed in the search shows the installed plugins
  too; installed means installed from GitHub, so locally linked plugins are
  neither listed nor removed.
- **Focus.** A click on a pane without the focus only gives it the focus:
  clicking the sidebar to type a search never opens a plugin.
- **Details.** The README is the one at the catalog's commit. A click on the
  commit, or `s`, shows the full SHA. A drag selects text, even while another
  pane has the focus, and releasing the button copies it, as anywhere in
  Herdr: code comes without its frame, a line the pane cuts comes back whole,
  and images, rules and table borders are left out.
- **Installs.** The preview lists the source, the full SHA, the build and
  startup commands, events, actions and panes; for a plugin installed at
  another commit, it shows both SHAs. An install goes on if you close the
  details pane; reopen it to see the result, confirmed by the Herdr registry.
  One marketplace operation runs at a time: avoid concurrent `herdr plugin`
  commands from another terminal.

</details>

<details>
<summary>README rendering</summary>

- **Text.** Headings with GitHub's rules under the first two levels, code
  blocks colored by [syntect](https://github.com/trishume/syntect), boxed
  tables, GitHub alerts, task lists. HTML keeps its text, links, images and
  centering; nothing in a README runs.
- **Links.** A click opens a link in the browser; a relative one opens the
  file on GitHub at the commit shown, and `#anchor` scrolls to its heading.
- **Images.** PNG, JPEG, GIF, WebP and SVG, downloaded from public HTTPS
  addresses without redirects or proxies, and decoded by the plugin; animated
  images show their first frame, and videos open in the browser. Local and
  private destinations are refused after DNS resolution, and SVG images
  cannot read local files. Raster decoding is limited to 8192 pixels per side
  and 64 MiB, with two image workers per pane. Images are drawn with the kitty
  graphics protocol in terminals that support it (Ghostty, kitty, WezTerm…),
  with half blocks otherwise, and badges become small labels.
- **Files.** `README.md`, `readme.md`, `Readme.md`, `README`,
  `README.markdown` and `readme.markdown` are tried at the commit shown, in
  the plugin's folder, then at the repository root.

</details>

<details>
<summary>Configuration</summary>

| Variable | Default | Effect |
| --- | --- | --- |
| `HERDR_MARKETPLACE_INDEX_URL` | `https://assets.herdr.dev/plugins/index.json` | The catalog, over http(s) or `file://`, loaded again every time the sidebar opens. |
| `HERDR_MARKETPLACE_OPEN` | `open` on macOS, `xdg-open` elsewhere | The command that opens links. |
| `HERDR_MARKETPLACE_IMAGES` | kitty images when the terminal supports them | `blocks` draws README images with half blocks, `off` leaves them out. |

Herdr accepts kitty images even when the terminal around it cannot show them:
set `HERDR_MARKETPLACE_IMAGES=blocks` in that case, unless
`kitty_graphics = false` is set in Herdr's `[terminal]` configuration, which
makes the plugin use half blocks on its own.

</details>

<details>
<summary>Develop</summary>

```sh
git clone https://github.com/massdo/herdr-marketplace
cd herdr-marketplace
sh scripts/build.sh
herdr plugin link "$PWD" --enabled
```

`scripts/build.sh` builds `target/release/herdr-marketplace`, which the
`herdr-plugin.toml` manifest runs. Run it again after updating the checkout.

- `sh scripts/check.sh`: format, lint and tests, offline.
- `cargo test -- --ignored`: loads the real index over the network.
- `sh scripts/e2e.sh`: the full journey in a disposable Herdr profile
  (server, configuration and state isolated under `/tmp`), never in your
  daily session. It installs the test plugin
  `massdo/herdr-marketplace-fixture` from GitHub.
- [`docs/demo`](docs/demo): how the animation above is captured and
  rendered.

</details>

<details>
<summary>Limits</summary>

- No Windows, a single sort (relevance, then stars), no version picker, and
  no catalog refresh while the sidebar is open.
- README images are fetched automatically from their public HTTPS hosts,
  which see your IP address.
- The plugin does not sandbox the plugins it installs.
- If an operation worker is killed, its result is unconfirmed: check
  `herdr plugin list` and let any surviving build finish before retrying.

</details>

## License

MIT, see [LICENSE](LICENSE). The Herdr socket client, the left dock and the
launcher lock come from herdr-npm and herdr-sidebar (MIT): see
[NOTICE](NOTICE).
