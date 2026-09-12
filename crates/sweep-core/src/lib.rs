//! sweep-core: scanning, detection and cleaning engine for sweep.
//!
//! The crate is intentionally UI-free so the same engine powers the
//! human TUI/CLI and machine-consumable `--json` output for agents.

pub mod cleaner;
pub mod detectors;
pub mod model;
pub mod scanner;

pub use cleaner::{clean, execute, plan, CleanPlan};
pub use model::{
    CleanAction, CleanOptions, CleanReceipt, DirEntry, Finding, IncludeLevel, Safety, ScanOptions,
    ScanReport,
};
