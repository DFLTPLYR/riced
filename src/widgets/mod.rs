//! Bar widgets: one small view per slot kind.
//!
//! Each widget is a pure function returning an
//! [`Element`](iced::Element) for a bar grid cell. Slots pick a
//! widget by position (see `SlotWidget` on `TopLocal`); empty slots
//! render a numbered placeholder from `Top::view`.

pub mod clock;
