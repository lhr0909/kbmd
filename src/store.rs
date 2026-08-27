//! Project discovery and concurrency-safe card persistence.

use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use atomicwrites::{AllowOverwrite, AtomicFile, DisallowOverwrite};
use fs4::FileExt;
use serde::Serialize;
use serde_json::Value;

use crate::config::BoardConfig;
use crate::frontmatter;
use crate::model::{Card, CardMetadata};

pub const INTERNAL_DIR: &str = ".kbmd";
pub const CONFIG_FILE: &str = "config.yml";
const LOCK_FILE: &str = ".lock";
const ORDER_STEP: i64 = 1_024;

#[derive(Clone, Debug)]
pub struct Project {
    pub root: PathBuf,
    pub internal_dir: PathBuf,
    pub config_path: PathBuf,
    pub cards_dir: PathBuf,
    pub config: BoardConfig,
}

#[derive(Clone, Debug, Default)]
pub struct CreateCard {
    pub title: String,
    pub status: Option<String>,
    pub body: String,
    pub labels: Vec<String>,
    pub assignees: Vec<String>,
    pub due_date: Option<String>,
    pub extra: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ValidationIssue {
    pub path: PathBuf,
    pub message: String,
}

struct ProjectLock {
    file: File,
}

impl Project {
    pub fn init(root: &Path, config: &BoardConfig) -> Result<Self> {
        config.validate()?;
        let root = absolute_path(root)?;
        let internal_dir = root.join(INTERNAL_DIR);
        let config_path = internal_dir.join(CONFIG_FILE);
        if config_path.exists() {
            bail!("a kbmd project already exists at {}", root.display());
        }

        let cards_dir = internal_dir.join(&config.cards_dir);
        fs::create_dir_all(&cards_dir)
            .with_context(|| format!("could not create {}", cards_dir.display()))?;
        atomic_write_new(&internal_dir.join(".gitignore"), b".lock\n*.tmp\n")?;
        let rendered = serialize_yaml(&config)?;
        atomic_write_new(&config_path, rendered.as_bytes())?;

        Self::open(&root)
    }

    /// Find the nearest `.kbmd/config.yml`, starting at `start` and walking upward.
    pub fn discover(start: &Path) -> Result<Self> {
        let start = absolute_path(start)?;
        let start = if start.is_file() {
            start.parent().unwrap_or(&start).to_path_buf()
        } else {
            start
        };

        for candidate in start.ancestors() {
            if candidate.join(INTERNAL_DIR).join(CONFIG_FILE).is_file() {
                return Self::open(candidate);
            }
        }
        bail!(
            "no kbmd project found from {}; run `kbmd init` first",
            start.display()
        )
    }

    pub fn open(root: &Path) -> Result<Self> {
        let root = absolute_path(root)?;
        let internal_dir = root.join(INTERNAL_DIR);
        let config_path = internal_dir.join(CONFIG_FILE);
        let source = fs::read_to_string(&config_path)
            .with_context(|| format!("could not read {}", config_path.display()))?;
        let config: BoardConfig =
            serde_saphyr::from_str(&source).context("invalid kbmd config YAML")?;
        config.validate()?;
        let cards_dir = internal_dir.join(&config.cards_dir);
        fs::create_dir_all(&cards_dir)
            .with_context(|| format!("could not create {}", cards_dir.display()))?;

        Ok(Self {
            root,
            internal_dir,
            config_path,
            cards_dir,
            config,
        })
    }

    pub fn load_cards(&self) -> Result<Vec<Card>> {
        let mut cards = self.load_cards_unvalidated()?;
        validate_collection(&cards, &self.config)?;
        sort_cards(&mut cards, &self.config);
        Ok(cards)
    }

    pub fn load_card(&self, id: &str) -> Result<Card> {
        let cards = self.load_cards_unvalidated()?;
        find_unique_card(cards, id)
    }

