# Structured file persistence (experimental)

Framework apps can own cached JSON, TOML, and Lua-table documents through
`riced.file`. Construction, getters, and setters do no filesystem work.
Reloads and saves run on the managed background executor; completion updates
the cache and then invokes the callback on the host update thread.

## Basic use

```lua
function app:mount()
    self.settings = riced.file({
        type = "toml",
        path = "state/preferences.toml", -- relative to this app; absolute paths also work
        create_parents = true, -- save may create missing parent directories
        defaults = {
            appearance = { opacity = 0.9 },
            enabled = true,
        },
    })

    self.settings:reload(function(result)
        if not result.ok then
            self.error = result.error.message
        end
    end)
end

function app:actions(event)
    local status = self.settings:status()
    if event.key == "save" and status.loaded and not status.busy then
        self.settings:set({ "appearance", "opacity" }, 0.8)
        self.settings:save(function(result)
            self.error = not result.ok and result.error.message or nil
        end)
    end
end
```

Use these methods in framework lifecycle handlers or managed callbacks, not
in `view()` or top-level module evaluation. Views may call `get()` and `status()`.
Callbacks run with the owning app's normal context and execution budgets and
automatically refresh its Lua view. Callbacks are optional for both I/O methods;
cache updates still happen when no callback is supplied.

## API

| Operation | Behavior |
| --- | --- |
| `riced.file({type, path, defaults?, create_parents?, write_mode?})` | Creates a handle with an in-memory defaults snapshot. Unknown options fail. |
| `file:get()` | Returns a detached copy of the whole document. |
| `file:get("key")` | Returns a detached value, or `nil` when missing. Dots are literal key characters. |
| `file:get({"appearance", "opacity"})` | Traverses nested maps. |
| `file:set(key, value)` | Sets/replaces a value; returns whether data changed. Creates missing intermediate maps, but never replaces a scalar intermediate implicitly. |
| `file:remove(key)` | Deletes a key; returns whether data changed. |
| `file:status()` | Returns `path`, `type`, `revision`, `saved_revision`, `loaded`, `dirty`, `busy`, and `watching`. |
| `file:reload([options][, callback])` | Loads, parses, merges defaults, and applies a new snapshot asynchronously. Returns a cancellation token. |
| `file:save([callback])` | Saves the captured revision asynchronously. Returns a cancellation token. |
| `file:watch([options], callback)` | Watches content changes through managed background scans. Returns a cancellation token. |
| `file:close()` | Stops its watcher and releases the handle. Explicit reload/save must finish or be cancelled first. Subsequent access fails. |

Key paths are a string or a dense array of 1–32 string segments. Array elements
are edited by replacing the complete array, not by numerical key paths.
Mutating the table returned by `get()` does **not** edit the document; use `set()`.
`set(key, nil)` is invalid; use `remove()` instead.

One I/O operation may be pending per handle. A second reload/save fails immediately
while `busy` is true; setters remain available so edits can continue during saves.
For a retryable UI, disable save/reload controls while the handle is busy.

## Data and format rules

All document roots must be string-keyed maps. Nested values may be maps, dense
arrays, strings, booleans, signed 64-bit integers, or finite floating-point
numbers. Null, sparse/mixed Lua tables, cyclic references, functions, userdata,
threads, non-finite numbers, and TOML datetime values are unsupported. Store
datetimes as quoted strings.

An ordinary empty Lua table means an empty map. Use `riced.array({})` for an
explicit empty array. Detached arrays returned by `get()` retain their array tag.
Generated Lua files use an `array({...})` helper supplied by the restricted loader
to preserve arrays, including empty ones.

Defaults merge recursively into missing map keys. Arrays and scalar values replace
rather than merge element-by-element. A missing file exposes the defaults, reports
`missing = true`, and does not create a file. Saving writes the complete resolved
document. Removing a defaulted key is not a deletion tombstone: the default returns
when the file is reloaded and the key is still absent.

JSON and TOML writes generate a new document; comments and formatting are not
preserved. Lua reads execute in a separate, instruction- and memory-bounded config
VM without filesystem access, shell execution, or module loaders. Lua writes are
disabled unless explicitly enabled:

```lua
self.preferences = riced.file({
    type = "lua",
    path = "preferences.lua",
    write_mode = "generated",
})
```

Generated Lua replaces executable expressions, comments, and formatting with a
data-only table. It is not a Lua source editor. Use a dedicated data file, not
`app.lua` or a component module. App code reload watches `app.lua` and
`components/**/*.lua`; ordinary generated preference files outside those locations
do not reload the application.

## Revisions, conflicts, and failures

A handle must successfully reload before its first save, including when the file
does not exist. This prevents blind overwrites of an existing document.

Saves capture a particular revision. If the cache changes while the save runs,
the saved revision advances only to the captured revision and `dirty` remains
true. Reloads never silently discard unsaved changes. An intentional discard is:

```lua
self.settings:reload({ discard = true }, function(result)
    -- Handle result.ok and result.error.
end)
```

Even `discard = true` cannot discard edits made **after** that reload started;
the reload then returns a `dirty` error and leaves the current cache untouched.
Parse errors and I/O errors preserve the last working cache.

Before a save, the worker compares the complete bounded prior contents against
the current file, not just timestamps or decoded values. External changes,
including formatting-only changes or deletion, return `conflict`. Reload and
review/discard intentionally rather than blindly retrying the save.

