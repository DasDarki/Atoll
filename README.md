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
- **Moving tabs**: a tab is dragged to another place in the sidebar, inside its island or into
  another one, with a line showing where it lands. Dropped on the header of an island it goes to
  the top of it, which is the only way into one that is collapsed, and dropped outside the window
  it moves to the window under the pointer or into a new one.
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
- **Already open**: when the page in front of you is open in another tab, the address bar says
  so with a pill, and a click goes there. It counts the tabs of every workspace, names the
  workspace when the other tab lives in one of its own, and counts them when there are several;
  clicking then walks through them in the order they were opened. Two addresses that differ only
  after the `#` are the same page. Private tabs keep out of it, in both directions.
- **Search the pages of the workspace**: `Ctrl+Shift+F` or the palette opens a search over every
  tab of the workspace, the sleeping ones included. By default it looks at address and title, and
  a switch in the dialog looks inside the pages as well, with the matching line next to the hit.
  For that Atoll remembers what a page shows, read from the page itself: the rendered text, never
  the markup, and only when the page is finished - when the tab is left, five seconds after it
  loaded, and once more when Atoll closes, so an open page is remembered as it last stood. One row
  per tab, at most 64 KB, replaced when the page is left and thrown away with the tab, searched by
  SQLite's own index. Private tabs stay out. The settings decide whether anything is remembered at
  all, whether what was typed into fields counts (never a password field) and whether drafts in
  editors inside a page count, and they throw away what was remembered at any time.
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
  elements pages add later. The lists are updated every four days.
- **The shield** in the address bar, left of the lock: what this page is protected from, in two
  clicks, because a site that notices the blocker has to be dealt with right away. Five switches -
  protection at all, advertising, popups, being sent to another site unasked, tabs another site
  opens unasked - on one of three levels: everywhere, this site, or this address alone. The
  narrower level wins and only says what it was asked to say, so switching popups on for one shop
  leaves everything else as it was. Advertising and the switch above the others take effect with a
  reload, which the shield does itself; the rest counts for the next thing the page tries.
  Redirects and new tabs are stopped only where the page acts on its own: a link the user clicks
  is always followed, so signing in through another site keeps working.
- **Cookie banners**: two switches, both on from the start. Hiding takes the banners away that the
  EasyList Cookie List knows, with a list and an engine of its own, so it works whether or not
  advertising is blocked. Refusing presses the button that declines everything at the consent
  tools Atoll knows, from OneTrust and Cookiebot to Didomi and Usercentrics, so the site learns
  that the answer is no instead of never hearing one. A site the ad blocker is switched off for
  answers for itself.
- **1Password**: on pages with a login form a key appears in the address bar. It, `Ctrl+Shift+L`
  and the command palette list the 1Password logins saved for the site. A click fills the chosen
  one into every frame of that site, including fields inside shadow roots, and a message says
  what was filled; the buttons next to it copy name, password or one-time code on their own. A
  copied secret is taken back out of the clipboard after a minute and a half, as long as nothing
  else was copied meanwhile. When a site has exactly one saved login, Atoll fills it in by itself
  as the page opens, leaving anything already typed alone; that can be switched off in the
  settings. Logins of other sites can be searched and used after a confirmation. Nothing is ever
  written to 1Password.
  Atoll talks to the 1Password CLI (`op`, package `1password-cli`), which asks the 1Password app
  to unlock, so enable "Integrate with 1Password CLI" in the app's developer settings. The
  1Password SDKs are no alternative here: there is no official Rust one, and their service
  accounts cannot reach personal vaults at all.
- **Passkeys** are refused, with the reason said plainly. 1Password hands its passkeys to its own
  browser extension only, neither its CLI nor its SDKs can read them, and Chromium's own passkey
  window cannot be shown by a browser that renders its pages off screen.
- **New versions**: once a day Atoll asks GitHub which release is the newest, and says so when
  it is not the one that runs. It only looks: where a package manager put Atoll, that manager
  does the updating, and the message says so. The question goes the same way as everything else
  Atoll fetches, so a VPN stays in front of it, and the settings switch it off entirely.
- **Saving and converting**: `Ctrl+S` saves what the page shows as a file, which is what one
  wants on a page that is a picture, a sound or a video. `Ctrl+Shift+S` asks first: a dialog
  offers the formats that fit (PNG, JPEG, WebP, GIF, BMP, TIFF, AVIF for pictures; MP4, WebM,
  MKV, GIF for videos, or their sound alone as MP3, M4A, Opus, FLAC or WAV) and a switch that
  mirrors a picture or a video from left to right. The same dialog is what the video download
  uses, so a video can be saved as sound or mirrored on the way. Pictures are converted by Atoll
  itself; everything else needs `ffmpeg` installed and is left out of the list without it. The
  context menu of a picture offers "save" and "convert and save" with the same dialog.