    pub fn create_card(&self, input: CreateCard) -> Result<Card> {
        if input.title.trim().is_empty() {
            bail!("card title cannot be empty");
        }
        let _lock = self.lock()?;
        // Refresh inside the lock so status and ID allocation use the latest config and files.
        let project = Self::open(&self.root)?;
        let cards = project.load_cards_unvalidated()?;
        validate_collection(&cards, &project.config)?;

        let status = input
            .status
            .as_deref()
            .unwrap_or(&project.config.default_status);
        let status = project
            .config
            .resolve_status(status)
            .ok_or_else(|| unknown_status(status, &project.config))?
            .to_owned();
        enforce_wip_limit(&cards, &project.config, &status, None)?;

        let number = next_id_number(&cards, &project.config.id_prefix)?;
        let id = format!("{}-{number}", project.config.id_prefix.to_ascii_uppercase());
        let ordinal = next_ordinal(&cards, &status)?;
        let mut metadata =
            CardMetadata::new(id.clone(), input.title.trim().to_owned(), status, ordinal);
        metadata.labels = unique_nonempty(input.labels);
        metadata.assignees = unique_nonempty(input.assignees);
        metadata.due_date = input.due_date.filter(|value| !value.trim().is_empty());
        metadata.extra = input.extra;
        reject_reserved_extra_keys(&metadata.extra)?;
        metadata.validate()?;

        let path = project.cards_dir.join(format!("{id}.md"));
        let rendered = frontmatter::serialize(&metadata, &input.body)?;
        atomic_write_new(&path, rendered.as_bytes())?;
        Ok(Card {
            metadata,
            body: input.body.trim_matches(['\r', '\n']).to_owned(),
            path,
        })
    }

    /// Lock, re-read, narrowly mutate, validate, and atomically replace one card.
    pub fn update_card<F>(&self, id: &str, mutate: F) -> Result<Card>
    where
        F: FnOnce(&mut Card) -> Result<()>,
    {
        let _lock = self.lock()?;
        let project = Self::open(&self.root)?;
        let cards = project.load_cards_unvalidated()?;
        validate_collection(&cards, &project.config)?;
        let mut card = find_unique_card(cards.clone(), id)?;
        let previous_status = card.metadata.status.clone();
        let original = fs::read_to_string(&card.path)
            .with_context(|| format!("could not re-read {}", card.path.display()))?;

        mutate(&mut card)?;
        card.metadata.title = card.metadata.title.trim().to_owned();
        card.metadata.validate()?;
        reject_reserved_extra_keys(&card.metadata.extra)?;
        let canonical_status = project
            .config
            .resolve_status(&card.metadata.status)
            .ok_or_else(|| unknown_status(&card.metadata.status, &project.config))?
            .to_owned();
        card.metadata.status.clone_from(&canonical_status);
        if !previous_status.eq_ignore_ascii_case(&canonical_status) {
            enforce_wip_limit(
                &cards,
                &project.config,
                &canonical_status,
                Some(&card.metadata.id),
            )?;
            card.metadata.ordinal = Some(next_ordinal(&cards, &canonical_status)?);
        }
        card.metadata.labels = unique_nonempty(card.metadata.labels);
        card.metadata.assignees = unique_nonempty(card.metadata.assignees);
        card.metadata.touch();

        let rendered = frontmatter::serialize(&card.metadata, &card.body)?;
        atomic_replace_if_unchanged(&card.path, original.as_bytes(), rendered.as_bytes())?;
        card.body = card.body.trim_matches(['\r', '\n']).to_owned();
        Ok(card)
    }

    pub fn move_card(&self, id: &str, status: &str) -> Result<Card> {
        let status = status.to_owned();
        self.update_card(id, |card| {
            card.metadata.status.clone_from(&status);
            Ok(())
        })
    }

    pub fn validate(&self) -> Vec<ValidationIssue> {
        let paths = match card_paths(&self.cards_dir) {
            Ok(paths) => paths,
            Err(error) => {
                return vec![ValidationIssue {
                    path: self.cards_dir.clone(),
                    message: format!("{error:#}"),
                }];
            }
        };
        let mut cards = Vec::new();
        let mut issues = Vec::new();
        for path in paths {
            match load_card_path(&path) {
                Ok(card) => cards.push(card),
                Err(error) => issues.push(ValidationIssue {
                    path,
                    message: format!("{error:#}"),
                }),
            }
        }

        let mut ids: HashMap<String, Vec<&Card>> = HashMap::new();
        for card in &cards {
            ids.entry(card.metadata.id.to_ascii_lowercase())
                .or_default()
                .push(card);
            if self.config.resolve_status(&card.metadata.status).is_none() {
                issues.push(ValidationIssue {
                    path: card.path.clone(),
                    message: unknown_status(&card.metadata.status, &self.config).to_string(),
                });
            }
        }
        for duplicates in ids.values().filter(|matches| matches.len() > 1) {
            for card in duplicates {
                issues.push(ValidationIssue {
                    path: card.path.clone(),
                    message: format!(
                        "duplicate card id {:?} appears in {} files",
                        card.metadata.id,
                        duplicates.len()
                    ),
                });
            }
        }
        issues.sort_by(|left, right| left.path.cmp(&right.path));
        issues
    }

