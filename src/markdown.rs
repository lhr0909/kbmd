//! Conservative mutations for user-defined Markdown sections and checklists.
//!
//! The parser deliberately recognizes ATX headings and task-list markers only outside fenced
//! code blocks. Mutations touch the smallest possible body range and leave unrelated sections
//! byte-for-byte unchanged.

use anyhow::{Result, bail};

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
    let mut headings = Vec::<(String, u8, usize, usize)>::new();
    let mut fence: Option<(char, usize)> = None;

    for line in &lines {
        let trimmed = line.text.trim_start();
        if let Some((character, length)) = fence {
            if fence_marker(trimmed)
                .is_some_and(|(next, count)| next == character && count >= length)
            {
                fence = None;
            }
            continue;
        }
        if let Some(marker) = fence_marker(trimmed) {
            fence = Some(marker);
            continue;
        }
        if let Some((level, title)) = atx_heading(trimmed) {
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
    let mut counts = vec![0_usize; body_sections.len() + 1];
    let mut items = Vec::new();
    let mut fence: Option<(char, usize)> = None;

    for line in lines(body) {
        let trimmed = line.text.trim_start();
        if let Some((character, length)) = fence {
            if fence_marker(trimmed)
                .is_some_and(|(next, count)| next == character && count >= length)
            {
                fence = None;
            }
            continue;
        }
        if let Some(marker) = fence_marker(trimmed) {
            fence = Some(marker);
            continue;
        }
        let Some((relative_checkbox_offset, checked, text)) = checkbox(trimmed) else {
            continue;
        };
        let indentation = line.text.len() - trimmed.len();
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
            checkbox_offset: line.start + indentation + relative_checkbox_offset,
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
    append_section(body, section, &format!("- [ ] {}", text.trim()))
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
    replace_checkbox(body, &item, !item.checked)
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
    checklist_items(body)
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
        })
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

fn fence_marker(line: &str) -> Option<(char, usize)> {
    let character = line.chars().next()?;
    if character != '`' && character != '~' {
        return None;
    }
    let count = line.chars().take_while(|next| *next == character).count();
    (count >= 3).then_some((character, count))
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
    if let Some(without_hashes) = title.strip_suffix('#') {
        title = without_hashes.trim_end_matches('#').trim_end();
    }
    (!title.is_empty()).then_some((count as u8, title))
}

/// Returns the offset of the checkbox state character relative to `line`.
fn checkbox(line: &str) -> Option<(usize, bool, &str)> {
    let bytes = line.as_bytes();
    if bytes.len() < 6
        || !matches!(bytes[0], b'-' | b'*' | b'+')
        || bytes[1] != b' '
        || bytes[2] != b'['
    {
        return None;
    }
    let checked = match bytes[3] {
        b' ' => false,
        b'x' | b'X' => true,
        _ => return None,
    };
    if bytes[4] != b']' || !matches!(bytes[5], b' ' | b'\t') {
        return None;
    }
    Some((3, checked, line[6..].trim_end()))
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
        let body = "## Real\n\n```md\n## Fake\n- [ ] fake\n```\n\n- [ ] real\n";
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
    }
}
