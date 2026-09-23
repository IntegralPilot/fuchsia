// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! doc_checker is a CLI tool to check markdown files for correctness in
//! the Fuchsia project.

pub(crate) use crate::checker::{
    DocCheck, DocCheckError, DocLine, DocYamlCheck, ErrorLevel, ExemptionSet, ReachabilityGraph,
};
pub(crate) use crate::md_element::DocContext;
pub(crate) use crate::path_ext::DocPathExt;
use anyhow::{Context, Result, bail};
use argh::FromArgs;
use glob::glob;
use serde_yaml::Value;
use std::fs::{self, File};
use std::io::BufReader;
use std::path::PathBuf;
mod checker;
mod include_checker;
mod link_checker;
mod md_element;
mod mermaid_checker;
mod parser;
pub(crate) mod path_ext;
mod yaml;

static EXEMPT_REGEX: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"(?i)<!-+\s*doc[-_]checker\s*:\s*ignore[-_]unused(?:\s*[:\s-][^>]*)?-+>")
        .unwrap()
});

// path_helper includes methods to check path attributes
// so that these methods can be mocked for unit tests.
pub mod path_helper_module {
    use std::path::Path;

    pub fn exists(path: &Path) -> bool {
        path.exists()
    }

    pub fn is_dir(path: &Path) -> bool {
        path.is_dir()
    }
}

// Used when testing, the existence and is_dir
// is based on the name.
// Originally, This was attempted using mockall::automock.
// However, since the mocked module is global, and tests
// are run multithreaded, keeping the mocking context exclusive
// was hard. A mutex could be used, but mutex and async code
// do not work nicely together.
//
// As a result, a name based mock is used for all the tests.
#[cfg(test)]
pub(crate) mod mock_path_helper_module {
    use std::path::Path;

    pub fn exists(path: &Path) -> bool {
        let path_str = path.to_string_lossy();

        // If the path actually exists, return true. This allows for
        // staging test data.
        path.exists()
            // if is_dir returns true, the directory needs to exist as well.
            || (is_dir(path) && !path_str.ends_with("no-extension"))
            // markdown files exist with a couple exceptions.
            || (path_str.ends_with(".md")
                && (!path_str.ends_with("no_readme/README.md")
                    && !path_str.ends_with("unused/README.md")
                    && !path_str.ends_with("missing.md")
                    && !path_str.ends_with("no-extension")))
            // OWNERS file exists.
            || path.ends_with("OWNERS")
    }

    pub fn is_dir(path: &Path) -> bool {
        // If the path is actually a directory, return true. This allows for
        // staged test data.
        path.is_dir()
            // Paths with no extension are directories, except OWNERS.
            || (path.extension().is_none() && !path.ends_with("OWNERS"))
    }
}

/// Check the markdown documentation using a variety of checks.
#[derive(Debug, FromArgs)]
pub struct DocCheckerArgs {
    /// path to the root of the checkout of the project.
    #[argh(option, default = r#"PathBuf::from(".")"#)]
    pub root: PathBuf,

    /// name of project to check, defaults to fuchsia.
    #[argh(option, default = r#"String::from("fuchsia")"#)]
    pub project: String,

    /// (Experimental) Name of the folder inside the project
    ///  which contains documents to check. Defaults to 'docs'.
    #[argh(option, default = r#"PathBuf::from("docs")"#)]
    pub docs_folder: PathBuf,

    /// resolve http(s) links
    #[argh(switch)]
    pub check_external_links: bool,

    /// output in JSON format
    #[argh(switch)]
    pub json: bool,

    /// allow links to fuchsia-src. Usually links to
    /// fuchsia-src should be written as file paths
    /// to /docs.
    #[argh(switch)]
    pub allow_fuchsia_src_links: bool,

    /// path to reference docs. Links to the main docs folder.
    #[argh(option)]
    pub reference_docs_root: Option<PathBuf>,

    /// do not check links between docs,
    #[argh(switch)]
    pub skip_link_check: bool,