    fn load_cards_unvalidated(&self) -> Result<Vec<Card>> {
        card_paths(&self.cards_dir)?
            .into_iter()
            .map(|path| load_card_path(&path))
            .collect()
    }

    fn lock(&self) -> Result<ProjectLock> {
        let path = self.internal_dir.join(LOCK_FILE);
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .with_context(|| format!("could not open project lock {}", path.display()))?;
        FileExt::lock(&file).with_context(|| format!("could not lock {}", path.display()))?;
        Ok(ProjectLock { file })
    }
}

impl Drop for ProjectLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

fn absolute_path(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()
            .context("could not read the current directory")?
            .join(path))
    }
}

fn serialize_yaml<T: Serialize>(value: &T) -> Result<String> {
    serde_saphyr::to_string(value).context("could not serialize YAML")
}

fn card_paths(cards_dir: &Path) -> Result<Vec<PathBuf>> {
    let entries = fs::read_dir(cards_dir)
        .with_context(|| format!("could not read cards directory {}", cards_dir.display()))?;
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.with_context(|| format!("could not inspect {}", cards_dir.display()))?;
        if !entry
            .file_type()
            .with_context(|| format!("could not inspect {}", entry.path().display()))?
            .is_file()
        {
            continue;
        }
        let path = entry.path();
        if path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
        {
            paths.push(path);
        }
    }
    paths.sort();
    Ok(paths)
}

fn load_card_path(path: &Path) -> Result<Card> {
    let source = fs::read_to_string(path)
        .with_context(|| format!("could not read card {}", path.display()))?;
    let document = frontmatter::parse::<CardMetadata>(&source)
        .with_context(|| format!("could not parse card {}", path.display()))?;
    document
        .metadata
        .validate()
        .with_context(|| format!("invalid card {}", path.display()))?;
    reject_reserved_extra_keys(&document.metadata.extra)?;
    Ok(Card {
        metadata: document.metadata,
        body: document.body,
        path: path.to_path_buf(),
    })
}

fn validate_collection(cards: &[Card], config: &BoardConfig) -> Result<()> {
    let mut ids = HashMap::<String, Vec<&Path>>::new();
    for card in cards {
        ids.entry(card.metadata.id.to_ascii_lowercase())
            .or_default()
            .push(&card.path);
        if config.resolve_status(&card.metadata.status).is_none() {
            return Err(unknown_status(&card.metadata.status, config))
                .with_context(|| format!("in {}", card.path.display()));
        }
    }
    if let Some((id, paths)) = ids.into_iter().find(|(_, paths)| paths.len() > 1) {
        let rendered = paths
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        bail!("duplicate card id {id:?} in: {rendered}");
    }
    Ok(())
}

fn find_unique_card(cards: Vec<Card>, id: &str) -> Result<Card> {
    let matches = cards
        .into_iter()
        .filter(|card| card.metadata.id.eq_ignore_ascii_case(id.trim()))
        .collect::<Vec<_>>();
    match matches.len() {
        0 => bail!("card {:?} was not found", id.trim()),
        1 => Ok(matches.into_iter().next().expect("length checked")),
        count => bail!("card id {:?} is ambiguous across {count} files", id.trim()),
    }
}

fn next_id_number(cards: &[Card], prefix: &str) -> Result<u64> {
    let prefix = format!("{}-", prefix.to_ascii_lowercase());
    let maximum = cards
        .iter()
        .filter_map(|card| {
            let id = card.metadata.id.to_ascii_lowercase();
            id.strip_prefix(&prefix)?.parse::<u64>().ok()
        })
        .max()
        .unwrap_or(0);
    maximum
        .checked_add(1)
        .ok_or_else(|| anyhow!("card ID space is exhausted"))
}

