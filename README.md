# stools

A minimal, Fuzzel-style application launcher built with [Slint](https://slint.dev/).

- **Fixed-size, borderless, centered** window (no dynamic re-layout).
- **Fuzzy matching** with pinyin support (`nucleo-matcher` + `pinyin`), including
  heteronym (multi-pronunciation) characters so e.g. `wyyyy` matches 网易云音乐.
- **Binary & executable search**: alongside `.desktop` apps, launcher searches
  common PATH directories (`~/.local/bin`, `~/.cargo/bin`, `/usr/bin`, …) and
  any extra directories passed on the command line. Binaries rank *below*
  desktop entries and display their (prettified) path as a grey subtitle.
- **MRU history**: recently launched apps float to the top of the list, and
  frequently-used items are boosted during fuzzy search. History is stored in
  `~/.local/share/stools/history.json`.
- **Linux**: single-shot stateless runner — launch via your window manager, pick
  an app, done. No tray, no built-in hotkey (your tiling WM binds a key).
  Indexes are cached to `~/.cache` for a near-instant cold start.
- **Windows**: resident tray daemon with a global hotkey (`Alt+Space`) to summon
  the window. `Esc` hides it; summoning again selects all the typed text.
- **macOS is not supported** by design.

## Build

```sh
cargo build --release
```

The binary is `target/release/stools`.

## Linux usage

Bind a key in your window manager to run the binary. Because the window is
borderless and fixed-size, you configure the window manager to float and center
it.

### Hyprland

```ini
windowrulev2 = float, class:(stools)
windowrulev2 = center, class:(stools)
windowrulev2 = noanim, class:(stools)
windowrulev2 = noborder, class:(stools)
```

Example keybinding:

```ini
bind = SUPER, SPACE, exec, stools
```

### Sway

```ini
for_window [app_id="stools"] floating enable
for_window [app_id="stools"] move position center
```

Example keybinding:

```
bindsym $mod+space exec stools
```

### Behaviour

| Key      | Action                                   |
|----------|------------------------------------------|
| `↑` / `↓` | Move selection                         |
| `Enter`  | Launch the selected app and exit         |
| `Esc`    | Exit immediately                         |
| typing   | Live fuzzy / pinyin filtering            |

Note: the first run scans `.desktop` files *and* executable directories, writing a
cache; subsequent runs load the cache instantly and refresh it in the background
(so new installs appear automatically).

### Searching binaries with custom paths

By default the launcher scans a built-in list of common locations (`~/.local/bin`,
`~/.cargo/bin`, `~/.deno/bin`, `~/.bun/bin`, `~/.zvm/bin`, `/usr/local/bin`,
`/usr/bin`, `/home/linuxbrew/.linuxbrew/bin`, `/opt/rocm/bin`, …). To scan extra
directories, add them to `path` in `config.toml`, or pass them as arguments —
`~` and `$VAR` are expanded either way:

```sh
stools "$HOME/.zvm/bin" "/home/linuxbrew/.linuxbrew/bin"
```

Binary results are shown *below* desktop apps, and each shows its path as a grey
subtitle. Paths are prettified for display (`/home/you/...` → `~`, `/opt/rocm` →
`$ROCM_HOME`); paths too long to fit scroll across the row when selected.

## Windows usage

The launcher runs in the background after start:

- **`Alt+A`** (or left-click the tray icon) summons and focuses the window — the
  hotkey is whatever `[keybindings]` maps to the `stools` action.
- **`Esc`** hides it.
- The tray menu has **Show** and **Quit**.
- Selecting an app launches it and hides the launcher.

### Behaviour notes

- Hotkey/tray require the Windows message loop, which Slint's event loop drives,
  so the daemon stays alive even while the launcher window is hidden.
- Start-menu `.lnk` files are scanned for app names; launching goes through the
  shell (`ShellExecute`) so `.exe`/`.lnk`/URL targets all work. Icon extraction
  from `.lnk` is not implemented yet (entries render text-only).
- The directories in `config.toml`'s `path` are scanned for executables the same
  way they are on Linux, and rank below Start Menu entries.

## Configuration

Everything (search paths, keybindings, theme) is configured in a single
`config.toml`, which is created on first run with a fully commented English
default file:

| Platform | Path                                                                    |
|----------|-------------------------------------------------------------------------|
| Linux    | `$XDG_CONFIG_HOME/stools/config.toml` (→ `~/.config/stools/config.toml`) |
| Windows  | `%APPDATA%\stools\config.toml`                                           |

Every entry is optional — deleting the file (or a single key) restores the
built-in Dracula defaults. A parse error is reported on `stderr` and the defaults
are used instead, so a broken config never prevents the launcher from starting.

```toml
path = ["$HOME/.cargo/bin", "~/.deno/bin"]   # extra scan dirs (~, $VAR, %VAR% expanded)

[keybindings]                 # no modifier
tab = "down"
esc = "close"                 # Linux: quit / Windows: hide
Return = "execute"
Up = "up"
Down = "down"

[keybindings.shift]
tab = "up"

[keybindings.alt]
a = "stools"                  # summon the window (Windows global hotkey)

[theme]                       # Fuzzel RRGGBBAA colours
background = "282a36dd"
text = "f8f8f2ff"
match = "8be9fdff"
selection-match = "8be9fdff"
selection = "44475add"
selection-text = "f8f8f2ff"
border = "bd93f9ff"
font = ["ComicShannsMono Nerd Font", "LXGW WenKai GB Screen"]
```

### Search paths

`path` entries are appended to the built-in binary directories and, on Linux, are
also scanned for `.desktop` files (Windows scans them for `.exe`/`.bat`/`.cmd`/
`.ps1`/`.lnk`). `~`, `$VAR`, `${VAR}` and `%VAR%` are expanded, missing
directories are ignored. Directories passed on the command line still work and
are added on top of the configured ones.

The Linux scan cache records which directories it was built from, so editing
`path` invalidates it and the next launch rescans instead of serving stale
results.

### Keybindings

Actions: `down`, `up`, `execute`, `close` (Linux: quit / Windows: hide) and
`stools` (summon; registered as a global hotkey on Windows).

The table name is the modifier combination — `[keybindings]` or
`[keybindings.none]` for no modifier, otherwise any combination of `ctrl`,
`alt`, `shift`, `super` (`win`/`meta`/`cmd`) in any order, joined by `_` or `+`
(quote the name when using `+`, e.g. `[keybindings."super+shift"]`).

Key names are case-insensitive and accept both the XKB spelling reported by
`wev` (`Escape`, `Return`, `Prior`, `Next`, `space`, …) and the usual aliases
(`esc`, `enter`, `pageup`, `pagedown`, …); single characters are literal keys.
On Windows, the binding mapped to `stools` becomes the global hotkey (default
**Alt+A**).

### Theme

Colours use Fuzzel's `RRGGBBAA` notation (a leading `#` and the `RGB`/`RGBA`/
`RRGGBB` forms are also accepted), so existing Fuzzel themes can be pasted in
as-is; invalid values keep the built-in colour.

