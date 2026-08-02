# Atoll

A desktop browser built around one idea: **a tab is just a record until you look at it.**

Atoll is a native Rust shell around Chromium (via the Chromium Embedded Framework). All
workspaces, islands and tabs live in a SQLite database. At startup only that database is read and
the sidebar is drawn; exactly one engine instance is created, for the active tab. Hundreds of
saved tabs therefore cost nothing at startup.

## Features

- **Workspaces, islands, tabs**: a workspace is a complete context, islands are named, colored,
  collapsible tab groups inside it.
- **Windows**: every window shows one workspace, and a workspace is shown in at most one window.
  Dragging a tab out of the sidebar and releasing it outside the window moves it into a new window
  with a workspace of its own, dropping it on another Atoll window moves it there. The page keeps
  running either way. Tabs and islands can also be moved through their context menus, and a
  workspace can be opened in a window of its own. Open windows are restored at the next start.
- **Own window frame**: the toolbar doubles as the title bar with the window buttons, a double
  click maximizes the window, and the edges resize it.
- **Four tab states**: *Live* (visible), *Frozen* (recently used, paused in the background),
  *Sleeping* (plain record, the normal state) and *Archived* (part of an archived island).
  Frozen tabs beyond the limit and tabs put to sleep release their engine instance.
- **Memory limit**: while the pages use more memory than the budget (4 GB by default), the least
  recently used background tabs go to sleep. Pinned tabs, tabs playing audible media and pages
  with input that was not sent yet stay loaded, also when the frozen limit is reached.
- **Archive**: islands can be archived and restored with one click. Islands unused for four
  weeks, configurable in the settings, are marked in the sidebar and suggested in the archive.
  Nothing is archived without confirmation.
- **Quick search and command palette**: `Ctrl+K` searches the tabs of all workspaces (archived
  ones included), bookmarks and history. `Ctrl+Shift+P` or a leading `>` lists commands instead.
- **Address bar autocomplete**: addresses are completed inline from history and bookmarks. The
  suggestions offer open tabs to switch to, bookmarks and visited pages, ranked by how often and
  how recently they were visited.
- **Bookmarks**: a bookmarks bar with folders, `Ctrl+D` to bookmark the current page, whole
  islands saved as folders, a bookmark manager and import from the HTML export of other browsers.
  Entries of the bar are rearranged and moved into folders by drag and drop.
- **Uploads**: when a page asks for a file, Atoll first offers the image in the clipboard, saved
  as a PNG, and the newest matching files from the download folder and the download history.
  The system dialog is one click away and opens directly if there is nothing to offer.
- **Downloads**: a download list with progress, pause, resume and cancel that is kept in the
  database. The download folder, asking for a location each time and opening the list when a
  download starts are configurable.
- **Ad blocker**: requests are checked against EasyList, EasyPrivacy and EasyList Germany with
  Brave's `adblock` engine, and ad elements are hidden with the lists' cosmetic rules, including
  elements pages add later. The lists are updated every four days. The blocker can be switched
  off per site in the site information or everywhere in the settings.
- **Site information**: the symbol in the address bar shows whether the connection is secure,
  the certificate and the zoom level, and deletes cookies and stored data of the site.
- **Developer tools** docked below or next to the page, see *Architecture*.
- **Settings** (`Ctrl+,`): start page and home button, search engine, sidebar and bookmarks bar
  (optionally hidden while empty), the number of frozen tabs, downloads and privacy.
- **Collapsible sidebar** (`Ctrl+B`). Hovering a tab shows its full title and address.
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
| `ATOLL_SYSTEM_KEYRING` | Keep the key for cookies in the system keyring (KWallet, GNOME Keyring or the Secret portal) instead of Chromium's own store. Chromium waits for the keyring before loading anything, so an unresponsive keyring service stops all pages from loading |
| `RUST_LOG` | Log filter, e.g. `info,atoll=debug` |

Data lives in `~/.local/share/atoll/`: `atoll.db` holds workspaces, islands, tabs, bookmarks,
history, downloads, favicons and settings, `engine/` the Chromium profile (cookies, cache, local
storage).

## Keyboard shortcuts