- **Video download**: on pages with a video a download button appears in the address bar. It and
  the command palette save the video that plays, else the largest one, and `Ctrl+Shift+S` asks
  for the format first. Plain
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
- **Torrents**, in the download list like everything else that loads: a magnet address typed or
  clicked, or the address of a torrent file, and `librqbit` does the rest. A torrent only ever
  runs while a way out stands that hides where you are, and there are two. Through Atoll's own
  VPN the peers are reached through its bridge, which carries TCP alone: the distributed table
  and the trackers that speak UDP are switched off, because a client that used them would hand
  out your address behind the VPN's back, and Atoll asks the remaining trackers itself so even
  their names are looked up at the far end of the tunnel. That costs peers. Through a VPN of the
  system every socket of the torrent is tied to its device, which the kernel then enforces, and
  BitTorrent can do everything it knows. Giving pieces back to the swarm while a torrent loads is
  on - switching it off is offered, with a word about what the swarms do to those who take
  without giving. A torrent file that was downloaded is asked about, loaded at once or simply
  left lying there, whichever the settings say; asking is what Atoll does unless told otherwise.
  Where no VPN is set up at all, Atoll asks about that one torrent instead of refusing it: an
  installation image is nobody's business but your own, and the question says plainly that the
  swarm and the provider will see you. Nothing of that answer is remembered, so the next torrent
  asks again. A torrent that loses its way out stops where it is and goes on by itself once the
  way is back, because the files are already there.
- **Your VPN, with a killswitch**: Atoll runs no VPN service of its own. You write down the proxy
  of the service you already pay for - SOCKS5, or one that takes `CONNECT`, with TLS on the way to
  it if you like, user name and password included - or you hand it the WireGuard configuration the
  service wrote out, and Atoll builds that tunnel itself inside the browser, without root and
  without changing anything on the system: boringtun speaks the protocol, a network stack of its
  own lives in the tunnel, and names are looked up inside it, which is why such a configuration
  needs its DNS line. From then on everything the browser does
  takes that way: the pages, the private tabs, the downloads, the filter lists, what extensions
  fetch and what yt-dlp loads. A SOCKS server of Atoll's own sits on the loopback address in front
  of the service, because the engine only speaks to proxies, Chromium cannot hand a password to a
  SOCKS proxy, and one place every connection passes through is the only honest place for a
  killswitch. That killswitch is not a setting: while the way out is not known to be open, every
  connection is refused there, and nothing falls back to the ordinary line, because the fallback
  is the leak. WebRTC is held to the same way, so it cannot hand a site the real address past the
  proxy. Single sites can be kept out on purpose - the works intranet, say - through the shield,
  and they are listed in the settings so an exception cannot be forgotten.
- **Tabs through Tor**: a private tab whose pages travel the Tor network, so a site does not see
  where the request comes from. Tor runs inside Atoll through Arti, the Rust implementation, with
  a small SOCKS bridge on the loopback address that only the engine talks to; nothing has to be
  installed. Starting Tor takes a moment, and the tab loads nothing before it is through, so no
  request of such a tab ever leaves unprotected. This is not the Tor Browser: it hides the
  address, not what a page can read about fonts, screen or canvas.
- **Pages that are gone**: when nothing answers at an address, Atoll asks the Internet Archive
  whether it kept a copy and offers it, by default with a question, or opens it right away when
  the settings say so, or not at all. A second switch extends this to pages that answer that they
  are not there. The palette finds the copy of any page on request. Asking sends the address to
  archive.org, so it never happens in a private tab.
- **Searching the pages of a workspace** (`Ctrl+Shift+F`, or the palette): one field, and a switch
  for how deep it looks. Switched off it searches what a tab knows anyway, its address and its
  title, which costs nothing and answers while the user types. Switched on it searches what the
  pages show, with the sentence the words were found in next to each hit, and a hit jumps to that
  tab - a sleeping one as well, because what it showed was read before it went to sleep.
  What is read is the rendered text of a page and nothing else: no markup, no class names, and
  nothing that is hidden, because the page is asked for `innerText` in a world of its own. It is
  read when the page is left - at its first load a page that builds itself has nothing to show
  yet - five seconds after it finished loading, so a tab that is never left is searchable too,
  and once more when Atoll closes, so what a page became is what is remembered. What the user
  typed into a field counts as part of the page, so the tab it was typed into can be found again;
  a password field never does, and neither does a field that is not shown. One row per open tab, at most 64 KB, replaced when the page is left, thrown away with the
  tab and cleaned up at startup, so nothing piles up; SQLite's full text index does the searching,
  so a search costs the matches and not the pages, and nothing of it is held in memory. Private
  tabs are left out of all of it.
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

Atoll has a package repository of its own, signed, so `pacman` keeps it up to date like
everything else. The key is added once, then two lines go into `/etc/pacman.conf`:

