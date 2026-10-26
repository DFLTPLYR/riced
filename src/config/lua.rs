//! `config.lua`: execute the user file in a restricted VM, serialize the
//! resolved [`Config`](super::Config) back to Lua text.
//!
//! The file is executable Lua (locals, loops, arithmetic, table/string/math
//! helpers) so users can build their config, but it cannot touch the
//! filesystem or require modules. It must return a plain-data table; Rust
//! owns policy and validation (unknown keys and mistyped values fail).
//! The file only supports UTF-8 scalar tables and sequences — no functions,
//! userdata, or threads — mirroring widget `props`.
use mlua::{LuaSerdeExt, Value};
use serde_json::Value as Json;

/// Field order of the generated `config.lua`. serde_json maps preserve
/// declaration order for structs, so this accessor collects the top-level
/// fields of `value` in read order and re-emits them canonically — giving
/// stable diffs regardless of which profile inserted the key first.
const CANON: &[&str] = &[
    "apps",
    "extensions",
    "component_defaults",
    "desktop",
    "composable",
    "background",
    "theme",
    "animation",
    "interaction",
    "notifications",
    "overlays",
    "bar",
];

/// Reserved words that force bracket-quoted table keys when they appear
/// as identifiers.
const KEYWORDS: &[&str] = &[
    "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "goto", "if", "in",
    "local", "nil", "not", "or", "repeat", "return", "then", "true", "until", "while",
];

/// Evaluate `content` in the config sandbox and deserialize the returned
/// value into `T` (usually the top-level `Config`). Syntax errors name the
/// caller-provided `label`; schema errors name the offending data.
pub(super) fn parse<T>(content: &str, label: &str) -> Result<T, String>
where
    T: serde::de::DeserializeOwned,
{
    let lua = crate::lua::sandbox::new_lua(crate::lua::sandbox::Profile::Config)
        .map_err(|error| error.to_string())?;
    let returned: Value = lua
        .load(content)
        .set_name(format!("@{label}"))
        .eval()
        .map_err(|error| error.to_string())?;
    lua.from_value_with(
        returned,
        mlua::serde::de::Options::new().encode_empty_tables_as_array(true),
    )
    .map_err(|error| error.to_string())
}

/// Parse one component-default-style value (`return {raw}` wrapped) for the
/// Settings editor: `None` for an empty/`nil` entry, an error describing
/// the invalid literal otherwise. Functions and friends are rejected.
pub(crate) fn parse_value(raw: &str) -> Result<Option<Json>, String> {
    let raw = raw.trim();
    if raw.is_empty() || raw.eq_ignore_ascii_case("none") {
        return Ok(None);
    }
    let lua = crate::lua::sandbox::new_lua(crate::lua::sandbox::Profile::Config)
        .map_err(|error| error.to_string())?;
    let returned: Value = lua
        .load(format!("return {raw}"))
        .set_name("@component-value")
        .eval()
        .map_err(|error| error.to_string())?;
    if matches!(returned, Value::Nil) {
        return Ok(None);
    }
    let json: Json = lua
        .from_value(returned)
        .map_err(|error| error.to_string())?;
    Ok(Some(json))
}

/// Render `config` as a `config.lua` document: a comment preamble plus one
/// returned table.
pub(super) fn to_string(config: &super::Config) -> Result<String, String> {
    let value = serde_json::to_value(config).map_err(|error| error.to_string())?;
    Ok(format!(
        "-- riced configuration. Settings rewrites this file; edits reload live.\nreturn {}",
        stringify(&value, 0)?
    ))
}

/// Render one JSON value (component default, saved-list entry, editor
/// draft) as compact Lua text. Lua has a single `{}` constructor, so an
/// empty map prints as `{}` and round-trips as the caller's target type.
pub(crate) fn to_literal(value: &Json) -> String {
    stringify(value, 0).unwrap_or_else(|_| "\"<invalid>\"".into())
}

fn is_scalar(value: &Json) -> bool {
    value.is_null() || value.is_boolean() || value.is_number() || value.is_string()
}

