//! `cargo ship` (`cargo xtask release`): betas from `next`, fixes and stable releases through pull requests
//! into `main`, which the Release workflow publishes once merged. Drives git and the GitHub CLI (`gh`),
//! so the branches, pull requests and release requests are all made from here.

use std::path::{Path, PathBuf};

use semver::Version;

use crate::changelog::{self, CHANGELOG};
use crate::deps;
use crate::tools::{Result, cargo, gh_command, gh_output, git, git_ok, repo_root, run};
use crate::version::{self, Request};

/// The stable line: what's released as stable, and fixes about to be. Changed by pull requests only.
const MAIN: &str = "main";
/// The betas' line: what the next minor or major brings.
const NEXT: &str = "next";
/// The branches of fixes of the stable release (`fix/<name>`), each with its pull request into main.
const FIX: &str = "fix/";
/// The branches a stable release of next is merged into main from (`stable/<version>`).
const STABLE: &str = "stable/";
/// The fix branches of the dependency updates Renovate merged into main (`fix/deps-<version>`).
const DEPS: &str = "fix/deps-";
/// The lock file, which `cargo ship sync` resolves the conflicts of.
const LOCK: &str = "Cargo.lock";
/// The token the Dependencies workflow works with (a repository secret): its pushes start the checks.
const TOKEN: &str = "RENOVATE_TOKEN";
/// The branches that ask the Release workflow for a beta (`release/<request>`).
const REQUEST: &str = "release/";
/// The label of a pull request the Release workflow publishes once it's merged.
const LABEL: &str = "release";
/// The checks a pull request into main must pass: the CI's tests.
const CHECKS: [&str; 2] = ["Test (ubuntu-latest)", "Test (windows-latest)"];

#[derive(clap::Subcommand)]
pub enum Ship {
    /// On next: release it as the next beta (what `cargo ship` alone does)
    Beta {
        /// After a stable release: start the betas of the next minor (the default)
        #[arg(long)]
        minor: bool,
        /// After a stable release: start the betas of the next major
        #[arg(long, conflicts_with = "minor")]
        major: bool,
    },
    /// On next: release its betas as stable, through a pull request into main merged once its checks pass
    Stable {
        /// With no betas since the last stable release: release the next minor at once
        #[arg(long)]
        minor: bool,
        /// With no betas since the last stable release: release the next major at once
        #[arg(long, conflicts_with = "minor")]
        major: bool,
        /// Don't ask before naming the changelog section and opening the pull request
        #[arg(long)]
        yes: bool,
    },
    /// Start a fix of the stable release: a fix/<name> branch from main, with a draft pull request
    StartFix {
        /// The fix's name, in the branch's: fix/<name>
        name: String,
    },
    /// On a fix/<name> branch: release it as the next fix, merging its pull request once its checks pass
    Fix {
        /// Don't ask before naming the changelog section and readying the pull request
        #[arg(long)]
        yes: bool,
    },
    /// Build the packages of the current branch on GitHub, without releasing them, into target/try
    Try,
    /// On next: bring in what main has that it lacks (fixes, a stable release)
    Sync,
    /// Where the branches, pull requests and releases stand
    Status,
    /// Once: create next and set GitHub up for this process (auto-merge, the rules of main, the label)
    Setup,
    /// In the Dependencies workflow: release what Renovate merged into main as a fix, by a pull request
    DepsRelease,
}

/// `cargo ship [command]`: a beta when no command is given.
pub fn run_task(command: Option<Ship>) -> Result<()> {
    let root = repo_root();
    let repo = github_repo(&root).ok_or("the `origin` remote is not a GitHub repository")?;
    let ship = Shipper { root, repo };
    match command.unwrap_or(Ship::Beta { minor: false, major: false }) {
        Ship::Beta { minor, major } => ship.beta(minor, major),
        Ship::Stable { minor, major, yes } => ship.stable(minor, major, yes),
        Ship::StartFix { name } => ship.start_fix(&name),
        Ship::Fix { yes } => ship.fix(yes),
        Ship::Try => ship.try_build(),
        Ship::Sync => ship.sync_next(),
        Ship::Status => ship.status(),
        Ship::Setup => ship.setup(),
        Ship::DepsRelease => ship.deps_release(),
    }
}

/// The `owner/repo` of the `origin` remote, when it is on GitHub.
fn github_repo(root: &Path) -> Option<String> {
    let url = git(root, &["remote", "get-url", "origin"]).ok()?;
    let path = url.split_once("github.com").map(|(_, path)| path.trim_start_matches([':', '/']))?;
    Some(path.trim_end_matches('/').trim_end_matches(".git").to_string())
}

/// Asks `question` on the terminal: true for y or yes.
fn confirm(question: &str) -> Result<bool> {
    print!("{question} [y/N] ");
    std::io::Write::flush(&mut std::io::stdout()).map_err(|e| e.to_string())?;
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer).map_err(|e| e.to_string())?;
    Ok(matches!(answer.trim().to_lowercase().as_str(), "y" | "yes"))
}

/// The repository, and its `owner/repo` on GitHub.
struct Shipper {
    root: PathBuf,
    repo: String,
}

