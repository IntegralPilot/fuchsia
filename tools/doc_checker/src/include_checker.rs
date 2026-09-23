// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! include_checker implements the `DocCheck` trait used to perform checks on the
//! files included using the << >> or {% include %} elements.

use crate::DocCheckerArgs;
use crate::checker::{DocCheck, DocCheckError, DocLine, ReachabilityGraph};
use crate::md_element::Element;
use crate::path_ext::normalize_and_validate_path;
use anyhow::Result;
use async_trait::async_trait;
use pulldown_cmark::Tag;
use regex::Regex;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

// path_help is a wrapper to allow mocking path checks
// exists. and is_dir.
cfg_if::cfg_if! {
    if #[cfg(test)] {
       use crate::mock_path_helper_module as path_helper;
    } else {
       use crate::path_helper_module as path_helper;
    }
}

static INCLUDE_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<<\s*(.+?\.md)\s*>>").unwrap());

static JINJA_INCLUDE_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"\{%-?\s*include\s*["']([^"']+\.md)["']\s*([^%]*?)-?%\}"#).unwrap()
});

static JINJA_IMPORT_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"\{%-?\s*import\s*["']([^"']+\.md)["']\s*([^%]*?)-?%\}"#).unwrap()
});

const FUCHSIA_SRC_PREFIX: &str = "fuchsia-src/";

pub(crate) struct IncludeChecker {
    /// Absolute path to the project root directory.
    root_dir: PathBuf,
    /// Relative path to the documents folder from the project root (e.g. "docs").
    docs_dir: PathBuf,
    /// Absolute path to the default documents directory (root_dir + docs_dir).
    full_docs_dir: PathBuf,
    /// The name of the documents directory as a String (e.g. "docs").
    docs_dir_name: String,
    /// Cached prefix for docs folder includes (e.g. "docs/").
    docs_prefix: String,
    /// Shared graph representing file reachability via includes.
    reachability_graph: ReachabilityGraph,
    /// Maps included files to the list of files and lines that include them, deferred for post-check validation.
    pending_existence_checks: std::collections::HashMap<PathBuf, Vec<(PathBuf, usize)>>,
}

impl IncludeChecker {
    pub fn new(
        root_dir: PathBuf,
        docs_dir: PathBuf,
        reachability_graph: ReachabilityGraph,
    ) -> Result<Self> {
        let full_docs_dir = root_dir.join(&docs_dir);
        if !path_helper::is_dir(&full_docs_dir) {
            anyhow::bail!(
                "Docs directory {:?} does not exist or is not a directory",
                full_docs_dir
            );
        }
        let docs_dir_name = docs_dir
            .file_name()
            .ok_or_else(|| anyhow::anyhow!("docs_dir must have a valid directory name"))?
            .to_string_lossy()
            .into_owned();
        let docs_prefix = format!("{}/", docs_dir.to_string_lossy());
        Ok(Self {
            root_dir,
            docs_dir,
            full_docs_dir,
            docs_dir_name,
            docs_prefix,
            reachability_graph,
            pending_existence_checks: Default::default(),
        })
    }

    /// Locates the enclosing documentation root folder (e.g. `docs/` or `vendor/.../docs/`)
    /// for `path` by traversing its ancestor directories.
    ///
    /// This is necessary because in multi-repo/vendor setups (such as `//vendor/google/docs/`),
    /// documentation files reside in doc trees outside the primary `self.full_docs_dir` (`//docs`).
    /// If a file is within a vendor doc tree, includes relative to that doc root should resolve
    /// against its enclosing `docs/` folder. If no ancestor matching `self.docs_dir_name` is
    /// found, it falls back to `self.full_docs_dir`.
    fn find_doc_root(&self, path: &Path) -> PathBuf {
        let docs_dir_name = OsStr::new(&self.docs_dir_name);
        for parent in path.ancestors() {
            if parent.file_name() == Some(docs_dir_name) {
                return parent.to_path_buf();
            }
        }
        self.full_docs_dir.clone()
    }

