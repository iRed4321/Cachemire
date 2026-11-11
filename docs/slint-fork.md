# The Slint fork

Cachemire builds against a fork of Slint, pinned to a commit, and uses a few things Slint doesn't promise to keep stable. This is what the fork changes, what the app relies on, and how to move to a newer Slint.

- [What the fork is](#what-the-fork-is)
- [What the fork changes](#what-the-fork-changes)
- [What the app uses that upstream doesn't promise](#what-the-app-uses-that-upstream-doesnt-promise)
- [Moving to a newer Slint](#moving-to-a-newer-slint)

## What the fork is

[`iRed4321/SlintFork`](https://github.com/iRed4321/SlintFork) is upstream [`slint-ui/slint`](https://github.com/slint-ui/slint) plus a short series of patches. The app uses its `patched/1.18.1` branch: upstream `v1.18.1` plus 14 commits (`gh api repos/iRed4321/SlintFork/compare/v1.18.1...<rev>` lists them). `slint` and `slint-build` in [`crates/cachemire/Cargo.toml`](../crates/cachemire/Cargo.toml) both point at the commit `8212a4db5a7d6acb8252f93955319e782cc4f733`, the tip of that branch.

The fork keeps its own notes in a `FORK.md` at its root (branch layout, one row per patch, an upgrade script `scripts/fork-upgrade.sh`). This page is the app's side of it.

## What the fork changes

The 14 commits over `v1.18.1`, oldest first as GitHub lists them. The "why" is the one given in each commit message; the last column is only filled in where the app's code shows the link.

| Commit | Subject | What it changes | Used by |
|---|---|---|---|
| `94c90ada` | flickable: leave Ctrl+wheel to the ancestors | A `Flickable` no longer consumes Ctrl+wheel (reserved by applications, typically for zooming), so it reaches the element that wants it. | The window zoom (`zoom-in`/`zoom-out` in `ui/main.slint`) |
| `bd18f360` | software renderer: hint glyph outlines | Hints glyph outlines to the pixel grid, since unhinted grayscale anti-aliasing leaves 10-13px text soft next to ClearType. | Small UI text on the software renderer (the Windows default) |
| `b28ef611` | winit software renderer: always present the whole buffer | Still renders only the damaged region, but presents the whole buffer: presenting only the damage assumes the surface still holds the previous frame, which the OS breaks on a DPI change, snap or un-minimize (pixels stayed transparent). | The Windows window (software renderer, see `select_backend` in `main.rs`) |
| `ed5936ff` | styled-text: add StyledText::from_colored_paragraphs | Builds styled text from plain-text paragraphs and per-range colors, without markdown. | `key_detail/` colored values, via `src/styled_text.rs` |
| `56377498` | highlight part of text | Adds a background style for a span (`Style::Background`) and `StyledText::from_highlighted_paragraphs`; touches `internal/common/styled_text.rs`, `internal/core/styled_text.rs` and the parley drawing/shaping code, with a test. The commit message says nothing more; the intent is inferred from the diff and from the fork's FORK.md ("a marker-pen background behind a span of text (search matches)"). | Search matches and the query highlighting (`key_detail/highlight`, `redisql/highlight`) |
| `1f09810b` | styled-text: read-only selection, copy, click and drag callbacks | `StyledText` can be selected with the mouse like a read-only `TextInput` (`selected-text`, Ctrl+C, primary selection), fires `clicked`, and has `selection-drag-moved` / `selection-drag-ended` so a host in a `Flickable` can auto-scroll; other mouse buttons are left to ancestors. | The value rows (`ui/components/key-detail-row.slint`: selection, drag auto-scroll, right-click menu) |
| `b71eca0e` | styled-text: add an `overflow` property (clip \| elide) | `overflow: elide` drops the trailing lines that don't fit and puts an ellipsis on the last visible one; only the last glyph run of a line gets it. | Not checked: `overflow: elide` is also valid on upstream `Text`, so the `ui` uses can't be told apart |
| `60a55c5f` | text-input: add `clear-focus-on-outside-press` (default true) | A press outside a `TextInput` takes the focus away from it, moving it to the nearest enclosing `FocusScope`. The property set to `false` keeps the old behavior. | Not established: the property is on by default, and no `.slint` file sets it |
| `64a85d8e` | window: fall back to the nearest visible ancestor when the focused item is culled | When the focused item is scrolled out of a virtualized list, key events start from its nearest visible ancestor instead of being dropped. | Not checked |
| `eae0722f` | examples: add selection-demo | A standalone playground for the `StyledText` features. | Nothing (not built by the app) |
| `c95b7418`, `95b8b7ca`, `443378dd` | fork: document ... / fork: note ... | `FORK.md` and the upgrade script; the two others record the upstream backport that 1.18.1 made unnecessary. | Nothing |
| `8212a4db` | fix render dirty region paths | Rewrites a part of `internal/renderers/software/lib.rs` (52 lines added, 40 removed). The commit message gives no reason, and the diff was not analyzed: the maintainer should fill in what it fixes. | The software renderer (assumed, to confirm) |

Still to fill in by the maintainer: why `8212a4db` exists, and which feature needs `64a85d8e` and `60a55c5f`.

## What the app uses that upstream doesn't promise

**Private API.** `slint::private_unstable_api::re_exports::StyledText`, the type the patches above extend (`from_colored_paragraphs`, `from_highlighted_paragraphs`). It is named in one module only, [`crates/cachemire/src/styled_text.rs`](../crates/cachemire/src/styled_text.rs), and every other module imports it from there (`key_detail/{edit,value_lines,highlight,table}`, `redisql/highlight`). `rg -n private_unstable_api crates` must show that file alone. Its other uses are `.slint` `StyledText` elements and their properties, which the fork also extends.

**`unstable-winit-030`.** Public but unstable: `slint::winit_030` exposes winit 0.30 and the window hooks (`WinitWindowAccessor`, `on_winit_window_event`, `EventResult`, `with_winit_window`). Used in:

- `main.rs`: the single winit event hook (`install_window_events`), the Windows taskbar icon (`use_exe_icon`)
- `toast.rs`: window events for the toasts
- `drag_latch.rs` (Windows): clears winit's drag latch
- `cloud/mod.rs`: restores, focuses or flags the window (`UserAttentionType`)
- `window_state/mod.rs`: saving and restoring the window geometry

**Cargo features** of `slint` (`default-features = false`): `std`, `backend-winit`, `renderer-software`, `compat-1-2`, `unstable-winit-030`. The manifest does not say why each is enabled, so only what the code shows is stated: the app uses the winit backend with the software renderer (`select_backend` in `main.rs` makes it the Windows default, because "the OpenGL path presents unreliably there — stale buffers, stalls during DWM animations"), and `unstable-winit-030` is the feature behind the winit hooks above. Why `compat-1-2` is needed is for the maintainer to fill in. `slint-build` is used in `build.rs` with embedded files.

## Moving to a newer Slint

1. In the fork, put the patches on the new upstream tag (the fork's `scripts/fork-upgrade.sh v1.18.1 v1.19.0` does it, or `git rebase --onto v1.19.0 v1.18.1`), resolve the conflicts, then push a `patched/1.19.0` branch. A patch that upstream now carries drops out as an empty commit. If upstream reshaped the code a patch touches, redo the patch on the new structure. The fork's `FORK.md` lists where things moved last time and which tests to run in the fork.
2. In [`crates/cachemire/Cargo.toml`](../crates/cachemire/Cargo.toml), move **both** `rev`s (`slint` and `slint-build`) to the new tip, and update the comment naming the branch.
3. `cargo update -p slint -p slint-build` (both are git dependencies, so this takes the new revision into `Cargo.lock`; add `--precise <rev>` if cargo picks another), then commit the lock file.
4. Check:
   - `src/styled_text.rs` still compiles: the private path and the constructors may have moved or changed
   - the winit hooks in the files above: `slint::winit_030` follows the winit version Slint bundles
   - `cargo test`: the headless UI tests (`ui_tests`) drive the real window on the software renderer
   - `cargo xtask e2e`
5. Renovate ignores `slint` and `slint-build` on purpose ([`.github/renovate.json`](../.github/renovate.json)), so it won't propose any of this: watch the upstream releases and security advisories by hand, and move to a fixed release without waiting for a feature.
