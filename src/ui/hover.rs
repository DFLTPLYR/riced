//! Host-owned hover state for Lua. Lua never holds a live reference:
//! the engine derives booleans from iced enter/exit reports and re-runs the
//! affected view with them. Absence means unhovered, so untracked lists
//! decode `hovered` as `false` without special handling.
use std::collections::HashSet;

/// Item keys hovered within one list scope: `(list id, item key)`.
/// Hosts keep one set per independently re-rendered surface (overlay scene,
/// popup) and pass it as decode context so delegates observe `item.hovered`.
pub(crate) type HoverSet = HashSet<(String, String)>;
