//! `cargo wiki`: the docs of both crates and the wiki chapters. Their `mermaid` blocks are drawn in the
//! browser (resources/wiki), their `dot` blocks are rendered to SVG here, by Graphviz, into the pages.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::tools::{Result, cargo, repo_root, run, works};

const DOC_ARGS: [&str; 3] = ["doc", "--no-deps", "--document-private-items"];
const BLOCK_START: &str = r#"<div class="example-wrap"><pre class="language-dot"><code>"#;
const BLOCK_END: &str = "</code></pre></div>";

pub fn run_task(open: bool) -> Result<()> {
    let root = repo_root();
    let assets = root.join("resources/wiki");
    let doc = root.join("target/doc");
    if doc.exists() {
        std::fs::remove_dir_all(&doc).map_err(|e| format!("could not clear {}: {e}", doc.display()))?;
    }
    run(cargo().current_dir(&root).env("CARGO_ENCODED_RUSTDOCFLAGS", rustdoc_flags(&assets.join("header.html"))).args(DOC_ARGS))?;
    std::fs::copy(assets.join("mermaid.min.js"), doc.join("mermaid.min.js")).map_err(|e| format!("could not copy mermaid.min.js: {e}"))?;
    if works("dot", "-V") {
        let rendered = render_dot_blocks(&doc)?;
        println!("Rendered {rendered} Graphviz diagram(s).");
    } else {
        println!("warning: Graphviz (`dot`) is not installed: its diagrams stay as text.");
    }
    if open {
        run(cargo()
            .current_dir(&root)
            .env("CARGO_ENCODED_RUSTDOCFLAGS", rustdoc_flags(&assets.join("header.html")))
            .args(DOC_ARGS)
            .arg("--open"))?;
    }
    Ok(())
}

/// The rustdoc flags already set in the environment, then the page header that draws the Mermaid blocks.
fn rustdoc_flags(header: &Path) -> String {
    let mut flags: Vec<String> = match std::env::var("CARGO_ENCODED_RUSTDOCFLAGS") {
        Ok(encoded) => encoded.split('\x1f').map(str::to_string).collect(),
        Err(_) => std::env::var("RUSTDOCFLAGS").unwrap_or_default().split_whitespace().map(str::to_string).collect(),
    };
    flags.retain(|flag| !flag.is_empty());
    flags.extend(["--html-in-header".to_string(), header.display().to_string()]);
    flags.join("\x1f")
}

/// Replaces the `dot` code blocks of every page of `doc` with the SVG Graphviz draws from them; how many.
fn render_dot_blocks(doc: &Path) -> Result<usize> {
    let mut count = 0;
    for page in html_pages(doc)? {
        let Ok(html) = std::fs::read_to_string(&page) else { continue };
        if !html.contains(BLOCK_START) {
            continue;
        }
        let mut out = String::with_capacity(html.len());
        let mut rest = html.as_str();
        while let Some(start) = rest.find(BLOCK_START) {
            let body = &rest[start + BLOCK_START.len()..];
            let end = body.find(BLOCK_END).ok_or_else(|| format!("{}: a dot block is not closed", page.display()))?;
            let svg = dot_to_svg(&unescape(&body[..end])).map_err(|e| format!("{}: {e}", page.display()))?;
            out.push_str(&rest[..start]);
            out.push_str(&format!(r#"<div class="dot-diagram">{svg}</div>"#));
            rest = &body[end + BLOCK_END.len()..];
            count += 1;
        }
        out.push_str(rest);
        std::fs::write(&page, out).map_err(|e| format!("could not write {}: {e}", page.display()))?;
    }
    Ok(count)
}

/// Every `.html` page under `dir`, but the source pages (`src`) and the static files.
fn html_pages(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut pages = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(|e| format!("could not read {}: {e}", dir.display()))? {
        let path = entry.map_err(|e| e.to_string())?.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        if path.is_dir() && name != "src" && name != "static.files" {
            pages.extend(html_pages(&path)?);
        } else if path.extension().is_some_and(|ext| ext == "html") {
            pages.push(path);
        }
    }
    Ok(pages)
}

/// Graphviz's SVG for `source`, transparent, its lines and texts in the current text color (the theme's).
fn dot_to_svg(source: &str) -> Result<String> {
    let mut child = Command::new("dot")
        .args(["-Tsvg", "-Gbgcolor=transparent", "-Gfontname=sans-serif", "-Nfontname=sans-serif", "-Efontname=sans-serif"])
        .args(["-Gfontsize=10", "-Nfontsize=10", "-Efontsize=9", "-Nmargin=0.1,0.05"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not run dot: {e}"))?;
    child.stdin.take().ok_or("dot has no input")?.write_all(source.as_bytes()).map_err(|e| e.to_string())?;
    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(format!("dot failed on:\n{source}\n{}", String::from_utf8_lossy(&output.stderr).trim()));
    }
    let svg = String::from_utf8_lossy(&output.stdout);
    let svg = svg.find("<svg").map_or(&*svg, |start| &svg[start..]);
    Ok(svg.replace("\"black\"", "\"currentColor\"").replace("<text ", "<text fill=\"currentColor\" "))
}

/// The text of a code block, as rustdoc escaped it into HTML.
fn unescape(html: &str) -> String {
    html.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&#39;", "'").replace("&amp;", "&")
}
