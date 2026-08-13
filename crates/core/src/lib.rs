//! Core library for rUEXDataRunner — a Rust rebuild of SC-Datarunner-UEX.
//!
//! Pipeline: watch the screenshots folder ([`watcher`]) -> OCR + structured
//! extraction ([`pipeline`], [`ocr`], [`extract`]) using the UEX reference data
//! ([`reference`]) -> review/edit ([`model`]) -> submit to UEX ([`api`]) ->
//! persist and track reports ([`store`]). Settings live in [`config`].

pub mod api;
pub mod config;
pub mod deskew;
pub mod extract;
pub mod matching;
pub mod model;
pub mod ocr;
pub mod pipeline;
pub mod pricing;
pub mod preprocess;
pub mod reference;
pub mod status;
pub mod store;
pub mod trade;
pub mod watcher;

pub use model::{Commodity, Extraction, TerminalRef, TerminalType};
pub use reference::Reference;
