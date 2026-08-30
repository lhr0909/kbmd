//! Command-line interface over the shared project model.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use serde::Serialize;
use serde_json::Value;

use crate::config::BoardConfig;
use crate::markdown;
use crate::model::Card;
use crate::store::{CreateCard, Project};

const JSON_SCHEMA_VERSION: u8 = 1;

#[derive(Debug, Parser)]
#[command(
    name = "kbmd",
    version,
    about = "A flexible, Markdown-native kanban",
    long_about = "Manage local YAML-frontmatter Markdown cards from a scriptable CLI or a mouse-friendly live terminal board. Card bodies, sections, checklists, and non-reserved frontmatter are user-defined."
)]
pub struct Cli {
    /// Project directory or any path below it.
    #[arg(
        short = 'p',
        long,
        global = true,
        default_value = ".",
        value_name = "PATH"
    )]
    project: PathBuf,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Open the live keyboard- and mouse-driven terminal board.
    Tui,
    /// Initialize `.kbmd` in a directory.
    Init(InitArgs),
    /// Create a card.
    #[command(alias = "new")]
    Add(AddArgs),
    /// List cards.
    List(ListArgs),
    /// Show one card.
    Show(ShowArgs),
    /// Edit canonical card metadata.
    Edit(EditArgs),
    /// Move a card to another configured column.
    Move(MoveArgs),
    /// Read or mutate arbitrary frontmatter fields.
    Field(FieldArgs),
    /// Read or mutate any Markdown section.
    Section(SectionArgs),
    /// Read or mutate checklists in any section.
    #[command(alias = "checklist")]
    Check(CheckArgs),
    /// Print the board grouped into columns.
    Board(BoardArgs),
    /// Validate config and all card files.
    #[command(alias = "doctor")]
    Validate,
}

#[derive(Debug, Args)]
struct InitArgs {
    /// Board display name. Defaults to the directory name.
    name: Option<String>,
    /// Ordered column names.
    #[arg(
        long,
        value_delimiter = ',',
        default_value = "Backlog,Ready,Doing,Done",
        value_name = "NAME,..."
    )]
    statuses: Vec<String>,
    /// Prefix for generated human-readable IDs.
    #[arg(long, default_value = "KB")]
    prefix: String,
}

