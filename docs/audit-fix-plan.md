# Audit fix plan

Status: all six fixes implemented; automated shipping checks passed. Live Wayland
checks remain manual.

Scope: the six findings from the repository audit, including the current
uncommitted frontend changes. Keep their audit numbers for tracking.

## Rules for this work

- Preserve existing user changes and user-owned installed Lua files.
- Fix shared causes and update affected callers, fixtures, examples, and docs.
- Add a failing regression test before each fix; mark a task done only after
  its test passes.
- Reuse existing helpers and dependencies. No new framework, settings system,
  or unrelated cleanup.
- Update this checklist with results and any remaining limitations as work lands.

## Implementation order

### 1. Protect configuration edits during wallpaper Apply — must fix

- [x] Trace wallpaper commit and configuration-save callers before editing.
- [x] Separate change detection from configuration parsing. A changed, deleted,
  or unreadable file must block Apply, even if it cannot be parsed.
- [x] Use the existing baseline/conflict-checking approach where practical;
  recheck immediately before publication. Do not claim cross-process locking
  unless cooperating writers actually use it.
- [x] On conflict, preserve disk contents, the previous desktop, and its manifest;
  leave an actionable error in Settings.
- [x] Test valid and invalid external edits, deletion, and read failure during
  an Apply, plus the unchanged-file success case.

Primary files: `src/shell/wallpaper.rs`, `src/config/io.rs`,
`src/wallpaper/mod.rs`, and their existing tests in `src/shell/state.rs` and
`src/wallpaper/tests.rs`.

### 2. Preserve notification action keys — must fix

- [x] Guard a missing event key separately, then assign both `match` results
  directly in the root action handler.
- [x] Correct the same example in `README.md`; search other shipped handlers
  for this exact multiple-return mistake.
- [x] Test the actual bundled root handler with `invoke:42:open:details`:
  preserve the ID and complete action key, and decode a valid native command.
- [x] Check missing/malformed keys remain harmless and dismissal still works.

Primary files: `scripts/frontend/root.lua`, `README.md`,
`src/shell/apps/tests.rs`, and the existing notification action decoder.

### 3. Make bar add/remove honor output — must fix

- [x] Keep existing `data/bars/<edge>.lua` files as all-output bars.
- [x] Give output-specific bars a collision-free, path-safe filename identity
  and persist the original output name in their data. Never insert unchecked
  connector names into paths or delimiter-based Settings action keys.
- [x] Use the same identity for duplicate detection, creation, and removal.
  A targeted removal must never fall back to deleting an all-output bar.
- [x] Filter root layer declarations by saved output; keep disconnected-output
  files so reconnecting restores their bars.
- [x] Make Settings list, identify, edit, and remove these files without losing
  their output field. Do not add a new output-picker UI for this fix.
- [x] Test two monitors, all-output compatibility, targeted add/remove,
  duplicate requests, reconnects, and output names requiring safe encoding.
- [x] Update command examples and compatibility notes.

Primary files: `scripts/frontend/root.lua`, `scripts/frontend/state.lua`,
`scripts/settings/panel.lua`, `README.md`, and existing frontend/Settings tests.
Inspect example project copies and update any affected equivalents.

### 4. Release D-Bus ownership when notifications stop — must fix

- [x] Tie the shared signal-emission connection to the serving subscription's
  lifetime. Cleanup must also run when the subscription future is cancelled.
- [x] Ensure cleanup from an old subscription cannot clear a newer connection.
- [x] Test enable → disable: the notification bus name is released and another
  server can claim it. Test re-enable, timeout changes, and signal emission.
- [x] Keep tests on an isolated session bus, not the user's desktop bus.

Primary files: `src/notify/server.rs`, `src/shell/state.rs`, and the existing
isolated D-Bus test patterns.

The audit probe showed timeout-change restarts can succeed. The confirmed bug
is ownership retained after disabling, not a general restart failure.

### 6. Bound image work before decoding — should fix

- [x] Inspect image-ingestion callers; retain existing image-source precedence
  and text-only fallback behavior.
- [x] Check raster dimensions and decoder allocation limits before decoding or
  converting pixels. Bound file/SVG input and reject non-regular files so an
  image path cannot block on a pipe.
- [x] Move blocking image ingestion off the async request handler, with bounded
  concurrency; reuse an existing worker pattern rather than adding a subsystem.
- [x] Test oversized dimensions, malformed images, special files, normal images,
  and recovery after rejected input, without allocating a huge test image.

Primary files: `src/notify/images.rs`, `src/notify/server.rs`, and their tests.

### 5. Bound notification history and retained images — should fix

- [x] Set fixed history-count and retained-image-byte budgets; document the
  chosen limits rather than adding configuration options.
- [x] Evict oldest closed history first. Define the overflow behavior for live
  records too, so never-expiring notifications cannot bypass the limits.
- [x] Preserve replacement generations, action validation, and exactly-once
  close signals. Evicting an already-closed record must not close it again.
- [x] Account for image references retained by snapshots and exit animations;
  do not describe a queue budget as a hard limit on total process memory.
- [x] Test count/byte limits, replacement accounting, live overflow, dismissal,
  output removal, and image release after remaining visual owners finish.

Primary files: `src/shell/notifications.rs`,
`src/shell/screens/notification.rs`, `src/services/notifications.rs`,
`src/notify/images.rs`, and existing notification/scene tests.

