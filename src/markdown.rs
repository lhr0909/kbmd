//! Conservative mutations for user-defined Markdown sections and checklists.
//!
//! The parser deliberately recognizes CommonMark ATX headings and task-list markers only outside
//! code blocks. Mutations touch the smallest possible body range and leave unrelated sections
//! byte-for-byte unchanged.

use std::collections::HashSet;

use anyhow::{Result, bail};
use pulldown_cmark::{Event, Options, Parser, Tag};

use crate::comments;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section {
    pub title: String,
    pub level: u8,
    pub start: usize,
    pub content_start: usize,
    pub end: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChecklistItem {
    pub section: Option<String>,
    /// One-based position within the containing section (or unsectioned body).
    pub index: usize,
    /// One-based position among every checkbox in the document.
    pub global_index: usize,
    pub text: String,
    pub checked: bool,
    pub line_number: usize,
    checkbox_offset: usize,
    line_start: usize,
    line_end: usize,
}

#[derive(Clone, Debug)]
struct LineInfo<'a> {
    start: usize,
    end: usize,
    text: &'a str,
    number: usize,
}

pub fn sections(body: &str) -> Vec<Section> {
    let lines = lines(body);
    let (commonmark_headings, _) = commonmark_offsets(body);
    let mut headings = Vec::<(String, u8, usize, usize)>::new();

    for line in &lines {
        let trimmed = line.text.trim_start();
        if commonmark_headings.contains(&line.start)
            && let Some((level, title)) = atx_heading(trimmed)
        {
            headings.push((title.to_owned(), level, line.start, line.end));
        }
    }

    headings
        .iter()
        .enumerate()
        .map(|(index, (title, level, start, content_start))| {
            let end = headings[index + 1..]
                .iter()
                .find(|(_, next_level, _, _)| next_level <= level)
                .map_or(body.len(), |(_, _, next_start, _)| *next_start);
            Section {
                title: title.clone(),
                level: *level,
                start: *start,
                content_start: *content_start,
                end,
            }
        })
        .collect()
}

pub fn section_content(body: &str, title: &str) -> Result<Option<String>> {
    let Some(section) = unique_section(body, title)? else {
        return Ok(None);
    };
    Ok(Some(
        body[section.content_start..section.end]
            .trim_matches(['\r', '\n'])
            .to_owned(),
    ))
}

pub fn set_section(body: &str, title: &str, content: &str) -> Result<String> {
    validate_heading(title)?;
    ensure_section_writable(body, title)?;
    if let Some(section) = unique_section(body, title)? {
        Ok(replace_section_content(body, &section, content))
    } else {
        let mut result = body.trim_end_matches(['\r', '\n']).to_owned();
        if !result.is_empty() {
            result.push_str("\n\n");
        }
        result.push_str("## ");
        result.push_str(title.trim());
        if !content.trim().is_empty() {
            result.push_str("\n\n");
            result.push_str(content.trim_matches(['\r', '\n']));
        }
        result.push('\n');
        Ok(result)
    }
}

pub fn append_section(body: &str, title: &str, content: &str) -> Result<String> {
    if content.trim().is_empty() {
        return Ok(body.to_owned());
    }
    let existing = section_content(body, title)?.unwrap_or_default();
    let next = if existing.trim().is_empty() {
        content.trim_matches(['\r', '\n']).to_owned()
    } else {
        format!(
            "{}\n\n{}",
            existing.trim_end_matches(['\r', '\n']),
            content.trim_matches(['\r', '\n'])
        )
    };
    set_section(body, title, &next)
}

pub fn remove_section(body: &str, title: &str) -> Result<String> {
    ensure_section_writable(body, title)?;
    let Some(section) = unique_section(body, title)? else {
        bail!("section {title:?} was not found");
    };
    let mut result = String::with_capacity(body.len() - (section.end - section.start));
    result.push_str(body[..section.start].trim_end_matches(['\r', '\n']));
    if !result.is_empty()
        && !body[section.end..]
            .trim_start_matches(['\r', '\n'])
            .is_empty()
    {
        result.push_str("\n\n");
    }
    result.push_str(body[section.end..].trim_start_matches(['\r', '\n']));
    if !result.is_empty() && body.ends_with('\n') {
        result.push('\n');
    }
    Ok(result)
}