    /// Resolves an include path string from markdown into an absolute target path.
    ///
    /// Includes can be expressed in several formats:
    /// 1. Repository-relative with `docs/` prefix: e.g. `{% include "docs/sub/file.md" %}`
    ///    which resolves relative to `self.root_dir`.
    /// 2. Legacy `fuchsia-src/` prefix: e.g. `{% include "fuchsia-src/sub/file.md" %}`
    ///    used in Jekyll roadmap documentation, mapping `fuchsia-src/` to `self.docs_dir`.
    /// 3. File-relative or doc-root-relative path: e.g. `<<_common/file.md>>` or
    ///    `{% include "_includes/header.md" %}`. These are checked first relative to the
    ///    referencing file's directory (`current_file_dir`), and if not found, relative to
    ///    the enclosing documentation root (`doc_root`).
    fn resolve_include_path(
        &self,
        path_str: &str,
        current_file_dir: &Path,
        doc_root: &Path,
    ) -> anyhow::Result<PathBuf> {
        if path_str.starts_with('/') {
            anyhow::bail!("Included markdown file {:?} must be a relative path.", path_str);
        }

        if path_str.starts_with(&self.docs_prefix) {
            // Include path explicitly starts with docs folder prefix (e.g. "docs/path/to/file.md").
            // Resolve directly against the repository root.
            Ok(self.root_dir.join(path_str))
        } else if let Some(clean_path) = path_str.strip_prefix(FUCHSIA_SRC_PREFIX) {
            // NOTE: Even though 'fuchsia-src/' is technically a Copybara publishing artifact and
            // in-tree files should ideally avoid it, several existing documentation files
            // (specifically under //docs/contribute/roadmap/) use this prefix in their Jekyll
            // includes. We retain this resolution logic for backward compatibility.
            Ok(self.root_dir.join(&self.docs_dir).join(clean_path))
        } else {
            // Fallback resolution strategy: check if the file exists relative to the current
            // document's directory. If not, check relative to the enclosing doc root.
            let file_path = Path::new(path_str);
            let rel_path = current_file_dir.join(file_path);
            if path_helper::exists(&rel_path) {
                Ok(rel_path)
            } else {
                let abs_doc_path = doc_root.join(file_path);
                if path_helper::exists(&abs_doc_path) {
                    Ok(abs_doc_path)
                } else {
                    // Fallback to relative path so missing file errors report the relative path
                    Ok(rel_path)
                }
            }
        }
    }

    fn process_include(
        &mut self,
        path_str: &str,
        line_num: usize,
        referencing_file: &Path,
        current_file_dir: &Path,
        doc_root: &Path,
    ) -> Option<DocCheckError> {
        match self.resolve_include_path(path_str, current_file_dir, doc_root) {
            Ok(target_file) => match normalize_and_validate_path(&target_file, &self.root_dir) {
                Ok(normalized) => {
                    self.reachability_graph
                        .lock()
                        .unwrap()
                        .entry(referencing_file.to_path_buf())
                        .or_default()
                        .insert(normalized.clone());

                    self.pending_existence_checks
                        .entry(normalized)
                        .or_default()
                        .push((referencing_file.to_path_buf(), line_num));
                    None
                }
                Err(e) => Some(DocCheckError::new_error(
                    line_num,
                    referencing_file.to_path_buf(),
                    &e.to_string(),
                )),
            },
            Err(e) => Some(DocCheckError::new_error(
                line_num,
                referencing_file.to_path_buf(),
                &e.to_string(),
            )),
        }
    }