| Keys | Action |
|---|---|
| `Ctrl+N` | New window with a new workspace |
| `Ctrl+Shift+W` | Close window |
| `Ctrl+Shift+Q` | Quit, every open window is restored at the next start |
| `Ctrl+T` | New tab next to the active one |
| `Ctrl+W`, `Ctrl+F4` | Close tab |
| `Ctrl+Shift+T` | Reopen closed tab |
| `Ctrl+L`, `Alt+D`, `F6` | Focus the address bar |
| `Ctrl+R`, `F5` | Reload, add `Shift` to bypass the cache |
| `Alt+Left`, `Alt+Right` | Back, forward |
| `Ctrl+Tab`, `Ctrl+PageDown` | Next tab, add `Shift` or use `PageUp` for the previous one |
| `Ctrl+F` | Find on page |
| `Ctrl+K` | Search all tabs, bookmarks and history |
| `Ctrl+Shift+P` | Command palette |
| `Ctrl+B` | Show or hide the sidebar |
| `Ctrl+D` | Bookmark the current page |
| `Ctrl+Shift+B` | Show or hide the bookmarks bar |
| `Ctrl+Shift+O` | Manage bookmarks |
| `Ctrl+J` | Downloads |
| `Ctrl+,` | Settings |
| `Alt+Home` | Open the start page |
| `Ctrl+Plus`, `Ctrl+Minus`, `Ctrl+0` | Zoom |
| `F11` | Full screen |
| `F12`, `Ctrl+Shift+I` | Show or hide the developer tools |

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
  atoll-adblock    ad blocking: filter lists, their updates and the per-site switch
  atoll-engine     CEF integration: off-screen browsers, rendering, input, clipboard bridge
  atoll            the application: Slint user interface and the controller
```

- **Rendering**: browsers render off-screen. On Linux each frame arrives as a DMA-BUF, is
  imported into Vulkan, converted to RGBA on the GPU and shown by Slint as a texture of the very
  same `wgpu` device, without a CPU copy. The CPU path is used as a fallback.
- **Event loop**: CEF runs with an external message pump driven by Slint timers.
- **Windows**: the engine, the database and the engine instances of all tabs are shared, each
  window has its own controller. The tab lifecycle keeps one live tab per window, so moving a tab
  hands its engine instance over without reloading. Engine events are routed to the window whose
  workspace holds the tab. Wayland does not tell applications where the pointer is outside their
  windows, so a tab dropped outside its window moves to the Atoll window that reports the pointer
  entering it right after the drop.
- **Clipboard**: off-screen browsers never own keyboard focus on Wayland, so Chromium's clipboard
  is separate from the system clipboard. A hidden helper page moves text between both, using real
  `copy` and `paste` events so web applications keep working.
- **Developer tools**: Chromium only opens DevTools in native windows, which does not work for
  off-screen browsers under Wayland. Atoll loads the bundled DevTools frontend into an ordinary
  off-screen view instead and connects it through a WebSocket on the loopback interface, served
  in-process under a random path and only accepted from the frontend's origin. Every message is
  relayed to the protocol session of the inspected browser.
- **Ad blocking**: the engine asks a content filter about every request on Chromium's network
  thread and injects the page's cosmetic rules when a document starts loading. A small observer
  script reports the class names and ids appearing on the page, the matching generic rules are
  added as a stylesheet. The prepared filter engine is kept on disk and loads in a few
  milliseconds, so the first page is filtered as well.
- **Memory limit**: a background thread sums the proportional memory of the processes below
  Chromium's sandboxed zygote, which are the renderers, every 30 seconds. A small script on
  every page reports audible media and changed form fields through the console, the tab
  lifecycle keeps such busy tabs like pinned ones.
- **Storage**: structural changes are written to the database first and the in-memory session is
  reloaded from it. The database stays the single source of truth.

## Status

Phases 1 to 3 of the concept are implemented: the shell, tab model, workspaces and islands,
database, fast startup, URL list import and export, the ad blocker, the global quick search, the
archive with its suggestions, uploads from the clipboard and the download folder, the download
history and the memory limit. On top come a command palette, address bar autocomplete, bookmarks,
settings, docked developer tools and multiple windows. Next up is 1Password.

Known limitations:

- Only Linux is tested. Windows builds lack the shared texture import and would use the CPU path.
- Permission prompts (camera, microphone, notifications, location) are not implemented yet and
  are denied.
- Dragging files from other applications into a page is not supported yet.
- Input method composition (for example for CJK input) is forwarded but not positioned.
- The ad blocker applies no scriptlets and none of the procedural cosmetic rules that need
  script, such as `:has-text()`.
- Under Wayland, new windows are placed by the compositor, not at the point where a tab was
  dropped.