    /// allow unreferenced hidden files (files starting with _) from being published.
    #[argh(switch)]
    pub allow_unreferenced_hidden: bool,
}

#[fuchsia::main]
async fn main() -> Result<()> {
    let mut opt: DocCheckerArgs = argh::from_env();

    // Canonicalize the root directory so the rest of the code can rely on
    // the root directory existing and being a normalized path.
    opt.root =
        opt.root.canonicalize().context(format!("invalid root dir for source: {:?} ", opt.root))?;
    if let Some(reference_root) = opt.reference_docs_root {
        opt.reference_docs_root =
            Some(reference_root.canonicalize().context(format!(
                "could not get canonical reference root for {:?}",
                reference_root
            ))?);
    }

    if let Some(mut errors) = do_main(&opt).await? {
        // Output the result
        let mut error_count = 0;
        let mut warning_count = 0;
        let mut info_count = 0;
        errors.sort();
        if opt.json {
            println!("{}", serde_json::to_string(&errors)?);
            if errors.iter().any(|e| e.level == ErrorLevel::Error) {
                std::process::exit(1);
            }
        } else {
            for e in &errors {
                match e.level {
                    ErrorLevel::Info => info_count += 1,
                    ErrorLevel::Warning => warning_count += 1,
                    ErrorLevel::Error => error_count += 1,
                }
                println!("{}\n", e);
            }
            // Only bail if there are errors, warnings should return OK.
            if error_count > 0 {
                bail!(
                    "Found {} errors, {} warnings, {} info.",
                    error_count,
                    warning_count,
                    info_count
                )
            } else {
                println!(
                    "Found {} errors, {} warnings, {} info.",
                    error_count, warning_count, info_count
                )
            }
        }
    } else if opt.json {
        println!("[]");
    } else {
        println!("No errors found");
    }
    Ok(())
}

/// The actual main function. It is refactored like this to make it easier
/// to run it in a unit test.
async fn do_main(opt: &DocCheckerArgs) -> Result<Option<Vec<DocCheckError>>> {
    let root_dir = &opt.root;
    let docs_project = &opt.project;
    let docs_dir = root_dir.join(&opt.docs_folder);

    eprintln!("Checking Project {} {:?}.", docs_project, docs_dir);

    // Find all the markdown in the docs folder.
    let pattern = format!("{}/**/*.md", docs_dir.to_string_lossy());
    let mut markdown_files: Vec<PathBuf> =
        glob(&pattern)?.filter_map(|p| p.ok()).filter(|p| !p.is_macos_hidden_doc()).collect();

    // Find all the .yaml files.
    let yaml_pattern = format!("{}/**/*.yaml", docs_dir.to_string_lossy());
    let mut yaml_files: Vec<PathBuf> =
        glob(&yaml_pattern)?.filter_map(|p| p.ok()).filter(|p| !p.is_ignored_doc()).collect();

    if let Some(reference_root) = &opt.reference_docs_root {
        eprintln!("Also checking reference docs in {reference_root:?}.");
        let pattern = format!("{}/**/*.md", reference_root.to_string_lossy());
        let reference_markdown =
            glob(&pattern)?.filter_map(|p| p.ok()).filter(|p| !p.is_macos_hidden_doc());
        markdown_files.extend(reference_markdown);

        let yaml_pattern = format!("{}/**/*.yaml", reference_root.to_string_lossy());
        yaml_files
            .extend(glob(&yaml_pattern)?.filter_map(|p| p.ok()).filter(|p| !p.is_ignored_doc()));
    }
    eprintln!(
        "Checking {} markdown files and {} yaml files",
        markdown_files.len(),
        yaml_files.len()
    );

    /*
    Doc checking is broken into a couple major phases.

    1. Checks are registered from the modules that have structs that implement the DocCheck trait.
    2. Each markdown file is parsed into a stream of Elements. Each element is passed to each registered checker.
    3. After all the markdown files are parsed, the post-check check is called on each checker. This allows
       checkers to perform cross-file checks and checks that used data collected from the individual documents.
    4. Each yaml file is checked for each yaml checker registered.
    5. After all the yaml is checked, the post-check check is called.
    6. All the errors are collected and returned.
    */
    let mut markdown_checks: Vec<Box<dyn DocCheck>> = vec![];
    let mut errors: Vec<DocCheckError> = vec![];

    let reachability_graph: ReachabilityGraph = Default::default();
    let exemption_set: ExemptionSet = Default::default();

    markdown_checks
        .extend(link_checker::register_markdown_checks(opt, reachability_graph.clone())?);
    markdown_checks
        .extend(include_checker::register_markdown_checks(opt, reachability_graph.clone())?);
    markdown_checks.extend(mermaid_checker::register_markdown_checks(opt)?);

    let mut yaml_checks =
        yaml::register_yaml_checks(opt, reachability_graph.clone(), exemption_set.clone())?;

    let markdown_errors: Vec<DocCheckError> =
        check_markdown(&markdown_files, &mut markdown_checks, exemption_set.clone())?;
    errors.extend(markdown_errors);

    let yaml_errors = check_yaml(&yaml_files, &mut yaml_checks)?;
    errors.extend(yaml_errors);

    // Post checks
    for c in yaml_checks {
        match c.post_check(&markdown_files, &yaml_files).await {
            Ok(Some(check_errors)) => errors.extend(check_errors),
            Ok(None) => {}
            Err(e) => errors.push(DocCheckError::new_error(
                0,
                "".into(),
                &format!("Error {} running check: {} ", e, c.name()),
            )),
        }
    }
    for c in markdown_checks {
        match c.post_check().await {
            Ok(Some(check_errors)) => errors.extend(check_errors),
            Ok(None) => {}
            Err(e) => errors.push(DocCheckError::new_error(
                0,
                "".into(),
                &format!("Error {} running check: {} ", e, c.name()),
            )),
        }
    }

    let result = if errors.is_empty() { None } else { Some(errors) };
    Ok(result)
}

