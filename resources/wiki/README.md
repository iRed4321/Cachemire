# Assets of the wiki (`cargo wiki`)

- `header.html`: put in the head of every page by `--html-in-header`; it draws the `mermaid` code blocks in the browser.
- `mermaid.min.js`: Mermaid 11.17.2 (MIT, https://github.com/mermaid-js/mermaid), taken from
  https://cdn.jsdelivr.net/npm/mermaid@11.17.2/dist/mermaid.min.js, sha256 581ed7d74bd9048d0e3a91363927d72ef22942d7722546b27f7cc29e35390eb8.
  `cargo wiki` copies it into `target/doc`. To update it, change the version in that URL and in this file.

`dot` code blocks need Graphviz (`dot`) installed: `cargo wiki` renders them to SVG in the pages.

`cargo wiki` clears `target/doc` first, so a changed `header.html` or diagram renderer always shows.