pub fn checklist_items(body: &str) -> Vec<ChecklistItem> {
    let body_sections = sections(body);
    let (_, commonmark_tasks) = commonmark_offsets(body);
    let mut counts = vec![0_usize; body_sections.len() + 1];
    let mut items = Vec::new();

    for line in lines(body) {
        let trimmed = line.text.trim_start();
        let Some((relative_checkbox_offset, checked, text)) = checkbox(trimmed) else {
            continue;
        };
        let indentation = line.text.len() - trimmed.len();
        let checkbox_offset = line.start + indentation + relative_checkbox_offset;
        if !commonmark_tasks.contains(&checkbox_offset) {
            continue;
        }
        let containing = body_sections
            .iter()
            .enumerate()
            .filter(|(_, section)| line.start >= section.content_start && line.start < section.end)
            .max_by_key(|(_, section)| section.level)
            .map(|(index, section)| (index + 1, Some(section.title.clone())));
        let (count_slot, section) = containing.unwrap_or((0, None));
        counts[count_slot] += 1;
        items.push(ChecklistItem {
            section,
            index: counts[count_slot],
            global_index: items.len() + 1,
            text: text.to_owned(),
            checked,
            line_number: line.number,
            checkbox_offset,
            line_start: line.start,
            line_end: line.end,
        });
    }

    items
}

pub fn add_checklist_item(body: &str, section: &str, text: &str) -> Result<String> {
    if text.trim().is_empty() {
        bail!("checklist text cannot be empty");
    }
    let line = format!("- [ ] {}", text.trim());
    let Some(target) = unique_section(body, section)? else {
        return set_section(body, section, &line);
    };
    let insertion_end = sections(body)
        .into_iter()
        .find(|candidate| {
            candidate.start >= target.content_start
                && candidate.start < target.end
                && candidate.level > target.level
        })
        .map_or(target.end, |child| child.start);
    ensure_range_writable(body, target.content_start..insertion_end, section)?;
    let direct_content = body[target.content_start..insertion_end].trim_matches(['\r', '\n']);
    let separator = if direct_content.is_empty() {
        ""
    } else if direct_content
        .lines()
        .next_back()
        .and_then(|last| checkbox(last.trim_start()))
        .is_some()
    {
        "\n"
    } else {
        "\n\n"
    };
    let replacement = format!("\n{direct_content}{separator}{line}\n");
    let mut result = String::with_capacity(body.len() + replacement.len());
    result.push_str(&body[..target.content_start]);
    result.push_str(&replacement);
    if insertion_end < body.len() {
        result.push('\n');
        result.push_str(body[insertion_end..].trim_start_matches(['\r', '\n']));
    }
    if body.ends_with('\n') && !result.ends_with('\n') {
        result.push('\n');
    }
    Ok(result)
}

pub fn set_checklist_item(
    body: &str,
    section: &str,
    index: usize,
    checked: bool,
) -> Result<String> {
    let item = checklist_in_section(body, section, index)?;
    replace_checkbox(body, &item, checked)
}

pub fn toggle_checklist_item(body: &str, section: &str, index: usize) -> Result<String> {
    let item = checklist_in_section(body, section, index)?;
    replace_checkbox(body, &item, !item.checked)
}

pub fn toggle_checklist_global(body: &str, global_index: usize) -> Result<String> {
    let Some(item) = checklist_items(body)
        .into_iter()
        .find(|item| item.global_index == global_index)
    else {
        bail!("checklist item {global_index} was not found");
    };
    ensure_item_writable(body, &item)?;
    replace_checkbox(body, &item, !item.checked)
}

pub fn set_checklist_global(body: &str, global_index: usize, checked: bool) -> Result<String> {
    let Some(item) = checklist_items(body)
        .into_iter()
        .find(|item| item.global_index == global_index)
    else {
        bail!("checklist item {global_index} was not found");
    };
    ensure_item_writable(body, &item)?;
    replace_checkbox(body, &item, checked)
}

pub fn remove_checklist_global(body: &str, global_index: usize) -> Result<String> {
    let Some(item) = checklist_items(body)
        .into_iter()
        .find(|item| item.global_index == global_index)
    else {
        bail!("checklist item {global_index} was not found");
    };
    ensure_item_writable(body, &item)?;
    let mut result = String::with_capacity(body.len() - (item.line_end - item.line_start));
    result.push_str(&body[..item.line_start]);
    result.push_str(&body[item.line_end..]);
    Ok(result)
}