impl Shipper {
    fn git(&self, args: &[&str]) -> Result<String> {
        git(&self.root, args)
    }

    fn git_ok(&self, args: &[&str]) -> bool {
        git_ok(&self.root, args)
    }

    /// `gh` with `args`, its output returned.
    fn gh(&self, args: &[&str]) -> Result<String> {
        gh_output(gh_command(&self.root, &self.repo).args(args))
    }

    /// `gh` with `args`, its output shown as it goes (a run being watched, a download).
    fn gh_shown(&self, args: &[&str]) -> Result<()> {
        let status =
            gh_command(&self.root, &self.repo).args(args).status().map_err(|e| format!("could not run the GitHub CLI (gh): {e}"))?;
        status.success().then_some(()).ok_or_else(|| format!("`gh {}` failed ({status})", args.join(" ")))
    }

    /// The branch HEAD is on; `None` when detached.
    fn branch(&self) -> Option<String> {
        self.git(&["symbolic-ref", "--quiet", "--short", "HEAD"]).ok()
    }

    /// Fails unless HEAD is on `branch`, saying `why`.
    fn on(&self, branch: &str, why: &str) -> Result<()> {
        match self.branch() {
            Some(current) if current == branch => Ok(()),
            current => {
                Err(format!("{why}, and HEAD is on {}: `git switch {branch}` first", current.as_deref().unwrap_or("a detached commit")))
            }
        }
    }

    /// Whether `ancestor` is part of `of`'s history.
    fn contains(&self, of: &str, ancestor: &str) -> bool {
        self.git_ok(&["merge-base", "--is-ancestor", ancestor, of])
    }

    /// Whether the files git tracks have uncommitted changes.
    fn dirty(&self) -> Result<bool> {
        Ok(!self.git(&["status", "--porcelain", "--untracked-files=no"])?.is_empty())
    }

    /// Brings the branches and tags of GitHub, forgetting the branches deleted there.
    fn fetch(&self) -> Result<()> {
        self.git(&["fetch", "--quiet", "--prune", "--tags", "origin"]).map(drop)
    }

    fn note_uncommitted(&self) -> Result<()> {
        if self.dirty()? {
            println!("Note: the uncommitted changes are not part of it, only HEAD is.");
        }
        Ok(())
    }

    /// The commit HEAD is on, as `a1b2c3d Its subject`.
    fn head(&self) -> Result<String> {
        self.git(&["log", "-1", "--format=%h %s"])
    }

    /// Where the runs of the Release workflow are watched.
    fn runs_url(&self) -> String {
        format!("https://github.com/{}/actions/workflows/release.yml", self.repo)
    }

    /// Pushes HEAD as `branch` on GitHub, with `force` when its history was rewritten (a rebased fix).
    fn push(&self, branch: &str, force: bool) -> Result<()> {
        let refspec = format!("HEAD:refs/heads/{branch}");
        let lease = format!("--force-with-lease=refs/heads/{branch}");
        let args: Vec<&str> = ["push", "--quiet"].into_iter().chain(force.then_some(lease.as_str())).chain(["origin", &refspec]).collect();
        self.git(&args).map(drop)
    }

    /// Fails while a release is being published, unless `publishing_ok`, or a pull request is waiting to be
    /// released, unless `open_ok` (by their branch): one at a time, as each version follows the one before.
    fn no_release_pending(&self, open_ok: impl Fn(&str) -> bool, publishing_ok: impl Fn(&str) -> bool) -> Result<()> {
        // a merged pull request's release run is on its branch
        let publishing = r#".[] | select(.status != "completed" and .event == "pull_request") | "\(.headBranch) \(.url)""#;
        let runs = self.gh(&[
            "run",
            "list",
            "--workflow",
            "release.yml",
            "--limit",
            "20",
            "--json",
            "status,event,headBranch,url",
            "--jq",
            publishing,
        ])?;
        for line in runs.lines() {
            let (head, url) = line.split_once(' ').unwrap_or((line, ""));
            if !publishing_ok(head) {
                return Err(format!("{head} is being published ({url}): wait for it to end first"));
            }
        }
        let open = self.gh(&[
            "pr",
            "list",
            "--label",
            LABEL,
            "--state",
            "open",
            "--json",
            "headRefName,url",
            "--jq",
            r#".[] | "\(.headRefName) \(.url)""#,
        ])?;
        for line in open.lines() {
            let (head, url) = line.split_once(' ').unwrap_or((line, ""));
            if !open_ok(head) {
                return Err(format!("{head} is still being released ({url}): wait until it's merged and published, or close it"));
            }
        }
        Ok(())
    }

    /// next brought up to date with GitHub's, which the Dependencies workflow may have moved on, then with main.
    fn sync(&self) -> Result<()> {
        self.bring_in(&format!("origin/{NEXT}"))?;
        self.bring_in(&format!("origin/{MAIN}"))
    }

