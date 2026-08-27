//! Parsing and deterministic serialization for YAML-frontmatter Markdown files.

use anyhow::{Context, Result, bail};
use serde::Serialize;
use serde::de::DeserializeOwned;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrontmatterDocument<T> {
    pub metadata: T,
    pub body: String,
}

pub fn parse<T: DeserializeOwned>(input: &str) -> Result<FrontmatterDocument<T>> {
    let input = input.strip_prefix('\u{feff}').unwrap_or(input);
    let mut lines = input.split_inclusive('\n');
    let Some(first) = lines.next() else {
        bail!("document is empty; expected YAML frontmatter");
    };
    if trim_line_ending(first) != "---" {
        bail!("document must start with a YAML frontmatter delimiter (`---`)");
    }

    let yaml_start = first.len();
    let mut offset = yaml_start;
    let mut closing = None;
    for line in lines {
        let line_start = offset;
        offset += line.len();
        let candidate = trim_line_ending(line);
        if candidate == "---" || candidate == "..." {
            closing = Some((line_start, offset));
            break;
        }
    }

    let Some((yaml_end, body_start)) = closing else {
        bail!("YAML frontmatter has no closing delimiter");
    };
    let yaml = input[yaml_start..yaml_end].trim_end_matches(['\r', '\n']);
    if yaml.trim().is_empty() {
        bail!("YAML frontmatter cannot be empty");
    }
    let metadata = serde_saphyr::from_str(yaml).context("invalid YAML frontmatter")?;
    // One blank line conventionally separates frontmatter from Markdown. Remove exactly that
    // separator, preserving every additional byte in the user-owned body.
    let body_source = &input[body_start..];
    let body = body_source
        .strip_prefix("\r\n")
        .or_else(|| body_source.strip_prefix('\n'))
        .unwrap_or(body_source)
        .to_owned();

    Ok(FrontmatterDocument { metadata, body })
}

pub fn serialize<T: Serialize>(metadata: &T, body: &str) -> Result<String> {
    let yaml = serde_saphyr::to_string(metadata)
        .context("could not serialize YAML frontmatter")?
        .trim_end()
        .to_owned();
    if body.is_empty() {
        Ok(format!("---\n{yaml}\n---\n"))
    } else {
        Ok(format!("---\n{yaml}\n---\n\n{body}"))
    }
}

fn trim_line_ending(line: &str) -> &str {
    line.strip_suffix("\r\n")
        .or_else(|| line.strip_suffix('\n'))
        .unwrap_or(line)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use pretty_assertions::assert_eq;
    use serde::{Deserialize, Serialize};
    use serde_json::{Value, json};

    use super::*;

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct TestMetadata {
        title: String,
        #[serde(flatten)]
        extra: BTreeMap<String, Value>,
    }

    #[test]
    fn parses_crlf_and_preserves_unknown_fields() {
        let input = "---\r\ntitle: Hello\r\nestimate: 3\r\n---\r\n\r\n## Notes\r\n\r\nKeep me\r\n";
        let document = parse::<TestMetadata>(input).unwrap();

        assert_eq!(document.metadata.title, "Hello");
        assert_eq!(document.metadata.extra["estimate"], json!(3));
        assert_eq!(document.body, "## Notes\r\n\r\nKeep me\r\n");
    }

    #[test]
    fn round_trip_keeps_arbitrary_nested_values_and_body() {
        let input = "---\ntitle: Demo\ncustom:\n  points: 5\n  flags: [one, two]\n---\n\n## Bespoke\n\n- [ ] flexible\n";
        let first = parse::<TestMetadata>(input).unwrap();
        let rendered = serialize(&first.metadata, &first.body).unwrap();
        let second = parse::<TestMetadata>(&rendered).unwrap();

        assert_eq!(second, first);
    }

    #[test]
    fn round_trip_preserves_body_boundaries_exactly() {
        let input = "---\ntitle: Demo\n---\n\n\nIntentional leading blank\n\nNo final newline";
        let first = parse::<TestMetadata>(input).unwrap();
        assert_eq!(
            first.body,
            "\nIntentional leading blank\n\nNo final newline"
        );

        let rendered = serialize(&first.metadata, &first.body).unwrap();
        let second = parse::<TestMetadata>(&rendered).unwrap();
        assert_eq!(second.body, first.body);
        assert!(!rendered.ends_with('\n'));
    }

    #[test]
    fn rejects_missing_or_unterminated_frontmatter() {
        assert!(parse::<TestMetadata>("# title").is_err());
        assert!(parse::<TestMetadata>("---\ntitle: nope").is_err());
    }
}
