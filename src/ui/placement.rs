//! Parent-local alignment, independent of native surfaces and notification policy.
use iced::{Alignment, Point, Size};

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Placement {
    pub horizontal: Alignment,
    pub vertical: Alignment,
}
impl Placement {
    pub(crate) fn parse(name: &str) -> Result<Self, String> {
        use Alignment::{Center, End, Start};
        let names = [
            (
                "top_left",
                Self {
                    horizontal: Start,
                    vertical: Start,
                },
            ),
            (
                "top_center",
                Self {
                    horizontal: Center,
                    vertical: Start,
                },
            ),
            (
                "top_right",
                Self {
                    horizontal: End,
                    vertical: Start,
                },
            ),
            (
                "left",
                Self {
                    horizontal: Start,
                    vertical: Center,
                },
            ),
            (
                "center",
                Self {
                    horizontal: Center,
                    vertical: Center,
                },
            ),
            (
                "right",
                Self {
                    horizontal: End,
                    vertical: Center,
                },
            ),
            (
                "bottom_left",
                Self {
                    horizontal: Start,
                    vertical: End,
                },
            ),
            (
                "bottom_center",
                Self {
                    horizontal: Center,
                    vertical: End,
                },
            ),
            (
                "bottom_right",
                Self {
                    horizontal: End,
                    vertical: End,
                },
            ),
        ];
        crate::shared::enums::lookup(&names, name).ok_or_else(|| {
            format!(
                "anchor must be {}, got {name:?}",
                crate::shared::enums::expected(&names)
            )
        })
    }
    pub(crate) fn name(self) -> &'static str {
        use Alignment::{Center, End, Start};
        match (self.horizontal, self.vertical) {
            (Start, Start) => "top_left",
            (Center, Start) => "top_center",
            (End, Start) => "top_right",
            (Start, Center) => "left",
            (Center, Center) => "center",
            (End, Center) => "right",
            (Start, End) => "bottom_left",
            (Center, End) => "bottom_center",
            (End, End) => "bottom_right",
        }
    }
    pub(crate) fn origin(self, parent: Size, child: Size, margin: f32) -> Point {
        let align = |space: f32, extent: f32, alignment| {
            let spare = (space - extent).max(0.0);
            let margin = margin.min(spare / 2.0);
            match alignment {
                Alignment::Start => margin,
                Alignment::Center => spare / 2.0,
                Alignment::End => spare - margin,
            }
        };
        Point::new(
            align(parent.width, child.width, self.horizontal),
            align(parent.height, child.height, self.vertical),
        )
    }
}
