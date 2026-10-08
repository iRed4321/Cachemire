<div align="center">

**English** · [Français](README.fr.md)

<img src="assets/icon.svg" alt="Cachemire" width="128" height="128">

# Cachemire

**Knit-pick your cache.**

A fast, native Redis key explorer. Browse your keys as a tree, read values as colored JSON, filter them with a JSON path and see them as a table. Reach servers behind an SSH bastion, with every password kept in your OS keyring.

Rust · [Slint](https://slint.dev) · Windows and Linux

</div>

---

> **Opinionated by design.** Cachemire is built around one use case: hashes whose values are JSON. That is where its filters, tables and Redisql shine; other types are readable, but get less attention.

## Features

### Browse and read

- 🌳 **Key tree** built from the `:` in key names (`app:users:42` becomes `app` › `users` › `42`). It loads in the background, so it is usable right away and fills in as it goes. Search it with plain text or Redis globs (`user:*`).
- 📦 **Every Redis type**: string, hash, list, set, sorted set and stream, with the type and TTL in the key header. Big hashes load page by page as you scroll.
- 🎨 **Colored JSON**: one line per field while collapsed, pretty-printed when expanded. Select and copy text, or copy, open and download a value as JSON.
- 🔍 **Field search**: match fields by name, value or both, and find text inside an open value.

### Filter and tabulate

Type a JSON path in the filter bar and press <kbd>Enter</kbd>:

```
records                 one key
records.*.name          the name of every record
records[0].address.city an array index
..label                 a key at any depth
```

The path is completed from the loaded values. Show the result as a **field/value list** or a **table** with resizable columns, copy selected rows as a JSON array, or export to CSV.

### Search and query across keys

- 🧭 **Search** the whole instance: a word or glob checked against every key's name, field names and values.
- 🧮 **Redisql**, a small query language to pull rows out of keys that hold JSON and join them like SQL:

  ```
  FROM KEY 'orders:items' AS i
  JOIN KEY 'products' AS p ON i.productId = p.id
  WHERE i.qty IN [1, 2, 3]
  SELECT i.productId, p.label, i.qty
  LIMIT 100
  ```

  The editor marks errors in place, completes keywords, keys and `alias.field`, keeps your run history, and lets you save named queries. Results open like any key, so filters, views and exports all work on them.

### Tabs

Open keys, searches and queries in tabs. Drag them around; each remembers its filters and scroll position, and each connection keeps its own set.

### Connect

- **Connection URL**: `redis://user:pass@host:6379/0`, or `rediss://` for TLS. Test it before saving.
- 🔐 **SSH tunnels** with a password, a private key, or both. Host keys are remembered on first use and refused if they change. SSH profiles let you share one login across tunnels.
- 🎛️ **Connection profiles**: a name and a color that tints the app while connected, so you always know you're on `Production`.
- ☁️ **AWS**: sign in with your local `~/.aws` profiles (SSO included) and list their ElastiCache endpoints.
- 📥 **Import / Export** your connections as JSON, with or without passwords.

> Cluster mode isn't supported yet: such a server is refused with a clear message.

### Safe by default

- 🛡️ Passwords go to the OS credential store (Credential Manager or Secret Service).
- Nothing is ever written to Redis: this is a read-only explorer.

### Polish

Zoom with <kbd>Ctrl</kbd>+<kbd>+</kbd>/<kbd>-</kbd>/<kbd>0</kbd> or <kbd>Ctrl</kbd>+scroll. 🌍 **English and French**, following your system or chosen in **Settings**, switched without a restart.

## Getting started

You need a recent stable **Rust**. On Linux, you also need a running Secret Service and Wayland or X11.

```sh
cargo run --release
```

Add a connection from the title bar (**New connection…**), pick it, and browse. To try it locally: `docker run --rm -p 6379:6379 redis:7`, then add `redis://127.0.0.1:6379`.

| Command | For |
|---|---|
| `cargo run` | debug: quickest to compile, slowest to run |
| `cargo run --profile fast` | optimized, no LTO: rebuilds in seconds, good for trying the real thing |
| `cargo run --release` | LTO, stripped: the one to ship |

Installers are built with `cargo xtask build-windows` (MSI and portable zip), `cargo xtask build-deb` and `cargo xtask build-appimage` (or both from one build with `cargo xtask build-linux`).

## Your data

Saved in your user data folder (`%APPDATA%\Cachemire` or `~/.local/share/Cachemire`): your connections, settings, saved queries, SSH host keys and query history.

## About this project

Most of Cachemire's code was written by AI ([Claude Code](https://claude.com/claude-code)), under my direction and review.

## License

Cachemire is free software under the [GNU GPL v3](LICENSE). You can use it and fork it freely; if you redistribute a version, it must stay under the same license, with its source and credit to the original author.

Bundled fonts: [Roboto](assets/fonts/Roboto-OFL.txt) and [JetBrains Mono](assets/fonts/JetBrainsMono-OFL.txt), under the SIL Open Font License.