    /// HEAD (next) with `rev` in it: fast-forwarded, or merged with it, resolving the conflicts of the
    /// changelog (see [`changelog::merged`]) and of `Cargo.lock` (`rev`'s, completed). Stops on any other.
    fn bring_in(&self, rev: &str) -> Result<()> {
        if self.contains("HEAD", rev) {
            return Ok(());
        }
        if self.contains(rev, "HEAD") {
            self.git(&["merge", "--quiet", "--ff-only", rev])?;
            println!("Fast-forwarded {NEXT} to {rev}.");
            return Ok(());
        }
        if self.dirty()? {
            return Err(format!(
                "{NEXT} must first take in what {rev} has, and the working tree has uncommitted changes: commit or stash them first"
            ));
        }
        let message = format!("Merge {rev} into {NEXT}");
        if let Err(e) = self.git(&["merge", "--quiet", "--no-ff", "-m", &message, rev]) {
            let conflicts = self.git(&["diff", "--name-only", "--diff-filter=U"])?;
            if conflicts.is_empty() {
                return Err(e);
            }
            let has = |file: &str| conflicts.lines().any(|f| f == file);
            if has(CHANGELOG) {
                let ours = self.git(&["show", &format!(":2:{CHANGELOG}")])?;
                let theirs = self.git(&["show", &format!(":3:{CHANGELOG}")])?;
                std::fs::write(self.root.join(CHANGELOG), changelog::merged(&ours, &theirs))
                    .map_err(|e| format!("writing {CHANGELOG}: {e}"))?;
                self.git(&["add", "--", CHANGELOG])?;
            }
            let left: Vec<&str> = conflicts.lines().filter(|file| *file != CHANGELOG && *file != LOCK).collect();
            if !left.is_empty() {
                return Err(format!(
                    "merging {rev} into {NEXT} stopped on conflicts in {}: resolve them, `git add` them and `git commit`, then run this again",
                    left.join(", ")
                ));
            }
            if has(LOCK) {
                // the versions rev locks, with what next's manifests need on top
                self.git(&["checkout", "--theirs", "--", LOCK])?;
                run(cargo().current_dir(&self.root).args(["update", "--workspace", "--quiet"]))?;
                self.git(&["add", "--", LOCK])?;
            }
            self.git(&["commit", "--quiet", "--no-edit"])?;
        }
        println!("Merged {rev} into {NEXT}.");
        Ok(())
    }

    /// Pushes next when GitHub's lacks some of it.
    fn push_next(&self) -> Result<()> {
        if self.git(&["rev-parse", "HEAD"])? != self.git(&["rev-parse", &format!("origin/{NEXT}")])? {
            self.push(NEXT, false)?;
        }
        Ok(())
    }

    /// `cargo ship sync`: next with what main has, pushed; the fix branches merged since are removed.
    fn sync_next(&self) -> Result<()> {
        self.on(NEXT, "sync brings main into next")?;
        self.fetch()?;
        self.sync()?;
        self.push_next()?;
        self.remove_merged_fixes()?;
        println!("{NEXT} has everything {MAIN} has.");
        Ok(())
    }

    /// Deletes the local fix branches whose pull request is merged (GitHub then deleted the branch).
    fn remove_merged_fixes(&self) -> Result<()> {
        let branches = self.git(&["for-each-ref", "--format=%(refname:short) %(upstream:track)", &format!("refs/heads/{FIX}")])?;
        for line in branches.lines().filter(|line| line.ends_with("[gone]")) {
            let branch = line.split(' ').next().unwrap_or_default();
            let state = self.gh(&["pr", "view", branch, "--json", "state", "--jq", ".state"]).unwrap_or_default();
            if state == "MERGED" && self.branch().as_deref() != Some(branch) {
                self.git(&["branch", "--quiet", "-D", branch])?;
                println!("Removed {branch}: its pull request is merged.");
            }
        }
        Ok(())
    }

    /// `cargo ship beta`: next, brought up to date and pushed, released as the next beta by pushing the
    /// `release/<request>` branch the Release workflow starts from.
    fn beta(&self, minor: bool, major: bool) -> Result<()> {
        self.on(NEXT, "betas are released from next")?;
        self.fetch()?;
        // a pending stable release of next comes first; a fix, of the stable line, doesn't hold betas up
        let fix = |head: &str| head.starts_with(FIX);
        self.no_release_pending(fix, fix)?;
        self.sync()?;
        let request = if major {
            Request::BetaMajor
        } else if minor {
            Request::BetaMinor
        } else {
            Request::Beta
        };
        let version = version::next_version(&self.root, &request)?;
        self.note_uncommitted()?;
        self.push_next()?;
        println!("Requesting the release of {version} from {}...", self.head()?);
        let branch = format!("{REQUEST}{}", request.name());
        self.push(&branch, false).map_err(|e| {
            format!("{e}\nIf {branch} is already on origin, a release is under way, or one ended without removing it: wait, or remove it with `git push origin --delete {branch}`.")
        })?;
        println!("Requested. The Release workflow tags and publishes v{version} once its tests and builds pass: {}", self.runs_url());
        Ok(())
    }

