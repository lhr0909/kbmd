//! Core library for `kbmd`, a Markdown-native kanban.

pub mod cli;
pub mod config;
pub mod frontmatter;
pub mod markdown;
pub mod model;
pub mod store;

pub use config::{BoardConfig, ColumnConfig};
pub use model::{Card, CardMetadata};
pub use store::{CreateCard, Project};