fn stringify(value: &Json, indent: usize) -> Result<String, String> {
    match value {
        Json::Null => Ok("nil".into()),
        Json::Bool(flag) => Ok(flag.to_string()),
        Json::Number(number) => {
            if number.is_i64() || number.is_u64() {
                Ok(number.to_string())
            } else {
                let float = number.as_f64().expect("JSON float");
                if !float.is_finite() {
                    return Err("non-finite numbers cannot appear in config.lua".into());
                }
                let mut text = format!("{float:?}");
                if !text.contains('.') && !text.contains('e') {
                    text.push_str(".0");
                }
                Ok(text)
            }
        }
        Json::String(text) => Ok(quote(text)),
        Json::Array(items) => {
            if items.is_empty() {
                return Ok("{}".into());
            }
            if items.iter().all(is_scalar) {
                let joined = items
                    .iter()
                    .map(|item| stringify(item, 0))
                    .collect::<Result<Vec<_>, _>>()?
                    .join(", ");
                return Ok(format!("{{ {joined} }}"));
            }
            let pad = "    ".repeat(indent + 1);
            let close = "    ".repeat(indent);
            let mut out = String::from("{\n");
            for item in items {
                out.push_str(&pad);
                out.push_str(&stringify(item, indent + 1)?);
                out.push_str(",\n");
            }
            out.push_str(&close);
            out.push('}');
            Ok(out)
        }
        Json::Object(map) => {
            if map.is_empty() {
                return Ok("{}".into());
            }
            let entries: Vec<(&String, &Json)> = ordered(map);
            let inline = entries.iter().all(|(_, value)| is_scalar(value))
                && rendered_len(&entries).is_some_and(|len| indent * 4 + len <= 96);
            if inline {
                let joined = entries
                    .iter()
                    .map(|(key, value)| Ok(format!("{} = {}", field(key)?, stringify(value, 0)?)))
                    .collect::<Result<Vec<_>, String>>()?
                    .join(", ");
                return Ok(format!("{{ {joined} }}"));
            }
            let pad = "    ".repeat(indent + 1);
            let close = "    ".repeat(indent);
            let mut out = String::from("{\n");
            for (key, value) in entries {
                out.push_str(&pad);
                out.push_str(&field(key)?);
                out.push_str(" = ");
                out.push_str(&stringify(value, indent + 1)?);
                out.push_str(",\n");
            }
            out.push_str(&close);
            out.push('}');
            Ok(out)
        }
    }
}

/// Top-level fields come out in [`CANON`] order for stable diffs; all other
/// tables sort by key so per-instance overrides always print identically.
fn ordered(map: &serde_json::Map<String, Json>) -> Vec<(&String, &Json)> {
    let mut rest: Vec<(&String, &Json)> = Vec::with_capacity(map.len());
    let mut cannon: Vec<(&String, &Json)> = Vec::new();
    for (key, value) in map {
        if CANON.contains(&key.as_str()) {
            cannon.push((key, value));
        } else {
            rest.push((key, value));
        }
    }
    cannon.sort_by_key(|(key, _)| {
        CANON
            .iter()
            .position(|name| name == key)
            .unwrap_or(usize::MAX)
    });
    rest.sort_by_key(|(key, _)| *key);
    cannon.extend(rest);
    cannon
}

fn rendered_len(entries: &[(&String, &Json)]) -> Option<usize> {
    entries.iter().try_fold(4usize, |len, (key, value)| {
        stringify(value, 0)
            .ok()
            .map(|text| len + key.len() + text.len() + 7)
    })
}

fn field(key: &str) -> Result<String, String> {
    if is_ident(key) && !KEYWORDS.contains(&key) {
        Ok(key.to_string())
    } else {
        Ok(format!("[{}]", quote(key)))
    }
}

fn is_ident(key: &str) -> bool {
    !key.is_empty()
        && key.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_alphabetic() || byte == b'_' || (index > 0 && byte.is_ascii_digit())
        })
}

/// Double-quoted Lua string literal with ASCII escapes; arbitrary bytes
/// become `\ddd` escapes so any UTF-8 path round-trips.
fn quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for byte in text.bytes() {
        match byte {
            b'"' => out.push_str("\\\""),
            b'\\' => out.push_str("\\\\"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            b'\t' => out.push_str("\\t"),
            0x20..=0x7e => out.push(byte as char),
            _ => out.push_str(&format!("\\{byte:03}")),
        }
    }
    out.push('"');
    out
}