#[derive(Debug, Args)]
struct AddArgs {
    title: String,
    #[arg(long)]
    status: Option<String>,
    #[arg(long = "label")]
    labels: Vec<String>,
    #[arg(long = "assignee")]
    assignees: Vec<String>,
    /// Complete Markdown body.
    #[arg(long, conflicts_with = "body_file", allow_hyphen_values = true)]
    body: Option<String>,
    /// Read the complete Markdown body from a file, or `-` for stdin.
    #[arg(long, value_name = "PATH", conflicts_with = "body")]
    body_file: Option<PathBuf>,
    /// Add a custom section as `HEADING=MARKDOWN`. May be repeated.
    #[arg(long = "section", value_name = "HEADING=MARKDOWN")]
    sections: Vec<String>,
    /// Add an unchecked item as `HEADING=TEXT`. May be repeated.
    #[arg(long = "check", value_name = "HEADING=TEXT")]
    checks: Vec<String>,
    /// Add a string frontmatter field as `KEY=VALUE`. May be repeated.
    #[arg(long = "field", value_name = "KEY=VALUE")]
    fields: Vec<String>,
    /// Add a typed YAML frontmatter field as `KEY=YAML`. May be repeated.
    #[arg(long = "field-yaml", value_name = "KEY=YAML")]
    yaml_fields: Vec<String>,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct ListArgs {
    #[arg(long)]
    status: Option<String>,
    #[arg(long)]
    label: Option<String>,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct ShowArgs {
    id: String,
    /// Print the card file without interpretation.
    #[arg(long, conflicts_with = "json")]
    raw: bool,
    #[arg(long)]
    json: bool,
    /// Print only the filesystem path.
    #[arg(long, conflicts_with_all = ["raw", "json"])]
    path: bool,
}

#[derive(Debug, Args)]
struct EditArgs {
    id: String,
    #[arg(long)]
    title: Option<String>,
    #[arg(long)]
    status: Option<String>,
    #[arg(long = "add-label")]
    add_labels: Vec<String>,
    #[arg(long = "remove-label")]
    remove_labels: Vec<String>,
    #[arg(long = "add-assignee")]
    add_assignees: Vec<String>,
    #[arg(long = "remove-assignee")]
    remove_assignees: Vec<String>,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct MoveArgs {
    id: String,
    status: String,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct FieldArgs {
    #[command(subcommand)]
    command: FieldCommand,
}

#[derive(Debug, Subcommand)]
enum FieldCommand {
    /// List all custom fields.
    List {
        id: String,
        #[arg(long)]
        json: bool,
    },
    /// Print one custom field.
    Get { id: String, key: String },
    /// Set a custom field. Values are strings unless `--yaml` is used.
    Set {
        id: String,
        key: String,
        #[arg(allow_hyphen_values = true)]
        value: String,
        #[arg(long)]
        yaml: bool,
    },
    /// Remove a custom field.
    Unset { id: String, key: String },
}

#[derive(Debug, Args)]
struct SectionArgs {
    #[command(subcommand)]
    command: SectionCommand,
}

#[derive(Debug, Subcommand)]
enum SectionCommand {
    /// List headings discovered in the card body.
    List {
        id: String,
        #[arg(long)]
        json: bool,
    },
    /// Print one section body.
    Show { id: String, heading: String },
    /// Create or replace one section.
    Set(SectionContentArgs),
    /// Append Markdown to a section, creating it if absent.
    Append(SectionContentArgs),
    /// Remove a section.
    Remove { id: String, heading: String },
}

#[derive(Debug, Args)]
struct SectionContentArgs {
    id: String,
    heading: String,
    /// Markdown content. Omit it for an empty section or when using `--file`.
    #[arg(allow_hyphen_values = true)]
    content: Option<String>,
    /// Read Markdown from a file, or `-` for stdin.
    #[arg(long, value_name = "PATH", conflicts_with = "content")]
    file: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct CheckArgs {
    #[command(subcommand)]
    command: CheckCommand,
}

#[derive(Debug, Subcommand)]
enum CheckCommand {
    /// List every checklist item, optionally restricted to a section.
    List {
        id: String,
        #[arg(long)]
        section: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Add an unchecked item to any section.
    Add {
        id: String,
        section: String,
        #[arg(allow_hyphen_values = true)]
        text: String,
    },
    /// Invert an item's state. Indexes are one-based within the section.
    Toggle {
        id: String,
        section: String,
        index: usize,
    },
    /// Mark an item checked.
    Check {
        id: String,
        section: String,
        index: usize,
    },
    /// Mark an item unchecked.
    Uncheck {
        id: String,
        section: String,
        index: usize,
    },
    /// Remove an item.
    Remove {
        id: String,
        section: String,
        index: usize,
    },
    /// Invert an item by its global document-order index.
    ToggleGlobal { id: String, index: usize },
    /// Mark an item checked by its global document-order index.
    CheckGlobal { id: String, index: usize },
    /// Mark an item unchecked by its global document-order index.
    UncheckGlobal { id: String, index: usize },
    /// Remove an item by its global document-order index.
    RemoveGlobal { id: String, index: usize },
}

#[derive(Debug, Args)]
struct BoardArgs {
    #[arg(long)]
    json: bool,
}

#[derive(Serialize)]
struct JsonEnvelope<T> {
    schema_version: u8,
    data: T,
}

#[derive(Serialize)]
struct CardOutput<'a> {
    metadata: &'a crate::model::CardMetadata,
    body: &'a str,
    path: &'a Path,
    checklist: Progress,
}

#[derive(Clone, Copy, Serialize)]
struct Progress {
    checked: usize,
    total: usize,
}

#[derive(Serialize)]
struct BoardOutput<'a> {
    name: &'a str,
    columns: Vec<ColumnOutput<'a>>,
}

#[derive(Serialize)]
struct ColumnOutput<'a> {
    name: &'a str,
    cards: Vec<CardOutput<'a>>,
}

pub fn run() -> Result<()> {
    run_with(Cli::parse())
}

pub fn run_with(cli: Cli) -> Result<()> {
    match cli.command {
        None | Some(Command::Tui) => command_tui(&cli.project),
        Some(Command::Init(arguments)) => command_init(&cli.project, arguments),
        Some(Command::Add(arguments)) => command_add(&cli.project, arguments),
        Some(Command::List(arguments)) => command_list(&cli.project, arguments),
        Some(Command::Show(arguments)) => command_show(&cli.project, arguments),
        Some(Command::Edit(arguments)) => command_edit(&cli.project, arguments),
        Some(Command::Move(arguments)) => command_move(&cli.project, arguments),
        Some(Command::Field(arguments)) => command_field(&cli.project, arguments.command),
        Some(Command::Section(arguments)) => command_section(&cli.project, arguments.command),
        Some(Command::Check(arguments)) => command_check(&cli.project, arguments.command),
        Some(Command::Board(arguments)) => command_board(&cli.project, arguments),
        Some(Command::Validate) => command_validate(&cli.project),
    }
}

fn command_tui(start: &Path) -> Result<()> {
    let project = Project::discover(start)?;
    crate::tui::run(project)
}

fn command_init(root: &Path, arguments: InitArgs) -> Result<()> {
    let absolute_root = if root.is_absolute() {
        root.to_path_buf()
    } else {
        std::env::current_dir()
            .context("could not read the current directory")?
            .join(root)
    };
    let name = arguments.name.unwrap_or_else(|| {
        absolute_root
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .unwrap_or("Kanban")
            .to_owned()
    });
    let statuses = arguments
        .statuses
        .into_iter()
        .map(|status| status.trim().to_owned())
        .collect();
    let config = BoardConfig::new(name, arguments.prefix, statuses);
    let project = Project::init(root, &config)?;
    println!(
        "Initialized {} at {}",
        project.config.name,
        project.internal_dir.display()
    );
    Ok(())
}

fn command_add(start: &Path, arguments: AddArgs) -> Result<()> {
    let project = Project::discover(start)?;
    let mut body = read_optional_content(arguments.body, arguments.body_file)?.unwrap_or_default();
    for definition in arguments.sections {
        let (heading, content) = split_assignment(&definition, "section")?;
        body = markdown::set_section(&body, heading, content)?;
    }
    for definition in arguments.checks {
        let (heading, text) = split_assignment(&definition, "check")?;
        body = markdown::add_checklist_item(&body, heading, text)?;
    }

    let mut extra = BTreeMap::new();
    for definition in arguments.fields {
        let (key, value) = split_assignment(&definition, "field")?;
        ensure_custom_key(key)?;
        extra.insert(key.to_owned(), Value::String(value.to_owned()));
    }
    for definition in arguments.yaml_fields {
        let (key, value) = split_assignment(&definition, "field-yaml")?;
        ensure_custom_key(key)?;
        extra.insert(key.to_owned(), parse_yaml_value(value)?);
    }

    let card = project.create_card(CreateCard {
        title: arguments.title,
        status: arguments.status,
        body,
        labels: arguments.labels,
        assignees: arguments.assignees,
        extra,
    })?;
    if arguments.json {
        print_card_json(&card)
    } else {
        println!("Created {} — {}", card.metadata.id, card.metadata.title);
        println!("{}", card.path.display());
        Ok(())
    }
}

fn command_list(start: &Path, arguments: ListArgs) -> Result<()> {
    let project = Project::discover(start)?;
    let canonical_status = arguments
        .status
        .as_deref()
        .map(|status| {
            project
                .config
                .resolve_status(status)
                .map(str::to_owned)
                .ok_or_else(|| anyhow::anyhow!("unknown status {status:?}"))
        })
        .transpose()?;
    let cards = project
        .load_cards()?
        .into_iter()
        .filter(|card| {
            canonical_status
                .as_deref()
                .is_none_or(|status| card.metadata.status.eq_ignore_ascii_case(status))
                && arguments.label.as_deref().is_none_or(|label| {
                    card.metadata
                        .labels
                        .iter()
                        .any(|candidate| candidate.eq_ignore_ascii_case(label))
                })
        })
        .collect::<Vec<_>>();
    if arguments.json {
        print_cards_json(&cards)
    } else {
        print_card_table(&cards);
        Ok(())
    }
}

fn command_show(start: &Path, arguments: ShowArgs) -> Result<()> {
    let project = Project::discover(start)?;
    let card = project.load_card(&arguments.id)?;
    if arguments.path {
        println!("{}", card.path.display());
        return Ok(());
    }
    if arguments.raw {
        print!("{}", fs::read_to_string(&card.path)?);
        return Ok(());
    }
    if arguments.json {
        return print_card_json(&card);
    }

    let (checked, total) = card.checklist_progress();
    println!("{} — {}", card.metadata.id, card.metadata.title);
    println!("Status: {}", card.metadata.status);
    if !card.metadata.labels.is_empty() {
        println!("Labels: {}", card.metadata.labels.join(", "));
    }
    if !card.metadata.assignees.is_empty() {
        println!("Assignees: {}", card.metadata.assignees.join(", "));
    }
    if total > 0 {
        println!("Checklist: {checked}/{total}");
    }
    if !card.metadata.extra.is_empty() {
        println!("Custom fields:");
        print!("{}", serde_saphyr::to_string(&card.metadata.extra)?);
    }
    if !card.body.is_empty() {
        println!("\n{}", card.body.trim_end());
    }
    Ok(())
}

fn command_edit(start: &Path, arguments: EditArgs) -> Result<()> {
    if arguments.title.is_none()
        && arguments.status.is_none()
        && arguments.add_labels.is_empty()
        && arguments.remove_labels.is_empty()
        && arguments.add_assignees.is_empty()
        && arguments.remove_assignees.is_empty()
    {
        bail!("no edits were supplied");
    }
    let project = Project::discover(start)?;
    let card = project.update_card(&arguments.id, |card| {
        if let Some(title) = &arguments.title {
            card.metadata.title.clone_from(title);
        }
        if let Some(status) = &arguments.status {
            card.metadata.status.clone_from(status);
        }
        extend_unique(&mut card.metadata.labels, &arguments.add_labels);
        remove_values(&mut card.metadata.labels, &arguments.remove_labels);
        extend_unique(&mut card.metadata.assignees, &arguments.add_assignees);
        remove_values(&mut card.metadata.assignees, &arguments.remove_assignees);
        Ok(())
    })?;
    print_mutation_result(&card, arguments.json, "Updated")
}

fn command_move(start: &Path, arguments: MoveArgs) -> Result<()> {
    let project = Project::discover(start)?;
    let card = project.move_card(&arguments.id, &arguments.status)?;
    print_mutation_result(&card, arguments.json, "Moved")
}

fn command_field(start: &Path, command: FieldCommand) -> Result<()> {
    let project = Project::discover(start)?;
    match command {
        FieldCommand::List { id, json } => {
            let card = project.load_card(&id)?;
            if json {
                print_json(&card.metadata.extra)
            } else if card.metadata.extra.is_empty() {
                println!("No custom fields.");
                Ok(())
            } else {
                print!("{}", serde_saphyr::to_string(&card.metadata.extra)?);
                Ok(())
            }
        }
        FieldCommand::Get { id, key } => {
            let card = project.load_card(&id)?;
            let value = get_custom_field(&card, &key)?;
            print!("{}", serde_saphyr::to_string(value)?);
            Ok(())
        }
        FieldCommand::Set {
            id,
            key,
            value,
            yaml,
        } => {
            ensure_custom_key(&key)?;
            let value = if yaml {
                parse_yaml_value(&value)?
            } else {
                Value::String(value)
            };
            let card = project.update_card(&id, |card| {
                card.metadata.extra.insert(key.clone(), value.clone());
                Ok(())
            })?;
            println!("Set {key} on {}", card.metadata.id);
            Ok(())
        }
        FieldCommand::Unset { id, key } => {
            ensure_custom_key(&key)?;
            let card = project.update_card(&id, |card| {
                if card.metadata.extra.remove(&key).is_none() {
                    bail!("custom field {key:?} was not found");
                }
                Ok(())
            })?;
            println!("Removed {key} from {}", card.metadata.id);
            Ok(())
        }
    }
}

fn command_section(start: &Path, command: SectionCommand) -> Result<()> {
    let project = Project::discover(start)?;
    match command {
        SectionCommand::List { id, json } => {
            let card = project.load_card(&id)?;
            let sections = markdown::sections(&card.body);
            if json {
                let output = sections
                    .iter()
                    .map(|section| serde_json::json!({"title": section.title, "level": section.level}))
                    .collect::<Vec<_>>();
                print_json(&output)
            } else {
                for section in sections {
                    println!("{} {}", "#".repeat(section.level.into()), section.title);
                }
                Ok(())
            }
        }
        SectionCommand::Show { id, heading } => {
            let card = project.load_card(&id)?;
            let Some(content) = markdown::section_content(&card.body, &heading)? else {
                bail!("section {heading:?} was not found");
            };
            println!("{content}");
            Ok(())
        }
        SectionCommand::Set(arguments) => {
            let content =
                read_optional_content(arguments.content, arguments.file)?.unwrap_or_default();
            let heading = arguments.heading;
            let card = project.update_card(&arguments.id, |card| {
                card.body = markdown::set_section(&card.body, &heading, &content)?;
                Ok(())
            })?;
            println!("Set section {heading:?} on {}", card.metadata.id);
            Ok(())
        }
        SectionCommand::Append(arguments) => {
            let content =
                read_optional_content(arguments.content, arguments.file)?.unwrap_or_default();
            if content.trim().is_empty() {
                bail!("content to append cannot be empty");
            }
            let heading = arguments.heading;
            let card = project.update_card(&arguments.id, |card| {
                card.body = markdown::append_section(&card.body, &heading, &content)?;
                Ok(())
            })?;
            println!("Appended to section {heading:?} on {}", card.metadata.id);
            Ok(())
        }
        SectionCommand::Remove { id, heading } => {
            let card = project.update_card(&id, |card| {
                card.body = markdown::remove_section(&card.body, &heading)?;
                Ok(())
            })?;
            println!("Removed section {heading:?} from {}", card.metadata.id);
            Ok(())
        }
    }
}

fn command_check(start: &Path, command: CheckCommand) -> Result<()> {
    let project = Project::discover(start)?;
    match command {
        CheckCommand::List { id, section, json } => {
            let card = project.load_card(&id)?;
            let items = markdown::checklist_items(&card.body)
                .into_iter()
                .filter(|item| {
                    section.as_deref().is_none_or(|section| {
                        item.section
                            .as_deref()
                            .is_some_and(|candidate| candidate.eq_ignore_ascii_case(section))
                    })
                })
                .collect::<Vec<_>>();
            if json {
                let output = items
                    .iter()
                    .map(|item| {
                        serde_json::json!({
                            "global_index": item.global_index,
                            "section": item.section,
                            "index": item.index,
                            "checked": item.checked,
                            "text": item.text,
                            "line": item.line_number,
                        })
                    })
                    .collect::<Vec<_>>();
                print_json(&output)
            } else {
                for item in items {
                    println!(
                        "{:>3} {:<20} {:>3} [{}] {}",
                        item.global_index,
                        item.section.as_deref().unwrap_or("(preamble)"),
                        item.index,
                        if item.checked { "x" } else { " " },
                        item.text
                    );
                }
                Ok(())
            }
        }
        CheckCommand::Add { id, section, text } => {
            mutate_checklist(&project, &id, &section, |body| {
                markdown::add_checklist_item(body, &section, &text)
            })
        }
        CheckCommand::Toggle { id, section, index } => {
            mutate_checklist(&project, &id, &section, |body| {
                markdown::toggle_checklist_item(body, &section, index)
            })
        }
        CheckCommand::Check { id, section, index } => {
            mutate_checklist(&project, &id, &section, |body| {
                markdown::set_checklist_item(body, &section, index, true)
            })
        }
        CheckCommand::Uncheck { id, section, index } => {
            mutate_checklist(&project, &id, &section, |body| {
                markdown::set_checklist_item(body, &section, index, false)
            })
        }
        CheckCommand::Remove { id, section, index } => {
            mutate_checklist(&project, &id, &section, |body| {
                markdown::remove_checklist_item(body, &section, index)
            })
        }
        CheckCommand::ToggleGlobal { id, index } => {
            mutate_checklist(&project, &id, "global", |body| {
                markdown::toggle_checklist_global(body, index)
            })
        }
        CheckCommand::CheckGlobal { id, index } => {
            mutate_checklist(&project, &id, "global", |body| {
                markdown::set_checklist_global(body, index, true)
            })
        }
        CheckCommand::UncheckGlobal { id, index } => {
            mutate_checklist(&project, &id, "global", |body| {
                markdown::set_checklist_global(body, index, false)
            })
        }
        CheckCommand::RemoveGlobal { id, index } => {
            mutate_checklist(&project, &id, "global", |body| {
                markdown::remove_checklist_global(body, index)
            })
        }
    }
}

fn command_board(start: &Path, arguments: BoardArgs) -> Result<()> {
    let project = Project::discover(start)?;
    let cards = project.load_cards()?;
    if arguments.json {
        let columns = project
            .config
            .columns
            .iter()
            .map(|column| ColumnOutput {
                name: &column.name,
                cards: cards
                    .iter()
                    .filter(|card| card.metadata.status.eq_ignore_ascii_case(&column.name))
                    .map(card_output)
                    .collect(),
            })
            .collect();
        print_json(&BoardOutput {
            name: &project.config.name,
            columns,
        })
    } else {
        println!("{}", project.config.name);
        for column in &project.config.columns {
            let column_cards = cards
                .iter()
                .filter(|card| card.metadata.status.eq_ignore_ascii_case(&column.name))
                .collect::<Vec<_>>();
            if let Some(limit) = column.wip_limit {
                println!("\n{} ({}/{limit})", column.name, column_cards.len());
            } else {
                println!("\n{} ({})", column.name, column_cards.len());
            }
            if column_cards.is_empty() {
                println!("  ·");
            }
            for card in column_cards {
                let (checked, total) = card.checklist_progress();
                let progress = if total > 0 {
                    format!(" [{checked}/{total}]")
                } else {
                    String::new()
                };
                let labels = if card.metadata.labels.is_empty() {
                    String::new()
                } else {
                    format!(" #{}", card.metadata.labels.join(" #"))
                };
                println!(
                    "  {:<10} {}{progress}{labels}",
                    card.metadata.id, card.metadata.title
                );
            }
        }
        Ok(())
    }
}

fn command_validate(start: &Path) -> Result<()> {
    let project = Project::discover(start)?;
    let issues = project.validate();
    if issues.is_empty() {
        println!("OK — config and all cards are valid");
        return Ok(());
    }
    for issue in &issues {
        eprintln!("{}: {}", issue.path.display(), issue.message);
    }
    bail!("validation found {} issue(s)", issues.len())
}

fn mutate_checklist<F>(project: &Project, id: &str, section: &str, mutate: F) -> Result<()>
where
    F: FnOnce(&str) -> Result<String>,
{
    let card = project.update_card(id, |card| {
        card.body = mutate(&card.body)?;
        Ok(())
    })?;
    println!("Updated checklist {section:?} on {}", card.metadata.id);
    Ok(())
}

fn split_assignment<'a>(definition: &'a str, kind: &str) -> Result<(&'a str, &'a str)> {
    let Some((key, value)) = definition.split_once('=') else {
        bail!("{kind} must use NAME=VALUE syntax");
    };
    if key.trim().is_empty() {
        bail!("{kind} name cannot be empty");
    }
    Ok((key.trim(), value))
}

fn read_optional_content(content: Option<String>, file: Option<PathBuf>) -> Result<Option<String>> {
    if let Some(content) = content {
        return Ok(Some(content));
    }
    let Some(path) = file else {
        return Ok(None);
    };
    if path == Path::new("-") {
        let mut input = String::new();
        io::stdin().read_to_string(&mut input)?;
        Ok(Some(input))
    } else {
        Ok(Some(fs::read_to_string(&path).with_context(|| {
            format!("could not read {}", path.display())
        })?))
    }
}

fn parse_yaml_value(source: &str) -> Result<Value> {
    serde_saphyr::from_str(source).context("invalid YAML value")
}

fn ensure_custom_key(key: &str) -> Result<()> {
    const RESERVED: [&str; 9] = [
        "id",
        "title",
        "status",
        "labels",
        "assignee",
        "assignees",
        "ordinal",
        "created_date",
        "updated_date",
    ];
    if key.trim().is_empty() || key.contains(['\r', '\n']) {
        bail!("custom field key must be a non-empty single line");
    }
    if RESERVED
        .iter()
        .any(|reserved| key.eq_ignore_ascii_case(reserved))
    {
        bail!("{key:?} is reserved; use `kbmd edit` for canonical fields");
    }
    Ok(())
}

fn get_custom_field<'a>(card: &'a Card, key: &str) -> Result<&'a Value> {
    card.metadata
        .extra
        .get(key)
        .ok_or_else(|| anyhow::anyhow!("custom field {key:?} was not found"))
}

