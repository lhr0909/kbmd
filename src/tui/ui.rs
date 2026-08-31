use std::collections::HashMap;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::markdown;
use crate::model::Card;

use super::app::{App, Focus, Mode};

const MIN_COLUMN_WIDTH: u16 = 22;
const COLUMN_GAP: u16 = 1;
const CARD_HEIGHT: u16 = 4;
const CARD_GAP: u16 = 1;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum HitTarget {
    BoardPane,
    DetailPane,
    Column(usize),
    Card {
        id: String,
        column: usize,
        row: usize,
    },
    Checklist {
        card_id: String,
        global_index: usize,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct HitRegion {
    rect: Rect,
    target: HitTarget,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct HitMap {
    regions: Vec<HitRegion>,
}

impl HitMap {
    pub fn push(&mut self, rect: Rect, target: HitTarget) {
        if rect.width > 0 && rect.height > 0 {
            self.regions.push(HitRegion { rect, target });
        }
    }

    /// Returns the most specific region, since children are registered after their parent.
    pub fn target_at(&self, x: u16, y: u16) -> Option<&HitTarget> {
        let position = Position::new(x, y);
        self.regions
            .iter()
            .rev()
            .find(|region| region.rect.contains(position))
            .map(|region| &region.target)
    }

    pub fn column_at(&self, x: u16, y: u16) -> Option<usize> {
        let position = Position::new(x, y);
        self.regions.iter().rev().find_map(|region| {
            if region.rect.contains(position)
                && let HitTarget::Column(column) = region.target
            {
                return Some(column);
            }
            None
        })
    }

    pub fn is_detail_at(&self, x: u16, y: u16) -> bool {
        let position = Position::new(x, y);
        self.regions.iter().any(|region| {
            region.rect.contains(position) && matches!(region.target, HitTarget::DetailPane)
        })
    }
}

pub(crate) fn render(frame: &mut Frame<'_>, app: &App) -> HitMap {
    let area = frame.area();
    let [header, main, status, footer] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(3),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(area);

    render_header(frame, app, header);
    let mut hit_map = HitMap::default();
    render_main(frame, app, main, &mut hit_map);
    render_status(frame, app, status);
    render_footer(frame, app, footer);

    match &app.mode {
        Mode::QuickAdd { title } => render_quick_add(frame, app, title),
        Mode::Help => render_help(frame),
        Mode::Normal => {}
    }

    hit_map
}

fn render_header(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let cards = app.cards.len();
    let title = format!(" {}  ·  {cards} cards ", app.project.config.name);
    let line = Paragraph::new(Line::styled(
        truncate(&title, area.width as usize),
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    ))
    .block(Block::default().borders(Borders::BOTTOM));
    frame.render_widget(line, area);
}

fn render_main(frame: &mut Frame<'_>, app: &App, area: Rect, hit_map: &mut HitMap) {
    if area.width >= 86 {
        let detail_width = (area.width / 3).clamp(30, 52);
        let [board, detail] =
            Layout::horizontal([Constraint::Min(40), Constraint::Length(detail_width)])
                .spacing(1)
                .areas(area);
        render_board(frame, app, board, hit_map);
        render_detail(frame, app, detail, hit_map);
    } else {
        match app.focus {
            Focus::Board => render_board(frame, app, area, hit_map),
            Focus::Detail => render_detail(frame, app, area, hit_map),
        }
    }
}

fn render_board(frame: &mut Frame<'_>, app: &App, area: Rect, hit_map: &mut HitMap) {
    hit_map.push(area, HitTarget::BoardPane);

    if app.project.config.columns.is_empty() || area.width == 0 || area.height == 0 {
        return;
    }

    let columns = column_rects(app, area);
    for (column_index, rect) in columns {
        render_column(frame, app, column_index, rect, hit_map);
    }
}

fn column_rects(app: &App, area: Rect) -> Vec<(usize, Rect)> {
    let total = app.project.config.columns.len();
    if total == 0 || area.width == 0 {
        return Vec::new();
    }
    let capacity = ((area.width.saturating_add(COLUMN_GAP))
        / MIN_COLUMN_WIDTH.saturating_add(COLUMN_GAP))
    .max(1) as usize;
    let visible = capacity.min(total);
    let max_offset = total.saturating_sub(visible);
    let offset = if app.active_column < visible {
        0
    } else {
        app.active_column.saturating_add(1).saturating_sub(visible)
    }
    .min(max_offset);

    let gaps = COLUMN_GAP.saturating_mul(visible.saturating_sub(1) as u16);
    let available = area.width.saturating_sub(gaps);
    let base = available / visible as u16;
    let remainder = available % visible as u16;
    let mut x = area.x;
    (0..visible)
        .map(|visible_index| {
            let width = base + u16::from((visible_index as u16) < remainder);
            let rect = Rect::new(x, area.y, width, area.height);
            x = x.saturating_add(width).saturating_add(COLUMN_GAP);
            (offset + visible_index, rect)
        })
        .collect()
}

fn render_column(
    frame: &mut Frame<'_>,
    app: &App,
    column_index: usize,
    area: Rect,
    hit_map: &mut HitMap,
) {
    let Some(column) = app.project.config.columns.get(column_index) else {
        return;
    };
    hit_map.push(area, HitTarget::Column(column_index));
    let cards = app.cards_in_column(column_index);
    let is_active = app.active_column == column_index && app.focus == Focus::Board;
    let is_drop_target = app
        .drag
        .as_ref()
        .is_some_and(|drag| drag.hover_column == column_index);
    let color = if is_drop_target {
        Color::Magenta
    } else if is_active {
        column
            .color
            .as_deref()
            .and_then(parse_color)
            .unwrap_or(Color::Cyan)
    } else {
        Color::DarkGray
    };
    let wip = column
        .wip_limit
        .map(|limit| format!(" / {limit}"))
        .unwrap_or_default();
    let title = format!(" {}  {}{wip} ", column.name, cards.len());
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(color))
        .title(Line::styled(
            truncate(&title, area.width.saturating_sub(2) as usize),
            Style::default().add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if cards.is_empty() {
        let empty = Paragraph::new("Drop a card here").style(Style::default().fg(Color::DarkGray));
        frame.render_widget(empty, inner);
        return;
    }

    let pitch = CARD_HEIGHT + CARD_GAP;
    let visible_count = ((inner.height + CARD_GAP) / pitch).max(1) as usize;
    let selected_row = if app.active_column == column_index {
        app.selected_row.min(cards.len() - 1)
    } else {
        0
    };
    let start = if selected_row < visible_count {
        0
    } else {
        selected_row + 1 - visible_count
    };

    for (visible_row, (row, card)) in cards
        .iter()
        .enumerate()
        .skip(start)
        .take(visible_count)
        .enumerate()
    {
        let y = inner.y.saturating_add(visible_row as u16 * pitch);
        let height = CARD_HEIGHT.min(inner.bottom().saturating_sub(y));
        if height < 3 {
            break;
        }
        let rect = Rect::new(inner.x, y, inner.width, height);
        hit_map.push(
            rect,
            HitTarget::Card {
                id: card.metadata.id.clone(),
                column: column_index,
                row,
            },
        );
        render_card(frame, app, card, rect);
    }
}

fn render_card(frame: &mut Frame<'_>, app: &App, card: &Card, area: Rect) {
    let selected = app
        .selected_id
        .as_deref()
        .is_some_and(|id| card.metadata.id.eq_ignore_ascii_case(id));
    let dragged = app
        .drag
        .as_ref()
        .is_some_and(|drag| card.metadata.id.eq_ignore_ascii_case(&drag.card_id));
    let border = if dragged {
        Color::Magenta
    } else if selected {
        Color::Yellow
    } else {
        Color::Gray
    };
    let mut style = Style::default().fg(Color::White);
    if dragged {
        style = style.add_modifier(Modifier::DIM);
    }
    let inner_width = area.width.saturating_sub(2) as usize;
    let (checked, total) = card.checklist_progress();
    let progress = if total == 0 {
        String::new()
    } else {
        format!("  ✓ {checked}/{total}")
    };
    let details = format!("{}{}", card.metadata.id, progress);
    let paragraph = Paragraph::new(vec![
        Line::styled(
            truncate(&card.metadata.title, inner_width),
            style.add_modifier(Modifier::BOLD),
        ),
        Line::styled(
            truncate(&details, inner_width),
            Style::default().fg(Color::DarkGray),
        ),
    ])
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border)),
    );
    frame.render_widget(paragraph, area);
}

fn render_detail(frame: &mut Frame<'_>, app: &App, area: Rect, hit_map: &mut HitMap) {
    hit_map.push(area, HitTarget::DetailPane);
    let focus_color = if app.focus == Focus::Detail {
        Color::Cyan
    } else {
        Color::DarkGray
    };
    let Some(card) = app.selected_card() else {
        frame.render_widget(
            Paragraph::new("Select a card to inspect its frontmatter and Markdown.").block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(focus_color))
                    .title(" Card detail "),
            ),
            area,
        );
        return;
    };

    let (checked, total) = card.checklist_progress();
    let progress = if total == 0 {
        String::new()
    } else {
        format!(" · {checked}/{total}")
    };
    let title = format!(" {} · {}{progress} ", card.metadata.id, card.metadata.title);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(focus_color))
        .title(truncate(&title, area.width.saturating_sub(2) as usize));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 || inner.width == 0 {
        return;
    }

    let lines = detail_lines(card);
    let visible = inner.height as usize;
    let max_start = lines.len().saturating_sub(visible);
    let selected_global = app.selected_checklist_global();
    let mut offset = app.detail_scroll.min(max_start);
    if app.detail_follow_cursor
        && let Some(selected_global) = selected_global
        && let Some(selected_line) = lines
            .iter()
            .position(|line| line.checklist == Some(selected_global))
    {
        if selected_line < offset {
            offset = selected_line;
        } else if selected_line >= offset + visible {
            offset = selected_line + 1 - visible;
        }
    }

    for (visible_row, line) in lines.iter().skip(offset).take(visible).enumerate() {
        let row = Rect::new(inner.x, inner.y + visible_row as u16, inner.width, 1);
        let is_selected = line.checklist.is_some() && line.checklist == selected_global;
        let mut style = line.style;
        if is_selected {
            style = style
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD);
        }
        frame.render_widget(
            Paragraph::new(Line::styled(
                truncate(&line.text, inner.width as usize),
                style,
            )),
            row,
        );
        if let Some(global_index) = line.checklist {
            hit_map.push(
                row,
                HitTarget::Checklist {
                    card_id: card.metadata.id.clone(),
                    global_index,
                },
            );
        }
    }
}

