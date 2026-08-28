//! Flat, append-only comments embedded in a card's Markdown body.
//!
//! Control records are standalone CommonMark HTML comments. Human-readable attribution and
//! comment text remain ordinary Markdown, while stable IDs and authorship metadata stay machine
//! readable. Comments intentionally have no parent/reply relationship.

use std::collections::HashSet;
use std::ops::Range;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, SecondsFormat, Utc};
use pulldown_cmark::{Event, Options, Parser, Tag};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const COMMENTS_MARKER: &str = "<!-- kbmd:comments:v1 -->";
const COMMENT_START: &str = "<!-- kbmd:comment:v1";
const COMMENT_END_PREFIX: &str = "<!-- kbmd:comment:end ";
const COMMENT_ID_PREFIX: &str = "CMT-";
const MAX_AUTHOR_CHARS: usize = 128;

/// A comment in document order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Comment {
    pub version: u8,
    pub id: String,
    pub author: String,
    pub created_at: String,
    pub body: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CommentMetadata {
    version: u8,
    id: String,
    author: String,
    created_at: String,
}

#[derive(Clone, Debug)]
struct Document {
    comments: Vec<Comment>,
    registry: Option<Registry>,
    comment_ranges: Vec<Range<usize>>,
    hidden_ranges: Vec<Range<usize>>,
}

#[derive(Clone, Debug)]
struct Registry {
    section: Section,
    direct_end: usize,
}

#[derive(Clone, Debug)]
struct Section {
    title: String,
    level: u8,
    start: usize,
    content_start: usize,
    end: usize,
}

#[derive(Debug)]
enum Token {
    Registry(Range<usize>),
    Start(CommentMetadata, Range<usize>),
    End(String, Range<usize>),
}

/// Parse and validate all structured comments, retaining their physical document order.
pub fn parse(body: &str) -> Result<Vec<Comment>> {
    Ok(parse_document(body)?.comments)
}

/// Find a comment by stable ID.
pub fn find(body: &str, id: &str) -> Result<Comment> {
    parse(body)?
        .into_iter()
        .find(|comment| comment.id.eq_ignore_ascii_case(id.trim()))
        .ok_or_else(|| anyhow::anyhow!("comment {:?} was not found", id.trim()))
}

/// Validate comments without allocating a public result.
pub fn validate(body: &str) -> Result<()> {
    parse_document(body).map(|_| ())
}

/// Byte ranges occupied by complete standalone control lines.
///
/// Ranges include terminating line endings when present. They cover only registry, start-metadata,
/// and end-marker records, never the visible attribution or comment body.
pub fn hidden_ranges(body: &str) -> Result<Vec<Range<usize>>> {
    Ok(parse_document(body)?.hidden_ranges)
}

/// Byte ranges covering whole structured comment blocks, used to hide their internal Markdown
/// headings and task-list examples from generic card discovery.
pub fn content_ranges(body: &str) -> Result<Vec<Range<usize>>> {
    Ok(parse_document(body)?.comment_ranges)
}

/// Replace structured comment blocks with offset-preserving whitespace for CommonMark parsing.
///
/// Parsing the original document and discarding events afterward is insufficient: an unclosed
/// fence or raw HTML block in a comment can otherwise change how neighboring card Markdown parses.
pub(crate) fn masked_content(body: &str) -> Result<String> {
    let ranges = content_ranges(body)?;
    Ok(mask_ranges(body, &ranges))
}

/// The complete Markdown section protected by the semantic comments registry.
pub fn registry_section_range(body: &str) -> Result<Option<Range<usize>>> {
    Ok(parse_document(body)?
        .registry
        .map(|registry| registry.section.start..registry.section.end))
}

/// Append one flat comment and return the narrowly updated body plus parsed comment.
///
/// ID and timestamp creation happens inside this call. Callers should invoke it from inside their
/// project update/lock closure.
pub fn append(body: &str, author: &str, text: &str) -> Result<(String, Comment)> {
    let author = normalized_author(author)?;
    let text = text.trim_matches(['\r', '\n']);
    if text.trim().is_empty() {
        bail!("comment text cannot be empty");
    }

    let id = format!("{COMMENT_ID_PREFIX}{}", Uuid::now_v7());
    let created_at = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
    append_with(body, &author, text, &id, &created_at)
}