pub fn remove_checklist_item(body: &str, section: &str, index: usize) -> Result<String> {
    let item = checklist_in_section(body, section, index)?;
    let mut result = String::with_capacity(body.len() - (item.line_end - item.line_start));
    result.push_str(&body[..item.line_start]);
    result.push_str(&body[item.line_end..]);
    Ok(result)
}

fn unique_section(body: &str, title: &str) -> Result<Option<Section>> {
    let matches = sections(body)
        .into_iter()
        .filter(|section| section.title.eq_ignore_ascii_case(title.trim()))
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [] => Ok(None),
        [section] => Ok(Some(section.clone())),
        _ => bail!(
            "section {title:?} is ambiguous because it appears {} times",
            matches.len()
        ),
    }
}

fn replace_section_content(body: &str, section: &Section, content: &str) -> String {
    let mut result = String::with_capacity(body.len() + content.len());
    result.push_str(&body[..section.content_start]);
    if !content.trim().is_empty() {
        result.push('\n');
        result.push_str(content.trim_matches(['\r', '\n']));
        result.push('\n');
    }
    if section.end < body.len() {
        result.push('\n');
        result.push_str(body[section.end..].trim_start_matches(['\r', '\n']));
    }
    if body.ends_with('\n') && !result.ends_with('\n') {
        result.push('\n');
    }
    result
}

fn checklist_in_section(body: &str, section: &str, index: usize) -> Result<ChecklistItem> {
    if index == 0 {
        bail!("checklist indexes start at 1");
    }
    unique_section(body, section)?
        .ok_or_else(|| anyhow::anyhow!("section {section:?} was not found"))?;
    let item = checklist_items(body)
        .into_iter()
        .find(|item| {
            item.index == index
                && item
                    .section
                    .as_deref()
                    .is_some_and(|name| name.eq_ignore_ascii_case(section.trim()))
        })
        .ok_or_else(|| {
            anyhow::anyhow!("checklist item {index} was not found in section {section:?}")
        })?;
    ensure_item_writable(body, &item)?;
    Ok(item)
}

fn replace_checkbox(body: &str, item: &ChecklistItem, checked: bool) -> Result<String> {
    if item.checkbox_offset >= body.len() || !body.is_char_boundary(item.checkbox_offset) {
        bail!("internal checklist offset is invalid");
    }
    let mut result = body.to_owned();
    result.replace_range(
        item.checkbox_offset..item.checkbox_offset + 1,
        if checked { "x" } else { " " },
    );
    Ok(result)
}

fn lines(body: &str) -> Vec<LineInfo<'_>> {
    let mut result = Vec::new();
    let mut offset = 0;
    for (index, raw) in body.split_inclusive('\n').enumerate() {
        let text = raw
            .strip_suffix("\r\n")
            .or_else(|| raw.strip_suffix('\n'))
            .unwrap_or(raw);
        result.push(LineInfo {
            start: offset,
            end: offset + raw.len(),
            text,
            number: index + 1,
        });
        offset += raw.len();
    }
    if body.is_empty() {
        return result;
    }
    if !body.ends_with('\n') && result.is_empty() {
        result.push(LineInfo {
            start: 0,
            end: body.len(),
            text: body,
            number: 1,
        });
    }
    result
}

fn commonmark_offsets(body: &str) -> (HashSet<usize>, HashSet<usize>) {
    // pulldown-cmark currently has a reported offset-iterator panic for a few unsupported control
    // bytes. Treat such documents as opaque instead of risking a crash during a read-only view.
    if body.bytes().any(|byte| matches!(byte, b'\0' | 0x0b)) {
        return (HashSet::new(), HashSet::new());
    }

    let mut headings = HashSet::new();
    let mut tasks = HashSet::new();
    let masked = match comments::masked_content(body) {
        Ok(masked) => masked,
        Err(_) => return (headings, tasks),
    };
    let protected_comments = match comments::registry_section_range(body) {
        Ok(range) => range,
        Err(_) => return (headings, tasks),
    };
    let parser = Parser::new_ext(&masked, Options::ENABLE_TASKLISTS).into_offset_iter();
    for (event, range) in parser {
        match event {
            Event::Start(Tag::Heading { .. }) => {
                headings.insert(source_line_start(body, range.start));
            }
            Event::TaskListMarker(_) => {
                if protected_comments.as_ref().is_some_and(|protected| {
                    range.start >= protected.start && range.start < protected.end
                }) {
                    continue;
                }
                let line_start = source_line_start(body, range.start);
                let line_end = body[range.start..]
                    .find('\n')
                    .map_or(body.len(), |offset| range.start + offset);
                let line = &body[line_start..line_end];
                let trimmed = line.trim_start();
                if let Some((relative, _, _)) = checkbox(trimmed) {
                    let indentation = line.len() - trimmed.len();
                    tasks.insert(line_start + indentation + relative);
                }
            }
            _ => {}
        }
    }
    (headings, tasks)
}

