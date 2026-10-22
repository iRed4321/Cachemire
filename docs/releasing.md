# Releasing Cachemire

How the branches work, how to ship a beta, a stable release or an urgent fix, how the dependency updates release themselves, how to try the packages before releasing them, and how to build them locally.

- [How it works](#how-it-works)
- [Setting up](#setting-up)
- [Features and betas](#features-and-betas)
- [Releasing as stable](#releasing-as-stable)
- [An urgent fix](#an-urgent-fix)
- [Trying the packages](#trying-the-packages)
- [Dependency updates](#dependency-updates)
- [Bringing main into next](#bringing-main-into-next)
- [The changelog](#the-changelog)
- [Which version you get](#which-version-you-get)
- [The workflows](#the-workflows)
- [Command reference](#command-reference)
- [Building packages locally](#building-packages-locally)
- [Trying the installer](#trying-the-installer)
- [Troubleshooting](#troubleshooting)

## How it works

Two lines of work, so a fix of the stable release never waits for the features in beta:

| Branch | Holds | Changes through |
|---|---|---|
| `main` | the stable line: what's released as stable, and the fixes about to be | pull requests only, opened and merged by `cargo ship` once their checks pass |
| `next` | what the next minor or major brings; betas are released from it | your commits, pushed as usual |
| `fix/<name>` | one fix of the stable release, from `main` | `cargo ship start-fix`, then your commits; its pull request into `main` |
| `stable/<version>` | `next` as it's released as stable | `cargo ship stable`; its pull request into `main` |
| `release/<request>` | a request for a beta | `cargo ship`; the Release workflow deletes it |
| `renovate/*` | a dependency update | Renovate; its pull request into `main` (or `next` for a major), merged once its checks pass |
| `fix/deps-<version>` | the updates Renovate merged into `main`, released as a fix | the Dependencies workflow; its pull request into `main` |

```
main  ──●────────────────●(3.1.1)───────────────────────●(3.2.0)
         \               ↑ fix/tls (squashed)           ↑ stable/3.2.0 (merged)
next      ●──β1──●──β2───┴─merge──●──β3──●──"Release 3.2.0"──→ fast-forwarded to main
```

- **Versions only exist as tags** (`v3.2.0-beta.3`, `v3.1.1`). `crates/cachemire/Cargo.toml` always says `0.0.0-dev`: the release build writes the real version into its own checkout and never commits it.
- **`cargo ship` does the git and GitHub work**: it makes the branches, pushes them, opens the pull requests, labels them and turns their auto-merge on, through git and the [GitHub CLI](https://cli.github.com) (`gh`, signed in with `gh auth login`). There's nothing to do on github.com.
- **The Release workflow publishes**: it works out the version from the tags, runs the tests, builds the Windows and Linux packages as that version, and only if everything passed, tags the commit and publishes the GitHub release. A beta starts it by a pushed `release/<request>` branch; a fix or a stable release by its pull request being merged into `main` with the `release` label.
- **Betas are pre-releases** with automatic notes. **Stable releases and fixes** get hand-written notes from [`CHANGELOG.md`](../CHANGELOG.md).
- **The app updates itself from these releases**: the stable channel only sees stable releases, the beta channel sees both. On Windows the update installs in one click; on Linux the app opens the release page. The app looks when asked (the info window's button), or at each start once **Check for Updates at startup** is on in Settings.

## Setting up

Once for the repository, from an up-to-date clone:

```console
$ cargo ship setup
Auto-merge is on, and merged branches are deleted.
The `release` label is there.
Created next on GitHub, from main.
Created next here, following GitHub's.
main only changes through pull requests whose checks pass.
next can't be deleted or have its history rewritten.
Dependabot alerts are on: Renovate reads them to fix the advisories of indirect dependencies.

The Dependencies workflow needs a token of yours, as the RENOVATE_TOKEN secret, ...
Switched to next: work goes there from now on.
```

It sets up, on GitHub:

- the repository's **auto-merge**, and the **deletion of merged branches** (`fix/*`, `stable/*`);
- the **`release` label**, which tells the Release workflow to publish a merged pull request;
- the **`next` branch**, from `main`;
- the **rules of `main`** (a ruleset named `cargo ship: main`): changed by pull requests only, which must pass the CI's `Test (ubuntu-latest)` and `Test (windows-latest)` checks and be up to date with `main`; no force-push, no deletion;
- the **rules of `next`** (`cargo ship: next`): no force-push, no deletion;
- **Dependabot alerts**, which Renovate reads to fix the advisories of indirect dependencies (its own OSV check only covers direct ones).

It also checks the **`RENOVATE_TOKEN` secret** the [Dependencies workflow](#dependency-updates) works with, and says how to add it if it's missing: a [fine-grained token](https://github.com/settings/personal-access-tokens/new) of yours for this repository alone, with the permissions *Contents*, *Pull requests*, *Issues*, *Workflows* and *Commit statuses* (read and write), *Actions* and *Dependabot alerts* (read), then `gh secret set RENOVATE_TOKEN`, which asks for it. The workflow can't do with its own token: GitHub starts no checks on the pull requests and pushes it makes, so they'd never be merged, nor released.

Running it again changes nothing that's already set. On another clone, `git switch next` is all it takes.

## Features and betas

Work on `next`: commit and push as usual, the CI runs at each push. To release a beta, from `next`:

```console
$ cargo ship
Requesting the release of 3.2.0-beta.3 from a1b2c3d Show the TTL of each key
Requested. The Release workflow tags and publishes v3.2.0-beta.3 once its tests and builds pass: https://github.com/iRed4321/Cachemire/actions/workflows/release.yml
```

`cargo ship` is `cargo ship beta`. Before releasing, it:

- brings `next` up to date with GitHub's, and takes in what `main` has that it lacks: a fix released meanwhile, say (see [Bringing main into next](#bringing-main-into-next));
- pushes `next`, then the `release/beta` request.

After a stable release, the first beta is that of the next minor (`3.2.0` → `3.3.0-beta.1`); `cargo ship beta --major` starts those of the next major (`4.0.0-beta.1`) instead.

Only HEAD is released: uncommitted changes aren't part of it, and `cargo ship` says so (`Note: the uncommitted changes are not part of it, only HEAD is.`). A beta waits for a stable release under way to be published; a fix under way doesn't hold it up.

## Releasing as stable

1. **Check `## Unreleased`** in `CHANGELOG.md` on `next`: it's the release message. Edit it as you like; committing it isn't needed.
2. **From `next`, run:**

   ```console
   $ cargo ship stable
   Release notes of 3.2.0, from CHANGELOG.md:

   ### Added
   ...

   Release 3.2.0 with these notes, committing CHANGELOG.md on next and merging it into main once its checks pass? [y/N] y
   https://github.com/iRed4321/Cachemire/pull/14 merges next into main once its checks pass; the Release workflow then publishes v3.2.0: https://github.com/iRed4321/Cachemire/actions/workflows/release.yml
   ```

   Once you answer `y`, it:
   - renames `## Unreleased` to `## 3.2.0 - <today>`, under a new empty `## Unreleased`, and commits `CHANGELOG.md` alone on `next` as `Release 3.2.0` (anything else staged stays staged);
   - pushes `next`, and the same commit as `stable/3.2.0`;
   - opens the pull request of `stable/3.2.0` into `main`, with the notes as its description, labels it `release` and turns its auto-merge on (a merge commit, so the commits released as betas are those of `main`).

   Answering anything else calls it off: nothing is written, committed or pushed.

3. **That's all.** GitHub merges the pull request once its checks pass, and the Release workflow publishes `v3.2.0` with the changelog section as its notes. `next` is free meanwhile: what you commit there goes into the next version, as `stable/3.2.0` doesn't move. The next `cargo ship` brings the merge into `next` (see [Bringing main into next](#bringing-main-into-next)).

`cargo ship stable` releases the betas' version as stable (`3.2.0-beta.3` → `3.2.0`). With no betas since the last stable release, name the new version: `cargo ship stable --minor` (`3.1.1` → `3.2.0`) or `--major` (`4.0.0`). Add `--yes` to skip the question.

## An urgent fix

A fix of the stable release, while `next` is full of features in beta:

1. **Start it**, from anywhere:

   ```console
   $ cargo ship start-fix tls
   Stashed the uncommitted changes of next: `cargo ship fix` brings them back.
   On fix/tls, from main (v3.1.0), with the draft pull request https://github.com/iRed4321/Cachemire/pull/12.
   Commit and push the fix: the checks run at each push. `cargo ship try` builds its packages, `cargo ship fix` releases it.
   ```

   `fix/tls` starts from `origin/main`: the stable release, without the features of `next`. It is pushed with a first empty commit (GitHub opens no pull request on a branch without a commit of its own; the squash merge drops it), and its pull request into `main` is opened as a draft, so it can't be merged yet.

2. **Fix it**: commit the fix and its line under `## Unreleased` in `CHANGELOG.md`, and push. The checks run on the pull request at each push. To install the packages before releasing them: `cargo ship try` (see [Trying the packages](#trying-the-packages)).

3. **Release it**, from `fix/tls`:

   ```console
   $ cargo ship fix
   Release notes of 3.1.1, from CHANGELOG.md:

   ### Fixes

   - **Security**: ...

   Release 3.1.1 with these notes, committing CHANGELOG.md and merging the fix into main once its checks pass? [y/N] y
   https://github.com/iRed4321/Cachemire/pull/12 merges the fix into main once its checks pass; the Release workflow then publishes v3.1.1: https://github.com/iRed4321/Cachemire/actions/workflows/release.yml
   Once it's out, next takes it in at the next `cargo ship` (or `cargo ship sync`): a beta brings it to the beta channel.
   Back on next.
   Its uncommitted changes are back.
   ```

   It:
   - rebases `fix/tls` on `origin/main` if `main` has moved on (the branch was never released, so rewriting it is safe);
   - names the changelog section `## 3.1.1 - <today>` and commits it as `Release 3.1.1`. The version is the latest **stable** one with its patch raised, whatever betas are under way;
   - pushes the branch, updates the pull request's title and description with the notes, marks it ready, labels it `release` and turns its auto-merge on (squashed: one commit on `main` per fix);
   - goes back to the branch you started from, with its stashed changes.

4. **GitHub does the rest**: it squashes the fix into `main` once the checks pass, and the Release workflow publishes `v3.1.1` as a stable release.

5. **Take it into `next`**: the next `cargo ship` (or `cargo ship sync`) merges `main` into `next`. Beta users only get the fix with the next beta: `3.1.1` sorts below `3.2.0-beta.3`, so their app doesn't offer it.

If a check fails once `cargo ship fix` ran: `git switch fix/tls`, commit the correction and push. The auto-merge is still on, so the pull request merges as soon as the checks pass. Running `cargo ship fix` again does no harm: the changelog is already named, so it only pushes and updates the pull request.

To give a fix up: close its pull request on GitHub, then `git switch next` and `git branch -D fix/tls` (the stash, if any, is in `git stash list`).

## Trying the packages

To install a build before releasing it, from `next` or a `fix/<name>` branch:

```console
$ cargo ship try
Building fix/tls at d4e5f6a Check the certificate chain: https://github.com/iRed4321/Cachemire/actions/runs/123456
...
The packages, to install and try:
  target/try/package-linux/appimage/Cachemire-3.1.1-try.57-x86_64.AppImage
  target/try/package-linux/debian/cachemire_3.1.1.try.57-1_amd64.deb
  target/try/package-windows/Cachemire-3.1.1-try.57-x64.msi
  target/try/package-windows/Cachemire-Portable-3.1.1-try.57-x64.zip
```

It pushes the branch, starts the Release workflow on it in its "try" mode (the same Windows and Linux builds, no tests, no tag, no release), follows the run, and downloads the packages into `target/try/`, replacing the previous ones. It takes as long as a release's builds, about 15 minutes.

**The version of a try build** sits just below what the branch leads to, with the workflow's run number, so the real release is offered over it as an update:

| Branch | Leads to | Try build |
|---|---|---|
| `fix/<name>` (or `main`) | the next fix, `3.1.1` | `3.1.1-try.57` |
| `next`, betas under way | the next beta, `3.2.0-beta.4` | `3.2.0-beta.3.try.57`, between beta 3 and beta 4 |
| `next`, no betas since `3.1.0` | the first beta, `3.2.0-beta.1` | `3.2.0-beta.0.try.57` |

A try build counts as a beta for the app's updates.

## Dependency updates

[Renovate](https://docs.renovatebot.com) updates the dependencies, and the [Dependencies workflow](../.github/workflows/dependencies.yml) releases them as fixes, with nothing to do by hand:

| Updates | When | Into | Released |
|---|---|---|---|
| fixing a security advisory (RustSec, through OSV and Dependabot alerts) | the next night | `main` | as a fix, that night |
| minor and patch, in one pull request, and the refresh of `Cargo.lock` | Monday night, once a version is 3 days old | `main` | as a fix, Monday |
| major | Monday night | `next` | with the next beta |

The Slint fork is left alone: it's pinned to a commit on purpose. The config is [`.github/renovate.json`](../.github/renovate.json).

**What happens, each night at 2am (Paris time):**

1. **Renovate** opens its pull requests (`renovate/*`), with auto-merge on: GitHub merges each one once its checks pass, and Renovate rebases the others, as `main` needs them up to date. Its *Dependency Dashboard* issue lists what's pending; ticking a box there makes it act at its next run.
2. **After each merge into `main`**, the workflow runs Renovate again, to rebase the others at once, then `cargo ship deps-release`. Once none of Renovate's pull requests is on its way into `main` any more (one with a failed check doesn't count), and no other release is under way, it:
   - compares `Cargo.lock` with that of the latest stable release, and stops if the app's dependencies are the same (updates of the workflows' actions alone wait for the next release);
   - writes the changelog lines under `### Fixes` of `## Unreleased`: a **Security** one naming the advisories fixed (`cargo audit` on both lock files), a **Dependencies** one for the rest;
   - names the section after the next fix (`3.1.1`), and opens the `fix/deps-3.1.1` pull request, labeled `release`, auto-merge on. If one is already open, it's made again on `main` as it is now.

   GitHub merges it once its checks pass, and the Release workflow publishes `v3.1.1`.
3. **After each fix or stable release published**, the workflow brings `main` into `next` (`cargo ship sync`), and pushes it.

```markdown
### Fixes

- **Dependencies**: the libraries it's built on are updated (serde, tokio, zip and 14 more)
- **Security**: tokio updated, fixing RUSTSEC-2026-0384
```

**By hand**, without waiting for 2am: **Actions** → **Dependencies** → **Run workflow**, or `gh workflow run dependencies.yml`. Check **everything** (`-f everything=true`) to have the weekly updates too, whatever the day.

If Renovate's pull request fails its checks (an update breaks the build or a test), it stays open, and holds nothing up: correct it on its branch, or close it to skip that version.

## Bringing main into next

`main` gets what `next` doesn't have when a fix or a stable release is merged into it. The Dependencies workflow brings it into `next` once it's published, and `cargo ship`, `cargo ship stable` and `cargo ship sync` do too, first bringing in GitHub's `next` (fast-forwarded, or merged if you committed here meanwhile):

- after a stable release, `next` is part of `main`'s history: it's **fast-forwarded** to it, or, if you committed on `next` since, merged with it without any conflict;
- after a fix, `main` is **merged** into `next` (a `Merge main into next` commit). `next` isn't rebased: its commits are already released as betas, and rewriting them would take its beta tags off the branch;
- a conflict in **`CHANGELOG.md`** (both sides wrote under `## Unreleased`) is resolved by itself: `next`'s `## Unreleased`, with the lines `main` added to it, then every released section as `main` has it, newest first;
- a conflict in **`Cargo.lock`** too: `main`'s, completed by `cargo update --workspace` with what `next`'s manifests need on top;
- a conflict anywhere else stops it, with the merge under way: resolve it, `git add` the files and `git commit`, then run the command again.

The merge needs a clean working tree: commit or stash your changes first. `cargo ship sync` alone does the same, pushes `next`, and deletes the local `fix/*` branches whose pull request is merged.

## The changelog

[`CHANGELOG.md`](../CHANGELOG.md) at the repository root has one section per stable release, newest first, and an `## Unreleased` section on top for what the next stable release brings:

```markdown
# Changelog

## Unreleased

### Added
- **Key tree**: the TTL of each key shows next to its name

## 3.1.1 - 2026-10-23

### Fixes
- **Security**: ...
```

- **Write what changed for the user**, as you go, under `## Unreleased` of the branch you're on: features and their fixes on `next`, an urgent fix on its `fix/<name>` branch. Internal work (refactors, CI, tooling) has no place there.
- **Never write a version heading yourself.** `cargo ship stable` and `cargo ship fix` rename `## Unreleased` to `## <version> - <date>` and put a new empty `## Unreleased` above it.
- **Betas don't touch it.** `## Unreleased` of `next` keeps filling up across the betas until the stable release. A fix released meanwhile gets its own section, which `next` takes in from `main`.
- To preview a released version's notes as the workflow publishes them: `cargo xtask release-notes 3.1.1`.

## Which version you get

The version follows the **tags**, betas included. Betas are those of one version at a time.

| Latest tags | Command | Version |
|---|---|---|
| `v3.1.0` | `cargo ship` (on `next`) | `3.2.0-beta.1` |
| `v3.1.0` | `cargo ship beta --major` | `4.0.0-beta.1` |
| `v3.2.0-beta.3` | `cargo ship` | `3.2.0-beta.4` |
| `v3.2.0-beta.3` | `cargo ship stable` | `3.2.0` |
| `v3.1.0` | `cargo ship stable --minor` | `3.2.0` |
| `v3.1.0` | `cargo ship stable --major` | `4.0.0` |
| `v3.1.0`, `v3.2.0-beta.3` | `cargo ship fix` (on `fix/<name>`) | `3.1.1` |
| `v3.1.1`, `v3.2.0-beta.3` | `cargo ship fix` | `3.1.2` |

Requests that don't apply are refused before anything is pushed:

- while betas are under way, `beta --minor`/`--major` and `stable --minor`/`--major` (``the betas of 3.2.0 are under way (3.2.0-beta.3): `beta` releases the next one, `stable` releases them``);
- with no betas since the last stable release, `stable` alone (`3.1.0 is already stable, with no betas since: name the new version with --minor or --major`);
- one release at a time: a pull request waiting to be released, or one being published, holds up the others (`fix/tls is still being released (…): wait until it's merged and published, or close it`), except that a fix doesn't hold up a beta.

To see what a request would release without pushing anything: `cargo xtask next-version <request>` (`beta`, `beta-minor`, `beta-major`, `stable`, `stable-minor`, `stable-major` or `fix`).

## The workflows

### CI

[`.github/workflows/ci.yml`](../.github/workflows/ci.yml): on every pull request, and every push to `main` and `next`. Formatting, Clippy, the tests on Ubuntu and Windows with their coverage, and a summary. Its `Test (ubuntu-latest)` and `Test (windows-latest)` jobs are the checks `main`'s rules require.

### Dependencies

[`.github/workflows/dependencies.yml`](../.github/workflows/dependencies.yml), with the `RENOVATE_TOKEN` secret (see [Dependency updates](#dependency-updates)):

| Job | Started by | Does |
|---|---|---|
| Renovate | each night at 2am Paris time (two schedules, 00:07 and 01:07 UTC, the one that isn't 2am in Paris doing nothing), by hand, and each push to `main` | runs Renovate on the repository |
| Release the updates | after Renovate | `cargo ship deps-release` |
| Bring main into next | the Release workflow's success on a merged pull request | `cargo ship sync` on `next` |

### Release

[`.github/workflows/release.yml`](../.github/workflows/release.yml), started by:

| What | Started by | Releases |
|---|---|---|
| a beta | a push to `release/<request>` (`cargo ship`) | that commit, which must be on `next` |
| a fix | the merge of a `fix/<name>` pull request labeled `release` into `main` (`cargo ship fix`) | the merge commit |
| a stable release | the merge of a `stable/<version>` pull request labeled `release` into `main` (`cargo ship stable`) | the merge commit, as that version once checked against the tags |
| a try build | `cargo ship try`, or by hand: **Actions** → **Release** → **Run workflow**, on a branch, with `try` | nothing: builds the packages only |

A pull request closed without being merged, or without the label, releases nothing.

| Job | Runs on | Does |
|---|---|---|
| Version | Ubuntu | works out what's released and its version from the tags, once for all the jobs; checks a beta is on `next` |
| Tests | Ubuntu, Windows | `cargo test` (skipped by a try build: its pull request's checks run them) |
| Windows | Windows | builds the MSI and the portable zip as that version |
| Linux | Ubuntu | takes a stable release's notes from `CHANGELOG.md` (failing early if they're missing), builds the .deb and the AppImage |
| GitHub release | Ubuntu | tags the commit, creates the release with the 4 files: a pre-release for a beta, the changelog notes for a stable release or a fix (not for a try build) |
| Remove the release request | Ubuntu | for a beta: deletes the `release/*` branch, whatever happened, so the same request can be pushed again |
| Summary | Ubuntu | a report of what was built and how long it took, on the run's page |

Only one release runs at a time; try builds run beside them, one per branch at a time. The tag is created only once everything passed, so a failed run leaves no tag and no release behind.

The files of a release:

```
Cachemire-3.1.1-x64.msi            Windows installer
Cachemire-Portable-3.1.1-x64.zip   Windows portable exe
cachemire_3.1.1-1_amd64.deb        Debian / Ubuntu package
Cachemire-3.1.1-x86_64.AppImage    Linux, any distribution
```

## Command reference

`cargo ship` is an alias of `cargo xtask release` (see [`.cargo/config.toml`](../.cargo/config.toml)).

| Command | Run on | Does |
|---|---|---|
| `cargo ship` / `cargo ship beta` | `next` | brings in `main`, pushes `next` and releases it as the next beta |
| `cargo ship beta --major` | `next`, no betas under way | starts the betas of the next major (`--minor`, the default, those of the next minor) |
| `cargo ship stable` | `next` | names the changelog section, and merges `next` into `main` by a pull request, to be published |
| `cargo ship stable --minor` / `--major` | `next`, no betas under way | the same, as the next minor or major |
| `cargo ship start-fix <name>` | anywhere | stashes the changes, makes `fix/<name>` from `main` with its draft pull request |
| `cargo ship fix` | `fix/<name>` | rebases it on `main`, names the changelog section, and squashes it into `main` by its pull request, to be published |
| `cargo ship try` | any branch | builds its packages on GitHub, without releasing them, into `target/try/` |
| `cargo ship sync` | `next` | brings in `main`, pushes `next`, deletes the merged `fix/*` branches |
| `cargo ship status` | anywhere | the latest releases, what `next` and `main` lack, the pull requests into `main` and the runs under way |
| `cargo ship setup` | anywhere, once | the GitHub settings, `next` and the rules (see [Setting up](#setting-up)) |
| `cargo ship deps-release` | the Dependencies workflow only | releases the dependency updates merged into `main` (see [Dependency updates](#dependency-updates)) |

`stable` and `fix` take `--yes` to skip their question.

### Release helpers

What the Release workflow runs:

| Command | Prints |
|---|---|
| `cargo xtask next-version <request>` | the version a request releases: `beta`, `beta-minor`, `beta-major`, `stable`, `stable-minor`, `stable-major`, `fix`, or a stable version, checked to be one a stable release may take next |
| `cargo xtask next-version <request> --try <run>` | the version of a try build of it, just below |
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

**`If release/beta is already on origin, a release is under way, or one ended without removing it`**: a beta's run is still going (wait for it), or one was interrupted before its cleanup. Remove the branch with `git push origin --delete release/beta`, then ship again.

**A stable release or a fix fails a check on its pull request**: it stays open, auto-merge on. For a fix, commit the correction on its branch and push; for a stable release, commit it on `next` and run `cargo ship stable` again (the changelog already says `## 3.2.0`, so it's kept as is, and `stable/3.2.0` moves to the new commit). The pull request merges once the checks pass. If you added lines to a new `## Unreleased` meanwhile, it stops and asks you to move them into `## 3.2.0` first.

**The Release workflow fails after the merge**: `main` has the commit, but there's no tag nor release. For a hiccup (a download, a runner), **Re-run failed jobs** on the run's page. For a real problem, release a correction the same way: on a new fix branch for a fix (`cargo ship start-fix`, then `cargo ship fix`), on `next` for a stable release (`cargo ship stable`). The version is still free and its changelog section already named, so it's released as that version, with that section (add the correction's line to it, not under `## Unreleased`).

**`betas are released from next, and HEAD is on main`** (or `stable`, `sync`): `git switch next` first.

**The Dependencies workflow's "Bring main into next" fails**: `main` and `next` changed the same lines of a file other than the changelog and the lock file. Run `cargo ship sync` here, resolve, `git commit`, and run it again to push.

**The Dependencies workflow fails with `Bad credentials`** (or Renovate with an authentication error): the `RENOVATE_TOKEN` secret is missing or has expired; make a new token as in [Setting up](#setting-up), then `gh secret set RENOVATE_TOKEN`.

**`merging main into next stopped on conflicts in …`**: the merge is under way: resolve the files named, `git add` them and `git commit`, then run the command again.

**`fix/tls is still being released (…)`** / **`… is being published (…)`**: one release at a time; wait for it (`cargo ship status` shows where it is), or close the pull request if it's given up.

**`CHANGELOG.md has nothing under ## Unreleased`**: write what the release changes there first.

**`no release tag to start from (are the tags fetched?)`**: `cargo ship` fetches the tags itself; for `next-version`, run `git fetch --tags` first.

**`gh` errors** (`could not run the GitHub CLI`, `authentication required`): install the [GitHub CLI](https://cli.github.com) and sign in with `gh auth login`.

**Local builds of older tags**: without `--as-version` or `--fast`, the build tasks check out the latest release tag. Tags made before the `crates/` and `resources/` layout can't be built by the current tasks; use `--as-version` or `--fast` until a release exists with the new layout.
