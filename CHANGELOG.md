# Changelog

Every stable release of Cachemire; `Unreleased` is what the next one brings. Betas aren't listed: their notes are on the [releases page](https://github.com/iRed4321/Cachemire/releases).

## Unreleased

### Added

- **Settings**: the window now lists its categories on the left (general, key bindings, SSH profiles, connection profiles, experimental, about) and shows the one picked beside them. The title bar's **i** button opens its About page, which replaces the separate info window. A search box above the categories narrows them, and their settings, as you type
- **Key bindings** (Settings): lists every keyboard shortcut, and the app's own (zoom, search, close the tab, run a query, save a value...) can be given other keys. Copy, paste, undo and Esc stay as they are
- **New tabs open in** (Settings, General): a key's new tab starts in the fields view (as before), the table view, or the view you last picked
- **Links**: the link button next to a key's TTL copies a `cachemire://` link to it, with its filter but never a login or password. Opening one goes to that key on the right connection, saved or AWS, and offers to add a server you haven't saved; the Windows installer registers these links. An opened field has one too, next to its edit button: its link opens the key on that field

### Fixes

- **Query**: the completion list is drawn over the results' buttons below the box, no longer mixed with them

## 3.2.0 - 2026-11-02

### Added

- **Value editor**: a row's pencil button edits its value as JSON, with line numbers and colors, then saves it to Redis. The **+** button adds a field to a hash, and a field can be renamed or deleted
- **Keyspace**: right-click a key or folder to rename or delete it, or the empty space to add a key. A connection's open folders come back when you return to it
- **Table view**: click a cell and move with the arrow keys; **Ctrl+C** copies its value
- **Restore the last connection** (Settings, off by default): at startup, Cachemire reopens the connection it was closed on, with its tabs and folders
- **Large values**: hash fields are shown up to 4 MB (was 128 KB), and strings over 10,000 bytes are shortened on screen; bigger values can still be opened, saved or copied whole

### Fixes

- **App**: a second launch says Cachemire is already open; a failed task no longer leaves a dialog or button stuck busy
- **Connection**: recovers from dropped SSH tunnels and silent disconnects, waits through a slow keyring prompt, and handles failed switches cleanly
- **Export**: unreadable keys are skipped and counted, folder exports never overwrite existing files, and failed copies or saves are reported
- **Keyspace**: keys your Redis user can't read no longer stop the tree or a search; results no longer list keys twice
- **Performance**: exports and searches no longer stall Redis on big keys or strings; big keyspaces and hashes don't freeze the window
- **Query**: big joins no longer run out of memory, rows no longer repeat, and re-run or closed queries stop reading Redis
- **Query**: `'it''s'` finds it's, a result capped at 5,000 rows says so, and completion no longer offers `OR`
- **Saved data**: an unreadable settings or connections file is never overwritten, and settings saved by a newer version are kept
- **Security**: data files, exports and opened values are readable by your user only; CSV exports can't trigger spreadsheet formulas
- **SSH profiles**: a changed password is now kept after restarting
- **Table view**: shows every shared key, copies exactly the ticked rows, and keeps updating during a search
- **Updates**: a corrupted installer download is caught before installing, and the administrator prompt comes up sooner
- **Values**: long values no longer crash the app, nor show the previous key's data after switching; truncated streams say so
- **Window**: scrollbars are easier to grab, and on Windows moving or resizing no longer stops working until a restart

## 3.1.3 - 2026-11-02

### Fixes

- **Dependencies**: the libraries it's built on are updated (aws-sdk-elasticache, aws-sdk-sso, aws-sdk-ssooidc and 26 more)

## 3.1.2 - 2026-10-27

### Fixes

- **Dependencies**: the libraries it's built on are updated (aws-config, aws-credential-types, aws-runtime and 68 more)
- **Security**: crypto-bigint and ctutils updated, fixing RUSTSEC-2026-0385 and RUSTSEC-2026-0387

## 3.1.1 - 2026-10-22

### Fixes

- **Security**: tokio updated, fixing RUSTSEC-2026-0384

## 3.1.0 - 2026-10-22

### Added

- **Export** of a single key or folder now asks where to save the file and lets you rename it, with the usual name filled in
- **Check for Updates at startup** (Settings, off by default): when a newer version is out on the channel you picked, the info window opens at startup to offer it, with a one-click install on Windows and a link to the release page on Linux
- **AWS**: the SSO sign-in is remembered between launches and shared with the AWS CLI (in `~/.aws/sso/cache`), so the browser only opens again once it has run out; it also uses the CLI's current browser sign-in, which comes back to Cachemire by itself
- **Table view**: every column holding plain text, numbers or booleans has a small triangle in its title: click it to sort the rows in natural order (`item2` before `item10`), click again to flip between ascending and descending; on a hash, the Field column's triangle flips the order of the whole hash, as in the field list
- **Hints** on every icon button, saying what it does when hovered

### Fixes

- **Appearance**: the logo shown when no key is open now takes the connection profile's colors
- **AWS**: browser sign-in no longer fails with "No access" when the first profile's role isn't granted to you
- **Connection**: IPv6 addresses (`redis://[::1]:6379`) now work, directly or through an SSH tunnel
- **Export**: on Windows, keys differing only by case (`User`, `user`) no longer overwrite each other
- **Export**: a key deleted during an export no longer leaves an empty file
- **Query**: `alias.` completion now finds the right key in queries holding letters like `ẞ` or `İ`
- **Query**: a second `WHERE`, `SELECT` or `LIMIT` is now an error instead of being silently dropped
- **Search**: typing an unclosed `[` in the key tree's search no longer crashes the app
- **Search**: find-in-value and match highlighting now ignore case on lines holding `İ` or `ẞ`
- **Security**: TLS library behind `rediss://` and AWS updated, fixing four vulnerabilities (RUSTSEC-2026-0346 to 0349)
- **Security**: TLS checks behind `rediss://` and AWS no longer trust distrusted intermediate certificates (RUSTSEC-2026-0381, RUSTSEC-2026-0382)
- **Updates**: the update check falls back to the releases feed when GitHub's API answers with a 403 error
- **Window**: on Windows, the taskbar's hover preview now shows the Cachemire icon
- **Window**: the window can no longer shrink below 720 px, where its layout stops fitting
- **Window**: under 960 px wide, the title bar's connection pill shrinks to the connection's name
- **Window**: when maximized, the screen edges no longer resize the window, so the top-right corner hits Close

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