```sh
curl -LO https://github.com/DasDarki/Atoll/releases/download/arch-repo/atoll-repo.gpg
sudo pacman-key --add atoll-repo.gpg
sudo pacman-key --lsign-key 'Atoll'
sudo tee -a /etc/pacman.conf >/dev/null <<'EOF'

[atoll]
Server = https://github.com/DasDarki/Atoll/releases/download/arch-repo
EOF
sudo pacman -Sy atoll
```

From then on `sudo pacman -Syu` brings every new version. The package puts the same tree in
place that `install.sh` builds. To build it yourself instead:

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
| `Ctrl+Shift+F` | Search the pages of this workspace |
| `Ctrl+K` | Search all tabs, bookmarks and history |
| `Ctrl+Shift+P` | Command palette |
| `Ctrl+B` | Show or hide the sidebar |
| `Ctrl+D` | Bookmark the current page |
| `Ctrl+Shift+B` | Show or hide the bookmarks bar |
| `Ctrl+Shift+O` | Manage bookmarks |
| `Ctrl+J` | Downloads |
| `Ctrl+Shift+L` | Log in with 1Password |
| `Ctrl+S` | Save what the page shows as a file |
| `Ctrl+Shift+S` | Save it after saying what should become of it |
| `Ctrl+,` | Settings |
| `Alt+Home` | Open the start page |
| `Ctrl+Plus`, `Ctrl+Minus`, `Ctrl+0` | Zoom |
| `F11` | Full screen |
| `F12`, `Ctrl+Shift+I` | Show or hide the developer tools |

## Releasing

A tag starts everything, the version is never written by hand anywhere else:

```sh
git tag v1.2.0 && git push origin v1.2.0
```

[`.github/workflows/release.yml`](.github/workflows/release.yml) then builds Atoll in an Arch
container, writes the version from the tag into the workspace, packs
`atoll-<version>-x86_64.tar.zst` with the binary, the CEF runtime, the starter, the desktop entry
and the icons, hangs it into a GitHub release together with its checksum, and pushes a fresh
`PKGBUILD` for `atoll-bin` to the AUR.

Two secrets steer the last two steps, and each step that misses its secret is skipped while the
rest runs:

- `REPO_GPG_KEY` signs the packages of the repository. A key of its own is made for it, without
  a passphrase, because the workbench has to sign unattended:

  ```sh
  gpg --batch --passphrase '' --quick-generate-key 'Atoll <thedasdarki@gmail.com>' rsa4096 sign never
  gpg --armor --export-secret-keys 'Atoll <thedasdarki@gmail.com>' | gh secret set REPO_GPG_KEY
  ```

  The packages, the database and the public key land in the release under the tag `arch-repo`,
  whose address never changes. [`packaging/repo/PKGBUILD.in`](packaging/repo/PKGBUILD.in) is the
  template for it.
- `AUR_SSH_KEY` is the private half of an SSH key whose public half lies in an AUR account under
  "My Account". With it, every tag also updates `atoll-bin` in the AUR;
  [`packaging/aur/PKGBUILD.in`](packaging/aur/PKGBUILD.in) is that template.

`scripts/package.sh` builds the archive everything else is made of, also by hand:

```sh
scripts/package.sh --version 1.2.0
```

The release carries the free codecs of CEF. H.264 and AAC need a CEF of one's own, see below.

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
  atoll-adblock    ad blocking: filter lists, their updates and the rules of the shield
  atoll-consent    what Atoll knows about consent tools and the script that refuses them
  atoll-onepassword  logins from the 1Password CLI and matching them to sites
  atoll-video      video downloads through yt-dlp with progress and cancel
  atoll-i18n       the languages: the `t!` macro and the catalogs built from lang/
  atoll-socks      the SOCKS server the engine talks to, shared by Tor and the VPN
  atoll-tor        Tor through Arti, carried by that server
  atoll-vpn        the user's VPN: their proxy or a WireGuard tunnel, and the killswitch
  atoll-net        the one way out for what Atoll fetches itself, so nothing goes past the VPN
  atoll-torrent    torrents through librqbit, tied to the way out and to the network devices
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
- A window a page opens without a click is stopped by the engine itself, before the shield is
  asked, so such a popup is never reported as blocked.
- Cookie banners are only refused where a consent tool Atoll knows is in use and a refusing button
  exists. A site that only offers agreeing or paying is left to the user.
- A tab through Tor keeps going through Tor while a VPN carries the rest: Arti builds its own way
  out. The 1Password CLI also talks to its service by itself.
- The tunnel carries TCP over IPv4. That is what a browser needs, and what cannot travel it - the
  QUIC of a page, say - falls back to TCP by itself. Where the server is has to be looked up
  before the tunnel stands, which is the one lookup no WireGuard client can hide.
- The user name and the password of the proxy are kept in the profile as they are, as with any
  other proxy setting of a browser.
- Under Wayland, new windows are placed by the compositor, not at the point where a tab was
  dropped.
