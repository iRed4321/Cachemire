//! `CHANGELOG.md`: what changed, written by hand under `## Unreleased`. `cargo ship stable` and `fix` name
//! that section after the version they release and commit it; the Release workflow publishes it as the notes.

use std::time::{SystemTime, UNIX_EPOCH};

use semver::Version;

use crate::tools::{Result, repo_root};

/// The changelog, from the repository root.
pub const CHANGELOG: &str = "CHANGELOG.md";

/// The section that gathers what the next stable release brings.
const UNRELEASED: &str = "Unreleased";

/// The line index and body of the `## <name>` section of `changelog` (text may follow the name in
/// its heading): up to the next `## ` heading, without the blank lines around it.
fn find(changelog: &str, name: &str) -> Option<(usize, String)> {
    let heading = format!("## {name}");
    let lines: Vec<&str> = changelog.lines().collect();
    let start = lines.iter().position(|line| line.trim_end() == heading || line.starts_with(&format!("{heading} ")))?;
    let body: Vec<&str> = lines[start + 1..].iter().take_while(|line| !line.starts_with("## ")).copied().collect();
    Some((start, body.join("\n").trim().to_string()))
}

/// The body of the `## <name>` section, if it has one.
fn section(changelog: &str, name: &str) -> Option<String> {
    find(changelog, name).map(|(_, body)| body).filter(|body| !body.is_empty())
}

/// `changelog` with the heading at `line` replaced by `heading`, under a new empty `## Unreleased`.
fn renamed(changelog: &str, line: usize, heading: &str) -> String {
    changelog
        .split_inclusive('\n')
        .enumerate()
        .map(|(i, text)| {
            if i != line {
                return text.to_string();
            }
            let ending = match &text[text.trim_end().len()..] {
                "" => "\n",
                ending => ending,
            };
            format!("## {UNRELEASED}{ending}{ending}{heading}{ending}")
        })
        .collect()
}

/// Today's date (UTC) as `YYYY-MM-DD`, from the days since 1970 (Howard Hinnant's `civil_from_days`).
fn today() -> String {
    let days = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_secs() / 86_400) as i64 + 719_468;
    let (era, day_of_era) = (days.div_euclid(146_097), days.rem_euclid(146_097));
    let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 { shifted_month + 3 } else { shifted_month - 9 };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

/// The changelog of a stable release: its notes, and the text to write first (`None` when the
/// version's section is already there, from an earlier try of the same release).
pub struct StableChangelog {
    pub notes: String,
    pub text: Option<String>,
}

/// The working tree's changelog for releasing `version`: its `## Unreleased` section named
/// `## <version> - <date>`, or the `## <version>` section an earlier try of this release named.
pub fn for_stable(version: &Version) -> Result<StableChangelog> {
    let current = std::fs::read_to_string(repo_root().join(CHANGELOG)).map_err(|e| format!("reading {CHANGELOG}: {e}"))?;
    let name = version.to_string();
    let text = match (find(&current, &name), find(&current, UNRELEASED).filter(|(_, body)| !body.is_empty())) {
        (Some(_), Some(_)) => {
            return Err(format!(
                "{CHANGELOG} has both a `## {UNRELEASED}` and a `## {name}` section (from an earlier try of {name}): move the unreleased lines into {name}"
            ));
        }
        (Some(_), None) => None,
        (None, Some((line, _))) => Some(renamed(&current, line, &format!("## {name} - {}", today()))),
        (None, None) => return Err(format!("{CHANGELOG} has nothing under `## {UNRELEASED}`: write what {name} changes there first")),
    };
    let notes =
        section(text.as_deref().unwrap_or(&current), &name).ok_or_else(|| format!("the `## {name}` section of {CHANGELOG} is empty"))?;
    Ok(StableChangelog { notes, text })
}

/// The version a `## <version> - <date>` heading names, if it does.
fn heading_version(heading: &str) -> Option<Version> {
    Version::parse(heading.strip_prefix("## ")?.split_whitespace().next()?).ok()
}

/// The text before the first `## ` heading, then each heading with the lines under it.
fn split(changelog: &str) -> (Vec<&str>, Vec<(&str, Vec<&str>)>) {
    let (mut head, mut sections) = (Vec::new(), Vec::<(&str, Vec<&str>)>::new());
    for line in changelog.lines() {
        match sections.last_mut() {
            _ if line.starts_with("## ") => sections.push((line.trim_end(), Vec::new())),
            Some((_, body)) => body.push(line),
            None => head.push(line),
        }
    }
    (head, sections)
}

/// `lines` without the blank ones at their end.
fn trimmed<'a>(lines: &[&'a str]) -> Vec<&'a str> {
    let end = lines.iter().rposition(|line| !line.trim().is_empty()).map_or(0, |i| i + 1);
    lines[..end].to_vec()
}

