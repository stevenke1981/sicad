//! PromptCAD Rust core (ADR 0005): the CadDocument 2.0 document model, validation, solver
//! pre-pass, 1.0 migration and patch / op-log engine.
//!
//! The library performs no I/O; hosts (Tauri commands, the CLI in `src/bin`, a server shim)
//! read and write files. Normative spec: `docs/CAD_DSL_2.md`.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod canonical;
pub mod fields;
pub mod model;

pub mod bindings;
pub mod expr;
pub mod migrate;
pub mod oplog;
pub mod patch_engine;
pub mod ports;
pub mod solver;
pub mod validate;

pub use model::{CadDocument1, CadDocument2};