    /// Names the changelog's `## Unreleased` section after `version`, once confirmed (unless `yes`),
    /// and commits it alone. The release notes, or `None` when called off with nothing changed.
    fn name_changelog(&self, version: &Version, yes: bool, then: &str) -> Result<Option<String>> {
        let changelog = changelog::for_stable(version)?;
        println!("Release notes of {version}, from {CHANGELOG}:\n\n{}\n", changelog.notes);
        if !yes && !confirm(&format!("Release {version} with these notes, {then}?"))? {
            println!("Called off: nothing was changed.");
            return Ok(None);
        }
        if let Some(text) = changelog.text {
            std::fs::write(self.root.join(CHANGELOG), text).map_err(|e| format!("writing {CHANGELOG}: {e}"))?;
        }
        // committed alone, whatever else is staged; skipped when HEAD already has it as it is
        if !self.git(&["status", "--porcelain", "--", CHANGELOG])?.is_empty() {
            self.git(&["add", "--", CHANGELOG])?;
            self.git(&["commit", "--quiet", "--message", &format!("Release {version}"), "--", CHANGELOG])?;
        }
        Ok(Some(changelog.notes))
    }

    /// The open pull request of `branch` into main, made or updated with `title` and `body`; marked
    /// ready unless `draft`. Its address.
    fn pull_request(&self, branch: &str, title: &str, body: &str, draft: bool) -> Result<String> {
        let found = self.gh(&[
            "pr",
            "list",
            "--head",
            branch,
            "--base",
            MAIN,
            "--state",
            "open",
            "--json",
            "url,isDraft",
            "--jq",
            r#".[] | "\(.url) \(.isDraft)""#,
        ])?;
        let Some((url, is_draft)) = found.lines().next().and_then(|line| line.split_once(' ')) else {
            let args = ["pr", "create", "--base", MAIN, "--head", branch, "--title", title, "--body", body];
            return self.gh(&args.into_iter().chain(draft.then_some("--draft")).collect::<Vec<_>>());
        };
        self.gh(&["pr", "edit", url, "--title", title, "--body", body])?;
        if is_draft == "true" && !draft {
            self.gh(&["pr", "ready", url])?;
        }
        Ok(url.to_string())
    }

    /// Labels the pull request at `url` for release and merges it (`method`: `--squash` or `--merge`)
    /// as `subject` once its checks pass: at once if they already have.
    fn merge_when_ready(&self, url: &str, method: &str, subject: &str) -> Result<()> {
        self.gh(&["pr", "edit", url, "--add-label", LABEL])?;
        match self.gh(&["pr", "merge", url, "--auto", method, "--subject", subject]) {
            // GitHub refuses auto-merge for a pull request that can be merged already
            Err(e) if e.contains("clean status") => self.gh(&["pr", "merge", url, method, "--subject", subject]).map(drop),
            result => result.map(drop),
        }
    }

    /// `cargo ship stable`: next, up to date with main, its changelog section named, pushed as
    /// `stable/<version>` with a pull request into main, merged once its checks pass, then published.
    fn stable(&self, minor: bool, major: bool, yes: bool) -> Result<()> {
        self.on(NEXT, "a stable release is made from next, through a pull request into main")?;
        self.fetch()?;
        // its own pull request is open when this is run again (after a failed check)
        self.no_release_pending(|head| head.starts_with(STABLE), |_| false)?;
        self.sync()?;
        let request = if major {
            Request::StableMajor
        } else if minor {
            Request::StableMinor
        } else {
            Request::Stable
        };
        let version = version::next_version(&self.root, &request)?;
        self.note_uncommitted()?;
        let then = format!("committing {CHANGELOG} on {NEXT} and merging it into {MAIN} once its checks pass");
        let Some(notes) = self.name_changelog(&version, yes, &then)? else { return Ok(()) };
        self.push_next()?;
        let branch = format!("{STABLE}{version}");
        // a branch of its own: next can move on while the release is checked and merged
        self.git(&["push", "--quiet", "--force", "origin", &format!("HEAD:refs/heads/{branch}")])?;
        let title = format!("Release {version}");
        let body = format!(
            "{notes}\n\n---\nMerged once its checks pass, then published as v{version} by the Release workflow (`cargo ship stable`)."
        );
        let url = self.pull_request(&branch, &title, &body, false)?;
        self.merge_when_ready(&url, "--merge", &title)?;
        println!(
            "{url} merges {NEXT} into {MAIN} once its checks pass; the Release workflow then publishes v{version}: {}",
            self.runs_url()
        );
        Ok(())
    }

