//! `cargo xtask i18n`: extracts every `@tr("...")` of the .slint files into
//! lang/cachemire.pot, merges it into each language's .po (keeping what is translated,
//! adding new messages empty) and lists what is left to translate.

use std::collections::HashMap;
use std::path::Path;

use rspolib::{POEntry, Save, pofile};

use crate::tools::{Result, cargo, repo_root, run, works};

const EXTRACTOR: &str = "slint-tr-extractor";

pub fn run_task() -> Result<()> {
    let root = repo_root();
    if !works(EXTRACTOR, "--version") {
        println!("{EXTRACTOR} is not installed: installing it (once)...");
        run(cargo().args(["install", EXTRACTOR, "--locked"]))?;
    }

    let mut sources = vec!["ui/main.slint".to_string()];
    let mut components: Vec<String> = std::fs::read_dir(root.join("ui/components"))
        .map_err(|e| format!("reading ui/components: {e}"))?
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter(|name| name.ends_with(".slint"))
        .map(|name| format!("ui/components/{name}"))
        .collect();
    components.sort();
    sources.extend(components);
    run(std::process::Command::new(EXTRACTOR)
        .current_dir(&root)
        .args(["--no-default-translation-context", "--package-name", "cachemire", "-o", "lang/cachemire.pot"])
        .args(&sources))?;

    let pot = root.join("lang/cachemire.pot");
    let mut languages: Vec<_> = std::fs::read_dir(root.join("lang"))
        .map_err(|e| format!("reading lang: {e}"))?
        .filter_map(|entry| Some(entry.ok()?.path().join("LC_MESSAGES/cachemire.po")))
        .filter(|po| po.is_file())
        .collect();
    languages.sort();
    for po in languages {
        merge(&pot, &po)?;
    }
    Ok(())
}

/// Rewrites `po` with the messages of `pot`, in its order: a message already
/// there keeps its translation, a new one is added untranslated, one no longer in `pot` goes.
fn merge(pot: &Path, po: &Path) -> Result<()> {
    let load = |path: &Path| pofile(path.to_string_lossy().as_ref()).map_err(|e| format!("{}: {e:?}", path.display()));
    let template = load(pot)?;
    let mut file = load(po)?;
    let key = |e: &POEntry| (e.msgctxt.clone(), e.msgid.clone());
    let mut old: HashMap<_, _> = file.entries.drain(..).filter(|e| !e.obsolete).map(|e| (key(&e), e)).collect();
    file.entries = template
        .entries
        .iter()
        .filter(|t| !t.obsolete)
        .map(|t| match old.remove(&key(t)) {
            Some(kept) => POEntry { occurrences: t.occurrences.clone(), comment: t.comment.clone(), ..kept },
            None => t.clone(),
        })
        .collect();
    file.save(po.to_string_lossy().as_ref());

    let untranslated: Vec<&POEntry> = file.untranslated_entries();
    println!("{}: {} translated, {} untranslated", po.display(), file.translated_entries().len(), untranslated.len());
    for entry in untranslated {
        println!("  - {}", entry.msgid);
    }
    Ok(())
}