#[derive(Clone, Debug)]
struct DetailLine {
    text: String,
    style: Style,
    checklist: Option<usize>,
}

fn detail_lines(card: &Card) -> Vec<DetailLine> {
    let mut result = vec![DetailLine {
        text: "FRONTMATTER".to_owned(),
        style: Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
        checklist: None,
    }];
    let yaml = serde_saphyr::to_string(&card.metadata)
        .unwrap_or_else(|error| format!("could not render frontmatter: {error}"));
    result.extend(yaml.trim_end().lines().map(|line| DetailLine {
        text: line.to_owned(),
        style: Style::default().fg(Color::Gray),
        checklist: None,
    }));
    result.push(DetailLine {
        text: String::new(),
        style: Style::default(),
        checklist: None,
    });
    result.push(DetailLine {
        text: "MARKDOWN".to_owned(),
        style: Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
        checklist: None,
    });

    if card.body.is_empty() {
        result.push(DetailLine {
            text: "(empty body)".to_owned(),
            style: Style::default().fg(Color::DarkGray),
            checklist: None,
        });
        return result;
    }

    let checklists = markdown::checklist_items(&card.body)
        .into_iter()
        .map(|item| (item.line_number, item.global_index))
        .collect::<HashMap<_, _>>();
    result.extend(card.body.lines().enumerate().map(|(index, line)| {
        let checklist = checklists.get(&(index + 1)).copied();
        let style = if checklist.is_some() {
            Style::default().fg(Color::White)
        } else if line.trim_start().starts_with('#') {
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Gray)
        };
        DetailLine {
            text: line.to_owned(),
            style,
            checklist,
        }
    }));
    result
}

