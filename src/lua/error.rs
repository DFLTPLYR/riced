//! Common Lua diagnostics and deduplication across host adapters.
use std::{collections::HashMap, sync::Arc};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LuaError {
    pub entry: String,
    pub message: String,
}

pub(crate) fn report_once(last: &mut Option<String>, prefix: &str, message: String) {
    if last.as_ref() != Some(&message) {
        eprintln!("{prefix}{message}");
        *last = Some(message);
    }
}

pub(crate) fn report_keyed(
    errors: &mut HashMap<String, String>,
    key: &str,
    prefix: &str,
    message: String,
) {
    if errors.get(key) != Some(&message) {
        eprintln!("{prefix}{message}");
        errors.insert(key.into(), message);
    }
}

pub(crate) fn record_entry(last: &mut Option<Arc<LuaError>>, entry: &str, message: String) {
    let failure = LuaError {
        entry: entry.into(),
        message,
    };
    if last.as_deref() != Some(&failure) {
        tracing::error!(entry, error = %failure.message, "Lua entry failed");
    }
    *last = Some(Arc::new(failure));
}
