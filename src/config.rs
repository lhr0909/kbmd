use std::collections::HashSet;
use std::path::{Component, Path};

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

/// Project configuration stored in `.kbmd/config.yml`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoardConfig {
    #[serde(default = "schema_version")]
    pub version: u8,
    pub name: String,
    #[serde(default = "default_cards_dir")]
    pub cards_dir: String,
    #[serde(default = "default_id_prefix")]
    pub id_prefix: String,
    pub default_status: String,
    pub columns: Vec<ColumnConfig>,
}

/// One kanban column. The name is the exact value written to card frontmatter.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColumnConfig {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wip_limit: Option<usize>,
}

const fn schema_version() -> u8 {
    1
}

fn default_cards_dir() -> String {
    "cards".to_owned()
}

fn default_id_prefix() -> String {
    "KB".to_owned()
}

impl BoardConfig {
    pub fn new(
        name: impl Into<String>,
        id_prefix: impl Into<String>,
        statuses: Vec<String>,
    ) -> Self {
        let columns = statuses
            .into_iter()
            .enumerate()
            .map(|(index, name)| ColumnConfig {
                name: name.trim().to_owned(),
                color: Some(default_column_color(index).to_owned()),
                wip_limit: None,
            })
            .collect::<Vec<_>>();
        let default_status = columns
            .first()
            .map(|column| column.name.clone())
            .unwrap_or_default();

        Self {
            version: schema_version(),
            name: name.into().trim().to_owned(),
            cards_dir: default_cards_dir(),
            id_prefix: id_prefix.into().trim().to_ascii_uppercase(),
            default_status,
            columns,
        }
    }

    pub fn validate(&self) -> Result<()> {
        if self.version != schema_version() {
            bail!(
                "unsupported config version {}; this build supports version {}",
                self.version,
                schema_version()
            );
        }
        if self.name.trim().is_empty() {
            bail!("board name cannot be empty");
        }
        if self.name != self.name.trim() {
            bail!("board name cannot start or end with whitespace");
        }
        reject_control_characters("board name", &self.name)?;
        if self.columns.is_empty() {
            bail!("at least one column is required");
        }

        let mut names = HashSet::new();
        for column in &self.columns {
            let name = column.name.trim();
            if name.is_empty() {
                bail!("column names cannot be empty");
            }
            if name != column.name {
                bail!("column names cannot start or end with whitespace");
            }
            reject_control_characters("column names", &column.name)?;
            if !names.insert(name.to_lowercase()) {
                bail!("duplicate column name: {name}");
            }
            if column.wip_limit == Some(0) {
                bail!("WIP limit for {name} must be greater than zero");
            }
        }

        if self.resolve_status(&self.default_status).is_none() {
            bail!(
                "default status {:?} is not one of the configured columns",
                self.default_status
            );
        }

        let prefix = self.id_prefix.trim();
        if prefix != self.id_prefix {
            bail!("id_prefix cannot start or end with whitespace");
        }
        if prefix.is_empty()
            || !prefix
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '_')
        {
            bail!("id_prefix must contain only ASCII letters, digits, or underscores");
        }

        let path = Path::new(&self.cards_dir);
        reject_control_characters("cards_dir", &self.cards_dir)?;
        if path.as_os_str().is_empty()
            || path.is_absolute()
            || path
                .components()
                .any(|component| !matches!(component, Component::Normal(_) | Component::CurDir))
        {
            bail!("cards_dir must be a relative path contained inside .kbmd");
        }

        Ok(())
    }

    /// Resolve a user-supplied status case-insensitively to its canonical spelling.
    pub fn resolve_status(&self, candidate: &str) -> Option<&str> {
        self.columns
            .iter()
            .find(|column| column.name.eq_ignore_ascii_case(candidate.trim()))
            .map(|column| column.name.as_str())
    }

    pub fn column_index(&self, status: &str) -> Option<usize> {
        self.columns
            .iter()
            .position(|column| column.name.eq_ignore_ascii_case(status))
    }
}

fn reject_control_characters(field: &str, value: &str) -> Result<()> {
    if value.chars().any(char::is_control) {
        bail!("{field} cannot contain control characters");
    }
    Ok(())
}

fn default_column_color(index: usize) -> &'static str {
    const COLORS: [&str; 6] = ["gray", "blue", "yellow", "magenta", "green", "cyan"];
    COLORS[index % COLORS.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_config_uses_first_status_as_default() {
        let config = BoardConfig::new("Demo", "demo", vec!["Inbox".to_owned(), "Done".to_owned()]);

        assert_eq!(config.default_status, "Inbox");
        assert_eq!(config.id_prefix, "DEMO");
        assert_eq!(config.resolve_status("done"), Some("Done"));
        config.validate().unwrap();
    }

    #[test]
    fn config_rejects_escaping_cards_directory() {
        let mut config = BoardConfig::new("Demo", "KB", vec!["Todo".to_owned()]);
        config.cards_dir = "../outside".to_owned();

        assert!(
            config
                .validate()
                .unwrap_err()
                .to_string()
                .contains("cards_dir")
        );
    }

    #[test]
    fn config_rejects_case_insensitive_duplicate_columns() {
        let config = BoardConfig::new("Demo", "KB", vec!["Doing".to_owned(), "doing".to_owned()]);

        assert!(
            config
                .validate()
                .unwrap_err()
                .to_string()
                .contains("duplicate")
        );
    }

    #[test]
    fn new_config_normalizes_user_supplied_boundaries() {
        let config = BoardConfig::new(" Demo ", " kb ", vec![" Todo ".to_owned()]);
        assert_eq!(config.name, "Demo");
        assert_eq!(config.id_prefix, "KB");
        assert_eq!(config.columns[0].name, "Todo");
        config.validate().unwrap();
    }

    #[test]
    fn config_rejects_padded_values_loaded_from_yaml() {
        let mut config = BoardConfig::new("Demo", "KB", vec!["Todo".to_owned(), "Done".to_owned()]);
        config.columns[1].name = " Done ".to_owned();
        assert!(
            config
                .validate()
                .unwrap_err()
                .to_string()
                .contains("whitespace")
        );

        config.columns[1].name = "Done".to_owned();
        config.id_prefix = " KB ".to_owned();
        assert!(
            config
                .validate()
                .unwrap_err()
                .to_string()
                .contains("id_prefix")
        );
    }

    #[test]
    fn config_rejects_multiline_display_values() {
        let mut config = BoardConfig::new("Demo", "KB", vec!["Todo".to_owned()]);
        config.name = "Demo\nspoofed".to_owned();
        assert!(
            config
                .validate()
                .unwrap_err()
                .to_string()
                .contains("control")
        );

        config.name = "Demo".to_owned();
        config.columns[0].name = "Todo\rDone".to_owned();
        assert!(
            config
                .validate()
                .unwrap_err()
                .to_string()
                .contains("control")
        );
    }
}
