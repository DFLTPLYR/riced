//! Settings-only arrangement preview: chips are labels, never executable widgets.
use crate::app::{BarEvent, Plant, SettingEvent, TopEvent};
use crate::theme;
use iced::widget::canvas::{self, Action, Event, Frame, Geometry, Path, Stroke};
use iced::{Element, Length, Point, Rectangle, Renderer, Size, Theme, keyboard, mouse, window};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Site {
    Slot { slot: usize, index: usize },
    Pool(String),
}

/// Drop on an occupied position swaps; drop at the end appends.
/// Pool -> occupied inserts before it (the pool is not a placed position).
pub fn rearrange(slots: &[Vec<String>], from: &Site, to: &Site) -> Option<Vec<Vec<String>>> {
    if from == to {
        return None;
    }
    let mut next = slots.to_vec();
    let name = match from {
        Site::Slot { slot, index } => slots.get(*slot)?.get(*index)?.clone(),
        Site::Pool(name) => name.clone(),
    };
    if name.trim().is_empty() || name.eq_ignore_ascii_case("none") {
        return None;
    }
    match to {
        Site::Pool(_) => {
            let Site::Slot { slot, index } = from else {
                return None;
            };
            next[*slot].remove(*index);
        }
        Site::Slot {
            slot: target,
            index: at,
        } => {
            if *at > slots.get(*target)?.len() {
                return None;
            }
            match from {
                Site::Pool(_) => next[*target].insert(*at, name),
                Site::Slot {
                    slot: origin,
                    index,
                } => {
                    if let Some(other) = slots[*target].get(*at) {
                        next[*origin][*index] = other.clone();
                        next[*target][*at] = name;
                    } else {
                        next[*origin].remove(*index);
                        next[*target].push(name);
                    }
                }
            }
        }
    }
    // Names may appear on multiple slots, but not twice within a slot.
    if next
        .iter()
        .any(|slot| slot.iter().enumerate().any(|(i, n)| slot[..i].contains(n)))
    {
        return None;
    }
    (next != slots).then_some(next)
}

#[derive(Clone)]
struct Chip {
    site: Site,
    label: String,
    rect: Rectangle,
}

pub struct Preview {
    settings: window::Id,
    bar: window::Id,
    slots: Vec<Vec<String>>,
    regions: Vec<Rectangle>,
    chips: Vec<Chip>,
    pool: Rectangle,
    size: Size,
    selected: usize,
}

#[derive(Default)]
pub struct State {
    drag: Option<Drag>,
}

struct Drag {
    bar: window::Id,
    from: Site,
    label: String,
    start: Point,
    cursor: Point,
    snapshot: Vec<Vec<String>>,
}