    /// `cargo ship start-fix`: a `fix/<name>` branch from main, the work in progress put aside (stashed),
    /// pushed with a draft pull request into main, so its checks run at each push.
    fn start_fix(&self, name: &str) -> Result<()> {
        let branch = format!("{FIX}{name}");
        if !self.git_ok(&["check-ref-format", "--branch", &branch]) {
            return Err(format!("`{name}` can't name a branch: use letters, digits and dashes"));
        }
        self.fetch()?;
        let known = |reference: String| self.git_ok(&["rev-parse", "--verify", "--quiet", &reference]);
        if known(format!("refs/heads/{branch}")) || known(format!("refs/remotes/origin/{branch}")) {
            return Err(format!("{branch} already exists: `git switch {branch}` to work on it"));
        }
        let from = self.branch().unwrap_or(self.git(&["rev-parse", "HEAD"])?);
        let stashed = !self.git(&["status", "--porcelain"])?.is_empty();
        if stashed {
            self.git(&["stash", "push", "--quiet", "--include-untracked", "--message", &stash_message(&from, &branch)])?;
            println!("Stashed the uncommitted changes of {from}: `cargo ship fix` brings them back.");
        }
        if let Err(e) = self.git(&["switch", "--quiet", "--no-track", "--create", &branch, &format!("origin/{MAIN}")]) {
            if stashed {
                self.git(&["stash", "pop", "--quiet"])?;
            }
            return Err(e);
        }
        let started = || -> Result<String> {
            self.git(&["config", &format!("branch.{branch}.ship-from"), &from])?;
            self.git(&["config", &format!("branch.{branch}.ship-stashed"), if stashed { "true" } else { "false" }])?;
            // GitHub only opens a pull request on a branch with a commit of its own; the squash merge drops it
            self.git(&["commit", "--quiet", "--allow-empty", "--message", &format!("Start fix {name}")])?;
            self.git(&["push", "--quiet", "--set-upstream", "origin", &branch])?;
            let body = "A fix of the stable release. `cargo ship fix` releases it: it names its changelog section, \
                        marks this pull request ready and merges it once its checks pass.";
            self.pull_request(&branch, &format!("Fix {name}"), body, true)
        };
        let url = started().map_err(|e| {
            let stash = if stashed { format!("; the changes of {from} are stashed (`git stash list`)") } else { String::new() };
            format!("{e}\nHEAD is on {branch}{stash}.")
        })?;
        let stable = version::latest(&self.root)?.stable.map_or_else(|| MAIN.to_string(), |v| format!("v{v}"));
        println!("On {branch}, from {MAIN} ({stable}), with the draft pull request {url}.");
        println!(
            "Commit and push the fix: the checks run at each push. `cargo ship try` builds its packages, `cargo ship fix` releases it."
        );
        Ok(())
    }

    /// `cargo ship fix`: the fix branch rebased on main, its changelog section named, pushed, and its pull
    /// request readied to be squashed into main once its checks pass, then published. Back where it started.
    fn fix(&self, yes: bool) -> Result<()> {
        let branch = self.branch().filter(|b| b.starts_with(FIX)).ok_or(
            "`cargo ship fix` releases the fix/<name> branch HEAD is on: start one with `cargo ship start-fix <name>`, or switch to it",
        )?;
        if self.dirty()? {
            return Err("the fix has uncommitted changes: commit them first (or stash them), as only commits are released".into());
        }
        self.fetch()?;
        self.no_release_pending(|head| head == branch, |_| false)?;
        let main = format!("origin/{MAIN}");
        if !self.contains("HEAD", &main) {
            println!("Rebasing {branch} on {MAIN}, which has moved on...");
            if let Err(e) = self.git(&["rebase", "--quiet", &main]) {
                let _ = self.git(&["rebase", "--abort"]);
                return Err(format!(
                    "{e}\nRebasing {branch} on {MAIN} stopped on conflicts: `git rebase {main}`, resolve them, then run this again"
                ));
            }
        }
        if self.git_ok(&["diff", "--quiet", &main, "HEAD"]) {
            return Err(format!("{branch} doesn't change anything yet: commit the fix first"));
        }
        let version = version::next_version(&self.root, &Request::Fix)?;
        let then = format!("committing {CHANGELOG} and merging the fix into {MAIN} once its checks pass");
        let Some(notes) = self.name_changelog(&version, yes, &then)? else { return Ok(()) };
        self.push(&branch, true)?;
        let name = &branch[FIX.len()..];
        let title = format!("Release {version}: {name}");
        let body = format!(
            "{notes}\n\n---\nSquashed into {MAIN} once its checks pass, then published as v{version} by the Release workflow (`cargo ship fix`)."
        );
        let url = self.pull_request(&branch, &title, &body, false)?;
        self.merge_when_ready(&url, "--squash", &title)?;
        println!(
            "{url} merges the fix into {MAIN} once its checks pass; the Release workflow then publishes v{version}: {}",
            self.runs_url()
        );
        println!(
            "Once it's out, {NEXT} takes it in at the next `cargo ship` (or `cargo ship sync`): a beta brings it to the beta channel."
        );
        self.go_back(&branch)
    }

    /// Back on the branch `cargo ship start-fix` left for `branch`, its stashed changes restored.
    fn go_back(&self, branch: &str) -> Result<()> {
        let from = self.git(&["config", &format!("branch.{branch}.ship-from")]).unwrap_or_else(|_| NEXT.to_string());
        let stashed = self.git(&["config", &format!("branch.{branch}.ship-stashed")]).is_ok_and(|v| v == "true");
        self.git(&["checkout", "--quiet", &from]).map_err(|e| format!("{e}\nHEAD stays on {branch}"))?;
        println!("Back on {from}.");
        if !stashed {
            return Ok(());
        }
        let message = stash_message(&from, branch);
        let stashes = self.git(&["stash", "list", "--format=%gd %s"])?;
        let Some(entry) = stashes.lines().find(|line| line.ends_with(&message)).and_then(|line| line.split(' ').next()) else {
            println!("Its stashed changes aren't in the stash any more (`git stash list`).");
            return Ok(());
        };
        self.git(&["stash", "pop", "--quiet", entry])
            .map_err(|e| format!("{e}\nThe stashed changes of {from} stay in the stash ({entry})"))?;
        let _ = self.git(&["config", "--unset", &format!("branch.{branch}.ship-stashed")]);
        println!("Its uncommitted changes are back.");
        Ok(())
    }

