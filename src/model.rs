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
        if self.title.trim().is_empty() {
            bail!("card title cannot be empty");
        }
        if self.status.trim().is_empty() {
            bail!("card status cannot be empty");
        }
        Ok(())
    }

    pub fn touch(&mut self) {
        self.updated_date = now();
    }
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
