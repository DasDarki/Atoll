# Atoll

A desktop browser built around one idea: **a tab is just a record until you look at it.**

Atoll is a native Rust shell around Chromium (via the Chromium Embedded Framework). All
workspaces, islands and tabs live in a SQLite database. At startup only that database is read and
the sidebar is drawn; exactly one engine instance is created, for the active tab. Hundreds of
saved tabs therefore cost nothing at startup.

## Features

- **Workspaces, islands, tabs**: a workspace is a complete context, islands are named, colored,
  collapsible tab groups inside it.
- **Four tab states**: *Live* (visible), *Frozen* (recently used, paused in the background),
  *Sleeping* (plain record, the normal state) and *Archived* (part of an archived island).
  Frozen tabs beyond the limit and tabs put to sleep release their engine instance.
- **Archive**: islands can be archived and restored with one click.
- **URL list import and export** in a plain text format, see below.
- **Fast startup**: the window shows the stored session in roughly 100 ms, the engine starts once
  the first frame is on screen.
- Scroll positions survive sleeping and restarts, a per-tab history replaces the back list of
  sleeping tabs, favicons are cached per host.

## Building

Requirements:

- Rust 1.95 or newer
- Linux with a Vulkan driver (Wayland or X11)

```sh
cargo run --release -p atoll
```

The first build downloads the matching CEF distribution (about 1.8 GB extracted) into `.cef/`.
The build copies the CEF runtime files next to the binary in `target/<profile>/`.

### Configuration

| Variable | Effect |
|---|---|
| `ATOLL_DATA_DIR` | Use another data directory, e.g. for testing |
| `ATOLL_SOFTWARE_RENDERING` | Receive page frames as CPU buffers instead of shared GPU textures |
| `ATOLL_NO_SANDBOX` | Disable the Chromium sandbox |
| `ATOLL_CHROMIUM_FLAGS` | Additional Chromium switches, e.g. `remote-debugging-port=9222` |
| `RUST_LOG` | Log filter, e.g. `info,atoll=debug` |

Data lives in `~/.local/share/atoll/`: `atoll.db` holds workspaces, islands, tabs, favicons and
settings, `engine/` the Chromium profile (cookies, cache, local storage).

## Keyboard shortcuts

| Keys | Action |
|---|---|
| `Ctrl+T` | New tab next to the active one |
| `Ctrl+W`, `Ctrl+F4` | Close tab |
| `Ctrl+Shift+T` | Reopen closed tab |
| `Ctrl+L`, `Alt+D`, `F6` | Focus the address bar |
| `Ctrl+R`, `F5` | Reload, add `Shift` to bypass the cache |
| `Alt+Left`, `Alt+Right` | Back, forward |
| `Ctrl+Tab`, `Ctrl+PageDown` | Next tab, add `Shift` or use `PageUp` for the previous one |
| `Ctrl+F` | Find on page |
| `Ctrl+Plus`, `Ctrl+Minus`, `Ctrl+0` | Zoom |
| `F11` | Full screen |
| `F12`, `Ctrl+Shift+I` | Developer tools |

## URL list format

```text
--- Research ---
https://en.wikipedia.org/wiki/Web_browser
https://github.com/brave/adblock-rust

# comment lines are ignored
https://example.org/without-island
```

- `--- NAME ---` starts an island, the following addresses belong to it.
- Addresses before the first marker go to a fallback island ("Importiert" by default).
- Empty lines and lines starting with `#` are ignored. Addresses without a scheme get `https://`.
- Invalid lines are listed after the import instead of being dropped silently.
- Imported tabs start sleeping. Duplicates can be kept, skipped or merged (the existing tab moves
  into the imported island). Islands with the same name can be merged or created anew.

Export works per island, per workspace, for all workspaces or for a selection, with or without
island markers, optionally including archived islands, to the clipboard or a `.txt` file.

## Architecture

```text
crates/
  atoll-core       domain model and the tab lifecycle state machine
  atoll-storage    SQLite persistence (WAL), migrations, store traits
  atoll-transfer   URL list parser, exporter and import planner
  atoll-engine     CEF integration: off-screen browsers, rendering, input, clipboard bridge
  atoll            the application: Slint user interface and the controller
```

- **Rendering**: browsers render off-screen. On Linux each frame arrives as a DMA-BUF, is
  imported into Vulkan, converted to RGBA on the GPU and shown by Slint as a texture of the very
  same `wgpu` device, without a CPU copy. The CPU path is used as a fallback.
- **Event loop**: CEF runs with an external message pump driven by Slint timers.
- **Clipboard**: off-screen browsers never own keyboard focus on Wayland, so Chromium's clipboard
  is separate from the system clipboard. A hidden helper page moves text between both, using real
  `copy` and `paste` events so web applications keep working.
- **Storage**: structural changes are written to the database first and the in-memory session is
  reloaded from it. The database stays the single source of truth.

## Status

Phase 1 of the concept (shell, tab model, workspaces and islands, database, fast startup, URL list
import and export) is implemented. Next up are adblocking, the global quick search and archive
suggestions, then uploads from the clipboard and the memory limit, then 1Password.

Known limitations:

- Only Linux is tested. Windows builds lack the shared texture import and would use the CPU path.
- Permission prompts (camera, microphone, notifications, location) are not implemented yet and
  are denied.
- Dragging files from other applications into a page is not supported yet.
- Input method composition (for example for CJK input) is forwarded but not positioned.
