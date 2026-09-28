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

On macOS and on Linux x86_64, this takes a few seconds: the install fetches
the plugin's prebuilt binary, about 15 MB, and checks it against the release's
SHA-256 sums. Elsewhere, or for a commit without a release, it builds the
plugin from source, which takes a few minutes. Then, from any Herdr pane, open
the marketplace: a sidebar on the left of the current tab. The same command
closes it.

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
- `git`, which Herdr installs plugins with, `curl` or GNU wget, and network
  access: the binary, READMEs and manifests come from GitHub, the catalog from
  herdr.dev.
- Rust 1.89 or later, only when the plugin builds from source. If Homebrew's
  Cargo comes before rustup's on your `PATH`, run
  `export PATH="$HOME/.cargo/bin:$PATH"` first.

Good to know: the marketplace downloads the catalog index once, about 270 KB
compressed, then again only when it changes. We're happy with that for now,
and V2 will bring a small server so you can skip that download.

To update, run the install command again, then close and reopen the
marketplace.

## License

MIT, see [LICENSE](LICENSE). The Herdr socket client, the left dock, the
launcher lock and the install script come from herdr-npm and herdr-sidebar
(MIT): see [NOTICE](NOTICE).