fn next_ordinal(cards: &[Card], status: &str) -> Result<i64> {
    let maximum = cards
        .iter()
        .filter(|card| card.metadata.status.eq_ignore_ascii_case(status))
        .filter_map(|card| card.metadata.ordinal)
        .max()
        .unwrap_or(0);
    maximum
        .checked_add(ORDER_STEP)
        .ok_or_else(|| anyhow!("card ordering value is exhausted in status {status:?}"))
}

fn enforce_wip_limit(
    cards: &[Card],
    config: &BoardConfig,
    status: &str,
    moving_id: Option<&str>,
) -> Result<()> {
    let Some(column) = config
        .columns
        .iter()
        .find(|column| column.name.eq_ignore_ascii_case(status))
    else {
        return Err(unknown_status(status, config));
    };
    let Some(limit) = column.wip_limit else {
        return Ok(());
    };
    let count = cards
        .iter()
        .filter(|card| {
            card.metadata.status.eq_ignore_ascii_case(status)
                && moving_id.is_none_or(|id| !card.metadata.id.eq_ignore_ascii_case(id))
        })
        .count();
    if count >= limit {
        bail!(
            "column {:?} has reached its WIP limit of {limit}",
            column.name
        );
    }
    Ok(())
}

fn unknown_status(status: &str, config: &BoardConfig) -> anyhow::Error {
    let valid = config
        .columns
        .iter()
        .map(|column| column.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    anyhow!("unknown status {status:?}; expected one of: {valid}")
}

fn unique_nonempty(values: Vec<String>) -> Vec<String> {
    let mut result = Vec::new();
    for value in values {
        let value = value.trim();
        if !value.is_empty() && !result.iter().any(|existing: &String| existing == value) {
            result.push(value.to_owned());
        }
    }
    result
}

fn reject_reserved_extra_keys(extra: &BTreeMap<String, Value>) -> Result<()> {
    const RESERVED: [&str; 10] = [
        "id",
        "title",
        "status",
        "labels",
        "assignee",
        "assignees",
        "due_date",
        "ordinal",
        "created_date",
        "updated_date",
    ];
    if let Some(key) = extra.keys().find(|key| {
        RESERVED
            .iter()
            .any(|reserved| key.eq_ignore_ascii_case(reserved))
    }) {
        bail!("custom field {key:?} conflicts with a reserved card field");
    }
    Ok(())
}

fn sort_cards(cards: &mut [Card], config: &BoardConfig) {
    cards.sort_by(|left, right| {
        config
            .column_index(&left.metadata.status)
            .unwrap_or(usize::MAX)
            .cmp(
                &config
                    .column_index(&right.metadata.status)
                    .unwrap_or(usize::MAX),
            )
            .then_with(|| {
                left.metadata
                    .ordinal
                    .unwrap_or(i64::MAX)
                    .cmp(&right.metadata.ordinal.unwrap_or(i64::MAX))
            })
            .then_with(|| left.metadata.id.cmp(&right.metadata.id))
    });
}

fn atomic_write_new(path: &Path, contents: &[u8]) -> Result<()> {
    if path.exists() {
        bail!("refusing to overwrite existing file {}", path.display());
    }
    AtomicFile::new(path, DisallowOverwrite)
        .write(|file| file.write_all(contents))
        .map_err(std::io::Error::from)
        .with_context(|| format!("could not create {}", path.display()))?;
    Ok(())
}

fn atomic_replace_if_unchanged(path: &Path, expected: &[u8], contents: &[u8]) -> Result<()> {
    let current =
        fs::read(path).with_context(|| format!("could not re-read {}", path.display()))?;
    if current != expected {
        bail!(
            "{} changed during the update; no data was overwritten, so retry the command",
            path.display()
        );
    }
    AtomicFile::new(path, AllowOverwrite)
        .write(|file| file.write_all(contents))
        .map_err(std::io::Error::from)
        .with_context(|| format!("could not replace {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Barrier};
    use std::thread;

    use pretty_assertions::assert_eq;
    use tempfile::tempdir;

    use super::*;

    fn project() -> (tempfile::TempDir, Project) {
        let directory = tempdir().unwrap();
        let config = BoardConfig::new(
            "Test board",
            "KB",
            vec!["Todo".to_owned(), "Doing".to_owned(), "Done".to_owned()],
        );
        let project = Project::init(directory.path(), &config).unwrap();
        (directory, project)
    }

    #[test]
    fn initializes_and_discovers_from_a_child_directory() {
        let (directory, project) = project();
        let child = directory.path().join("src/deep");
        fs::create_dir_all(&child).unwrap();

        let discovered = Project::discover(&child).unwrap();
        assert_eq!(discovered.root, project.root);
        assert!(project.config_path.is_file());
        assert_eq!(
            fs::read_to_string(project.internal_dir.join(".gitignore")).unwrap(),
            ".lock\n*.tmp\n"
        );
    }

    #[test]
    fn creates_loads_and_moves_a_card() {
        let (_directory, project) = project();
        let created = project
            .create_card(CreateCard {
                title: "A flexible card".to_owned(),
                body: "## Release checklist\n\n- [ ] macOS\n".to_owned(),
                labels: vec!["mvp".to_owned()],
                ..CreateCard::default()
            })
            .unwrap();

        assert_eq!(created.metadata.id, "KB-1");
        assert_eq!(created.metadata.status, "Todo");
        assert!(created.path.ends_with("KB-1.md"));
        assert_eq!(project.load_cards().unwrap().len(), 1);

        let moved = project.move_card("kb-1", "doing").unwrap();
        assert_eq!(moved.metadata.status, "Doing");
        assert!(
            fs::read_to_string(moved.path)
                .unwrap()
                .contains("status: Doing")
        );
    }

    #[test]
    fn preserves_nested_custom_frontmatter_and_unrelated_body() {
        let (_directory, project) = project();
        let mut extra = BTreeMap::new();
        extra.insert("estimate".to_owned(), serde_json::json!({"points": 3}));
        let created = project
            .create_card(CreateCard {
                title: "Custom".to_owned(),
                body: "## Strange section\n\nNever discard this.\n".to_owned(),
                extra,
                ..CreateCard::default()
            })
            .unwrap();

        project.move_card(&created.metadata.id, "Done").unwrap();
        let loaded = project.load_card("KB-1").unwrap();
        assert_eq!(
            loaded.metadata.extra["estimate"],
            serde_json::json!({"points": 3})
        );
        assert_eq!(loaded.body, "## Strange section\n\nNever discard this.\n");
    }

    #[test]
    fn concurrent_creates_allocate_unique_ids() {
        let (_directory, project) = project();
        let project = Arc::new(project);
        let barrier = Arc::new(Barrier::new(8));
        let handles = (0..8)
            .map(|index| {
                let project = Arc::clone(&project);
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    project
                        .create_card(CreateCard {
                            title: format!("Card {index}"),
                            ..CreateCard::default()
                        })
                        .unwrap()
                        .metadata
                        .id
                })
            })
            .collect::<Vec<_>>();
        let mut ids = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>();
        ids.sort();
        ids.dedup();

        assert_eq!(ids.len(), 8);
        assert_eq!(project.load_cards().unwrap().len(), 8);
    }

    #[test]
    fn doctor_reports_broken_and_duplicate_cards() {
        let (_directory, project) = project();
        let card = project
            .create_card(CreateCard {
                title: "Valid".to_owned(),
                ..CreateCard::default()
            })
            .unwrap();
        fs::copy(&card.path, project.cards_dir.join("copy.md")).unwrap();
        fs::write(project.cards_dir.join("broken.md"), "not frontmatter").unwrap();

        let issues = project.validate();
        assert_eq!(issues.len(), 3);
        assert!(
            issues
                .iter()
                .any(|issue| issue.message.contains("duplicate"))
        );
        assert!(
            issues
                .iter()
                .any(|issue| issue.message.contains("frontmatter"))
        );
    }

    #[test]
    fn wip_limits_are_enforced() {
        let (_directory, project) = project();
        let mut config = project.config.clone();
        config.columns[1].wip_limit = Some(1);
        fs::write(&project.config_path, serialize_yaml(&config).unwrap()).unwrap();
        let project = Project::open(&project.root).unwrap();
        let first = project
            .create_card(CreateCard {
                title: "First".to_owned(),
                status: Some("Doing".to_owned()),
                ..CreateCard::default()
            })
            .unwrap();
        let second = project
            .create_card(CreateCard {
                title: "Second".to_owned(),
                ..CreateCard::default()
            })
            .unwrap();

        assert!(project.move_card(&second.metadata.id, "Doing").is_err());
        assert_eq!(
            project
                .load_card(&first.metadata.id)
                .unwrap()
                .metadata
                .status,
            "Doing"
        );
    }
}
