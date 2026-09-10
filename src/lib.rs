#![allow(
    clippy::missing_errors_doc,
    reason = "Los errores públicos son enums tipados documentados y se muestran de forma accionable en la UI"
)]

pub mod actions;
pub mod app;
pub mod cursor;
pub mod domain;
pub mod git;
pub mod persistence;
pub mod process;
pub mod ui;
pub mod watcher;
