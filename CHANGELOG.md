# Changelog

Every stable release of Cachemire; `Unreleased` is what the next one brings. Betas aren't listed: their notes are on the [releases page](https://github.com/iRed4321/Cachemire/releases).

## Unreleased

### Added

- **Editing**: an open row's pencil button opens its value in an editor with line numbers, colored as you type; **Format JSON** re-indents it, and **Save** writes it back to Redis once it's valid JSON (hash fields and string keys; a click outside cancels). A hash field can also be renamed, to a name the hash doesn't have yet, or deleted. Values that aren't UTF-8 text stay read-only
- **Table view**: a click on a cell outlines it in blue, and the arrow keys move it from cell to cell, the table scrolling to keep it in view; **Ctrl+C** copies the outlined cell's whole value
- **New field**: the **+** button left of a hash's export buttons opens the value editor on a new field, written once it has a name and a valid JSON value
- **Keyspace**: right-click a key or folder to **Rename** or **Delete** it (a folder with every key under it); right-click the empty space, or a row, for **Add key…**, which makes an empty hash under the name you type (a `:` puts it in that folder) and opens its tab. Redis stores it once its first field is added
- **Restore the last connection** (Settings, off by default): at startup, Cachemire reconnects to the connection it was closed on, with its tabs and open folders; an AWS connection signs in again first, in the browser if its session has expired. It's saved every 30 seconds too, so a crash doesn't lose it
- **Keyspace**: going back to a connection reopens the folders it had open, as it already did for its tabs
- **Long strings**: a string over 10,000 bytes inside a value is shown as `[REDACTED: string too long]` in its own color; opening, saving or copying the value still gives it whole
- **Large values**: a hash field is now shown up to 4 MB (was 128 KB); past that, it shows its size with buttons to open, save or copy it, and **Try to load** to read and show it


### Fixes

- **App**: a second launch now says Cachemire is already open, so two windows can't overwrite each other's saved settings
- **App**: a task that fails unexpectedly no longer leaves its dialog or button stuck busy
- **Connection**: taking your time at the keyring's password prompt no longer makes the connection time out
- **Connection**: an SSH tunnel dropped by sleep or a network change is opened again by itself on the next action
- **Connection**: a connection that died silently is noticed sooner, and the refresh button now waits for it to reopen
- **Connection**: opening a key stays quick while the tree loads, a search runs or an export is under way
- **Connection**: after a failed switch, the refresh button retries the connection picked, not the previous one
- **Connection**: the memory figure and query completion now follow the connection in use, even after a failed switch
- **Export**: big keys are read in chunks, so an export no longer stalls the Redis server for its other clients
- **Export**: a key that can't be read is left out and counted, instead of stopping the export with a broken file
- **Export**: a copy, open or save that fails now says so, instead of silently doing nothing
- **Export**: a folder export no longer replaces files already in the chosen folder: it numbers its own
- **Fields and table views**: the scrollbars are easier to grab, this helps navigating horizontally
- **Keyspace**: a key your Redis user isn't allowed to read no longer stops the tree or a search part way
- **Keyspace**: the tree stays responsive while a large keyspace loads, even with thousands of keys in one folder
- **Query**: a hash read while Redis resizes it no longer gives some of its rows twice
- **Query**: a join matching many rows no longer runs out of memory: it stops once past the limit
- **Query**: a query run again, or whose tab closes, stops reading from Redis instead of finishing in the background
- **Query**: a quote written twice inside a string stands for itself: `'it''s'` finds it's
- **Query**: a query capped at 5,000 rows now says so, instead of asking to raise LIMIT
- **Query**: completion no longer offers `OR`, which queries don't support
- **Saved data**: a connections or settings file that can't be read is never overwritten, even when it can't be set aside
- **Saved data**: settings a newer version saved are kept when an older version saves the file
- **Search**: only a string's first megabyte is searched, so a huge string no longer holds the Redis server up
- **Search**: a key no longer appears twice in the results, nor counts twice in the totals
- **Security**: data files, connection exports and values opened in an editor are now readable by your user only
- **Security**: CSV exports guard cells starting with `=`, `+`, `-` or `@`, so spreadsheets don't run them as formulas
- **SSH profiles**: a new password typed for a profile is now kept after restarting, instead of the old one returning
- **Table view**: shows every key the rows share, not just the first 30, judged on the first 100 rows
- **Table view**: copying or exporting ticked rows gives exactly those rows, also while searching or next to values over 4 MB
- **Table view**: keeps updating while a search reads through a big hash, instead of waiting for the end
- **Updates**: an interrupted or corrupted installer download is now caught before installing, instead of running a broken file
- **Updates**: the installer no longer makes a System Restore point first, so the administrator prompt comes up sooner
- **Values**: opening a value of thousands of lines no longer crashes the app
- **Values**: a stream with more entries than shown is now marked as cut short, in the key panel and in exports
- **Values**: switching keys mid-load or mid-scroll no longer mixes the previous key's fields, text or exports into the new one
- **Values**: scrolling through a big hash no longer makes the window stutter while each page is prepared
- **Window**: on Windows, moving or resizing the window no longer stops working until a restart

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