/// Resolve an author without ever invoking a shell.
///
/// Resolution order is explicit value, `KBMD_AUTHOR`, then Git's effective `user.name` (including
/// worktree, local, conditional-include, global, and system configuration). An explicitly supplied
/// invalid value is an error and never falls through.
pub fn resolve_author(explicit: Option<&str>, project_root: &Path) -> Result<String> {
    if let Some(author) = explicit {
        return normalized_author(author).context("invalid --author value");
    }

    match std::env::var("KBMD_AUTHOR") {
        Ok(author) => return normalized_author(&author).context("invalid KBMD_AUTHOR value"),
        Err(std::env::VarError::NotUnicode(_)) => bail!("KBMD_AUTHOR is not valid Unicode"),
        Err(std::env::VarError::NotPresent) => {}
    }

    if let Some(author) = git_author(project_root)? {
        return normalized_author(&author).context("invalid Git user.name");
    }
    bail!(
        "no comment author is configured; use --author, set KBMD_AUTHOR, or configure Git user.name"
    )
}

fn git_author(project_root: &Path) -> Result<Option<String>> {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(project_root)
        .args(["config", "--get", "user.name"]);
    let output = match command.output() {
        Ok(output) => output,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("could not run git to resolve comment author"),
    };
    if !output.status.success() {
        return Ok(None);
    }
    let value = String::from_utf8(output.stdout).context("Git user.name is not valid UTF-8")?;
    Ok(Some(value.trim_end_matches(['\r', '\n']).to_owned()))
}

fn normalized_author(author: &str) -> Result<String> {
    let author = author.trim();
    validate_author(author)?;
    Ok(author.to_owned())
}

fn validate_author(author: &str) -> Result<()> {
    if author.is_empty() {
        bail!("comment author cannot be empty");
    }
    if author.chars().count() > MAX_AUTHOR_CHARS {
        bail!("comment author cannot exceed {MAX_AUTHOR_CHARS} characters");
    }
    if author.chars().any(char::is_control) || author.contains(['\r', '\n']) {
        bail!("comment author cannot contain control characters");
    }
    if author.contains("-->") {
        bail!("comment author cannot contain an HTML comment terminator");
    }
    Ok(())
}

fn append_with(
    body: &str,
    author: &str,
    text: &str,
    id: &str,
    created_at: &str,
) -> Result<(String, Comment)> {
    validate_author(author)?;
    validate_id(id)?;
    validate_timestamp(created_at)?;
    if text.trim().is_empty() {
        bail!("comment text cannot be empty");
    }

    let document = parse_document(body)?;
    let comment = Comment {
        version: 1,
        id: id.to_owned(),
        author: author.to_owned(),
        created_at: created_at.to_owned(),
        body: text.to_owned(),
    };
    let block = render_comment(&comment)?;

    let updated = if let Some(registry) = document.registry {
        insert_block(body, registry.direct_end, &block)
    } else if let Some(section) = unique_named_section(body, "Comments", &[])? {
        let direct_end = direct_content_end(body, &section, &[]);
        insert_block(body, direct_end, &format!("{COMMENTS_MARKER}\n\n{block}"))
    } else {
        let section = format!("## Comments\n\n{COMMENTS_MARKER}\n\n{block}");
        insert_block(body, body.len(), &section)
    };

    // Treat rendering bugs as write-time errors rather than persisting an invalid card.
    parse_document(&updated)?;
    Ok((updated, comment))
}

fn render_comment(comment: &Comment) -> Result<String> {
    let metadata = CommentMetadata {
        version: comment.version,
        id: comment.id.clone(),
        author: comment.author.clone(),
        created_at: comment.created_at.clone(),
    };
    let yaml = serde_saphyr::to_string(&metadata)
        .context("could not serialize comment metadata")?
        .trim_end_matches(['\r', '\n'])
        .to_owned();
    Ok(format!(
        "{COMMENT_START}\n{yaml}\n-->\nComment by **{}** · {}\n\n{}\n\n{COMMENT_END_PREFIX}{} -->",
        escape_markdown_inline(&comment.author),
        comment.created_at,
        comment.body,
        comment.id
    ))
}

fn insert_block(body: &str, offset: usize, block: &str) -> String {
    let before = &body[..offset];
    let after = &body[offset..];
    let mut result = String::with_capacity(body.len() + block.len() + 4);
    result.push_str(before);
    if !before.is_empty() {
        if before.ends_with("\n\n") || before.ends_with("\r\n\r\n") {
            // A blank line is already present.
        } else if before.ends_with('\n') {
            result.push('\n');
        } else {
            result.push_str("\n\n");
        }
    }
    result.push_str(block);
    if after.is_empty() {
        result.push('\n');
    } else {
        if after.starts_with("\n\n") || after.starts_with("\r\n\r\n") {
            // A blank line is already present.
        } else if after.starts_with('\n') {
            result.push('\n');
        } else {
            result.push_str("\n\n");
        }
        result.push_str(after);
    }
    result
}

