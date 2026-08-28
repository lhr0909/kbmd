//! Interactive Trello-like board over the same concurrency-safe [`Project`] API as the CLI.

mod app;
mod terminal;
mod ui;
mod watch;

use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};

use crate::store::Project;

use self::app::{Action, App, Effect, Focus, Mode};
use self::terminal::TerminalSession;
use self::ui::{HitMap, HitTarget};
use self::watch::LiveReload;

const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Runs the interactive board until the user presses `q` (or Ctrl-C).
///
/// The terminal session is restored through RAII on normal exit, errors, and unwinding. File
/// mutations always go through [`Project`], so the TUI shares locking, validation, and atomic
/// persistence behavior with the CLI.
pub fn run(project: Project) -> Result<()> {
    let cards = project
        .load_cards()
        .context("could not load cards for the TUI")?;
    let watch_path = project.internal_dir.clone();
    let mut app = App::new(project, cards);
    let (mut live_reload, setup_error) = LiveReload::start(&watch_path);
    if let Some(error) = setup_error {
        app.set_error(error);
    }

    let mut session = TerminalSession::enter()?;
    run_loop(session.terminal_mut(), &mut app, &mut live_reload)
}

fn run_loop(
    terminal: &mut terminal::KbmdTerminal,
    app: &mut App,
    live_reload: &mut LiveReload,
) -> Result<()> {
    loop {
        let tick = live_reload.tick(Instant::now());
        if tick.reload {
            app.reload();
        }
        for error in tick.errors {
            app.set_error(error);
        }

        let mut hit_map = HitMap::default();
        terminal
            .draw(|frame| hit_map = ui::render(frame, app))
            .context("could not draw the TUI")?;

        if !event::poll(EVENT_POLL_INTERVAL).context("could not poll terminal events")? {
            continue;
        }
        let event = event::read().context("could not read terminal event")?;
        let effects = match event {
            Event::Key(key) if !matches!(key.kind, KeyEventKind::Release) => handle_key(app, key),
            Event::Mouse(mouse) => handle_mouse(app, mouse, &hit_map),
            Event::Resize(_, _)
            | Event::FocusGained
            | Event::FocusLost
            | Event::Paste(_)
            | Event::Key(_) => Vec::new(),
        };
        for effect in effects {
            if app.apply_effect(effect)? {
                return Ok(());
            }
        }
    }
}

fn handle_key(app: &mut App, key: KeyEvent) -> Vec<Effect> {
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        return app.reduce(Action::Quit);
    }

    match app.mode.clone() {
        Mode::QuickAdd { .. } => handle_quick_add_key(app, key),
        Mode::AddComment { .. } => handle_comment_key(app, key),
        Mode::Help => match key.code {
            KeyCode::Char('q') => app.reduce(Action::Quit),
            KeyCode::Esc | KeyCode::Char('?') => app.reduce(Action::ToggleHelp),
            _ => Vec::new(),
        },
        Mode::Normal => handle_normal_key(app, key),
    }
}

fn handle_comment_key(app: &mut App, key: KeyEvent) -> Vec<Effect> {
    match key.code {
        KeyCode::Esc => app.reduce(Action::CancelModal),
        KeyCode::Enter => app.reduce(Action::SubmitComment),
        KeyCode::Backspace => app.reduce(Action::CommentBackspace),
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.reduce(Action::CommentClear)
        }
        KeyCode::Char(character)
            if !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            app.reduce(Action::CommentCharacter(character))
        }
        _ => Vec::new(),
    }
}

fn handle_quick_add_key(app: &mut App, key: KeyEvent) -> Vec<Effect> {
    match key.code {
        KeyCode::Esc => app.reduce(Action::CancelModal),
        KeyCode::Enter => app.reduce(Action::SubmitQuickAdd),
        KeyCode::Backspace => app.reduce(Action::QuickAddBackspace),
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.reduce(Action::QuickAddClear)
        }
        KeyCode::Char(character)
            if !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            app.reduce(Action::QuickAddCharacter(character))
        }
        _ => Vec::new(),
    }
}

