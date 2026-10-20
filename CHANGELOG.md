# Changelog

Every stable release of Cachemire; `Unreleased` is what the next one brings. Betas aren't listed: their notes are on the [releases page](https://github.com/iRed4321/Cachemire/releases).

## Unreleased

### Added

- **Check for Updates at startup** (Settings, off by default): when a newer version is out on the channel you picked, the info window opens at startup to offer it, with a one-click install on Windows and a link to the release page on Linux
- **AWS**: the SSO sign-in is remembered between launches and shared with the AWS CLI (in `~/.aws/sso/cache`), so the browser only opens again once it has run out; it also uses the CLI's current browser sign-in, which comes back to Cachemire by itself
- **Hints** on every icon button, saying what it does when hovered

### Fixes

- **Windows**: the window now shows the Cachemire icon in the taskbar's hover preview
- The window can no longer be made narrower than 720 px, where its layout stops fitting (Windows let it shrink further)
- In a window narrower than 960 px, the connection pill of the title bar is a third as wide and shows only the connection's name, so it leaves room for the buttons around it
- Typing a `[` without its closing `]` in the key tree's search no longer crashes the app
- The query box's completion of `alias.` fields now finds the right key when a query holds letters like `ẞ` or `İ`
- **Windows**: exporting keys whose names differ only by case (`User` and `user`) no longer writes one over the other
- A Redisql query with a second `WHERE`, `SELECT` or `LIMIT` is now an error, instead of the first one being silently dropped
- An export no longer leaves an empty file for a key deleted before it could be read
- Connections to an IPv6 address (`redis://[::1]:6379`) now work, directly or through an SSH tunnel
- Finding text in a value, and the highlighting of search matches, now ignore case on lines holding letters such as `İ` or `ẞ` too
- **AWS**: signing in through the browser no longer fails with "No access" when the first profile's role isn't granted to you: the sign-in succeeds, and only picking that profile reports it, naming the profile, role and account
- Checking for updates no longer fails with a 403 error when GitHub's API limits requests from your network; the update check falls back to the releases feed
- **Security**: the TLS library behind `rediss://` connections and AWS is updated, fixing four vulnerabilities (RUSTSEC-2026-0346 to 0349)

## 3.0.0 - 2026-10-15

The first public release of Cachemire! Versions 1 and 2 were internal tools, never published. 3.0.0 is a complete rewrite in Rust: a fast, native app for Windows and Linux, polished to be shared.

Cachemire is made for Redis hashes full of JSON: read each value as colored JSON, filter them all with a JSON path, and lay the matches out as a table.

### Browse and read

- **Key tree** built from the `:` in key names, usable right away while it loads in the background, searchable with plain text or Redis globs
- **Every Redis type**: string, hash, list, set, sorted set and stream, with the type and TTL in the key header. Big hashes load page by page as you scroll
- **Colored JSON**, one line per field while collapsed and pretty-printed when expanded. Copy, open or download any value
- **Field search** by name, value or both, and find-in-value inside an open field
- **Large values stay light**: a field over 128 KB shows its size instead of being downloaded, and is read only when you open, save or copy it

### Filter and tabulate

- **JSON path filter** (`records.*.name`, `..label`), completed from the loaded values
- **Table view** with resizable columns; copy the selected rows as a JSON array, or export to CSV

### Search and query across keys

- **Instance-wide search** through every key's name, field names and values
- **Redisql**, a small SQL-like query language to pull rows out of JSON keys and join them, with error marks, completion, run history and saved queries

### Connect

- `redis://` and `rediss://` (TLS) connection URLs, tested before saving
- **SSH tunnels** with a password, a private key or both, and remembered host keys
- **AWS**: sign in with your `~/.aws` profiles (SSO included) and list the ElastiCache endpoints
- **Connection profiles** that tint the app with a color, so you always know you're on production
- Passwords kept in the OS credential store; import and export of connections as JSON

### And also

- Tabs for keys, searches and queries, each remembering its filters and scroll position
- English and French, switched without a restart
- Updates checked from the app, installed in one click on Windows

### Known limits

- Cluster mode isn't supported yet: such a server is refused with a clear message
- Cachemire only reads: editing keys and values is planned