fn render_status(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let (message, style) = if let Some(error) = &app.error {
        (
            format!(" Error: {error}"),
            Style::default()
                .fg(Color::White)
                .bg(Color::Red)
                .add_modifier(Modifier::BOLD),
        )
    } else if let Some(message) = &app.message {
        (format!(" {message}"), Style::default().fg(Color::Green))
    } else if let Some(drag) = &app.drag {
        (
            format!(" Drag {} onto a column and release", drag.card_id),
            Style::default().fg(Color::Magenta),
        )
    } else {
        (String::new(), Style::default())
    };
    frame.render_widget(
        Paragraph::new(Line::styled(truncate(&message, area.width as usize), style)),
        area,
    );
}

fn render_footer(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let help = match app.focus {
        Focus::Board => {
            " ←/→ or h/l columns · ↑/↓ or j/k cards · [/] move · n add · r reload · Tab detail · ? help · q quit"
        }
        Focus::Detail => {
            " ↑/↓ or j/k checklist · Space toggle · wheel scroll · r reload · Tab board · ? help · q quit"
        }
    };
    frame.render_widget(
        Paragraph::new(truncate(help, area.width as usize))
            .style(Style::default().fg(Color::DarkGray)),
        area,
    );
}

fn render_quick_add(frame: &mut Frame<'_>, app: &App, title: &str) {
    let area = centered_rect(frame.area(), 64, 5);
    frame.render_widget(Clear, area);
    let status = app
        .project
        .config
        .columns
        .get(app.active_column)
        .map_or("?", |column| column.name.as_str());
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow))
        .title(format!(" New card in {status} "));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [input, hint, _] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .areas(inner);
    let shown = truncate(title, input.width.saturating_sub(1) as usize);
    frame.render_widget(Paragraph::new(shown.clone()), input);
    frame.render_widget(
        Paragraph::new("Enter create · Esc cancel · Ctrl-U clear")
            .style(Style::default().fg(Color::DarkGray)),
        hint,
    );
    let cursor = input
        .x
        .saturating_add(UnicodeWidthStr::width(shown.as_str()) as u16)
        .min(input.right().saturating_sub(1));
    frame.set_cursor_position((cursor, input.y));
}