/// `ours` with the lines of `theirs` it lacks, each at the end of its `### ` subsection (added if missing).
fn merged_unreleased<'a>(ours: &[&'a str], theirs: &[&'a str]) -> Vec<&'a str> {
    let mut out = trimmed(ours);
    let mut subsection = None;
    for &line in theirs {
        if line.starts_with("### ") {
            subsection = Some(line.trim_end());
            continue;
        }
        if line.trim().is_empty() || out.contains(&line) {
            continue;
        }
        let start = subsection.and_then(|heading| out.iter().position(|l| l.trim_end() == heading));
        match start {
            Some(start) => {
                let end = out[start + 1..].iter().position(|l| l.starts_with("### ")).map_or(out.len(), |i| start + 1 + i);
                match trimmed(&out[..end]).len() {
                    // a subsection with nothing under it yet: the line goes after a blank one
                    at if at == start + 1 => out.splice(at..at, ["", line]).for_each(drop),
                    at => out.insert(at, line),
                }
            }
            None => {
                if let Some(heading) = subsection.filter(|heading| !out.contains(heading)) {
                    out.extend(["", heading, ""]);
                }
                out.push(line);
            }
        }
    }
    out
}

/// The changelog of a branch (`ours`, next) merged with main's (`theirs`): its own preamble and
/// `## Unreleased` with the lines main's adds to it, then every released section, newest first,
/// as main has it when both do.
pub fn merged(ours: &str, theirs: &str) -> String {
    let ending = if ours.contains("\r\n") { "\r\n" } else { "\n" };
    let blank_end = ours.lines().rev().take_while(|line| line.trim().is_empty()).count();
    let ((head, ours), (_, theirs)) = (split(ours), split(theirs));
    let unreleased = format!("## {UNRELEASED}");
    fn body<'a>(sections: &[(&'a str, Vec<&'a str>)], heading: &str) -> Vec<&'a str> {
        sections.iter().find(|(h, _)| *h == heading).map(|(_, b)| b.clone()).unwrap_or_default()
    }
    let new_lines = merged_unreleased(&body(&ours, &unreleased), &body(&theirs, &unreleased));
    let mut released: Vec<&(&str, Vec<&str>)> = theirs.iter().filter(|(h, _)| *h != unreleased).collect();
    for section in ours.iter().filter(|(h, _)| *h != unreleased) {
        if !released.iter().any(|(h, _)| *h == section.0) {
            released.push(section);
        }
    }
    released.sort_by_key(|(heading, _)| std::cmp::Reverse(heading_version(heading)));
    let mut lines = trimmed(&head);
    lines.extend(["", unreleased.as_str()]);
    lines.extend(new_lines);
    for (heading, body) in released {
        lines.extend(["", *heading]);
        lines.extend(trimmed(body));
    }
    lines.extend(std::iter::repeat_n("", blank_end));
    lines.join(ending) + ending
}

/// The theme a changelog line starts with: `- **Window**: …` → `Window`.
fn theme(line: &str) -> Option<&str> {
    line.strip_prefix("- **")?.split("**").next()
}

/// `changelog` with `lines` under `### Fixes` of its `## Unreleased` section (both made if missing),
/// each before the first line whose theme sorts after its own, so the themes stay in order.
pub fn with_fixes(changelog: &str, lines: &[String]) -> String {
    let ending = if changelog.contains("\r\n") { "\r\n" } else { "\n" };
    let unreleased = format!("## {UNRELEASED}");
    let (head, mut sections) = split(changelog);
    if !sections.iter().any(|(heading, _)| *heading == unreleased) {
        sections.insert(0, (unreleased.as_str(), vec![""]));
    }
    let (_, body) = sections.iter_mut().find(|(heading, _)| *heading == unreleased).expect("added above");
    let mut body: Vec<&str> = std::mem::take(body);
    let start = match body.iter().position(|line| line.trim_end() == "### Fixes") {
        Some(start) => start,
        None => {
            body = trimmed(&body);
            body.extend(["", "### Fixes"]);
            body.len() - 1
        }
    };
    for line in lines {
        let end = body[start + 1..].iter().position(|l| l.starts_with("### ")).map_or(body.len(), |i| start + 1 + i);
        let later = body[start + 1..end].iter().position(|l| theme(l).zip(theme(line)).is_some_and(|(theirs, ours)| theirs > ours));
        match later.map(|i| start + 1 + i) {
            Some(at) => body.insert(at, line),
            None => match trimmed(&body[..end]).len() {
                // a subsection with nothing under it yet: the line goes after a blank one
                at if at == start + 1 => body.splice(at..at, ["", line.as_str()]).for_each(drop),
                at => body.insert(at, line),
            },
        }
    }
    let end = body.iter().rposition(|line| !line.trim().is_empty()).map_or(0, |i| i + 1);
    if end == body.len() && sections.iter().position(|(heading, _)| *heading == unreleased) != Some(sections.len() - 1) {
        body.push("");
    }
    let mut out: Vec<&str> = head;
    for (heading, lines) in &sections {
        out.push(heading);
        if *heading == unreleased { out.extend(&body) } else { out.extend(lines) }
    }
    out.join(ending) + ending
}

/// `cargo xtask release-notes`: prints the notes of `version` from the changelog of the working tree.
pub fn print_notes(version: &str) -> Result<()> {
    let version = Version::parse(version).map_err(|e| format!("{version} is not a version: {e}"))?;
    let text = std::fs::read_to_string(repo_root().join(CHANGELOG)).map_err(|e| format!("reading {CHANGELOG}: {e}"))?;
    let notes = section(&text, &version.to_string()).ok_or_else(|| {
        format!("{CHANGELOG} has no `## {version}` section: `cargo ship stable` and `cargo ship fix` name the `## {UNRELEASED}` one")
    })?;
    println!("{notes}");
    Ok(())
}
