//! Platform-independent logic: geometry, images, annotations, capture interaction, text blocks,
//! translation and settings. Everything here builds and is tested on any OS.

pub mod geom;
pub mod image;
pub mod annotation;
pub mod color;
pub mod colorsample;
pub mod textblocks;
pub mod translator;
pub mod settings;
pub mod export;
pub mod http;
pub mod textselection;
pub mod scrollstitcher;
