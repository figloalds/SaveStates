//! The history timeline: a left-to-right "metro line" of saves.

pub mod axis;
// Task 8 feeds the widget every timeline through it; until then only its tests use it.
#[allow(dead_code)]
pub mod data;
pub mod layout;
pub mod model;
mod paint;
pub mod widget;
