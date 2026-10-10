//! Generated icon lookup and rich-text placeholders, shared by all surfaces.
use crate::shell::Plant;
use iced::{
    Element,
    widget::{row, text},
};

pub(crate) fn icon_bytes(name: &str) -> Option<&'static [u8]> {
    let key = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_lowercase();
    generated_icon_bytes(&key)
}
include!(concat!(env!("OUT_DIR"), "/lucide_lookup.rs"));

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Segment<'a> {
    Text(&'a str),
    Icon(&'a str),
}

pub(crate) fn icon_segments(output: &str) -> Vec<Segment<'_>> {
    let mut segments = Vec::new();
    let mut rest = output;
    while let Some(start) = rest.find("{icon:") {
        if start > 0 {
            segments.push(Segment::Text(&rest[..start]));
        }
        let after = &rest[start + "{icon:".len()..];
        match after.find('}') {
            Some(end) => {
                segments.push(Segment::Icon(after[..end].trim()));
                rest = &after[end + 1..];
            }
            None => {
                segments.push(Segment::Text(&rest[start..]));
                rest = "";
            }
        }
    }
    if !rest.is_empty() {
        segments.push(Segment::Text(rest));
    }
    segments
}
pub(crate) fn rich_text(output: String, size: f32, spacing: f32) -> Element<'static, Plant> {
    let size = size.max(1.0);
    if !output.contains("{icon:") {
        return text(output).size(size).into();
    }
    let mut row = row![]
        .spacing(spacing.max(0.0))
        .align_y(iced::Alignment::Center)
        .width(iced::Length::Shrink)
        .height(iced::Length::Shrink);
    for segment in icon_segments(&output) {
        match segment {
            Segment::Text(t) if !t.is_empty() => row = row.push(text(t.to_owned()).size(size)),
            Segment::Icon(name) => match icon_bytes(name) {
                Some(bytes) => row = row.push(lucide_iced::themed_icon(bytes, size)),
                None => row = row.push(text(format!("{{icon:{name}}}")).size(size)),
            },
            _ => {}
        }
    }
    row.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rich_text_builds_without_a_renderer() {
        let _ = rich_text("12%".into(), 13.0, 4.0);
        let _ = rich_text("{icon:cpu} 12%".into(), 13.0, 4.0);
        let _ = rich_text("{icon:nope}".into(), 13.0, 4.0);
    }

    #[test]
    fn icon_segments_split_placeholders() {
        use Segment::{Icon, Text};
        assert_eq!(icon_segments("12%"), vec![Text("12%")]);
        assert_eq!(
            icon_segments("{icon:cpu} 12%"),
            vec![Icon("cpu"), Text(" 12%")]
        );
        assert_eq!(
            icon_segments("{icon:cpu}{icon:mem}"),
            vec![Icon("cpu"), Icon("mem")]
        );
        assert_eq!(icon_segments("{icon:cpu"), vec![Text("{icon:cpu")]);
        assert_eq!(icon_segments("{icon:}"), vec![Icon("")]);
        assert_eq!(icon_segments(""), Vec::new());
    }

    #[test]
    fn icon_bytes_resolves_names_case_insensitively() {
        for name in [
            "cpu",
            "CPU",
            "Heart",
            "memory-stick",
            "memory_stick",
            "MemoryStick",
            "mem",
            "disk",
            "bot",
            "Bot",
            "robot-vacuum",
            "house",
            "power",
        ] {
            assert!(icon_bytes(name).is_some(), "{name}");
        }
        assert!(icon_bytes("nope").is_none());
        assert!(icon_bytes("").is_none());
    }
}
