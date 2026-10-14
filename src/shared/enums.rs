//! Table-driven string-to-enum parsing shared by UI, shell, and theme code.
//! Each owner keeps its names and unknown-policy colocated; this module owns
//! only the lookup mechanics: case-insensitive, with `-` and `_` equivalent
//! (`word-or-glyph` matches a `word_or_glyph` table entry, and vice versa).

/// Normalize an identifier for lookup: lowercase, `-` becomes `_`.
fn normalize(value: &str) -> String {
    value.trim().to_lowercase().replace('-', "_")
}

/// Look up `input` in a `[(name, value)]` table, case-insensitively, with
/// `-` and `_` interchangeable. `None` on no match; callers own the policy.
pub(crate) fn lookup<T: Clone>(table: &[(&str, T)], input: &str) -> Option<T> {
    let input = normalize(input);
    table
        .iter()
        .find(|(name, _)| normalize(name) == input)
        .map(|(_, value)| value.clone())
}

/// Human-readable option list for error messages, e.g.
/// `"fill", "shrink", or "auto"`. Table names print verbatim (they are the
/// canonical spellings); callers may prefer them normalized already.
pub(crate) fn expected<T>(table: &[(&str, T)]) -> String {
    match table {
        [] => String::new(),
        [(only, _)] => format!("\"{only}\""),
        names => format!(
            "{}, or \"{}\"",
            names[..names.len() - 1]
                .iter()
                .map(|(name, _)| format!("\"{name}\""))
                .collect::<Vec<_>>()
                .join(", "),
            names.last().map(|(name, _)| name).copied().unwrap_or("")
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_normalizes_case_and_dashes() {
        let table = &[("none", 1), ("word", 2), ("word_or_glyph", 3)];
        assert_eq!(lookup(table, "none"), Some(1));
        assert_eq!(lookup(table, "Word"), Some(2));
        assert_eq!(lookup(table, " WORD "), Some(2));
        // Hyphens match underscores in both directions.
        assert_eq!(lookup(table, "word-or-glyph"), Some(3));
        assert_eq!(lookup(table, "Word_Or_Glyph"), Some(3));
        assert_eq!(lookup(table, "bogus"), None);
        assert_eq!(lookup(table, ""), None);
        // Empty table and one-entry table.
        assert_eq!(lookup::<u8>(&[], "x"), None);
        assert_eq!(lookup(&[("only", 7)], "ONLY"), Some(7));
    }

    #[test]
    fn expected_lists_every_option_with_a_final_or() {
        assert_eq!(expected::<u8>(&[]), "");
        assert_eq!(expected(&[("fill", 1)]), "\"fill\"");
        assert_eq!(
            expected(&[("fill", 1), ("shrink", 2)]),
            "\"fill\", or \"shrink\""
        );
        assert_eq!(
            expected(&[("fill", 1), ("shrink", 2), ("auto", 3)]),
            "\"fill\", \"shrink\", or \"auto\""
        );
    }
}
