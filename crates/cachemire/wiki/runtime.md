# Runtime and threads

Slint owns the main thread and is not async, so every Redis, disk or network operation runs
elsewhere and its result comes back to the main thread to be shown. This chapter lists the threads,
the thread-local [`App`](crate::app::App), the helpers that cross between threads, and the rules
about blocking.

Where to look:

- `crates/cachemire/src/app.rs`: `App`, [`install`](crate::app::install), [`with`](crate::app::with),
  [`later`](crate::app::later), [`spawn`](crate::app::spawn), [`spawn_blocking`](crate::app::spawn_blocking).
- `crates/cachemire-core/src/runtime.rs`: [`runtime::spawn`](cachemire_core::runtime::spawn).

## Threads at a glance

| Thread | Created by | Runs | May touch Slint? |
|---|---|---|---|
| **UI thread** (main) | the process | the Slint event loop, every callback, every `later` closure, timers | yes, only here |
| **`redis-io`** | [`runtime::spawn`](cachemire_core::runtime::spawn) | hosts the Tokio runtime; sits in `block_on(pending())` forever | no |
| Tokio **workers** (2) | that runtime | async tasks: Redis commands, SSH tunnels, HTTP, polling loops | no |
| Tokio **blocking pool** (max 8) | that runtime | `spawn_blocking`: keyring access, file writes, parsing and pretty-printing values | no |
| **rayon** pool | rayon's global pool | data-parallel work over fields, called from blocking tasks | no |

The Tokio runtime is built with `worker_threads(2)` and `max_blocking_threads(8)`: Redis I/O is
network-bound so two async workers suffice, and the blocking pool is capped instead of growing to
Tokio's default of 512 threads. A consequence: at most eight blocking jobs run at once, so a long
wait on the blocking pool (for example a keyring prompt) occupies one of eight slots.

The handle returned by [`runtime::spawn`](cachemire_core::runtime::spawn) is a Tokio `Handle`. It is
cloned freely: [`App::rt`](crate::app::App::rt) holds one, and feature contexts keep their own copy.
The runtime lives for the whole process (the thread never ends).

Rayon is a dependency of the **app** crate only (search, the key panel's table, fields and value
lists use `par_iter`); the core does not use it. Where it is called from is not uniform: the code
this wiki was written from calls it inside work already moved off the UI thread.

## The thread-local `App`

[`App`](crate::app::App) gathers the UI thread's state in one struct (see
[state](crate::wiki::state) for its fields). It is stored in a `thread_local!` `OnceCell<Rc<App>>`:

```text
 main thread only
   thread_local APP: OnceCell<Rc<App>>
        ^ install(app)   once, in main, before window.run()
        | with(f)        f(&Rc<App>, &MainWindow)  if the window is still open
```

Why a thread local rather than a global or an `Arc`: `App` holds `RefCell`s and Slint handles,
which are not `Send`. Keeping it in a thread local means the type system itself forbids sending it
to a Tokio task: closures that run on Tokio must be `Send`, so they cannot capture it, and they get
it again only through [`later`](crate::app::later).

## Crossing threads

| Helper | Called from | Runs the closure on | Use |
|---|---|---|---|
| [`app::with(f)`](crate::app::with) | UI thread | the UI thread, now | reach `App` and the window from a Slint callback |
| [`app::later(f)`](crate::app::later) | any thread | the UI thread, when the event loop gets to it | hand a result back; `f` must be `Send`; dropped if the loop ended |
| [`app::spawn(rt, task, then)`](crate::app::spawn) | UI thread | `task` on Tokio, `then` on the UI thread | the standard "do async work, then show it" |
| [`app::spawn_blocking(rt, work, then)`](crate::app::spawn_blocking) | UI thread | `work` on the blocking pool, `then` on the UI thread | the same for blocking work |

Details that matter:

- `later` is `slint::invoke_from_event_loop` wrapped around `with`, so a closure that runs after the
  window closed does nothing.
- `spawn` returns a Tokio `AbortHandle`. It spawns the task, then a second task that awaits it. A
  panic of the task reaches `then` as `Err(Error::System(SystemError::Task(..)))`
  (via [`Error::from`](cachemire_core::error::Error)). **Aborting the task skips `then`
  entirely**: callers use this to cancel.
- `spawn_blocking` catches panics itself (`catch_unwind`) and reports them as
  [`SystemError::Panicked`](cachemire_core::error::SystemError::Panicked) with the panic message when it is a string.
- `then` receives `Result<T, Error>` where the `Error` is only the *runtime's* failure (panic).
  The task's own result is usually itself a `Result`, hence the `.flatten()` seen in callers.
- Some modules call `slint::invoke_from_event_loop` or `Weak::upgrade_in_event_loop` directly
  (the key panel's table builds, the keyspace walk, `server_stats`). They capture a
  `slint::Weak<MainWindow>` and plain data instead of going through `App`.

## Plain-data contexts

Long-running features cannot hold `App`, so each keeps a small `Clone` context of `Send` pieces:

| Context | Holds | In |
|---|---|---|
| `ScanContext` | `Weak<MainWindow>`, `Arc<AppState>`, runtime handle, `Arc<Mutex<Tree>>`, generation counter, export-cancel flag | `keyspace/mod.rs` |
| `Ctx` (key panel) | `Weak<MainWindow>`, `Arc<AppState>`, runtime handle, shared fetched data | `key_detail/mod.rs` |

The tree is `Arc<Mutex<Tree>>` with *plain data only*: the comment in the source says that mutation
"still only happens on the UI thread". The mutex is there to satisfy `Send`, not to arbitrate real
contention. Slint models (`VecModel`, the tree's row model) are touched on the UI thread only.

## Rules about blocking

1. **Never block the UI thread** on network, disk-heavy work, the keyring or a long parse. Use
   [`app::spawn`](crate::app::spawn) for async work and
   [`app::spawn_blocking`](crate::app::spawn_blocking) (or `rt.spawn_blocking`) for blocking work.
2. **Never `.await` or `block_on` from a Slint callback.** The runtime is the only place to await.
3. **Keyring access is blocking and may wait for the user** (an unlock prompt). The core does it
   inside `spawn_blocking`, outside the store lock (see
   [`staged`](cachemire_core::secret_vault::staged)). The connection timeout starts only after it:
   the comment in `connect_then` says so.
4. **Heavy formatting leaves the event loop.** Parsing, colouring and pretty-printing a value run on
   a blocking thread; the UI-thread closure only wraps models and sets properties.
5. **Locks are held briefly.** The stores in [`AppState`](cachemire_core::state::AppState) are
   `parking_lot::Mutex`es (no poisoning). Handlers lock, read or change, and release. Note that
   store setters persist to disk while the lock is held, and several handlers call them directly on
   the UI thread (for example saving a preference): small JSON writes are accepted there, large or
   slow work is not.
6. **Check that an answer is still wanted** before applying it (see "tickets" in
   [state](crate::wiki::state)).

## Other threads and timers

- `slint::Timer`s run on the UI thread: [`last_session::keep_saved`](crate::last_session::keep_saved)
  saves every 30 seconds.
- [`server_stats::init`](crate::server_stats::init) spawns a Tokio loop that polls memory use every
  3 seconds and uses `upgrade_in_event_loop` to update `ConnectionState` only when the text changed.
- The link inbox ([`links::watch`](crate::links)) feeds links from other launches into the UI thread.
- `thread_local!` is also used for small UI-thread-only values: the pointer position in `toast.rs`,
  the written session in `last_session.rs`, plus state in `shortcuts.rs`, `links/mod.rs` and
  `updates.rs`.