/// Given the list of markdown files to check, iterate over each check, collecting any errors.
pub fn check_markdown<'a>(
    files: &[PathBuf],
    checks: &'a mut [Box<dyn DocCheck + 'static>],
    exemption_set: ExemptionSet,
) -> Result<Vec<DocCheckError>> {
    let mut errors: Vec<DocCheckError> = vec![];

    for mdfile in files {
        let mdcontent = fs::read_to_string(mdfile).expect("Unable to read file");
        if EXEMPT_REGEX.is_match(&mdcontent) {
            exemption_set.lock().unwrap().insert(mdfile.clone());
        }
        let mut callback = |broken_link: pulldown_cmark::BrokenLink<'_>| {
            DocContext::handle_broken_link(broken_link, &mdcontent)
        };

        let doc_context = DocContext::new(mdfile.clone(), &mdcontent, Some(&mut callback));

        for element in doc_context {
            for c in &mut *checks {
                match c.check(&element) {
                    Ok(Some(check_errors)) => errors.extend(check_errors),
                    Ok(None) => {}
                    Err(e) => errors.push(DocCheckError::new_error(
                        0,
                        "".into(),
                        &format!("Error {} running check: {} ", e, c.name()),
                    )),
                }
            }
        }
    }
    Ok(errors)
}

fn check_yaml<'a>(
    yaml_files: &[PathBuf],
    checks: &'a mut [Box<dyn DocYamlCheck + 'static>],
) -> Result<Vec<DocCheckError>> {
    let mut errors: Vec<DocCheckError> = vec![];

    for yaml_file in yaml_files {
        let f = File::open(yaml_file)?;
        let val: Value = match serde_yaml::from_reader(BufReader::new(f)) {
            Ok(v) => v,
            Err(e) => bail!("Error parsing {:?}: {}", yaml_file, e),
        };
        for c in &mut *checks {
            match c.check(yaml_file, &val) {
                Ok(Some(check_errors)) => errors.extend(check_errors),
                Ok(None) => {}
                Err(e) => errors.push(DocCheckError::new_error(
                    0,
                    "".into(),
                    &format!("Error {} running check: {} ", e, c.name()),
                )),
            }
        }
    }

    Ok(errors)
}