fn ensure_section_writable(body: &str, title: &str) -> Result<()> {
    let Some(protected) = comments::registry_section_range(body)? else {
        return Ok(());
    };
    let Some(section) = unique_section(body, title)? else {
        return Ok(());
    };
    if section.start < protected.end && protected.start < section.end {
        bail!(
            "section {title:?} contains structured comments; use `kbmd comment` commands instead"
        );
    }
    Ok(())
}

fn ensure_item_writable(body: &str, item: &ChecklistItem) -> Result<()> {
    let label = item.section.as_deref().unwrap_or("(preamble)");
    ensure_range_writable(body, item.line_start..item.line_end, label)
}

fn ensure_range_writable(
    body: &str,
    candidate: std::ops::Range<usize>,
    section: &str,
) -> Result<()> {
    if let Some(protected) = comments::registry_section_range(body)?
        && candidate.start < protected.end
        && protected.start < candidate.end
    {
        bail!(
            "section {section:?} contains structured comments; use `kbmd comment` commands instead"
        );
    }
    Ok(())
}

fn source_line_start(body: &str, offset: usize) -> usize {
    body[..offset.min(body.len())]
        .rfind('\n')
        .map_or(0, |newline| newline + 1)
}

fn atx_heading(line: &str) -> Option<(u8, &str)> {
    let count = line.bytes().take_while(|byte| *byte == b'#').count();
    if !(1..=6).contains(&count) {
        return None;
    }
    let rest = &line[count..];
    if !rest.is_empty() && !rest.starts_with([' ', '\t']) {
        return None;
    }
    let mut title = rest.trim();
    let trailing_hashes = title.bytes().rev().take_while(|byte| *byte == b'#').count();
    if trailing_hashes > 0 {
        let without_hashes = &title[..title.len() - trailing_hashes];
        if without_hashes.ends_with([' ', '\t']) {
            title = without_hashes.trim_end();
        }
    }
    (!title.is_empty()).then_some((count as u8, title))
}

/// Returns the offset of the checkbox state character relative to `line`.
fn checkbox(line: &str) -> Option<(usize, bool, &str)> {
    let bytes = line.as_bytes();
    if bytes.len() < 5 {
        return None;
    }
    let marker_end = if matches!(bytes[0], b'-' | b'*' | b'+') {
        1
    } else {
        let digits = bytes
            .iter()
            .take_while(|byte| byte.is_ascii_digit())
            .count();
        if digits == 0
            || digits > 9
            || !bytes
                .get(digits)
                .is_some_and(|byte| matches!(byte, b'.' | b')'))
        {
            return None;
        }
        digits + 1
    };
    let whitespace = bytes[marker_end..]
        .iter()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .count();
    if whitespace == 0 {
        return None;
    }
    let bracket = marker_end + whitespace;
    if bytes.get(bracket) != Some(&b'[') {
        return None;
    }
    let checked = match bytes.get(bracket + 1)? {
        b' ' => false,
        b'x' | b'X' => true,
        _ => return None,
    };
    if bytes.get(bracket + 2) != Some(&b']') {
        return None;
    }
    let content_start = bracket + 3;
    if bytes
        .get(content_start)
        .is_some_and(|byte| !matches!(byte, b' ' | b'\t'))
    {
        return None;
    }
    Some((bracket + 1, checked, line[content_start..].trim()))
}