impl Preview {
    pub fn new(
        settings: window::Id,
        bar: window::Id,
        slots: Vec<Vec<String>>,
        names: Vec<String>,
        horizontal: bool,
        selected: usize,
        width: f32,
    ) -> Self {
        let width = width.max(1.0);
        let gap = if horizontal {
            (width / slots.len().max(1) as f32 * 0.1).min(8.0)
        } else {
            0.0
        };
        let cell_width = if horizontal {
            (width - gap * slots.len().saturating_sub(1) as f32) / slots.len().max(1) as f32
        } else {
            width
        };
        let mut regions = Vec::new();
        let mut chips = Vec::new();
        let mut bottom: f32 = 0.0;
        for (slot, widgets) in slots.iter().enumerate() {
            let x = if horizontal {
                slot as f32 * (cell_width + gap)
            } else {
                0.0
            };
            let y = if horizontal { 0.0 } else { bottom + 8.0 };
            let mut cx = x + 12.0;
            let mut cy = y + 38.0;
            for (index, name) in widgets.iter().enumerate() {
                let label = if names.contains(name) {
                    name.clone()
                } else {
                    format!("{name} (missing)")
                };
                let w = (label.chars().count() as f32 * 7.5 + 24.0)
                    .max(64.0)
                    .min((cell_width - 24.0).max(1.0));
                if cx + w > x + cell_width - 12.0 {
                    cx = x + 12.0;
                    cy += 38.0;
                }
                chips.push(Chip {
                    site: Site::Slot { slot, index },
                    label,
                    rect: Rectangle::new(Point::new(cx, cy), Size::new(w, 30.0)),
                });
                cx += w + 6.0;
            }
            let height = (cy - y + 50.0).max(104.0);
            regions.push(Rectangle::new(
                Point::new(x, y),
                Size::new(cell_width, height),
            ));
            bottom = bottom.max(y + height);
        }
        let pool_y = bottom + 24.0;
        let mut x = 12.0;
        let mut y = pool_y + 58.0;
        for name in names
            .iter()
            .filter(|name| !slots.iter().flatten().any(|placed| placed == *name))
        {
            let w = (name.chars().count() as f32 * 7.5 + 24.0)
                .max(64.0)
                .min((width - 24.0).max(1.0));
            if x + w > width - 12.0 {
                x = 12.0;
                y += 38.0;
            }
            chips.push(Chip {
                site: Site::Pool(name.clone()),
                label: name.clone(),
                rect: Rectangle::new(Point::new(x, y), Size::new(w, 30.0)),
            });
            x += w + 6.0;
        }
        let pool = Rectangle::new(Point::new(0.0, pool_y), Size::new(width, y - pool_y + 50.0));
        let size = Size::new(width, pool.y + pool.height + 8.0);
        Self {
            settings,
            bar,
            slots,
            regions,
            chips,
            pool,
            size,
            selected,
        }
    }

    pub fn element(self) -> Element<'static, Plant> {
        let size = self.size;
        canvas::Canvas::new(self)
            .width(Length::Fill)
            .height(Length::Fixed(size.height))
            .into()
    }

    fn hit(&self, p: Point) -> Option<Site> {
        if let Some(chip) = self.chips.iter().find(|c| c.rect.contains(p)) {
            return Some(chip.site.clone());
        }
        if self.pool.contains(p) {
            return Some(Site::Pool(String::new()));
        }
        self.regions
            .iter()
            .position(|r| r.contains(p))
            .map(|slot| Site::Slot {
                slot,
                index: self.slots[slot].len(),
            })
    }
}

