//! `CHANGELOG.md`: what changed, written by hand under `## Unreleased`. A stable `cargo ship` names that
//! section after the version it releases and commits it; the Release workflow publishes it as the notes.

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

/// `cargo xtask release-notes`: prints the notes of `version` from the changelog of the working tree.
pub fn print_notes(version: &str) -> Result<()> {
    let version = Version::parse(version).map_err(|e| format!("{version} is not a version: {e}"))?;
    let text = std::fs::read_to_string(repo_root().join(CHANGELOG)).map_err(|e| format!("reading {CHANGELOG}: {e}"))?;
    let notes = section(&text, &version.to_string())
        .ok_or_else(|| format!("{CHANGELOG} has no `## {version}` section: a stable release is requested with `cargo ship`, which names the `## {UNRELEASED}` one"))?;
    println!("{notes}");
    Ok(())
}
