use anyhow::{Context, Result};

use crate::model::Card;
use crate::store::{CreateCard, Project};
use crate::{comments, markdown};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Focus {
    #[default]
    Board,
    Detail,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum Mode {
    #[default]
    Normal,
    QuickAdd {
        title: String,
    },
    AddComment {
        card_id: String,
        author: String,
        text: String,
    },
    Help,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DragState {
    pub card_id: String,
    pub hover_column: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    Quit,
    Reload,
    ToggleHelp,
    OpenQuickAdd,
    OpenComment,
    CancelModal,
    QuickAddCharacter(char),
    QuickAddBackspace,
    QuickAddClear,
    SubmitQuickAdd,
    CommentCharacter(char),
    CommentBackspace,
    CommentClear,
    SubmitComment,
    Focus(Focus),
    ToggleFocus,
    PreviousColumn,
    NextColumn,
    PreviousCard,
    NextCard,
    PreviousChecklist,
    NextChecklist,
    ScrollDetail(i32),
    MoveSelected(i32),
    ToggleSelectedChecklist,
    SelectColumn(usize),
    BeginDrag {
        id: String,
        column: usize,
        row: usize,
    },
    DragOver(Option<usize>),
    DropOn(Option<usize>),
    ToggleChecklist {
        card_id: String,
        global_index: usize,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Effect {
    Quit,
    Reload,
    CreateCard {
        title: String,
        status: String,
    },
    AddComment {
        id: String,
        author: String,
        text: String,
    },
    MoveCard {
        id: String,
        status: String,
    },
    ToggleChecklist {
        id: String,
        global_index: usize,
    },
}

pub(crate) struct App {
    pub project: Project,
    pub cards: Vec<Card>,
    pub selected_id: Option<String>,
    pub active_column: usize,
    pub selected_row: usize,
    pub focus: Focus,
    pub mode: Mode,
    pub detail_checklist: usize,
    pub detail_scroll: usize,
    pub detail_follow_cursor: bool,
    pub drag: Option<DragState>,
    pub error: Option<String>,
    pub message: Option<String>,
}

impl App {
    pub fn new(project: Project, cards: Vec<Card>) -> Self {
        let mut app = Self {
            project,
            cards,
            selected_id: None,
            active_column: 0,
            selected_row: 0,
            focus: Focus::Board,
            mode: Mode::Normal,
            detail_checklist: 0,
            detail_scroll: 0,
            detail_follow_cursor: true,
            drag: None,
            error: None,
            message: None,
        };
        app.select_first_available();
        app
    }

    pub fn selected_card(&self) -> Option<&Card> {
        let selected = self.selected_id.as_deref()?;
        self.cards
            .iter()
            .find(|card| card.metadata.id.eq_ignore_ascii_case(selected))
    }

    pub fn selected_checklist_global(&self) -> Option<usize> {
        let card = self.selected_card()?;
        markdown::checklist_items(&card.body)
            .get(self.detail_checklist)
            .map(|item| item.global_index)
    }

    pub fn cards_in_column(&self, column: usize) -> Vec<&Card> {
        let Some(configured) = self.project.config.columns.get(column) else {
            return Vec::new();
        };
        self.cards
            .iter()
            .filter(|card| card.metadata.status.eq_ignore_ascii_case(&configured.name))
            .collect()
    }

    pub fn reduce(&mut self, action: Action) -> Vec<Effect> {
        match action {
            Action::Quit => vec![Effect::Quit],
            Action::Reload => vec![Effect::Reload],
            Action::ToggleHelp => {
                self.mode = if self.mode == Mode::Help {
                    Mode::Normal
                } else {
                    Mode::Help
                };
                Vec::new()
            }
            Action::OpenQuickAdd => {
                self.mode = Mode::QuickAdd {
                    title: String::new(),
                };
                self.error = None;
                Vec::new()
            }
            Action::OpenComment => {
                self.open_comment();
                Vec::new()
            }
            Action::CancelModal => {
                self.mode = Mode::Normal;
                self.error = None;
                Vec::new()
            }
            Action::QuickAddCharacter(character) => {
                if let Mode::QuickAdd { title } = &mut self.mode {
                    title.push(character);
                }
                Vec::new()
            }
            Action::QuickAddBackspace => {
                if let Mode::QuickAdd { title } = &mut self.mode {
                    title.pop();
                }
                Vec::new()
            }
            Action::QuickAddClear => {
                if let Mode::QuickAdd { title } = &mut self.mode {
                    title.clear();
                }
                Vec::new()
            }
            Action::SubmitQuickAdd => self.submit_quick_add(),
            Action::CommentCharacter(character) => {
                if let Mode::AddComment { text, .. } = &mut self.mode {
                    text.push(character);
                    self.error = None;
                }
                Vec::new()
            }
            Action::CommentBackspace => {
                if let Mode::AddComment { text, .. } = &mut self.mode {
                    text.pop();
                    self.error = None;
                }
                Vec::new()
            }
            Action::CommentClear => {
                if let Mode::AddComment { text, .. } = &mut self.mode {
                    text.clear();
                    self.error = None;
                }
                Vec::new()
            }
            Action::SubmitComment => self.submit_comment(),
            Action::Focus(focus) => {
                self.focus = focus;
                Vec::new()
            }
            Action::ToggleFocus => {
                self.focus = match self.focus {
                    Focus::Board => Focus::Detail,
                    Focus::Detail => Focus::Board,
                };
                Vec::new()
            }
            Action::PreviousColumn => {
                self.change_column(-1);
                Vec::new()
            }
            Action::NextColumn => {
                self.change_column(1);
                Vec::new()
            }
            Action::PreviousCard => {
                self.change_card(-1);
                Vec::new()
            }
            Action::NextCard => {
                self.change_card(1);
                Vec::new()
            }
            Action::PreviousChecklist => {
                self.change_checklist(-1);
                Vec::new()
            }
            Action::NextChecklist => {
                self.change_checklist(1);
                Vec::new()
            }
            Action::ScrollDetail(delta) => {
                self.detail_follow_cursor = false;
                self.detail_scroll = add_signed(self.detail_scroll, delta);
                Vec::new()
            }
            Action::MoveSelected(delta) => self.move_selected(delta),
            Action::ToggleSelectedChecklist => self
                .selected_id
                .clone()
                .zip(self.selected_checklist_global())
                .map_or_else(Vec::new, |(id, global_index)| {
                    vec![Effect::ToggleChecklist { id, global_index }]
                }),
            Action::SelectColumn(column) => {
                self.select_column(column);
                Vec::new()
            }
            Action::BeginDrag { id, column, row } => {
                self.select_card(id.clone(), column, row);
                self.drag = Some(DragState {
                    card_id: id,
                    hover_column: column,
                });
                Vec::new()
            }
            Action::DragOver(column) => {
                if let (Some(drag), Some(column)) = (&mut self.drag, column) {
                    drag.hover_column = column;
                }
                Vec::new()
            }
            Action::DropOn(column) => self.drop_card(column),
            Action::ToggleChecklist {
                card_id,
                global_index,
            } => {
                if let Some((column, row)) = self.card_position(&card_id) {
                    self.select_card(card_id.clone(), column, row);
                    self.focus = Focus::Detail;
                    self.detail_checklist = global_index.saturating_sub(1);
                    self.detail_follow_cursor = true;
                }
                vec![Effect::ToggleChecklist {
                    id: card_id,
                    global_index,
                }]
            }
        }
    }

    pub fn reload(&mut self) {
        let preserve = self.selected_id.clone();
        let result = (|| {
            let project = Project::open(&self.project.root).context("could not reload project")?;
            let cards = project.load_cards().context("could not reload cards")?;
            Ok::<_, anyhow::Error>((project, cards))
        })();

        match result {
            Ok((project, cards)) => {
                self.apply_snapshot(project, cards, preserve.as_deref());
                self.error = None;
            }
            Err(error) => {
                // Keep the last successfully loaded project and cards visible.
                self.error = Some(format!("Reload failed: {error:#}"));
            }
        }
    }

    pub fn apply_effect(&mut self, effect: Effect) -> Result<bool> {
        match effect {
            Effect::Quit => return Ok(true),
            Effect::Reload => {
                self.reload();
                if self.error.is_none() {
                    self.message = Some("Reloaded project".to_owned());
                }
            }
            Effect::CreateCard { title, status } => {
                match self.project.create_card(CreateCard {
                    title,
                    status: Some(status),
                    ..CreateCard::default()
                }) {
                    Ok(card) => {
                        let id = card.metadata.id.clone();
                        self.reload_preserving(Some(&id));
                        self.message = Some(format!("Created {id}"));
                    }
                    Err(error) => self.error = Some(format!("Could not create card: {error:#}")),
                }
            }
            Effect::AddComment { id, author, text } => {
                let update = self.project.update_card(&id, |card| {
                    let (body, _) = comments::append(&card.body, &author, &text)?;
                    card.body = body;
                    Ok(())
                });
                match update {
                    Ok(_) => {
                        self.reload_preserving(Some(&id));
                        self.mode = Mode::Normal;
                        if self.error.is_none() {
                            self.message = Some(format!("Commented on {id}"));
                        }
                    }
                    Err(error) => {
                        self.error = Some(format!("Could not comment on {id}: {error:#}"));
                    }
                }
            }
            Effect::MoveCard { id, status } => match self.project.move_card(&id, &status) {
                Ok(_) => {
                    self.reload_preserving(Some(&id));
                    self.message = Some(format!("Moved {id} to {status}"));
                }
                Err(error) => self.error = Some(format!("Could not move {id}: {error:#}")),
            },
            Effect::ToggleChecklist { id, global_index } => {
                let update = self.project.update_card(&id, |card| {
                    card.body = markdown::toggle_checklist_global(&card.body, global_index)?;
                    Ok(())
                });
                match update {
                    Ok(_) => {
                        self.reload_preserving(Some(&id));
                        self.message = Some(format!("Updated checklist on {id}"));
                    }
                    Err(error) => {
                        self.error = Some(format!("Could not update checklist on {id}: {error:#}"));
                    }
                }
            }
        }
        Ok(false)
    }

    pub fn set_error(&mut self, message: impl Into<String>) {
        self.error = Some(message.into());
    }

    fn submit_quick_add(&mut self) -> Vec<Effect> {
        let Mode::QuickAdd { title } = &self.mode else {
            return Vec::new();
        };
        let title = title.trim();
        if title.is_empty() {
            self.error = Some("Card title cannot be empty".to_owned());
            return Vec::new();
        }
        let Some(status) = self
            .project
            .config
            .columns
            .get(self.active_column)
            .map(|column| column.name.clone())
        else {
            self.error = Some("No board column is available".to_owned());
            return Vec::new();
        };
        let title = title.to_owned();
        self.mode = Mode::Normal;
        self.error = None;
        vec![Effect::CreateCard { title, status }]
    }

    fn open_comment(&mut self) {
        let Some(card_id) = self.selected_card().map(|card| card.metadata.id.clone()) else {
            self.error = Some("Select a card before adding a comment".to_owned());
            return;
        };
        match comments::resolve_author(None, &self.project.root) {
            Ok(author) => {
                self.mode = Mode::AddComment {
                    card_id,
                    author,
                    text: String::new(),
                };
                self.error = None;
            }
            Err(error) => {
                self.error = Some(format!("Could not resolve comment author: {error:#}"));
            }
        }
    }

    fn submit_comment(&mut self) -> Vec<Effect> {
        let Mode::AddComment {
            card_id,
            author,
            text,
        } = &self.mode
        else {
            return Vec::new();
        };
        if text.trim().is_empty() {
            self.error = Some("Comment text cannot be empty".to_owned());
            return Vec::new();
        }
        self.error = None;
        vec![Effect::AddComment {
            id: card_id.clone(),
            author: author.clone(),
            text: text.clone(),
        }]
    }

    fn move_selected(&self, delta: i32) -> Vec<Effect> {
        let Some(card) = self.selected_card() else {
            return Vec::new();
        };
        let Some(current) = self.project.config.column_index(&card.metadata.status) else {
            return Vec::new();
        };
        let Some(target) = offset_index(
            current,
            delta,
            self.project.config.columns.len().saturating_sub(1),
        ) else {
            return Vec::new();
        };
        if target == current {
            return Vec::new();
        }
        vec![Effect::MoveCard {
            id: card.metadata.id.clone(),
            status: self.project.config.columns[target].name.clone(),
        }]
    }

    fn drop_card(&mut self, column: Option<usize>) -> Vec<Effect> {
        let Some(drag) = self.drag.take() else {
            return Vec::new();
        };
        // Releasing outside a current column cancels the drag. `hover_column` is visual state,
        // while the frame hit map at mouse-up is the authority for a persistent move.
        let Some(target) = column else {
            return Vec::new();
        };
        let Some(status) = self
            .project
            .config
            .columns
            .get(target)
            .map(|column| column.name.clone())
        else {
            return Vec::new();
        };
        let Some(card) = self
            .cards
            .iter()
            .find(|card| card.metadata.id.eq_ignore_ascii_case(&drag.card_id))
        else {
            return Vec::new();
        };
        if card.metadata.status.eq_ignore_ascii_case(&status) {
            return Vec::new();
        }
        vec![Effect::MoveCard {
            id: drag.card_id,
            status,
        }]
    }

    fn change_column(&mut self, delta: i32) {
        let last = self.project.config.columns.len().saturating_sub(1);
        let Some(column) = offset_index(self.active_column, delta, last) else {
            return;
        };
        self.select_column(column);
    }

    fn change_card(&mut self, delta: i32) {
        let ids = self.card_ids_in_column(self.active_column);
        if ids.is_empty() {
            self.selected_id = None;
            self.selected_row = 0;
            return;
        }
        let last = ids.len() - 1;
        let row = offset_index(self.selected_row.min(last), delta, last).unwrap_or(0);
        self.selected_row = row;
        self.selected_id = Some(ids[row].clone());
        self.reset_detail_cursor();
    }

    fn change_checklist(&mut self, delta: i32) {
        let Some(card) = self.selected_card() else {
            return;
        };
        let count = markdown::checklist_items(&card.body).len();
        if count == 0 {
            self.detail_scroll = add_signed(self.detail_scroll, delta);
            self.detail_follow_cursor = false;
            return;
        }
        self.detail_checklist =
            offset_index(self.detail_checklist.min(count - 1), delta, count - 1).unwrap_or(0);
        self.detail_follow_cursor = true;
    }

    fn select_column(&mut self, column: usize) {
        if self.project.config.columns.is_empty() {
            return;
        }
        self.active_column = column.min(self.project.config.columns.len() - 1);
        let ids = self.card_ids_in_column(self.active_column);
        if ids.is_empty() {
            self.selected_id = None;
        } else {
            self.selected_row = self.selected_row.min(ids.len() - 1);
            self.selected_id = Some(ids[self.selected_row].clone());
        }
        self.reset_detail_cursor();
    }

    fn select_card(&mut self, id: String, column: usize, row: usize) {
        if self.card_position(&id).is_none() {
            return;
        }
        self.active_column = column.min(self.project.config.columns.len().saturating_sub(1));
        self.selected_row = row;
        if self.selected_id.as_deref() != Some(id.as_str()) {
            self.selected_id = Some(id);
            self.reset_detail_cursor();
        } else {
            self.selected_id = Some(id);
        }
    }

    fn apply_snapshot(&mut self, project: Project, cards: Vec<Card>, preserve_id: Option<&str>) {
        let old_column = self.active_column;
        let old_row = self.selected_row;
        self.project = project;
        self.cards = cards;
        self.drag = None;

        if let Some(id) = preserve_id
            && let Some((column, row)) = self.card_position(id)
        {
            self.active_column = column;
            self.selected_row = row;
            self.selected_id = Some(id.to_owned());
            self.normalize_detail_cursor();
            return;
        }

        self.active_column = old_column.min(self.project.config.columns.len().saturating_sub(1));
        self.selected_row = old_row;
        self.select_column(self.active_column);
    }

    fn reload_preserving(&mut self, selected_id: Option<&str>) {
        let result = (|| {
            let project = Project::open(&self.project.root)?;
            let cards = project.load_cards()?;
            Ok::<_, anyhow::Error>((project, cards))
        })();
        match result {
            Ok((project, cards)) => {
                self.apply_snapshot(project, cards, selected_id);
                self.error = None;
            }
            Err(error) => self.error = Some(format!("Reload failed: {error:#}")),
        }
    }

    fn select_first_available(&mut self) {
        for column in 0..self.project.config.columns.len() {
            let ids = self.card_ids_in_column(column);
            if let Some(id) = ids.first() {
                self.active_column = column;
                self.selected_row = 0;
                self.selected_id = Some(id.clone());
                self.normalize_detail_cursor();
                return;
            }
        }
    }

    fn card_ids_in_column(&self, column: usize) -> Vec<String> {
        self.cards_in_column(column)
            .into_iter()
            .map(|card| card.metadata.id.clone())
            .collect()
    }

    fn card_position(&self, id: &str) -> Option<(usize, usize)> {
        let card = self
            .cards
            .iter()
            .find(|card| card.metadata.id.eq_ignore_ascii_case(id))?;
        let column = self.project.config.column_index(&card.metadata.status)?;
        let row = self
            .cards_in_column(column)
            .iter()
            .position(|candidate| candidate.metadata.id.eq_ignore_ascii_case(id))?;
        Some((column, row))
    }

    fn reset_detail_cursor(&mut self) {
        self.detail_checklist = 0;
        self.detail_scroll = 0;
        self.detail_follow_cursor = true;
    }

    fn normalize_detail_cursor(&mut self) {
        let count = self
            .selected_card()
            .map_or(0, |card| markdown::checklist_items(&card.body).len());
        self.detail_checklist = self.detail_checklist.min(count.saturating_sub(1));
    }
}

fn add_signed(value: usize, delta: i32) -> usize {
    if delta.is_negative() {
        value.saturating_sub(delta.unsigned_abs() as usize)
    } else {
        value.saturating_add(delta as usize)
    }
}

fn offset_index(index: usize, delta: i32, maximum: usize) -> Option<usize> {
    if delta == 0 {
        return Some(index.min(maximum));
    }
    if delta.is_negative() {
        index.checked_sub(delta.unsigned_abs() as usize)
    } else {
        Some(index.saturating_add(delta as usize).min(maximum))
    }
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use tempfile::tempdir;

    use crate::comments;
    use crate::config::BoardConfig;

    use super::*;

    fn app() -> (tempfile::TempDir, App) {
        let directory = tempdir().unwrap();
        let project = Project::init(
            directory.path(),
            &BoardConfig::new(
                "TUI test",
                "T",
                vec!["Inbox".to_owned(), "Doing".to_owned(), "Done".to_owned()],
            ),
        )
        .unwrap();
        project
            .create_card(CreateCard {
                title: "First".to_owned(),
                body: "## Plan\n\n- [ ] One\n- [x] Two\n".to_owned(),
                ..CreateCard::default()
            })
            .unwrap();
        project
            .create_card(CreateCard {
                title: "Second".to_owned(),
                ..CreateCard::default()
            })
            .unwrap();
        let cards = project.load_cards().unwrap();
        (directory, App::new(project, cards))
    }

    fn configure_git_author(app: &App) {
        let initialized = Command::new("git")
            .args(["init", "--quiet"])
            .arg(&app.project.root)
            .status()
            .unwrap();
        assert!(initialized.success());
        let configured = Command::new("git")
            .arg("-C")
            .arg(&app.project.root)
            .args(["config", "user.name", "TUI Author"])
            .status()
            .unwrap();
        assert!(configured.success());
    }

    #[test]
    fn reducer_navigates_and_emits_move_without_mutating_storage() {
        let (_directory, mut app) = app();

        app.reduce(Action::NextCard);
        assert_eq!(app.selected_id.as_deref(), Some("T-2"));
        let effects = app.reduce(Action::MoveSelected(1));

        assert_eq!(
            effects,
            vec![Effect::MoveCard {
                id: "T-2".to_owned(),
                status: "Doing".to_owned(),
            }]
        );
        assert_eq!(
            app.project.load_card("T-2").unwrap().metadata.status,
            "Inbox"
        );
    }

    #[test]
    fn reducer_builds_quick_add_for_the_active_column() {
        let (_directory, mut app) = app();
        app.reduce(Action::NextColumn);
        app.reduce(Action::OpenQuickAdd);
        for character in "A new card".chars() {
            app.reduce(Action::QuickAddCharacter(character));
        }

        assert_eq!(
            app.reduce(Action::SubmitQuickAdd),
            vec![Effect::CreateCard {
                title: "A new card".to_owned(),
                status: "Doing".to_owned(),
            }]
        );
        assert_eq!(app.mode, Mode::Normal);
    }

    #[test]
    fn reducer_tracks_drag_hover_and_emits_drop_move() {
        let (_directory, mut app) = app();
        app.reduce(Action::BeginDrag {
            id: "T-1".to_owned(),
            column: 0,
            row: 0,
        });
        app.reduce(Action::DragOver(Some(2)));

        assert_eq!(
            app.reduce(Action::DropOn(Some(2))),
            vec![Effect::MoveCard {
                id: "T-1".to_owned(),
                status: "Done".to_owned(),
            }]
        );
        assert!(app.drag.is_none());
    }

    #[test]
    fn reducer_captures_comment_target_author_and_draft_until_persisted() {
        let (_directory, mut app) = app();
        configure_git_author(&app);
        let expected_author = comments::resolve_author(None, &app.project.root).unwrap();

        assert!(app.reduce(Action::OpenComment).is_empty());
        for character in "Looks good".chars() {
            app.reduce(Action::CommentCharacter(character));
        }
        let expected_mode = Mode::AddComment {
            card_id: "T-1".to_owned(),
            author: expected_author.clone(),
            text: "Looks good".to_owned(),
        };
        assert_eq!(app.mode, expected_mode);

        let effects = app.reduce(Action::SubmitComment);

        assert_eq!(
            effects,
            vec![Effect::AddComment {
                id: "T-1".to_owned(),
                author: expected_author,
                text: "Looks good".to_owned(),
            }]
        );
        assert_eq!(app.mode, expected_mode);
    }

    #[test]
    fn comment_draft_survives_reload_and_selection_changes_without_retargeting() {
        let (_directory, mut app) = app();
        app.mode = Mode::AddComment {
            card_id: "T-1".to_owned(),
            author: "TUI Author".to_owned(),
            text: "Still writing".to_owned(),
        };
        app.selected_id = Some("T-2".to_owned());
        app.selected_row = 1;

        app.reload();

        assert_eq!(app.selected_id.as_deref(), Some("T-2"));
        assert_eq!(
            app.mode,
            Mode::AddComment {
                card_id: "T-1".to_owned(),
                author: "TUI Author".to_owned(),
                text: "Still writing".to_owned(),
            }
        );
        assert!(matches!(
            app.reduce(Action::SubmitComment).as_slice(),
            [Effect::AddComment { id, .. }] if id == "T-1"
        ));
    }

    #[test]
    fn empty_comment_and_failed_write_retain_the_modal_and_draft() {
        let (_directory, mut app) = app();
        app.mode = Mode::AddComment {
            card_id: "T-404".to_owned(),
            author: "TUI Author".to_owned(),
            text: "   ".to_owned(),
        };

        assert!(app.reduce(Action::SubmitComment).is_empty());
        assert_eq!(app.error.as_deref(), Some("Comment text cannot be empty"));
        assert!(matches!(app.mode, Mode::AddComment { .. }));

        app.reduce(Action::CommentClear);
        for character in "Keep this draft".chars() {
            app.reduce(Action::CommentCharacter(character));
        }
        let effect = app.reduce(Action::SubmitComment).pop().unwrap();
        app.apply_effect(effect).unwrap();

        assert!(
            app.error
                .as_deref()
                .unwrap()
                .contains("Could not comment on T-404")
        );
        assert_eq!(
            app.mode,
            Mode::AddComment {
                card_id: "T-404".to_owned(),
                author: "TUI Author".to_owned(),
                text: "Keep this draft".to_owned(),
            }
        );
    }

    #[test]
    fn comment_effect_persists_then_closes_without_resetting_detail_position() {
        let (_directory, mut app) = app();
        app.detail_checklist = 1;
        app.detail_scroll = 4;
        app.detail_follow_cursor = false;
        app.mode = Mode::AddComment {
            card_id: "T-1".to_owned(),
            author: "TUI Author".to_owned(),
            text: "Persist me".to_owned(),
        };

        app.apply_effect(Effect::AddComment {
            id: "T-1".to_owned(),
            author: "TUI Author".to_owned(),
            text: "Persist me".to_owned(),
        })
        .unwrap();

        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(app.detail_checklist, 1);
        assert_eq!(app.detail_scroll, 4);
        assert!(!app.detail_follow_cursor);
        let card = app.project.load_card("T-1").unwrap();
        let stored = comments::parse(&card.body).unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].author, "TUI Author");
        assert_eq!(stored[0].body, "Persist me");
    }

    #[test]
    fn failed_reload_keeps_last_good_cards_and_sets_banner() {
        let (_directory, mut app) = app();
        std::fs::write(&app.cards[0].path, "broken\n").unwrap();
        let before = app.cards.clone();

        app.reload();

        assert_eq!(app.cards, before);
        assert!(app.error.as_deref().unwrap().contains("Reload failed"));
    }

    #[test]
    fn project_effects_persist_and_preserve_the_selected_card() {
        let (_directory, mut app) = app();

        app.apply_effect(Effect::ToggleChecklist {
            id: "T-1".to_owned(),
            global_index: 1,
        })
        .unwrap();
        assert!(markdown::checklist_items(&app.project.load_card("T-1").unwrap().body)[0].checked);
        assert_eq!(app.selected_id.as_deref(), Some("T-1"));

        app.apply_effect(Effect::MoveCard {
            id: "T-1".to_owned(),
            status: "Doing".to_owned(),
        })
        .unwrap();
        assert_eq!(
            app.project.load_card("T-1").unwrap().metadata.status,
            "Doing"
        );
        assert_eq!(app.selected_id.as_deref(), Some("T-1"));
        assert_eq!(app.active_column, 1);
    }
}