    /// `cargo ship try`: the current branch, pushed, built by the Release workflow just below the version
    /// it leads to (`cargo xtask next-version --try`), not released; its packages downloaded into target/try.
    fn try_build(&self) -> Result<()> {
        let branch = self.branch().ok_or("HEAD is detached: try builds a branch, as GitHub has it")?;
        self.note_uncommitted()?;
        self.fetch()?;
        if branch == MAIN {
            if self.git(&["rev-parse", "HEAD"])? != self.git(&["rev-parse", &format!("origin/{MAIN}")])? {
                return Err(format!("{MAIN} changes through pull requests only: try builds origin/{MAIN} when HEAD is on it"));
            }
        } else {
            self.push(&branch, false).map_err(|e| format!("{e}\nGitHub's {branch} has commits this one lacks: bring them in first"))?;
        }
        let list = [
            "run",
            "list",
            "--workflow",
            "release.yml",
            "--event",
            "workflow_dispatch",
            "--branch",
            &branch,
            "--json",
            "databaseId",
            "--jq",
            ".[].databaseId",
        ];
        let before = self.gh(&list)?;
        self.gh(&["workflow", "run", "release.yml", "--ref", &branch, "--field", "request=try"])?;
        // the run takes a few seconds to be listed
        let mut id = None;
        for _ in 0..30 {
            std::thread::sleep(std::time::Duration::from_secs(2));
            id = self.gh(&list)?.lines().find(|id| !before.lines().any(|known| known == *id)).map(str::to_string);
            if id.is_some() {
                break;
            }
        }
        let id = id.ok_or_else(|| format!("the try build was asked for, but its run doesn't show up: see {}", self.runs_url()))?;
        println!("Building {branch} at {}: https://github.com/{}/actions/runs/{id}", self.head()?, self.repo);
        self.gh_shown(&["run", "watch", &id, "--exit-status", "--compact", "--interval", "15"])?;
        let out = self.root.join("target/try");
        let _ = std::fs::remove_dir_all(&out);
        let dir = out.to_string_lossy().into_owned();
        self.gh_shown(&["run", "download", &id, "--pattern", "package-*", "--dir", &dir])?;
        println!("\nThe packages, to install and try:");
        for file in files_in(&out) {
            println!("  {}", file.display());
        }
        Ok(())
    }

