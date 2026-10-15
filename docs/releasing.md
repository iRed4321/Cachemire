# Releasing Cachemire

How versions work, how to ship a beta or a stable release, and how to build the packages locally.

- [How it works](#how-it-works)
- [The changelog](#the-changelog)
- [Shipping a beta](#shipping-a-beta)
- [Shipping a stable release](#shipping-a-stable-release)
- [Which version you get](#which-version-you-get)
- [What the Release workflow does](#what-the-release-workflow-does)
- [Command reference](#command-reference)
- [Building packages locally](#building-packages-locally)
- [Trying the installer](#trying-the-installer)
- [Troubleshooting](#troubleshooting)

## How it works

- **Versions only exist as tags** (`v3.0.0-beta.24`, `v3.0.0`). `crates/cachemire/Cargo.toml` always says `0.0.0-dev`: the release build writes the real version into its own checkout and never commits it.
- **A release is a request**: `cargo ship` pushes the commit you're on (HEAD) as a `release/<request>` branch, like `release/beta` or `release/fix-stable`. Pushing that branch starts the **Release** workflow.
- **The workflow does the rest**: it works out the version from the tags, runs the tests, builds the Windows and Linux packages as that version, and only if everything passed, tags the commit and publishes the GitHub release. It then deletes the request branch.
- **Betas are pre-releases** with automatic notes. **Stable releases** get hand-written notes from [`CHANGELOG.md`](../CHANGELOG.md).
- **The app updates itself from these releases**: the stable channel only sees stable releases, the beta channel sees both. On Windows the update installs in one click; on Linux the app opens the release page. The app looks when asked (the info window's button), or at each start once **Check for Updates at startup** is on in Settings.

```
cargo ship stable
   │  (stable only) names ## Unreleased → ## 3.0.0, commits CHANGELOG.md, pushes main
   ▼
push HEAD → origin/release/stable
   ▼
Release workflow: version 3.0.0 → tests → Windows build ┐
                                        → Linux build   ┴→ tag v3.0.0 + GitHub release → delete release/stable
```

## The changelog

[`CHANGELOG.md`](../CHANGELOG.md) at the repository root has one section per stable release, newest first, and an `## Unreleased` section on top for what the next stable release brings:

```markdown
# Changelog

## Unreleased

### Fixed
- The table view now shows "Value too long" for values over 128 KB

## 3.0.0 - 2026-10-20

The first public release of Cachemire! ...
```

- **Write what changed for the user**, as you go, under `## Unreleased`: features, fixes, changed behavior or limits. Internal work (refactors, CI, tooling) has no place there.
- **Never write a version heading yourself.** A stable `cargo ship` renames `## Unreleased` to `## <version> - <date>` and puts a new empty `## Unreleased` above it.
- **Betas don't touch it.** `## Unreleased` keeps filling up across the betas until the stable release.
- To preview a released version's notes as the workflow publishes them: `cargo xtask release-notes 3.0.0`.

## Shipping a beta

From the commit you want to release (usually the tip of `main`, pushed or not):

```console
$ cargo ship
Requesting the release of 3.0.0-beta.25 from a1b2c3d Show oversized values by size
Requested. The Release workflow tags and publishes v3.0.0-beta.25 once its tests and builds pass: https://github.com/iRed4321/Cachemire/actions
```

`cargo ship` is `cargo ship beta`: it raises the beta number. To start the betas of a new version after a stable one, name the part instead:

```console
$ cargo ship fix      # 3.0.0 → 3.0.1-beta.1
$ cargo ship minor    # 3.0.0 → 3.1.0-beta.1
$ cargo ship major    # 3.0.0 → 4.0.0-beta.1
```

Only HEAD is released: uncommitted changes aren't part of it, and `cargo ship` says so (`Note: the uncommitted changes are not part of the release, only HEAD is.`).

The beta is published as a pre-release, with GitHub's automatic notes (a compare link with the previous release).

## Shipping a stable release

1. **Check `## Unreleased`** in `CHANGELOG.md`: it's the release message. Edit it as you like; committing it isn't needed.
2. **Be on `main`, up to date** with `origin/main`.
3. **Run the request:**

   ```console
   $ cargo ship stable
   Release notes of 3.0.0, from CHANGELOG.md:

   The first public release of Cachemire! Versions 1 and 2 were internal tools, never published. ...

   ### Browse and read
   ...

   Release 3.0.0 with these notes, committing CHANGELOG.md and pushing main? [y/N] y
   Requesting the release of 3.0.0 from 9f8e7d6 Release 3.0.0
   Requested. The Release workflow tags and publishes v3.0.0 once its tests and builds pass: https://github.com/iRed4321/Cachemire/actions
   ```

   Once you answer `y`, it:
   - renames `## Unreleased` to `## 3.0.0 - <today>`, under a new empty `## Unreleased`;
   - commits `CHANGELOG.md` alone as `Release 3.0.0` (anything else staged stays staged);
   - pushes `main`, then the `release/stable` branch.

   Answering anything else calls it off: nothing is written, committed or pushed.

4. **Watch the run** in the Actions tab. The release appears with the changelog section as its notes, followed by the compare link.

`cargo ship stable` releases the current beta's version as stable (`3.0.0-beta.24` → `3.0.0`). To release a new version directly, with no beta:

```console
$ cargo ship fix --stable     # 3.0.0 → 3.0.1
$ cargo ship minor --stable   # 3.0.0 → 3.1.0
$ cargo ship major --stable   # 3.0.0 → 4.0.0
```

These go through the same changelog step. Add `--yes` to skip the question (`cargo ship stable --yes`).

## Which version you get

The version follows the **highest tag**, betas included.

| Latest tag | Command | Branch pushed | Version |
|---|---|---|---|
| `v3.0.0-beta.24` | `cargo ship` | `release/beta` | `3.0.0-beta.25` |
| `v3.0.0-beta.24` | `cargo ship stable` | `release/stable` | `3.0.0` |
| `v3.0.0` | `cargo ship fix` | `release/fix` | `3.0.1-beta.1` |
| `v3.0.0` | `cargo ship minor` | `release/minor` | `3.1.0-beta.1` |
| `v3.0.0` | `cargo ship major` | `release/major` | `4.0.0-beta.1` |
| `v3.0.0` | `cargo ship fix --stable` | `release/fix-stable` | `3.0.1` |
| `v3.0.0` | `cargo ship minor --stable` | `release/minor-stable` | `3.1.0` |
| `v3.0.0` | `cargo ship major --stable` | `release/major-stable` | `4.0.0` |

Requests that don't apply are refused before anything is pushed:

- after a beta, only `beta` and `stable` (`3.0.0-beta.24 is a beta: run beta for the next one or stable to release it`);
- after a stable release, only `fix`, `minor` and `major` (`3.0.0 is already stable: run major, minor or fix`);
- `--stable` only goes with `fix`, `minor` or `major`.

To see what a request would release without pushing anything: `cargo xtask next-version fix-stable` (the branch name without `release/`).

## What the Release workflow does

[`.github/workflows/release.yml`](../.github/workflows/release.yml), started by a push to `release/*`, or by hand from the Actions tab (**Release** → **Run workflow**, picking the part and the `stable` box like the command's options).

| Job | Runs on | Does |
|---|---|---|
| Tests | Ubuntu, Windows | `cargo test` |
| Windows | Windows | works out the version, builds the MSI and the portable zip |
| Linux | Ubuntu | works out the version, takes a stable release's notes from `CHANGELOG.md` (failing early if they're missing), builds the .deb and the AppImage |
| GitHub release | Ubuntu | checks both builds got the same version, tags the commit, creates the release with the 4 files: a pre-release for a beta, the changelog notes for a stable |
| Remove the release request | Ubuntu | when started by a push: deletes the `release/*` branch, whatever happened, so the same request can be pushed again |
| Summary | Ubuntu | a report of what was built and how long it took, on the run's page |

Only one release runs at a time. The tag is created only once everything passed, so a failed run leaves no tag and no release behind.

The files of a release:

```
Cachemire-3.0.0-x64.msi            Windows installer
Cachemire-Portable-3.0.0-x64.zip   Windows portable exe
cachemire_3.0.0-1_amd64.deb        Debian / Ubuntu package
Cachemire-3.0.0-x86_64.AppImage    Linux, any distribution
```

A stable release started from the Actions tab skips `cargo ship`'s changelog step: it fails with `CHANGELOG.md has no ## 3.0.0 section` unless the section was already named. Request stable releases with `cargo ship`.

## Command reference

`cargo ship` is an alias of `cargo xtask release` (see [`.cargo/config.toml`](../.cargo/config.toml)).

### `cargo ship [PART]`

| | |
|---|---|
| `PART` | `beta` (default), `stable`, `fix`, `minor` or `major` |
| `--stable` | with `fix`, `minor` or `major`: release the new version at once, with no beta |
| `--yes` | don't ask before a stable release commits `CHANGELOG.md` and pushes `main` |

### Release helpers

| Command | Prints |
|---|---|
| `cargo xtask next-version <request>` | the version a request releases: `beta`, `stable`, `fix`, `minor`, `major`, or `fix-stable`, `minor-stable`, `major-stable` |
| `cargo xtask release-notes <version>` | the `## <version>` section of `CHANGELOG.md`, as published |

## Building packages locally

The same builds the workflow runs, for trying a package before releasing it. Each one writes into `target/`.

| Command | Builds | Output | Needs |
|---|---|---|---|
| `cargo xtask build-windows` | exe, MSI, portable zip | `target/windows/` | Windows, the WiX CLI (installed as a .NET tool if missing, needs the .NET SDK) |
| `cargo xtask build-linux` | .deb and AppImage, from one build | `target/debian/`, `target/appimage/` | Linux, `cargo-deb` (installed if missing) |
| `cargo xtask build-deb` | .deb only | `target/debian/` | Linux, `cargo-deb` |
| `cargo xtask build-appimage` | AppImage only | `target/appimage/` | Linux; `appimagetool` is downloaded into `target/tools/` if missing |

**What gets built**, by default: the latest release tag, checked out for the build, then HEAD is put back. If HEAD is on a release tag, that one. The working tree must be clean.

| Option | Builds | Available on |
|---|---|---|
| *(none)* | the latest release (or the tag on HEAD) in the release profile | all four |
| `--stable` | the latest **stable** release instead | all four |
| `--as-version <VERSION>` | the working tree, in the release profile, as that version (what the workflow does) | all four |
| `--fast` | the working tree as is, in the `fast` profile (no LTO, quick link); the MSI, zip and AppImage go to `target/windows-fast/` and `target/appimage-fast/` | all four |
| `--with-zip` | also zips the package (MSI, .deb or AppImage) | all four |
| `--report` | builds with `cargo --timings` and saves the CI report | `build-windows`, `build-linux`, `build-appimage` |
| `--clean` | runs `cargo clean` first | `build-windows` |
| `--skip-installer` | skips the MSI: exe and portable zip only | `build-windows` |

Examples:

```console
$ cargo xtask build-linux --fast                      # try the packages of what you're working on
$ cargo xtask build-windows --as-version 3.1.0-beta.1 # the working tree, built as a given version
$ cargo xtask build-windows --stable --skip-installer # the latest stable exe and zip, no MSI
```

`--as-version`, `--fast` and `--stable` don't combine. `--as-version` writes the version into `crates/cachemire/Cargo.toml` and `Cargo.lock` for the build and puts them back afterwards, so those two files must have no uncommitted changes.

To install the app on your own machine instead: `cargo xtask install` (`cargo install` plus the desktop entry and icons on Linux, or a Start menu shortcut on Windows; `--skip-binary` on Linux installs only the desktop entry and icons).

## Trying the installer

The real MSI around a stand-in app that builds in seconds, to try installing, updating and removing without a real release:

```console
$ cargo xtask build-installer-test --version 1.2.0
```

It lands in `target/installer-test/`. Build two versions (say `1.2.0` then `1.3.0-beta.2`) to try an update. On GitHub: **Actions** → **Installer test** → **Run workflow** with a version, then download the artifact.

## Troubleshooting

**`If release/beta is already on origin, a release is under way, or one ended without removing it`**: a run is still going (wait for it), or one was interrupted before its cleanup. Remove the branch with `git push origin --delete release/beta`, then ship again.

**A stable release failed in the workflow** (a test, a build): fix the problem, commit, and run `cargo ship stable` again. The changelog already says `## 3.0.0`, so it's kept as is, with no second commit; the tag doesn't exist yet, so the version is the same. If you added lines to a new `## Unreleased` meanwhile, it stops and asks you to move them into `## 3.0.0` first.

**`a stable release is made from main`** / **`origin/main has commits HEAD doesn't`**: switch to `main` (`git switch main`) or pull (`git pull`) first.

**`CHANGELOG.md has nothing under ## Unreleased`**: write what the release changes there first.

**`no release tag to start from (are the tags fetched?)`**: `cargo ship` fetches the tags itself; for `next-version`, run `git fetch --tags` first.

**Local builds of older tags**: without `--as-version` or `--fast`, the build tasks check out the latest release tag. Tags made before the `crates/` and `resources/` layout can't be built by the current tasks; use `--as-version` or `--fast` until a release exists with the new layout.
