//! Pointer and click identity state owned independently of window domains.
use iced::{Point, window};
use std::{collections::HashMap, time::Instant};

#[derive(Debug, Default)]
pub(crate) struct InputState {
    pub cursors: HashMap<window::Id, Point>,
    pub presses: HashMap<window::Id, (usize, Option<String>, Instant)>,
    pub global: Option<Point>,
}
