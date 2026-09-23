// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! mermaid_checker implements the `DocCheck` trait used to perform checks on
//! mermaid diagram code blocks to ensure they have a valid diagram type.

use crate::DocCheckerArgs;
use crate::checker::{DocCheck, DocCheckError};
use crate::md_element::Element;
use anyhow::Result;
use async_trait::async_trait;
use pulldown_cmark::CowStr;

static RECOGNIZED_DIAGRAM_TYPES: &[&str] = &[
    "graph",
    "flowchart",
    "sequenceDiagram",
    "classDiagram",
    "stateDiagram-v2",
    "stateDiagram",
    "erDiagram",
    "journey",
    "gantt",
    "pie",
    "quadrantChart",
    "xyChart",
    "sankey-beta",
    "sankey",
    "gitGraph",
    "c4Context",
    "mindmap",
    "timeline",
    "zenuml",
    "requirementDiagram",
    "packet-beta",
    "packet",
    "kanban",
];

pub(crate) struct MermaidChecker {}

impl MermaidChecker {
    fn check_mermaid_block(
        &self,
        elements: &[Element<'_>],
        element: &Element<'_>,
    ) -> Option<Vec<DocCheckError>> {
        let text = get_code_block_text(elements);
        let mut first_content_line = None;
        let mut inside_frontmatter = false;
        let mut seen_frontmatter_start = false;

        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            if trimmed == "---" {
                if !seen_frontmatter_start {
                    seen_frontmatter_start = true;
                    inside_frontmatter = true;
                    continue;
                } else if inside_frontmatter {
                    inside_frontmatter = false;
                    continue;
                }
            }

            if inside_frontmatter {
                continue;
            }

            if trimmed.starts_with("%%") {
                continue;
            }

            first_content_line = Some(trimmed.to_string());
            break;
        }

        let line_num = element.doc_line().line_num;
        let file_name = element.doc_line().file_name;

        match first_content_line {
            None => Some(vec![DocCheckError::new_error(
                line_num,
                file_name,
                "Mermaid diagram block is empty or contains only comments.",
            )]),
            Some(line) => {
                let first_word = line.split_whitespace().next().unwrap_or("");
                if !RECOGNIZED_DIAGRAM_TYPES.contains(&first_word) {
                    Some(vec![DocCheckError::new_error(
                        line_num,
                        file_name,
                        &format!(
                            "Invalid Mermaid diagram type '{}'. Supported types include: graph, flowchart, sequenceDiagram, pie, mindmap, timeline, etc.",
                            first_word
                        ),
                    )])
                } else {
                    None
                }
            }
        }
    }
}

fn get_code_block_text(elements: &[Element<'_>]) -> String {
    let mut text = String::new();
    for el in elements {
        if let Element::Text(t, _) = el {
            text.push_str(t);
        }
    }
    text
}

fn find_mermaid_blocks<'a, 'b>(
    element: &'a Element<'b>,
) -> Vec<(&'a CowStr<'b>, &'a [Element<'b>], &'a Element<'b>)> {
    let mut blocks = vec![];
    find_mermaid_blocks_helper(element, &mut blocks);
    blocks
}

fn find_mermaid_blocks_helper<'a, 'b>(
    element: &'a Element<'b>,
    blocks: &mut Vec<(&'a CowStr<'b>, &'a [Element<'b>], &'a Element<'b>)>,
) {
    match element {
        Element::CodeBlock(lang, elements, _) => {
            if lang.as_ref() == "mermaid" {
                blocks.push((lang, elements, element));
            }
        }
        Element::Block(_, elements, _)
        | Element::Image(_, _, _, elements, _)
        | Element::Link(_, _, _, elements, _)
        | Element::List(_, elements, _) => {
            for el in elements {
                find_mermaid_blocks_helper(el, blocks);
            }
        }
        _ => {}
    }
}

#[async_trait]
impl DocCheck for MermaidChecker {
    fn name(&self) -> &str {
        "MermaidChecker"
    }

    fn check<'a>(&mut self, element: &'a Element<'_>) -> Result<Option<Vec<DocCheckError>>> {
        let mermaid_blocks = find_mermaid_blocks(element);
        if mermaid_blocks.is_empty() {
            return Ok(None);
        }

        let mut errors = vec![];
        for (_, elements, block_element) in mermaid_blocks {
            if let Some(block_errors) = self.check_mermaid_block(elements, block_element) {
                errors.extend(block_errors);
            }
        }

        if errors.is_empty() { Ok(None) } else { Ok(Some(errors)) }
    }

    async fn post_check(&self) -> Result<Option<Vec<DocCheckError>>> {
        Ok(None)
    }
}

pub(crate) fn register_markdown_checks(_: &DocCheckerArgs) -> Result<Vec<Box<dyn DocCheck>>> {
    Ok(vec![Box::new(MermaidChecker {})])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DocLine;
    use std::path::PathBuf;

    #[test]
    fn test_valid_mermaid_diagrams() {
        let checker = MermaidChecker {};
        let doc_line = DocLine { line_num: 1, file_name: PathBuf::from("test.md") };

        // Test flowchart diagram
        let elements = vec![Element::Text("graph TD\n  A --> B".into(), doc_line.clone())];
        let block = Element::CodeBlock("mermaid".into(), elements, doc_line.clone());
        let res = checker.check_mermaid_block(
            match &block {
                Element::CodeBlock(_, e, _) => e,
                _ => unreachable!(),
            },
            &block,
        );
        assert!(res.is_none());

        // Test timeline diagram with comments
        let elements = vec![Element::Text(
            "%% This is a comment\n\n  timeline\n  2026 : Release".into(),
            doc_line.clone(),
        )];
        let block = Element::CodeBlock("mermaid".into(), elements, doc_line.clone());
        let res = checker.check_mermaid_block(
            match &block {
                Element::CodeBlock(_, e, _) => e,
                _ => unreachable!(),
            },
            &block,
        );
        assert!(res.is_none());

        // Test diagram with frontmatter config block
        let elements = vec![Element::Text(
            "---\ntitle: \"VMO Inspect Layout\"\n---\npacket-beta\n0-3: \"order\"".into(),
            doc_line.clone(),
        )];
        let block = Element::CodeBlock("mermaid".into(), elements, doc_line.clone());
        let res = checker.check_mermaid_block(
            match &block {
                Element::CodeBlock(_, e, _) => e,
                _ => unreachable!(),
            },
            &block,
        );
        assert!(res.is_none());
    }

    #[test]
    fn test_invalid_mermaid_diagrams() {
        let checker = MermaidChecker {};
        let doc_line = DocLine { line_num: 1, file_name: PathBuf::from("test.md") };

        // Test empty diagram
        let elements = vec![];
        let block = Element::CodeBlock("mermaid".into(), elements, doc_line.clone());
        let res = checker.check_mermaid_block(
            match &block {
                Element::CodeBlock(_, e, _) => e,
                _ => unreachable!(),
            },
            &block,
        );
        assert!(res.is_some());
        assert_eq!(
            res.unwrap()[0].message,
            "Mermaid diagram block is empty or contains only comments."
        );

        // Test invalid diagram type
        let elements = vec![Element::Text("invalidType TD\n  A --> B".into(), doc_line.clone())];
        let block = Element::CodeBlock("mermaid".into(), elements, doc_line.clone());
        let res = checker.check_mermaid_block(
            match &block {
                Element::CodeBlock(_, e, _) => e,
                _ => unreachable!(),
            },
            &block,
        );
        assert!(res.is_some());
        assert!(res.unwrap()[0].message.contains("Invalid Mermaid diagram type 'invalidType'"));
    }
}