fn handle_normal_key(app: &mut App, key: KeyEvent) -> Vec<Effect> {
    match key.code {
        KeyCode::Char('q') => app.reduce(Action::Quit),
        KeyCode::Char('r') => app.reduce(Action::Reload),
        KeyCode::Char('?') => app.reduce(Action::ToggleHelp),
        KeyCode::Char('n') => app.reduce(Action::OpenQuickAdd),
        KeyCode::Char('c') => app.reduce(Action::OpenComment),
        KeyCode::Tab | KeyCode::BackTab => app.reduce(Action::ToggleFocus),
        KeyCode::Char('[') => app.reduce(Action::MoveSelected(-1)),
        KeyCode::Char(']') => app.reduce(Action::MoveSelected(1)),
        KeyCode::Char(' ') => app.reduce(Action::ToggleSelectedChecklist),
        KeyCode::Enter if app.focus == Focus::Board => app.reduce(Action::Focus(Focus::Detail)),
        KeyCode::Left | KeyCode::Char('h') if app.focus == Focus::Board => {
            app.reduce(Action::PreviousColumn)
        }
        KeyCode::Right | KeyCode::Char('l') if app.focus == Focus::Board => {
            app.reduce(Action::NextColumn)
        }
        KeyCode::Up | KeyCode::Char('k') if app.focus == Focus::Board => {
            app.reduce(Action::PreviousCard)
        }
        KeyCode::Down | KeyCode::Char('j') if app.focus == Focus::Board => {
            app.reduce(Action::NextCard)
        }
        KeyCode::Up | KeyCode::Char('k') if app.focus == Focus::Detail => {
            app.reduce(Action::PreviousChecklist)
        }
        KeyCode::Down | KeyCode::Char('j') if app.focus == Focus::Detail => {
            app.reduce(Action::NextChecklist)
        }
        KeyCode::PageUp if app.focus == Focus::Detail => app.reduce(Action::ScrollDetail(-8)),
        KeyCode::PageDown if app.focus == Focus::Detail => app.reduce(Action::ScrollDetail(8)),
        KeyCode::Esc => {
            app.drag = None;
            app.error = None;
            Vec::new()
        }
        _ => Vec::new(),
    }
}