Operations on the same canonical-parent/file path serialize within this process.
A save writes a temporary sibling, flushes and syncs it, rechecks the baseline,
then atomically replaces the destination. Existing file permissions are preserved;
new files use the temporary-file library's private permissions. There is no
universal compare-and-swap guarantee against an unrelated external writer racing
the final comparison and rename, nor a guarantee of directory-entry durability
across power loss. Parent creation defaults to false.

Only regular UTF-8 files are supported. Final-component symlinks, directories,
and special files are rejected. Parent directories may resolve through symlinks.
Absolute paths are permitted; this is a trusted-app convenience API, **not** a
filesystem sandbox or capability boundary.

Completion results contain `ok`, operation metadata, current `revision`,
`saved_revision`, and `dirty`. Successful reloads also contain a detached `data`
snapshot and `missing`. Errors use `{ code, message }`; important codes include
`parse`, `value`, `limit`, `io`, `path`, `conflict`, `dirty`, `cancelled`, and `worker`.
Bad arguments, forbidden phases, busy handles, and saving before reload raise a
Lua error immediately rather than scheduling work.

## Cancellation and bounds

Cancel an operation with the existing managed token API:

```lua
self:cancel(token)
```

Cancellation is silent. Cancelling an explicit reload/save releases the handle's
busy state and marks the disk baseline unknown, so reload again before saving.
Cancelling a watcher stops its scans without invalidating loaded data.
Cancellation and app retirement
discard callback delivery, but a blocking filesystem call cannot be interrupted
and an already committed save cannot be rolled back. Workers check cancellation
before I/O and before replacement. Do not depend on shutdown hooks to flush changes.

- Maximum 16 open handles per app VM; call `close()` to release unused handles.
- Maximum 1 MiB for input, portable documents, and serialized output.
- Maximum nesting depth 32 and 16384 document nodes.
- At most four active blocking file workers across the process.
- Pending callbacks also count toward the existing managed-resource limits.

Each app/output instance owns its handles independently. The same path still
refers to the same physical file: choose distinct paths for independent output
preferences, or deliberately handle conflicts when sharing one file.

## File watching

```lua
function app:mount()
    self.settings = riced.file({
        type = "json",
        path = "preferences.json",
        defaults = { opacity = 0.9 },
    })
    self.settings_watch = self.settings:watch({ interval_ms = 500 }, function(event)
        if event.ok then
            self.error = nil
            self.opacity = self.settings:get("opacity")
        else
            self.error = event.error.message
        end
    end)
end
```

`watch(callback)` uses the default 500 ms interval. `interval_ms` must be an
integer in `500..86400000`; unknown options fail. One watcher is allowed per
handle. It counts toward the normal managed callback limit and starts only after
candidate validation, just like other managed resources.

If the handle has not loaded, its first scan initializes a clean cache and
delivers `loaded` (including defaults for a missing file). If it is already
loaded, an unchanged first scan is silent. Event fields include `ok`,
`type = "file"`, `operation = "watch"`, `event`, `path`, `revision`,
`saved_revision`, and `dirty`. Success events include detached `data`; `missing`
is included when known. Failures include `error = { code, message }`.

| Event | Behavior |
| --- | --- |
| `loaded` | First successful initialization of a previously unloaded, clean handle. |
| `changed` | New readable, valid contents applied to a clean cache. |
| `deleted` | A clean, previously loaded file disappeared; defaults are applied without recreating it. |
| `conflict` | External contents or deletion would overwrite unsaved edits. Cache and save baseline remain intact. |
| `error` | Read, parse, validation, or worker failure. The last working cache is preserved. |
| `recovered` | A transient read failure cleared and the contents are otherwise unchanged. |

Identical contents are silent. Identical recurring errors are coalesced until
the observed state changes. Invalid Lua contents are not repeatedly executed
when unchanged. Corrected contents are retried automatically. Recovery can still
report a content error or conflict if those problems remain after an I/O failure.

Clean documents reload automatically. Dirty documents require an explicit choice:
review the external change, or use `reload({discard=true})`. A watcher never
automatically saves, recreates a deleted file, or discards local edits. Successful
explicit saves/reloads update the watch baseline, avoiding self-write feedback.
Scans started before a newer local edit, save, or reload cannot apply stale data.

The implementation uses bounded content polling, not platform filesystem events.
It follows editor rename-and-replace saves and parent directories that disappear
and return. Checks are scheduled on the existing 500 ms host cadence, with the
next interval measured after completion; UI load and filesystem latency can add
delay. There is at most one scan in flight per watcher, so slow delivery does not
accumulate a backlog. Changes between checks coalesce to the observed snapshot;
this is not an event journal, and transient changes that revert between scans
may be invisible. Larger files are read in full for comparison; choose a longer
interval when appropriate.

All disk reads and parsing use the existing four-worker pool. Unchanged or stale
scans do not invoke Lua callbacks or reexecute Lua views. Watch callbacks run only
from the host update loop. `get()` and `status()` never poll the filesystem.

Use `self:cancel(self.settings_watch)` to stop watching. `file:close()` also cancels
its watcher, including a pending scan. App reload, removal, and teardown release
watchers automatically; late results cannot reach a replacement VM. Hidden apps
remain alive and keep watching until explicitly stopped or retired.

Property proxies such as `file.data.opacity` remain intentionally deferred so
mutations stay explicit and revision tracking remains reliable.