fn extend_unique(target: &mut Vec<String>, additions: &[String]) {
    for addition in additions {
        let addition = addition.trim();
        if !addition.is_empty()
            && !target
                .iter()
                .any(|existing| existing.eq_ignore_ascii_case(addition))
        {
            target.push(addition.to_owned());
        }
    }
}

fn remove_values(target: &mut Vec<String>, removals: &[String]) {
    target.retain(|existing| {
        !removals
            .iter()
            .any(|removal| existing.eq_ignore_ascii_case(removal.trim()))
    });
}

fn card_output(card: &Card) -> CardOutput<'_> {
    let (checked, total) = card.checklist_progress();
    CardOutput {
        metadata: &card.metadata,
        body: &card.body,
        path: &card.path,
        checklist: Progress { checked, total },
    }
}

fn print_cards_json(cards: &[Card]) -> Result<()> {
    let cards = cards.iter().map(card_output).collect::<Vec<_>>();
    print_json(&cards)
}

fn print_card_json(card: &Card) -> Result<()> {
    print_json(&card_output(card))
}

fn print_json<T: Serialize>(data: &T) -> Result<()> {
    println!(
        "{}",
        serde_json::to_string_pretty(&JsonEnvelope {
            schema_version: JSON_SCHEMA_VERSION,
            data,
        })?
    );
    Ok(())
}