fn handle_mouse(app: &mut App, mouse: MouseEvent, hit_map: &HitMap) -> Vec<Effect> {
    // Modal overlays intentionally block the board beneath them. Their hit map is still retained
    // for the next normal frame, so gate interaction explicitly rather than allowing a click or
    // drag through a visible prompt/help panel.
    if app.mode != Mode::Normal {
        return Vec::new();
    }
    let target = hit_map.target_at(mouse.column, mouse.row).cloned();
    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => match target {
            Some(HitTarget::Checklist {
                card_id,
                global_index,
            }) => {
                app.reduce(Action::Focus(Focus::Detail));
                app.reduce(Action::ToggleChecklist {
                    card_id,
                    global_index,
                })
            }
            Some(HitTarget::Card { id, column, row }) => {
                app.reduce(Action::Focus(Focus::Board));
                app.reduce(Action::BeginDrag { id, column, row })
            }
            Some(HitTarget::Column(column)) => {
                app.reduce(Action::Focus(Focus::Board));
                app.reduce(Action::SelectColumn(column))
            }
            Some(HitTarget::DetailPane) => app.reduce(Action::Focus(Focus::Detail)),
            Some(HitTarget::BoardPane) => app.reduce(Action::Focus(Focus::Board)),
            None => Vec::new(),
        },
        MouseEventKind::Drag(MouseButton::Left) => {
            let column = hit_map.column_at(mouse.column, mouse.row);
            app.reduce(Action::DragOver(column))
        }
        MouseEventKind::Up(MouseButton::Left) => {
            let column = hit_map.column_at(mouse.column, mouse.row);
            app.reduce(Action::DropOn(column))
        }
        MouseEventKind::ScrollUp => {
            if hit_map.is_detail_at(mouse.column, mouse.row) {
                app.reduce(Action::Focus(Focus::Detail));
                app.reduce(Action::ScrollDetail(-3))
            } else if let Some(column) = hit_map.column_at(mouse.column, mouse.row) {
                app.reduce(Action::Focus(Focus::Board));
                app.reduce(Action::SelectColumn(column));
                app.reduce(Action::PreviousCard)
            } else {
                Vec::new()
            }
        }
        MouseEventKind::ScrollDown => {
            if hit_map.is_detail_at(mouse.column, mouse.row) {
                app.reduce(Action::Focus(Focus::Detail));
                app.reduce(Action::ScrollDetail(3))
            } else if let Some(column) = hit_map.column_at(mouse.column, mouse.row) {
                app.reduce(Action::Focus(Focus::Board));
                app.reduce(Action::SelectColumn(column));
                app.reduce(Action::NextCard)
            } else {
                Vec::new()
            }
        }
        MouseEventKind::ScrollLeft => app.reduce(Action::PreviousColumn),
        MouseEventKind::ScrollRight => app.reduce(Action::NextColumn),
        MouseEventKind::Down(_)
        | MouseEventKind::Up(_)
        | MouseEventKind::Drag(_)
        | MouseEventKind::Moved => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::KeyModifiers;
    use ratatui::layout::Rect;
    use tempfile::tempdir;

    use crate::config::BoardConfig;
    use crate::store::{CreateCard, Project};

    use super::*;

    #[test]
    fn modal_overlays_block_mouse_actions_on_the_board_beneath_them() {
        let directory = tempdir().unwrap();
        let project = Project::init(
            directory.path(),
            &BoardConfig::new("Mouse test", "T", vec!["Todo".to_owned()]),
        )
        .unwrap();
        let card = project
            .create_card(CreateCard {
                title: "Card".to_owned(),
                body: "- [ ] item\n".to_owned(),
                ..CreateCard::default()
            })
            .unwrap();
        let mut app = App::new(project, vec![card.clone()]);
        let mut hit_map = HitMap::default();
        hit_map.push(
            Rect::new(0, 0, 5, 1),
            HitTarget::Checklist {
                card_id: card.metadata.id.clone(),
                global_index: 1,
            },
        );
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 1,
            row: 0,
            modifiers: KeyModifiers::NONE,
        };

        app.mode = Mode::QuickAdd {
            title: String::new(),
        };
        assert!(handle_mouse(&mut app, click, &hit_map).is_empty());
        app.mode = Mode::AddComment {
            card_id: card.metadata.id.clone(),
            author: "Mouse Author".to_owned(),
            text: "draft".to_owned(),
        };
        assert!(handle_mouse(&mut app, click, &hit_map).is_empty());
        app.mode = Mode::Help;
        assert!(handle_mouse(&mut app, click, &hit_map).is_empty());
        assert!(app.drag.is_none());
    }

    #[test]
    fn comment_composer_keys_type_command_letters_and_control_the_draft() {
        let directory = tempdir().unwrap();
        let project = Project::init(
            directory.path(),
            &BoardConfig::new("Key test", "K", vec!["Todo".to_owned()]),
        )
        .unwrap();
        let card = project
            .create_card(CreateCard {
                title: "Card".to_owned(),
                ..CreateCard::default()
            })
            .unwrap();
        let mut app = App::new(project, vec![card]);
        app.mode = Mode::AddComment {
            card_id: "K-1".to_owned(),
            author: "Key Author".to_owned(),
            text: String::new(),
        };

        assert!(
            handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE)
            )
            .is_empty()
        );
        assert!(
            handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE)
            )
            .is_empty()
        );
        assert!(matches!(
            app.mode,
            Mode::AddComment { ref text, .. } if text == "cr"
        ));

        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
        );
        assert!(matches!(
            app.mode,
            Mode::AddComment { ref text, .. } if text == "c"
        ));
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL),
        );
        assert!(matches!(
            app.mode,
            Mode::AddComment { ref text, .. } if text.is_empty()
        ));

        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE),
        );
        assert_eq!(
            handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            vec![Effect::AddComment {
                id: "K-1".to_owned(),
                author: "Key Author".to_owned(),
                text: "x".to_owned(),
            }]
        );
        assert!(matches!(app.mode, Mode::AddComment { .. }));

        assert_eq!(
            handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)
            ),
            vec![Effect::Quit]
        );
        assert!(handle_key(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)).is_empty());
        assert_eq!(app.mode, Mode::Normal);
    }
}