    /// `cargo ship deps-release`, in the Dependencies workflow: when main has dependency updates the latest
    /// stable release lacks, and no other release is under way, a `fix/deps-<version>` pull request of main
    /// with their changelog lines, merged once its checks pass and published as a fix; rebuilt if open.
    fn deps_release(&self) -> Result<()> {
        if std::env::var_os("GITHUB_ACTIONS").is_none() {
            return Err("deps-release runs in the Dependencies workflow: it resets the branch of its checkout".into());
        }
        self.fetch()?;
        let stable = version::latest(&self.root)?.stable.ok_or("no stable release to fix")?;
        let (tag, main) = (format!("v{stable}"), format!("origin/{MAIN}"));
        if self.git(&["rev-list", "--count", &format!("{tag}..{main}")])? == "0" {
            println!("{MAIN} is what {tag} released: nothing to release.");
            return Ok(());
        }
        let head = self.git(&["rev-parse", &main])?;
        let labels = self.gh(&["api", &format!("repos/{}/commits/{head}/pulls", self.repo), "--jq", ".[].labels[].name"])?;
        if labels.lines().any(|label| label == LABEL) {
            println!("The last merge into {MAIN} is being released, the updates with it.");
            return Ok(());
        }
        if let Err(e) = self.no_release_pending(|branch| branch.starts_with(DEPS), |_| false) {
            println!("Not now, the next run will: {e}.");
            return Ok(());
        }
        // one release for the updates of a night: Renovate's pull requests still on their way into main
        // (auto-merge on, no failed check) come first; the merge of the last one runs this again
        let on_their_way = r#".[] | select((.headRefName | startswith("renovate/")) and .autoMergeRequest != null
            and ([.statusCheckRollup[]? | select((.conclusion // .state) == "FAILURE" or (.conclusion // .state) == "ERROR")] | length == 0)) | .url"#;
        let fields = "headRefName,autoMergeRequest,statusCheckRollup,url";
        let pending = self.gh(&["pr", "list", "--base", MAIN, "--state", "open", "--json", fields, "--jq", on_their_way])?;
        if let Some(url) = pending.lines().next() {
            println!("Not now: {url} is on its way into {MAIN}, its merge runs this again.");
            return Ok(());
        }
        let lines = deps::changelog_lines(&self.root, &tag, &main)?;
        if lines.is_empty() {
            println!("The app's dependencies are those of {tag}: what {MAIN} has since goes out with the next release.");
            return Ok(());
        }
        let version = version::next_version(&self.root, &Request::Fix)?;
        let open = self.gh(&["pr", "list", "--label", LABEL, "--state", "open", "--json", "headRefName", "--jq", ".[].headRefName"])?;
        let branch = open.lines().find(|b| b.starts_with(DEPS)).map_or_else(|| format!("{DEPS}{version}"), str::to_string);
        // the branch is the workflow's own: made again from main as it is now
        self.git(&["checkout", "--quiet", "-B", &branch, &main])?;
        let path = self.root.join(CHANGELOG);
        let text = std::fs::read_to_string(&path).map_err(|e| format!("reading {CHANGELOG}: {e}"))?;
        std::fs::write(&path, changelog::with_fixes(&text, &lines)).map_err(|e| format!("writing {CHANGELOG}: {e}"))?;
        let notes = self.name_changelog(&version, true, "")?.unwrap_or_default();
        self.git(&["push", "--quiet", "--force", "origin", &format!("HEAD:refs/heads/{branch}")])?;
        let title = format!("Release {version}: dependencies");
        let body = format!(
            "{notes}\n\n---\nThe dependency updates merged into {MAIN} since {tag}, squashed into it once its checks pass, then published as v{version} by the Release workflow (`cargo ship deps-release`)."
        );
        let url = self.pull_request(&branch, &title, &body, false)?;
        self.merge_when_ready(&url, "--squash", &title)?;
        println!("{url} releases the updates as v{version} once its checks pass.");
        Ok(())
    }

    /// `cargo ship status`: the latest releases, what next and main have that the other lacks, the open
    /// pull requests and the runs of the Release workflow under way.
    fn status(&self) -> Result<()> {
        self.fetch()?;
        let latest = version::latest(&self.root)?;
        let stable = latest.stable.as_ref().map_or_else(|| "none".to_string(), |v| format!("v{v}"));
        println!("On:      {}", self.branch().unwrap_or_else(|| "a detached commit".into()));
        println!("Stable:  {stable}");
        if latest.last.pre.is_empty() {
            println!("Betas:   none since");
        } else {
            println!("Betas:   v{}", latest.last);
        }
        let count = |range: String| self.git(&["rev-list", "--count", &range]).unwrap_or_else(|_| "?".into());
        let (main, next) = (format!("origin/{MAIN}"), format!("origin/{NEXT}"));
        if !self.git_ok(&["rev-parse", "--verify", "--quiet", &next]) {
            println!("{NEXT}:    not on GitHub yet: `cargo ship setup`");
        } else {
            println!("{NEXT}:    {} commits since v{}", count(format!("v{}..{next}", latest.last)), latest.last);
            let behind = count(format!("{next}..{main}"));
            if behind != "0" {
                println!("         {behind} commits of {MAIN} it lacks: `cargo ship sync`");
            }
        }
        if let Some(stable) = &latest.stable {
            println!("{MAIN}:    {} commits since v{stable}", count(format!("v{stable}..{main}")));
        }
        let format = r#".[] | "  #\(.number) \(.title) (\(.headRefName))\(if .isDraft then ", draft" else "" end)\(if .autoMergeRequest then ", merges once its checks pass" else "" end)""#;
        let prs = self.gh(&[
            "pr",
            "list",
            "--base",
            MAIN,
            "--state",
            "open",
            "--json",
            "number,title,headRefName,isDraft,autoMergeRequest",
            "--jq",
            format,
        ])?;
        println!("\nPull requests into {MAIN}:\n{}", if prs.is_empty() { "  none" } else { &prs });
        let format = r#".[] | select(.status != "completed") | "  \(.displayTitle) (\(.headBranch), \(.status)): \(.url)""#;
        let runs = self.gh(&[
            "run",
            "list",
            "--workflow",
            "release.yml",
            "--limit",
            "20",
            "--json",
            "displayTitle,headBranch,status,url",
            "--jq",
            format,
        ])?;
        println!("\nReleases and try builds under way:\n{}", if runs.is_empty() { "  none" } else { &runs });
        Ok(())
    }

    /// `cargo ship setup`: next on GitHub and here, the repository's settings (auto-merge, merged branches
    /// deleted), the `release` label and the rules of main (pull requests passing the checks) and next.
    fn setup(&self) -> Result<()> {
        self.gh(&["auth", "status"]).map_err(|e| format!("{e}\nSign in to GitHub first: `gh auth login`"))?;
        self.fetch()?;
        let repo = format!("repos/{}", self.repo);
        let settings = ["allow_auto_merge=true", "delete_branch_on_merge=true", "allow_merge_commit=true", "allow_squash_merge=true"];
        let mut args = vec!["api", "--method", "PATCH", &repo, "--silent"];
        for setting in &settings {
            args.extend(["--field", setting]);
        }
        self.gh(&args)?;
        println!("Auto-merge is on, and merged branches are deleted.");
        self.gh(&[
            "label",
            "create",
            LABEL,
            "--color",
            "0e8a16",
            "--description",
            "Published by the Release workflow once merged (cargo ship)",
            "--force",
        ])?;
        println!("The `{LABEL}` label is there.");
        if !self.git_ok(&["rev-parse", "--verify", "--quiet", &format!("refs/remotes/origin/{NEXT}")]) {
            self.git(&["push", "--quiet", "origin", &format!("origin/{MAIN}:refs/heads/{NEXT}")])?;
            self.fetch()?;
            println!("Created {NEXT} on GitHub, from {MAIN}.");
        }
        if !self.git_ok(&["rev-parse", "--verify", "--quiet", &format!("refs/heads/{NEXT}")]) {
            self.git(&["branch", "--quiet", "--track", NEXT, &format!("origin/{NEXT}")])?;
            println!("Created {NEXT} here, following GitHub's.");
        }
        let checks: Vec<serde_json::Value> = CHECKS.iter().map(|check| serde_json::json!({ "context": check })).collect();
        let pull_request = serde_json::json!({
            "type": "pull_request",
            "parameters": {
                "required_approving_review_count": 0,
                "dismiss_stale_reviews_on_push": false,
                "require_code_owner_review": false,
                "require_last_push_approval": false,
                "required_review_thread_resolution": false
            }
        });
        let status_checks = serde_json::json!({
            "type": "required_status_checks",
            "parameters": { "strict_required_status_checks_policy": true, "required_status_checks": checks }
        });
        let kept = [serde_json::json!({ "type": "deletion" }), serde_json::json!({ "type": "non_fast_forward" })];
        let main_rules: Vec<serde_json::Value> = kept.iter().cloned().chain([pull_request, status_checks]).collect();
        self.ruleset(MAIN, main_rules)?;
        println!("{MAIN} only changes through pull requests whose checks pass.");
        self.ruleset(NEXT, kept.to_vec())?;
        println!("{NEXT} can't be deleted or have its history rewritten.");
        self.gh(&["api", "--method", "PUT", &format!("{repo}/vulnerability-alerts"), "--silent"])?;
        println!("Dependabot alerts are on: Renovate reads them to fix the advisories of indirect dependencies.");
        let secrets = self.gh(&["secret", "list", "--json", "name", "--jq", ".[].name"])?;
        if secrets.lines().any(|name| name == TOKEN) {
            println!("The {TOKEN} secret is there.");
        } else {
            println!(
                "\nThe Dependencies workflow needs a token of yours, as the {TOKEN} secret, for Renovate and for its pull requests'\n\
                 checks to run. Make a fine-grained one for this repository alone: https://github.com/settings/personal-access-tokens/new\n\
                 with the permissions Contents, Pull requests, Issues, Workflows, Commit statuses (read and write), Actions and\n\
                 Dependabot alerts (read), then: gh secret set {TOKEN}"
            );
        }
        if self.branch().as_deref() == Some(MAIN) && !self.dirty()? && self.contains(&format!("origin/{NEXT}"), "HEAD") {
            self.git(&["switch", "--quiet", NEXT])?;
            println!("Switched to {NEXT}: work goes there from now on.");
        } else {
            println!("Work goes on {NEXT} from now on (`git switch {NEXT}`).");
        }
        Ok(())
    }

    /// Creates or updates the ruleset of `branch`, named after it, with `rules`.
    fn ruleset(&self, branch: &str, rules: Vec<serde_json::Value>) -> Result<()> {
        let name = format!("cargo ship: {branch}");
        let body = serde_json::json!({
            "name": name,
            "target": "branch",
            "enforcement": "active",
            "conditions": { "ref_name": { "include": [format!("refs/heads/{branch}")], "exclude": [] } },
            "rules": rules,
        });
        let path = format!("repos/{}/rulesets", self.repo);
        let existing = self.gh(&["api", &path, "--jq", &format!(r#".[] | select(.name == "{name}") | .id"#)])?;
        let (method, path) = match existing.lines().next() {
            Some(id) => ("PUT", format!("{path}/{id}")),
            None => ("POST", path),
        };
        let mut child = gh_command(&self.root, &self.repo)
            .args(["api", "--method", method, &path, "--silent", "--input", "-"])
            .stdin(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| format!("could not run the GitHub CLI (gh): {e}"))?;
        std::io::Write::write_all(&mut child.stdin.take().ok_or("no stdin for gh")?, body.to_string().as_bytes())
            .map_err(|e| e.to_string())?;
        let output = child.wait_with_output().map_err(|e| e.to_string())?;
        if !output.status.success() {
            return Err(format!("setting the rules of {branch} failed: {}", String::from_utf8_lossy(&output.stderr).trim()));
        }
        Ok(())
    }
}

/// The message `cargo ship start-fix` stashes the changes of `from` under, to find them back.
fn stash_message(from: &str, branch: &str) -> String {
    format!("cargo ship: {from} before {branch}")
}

/// The files under `dir`, recursively, sorted.
fn files_in(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut dirs = vec![dir.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        for path in std::fs::read_dir(&dir).into_iter().flatten().flatten().map(|entry| entry.path()) {
            if path.is_dir() { dirs.push(path) } else { files.push(path) }
        }
    }
    files.sort();
    files
}