impl canvas::Program<Plant> for Preview {
    type State = State;
    fn update(
        &self,
        state: &mut State,
        event: &Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<Action<Plant>> {
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let p = cursor.position_in(bounds)?;
                if let Some(chip) = self.chips.iter().find(|c| c.rect.contains(p)) {
                    state.drag = Some(Drag {
                        bar: self.bar,
                        from: chip.site.clone(),
                        label: chip.label.clone(),
                        start: p,
                        cursor: p,
                        snapshot: self.slots.clone(),
                    });
                    return Some(Action::request_redraw().and_capture());
                }
                if let Some(Site::Slot { slot, .. }) = self.hit(p) {
                    return Some(
                        Action::publish(Plant::SettingPlot(SettingEvent::SelectSlot(
                            self.settings,
                            slot,
                        )))
                        .and_capture(),
                    );
                }
                None
            }
            Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                let Some(drag) = state.drag.as_mut() else {
                    return Some(Action::request_redraw());
                };
                if let Some(p) = cursor.position_in(bounds) {
                    drag.cursor = p;
                }
                Some(Action::request_redraw().and_capture())
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                let drag = state.drag.take()?;
                if drag.bar != self.bar || drag.snapshot != self.slots {
                    return Some(Action::request_redraw().and_capture());
                }
                let Some(p) = cursor.position_in(bounds) else {
                    return Some(Action::request_redraw());
                };
                if p.distance(drag.start) < 5.0 {
                    if let Site::Slot { slot, .. } = drag.from {
                        return Some(
                            Action::publish(Plant::SettingPlot(SettingEvent::SelectSlot(
                                self.settings,
                                slot,
                            )))
                            .and_capture(),
                        );
                    }
                } else if let Some(to) = self.hit(p)
                    && let Some(widgets) = rearrange(&self.slots, &drag.from, &to)
                {
                    return Some(
                        Action::publish(Plant::TopPlot(TopEvent::Bar(BarEvent::WidgetLayout {
                            bar: self.bar,
                            expected: drag.snapshot,
                            widgets,
                        })))
                        .and_capture(),
                    );
                }
                Some(Action::request_redraw().and_capture())
            }
            Event::Mouse(mouse::Event::CursorLeft)
            | Event::Keyboard(keyboard::Event::KeyPressed {
                key: keyboard::Key::Named(keyboard::key::Named::Escape),
                ..
            }) => {
                state.drag.take()?;
                Some(Action::request_redraw())
            }
            _ => None,
        }
    }

    fn draw(
        &self,
        state: &State,
        renderer: &Renderer,
        _: &Theme,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let palette = theme::active();
        let drag = state
            .drag
            .as_ref()
            .filter(|d| d.bar == self.bar && d.snapshot == self.slots);
        let target = drag.and_then(|d| self.hit(d.cursor));
        let paint = |frame: &mut Frame, rect: Rectangle, fill, outline| {
            let path = Path::rounded_rectangle(rect.position(), rect.size(), 8.0.into());
            frame.fill(&path, fill);
            frame.stroke(&path, Stroke::default().with_color(outline).with_width(1.5));
        };
        for (slot, rect) in self.regions.iter().enumerate() {
            let highlighted = matches!(target, Some(Site::Slot { slot: s, .. }) if s == slot);
            paint(
                &mut frame,
                *rect,
                palette.surface,
                if highlighted || self.selected == slot {
                    palette.primary
                } else {
                    palette.outline
                },
            );
            frame.fill_text(canvas::Text {
                content: format!("Slot {}", slot + 1),
                position: Point::new(rect.x + 12.0, rect.y + 10.0),
                color: palette.on_surface,
                size: 14.0.into(),
                ..Default::default()
            });
            if self.slots[slot].is_empty() {
                frame.fill_text(canvas::Text {
                    content: "Drop widget here".into(),
                    position: Point::new(rect.x + 12.0, rect.y + 46.0),
                    color: palette.on_surface_variant,
                    size: 12.0.into(),
                    ..Default::default()
                });
            }
        }
        paint(
            &mut frame,
            self.pool,
            palette.surface,
            if matches!(target, Some(Site::Pool(_))) {
                palette.primary
            } else {
                palette.outline
            },
        );
        frame.fill_text(canvas::Text {
            content: "Available widgets / remove from bar".into(),
            position: Point::new(12.0, self.pool.y + 10.0),
            color: palette.on_surface,
            size: 14.0.into(),
            ..Default::default()
        });
        frame.fill_text(canvas::Text {
            content: "Drag into a slot to add; drop here to remove.".into(),
            position: Point::new(12.0, self.pool.y + 32.0),
            color: palette.on_surface_variant,
            size: 12.0.into(),
            ..Default::default()
        });
        for chip in &self.chips {
            let picked = drag.is_some_and(|d| d.from == chip.site);
            let highlight = target.as_ref() == Some(&chip.site)
                || cursor
                    .position_in(bounds)
                    .is_some_and(|p| chip.rect.contains(p));
            paint(
                &mut frame,
                chip.rect,
                if picked {
                    palette.surface
                } else {
                    palette.surface_variant
                },
                if highlight {
                    palette.primary
                } else {
                    palette.outline
                },
            );
            let max = ((chip.rect.width - 20.0) / 7.5) as usize;
            let label = if chip.label.chars().count() > max {
                format!(
                    "{}…",
                    chip.label
                        .chars()
                        .take(max.saturating_sub(1))
                        .collect::<String>()
                )
            } else {
                chip.label.clone()
            };
            frame.fill_text(canvas::Text {
                content: label,
                position: Point::new(chip.rect.x + 10.0, chip.rect.y + 6.0),
                color: palette.on_surface,
                size: 13.0.into(),
                ..Default::default()
            });
        }
        if let Some(drag) = drag {
            let w = (drag.label.chars().count() as f32 * 7.5 + 24.0)
                .min((bounds.width - 16.0).max(1.0));
            let rect = Rectangle::new(
                Point::new(
                    (drag.cursor.x + 8.0).clamp(0.0, (bounds.width - w).max(0.0)),
                    (drag.cursor.y + 8.0).clamp(0.0, (bounds.height - 30.0).max(0.0)),
                ),
                Size::new(w, 30.0),
            );
            paint(&mut frame, rect, palette.primary, palette.on_primary);
            frame.fill_text(canvas::Text {
                content: drag.label.clone(),
                position: Point::new(rect.x + 10.0, rect.y + 6.0),
                color: palette.on_primary,
                size: 13.0.into(),
                ..Default::default()
            });
        }
        vec![frame.into_geometry()]
    }

    fn mouse_interaction(
        &self,
        state: &State,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        if state.drag.is_some() {
            mouse::Interaction::Grabbing
        } else if cursor
            .position_in(bounds)
            .is_some_and(|p| self.chips.iter().any(|c| c.rect.contains(p)))
        {
            mouse::Interaction::Grab
        } else {
            mouse::Interaction::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn slot(slot: usize, index: usize) -> Site {
        Site::Slot { slot, index }
    }
    fn model() -> Vec<Vec<String>> {
        vec![vec!["a".into(), "b".into()], vec!["c".into()], vec![]]
    }
    #[test]
    fn swap_move_add_remove_and_invalid_drop() {
        let s = model();
        assert_eq!(
            rearrange(&s, &slot(0, 0), &slot(0, 1)).unwrap()[0],
            ["b", "a"]
        );
        let swapped = rearrange(&s, &slot(0, 0), &slot(1, 0)).unwrap();
        assert_eq!(swapped[0], ["c", "b"]);
        assert_eq!(swapped[1], ["a"]);
        let moved = rearrange(&s, &slot(0, 1), &slot(2, 0)).unwrap();
        assert_eq!(moved[0], ["a"]);
        assert_eq!(moved[2], ["b"]);
        assert_eq!(
            rearrange(&s, &Site::Pool("d".into()), &slot(1, 0)).unwrap()[1],
            ["d", "c"]
        );
        assert_eq!(
            rearrange(&s, &slot(0, 0), &Site::Pool(String::new())).unwrap()[0],
            ["b"]
        );
        assert!(rearrange(&s, &slot(9, 0), &slot(0, 0)).is_none());
        assert!(rearrange(&s, &Site::Pool("a".into()), &slot(0, 0)).is_none());
        assert!(rearrange(&s, &Site::Pool("none".into()), &slot(2, 0)).is_none());
        assert!(rearrange(&s, &slot(0, 0), &slot(0, 0)).is_none());
    }
    #[test]
    fn hit_testing_tracks_chips_empty_slots_and_pool() {
        for horizontal in [true, false] {
            let preview = Preview::new(
                window::Id::unique(),
                window::Id::unique(),
                model(),
                vec!["a".into(), "b".into(), "c".into(), "d".into()],
                horizontal,
                0,
                600.0,
            );
            assert_eq!(
                preview.hit(preview.chips[0].rect.center()),
                Some(slot(0, 0))
            );
            assert_eq!(preview.hit(preview.regions[2].center()), Some(slot(2, 0)));
            assert!(matches!(
                preview.hit(Point::new(20.0, preview.pool.y + 20.0)),
                Some(Site::Pool(_))
            ));
            assert!(preview.hit(Point::new(-10.0, -10.0)).is_none());
        }
    }
    #[test]
    fn slots_fill_the_available_width_after_resize() {
        for width in [320.0, 800.0, 1200.0] {
            let p = Preview::new(
                window::Id::unique(),
                window::Id::unique(),
                model(),
                vec![],
                true,
                0,
                width,
            );
            assert!(
                (p.regions.last().unwrap().x + p.regions.last().unwrap().width - width).abs()
                    < 0.01
            );
            assert_eq!(p.pool.width, width);
            assert!(p.regions.iter().all(|r| r.width == p.regions[0].width));
            for chip in p
                .chips
                .iter()
                .filter(|c| matches!(c.site, Site::Slot { .. }))
            {
                assert_eq!(p.hit(chip.rect.center()), Some(chip.site.clone()));
            }
        }
    }
}