Image ingestion comes first so history accounting can reuse its validated image
sizes. Preserve retained-history behavior within the documented limits.

## Shipping and final checks

- [x] Explain how existing installations apply changed bundled Lua handlers.
  Startup and `init --repair` must not overwrite user-owned files. Provide
  minimal manual patch instructions rather than a destructive migration.
- [x] Run focused regression tests after each fix.
- [x] After code changes, run `graphify update .` and inspect its output.
- [x] Run `nix develop --command cargo fmt --all -- --check`.
- [x] Run `nix develop --command cargo test --workspace --locked`.
- [x] Run `nix develop --command cargo clippy --workspace --all-targets --locked`.
- [x] Run `git diff --check` and review only changes made for these fixes.
- [ ] Manually check two-monitor bars, notification action clicks, notification
  enable/disable, and config conflicts during wallpaper Apply in Wayland.

## Results and limits

- Wallpaper conflicts use a byte snapshot taken at Apply, plus the known mtime
  at admission, and a byte recheck immediately before atomic config rename.
  Invalid edits, deletion, and a directory replacing the file are tested without
  publication. This is optimistic conflict detection, not cross-process locking;
  an uncooperative writer can still race the final check/rename window.
- Root action and bar tests execute the bundled Lua. Encoded identities preserve
  punctuation, slashes, spaces, and UTF-8 output names. Settings tests preserve
  the target through edits and targeted removal. The example background app has
  its own independent managed bar list, not a copy of these IPC handlers.
- An isolated D-Bus test cancels serving futures, claims the released name from
  another connection, restarts with changed/default timeouts, emits close/action
  signals, and checks that an old guard cannot clear a newer connection.
- Image limits: two nonqueued blocking workers, 8 MiB input, 1024×1024 raster,
  16 MiB decoder allocation budget, existing 512×512 raw hints. SVG is vector-only
  UTF-8 input, rasterized within 96px; compressed and nested/linked images are
  excluded to avoid secondary unbounded reads/decodes. Decoder allocation limits
  are best-effort for codec internals. Shared icon/Settings readers inherit limits.
- History limits: 256 records and 32 MiB image bytes counted per record. Evict
  oldest closed first, then expire oldest live with reason 1 exactly once. Byte
  accounting is recomputed after replacements to avoid stale counters. Snapshot
  release tests and existing scene exit-owner tests cover remaining visual owners;
  renderer caches and these owners are not included in the queue budget.
- Regression tests were observed failing before each root-cause fix and passing
  afterward. The expanded image-header test initially exceeded the BMP codec's
  own header limit; its fixture was corrected to a valid 8192×8192 header only.
- Live compositor/GPU behavior and ignored manual memory probes remain unchecked.
- Final verification: workspace tests passed (642 main-library tests plus all
  workspace/integration/doc tests; existing ignored tests stayed ignored), fmt,
  Clippy, and diff whitespace checks passed. Graphify rebuilt 5,365 nodes and
  13,370 edges with AST extraction only; its unsupported/data-file and community
  label notices are informational, not extraction failures.

### Follow-up: memory retained after explicit dismissal

User testing found higher RSS after popup clicks, Dismiss all, and notification
center removal. Queue/weak-reference tests alone had missed the framework Lua
and allocator cleanup paths.

- Framework and embedded-widget VMs now participate in the existing deferred
  reclamation pass. It expires registry keys and runs the two Lua collection
  cycles needed to finalize discarded callback/item userdata.
- Notification removal, replacement, eviction, and popup expiry schedule cleanup.
  Framework/overlay and notification-center widget ghost sweeps schedule another
  pass when exit owners finish, including text-only lists; collection does not
  run on each render/frame. A clock-driven regression checks that widget cleanup
  is requested only after its last ghost exits, not throughout the animation.
- Image workers drop input hints before trimming their temporary allocations.
  Notifications without any image source avoid starting a decode worker.
- A new regression failed before the fix: discarded framework item userdata still
  owned native memory after host reclamation. It now passes in an isolated Linux
  process and measured 78,332 → 46,400 KiB RSS after releasing a 32 MiB native
  payload. A separate test checks cleanup scheduling for all four removal paths.
- This is a headless Lua/native cleanup measurement, not a live compositor/GPU
  notification benchmark. The user's running daemon was not stopped or modified.
- Follow-up verification: all workspace tests pass (645 main-library tests,
  existing ignores unchanged), including the isolated RSS and animation tests.
  Formatting, Clippy, whitespace checks, and the AST graph refresh pass.

### Commit-split verification (2026-10-28)

- Split the pending work into feature/fix commits without changing its logic.
- Copied the installed personal frontend into `config/riced/example/`, retaining
  personal paths and customizations but excluding runtime markers and locks.
  Moved the previous background app to `examples/apps/background/` and updated
  its test includes to preserve coverage.
- Tested an exported committed tree, excluding a concurrent, uncommitted
  `src/shell/apps/popup.rs` edit containing the invalid `OnDeman` variant.
  Main-library results: 648 passed, 1 failed, 6 ignored. The failure is
  `shell::overlay::tests::default_notification_scene_composes_scoped_data_and_routes_structured_actions`:
  notification card width measured 120.128 instead of 360. Later workspace
  suites did not run after that failure. This remains unresolved.
- Fixed formatting of the relocated test includes. The verbatim copied Neovim
  template retains one trailing-space line. No live Wayland checks were run.