fn validate_heading(title: &str) -> Result<()> {
    if title.trim().is_empty() {
        bail!("section title cannot be empty");
    }
    if title.contains(['\r', '\n']) {
        bail!("section title must fit on one line");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    const BODY: &str = "Intro\n\n## Plan\n\n- [ ] First\n- [x] Second\n\n### Detail\n\n- [ ] Nested\n\n## Notes\n\nKeep this\n";

    #[test]
    fn finds_sections_and_nested_boundaries() {
        let found = sections(BODY);
        assert_eq!(
            found
                .iter()
                .map(|section| (&*section.title, section.level))
                .collect::<Vec<_>>(),
            vec![("Plan", 2), ("Detail", 3), ("Notes", 2)]
        );
        assert_eq!(
            section_content(BODY, "plan").unwrap().unwrap(),
            "- [ ] First\n- [x] Second\n\n### Detail\n\n- [ ] Nested"
        );
    }

    #[test]
    fn ignores_headings_and_checkboxes_inside_fences() {
        let body = "## Real\n\n```md\n## Fake\n- [ ] fake\n```still code\n## Also fake\n```\n\n- [ ] real\n";
        assert_eq!(sections(body).len(), 1);
        let items = checklist_items(body);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].text, "real");
    }

    #[test]
    fn arbitrary_section_mutations_leave_neighbors() {
        let set = set_section(BODY, "Plan", "A new plan").unwrap();
        assert_eq!(
            section_content(&set, "Plan").unwrap().unwrap(),
            "A new plan"
        );
        assert_eq!(
            section_content(&set, "Notes").unwrap().unwrap(),
            "Keep this"
        );

        let appended = append_section(&set, "Ideas", "Try one").unwrap();
        assert_eq!(
            section_content(&appended, "Ideas").unwrap().unwrap(),
            "Try one"
        );

        let removed = remove_section(&appended, "Plan").unwrap();
        assert!(section_content(&removed, "Plan").unwrap().is_none());
        assert_eq!(
            section_content(&removed, "Notes").unwrap().unwrap(),
            "Keep this"
        );
    }

    #[test]
    fn supports_checklists_in_any_section() {
        let toggled = toggle_checklist_item(BODY, "Plan", 1).unwrap();
        assert!(checklist_items(&toggled)[0].checked);

        let added = add_checklist_item(&toggled, "Notes", "Call Alice").unwrap();
        let note_item = checklist_items(&added)
            .into_iter()
            .find(|item| item.section.as_deref() == Some("Notes"))
            .unwrap();
        assert_eq!(note_item.text, "Call Alice");

        let adjacent = add_checklist_item(&toggled, "Plan", "Third").unwrap();
        assert!(adjacent.contains("- [x] First\n- [x] Second\n- [ ] Third\n\n### Detail"));
        assert_eq!(
            checklist_items(&adjacent)
                .iter()
                .filter(|item| item.section.as_deref() == Some("Plan"))
                .count(),
            3
        );

        let removed = remove_checklist_item(&added, "Plan", 2).unwrap();
        assert_eq!(
            checklist_items(&removed)
                .into_iter()
                .filter(|item| item.section.as_deref() == Some("Plan"))
                .count(),
            1
        );
    }

    #[test]
    fn duplicate_section_names_fail_closed() {
        let body = "## Notes\nOne\n\n## notes\nTwo\n";
        assert!(set_section(body, "Notes", "unsafe").is_err());
        assert!(toggle_checklist_item(body, "Notes", 1).is_err());
    }

    #[test]
    fn global_toggle_changes_only_one_state_byte() {
        let toggled = toggle_checklist_global(BODY, 2).unwrap();
        assert_eq!(toggled, BODY.replacen("- [x] Second", "- [ ] Second", 1));

        let checked = set_checklist_global(&toggled, 2, true).unwrap();
        assert_eq!(checked, BODY);

        let removed = remove_checklist_global(BODY, 2).unwrap();
        assert!(!removed.contains("Second"));
        assert!(removed.contains("First"));
    }

    #[test]
    fn preserves_hashes_that_are_part_of_heading_text() {
        let body = "## C#\n\nText\n\n## Closed heading ###\n";
        let found = sections(body);
        assert_eq!(found[0].title, "C#");
        assert_eq!(found[1].title, "Closed heading");
    }

    #[test]
    fn parses_ordered_and_spaced_task_list_markers() {
        let body = "## Checks\n\n1. [ ] ordered\n-   [X] spaced\n* [ ] ordinary\n";
        let items = checklist_items(body);
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].text, "ordered");
        assert!(items[1].checked);

        let toggled = toggle_checklist_global(body, 1).unwrap();
        assert!(toggled.contains("1. [x] ordered"));
    }

    #[test]
    fn ignores_indented_code_but_keeps_real_nested_task_lists() {
        let body = "## Real\n\n    ## code heading\n    - [ ] literal code\n\n- parent\n  - [ ] nested task\n";
        let found = sections(body);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].title, "Real");
        let items = checklist_items(body);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].text, "nested task");
    }

    #[test]
    fn indented_fence_literal_does_not_hide_later_structure() {
        let body = "    ```literal indented code\n## Real heading\n\n- [ ] real task\n";
        let found = sections(body);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].title, "Real heading");
        let items = checklist_items(body);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].text, "real task");
    }

    #[test]
    fn unsupported_control_bytes_are_treated_as_opaque() {
        let body = "## Before\n\0\n- [ ] unsafe";
        assert!(sections(body).is_empty());
        assert!(checklist_items(body).is_empty());
    }

    #[test]
    fn comment_markdown_is_opaque_to_sections_and_checklists() {
        let base = "# Project\n\n- [ ] real task\n\n## Comments\n\nLegacy context\n";
        let (body, comment) = comments::append(
            base,
            "Alice",
            "### Suggested work\n\n- [ ] this is discussion, not a task",
        )
        .unwrap();

        assert_eq!(
            sections(&body)
                .iter()
                .map(|section| section.title.as_str())
                .collect::<Vec<_>>(),
            vec!["Project", "Comments"]
        );
        let items = checklist_items(&body);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].text, "real task");
        assert_eq!(
            comments::find(&body, &comment.id).unwrap().body,
            comment.body
        );
    }

    #[test]
    fn parent_checklists_remain_editable_while_comment_section_is_protected() {
        let base = "# Project\n\n- [ ] real task\n\n## Comments\n\nLegacy context\n";
        let (body, comment) = comments::append(base, "Alice", "- [ ] discussion only").unwrap();

        let toggled = toggle_checklist_item(&body, "Project", 1).unwrap();
        assert!(checklist_items(&toggled)[0].checked);
        let globally_toggled = toggle_checklist_global(&toggled, 1).unwrap();
        assert!(!checklist_items(&globally_toggled)[0].checked);
        let added = add_checklist_item(&globally_toggled, "Project", "second real task").unwrap();
        assert_eq!(checklist_items(&added).len(), 2);
        assert_eq!(
            comments::find(&added, &comment.id).unwrap().body,
            "- [ ] discussion only"
        );

        assert!(set_section(&body, "Project", "replacement").is_err());
        assert!(remove_section(&body, "Project").is_err());
        assert!(set_section(&body, "Comments", "replacement").is_err());
        assert!(add_checklist_item(&body, "Comments", "unsafe").is_err());
    }

    #[test]
    fn adopted_comment_section_checkboxes_are_not_presented_as_card_work() {
        let base = "## Comments\n\n- [ ] legacy discussion checkbox\n";
        let (body, _) = comments::append(base, "Alice", "A normal comment").unwrap();

        assert!(checklist_items(&body).is_empty());
        assert!(toggle_checklist_global(&body, 1).is_err());
        assert!(toggle_checklist_item(&body, "Comments", 1).is_err());
    }

    #[test]
    fn unclosed_comment_markup_does_not_change_neighboring_markdown_structure() {
        let base = "# Project\n\n- [ ] project task\n\n## Comments\n\nLegacy.\n\n## Notes\n\n- [ ] notes task\n";
        for text in ["```md\nunclosed", "<script>\nunclosed", "<style>\nunclosed"] {
            let (body, _) = comments::append(base, "Alice", text).unwrap();
            assert_eq!(
                sections(&body)
                    .iter()
                    .map(|section| section.title.as_str())
                    .collect::<Vec<_>>(),
                vec!["Project", "Comments", "Notes"],
                "comment: {text}"
            );
            assert_eq!(
                checklist_items(&body)
                    .iter()
                    .map(|item| item.text.as_str())
                    .collect::<Vec<_>>(),
                vec!["project task", "notes task"],
                "comment: {text}"
            );
        }
    }
}
