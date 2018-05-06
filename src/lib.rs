#[macro_use]
extern crate log;
#[macro_use]
extern crate failure;
#[cfg(test)]
#[macro_use]
extern crate proptest;
#[macro_use]
extern crate serde_derive;
extern crate serde_json;

use std::collections::HashSet;
use std::ops::Range;

use failure::Error;

pub mod diagnostics;
use diagnostics::{Diagnostic, DiagnosticSpan};
mod replace;

pub fn get_suggestions_from_json<S: ::std::hash::BuildHasher>(
    input: &str,
    only: &HashSet<String, S>,
) -> serde_json::error::Result<Vec<Suggestion>> {
    let mut result = Vec::new();
    for cargo_msg in serde_json::Deserializer::from_str(input).into_iter::<Diagnostic>() {
        // One diagnostic line might have multiple suggestions
        result.extend(collect_suggestions(&cargo_msg?, only));
    }
    Ok(result)
}

#[derive(Debug, Copy, Clone, Hash, PartialEq)]
pub struct LinePosition {
    pub line: usize,
    pub column: usize,
}

impl std::fmt::Display for LinePosition {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}:{}", self.line, self.column)
    }
}

#[derive(Debug, Copy, Clone, Hash, PartialEq)]
pub struct LineRange {
    pub start: LinePosition,
    pub end: LinePosition,
}

impl std::fmt::Display for LineRange {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}-{}", self.start, self.end)
    }
}

#[derive(Debug, Clone, Hash, PartialEq)]
/// An error/warning and possible solutions for fixing it
pub struct Suggestion {
    pub message: String,
    pub snippets: Vec<Snippet>,
    pub solutions: Vec<Solution>,
}

#[derive(Debug, Clone, Hash, PartialEq)]
pub struct Solution {
    pub message: String,
    pub replacements: Vec<Replacement>,
}

#[derive(Debug, Clone, Hash, PartialEq)]
pub struct Snippet {
    pub file_name: String,
    pub line_range: LineRange,
    pub range: Range<usize>,
    /// leading surrounding text, text to replace, trailing surrounding text
    ///
    /// This split is useful for higlighting the part that gets replaced
    pub text: (String, String, String),
}

#[derive(Debug, Clone, Hash, PartialEq)]
pub struct Replacement {
    pub snippet: Snippet,
    pub replacement: String,
}

fn parse_snippet(span: &DiagnosticSpan) -> Snippet {
    // unindent the snippet
    let indent = span.text
        .iter()
        .map(|line| {
            let indent = line.text
                .chars()
                .take_while(|&c| char::is_whitespace(c))
                .count();
            std::cmp::min(indent, line.highlight_start)
        })
        .min()
        .expect("text to replace is empty");
    let start = span.text[0].highlight_start - 1;
    let end = span.text[0].highlight_end - 1;
    let lead = span.text[0].text[indent..start].to_string();
    let mut body = span.text[0].text[start..end].to_string();
    for line in span.text.iter().take(span.text.len() - 1).skip(1) {
        body.push('\n');
        body.push_str(&line.text[indent..]);
    }
    let mut tail = String::new();
    let last = &span.text[span.text.len() - 1];
    if span.text.len() > 1 {
        body.push('\n');
        body.push_str(&last.text[indent..last.highlight_end - 1]);
    }
    tail.push_str(&last.text[last.highlight_end - 1..]);
    Snippet {
        file_name: span.file_name.clone(),
        line_range: LineRange {
            start: LinePosition {
                line: span.line_start,
                column: span.column_start,
            },
            end: LinePosition {
                line: span.line_end,
                column: span.column_end,
            },
        },
        range: (span.byte_start as usize)..(span.byte_end as usize),
        text: (lead, body, tail),
    }
}

fn collect_span(span: &DiagnosticSpan) -> Option<Replacement> {
    span.suggested_replacement
        .clone()
        .map(|replacement| Replacement {
            snippet: parse_snippet(span),
            replacement,
        })
}

pub fn collect_suggestions<S: ::std::hash::BuildHasher>(
    diagnostic: &Diagnostic,
    only: &HashSet<String, S>,
) -> Option<Suggestion> {
    if !only.is_empty() {
        if let Some(ref code) = diagnostic.code {
            if !only.contains(&code.code) {
                // This is not the code we are looking for
                return None;
            }
        } else {
            // No code, probably a weird builtin warning/error
            return None;
        }
    }

    let snippets = diagnostic
        .spans
        .iter()
        .map(|span| parse_snippet(span))
        .collect();

    let solutions: Vec<_> = diagnostic
        .children
        .iter()
        .filter_map(|child| {
            let replacements: Vec<_> = child.spans.iter().filter_map(collect_span).collect();
            if replacements.is_empty() {
                None
            } else {
                Some(Solution {
                    message: child.message.clone(),
                    replacements,
                })
            }
        })
        .collect();

    if solutions.is_empty() {
        None
    } else {
        Some(Suggestion {
            message: diagnostic.message.clone(),
            snippets,
            solutions,
        })
    }
}

pub fn apply_suggestions(code: &str, suggestions: &[Suggestion]) -> Result<String, Error> {
    use replace::Data;

    let mut fixed = Data::new(code.as_bytes());

    for sug in suggestions.iter().rev() {
        for sol in &sug.solutions {
            for r in &sol.replacements {
                fixed.replace_range(
                    r.snippet.range.start,
                    r.snippet.range.end.saturating_sub(1),
                    r.replacement.as_bytes(),
                )?;
            }
        }
    }

    Ok(String::from_utf8(fixed.to_vec())?)
}
