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
- **1Password**: on pages with a login form a key appears in the address bar. It, `Ctrl+Shift+L`
  and the command palette list the 1Password logins saved for the site and fill the chosen one
  into every frame of that site, including fields inside shadow roots, and a message says what
  was filled. A one-time password is copied to the clipboard. Logins of other sites can be
  searched and used after a confirmation. Atoll talks to the 1Password CLI (`op`, package
  `1password-cli`), which asks the 1Password app to unlock, so enable "Integrate with 1Password
  CLI" in the app's developer settings.
- **Video download**: on pages with a video a download button appears in the address bar. It,
  `Ctrl+Shift+S` and the command palette save the video that plays, else the largest one. Plain
  video files go through the download list like any file. Streams that pages put together from
  pieces (HLS, DASH, for example on Reddit or YouTube) are downloaded by `yt-dlp` (package
  `yt-dlp`, merging with `ffmpeg`) and show up in the same list with progress and cancel.
  Copy-protected videos cannot be saved.
- **Extensions** written in Luau: a folder with a `plugin.toml` and Luau files under
  `~/.local/share/Atoll/plugins/`. An extension reacts to events (pages, tabs, downloads), adds
  commands with their own shortcuts to the palette, opens tabs, keeps its own data and fetches
  from the sites it named. It only runs once the user allowed what its manifest asks for, and it
  is listed in the settings with its permissions, its log and switches to reload or remove it.
  Every extension runs in a sandboxed Luau state of its own with a memory limit and a time
  budget; one that keeps running too long is switched off. Waiting for the browser (`atoll.sleep`,
  `atoll.http`) pauses the script without blocking the window.
- **Installing extensions**: the settings take a zip archive or a git address next to the folder
  under `~/.local/share/Atoll/plugins/`. Both land in a staging folder first, and only a package
  with a readable `plugin.toml` moves to its place, named after the extension in the manifest.
  What came from git is fetched again with one button (`git pull --ff-only`), and an extension
  that then asks for more than before is put before the user again. The git ways need the `git`
  command in the search path.
- **Extensions in pages**: an extension puts styles and scripts into the sites of its manifest
  (`atoll.content.register`), by default into a world of its own that the page can neither see nor
  change, and before the page's own scripts run. It reads a page with `atoll.page.eval`, which
  answers with the value. Page scripts reach the extension through `window.atoll.send(channel,
  data)`, which answers with whatever the extension's `page.message` handler returns. All of this
  goes through Chromium's own protocol, so nothing depends on an open developer tools window.
- **Extension panels**: an extension can bring a page of its own along (`[[panel]]` in its
  manifest). Atoll shows it next to the page in an engine instance of its own, opened from the
  command palette or with `atoll.panels.show`. The panel talks to its extension through the same
  `window.atoll`, and only ever to the extension it belongs to.
- **Extension workers**: `atoll.worker.spawn(source, calls)` starts a Luau state on a thread of
  its own for work that would hold up the window. It shares nothing with the extension: jobs go in
  as values (`worker:call(name, value)`, which pauses the script but not the window) and answers
  come back as values. From inside a worker, `host.call(name, value)` asks the extension for
  something and waits for it, so a worker can reach the browser without the browser ever waiting
  for a worker. A worker that runs too long, or one the extension stops, is cut short.
- **Extension request rules**: with the `requests` permission an extension blocks or redirects
  requests (`atoll.requests.block`, `atoll.requests.redirect`) and hears about every hit through
  `request.caught`. The browser asks for every request on its network thread, where no script may
  run, so extensions hand in rules instead of a function. A redirect has to point at a site of the
  manifest. The ad blocker is asked first, the rules after it.
- **German and English**: the user interface follows the system language and can be switched in
  the settings without a restart. The strings in the source are German, `lang/<language>/LC_MESSAGES/atoll.po`
  holds the translations, and both sides read it: Slint bundles it into the user interface, the
  crate `atoll-i18n` compiles it into a catalog for the strings Atoll puts together in Rust.
  `scripts/translations.py report` says what is missing, `update` adds new strings to the files,
  and a test fails when a string has no translation.
- **Private tabs** (`Ctrl+Shift+N`) live in the same window as every other tab, marked with their
  own icon and a badge in the address bar, and are dragged out into a window of their own like any
  tab. They keep nothing: the database only holds the bare row that one is there, address and
  title stay in memory, no history, no favicon, no scroll position, no way to reopen them, and
  extensions never see them. Their pages run in a session without a cache folder, shared among the
  private tabs, so a sign-in reaches the next one and everything is gone when the last one closes.
  A page opened from a private tab stays private.
- **Tabs through Tor**: a private tab whose pages travel the Tor network, so a site does not see
  where the request comes from. Tor runs inside Atoll through Arti, the Rust implementation, with
  a small SOCKS bridge on the loopback address that only the engine talks to; nothing has to be
  installed. Starting Tor takes a moment, and the tab loads nothing before it is through, so no
  request of such a tab ever leaves unprotected. This is not the Tor Browser: it hides the
  address, not what a page can read about fonts, screen or canvas.
- **History** (`Ctrl+H`): the visited pages by day, with a search of its own, a way to forget a
  single page and one to throw the whole history away. The address bar and the palette search the
  same pages, the view is the place that shows all of them.
- **Site information**: the symbol in the address bar shows whether the connection is secure,
  the certificate and the zoom level, and deletes cookies and stored data of the site.