fn print_mutation_result(card: &Card, json: bool, verb: &str) -> Result<()> {
    if json {
        print_card_json(card)
    } else {
        println!("{verb} {} — {}", card.metadata.id, card.metadata.title);
        Ok(())
    }
}

fn print_card_table(cards: &[Card]) {
    if cards.is_empty() {
        println!("No cards.");
        return;
    }
    let id_width = cards
        .iter()
        .map(|card| card.metadata.id.len())
        .max()
        .unwrap_or(2)
        .max(2);
    let status_width = cards
        .iter()
        .map(|card| card.metadata.status.len())
        .max()
        .unwrap_or(6)
        .max(6);
    println!("{:<id_width$}  {:<status_width$}  TITLE", "ID", "STATUS");
    for card in cards {
        let (checked, total) = card.checklist_progress();
        let progress = if total > 0 {
            format!(" [{checked}/{total}]")
        } else {
            String::new()
        };
        println!(
            "{:<id_width$}  {:<status_width$}  {}{}",
            card.metadata.id, card.metadata.status, card.metadata.title, progress
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assignment_splits_only_once() {
        assert_eq!(
            split_assignment("Notes=a=b", "section").unwrap(),
            ("Notes", "a=b")
        );
    }

    #[test]
    fn reserved_keys_cannot_be_custom_fields() {
        assert!(ensure_custom_key("status").is_err());
        assert!(ensure_custom_key("STATUS").is_err());
        assert!(ensure_custom_key("due_date").is_ok());
        assert!(ensure_custom_key("estimate").is_ok());
    }

    #[test]
    fn yaml_values_keep_nested_types() {
        assert_eq!(
            parse_yaml_value("{points: 3, risky: false}").unwrap(),
            serde_json::json!({"points": 3, "risky": false})
        );
    }
}