`font` is a priority list: the first family installed on the system wins (checked
against the system font database), and glyphs missing from it — CJK in a Latin
mono font, say — are resolved through the system font fallback.

## Project layout

```
build.rs            # compiles the .slint UI
ui/launcher.slint   # Fuzzel-style UI definition
src/
  main.rs           # platform entry dispatch
  launcher.rs       # shared Slint model helpers
  core/
    config.rs       # config.toml location, template + path expansion
    keybind.rs      # modifier/key normalization + action dispatch
    theme.rs        # Fuzzel colour parsing + font family resolution
    matcher.rs      # nucleo + pinyin fuzzy matching (with MRU boost)
    model.rs        # AppEntry data model
    indexer.rs      # on-disk cache
    history.rs      # MRU history (JSON)
    path_utils.rs   # PATH/binary dir resolution + path prettifier
  platform/
    linux.rs        # single-shot Linux flow
    windows.rs      # Windows tray + global-hotkey daemon
```

## Debugging

Set `STOOLS_DEBUG=1` to print startup timing and live search statistics to
`stderr`. Each search outputs the rebuild time and how many entries matched, so
you can spot a slow cold start or an unexpected empty/over-populated result set.

```sh
STOOLS_DEBUG=1 stools
# [stools] load=89.4µs new=26.4ms model=37.1ms show=46.2ms apps=159   <- startup
# [stools] initial-n=30                                                <- seeded rows
# [stools] search-rebuild=48.2µs n=10                                  <- per keystroke
```

Field meanings:

- `load` — time to load the app list (cache read on warm start, full scan on cold start).
- `new` — Slint window / GPU backend initialization (fixed per-process cost).
- `model` / `show` — building the initial visible list and showing the window.
- `search-rebuild` — time to re-rank and rebuild the list for one keystroke, and
  how many matching entries (`n`) were produced. `n=0` means no match.

## Tests

```sh
cargo test
```

Covers pinyin field generation (incl. heteronyms), fuzzy ranking, config
parsing / path expansion, keybinding normalization and dispatch, and colour
parsing.