fn escape_markdown_inline(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        if matches!(
            character,
            '\\' | '`'
                | '*'
                | '_'
                | '{'
                | '}'
                | '['
                | ']'
                | '<'
                | '>'
                | '('
                | ')'
                | '#'
                | '+'
                | '-'
                | '.'
                | '!'
                | '|'
        ) {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

fn parse_document(body: &str) -> Result<Document> {
    let tokens = marker_tokens(body)?;
    let mut registry_range = None;
    let mut open: Option<(CommentMetadata, Range<usize>)> = None;
    let mut comments = Vec::new();
    let mut comment_ranges = Vec::new();
    let mut hidden_ranges = Vec::new();
    let mut ids = HashSet::new();

    for token in tokens {
        match token {
            Token::Registry(range) => {
                if open.is_some() {
                    bail!("comments registry marker cannot be nested inside a comment block");
                }
                if registry_range.replace(range.clone()).is_some() {
                    bail!("a card can contain only one kbmd comments registry");
                }
                hidden_ranges.push(complete_line_range(body, &range));
            }
            Token::Start(metadata, range) => {
                if open.is_some() {
                    bail!("nested comment start marker is not allowed");
                }
                validate_metadata(&metadata)?;
                if !ids.insert(metadata.id.to_ascii_lowercase()) {
                    bail!("duplicate comment id {:?}", metadata.id);
                }
                hidden_ranges.push(complete_line_range(body, &range));
                open = Some((metadata, range));
            }
            Token::End(id, range) => {
                let Some((metadata, start)) = open.take() else {
                    bail!("orphan comment end marker for {id:?}");
                };
                if id != metadata.id {
                    bail!(
                        "comment end marker id {id:?} does not match start id {:?}",
                        metadata.id
                    );
                }
                validate_id(&id)?;
                let comment = parse_comment_body(body, &metadata, start.end..range.start)?;
                comment_ranges.push(start.start..range.end);
                hidden_ranges.push(complete_line_range(body, &range));
                comments.push(comment);
            }
        }
    }
    if let Some((metadata, _)) = open {
        bail!("comment {:?} has no matching end marker", metadata.id);
    }
    if !comments.is_empty() && registry_range.is_none() {
        bail!("structured comments require a {COMMENTS_MARKER} registry marker");
    }

    let registry = registry_range
        .map(|range| {
            let sections = structural_sections(body, &comment_ranges);
            let section = sections
                .iter()
                .filter(|section| {
                    range.start >= section.content_start && range.start < section.end
                })
                .max_by_key(|section| section.level)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("comments registry must be inside a Markdown section"))?;
            let direct_end = direct_content_end_from_sections(&section, &sections);
            for comment_range in &comment_ranges {
                if comment_range.start < range.end || comment_range.end > direct_end {
                    bail!(
                        "every structured comment must follow the registry in its section's direct content"
                    );
                }
            }
            Ok(Registry {
                section,
                direct_end,
            })
        })
        .transpose()?;

    hidden_ranges.sort_by_key(|range| range.start);
    Ok(Document {
        comments,
        registry,
        comment_ranges,
        hidden_ranges,
    })
}

fn marker_tokens(body: &str) -> Result<Vec<Token>> {
    if body.bytes().any(|byte| matches!(byte, b'\0' | 0x0b)) {
        bail!("card body contains a control byte unsupported by structured comments");
    }
    let mut tokens = Vec::new();
    let mut cursor = 0;
    while cursor < body.len() {
        let mut next_cursor = None;
        for (event, local_range) in
            Parser::new_ext(&body[cursor..], Options::ENABLE_TASKLISTS).into_offset_iter()
        {
            let Event::Html(_) = event else {
                continue;
            };
            let range = cursor + local_range.start..cursor + local_range.end;
            let source = body[range.clone()].trim_end_matches(['\r', '\n']);
            // Markers are control records only when the actual CommonMark HTML block begins at
            // column zero. Examples inside block quotes, lists, fenced code, and indented code
            // remain inert. CommonMark permits one to three leading spaces for an HTML block, but
            // accepting those would make the record invisible to our exact-line boundaries.
            let line_start = source_line_start(body, range.start);
            if line_start != range.start {
                let prefix = &body[line_start..range.start];
                if prefix
                    .chars()
                    .all(|character| matches!(character, ' ' | '\t'))
                    && source.contains("kbmd:comment")
                {
                    bail!("kbmd comment control markers must start at column zero");
                }
                continue;
            }
            if source == COMMENTS_MARKER {
                next_cursor = Some(range.end);
                tokens.push(Token::Registry(range));
                break;
            } else if source == COMMENT_START {
                let full_range = comment_start_record_range(body, range.start)?;
                let full_source = body[full_range.clone()].trim_end_matches(['\r', '\n']);
                let metadata = parse_start_metadata(full_source)?;
                let end_range = matching_comment_end(body, full_range.end, &metadata.id)?;
                next_cursor = Some(end_range.end);
                tokens.push(Token::Start(metadata.clone(), full_range));
                tokens.push(Token::End(metadata.id, end_range));
                break;
            } else if source.starts_with(COMMENT_START) {
                bail!("malformed comment start marker");
            } else if source.starts_with(COMMENT_END_PREFIX) {
                bail!("orphan or malformed comment end marker");
            } else if source.contains("kbmd:comment") {
                bail!("malformed or unsupported kbmd comment marker");
            }
        }
        let Some(next) = next_cursor else {
            break;
        };
        cursor = next;
    }
    Ok(tokens)
}

fn matching_comment_end(body: &str, start: usize, expected_id: &str) -> Result<Range<usize>> {
    let expected = format!("{COMMENT_END_PREFIX}{expected_id} -->");
    let mut offset = start;
    for raw_line in body[start..].split_inclusive('\n') {
        let line = raw_line
            .strip_suffix("\r\n")
            .or_else(|| raw_line.strip_suffix('\n'))
            .unwrap_or(raw_line);
        if line == expected {
            return Ok(offset..offset + raw_line.len());
        }
        offset += raw_line.len();
    }
    bail!("comment {expected_id:?} has no matching end marker")
}

fn comment_start_record_range(body: &str, start: usize) -> Result<Range<usize>> {
    let mut offset = start;
    for (index, raw_line) in body[start..].split_inclusive('\n').enumerate() {
        let line = raw_line
            .strip_suffix("\r\n")
            .or_else(|| raw_line.strip_suffix('\n'))
            .unwrap_or(raw_line);
        if index == 0 {
            if line != COMMENT_START {
                bail!("malformed comment start marker");
            }
        } else if line == "-->" {
            return Ok(start..offset + raw_line.len());
        } else if line.contains("-->") {
            bail!("comment metadata cannot contain an inline HTML comment terminator");
        } else if line.starts_with("<!-- kbmd:comment") {
            bail!("nested comment control marker in comment metadata");
        }
        offset += raw_line.len();
    }
    bail!("comment start marker is missing a standalone --> terminator")
}

fn parse_start_metadata(source: &str) -> Result<CommentMetadata> {
    let lines = source.lines().collect::<Vec<_>>();
    if lines.len() < 3
        || lines.first().copied() != Some(COMMENT_START)
        || lines.last().copied() != Some("-->")
    {
        bail!("malformed comment start marker: {source:?}");
    }
    let yaml = lines[1..lines.len() - 1].join("\n");
    validate_metadata_keys(&yaml)?;
    serde_saphyr::from_str(&yaml).context("invalid comment metadata YAML")
}

fn validate_metadata_keys(yaml: &str) -> Result<()> {
    const KEYS: [&str; 4] = ["version", "id", "author", "created_at"];
    let mut found = HashSet::new();
    for line in yaml.lines() {
        if line.trim().is_empty() {
            continue;
        }
        if line.starts_with([' ', '\t']) {
            bail!("comment metadata values must be single-line YAML scalars");
        }
        let Some((key, _)) = line.split_once(':') else {
            bail!("malformed comment metadata entry");
        };
        if !KEYS.contains(&key) {
            bail!("unknown comment metadata key {key:?}");
        }
        if !found.insert(key) {
            bail!("duplicate comment metadata key {key:?}");
        }
    }
    for key in KEYS {
        if !found.contains(key) {
            bail!("comment metadata is missing {key:?}");
        }
    }
    Ok(())
}

fn validate_metadata(metadata: &CommentMetadata) -> Result<()> {
    if metadata.version != 1 {
        bail!("unsupported comment version {}", metadata.version);
    }
    validate_id(&metadata.id)?;
    if metadata.author.trim() != metadata.author {
        bail!("comment author cannot have leading or trailing whitespace");
    }
    validate_author(&metadata.author)?;
    validate_timestamp(&metadata.created_at)
}

fn validate_id(id: &str) -> Result<()> {
    let Some(raw) = id.strip_prefix(COMMENT_ID_PREFIX) else {
        bail!("comment id must start with {COMMENT_ID_PREFIX}");
    };
    let uuid = Uuid::parse_str(raw).context("comment id does not contain a valid UUID")?;
    if uuid.get_version_num() != 7 {
        bail!("comment id must contain a UUIDv7");
    }
    if format!("{COMMENT_ID_PREFIX}{uuid}") != id {
        bail!("comment id must use canonical CMT-<UUIDv7> form");
    }
    Ok(())
}

fn validate_timestamp(timestamp: &str) -> Result<()> {
    let parsed = DateTime::parse_from_rfc3339(timestamp).context("invalid comment created_at")?;
    if parsed.offset().local_minus_utc() != 0 {
        bail!("comment created_at must be UTC");
    }
    Ok(())
}

fn parse_comment_body(
    body: &str,
    metadata: &CommentMetadata,
    content_range: Range<usize>,
) -> Result<Comment> {
    let content = body[content_range].trim_matches(['\r', '\n']);
    let Some(newline) = content.find('\n') else {
        bail!(
            "comment {:?} is missing its visible attribution",
            metadata.id
        );
    };
    let attribution = content[..newline].trim_end_matches('\r');
    let expected = format!(
        "Comment by **{}** · {}",
        escape_markdown_inline(&metadata.author),
        metadata.created_at
    );
    if attribution != expected {
        bail!("comment {:?} has invalid visible attribution", metadata.id);
    }
    let remainder = &content[newline + 1..];
    let text = remainder
        .strip_prefix("\r\n")
        .or_else(|| remainder.strip_prefix('\n'))
        .ok_or_else(|| {
            anyhow::anyhow!(
                "comment {:?} must separate attribution and text with a blank line",
                metadata.id
            )
        })?
        .trim_end_matches(['\r', '\n']);
    if text.trim().is_empty() {
        bail!("comment {:?} text cannot be empty", metadata.id);
    }
    reject_active_comment_controls(text, &metadata.id)?;
    Ok(Comment {
        version: metadata.version,
        id: metadata.id.clone(),
        author: metadata.author.clone(),
        created_at: metadata.created_at.clone(),
        body: text.to_owned(),
    })
}

fn reject_active_comment_controls(text: &str, outer_id: &str) -> Result<()> {
    for (event, range) in Parser::new_ext(text, Options::ENABLE_TASKLISTS).into_offset_iter() {
        let Event::Html(_) = event else {
            continue;
        };
        let source = text[range.clone()].trim_end_matches(['\r', '\n']);
        if !source.contains("kbmd:comment") {
            continue;
        }
        let line_start = source_line_start(text, range.start);
        if line_start != range.start
            && !text[line_start..range.start]
                .chars()
                .all(|character| matches!(character, ' ' | '\t'))
        {
            continue;
        }
        bail!("comment {outer_id:?} contains a nested kbmd comment control marker");
    }
    Ok(())
}

fn structural_sections(body: &str, excluded: &[Range<usize>]) -> Vec<Section> {
    if body.bytes().any(|byte| matches!(byte, b'\0' | 0x0b)) {
        return Vec::new();
    }
    let masked = mask_ranges(body, excluded);
    let mut headings = Vec::<(String, u8, usize, usize)>::new();
    for (event, range) in Parser::new_ext(&masked, Options::ENABLE_TASKLISTS).into_offset_iter() {
        let Event::Start(Tag::Heading { .. }) = event else {
            continue;
        };
        if excluded
            .iter()
            .any(|excluded| range.start >= excluded.start && range.start < excluded.end)
        {
            continue;
        }
        let start = source_line_start(body, range.start);
        let end = source_line_end(body, start);
        let line = body[start..end].trim_end_matches(['\r', '\n']).trim_start();
        if let Some((level, title)) = atx_heading(line) {
            headings.push((title.to_owned(), level, start, end));
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

fn mask_ranges(body: &str, ranges: &[Range<usize>]) -> String {
    let mut masked = body.as_bytes().to_vec();
    for range in ranges {
        for byte in &mut masked[range.clone()] {
            if *byte != b'\n' {
                *byte = b' ';
            }
        }
    }
    String::from_utf8(masked).expect("masking UTF-8 with ASCII spaces preserves valid UTF-8")
}

fn unique_named_section(
    body: &str,
    title: &str,
    excluded: &[Range<usize>],
) -> Result<Option<Section>> {
    let matches = structural_sections(body, excluded)
        .into_iter()
        .filter(|section| section.title.eq_ignore_ascii_case(title))
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

fn direct_content_end(body: &str, section: &Section, excluded: &[Range<usize>]) -> usize {
    direct_content_end_from_sections(section, &structural_sections(body, excluded))
}

fn direct_content_end_from_sections(section: &Section, sections: &[Section]) -> usize {
    sections
        .iter()
        .find(|candidate| {
            candidate.start >= section.content_start
                && candidate.start < section.end
                && candidate.level > section.level
        })
        .map_or(section.end, |child| child.start)
}

fn source_line_start(body: &str, offset: usize) -> usize {
    body[..offset.min(body.len())]
        .rfind('\n')
        .map_or(0, |newline| newline + 1)
}

fn source_line_end(body: &str, start: usize) -> usize {
    body[start..]
        .find('\n')
        .map_or(body.len(), |offset| start + offset + 1)
}

fn complete_line_range(body: &str, range: &Range<usize>) -> Range<usize> {
    let end = if body[..range.end].ends_with('\n') {
        range.end
    } else {
        source_line_end(body, range.end)
    };
    source_line_start(body, range.start)..end
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

#[cfg(test)]
mod tests {
    use std::fs;

    use pretty_assertions::assert_eq;
    use tempfile::tempdir;

    use super::*;

    const ID_1: &str = "CMT-018f3f53-4e30-7c76-bf58-0123456789ab";
    const ID_2: &str = "CMT-018f3f53-4e31-7c76-8f58-0123456789ab";
    const TIME_1: &str = "2026-08-28T02:00:00Z";
    const TIME_2: &str = "2025-01-01T00:00:00Z";

    fn appended(body: &str, author: &str, text: &str, id: &str, time: &str) -> String {
        append_with(body, author, text, id, time).unwrap().0
    }

    #[test]
    fn creates_a_flat_comments_section_and_parses_it() {
        let body = "## Plan\n\nKeep this byte-for-byte.\n";
        let updated = appended(body, "Simon", "Looks good.", ID_1, TIME_1);
        assert!(updated.starts_with(body));
        assert!(updated.contains("## Comments\n\n<!-- kbmd:comments:v1 -->"));
        let found = parse(&updated).unwrap();
        assert_eq!(
            found,
            vec![Comment {
                version: 1,
                id: ID_1.to_owned(),
                author: "Simon".to_owned(),
                created_at: TIME_1.to_owned(),
                body: "Looks good.".to_owned(),
            }]
        );
        assert!(!serde_json::to_string(&found[0]).unwrap().contains("reply"));
    }

    #[test]
    fn reuses_existing_content_and_survives_a_renamed_section() {
        let body = "## Comments\n\nLegacy note stays.\n\n### Policy\n\nChild stays.\n\n## Notes\n\nNeighbor stays.\n";
        let first = appended(body, "Alice", "First", ID_1, TIME_1);
        let marker = first.find(COMMENTS_MARKER).unwrap();
        let child = first.find("### Policy").unwrap();
        assert!(marker < child);
        assert!(first.contains("Legacy note stays."));
        assert!(first.contains("Child stays."));
        assert!(first.contains("Neighbor stays."));

        let renamed = first.replacen("## Comments", "## Design conversation", 1);
        let second = appended(&renamed, "Bob", "Second", ID_2, TIME_2);
        assert_eq!(second.matches(COMMENTS_MARKER).count(), 1);
        assert_eq!(
            parse(&second)
                .unwrap()
                .iter()
                .map(|comment| comment.author.as_str())
                .collect::<Vec<_>>(),
            vec!["Alice", "Bob"]
        );
    }

    #[test]
    fn preserves_physical_order_instead_of_sorting_timestamps() {
        let first = appended("", "Alice", "newer", ID_1, TIME_1);
        let second = appended(&first, "Bob", "older timestamp", ID_2, TIME_2);
        let comments = parse(&second).unwrap();
        assert_eq!(comments[0].body, "newer");
        assert_eq!(comments[1].body, "older timestamp");
    }

    #[test]
    fn append_preserves_existing_crlf_and_no_final_newline_prefixes() {
        for body in ["## Notes\r\n\r\nKeep\r\n", "## Notes\n\nKeep"] {
            let updated = appended(body, "Alice", "one", ID_1, TIME_1);
            assert!(updated.starts_with(body));
            assert_eq!(parse(&updated).unwrap()[0].body, "one");
        }
    }

    #[test]
    fn duplicate_adoption_headings_and_registry_outside_a_section_fail_closed() {
        let duplicate = "## Comments\n\nOne.\n\n## comments\n\nTwo.\n";
        assert!(append_with(duplicate, "Alice", "one", ID_1, TIME_1).is_err());

        assert!(parse(&format!("{COMMENTS_MARKER}\n")).is_err());
    }

    #[test]
    fn arbitrary_markdown_is_inside_the_comment_content_range() {
        let text = "### Not a card section\n\n- [ ] not card progress\n\n```md\n<!-- kbmd:comment:end fake -->\n```";
        let updated = appended("## Work\n\n- [ ] real\n", "Agent *A*", text, ID_1, TIME_1);
        let ranges = content_ranges(&updated).unwrap();
        assert_eq!(ranges.len(), 1);
        let block = &updated[ranges[0].clone()];
        assert!(block.contains("### Not a card section"));
        assert_eq!(parse(&updated).unwrap()[0].body, text);
    }

    #[test]
    fn unclosed_markdown_constructs_cannot_escape_the_comment_boundary() {
        for text in [
            "```md\nunfinished fence",
            "<script>\nunclosed script",
            "<style>\nunclosed style",
            "<pre>\nunclosed pre",
        ] {
            let updated = appended("", "Alice", text, ID_1, TIME_1);
            assert_eq!(parse(&updated).unwrap()[0].body, text);
        }
    }

    #[test]
    fn unclosed_comment_html_cannot_swallow_neighboring_sections_or_later_comments() {
        let base =
            "## Comments\n\nLegacy.\n\n### Policy\n\nChild stays.\n\n## Notes\n\nNeighbor stays.\n";
        for tag in ["script", "style", "pre"] {
            let first = appended(base, "Alice", &format!("<{tag}>\nunclosed"), ID_1, TIME_1);
            let second = appended(&first, "Bob", "later", ID_2, TIME_2);
            let policy = second.find("### Policy").unwrap();
            let notes = second.find("## Notes").unwrap();
            assert!(second.rfind(ID_2).unwrap() < policy, "tag: {tag}");
            assert!(policy < notes, "tag: {tag}");
            assert!(second.contains("Neighbor stays."));
            assert_eq!(parse(&second).unwrap().len(), 2);
        }
    }

    #[test]
    fn marker_examples_in_code_quotes_lists_or_inline_text_are_inert() {
        let body = "## Examples\n\n```md\n<!-- kbmd:comments:v1 -->\n<!-- kbmd:comment:end fake -->\n```\n\n    <!-- kbmd:comments:v1 -->\n\n> <!-- kbmd:comments:v1 -->\n\n- <!-- kbmd:comments:v1 -->\n\ntext <!-- kbmd:comments:v1 -->\n";
        assert!(parse(body).unwrap().is_empty());
        assert!(hidden_ranges(body).unwrap().is_empty());
    }

    #[test]
    fn rejects_whitespace_indented_control_records_instead_of_orphaning_them() {
        let valid = appended("", "Alice", "one", ID_1, TIME_1);
        for marker in [
            COMMENTS_MARKER,
            COMMENT_START,
            &format!("{COMMENT_END_PREFIX}{ID_1} -->"),
        ] {
            let indented = valid.replacen(marker, &format!(" {marker}"), 1);
            assert!(parse(&indented).is_err(), "indented marker: {marker}");
        }
    }

    #[test]
    fn adopts_a_commonmark_heading_with_leading_spaces() {
        let body = "  ## Comments\n\nLegacy context.\n\n## Neighbor\n\nKeep.\n";
        let updated = appended(body, "Alice", "one", ID_1, TIME_1);

        assert_eq!(updated.matches(COMMENTS_MARKER).count(), 1);
        assert!(!updated.contains("\n## Comments\n"));
        assert!(updated.contains("Legacy context."));
        assert_eq!(parse(&updated).unwrap().len(), 1);
    }

    #[test]
    fn rejects_malformed_nested_or_duplicate_control_records() {
        let valid = appended("", "Alice", "one", ID_1, TIME_1);
        let duplicate = valid.replace(
            &format!("{COMMENT_END_PREFIX}{ID_1} -->"),
            &format!(
                "{COMMENT_END_PREFIX}{ID_1} -->\n\n{}",
                render_comment(&Comment {
                    version: 1,
                    id: ID_1.to_owned(),
                    author: "Bob".to_owned(),
                    created_at: TIME_2.to_owned(),
                    body: "two".to_owned(),
                })
                .unwrap()
            ),
        );
        assert!(
            parse(&duplicate)
                .unwrap_err()
                .to_string()
                .contains("duplicate")
        );

        let orphan =
            format!("## Comments\n\n{COMMENTS_MARKER}\n\n{COMMENT_END_PREFIX}{ID_1} -->\n");
        assert!(parse(&orphan).unwrap_err().to_string().contains("orphan"));

        let nested = valid.replace(
            "Comment by **Alice**",
            &format!("{COMMENT_START}\nversion: 1\nid: {ID_2}\nauthor: Bob\ncreated_at: {TIME_2}\n-->\n\nComment by **Alice**"),
        );
        assert!(parse(&nested).is_err());

        let inner = render_comment(&Comment {
            version: 1,
            id: ID_2.to_owned(),
            author: "Mallory".to_owned(),
            created_at: TIME_2.to_owned(),
            body: "masquerading as a reply".to_owned(),
        })
        .unwrap();
        let nested_block = format!(
            "## Comments\n\n{COMMENTS_MARKER}\n\n{}\n",
            render_comment(&Comment {
                version: 1,
                id: ID_1.to_owned(),
                author: "Alice".to_owned(),
                created_at: TIME_1.to_owned(),
                body: format!("outer body\n\n{inner}"),
            })
            .unwrap()
        );
        assert!(
            parse(&nested_block)
                .unwrap_err()
                .to_string()
                .contains("nested")
        );

        let fenced_example = appended("", "Alice", &format!("```md\n{inner}\n```"), ID_1, TIME_1);
        assert_eq!(parse(&fenced_example).unwrap().len(), 1);

        let unsupported = valid.replace("<!-- kbmd:comment:v1", "<!-- kbmd:comment:v2");
        assert!(parse(&unsupported).is_err());
    }

    #[test]
    fn rejects_bad_metadata_ids_authors_timestamps_and_empty_text() {
        let valid = appended("", "Alice", "one", ID_1, TIME_1);
        assert!(parse(&valid.replace("version: 1", "version: 2")).is_err());
        assert!(parse(&valid.replace("version: 1", "version: 1\nversion: 1")).is_err());
        assert!(parse(&valid.replace("version: 1", "unknown: 1\nversion: 1")).is_err());
        assert!(parse(&valid.replace("author: Alice", "author: Alice # -->")).is_err());
        assert!(parse(&valid.replace(ID_1, "CMT-550e8400-e29b-41d4-a716-446655440000")).is_err());
        assert!(parse(&valid.replace(TIME_1, "2026-08-28T10:00:00+08:00")).is_err());
        assert!(append("", "bad --> author", "text").is_err());
        assert!(append("", "Alice", " \n ").is_err());
    }

    #[test]
    fn rejects_parser_hazard_control_bytes_instead_of_hiding_comments() {
        for body in ["before\0after", "before\u{000b}after"] {
            assert!(parse(body).is_err());
            assert!(append(body, "Alice", "visible comment").is_err());
        }
    }

    #[test]
    fn hidden_ranges_cover_only_complete_control_lines() {
        let updated = appended("", "Alice", "Visible", ID_1, TIME_1);
        let hidden = hidden_ranges(&updated).unwrap();
        assert_eq!(hidden.len(), 3);
        for range in hidden {
            let source = &updated[range];
            assert!(source.starts_with("<!--"));
            assert!(source.ends_with('\n'));
            assert!(!source.contains("Visible"));
        }
    }

    #[test]
    fn explicit_and_effective_git_authors_resolve_without_a_shell() {
        let directory = tempdir().unwrap();
        let status = Command::new("git")
            .arg("init")
            .arg("--quiet")
            .arg(directory.path())
            .status()
            .unwrap();
        assert!(status.success());
        let status = Command::new("git")
            .arg("-C")
            .arg(directory.path())
            .args(["config", "user.name", "Local Author"])
            .status()
            .unwrap();
        assert!(status.success());

        assert_eq!(
            resolve_author(Some(" Explicit "), directory.path()).unwrap(),
            "Explicit"
        );
        assert_eq!(
            resolve_author(None, directory.path()).unwrap(),
            "Local Author"
        );

        let status = Command::new("git")
            .arg("-C")
            .arg(directory.path())
            .args(["config", "extensions.worktreeConfig", "true"])
            .status()
            .unwrap();
        assert!(status.success());
        let status = Command::new("git")
            .arg("-C")
            .arg(directory.path())
            .args(["config", "--worktree", "user.name", "Worktree Author"])
            .status()
            .unwrap();
        assert!(status.success());
        assert_eq!(
            resolve_author(None, directory.path()).unwrap(),
            "Worktree Author"
        );
        assert!(resolve_author(Some("-->"), directory.path()).is_err());
        assert!(fs::metadata(directory.path().join(".git/config")).is_ok());
    }
}