- **Developer tools** docked below or next to the page, see *Architecture*.
- **Settings** (`Ctrl+,`): start page and home button, search engine, sidebar and bookmarks bar
  (optionally hidden while empty), the number of frozen tabs, downloads and privacy.
- **Collapsible sidebar** (`Ctrl+B`). Hovering a tab shows its full title and address. Islands are
  dragged into another order, with a line showing where the island would land, and their menu
  moves them one place or all the way to the top or bottom.
- **Taking over from Opera** (menu or palette): Atoll reads Opera's profile and takes over its
  open tabs with their tab islands, its bookmarks and its history. An island Opera never named is
  named after the site most of its tabs belong to. Opera is only read, nothing in its profile
  changes, and its history is copied away first so a running Opera is left alone. The tabs land in
  the import window, where the workspace and the way duplicates are handled are chosen as for any
  other list. `ATOLL_OPERA_DIR` points at another profile.
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

### Installing on Arch and CachyOS

```sh
./install.sh            # build a release, package it for pacman and install it
./install.sh --package  # only build the package under target/package/
./install.sh --help     # every option, including --run and --uninstall
```

The package puts Atoll with its CEF runtime into `/opt/Atoll`, a starter into `/usr/bin/atoll`, and a
desktop entry with icons named `dev.dasdarki.Atoll`, the application id the window carries, so the
task bar shows the icon. The CEF files come straight from `.cef/`, which holds the build with
H.264 and AAC when `scripts/build-cef.sh` was used. Removing it leaves `~/.local/share/atoll` alone.

### CEF with H.264 and AAC

The prebuilt CEF cannot play H.264 and AAC, which many video sites use. `scripts/build-cef.sh`
builds the same CEF version with these codecs and installs it into `.cef/`, keeping the prebuilt
one for `scripts/build-cef.sh restore`. It needs about 100 GB in `~/.cache/atoll/cef-build`
(`ATOLL_CEF_BUILD_DIR`) and takes a few hours. Afterwards rebuild Atoll. The codecs are covered
by patents, so builds that contain them are for personal use and should not be distributed.

### Configuration

| Variable | Effect |
|---|---|
| `ATOLL_DATA_DIR` | Use another data directory, e.g. for testing |
| `ATOLL_SOFTWARE_RENDERING` | Receive page frames as CPU buffers instead of shared GPU textures |
| `ATOLL_NO_SANDBOX` | Disable the Chromium sandbox |
| `ATOLL_CHROMIUM_FLAGS` | Additional Chromium switches, e.g. `remote-debugging-port=9222` |
| `ATOLL_OP` | The 1Password CLI to use instead of `op` from the search path |
| `ATOLL_YTDLP` | The video downloader to use instead of `yt-dlp` from the search path |
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
| `Ctrl+Shift+L` | Log in with 1Password |
| `Ctrl+Shift+S` | Download the video of the page |
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
- Islands take their names from the markers, or are simply counted up from one, which also names
  the islands an import from Opera brings along.

Export works per island, per workspace, for all workspaces or for a selection, with or without
island markers, optionally including archived islands, to the clipboard or a `.txt` file.

## Architecture

```text
crates/
  atoll-core       domain model and the tab lifecycle state machine
  atoll-storage    SQLite persistence (WAL), migrations, store traits
  atoll-transfer   URL list parser, exporter and import planner
  atoll-adblock    ad blocking: filter lists, their updates and the per-site switch
  atoll-onepassword  logins from the 1Password CLI and matching them to sites
  atoll-video      video downloads through yt-dlp with progress and cancel
  atoll-i18n       the languages: the `t!` macro and the catalogs built from lang/
  atoll-tor        Tor through Arti, with the SOCKS bridge the engine speaks to
  atoll-plugin     the Luau runtime for extensions, their manifests, permissions and installation
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

All four phases of the concept are implemented: the shell, tab model, workspaces and islands,
database, fast startup, URL list import and export, the ad blocker, the global quick search, the
archive with its suggestions, uploads from the clipboard and the download folder, the download
history, the memory limit and 1Password. On top come a command palette, address bar
autocomplete, bookmarks, settings, docked developer tools and multiple windows.

Known limitations:

- Only Linux is tested. Windows builds lack the shared texture import and would use the CPU path.
- Permission prompts (camera, microphone, notifications, location) are not implemented yet and
  are denied.
- Dragging files from other applications into a page is not supported yet.
- Input method composition (for example for CJK input) is forwarded but not positioned.
- 1Password fills logins in but does not offer to save new ones.
- yt-dlp does not get Atoll's cookies, so streamed videos that need a login may fail to download.
- Extensions are not checked for a signature, and there is no directory to find them in.
- `window.atoll` is there for the scripts of a page as well, so an extension has to check the
  messages it gets just like any other input from the web.
- Passkeys and security keys are not supported. Chromium asks for them with a window of its own
  that an off-screen browser cannot show, so the Web Authentication API is hidden and requests are
  refused, which makes pages offer another way to sign in.
- The prebuilt CEF binaries contain no proprietary codecs, so H.264 and AAC media, which many
  sites such as Reddit serve, only play with a CEF built by `scripts/build-cef.sh`.
- The ad blocker applies no scriptlets and none of the procedural cosmetic rules that need
  script, such as `:has-text()`.
- Under Wayland, new windows are placed by the compositor, not at the point where a tab was
  dropped.
