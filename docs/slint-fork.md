# The Slint fork

Cachemire builds against a fork of Slint, pinned to a commit, and uses a few things Slint doesn't promise to keep stable. This is what the fork changes, what the app relies on, and how to move to a newer Slint.

- [What the fork is](#what-the-fork-is)
- [What the fork changes](#what-the-fork-changes)
- [What the app uses that upstream doesn't promise](#what-the-app-uses-that-upstream-doesnt-promise)
- [Moving to a newer Slint](#moving-to-a-newer-slint)

## What the fork is

[`iRed4321/SlintFork`](https://github.com/iRed4321/SlintFork) is upstream [`slint-ui/slint`](https://github.com/slint-ui/slint) plus a short series of patches. The app uses its `patched/1.19.0` branch: upstream `v1.19.0` plus 16 commits (`gh api repos/iRed4321/SlintFork/compare/v1.19.0...<rev>` lists them). `slint` and `slint-build` in [`crates/cachemire/Cargo.toml`](../crates/cachemire/Cargo.toml) both point at the commit `83de1d91f49ada0f7a17e999a48cbe88095f9efb`, the tip of that branch.

The fork keeps its own notes in a `FORK.md` at its root (branch layout, one row per patch, an upgrade script `scripts/fork-upgrade.sh`). This page is the app's side of it.

## What the fork changes

The 16 commits over `v1.19.0`, oldest first. The "why" is the one given in each commit message; the last column is only filled in where the app's code shows the link.

| Commit | Subject | What it changes | Used by |
|---|---|---|---|
| `7b92a8fd` | flickable: leave Ctrl+wheel to the ancestors | A `Flickable` no longer consumes Ctrl+wheel (reserved by applications, typically for zooming), so it reaches the element that wants it. | The window zoom (`zoom-in`/`zoom-out` in `ui/main.slint`) |
| `b302a538` | software renderer: hint glyph outlines | Hints glyph outlines to the pixel grid, since unhinted grayscale anti-aliasing leaves 10-13px text soft next to ClearType. | Small UI text on the software renderer (the Windows default) |
| `9c487a92` | styled-text: add StyledText::from_colored_paragraphs | Builds styled text from plain-text paragraphs and per-range colors, without markdown. | `key_detail/` colored values, via `src/styled_text.rs` |
| `6f1c2746` | styled-text: add an `overflow` property (clip \| elide) | `overflow: elide` on `StyledText`: drops the trailing lines that don't fit and puts an ellipsis on the last visible one. (The "one ellipsis per line" part of the 1.18 patch is upstream's own behavior since 1.19.) | The key's name in the header and the collapsed rows (`overflow: elide` on a `StyledText`) |
| `52bf5ee4` | text-input: add `clear-focus-on-outside-press` (default true) | A press outside a `TextInput` takes the focus away from it, moving it to the nearest enclosing `FocusScope`. The property set to `false` keeps the old behavior. | Inferred, no app commit records it: the window's `root-scope` `FocusScope` (`ui/main.slint`) holds the global shortcuts, and a click outside a text field now hands the focus back to it |
| `0830b20e` | window: fall back to the nearest visible ancestor when the focused item is culled | When the focused item is hidden, key events start from its nearest shown ancestor instead of being dropped. Upstream 1.19 keeps the focus on an item a `Flickable` only scrolled out of view, which was the main case; this remains for items really hidden. | Inferred: the toolkit-level counterpart of `refocus-tick` in `ui/main.slint` |
| `acb6e93e` | examples: add selection-demo | A standalone playground for the `StyledText` features. | Nothing (not built by the app) |
| `b300cbb5`, `5be9515f`, `f097d60f`, `0a0b7c16` | fork: document ... / fork: note ... / fork: notes for 1.19.0 | `FORK.md` and the upgrade script. | Nothing |
| `15880508` | highlight part of text | Adds a background style for a span (`Style::Background`) and `StyledText::from_highlighted_paragraphs`, with a test. | Search matches and the query highlighting (`key_detail/highlight`, `redisql/highlight`) |
| `1f7a269c` | software renderer: cull against the whole pixels of the dirty region | With a fractional scale factor, grows the logical dirty region to the whole physical pixels it is rounded out to, so an item reaching only into those is drawn again. (The other half of the 1.18 patch, clipping `Path`s to the dirty region, is upstream since 1.19, #14644.) | Partial repaints on the software renderer at 125%/150% scaling |
| `0dfadd1b` | styled-text: selection offsets, selected-text, clicked and drag callbacks | What the app needs on top of upstream's own `StyledText` selection (`selectable`, new in 1.19): `anchor-position-byte-offset` / `cursor-position-byte-offset` as properties, `selected-text`, `clicked`, `selection-drag-moved` / `selection-drag-ended`, the other mouse buttons left to ancestors, and a transparent `selection-foreground-color` keeping the text's own colors. It replaces the fork's whole selection patch of 1.18. | The value rows, the table cells and the key's name (`key-detail-row.slint`, `key-detail-table.slint`, `key-detail.slint`): selection across lines, drag auto-scroll, right-click menu, syntax colors kept under a selection |
| `47cda350` | styled-text: add `keep-mouse-cursor` | Upstream's selectable `StyledText` sets the text cursor whenever the pointer is over it. With `keep-mouse-cursor: true` it leaves the cursor to the `TouchArea` holding it (only a link changes it), and shows the text cursor only while the mouse selects. | The same three texts: the hand over a collapsed row, the key's name and a table cell, which are clicked as well as selected from |
| `83de1d91` | winit: no Adwaita window borders on Wayland | The winit backend's `wayland` feature no longer turns on `winit/wayland-csd-adwaita`, the window borders winit draws on Wayland (`sctk-adwaita`, its font rasterizer, a second `tiny-skia`): about 30 s of CPU off a clean Linux build. A framed window would get winit's plain frame. | The app's window has none (`no-frame: true` in `ui/main.slint`), so they were never drawn |

The "Used by" cells marked inferred come from reading the code, not from a commit that says so.

## What the app uses that upstream doesn't promise

**Private API.** `slint::private_unstable_api::re_exports::StyledText`, the type the patches above extend (`from_colored_paragraphs`, `from_highlighted_paragraphs`). It is named in one module only, [`crates/cachemire/src/styled_text.rs`](../crates/cachemire/src/styled_text.rs), and every other module imports it from there (`key_detail/{edit,value_lines,highlight,table}`, `redisql/highlight`). `rg -n private_unstable_api crates` must show that file alone. Its other uses are `.slint` `StyledText` elements and their properties, which the fork also extends.

**`unstable-winit-030`.** Public but unstable: `slint::winit_030` exposes winit 0.30 and the window hooks (`WinitWindowAccessor`, `on_winit_window_event`, `EventResult`, `with_winit_window`). Used in:

- `main.rs`: the single winit event hook (`install_window_events`), the Windows taskbar icon (`use_exe_icon`)
- `toast.rs`: window events for the toasts
- `drag_latch.rs` (Windows): clears winit's drag latch
- `cloud/mod.rs`: restores, focuses or flags the window (`UserAttentionType`)
- `window_state/mod.rs`: saving and restoring the window geometry

**Cargo features** of `slint` (`default-features = false`): `std`, `backend-winit`, `renderer-software`, `compat-1-2`, `unstable-winit-030`. The manifest does not say why each is enabled, so only what the code shows is stated: the app uses the winit backend with the software renderer (`select_backend` in `main.rs` makes it the Windows default, because "the OpenGL path presents unreliably there — stale buffers, stalls during DWM animations"), and `unstable-winit-030` is the feature behind the winit hooks above. `compat-1-2` is the compatibility marker Slint asks for (since 1.18) once default features are off (its manifest says "keep the mandatory `compat-1-2`"); it brings no behavior of its own here, apart from the pre-1.18 meaning of `backend-linuxkms`, which the app doesn't use. `slint-build` is used in `build.rs` with embedded files.

## Moving to a newer Slint

1. In the fork, put the patches on the new upstream tag (the fork's `scripts/fork-upgrade.sh v1.19.0 v1.20.0` does it, or `git rebase --onto v1.20.0 v1.19.0`), resolve the conflicts, then push a `patched/1.20.0` branch. Before forcing a patch through a conflict, check whether upstream now does the same thing: on 1.19 the selection patch shrank to a few additions and two others lost half of what they did. A patch that upstream now carries drops out as an empty commit. If upstream reshaped the code a patch touches, redo the patch on the new structure. The fork's `FORK.md` lists where things moved last time and which tests to run in the fork.
2. In [`crates/cachemire/Cargo.toml`](../crates/cachemire/Cargo.toml), move **both** `rev`s (`slint` and `slint-build`) to the new tip, and update the comment naming the branch.
3. `cargo update -p slint -p slint-build` (both are git dependencies, so this takes the new revision into `Cargo.lock`; add `--precise <rev>` if cargo picks another), then commit the lock file.
4. Check:
   - `src/styled_text.rs` still compiles: the private path and the constructors may have moved or changed
   - the winit hooks in the files above: `slint::winit_030` follows the winit version Slint bundles
   - `cargo test`: the headless UI tests (`ui_tests`) drive the real window on the software renderer
   - `cargo xtask e2e`
5. Renovate ignores `slint` and `slint-build` on purpose ([`.github/renovate.json`](../.github/renovate.json)), so it won't propose any of this: watch the upstream releases and security advisories by hand, and move to a fixed release without waiting for a feature.