#[cfg(test)]
mod test {
    use super::*;
    use std::env;

    #[fuchsia::test]
    async fn test_project_test() -> Result<()> {
        let opt = DocCheckerArgs {
            root: PathBuf::from("doc_checker_test_data"),
            project: "fuchsia".to_string(),
            docs_folder: PathBuf::from("docs"),
            check_external_links: false,
            json: false,
            allow_fuchsia_src_links: false,
            reference_docs_root: None,
            skip_link_check: false,
            allow_unreferenced_hidden: false,
        };

        // Set the current directory to the executable dir so the relative test paths WAI.
        env::set_current_dir(env::current_exe()?.parent().unwrap_or(&PathBuf::from(".")))?;

        let expected: Vec<DocCheckError> = vec![
            DocCheckError::new_error(
                10,
                PathBuf::from("doc_checker_test_data/docs/README.md"),
                "in-tree link to /docs/missing.md could not be found at \"doc_checker_test_data/docs/missing.md\"",
            ),
            DocCheckError::new_error(
                12,
                PathBuf::from("doc_checker_test_data/docs/README.md"),
                "Should not link to https://fuchsia.dev/fuchsia-src/path.md via https, use relative filepath",
            ),
            DocCheckError::new_error(
                21,
                PathBuf::from("doc_checker_test_data/docs/README.md"),
                "Obsolete or invalid project garnet: https://fuchsia.googlesource.com/garnet/+/refs/heads/main/README.md",
            ),
            DocCheckError::new_error(
                23,
                PathBuf::from("doc_checker_test_data/docs/README.md"),
                "Cannot normalize /docs/../../README.md, references parent beyond root.",
            ),
            DocCheckError::new_error_helpful(
                30,
                PathBuf::from("doc_checker_test_data/docs/README.md"),
                "in-tree link to /docs/no-extension could not be found at \"doc_checker_test_data/docs/no-extension\"",
                "\"no-extension.md\"",
            ),
            DocCheckError::new_error(
                6,
                PathBuf::from("doc_checker_test_data/docs/_common/_included.md"),
                "in-tree link to /docs/missing.md could not be found at \"doc_checker_test_data/docs/missing.md\"",
            ),
            DocCheckError::new_error(
                9,
                PathBuf::from("doc_checker_test_data/docs/include_here.md"),
                "Included markdown file \"doc_checker_test_data/docs/_common/missing.md\" not found.",
            ),
            DocCheckError::new_error(
                13,
                PathBuf::from("doc_checker_test_data/docs/include_here.md"),
                "Included markdown file \"/docs/_common/_included.md\" must be a relative path.",
            ),
            DocCheckError::new_error(
                2,
                PathBuf::from("doc_checker_test_data/docs/no_readme/details.md"),
                "in-tree link to /docs/no_readme could not be found at \"doc_checker_test_data/docs/no_readme\" or  \"doc_checker_test_data/docs/no_readme/README.md\"",
            ),
            DocCheckError::new_error(
                4,
                PathBuf::from("doc_checker_test_data/docs/path.md"),
                "in-tree link to /docs/missing-image.png could not be found at \"doc_checker_test_data/docs/missing-image.png\"",
            ),
            // There are 3 instances of [i] on the same line.
            DocCheckError::new_error_helpful(
                17,
                PathBuf::from("doc_checker_test_data/docs/path.md"),
                "Unknown reference link to [i][i]",
                "making sure you added a matching [i]: YOUR_LINK_HERE below this reference. If this is in a triple tick code block, consider removing empty lines - see https://fxbug.dev/373449734.",
            ),
            DocCheckError::new_error_helpful(
                17,
                PathBuf::from("doc_checker_test_data/docs/path.md"),
                "Unknown reference link to [i][i]",
                "making sure you added a matching [i]: YOUR_LINK_HERE below this reference. If this is in a triple tick code block, consider removing empty lines - see https://fxbug.dev/373449734.",
            ),
            DocCheckError::new_error_helpful(
                17,
                PathBuf::from("doc_checker_test_data/docs/path.md"),
                "Unknown reference link to [i][i]",
                "making sure you added a matching [i]: YOUR_LINK_HERE below this reference. If this is in a triple tick code block, consider removing empty lines - see https://fxbug.dev/373449734.",
            ),
            DocCheckError::new_error(
                6,
                PathBuf::from("doc_checker_test_data/docs/second.md"),
                "Invalid link http://{}.com/markdown : invalid uri character",
            ),
            DocCheckError::new_error(
                10,
                PathBuf::from("doc_checker_test_data/docs/second.md"),
                "Cannot normalize /docs/../../missing.md, references parent beyond root.",
            ),
            DocCheckError::new_error(
                28,
                PathBuf::from("doc_checker_test_data/docs/second.md"),
                "Invalid Mermaid diagram type 'invalid_type'. Supported types include: graph, flowchart, sequenceDiagram, pie, mindmap, timeline, etc.",
            ),
            DocCheckError::new_error(
                1,
                PathBuf::from("doc_checker_test_data/docs/unused/_toc.yaml"),
                "in-tree link to /docs/unused could not be found at \"doc_checker_test_data/docs/unused\" or  \"doc_checker_test_data/docs/unused/README.md\"",
            ),
            DocCheckError::new_error(
                0,
                PathBuf::from("doc_checker_test_data/docs/_toc.yaml"),
                "Cannot find file \"doc_checker_test_data/docs/missing/_toc.yaml\" included in \"doc_checker_test_data/docs/_toc.yaml\"",
            ),
            DocCheckError::new_error(
                0,
                PathBuf::from("doc_checker_test_data/docs/cycle/_toc.yaml"),
                "YAML files cannot include themselves \"doc_checker_test_data/docs/cycle/_toc.yaml\"",
            ),
            DocCheckError::new_error(
                0,
                PathBuf::from("doc_checker_test_data/docs/unreachable.md"),
                "File not referenced in any _toc.yaml files.",
            ),
            DocCheckError::new_error(
                0,
                PathBuf::from("doc_checker_test_data/docs/unused/_toc.yaml"),
                "File not reachable via _toc include references.",
            ),
            DocCheckError::new_error(
                0,
                PathBuf::from("doc_checker_test_data/docs/_hidden_dir/unreachable.md"),
                "Unused hidden file. If this file is only referenced by external repositories, append '<!-- doc-checker: ignore-unused -->' to the file to exempt it.",
            ),
            DocCheckError::new_error(
                0,
                PathBuf::from("doc_checker_test_data/docs/_unreachable_hidden.md"),
                "Unused hidden file. If this file is only referenced by external repositories, append '<!-- doc-checker: ignore-unused -->' to the file to exempt it.",
            ),
            DocCheckError::new_warning(
                0,
                PathBuf::from("doc_checker_test_data/docs/README.md"),
                "Redundant exemption comment. File is reachable locally; remove '<!-- doc-checker: ignore-unused -->' to clean up.",
            ),
            DocCheckError::new_error(
                0,
                PathBuf::from("doc_checker_test_data/docs/_toc.yaml"),
                "Cannot reference hidden file /docs/_hidden_in_toc.md in _toc.yaml. Hidden files are not published by Devsite.",
            ),
        ];

        if let Some(mut actual_errors) = do_main(&opt).await? {
            let mut expected = expected;
            actual_errors.sort();
            expected.sort();
            let mut expected_iter = expected.iter();
            for actual in actual_errors {
                if let Some(expected) = expected_iter.next() {
                    assert_eq!(&actual, expected);
                } else {
                    bail!("Unexpected error: {:?}", actual);
                }
            }
            let expected_errors: Vec<&DocCheckError> = expected_iter.collect();
            // Should be no other expected errors
            if !expected_errors.is_empty() {
                bail!("Expected at least one more error. Missing error(s): {:?}", expected_errors);
            }
        }
        Ok(())
    }
}