fn render_help(frame: &mut Frame<'_>) {
    let area = centered_rect(frame.area(), 68, 20);
    frame.render_widget(Clear, area);
    let text = [
        "Keyboard",
        "  ←/→, h/l        select column",
        "  ↑/↓, k/j        select card or checklist",
        "  [ / ]           move card one column",
        "  Tab             switch board/detail focus",
        "  Space           toggle selected checklist item",
        "  n               quick-add a card",
        "  r               reload files now",
        "  ?               close this help",
        "  q               quit",
        "",
        "Mouse",
        "  click card/checklist to select or toggle",
        "  wheel to navigate cards or scroll detail",
        "  drag a card and release over another column to move",
        "",
        "Files reload after a short debounce; invalid edits keep the last good board.",
    ]
    .join("\n");
    frame.render_widget(
        Paragraph::new(text).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan))
                .title(" Help "),
        ),
        area,
    );
}

fn centered_rect(area: Rect, preferred_width: u16, preferred_height: u16) -> Rect {
    if area.width == 0 || area.height == 0 {
        return area;
    }
    let width = preferred_width.min(area.width.saturating_sub(2)).max(1);
    let height = preferred_height.min(area.height.saturating_sub(2)).max(1);
    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    )
}

fn parse_color(value: &str) -> Option<Color> {
    let value = value.trim().to_ascii_lowercase();
    match value.as_str() {
        "black" => Some(Color::Black),
        "red" => Some(Color::Red),
        "green" => Some(Color::Green),
        "yellow" => Some(Color::Yellow),
        "blue" => Some(Color::Blue),
        "magenta" | "purple" => Some(Color::Magenta),
        "cyan" => Some(Color::Cyan),
        "gray" | "grey" => Some(Color::Gray),
        "darkgray" | "dark-gray" | "dark_gray" => Some(Color::DarkGray),
        "white" => Some(Color::White),
        _ => parse_rgb(&value),
    }
}

