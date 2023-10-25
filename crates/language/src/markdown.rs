use std::sync::Arc;
use std::{ops::Range, path::PathBuf};

use crate::{HighlightId, Language, LanguageRegistry};
use gpui::fonts::{self, HighlightStyle, Weight};
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag};

#[derive(Debug, Clone)]
pub struct ParsedMarkdown {
    pub text: String,
    pub highlights: Vec<(Range<usize>, MarkdownHighlight)>,
    pub region_ranges: Vec<Range<usize>>,
    pub regions: Vec<ParsedRegion>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MarkdownHighlight {
    Style(MarkdownHighlightStyle),
    Code(HighlightId),
}

impl MarkdownHighlight {
    pub fn to_highlight_style(&self, theme: &theme::SyntaxTheme) -> Option<HighlightStyle> {
        match self {
            MarkdownHighlight::Style(style) => {
                let mut highlight = HighlightStyle::default();

                if style.italic {
                    highlight.italic = Some(true);
                }

                if style.underline {
                    highlight.underline = Some(fonts::Underline {
                        thickness: 1.0.into(),
                        ..Default::default()
                    });
                }

                if style.weight != fonts::Weight::default() {
                    highlight.weight = Some(style.weight);
                }

                Some(highlight)
            }

            MarkdownHighlight::Code(id) => id.style(theme),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MarkdownHighlightStyle {
    pub italic: bool,
    pub underline: bool,
    pub weight: Weight,
}

#[derive(Debug, Clone)]
pub struct ParsedRegion {
    pub code: bool,
    pub link: Option<Link>,
}

#[derive(Debug, Clone)]
pub enum Link {
    Web { url: String },
    Path { path: PathBuf },
}

impl Link {
    fn identify(text: String) -> Option<Link> {
        if text.starts_with("http") {
            return Some(Link::Web { url: text });
        }

        let path = PathBuf::from(text);
        if path.is_absolute() {
            return Some(Link::Path { path });
        }

        None
    }
}

pub async fn parse_markdown(
    markdown: &str,
    language_registry: &Arc<LanguageRegistry>,
    language: Option<Arc<Language>>,
) -> ParsedMarkdown {
    let mut text = String::new();
    let mut highlights = Vec::new();
    let mut region_ranges = Vec::new();
    let mut regions = Vec::new();

    parse_markdown_block(
        markdown,
        language_registry,
        language,
        &mut text,
        &mut highlights,
        &mut region_ranges,
        &mut regions,
    )
    .await;

    ParsedMarkdown {
        text,
        highlights,
        region_ranges,
        regions,
    }
}

pub async fn parse_markdown_block(
    markdown: &str,
    language_registry: &Arc<LanguageRegistry>,
    language: Option<Arc<Language>>,
    text: &mut String,
    highlights: &mut Vec<(Range<usize>, MarkdownHighlight)>,
    region_ranges: &mut Vec<Range<usize>>,
    regions: &mut Vec<ParsedRegion>,
) {
    let mut bold_depth = 0;
    let mut italic_depth = 0;
    let mut link_url = None;
    let mut current_language = None;
    let mut list_stack = Vec::new();

    for event in Parser::new_ext(&markdown, Options::all()) {
        let prev_len = text.len();
        match event {
            Event::Text(t) => {
                if let Some(language) = &current_language {
                    highlight_code(text, highlights, t.as_ref(), language);
                } else {
                    text.push_str(t.as_ref());

                    let mut style = MarkdownHighlightStyle::default();

                    if bold_depth > 0 {
                        style.weight = Weight::BOLD;
                    }

                    if italic_depth > 0 {
                        style.italic = true;
                    }

                    if let Some(link) = link_url.clone().and_then(|u| Link::identify(u)) {
                        region_ranges.push(prev_len..text.len());
                        regions.push(ParsedRegion {
                            code: false,
                            link: Some(link),
                        });
                        style.underline = true;
                    }

                    if style != MarkdownHighlightStyle::default() {
                        let mut new_highlight = true;
                        if let Some((last_range, MarkdownHighlight::Style(last_style))) =
                            highlights.last_mut()
                        {
                            if last_range.end == prev_len && last_style == &style {
                                last_range.end = text.len();
                                new_highlight = false;
                            }
                        }
                        if new_highlight {
                            let range = prev_len..text.len();
                            highlights.push((range, MarkdownHighlight::Style(style)));
                        }
                    }
                }
            }

            Event::Code(t) => {
                text.push_str(t.as_ref());
                region_ranges.push(prev_len..text.len());

                let link = link_url.clone().and_then(|u| Link::identify(u));
                if link.is_some() {
                    highlights.push((
                        prev_len..text.len(),
                        MarkdownHighlight::Style(MarkdownHighlightStyle {
                            underline: true,
                            ..Default::default()
                        }),
                    ));
                }
                regions.push(ParsedRegion { code: true, link });
            }

            Event::Start(tag) => match tag {
                Tag::Paragraph => new_paragraph(text, &mut list_stack),

                Tag::Heading(_, _, _) => {
                    new_paragraph(text, &mut list_stack);
                    bold_depth += 1;
                }

                Tag::CodeBlock(kind) => {
                    new_paragraph(text, &mut list_stack);
                    current_language = if let CodeBlockKind::Fenced(language) = kind {
                        language_registry
                            .language_for_name(language.as_ref())
                            .await
                            .ok()
                    } else {
                        language.clone()
                    }
                }

                Tag::Emphasis => italic_depth += 1,

                Tag::Strong => bold_depth += 1,

                Tag::Link(_, url, _) => link_url = Some(url.to_string()),

                Tag::List(number) => {
                    list_stack.push((number, false));
                }

                Tag::Item => {
                    let len = list_stack.len();
                    if let Some((list_number, has_content)) = list_stack.last_mut() {
                        *has_content = false;
                        if !text.is_empty() && !text.ends_with('\n') {
                            text.push('\n');
                        }
                        for _ in 0..len - 1 {
                            text.push_str("  ");
                        }
                        if let Some(number) = list_number {
                            text.push_str(&format!("{}. ", number));
                            *number += 1;
                            *has_content = false;
                        } else {
                            text.push_str("- ");
                        }
                    }
                }

                _ => {}
            },

            Event::End(tag) => match tag {
                Tag::Heading(_, _, _) => bold_depth -= 1,
                Tag::CodeBlock(_) => current_language = None,
                Tag::Emphasis => italic_depth -= 1,
                Tag::Strong => bold_depth -= 1,
                Tag::Link(_, _, _) => link_url = None,
                Tag::List(_) => drop(list_stack.pop()),
                _ => {}
            },

            Event::HardBreak => text.push('\n'),

            Event::SoftBreak => text.push(' '),

            _ => {}
        }
    }
}

pub fn highlight_code(
    text: &mut String,
    highlights: &mut Vec<(Range<usize>, MarkdownHighlight)>,
    content: &str,
    language: &Arc<Language>,
) {
    let prev_len = text.len();
    text.push_str(content);
    for (range, highlight_id) in language.highlight_text(&content.into(), 0..content.len()) {
        let highlight = MarkdownHighlight::Code(highlight_id);
        highlights.push((prev_len + range.start..prev_len + range.end, highlight));
    }
}

pub fn new_paragraph(text: &mut String, list_stack: &mut Vec<(Option<u64>, bool)>) {
    let mut is_subsequent_paragraph_of_list = false;
    if let Some((_, has_content)) = list_stack.last_mut() {
        if *has_content {
            is_subsequent_paragraph_of_list = true;
        } else {
            *has_content = true;
            return;
        }
    }

    if !text.is_empty() {
        if !text.ends_with('\n') {
            text.push('\n');
        }
        text.push('\n');
    }
    for _ in 0..list_stack.len().saturating_sub(1) {
        text.push_str("  ");
    }
    if is_subsequent_paragraph_of_list {
        text.push_str("  ");
    }
}