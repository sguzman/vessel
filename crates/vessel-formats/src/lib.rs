use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum FormatSelector {
    Best,
    Worst,
    BestAudio,
    BestVideo,
    ExactFormatId(String),
    Merge(Box<FormatSelector>, Box<FormatSelector>),
    Fallback(Vec<FormatSelector>),
    Filtered {
        base: Box<FormatSelector>,
        predicate: String,
    },
}

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum ParseSelectorError {
    #[error("format selector cannot be empty")]
    Empty,
    #[error("invalid selector syntax: {0}")]
    InvalidSyntax(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputTemplate {
    pub raw: String,
}

impl Default for OutputTemplate {
    fn default() -> Self {
        Self {
            raw: "%(channel)s/%(upload_date)s - %(title)s [%(id)s].%(ext)s".to_owned(),
        }
    }
}

pub fn parse_selector(input: &str) -> Result<FormatSelector, ParseSelectorError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(ParseSelectorError::Empty);
    }

    let fallback_parts = split_top_level(trimmed, '/');
    if fallback_parts.len() > 1 {
        let selectors = fallback_parts
            .into_iter()
            .map(parse_merge_selector)
            .collect::<Result<Vec<_>, _>>()?;
        return Ok(FormatSelector::Fallback(selectors));
    }

    parse_merge_selector(trimmed)
}

fn parse_merge_selector(input: &str) -> Result<FormatSelector, ParseSelectorError> {
    let parts = split_top_level(input, '+');
    let mut selectors = parts
        .into_iter()
        .map(parse_filtered_selector)
        .collect::<Result<Vec<_>, _>>()?
        .into_iter();

    let mut selector = selectors
        .next()
        .ok_or_else(|| ParseSelectorError::InvalidSyntax(input.to_owned()))?;
    for part in selectors {
        selector = FormatSelector::Merge(Box::new(selector), Box::new(part));
    }
    Ok(selector)
}

fn parse_filtered_selector(input: &str) -> Result<FormatSelector, ParseSelectorError> {
    let mut rest = input.trim();
    if rest.is_empty() {
        return Err(ParseSelectorError::Empty);
    }

    let base_end = rest.find('[').unwrap_or(rest.len());
    let base = parse_atom(&rest[..base_end])?;
    rest = &rest[base_end..];

    let mut selector = base;
    while !rest.is_empty() {
        if !rest.starts_with('[') {
            return Err(ParseSelectorError::InvalidSyntax(input.to_owned()));
        }
        let end = rest
            .find(']')
            .ok_or_else(|| ParseSelectorError::InvalidSyntax(input.to_owned()))?;
        let predicate = rest[1..end].trim();
        if predicate.is_empty() {
            return Err(ParseSelectorError::InvalidSyntax(input.to_owned()));
        }
        selector = FormatSelector::Filtered {
            base: Box::new(selector),
            predicate: predicate.to_owned(),
        };
        rest = rest[end + 1..].trim_start();
    }

    Ok(selector)
}

fn parse_atom(input: &str) -> Result<FormatSelector, ParseSelectorError> {
    let atom = input.trim();
    if atom.is_empty() {
        return Err(ParseSelectorError::Empty);
    }

    let selector = match atom {
        "best" => FormatSelector::Best,
        "worst" => FormatSelector::Worst,
        "bestaudio" | "ba" => FormatSelector::BestAudio,
        "bestvideo" | "bv" => FormatSelector::BestVideo,
        _ => FormatSelector::ExactFormatId(atom.to_owned()),
    };
    Ok(selector)
}

fn split_top_level(input: &str, separator: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;

    for (index, ch) in input.char_indices() {
        match ch {
            '[' => depth += 1,
            ']' => depth = depth.saturating_sub(1),
            _ if ch == separator && depth == 0 => {
                parts.push(input[start..index].trim());
                start = index + ch.len_utf8();
            }
            _ => {}
        }
    }

    parts.push(input[start..].trim());
    parts
}

#[cfg(test)]
mod tests {
    use super::{FormatSelector, ParseSelectorError, parse_selector};

    #[test]
    fn parses_best() {
        assert_eq!(parse_selector("best").unwrap(), FormatSelector::Best);
    }

    #[test]
    fn parses_merge_with_fallback() {
        assert_eq!(
            parse_selector("bestvideo+bestaudio/best").unwrap(),
            FormatSelector::Fallback(vec![
                FormatSelector::Merge(
                    Box::new(FormatSelector::BestVideo),
                    Box::new(FormatSelector::BestAudio),
                ),
                FormatSelector::Best,
            ])
        );
    }

    #[test]
    fn parses_filtered_selector() {
        assert_eq!(
            parse_selector("best[ext=mp4]").unwrap(),
            FormatSelector::Filtered {
                base: Box::new(FormatSelector::Best),
                predicate: "ext=mp4".to_owned(),
            }
        );
    }

    #[test]
    fn parses_nested_filters() {
        assert_eq!(
            parse_selector("bestvideo[height<=720][ext=mp4]").unwrap(),
            FormatSelector::Filtered {
                base: Box::new(FormatSelector::Filtered {
                    base: Box::new(FormatSelector::BestVideo),
                    predicate: "height<=720".to_owned(),
                }),
                predicate: "ext=mp4".to_owned(),
            }
        );
    }

    #[test]
    fn rejects_empty_selector() {
        assert_eq!(parse_selector(" ").unwrap_err(), ParseSelectorError::Empty);
    }
}