fn parse_rgb(value: &str) -> Option<Color> {
    let hexadecimal = value.strip_prefix('#')?;
    if hexadecimal.len() != 6
        || !hexadecimal.is_ascii()
        || !hexadecimal.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return None;
    }
    let red = u8::from_str_radix(&hexadecimal[0..2], 16).ok()?;
    let green = u8::from_str_radix(&hexadecimal[2..4], 16).ok()?;
    let blue = u8::from_str_radix(&hexadecimal[4..6], 16).ok()?;
    Some(Color::Rgb(red, green, blue))
}

fn truncate(value: &str, width: usize) -> String {
    if UnicodeWidthStr::width(value) <= width {
        return value.to_owned();
    }
    if width == 0 {
        return String::new();
    }
    if width == 1 {
        return "…".to_owned();
    }
    let available = width - 1;
    let mut result = String::new();
    let mut used = 0;
    for character in value.chars() {
        let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
        if used + character_width > available {
            break;
        }
        result.push(character);
        used += character_width;
    }
    result.push('…');
    result
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use tempfile::tempdir;

    use crate::config::BoardConfig;
    use crate::store::{CreateCard, Project};

    use super::*;

    fn app() -> (tempfile::TempDir, App) {
        let directory = tempdir().unwrap();
        let project = Project::init(
            directory.path(),
            &BoardConfig::new(
                "Flexible board",
                "KB",
                vec!["Ideas".to_owned(), "Building".to_owned(), "Done".to_owned()],
            ),
        )
        .unwrap();
        project
            .create_card(CreateCard {
                title: "Mouse-friendly card".to_owned(),
                body: "## Bespoke plan\n\n- [ ] Flexible item\n- [x] Finished item\n".to_owned(),
                ..CreateCard::default()
            })
            .unwrap();
        let cards = project.load_cards().unwrap();
        (directory, App::new(project, cards))
    }

    #[test]
    fn hit_map_prefers_card_over_containing_column_and_is_half_open() {
        let mut map = HitMap::default();
        map.push(Rect::new(0, 0, 20, 10), HitTarget::Column(0));
        map.push(
            Rect::new(1, 2, 18, 4),
            HitTarget::Card {
                id: "KB-1".to_owned(),
                column: 0,
                row: 0,
            },
        );

        assert!(matches!(map.target_at(2, 3), Some(HitTarget::Card { .. })));
        assert_eq!(map.column_at(2, 3), Some(0));
        assert_eq!(map.target_at(20, 3), None);
    }

    #[test]
    fn render_contains_columns_progress_and_arbitrary_markdown() {
        let (_directory, mut app) = app();
        app.focus = Focus::Detail;
        let backend = TestBackend::new(110, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hit_map = HitMap::default();

        terminal
            .draw(|frame| hit_map = render(frame, &app))
            .unwrap();

        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(rendered.contains("Ideas"));
        assert!(rendered.contains("1/2"));
        assert!(rendered.contains("Bespoke plan"));
        assert!(hit_map.regions.iter().any(|region| matches!(
            region.target,
            HitTarget::Checklist {
                global_index: 1,
                ..
            }
        )));
    }

    #[test]
    fn board_columns_fill_the_unframed_board_pane() {
        let (_directory, app) = app();
        let backend = TestBackend::new(80, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hit_map = HitMap::default();

        terminal
            .draw(|frame| hit_map = render(frame, &app))
            .unwrap();

        let board = hit_map
            .regions
            .iter()
            .find(|region| matches!(region.target, HitTarget::BoardPane))
            .unwrap()
            .rect;
        let columns = hit_map
            .regions
            .iter()
            .filter_map(|region| {
                matches!(region.target, HitTarget::Column(_)).then_some(region.rect)
            })
            .collect::<Vec<_>>();

        assert!(!columns.is_empty());
        assert_eq!(columns.first().unwrap().x, board.x);
        assert_eq!(columns.last().unwrap().right(), board.right());
        assert!(
            columns
                .iter()
                .all(|column| column.y == board.y && column.height == board.height)
        );
    }

    #[test]
    fn truncate_respects_wide_characters() {
        assert_eq!(truncate("ab界cd", 5), "ab界…");
        assert_eq!(UnicodeWidthStr::width(truncate("ab界cd", 5).as_str()), 5);
    }
}
