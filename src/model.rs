use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Result, bail};
use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Canonical card fields. Unknown fields are retained in `extra` when a card is rewritten.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CardMetadata {
    pub id: String,
    pub title: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
    #[serde(
        default,
        alias = "assignee",
        deserialize_with = "deserialize_one_or_many",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub assignees: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due_date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ordinal: Option<i64>,
    #[serde(default = "now", skip_serializing_if = "String::is_empty")]
    pub created_date: String,
    #[serde(default = "now", skip_serializing_if = "String::is_empty")]
    pub updated_date: String,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// A card document, including its unopinionated Markdown body.
#[derive(Clone, Debug, PartialEq)]
pub struct Card {
    pub metadata: CardMetadata,
    pub body: String,
    pub path: PathBuf,
}

impl CardMetadata {
    pub fn new(id: String, title: String, status: String, ordinal: i64) -> Self {
        let timestamp = now();
        Self {
            id,
            title,
            status,
            labels: Vec::new(),
            assignees: Vec::new(),
            due_date: None,
            ordinal: Some(ordinal),
            created_date: timestamp.clone(),
            updated_date: timestamp,
            extra: BTreeMap::new(),
        }
    }

    pub fn validate(&self) -> Result<()> {
        if self.id.trim().is_empty() {
            bail!("card id cannot be empty");
        }
        if self.id != self.id.trim() {
            bail!("card id cannot start or end with whitespace");
        }
        reject_control_characters("card id", &self.id)?;
        if self.title.trim().is_empty() {
            bail!("card title cannot be empty");
        }
        reject_control_characters("card title", &self.title)?;
        if self.status.trim().is_empty() {
            bail!("card status cannot be empty");
        }
        if self.status != self.status.trim() {
            bail!("card status cannot start or end with whitespace");
        }
        reject_control_characters("card status", &self.status)?;
        for label in &self.labels {
            reject_control_characters("card labels", label)?;
        }
        for assignee in &self.assignees {
            reject_control_characters("card assignees", assignee)?;
        }
        if let Some(due_date) = &self.due_date {
            reject_control_characters("card due date", due_date)?;
        }
        Ok(())
    }

    pub fn touch(&mut self) {
        self.updated_date = now();
    }
}

fn reject_control_characters(field: &str, value: &str) -> Result<()> {
    if value.chars().any(char::is_control) {
        bail!("{field} cannot contain control characters");
    }
    Ok(())
}

impl Card {
    pub fn checklist_progress(&self) -> (usize, usize) {
        let items = crate::markdown::checklist_items(&self.body);
        let checked = items.iter().filter(|item| item.checked).count();
        (checked, items.len())
    }
}

pub fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn deserialize_one_or_many<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        One(String),
        Many(Vec<String>),
    }

    Ok(match Option::<OneOrMany>::deserialize(deserializer)? {
        None => Vec::new(),
        Some(OneOrMany::One(value)) => vec![value],
        Some(OneOrMany::Many(values)) => values,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn card_identity_and_status_reject_boundary_whitespace() {
        let mut metadata =
            CardMetadata::new("KB-1".to_owned(), "Card".to_owned(), "Todo".to_owned(), 1);
        metadata.id = " KB-1 ".to_owned();
        assert!(metadata.validate().unwrap_err().to_string().contains("id"));

        metadata.id = "KB-1".to_owned();
        metadata.status = " Todo ".to_owned();
        assert!(
            metadata
                .validate()
                .unwrap_err()
                .to_string()
                .contains("status")
        );
    }

    #[test]
    fn card_display_fields_reject_control_characters() {
        let mut metadata =
            CardMetadata::new("KB-1".to_owned(), "Card".to_owned(), "Todo".to_owned(), 1);
        metadata.title = "First line\nsecond line".to_owned();
        assert!(
            metadata
                .validate()
                .unwrap_err()
                .to_string()
                .contains("title")
        );

        metadata.title = "Card".to_owned();
        metadata.labels = vec!["safe".to_owned(), "unsafe\rlabel".to_owned()];
        assert!(
            metadata
                .validate()
                .unwrap_err()
                .to_string()
                .contains("labels")
        );
    }
}