    /// Walks the element tree to find include directives.
    fn find_includes<'el>(
        &mut self,
        element: &'el Element<'_>,
        buffer: &mut String,
        current_line: &mut Option<&'el DocLine>,
        errors: &mut Vec<DocCheckError>,
    ) -> Result<()> {
        match element {
            Element::Block(Tag::CodeBlock(_), _, _) | Element::CodeBlock(_, _, _) => {
                // Skip code blocks
            }
            Element::Block(_, children, _)
            | Element::List(_, children, _)
            | Element::Image(_, _, _, children, _)
            | Element::Link(_, _, _, children, _) => {
                for child in children {
                    self.find_includes(child, buffer, current_line, errors)?;
                }
                if !buffer.is_empty() {
                    if let Some(doc_line) = current_line.take() {
                        let text = std::mem::take(buffer);
                        self.check_text(&text, doc_line, errors)?;
                    }
                }
            }
            Element::Html(text, doc_line) if text.trim().starts_with("<!--") => {
                if text.contains("doc-checker: ignore-missing") {
                    if current_line.is_none() {
                        *current_line = Some(doc_line);
                    }
                    buffer.push_str(text);
                }
            }
            Element::Text(text, doc_line) | Element::Html(text, doc_line) => {
                if current_line.is_none() {
                    *current_line = Some(doc_line);
                }
                buffer.push_str(text);
            }
            Element::SoftBreak(_) | Element::HardBreak(_) => {
                if current_line.is_some() {
                    buffer.push('\n');
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn check_text(
        &mut self,
        text: &str,
        doc_line: &DocLine,
        errors: &mut Vec<DocCheckError>,
    ) -> Result<()> {
        let current_file_path = &doc_line.file_name;
        let current_file_dir = current_file_path.parent().ok_or_else(|| {
            anyhow::anyhow!("File {:?} has no parent directory", current_file_path)
        })?;
        let doc_root = self.find_doc_root(current_file_path);

        for (line_offset, line) in text.lines().enumerate() {
            let trimmed = line.trim();
            if trimmed.contains("doc-checker: ignore-missing") {
                continue;
            }
            let match_line_num = doc_line.line_num + line_offset;
            if trimmed.contains("<<") || trimmed.contains("{%") {
                let standard_includes = INCLUDE_REGEX
                    .captures_iter(trimmed)
                    .filter_map(|c| c.get(1))
                    .map(|m| m.as_str());
                let jinja_includes = JINJA_INCLUDE_REGEX
                    .captures_iter(trimmed)
                    .filter_map(|c| c.get(1))
                    .map(|m| m.as_str());
                let jinja_imports = JINJA_IMPORT_REGEX
                    .captures_iter(trimmed)
                    .filter_map(|c| c.get(1))
                    .map(|m| m.as_str());

                for path_str in standard_includes.chain(jinja_includes).chain(jinja_imports) {
                    if let Some(err) = self.process_include(
                        path_str,
                        match_line_num,
                        current_file_path,
                        current_file_dir,
                        &doc_root,
                    ) {
                        errors.push(err);
                    }
                }
            }
        }
        Ok(())
    }
}

#[async_trait]
impl DocCheck for IncludeChecker {
    fn name(&self) -> &str {
        "IncludeChecker"
    }

    fn check<'a>(&mut self, element: &'a Element<'_>) -> Result<Option<Vec<DocCheckError>>> {
        let mut errors = vec![];
        let mut buffer = String::new();
        let mut current_line = None;
        self.find_includes(element, &mut buffer, &mut current_line, &mut errors)?;
        if !buffer.is_empty() {
            if let Some(doc_line) = current_line.take() {
                let text = std::mem::take(&mut buffer);
                self.check_text(&text, doc_line, &mut errors)?;
            }
        }
        if errors.is_empty() { Ok(None) } else { Ok(Some(errors)) }
    }

    async fn post_check(&self) -> Result<Option<Vec<DocCheckError>>> {
        let mut errors = vec![];
        for (target_file, references) in &self.pending_existence_checks {
            if !path_helper::exists(target_file) {
                for (referencing_file, line_num) in references {
                    errors.push(DocCheckError::new_error(
                        *line_num,
                        referencing_file.clone(),
                        &format!("Included markdown file {:?} not found.", target_file),
                    ));
                }
            }
        }
        if errors.is_empty() { Ok(None) } else { Ok(Some(errors)) }
    }
}

/// Called from main to register all the checks to preform which are implemented in this module.
pub(crate) fn register_markdown_checks(
    opt: &DocCheckerArgs,
    reachability_graph: ReachabilityGraph,
) -> Result<Vec<Box<dyn DocCheck>>> {
    let clean_root = opt.root.clone();
    let checker = IncludeChecker::new(clean_root, opt.docs_folder.clone(), reachability_graph)?;
    Ok(vec![Box::new(checker)])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::md_element::DocContext;

    #[test]
    fn test_non_matching() -> Result<()> {
        let data = [
            (
                PathBuf::from("/docs/README.md"),
                "non-markdown file OK to link to docs [non-source](https://fuchsia.googlesource.com/fuchsia/+/refs/heads/main/docs/OWNERS)",
            ),
            (PathBuf::from("/docs/README.md"), "have 2 << but no close"),
            (PathBuf::from("/docs/README.md"), "have  << but no close\n on the same line>>"),
            (PathBuf::from("/docs/README.md"), "have a non-markdown file name  <<you name here>>"),
            (PathBuf::from("/docs/README.md"), "spaces   < <a-file.md> >"),
            (PathBuf::from("/docs/README.md"), "spaces   <<a-file.md> >"),
            (PathBuf::from("/docs/README.md"), "spaces   < <a-file.md>>"),
            (PathBuf::from("/docs/README.md"), "exists <<a-file.md>>"),
            (
                PathBuf::from("/docs/README.md"),
                "\n```md\nThis is a sample in codeblock to\n<<missing.md>>\n```\n",
            ),
            (
                PathBuf::from("/docs/README.md"),
                "\n<!-- This is a comment {% include \"docs/missing.md\" %} -->\n",
            ),
        ];

        for (file, input) in data {
            let mut checker =
                IncludeChecker::new(PathBuf::from("/"), PathBuf::from("docs"), Default::default())
                    .unwrap();
            let callback = &mut |broken_link: pulldown_cmark::BrokenLink<'_>| {
                DocContext::handle_broken_link(broken_link, input)
            };
            let ctx = DocContext::new(file, input, Some(callback));
            for ele in ctx {
                let errors = checker.check(&ele)?;
                assert!(errors.is_none(), "Expected no errors got {:?}", errors);
            }
            if let Some(post_errs) = futures::executor::block_on(checker.post_check())? {
                assert!(post_errs.is_empty(), "Expected no post errors, got {:?}", post_errs);
            }
        }
        Ok(())
    }

    #[test]
    fn test_errors() -> Result<()> {
        let data = [
            (
                PathBuf::from("/docs/README.md"),
                "does not exist <<missing.md>>",
                vec![DocCheckError::new_error(
                    1,
                    PathBuf::from("/docs/README.md"),
                    "Included markdown file \"/docs/missing.md\" not found.",
                )],
            ),
            (
                PathBuf::from("/docs/README.md"),
                " no absolute\" <</docs/README.md>>",
                vec![DocCheckError::new_error(
                    1,
                    PathBuf::from("/docs/README.md"),
                    "Included markdown file \"/docs/README.md\" must be a relative path.",
                )],
            ),
            (
                PathBuf::from("/docs/README.md"),
                "does not exist {% include \"missing.md\" %}",
                vec![DocCheckError::new_error(
                    1,
                    PathBuf::from("/docs/README.md"),
                    "Included markdown file \"/docs/missing.md\" not found.",
                )],
            ),
            (
                PathBuf::from("/docs/README.md"),
                "does not exist {% import \"missing.md\" %}",
                vec![DocCheckError::new_error(
                    1,
                    PathBuf::from("/docs/README.md"),
                    "Included markdown file \"/docs/missing.md\" not found.",
                )],
            ),
        ];

        for (file, input, expected) in data {
            let mut checker =
                IncludeChecker::new(PathBuf::from("/"), PathBuf::from("docs"), Default::default())
                    .unwrap();
            let callback = &mut |broken_link: pulldown_cmark::BrokenLink<'_>| {
                DocContext::handle_broken_link(broken_link, input)
            };
            let ctx = DocContext::new(file, input, Some(callback));
            let mut actual_errors = vec![];
            for ele in ctx {
                if let Some(errs) = checker.check(&ele)? {
                    actual_errors.extend(errs);
                }
            }
            if let Some(post_errs) = futures::executor::block_on(checker.post_check())? {
                actual_errors.extend(post_errs);
            }
            actual_errors.sort();
            assert_eq!(actual_errors.len(), expected.len());
            let mut expected_iter = expected.iter();
            for actual in actual_errors {
                if let Some(expected_err) = expected_iter.next() {
                    assert_eq!(&actual, expected_err);
                }
            }
        }
        Ok(())
    }

    #[test]
    fn test_reachability_graph_and_prefixes() -> Result<()> {
        let reachability_graph = ReachabilityGraph::default();
        let mut checker = IncludeChecker::new(
            PathBuf::from("/"),
            PathBuf::from("docs"),
            reachability_graph.clone(),
        )
        .unwrap();

        // Current file is at /docs/sub/README.md
        let file = PathBuf::from("/docs/sub/README.md");
        let input = "include with prefix {% include \"docs/sub/file.md\" %}\n\
                     include with fuchsia-src {% include \"fuchsia-src/sub/fuchsia_file.md\" %}\n\
                     relative include <<_common/relative_file.md>>\n\
                     import with alias {% import \"docs/sub/_macros.md\" as macros %}\n\
                     whitespace trimmed {%- import \"docs/sub/_macros2.md\" as macros2 -%}\n\
                     trimmed include {%- include \"docs/sub/_trimmed.md\" -%}\n\
                     spaced relative <<   _common/spaced.md   >>\n\
                     single quoted import {% import 'docs/sub/_single.md' as sq %}\n\
                     multiple on line {% import \"docs/sub/_first.md\" as f %} and {% include \"docs/sub/_second.md\" %}";
        let callback = &mut |broken_link: pulldown_cmark::BrokenLink<'_>| {
            DocContext::handle_broken_link(broken_link, input)
        };
        let ctx = DocContext::new(file.clone(), input, Some(callback));
        for ele in ctx {
            let errors = checker.check(&ele)?;
            assert!(errors.is_none(), "Expected no errors, got {:?}", errors);
        }
        if let Some(post_errs) = futures::executor::block_on(checker.post_check())? {
            let actual_errors: Vec<DocCheckError> = post_errs;
            assert!(actual_errors.is_empty(), "Expected no post errors, got {:?}", actual_errors);
        }

        let graph = reachability_graph.lock().unwrap();
        assert!(graph.contains_key(&file));
        let targets = graph.get(&file).unwrap();
        assert!(targets.contains(&PathBuf::from("/docs/sub/file.md")));
        assert!(targets.contains(&PathBuf::from("/docs/sub/fuchsia_file.md")));
        assert!(targets.contains(&PathBuf::from("/docs/sub/_common/relative_file.md")));
        assert!(targets.contains(&PathBuf::from("/docs/sub/_macros.md")));
        assert!(targets.contains(&PathBuf::from("/docs/sub/_macros2.md")));
        assert!(targets.contains(&PathBuf::from("/docs/sub/_trimmed.md")));
        assert!(targets.contains(&PathBuf::from("/docs/sub/_common/spaced.md")));
        assert!(targets.contains(&PathBuf::from("/docs/sub/_single.md")));
        assert!(targets.contains(&PathBuf::from("/docs/sub/_first.md")));
        assert!(targets.contains(&PathBuf::from("/docs/sub/_second.md")));
        Ok(())
    }
}
