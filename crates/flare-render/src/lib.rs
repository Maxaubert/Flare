//! Window-free terminal grid rendering: the grid model, layout into a scene, and two painters.

pub mod atlas;
pub mod blend;
pub mod color;
pub mod cpu;
pub mod decor;
pub mod device_pixels;
pub mod fallback;
pub mod font_catalog;
pub mod glyph;
pub mod gpu;
pub mod grid;
pub mod hit;
pub mod metrics;
pub mod raster;
pub mod sample;
pub mod scene;
pub mod shapes;
pub mod stats;
