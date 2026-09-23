// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! yaml checks the yaml  files that part of the //docs publishing process
//! for correctness.

use self::toc_checker::Toc;
use crate::link_checker::{
    LinkReference, PUBLISHED_DOCS_HOST, check_external_links, do_check_link, do_in_tree_check,
    is_intree_link,
};
use crate::path_ext::{DocPathExt, normalize_path};
use crate::{
    DocCheckError, DocCheckerArgs, DocLine, DocYamlCheck, ExemptionSet, ReachabilityGraph,
};
use anyhow::Result;
use async_trait::async_trait;
use pulldown_cmark::{BrokenLink, Event, LinkType, Options, Parser, Tag};
use regex::Regex;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_yaml::{Mapping, Value};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

mod toc_checker;

cfg_if::cfg_if! {
    if #[cfg(test)] {
        use crate::mock_path_helper_module as path_helper;
    } else {
       use crate::path_helper_module as path_helper;
    }
}

#[derive(Deserialize, PartialEq, Debug)]
struct AreaEntry {
    name: String,
    api_primary: String,
    api_secondary: String,
    description: Option<String>,
    examples: Option<Vec<Mapping>>,
}

#[derive(Deserialize, PartialEq, Debug)]
struct Deprecations {
    included: Vec<FromTo>,
}

#[derive(Deserialize, Debug)]
// Dead code is used here so the names
// of the fields can be used by Deserialize
// even though there is no reading of the fields.
#[allow(dead_code)]
struct DriverEpitaph {
    short_description: String,
    deletion_reason: String,
    gerrit_change_id: String,
    available_in_git: String,
    areas: Option<Vec<String>>,
    path: String,
}

#[derive(Deserialize, Debug)]
// Dead code is used here so the names
// of the fields can be used by Deserialize
// even though there is no reading of the fields.
#[allow(dead_code)]
struct AllDrivers {
    drivers_areas: Vec<String>,
    drivers_documentation: Vec<Value>,
}

#[derive(Deserialize, PartialEq, Debug)]
struct EngCouncil {
    members: Vec<String>,
}

#[derive(Deserialize, Eq, PartialEq, Debug)]
pub struct FromTo {
    pub from: String,
    pub to: String,
}

#[derive(Deserialize, Debug)]
// Dead code is used here so the names
// of the fields can be used by Deserialize
// even though there is no reading of the fields.
#[allow(dead_code)]
struct GlossaryTerm {
    term: String,
    short_description: String,
    full_description: Option<String>,
    see_also: Option<Vec<String>>,
    related_guides: Vec<String>,
    area: Vec<String>,
}

#[derive(Deserialize, Debug)]
// Dead code is used here so the names
// of the fields can be used by Deserialize
// even though there is no reading of the fields.
#[allow(dead_code)]
struct GuideEntry {
    #[serde(alias = "type")]
    entry_type: String,
    product: String,
    board: String,
    method: String,
    host: String,
    url: String,
    title: String,
}

#[derive(Deserialize, Debug)]
// Dead code is used here so the names
// of the fields can be used by Deserialize
// even though there is no reading of the fields.
#[allow(dead_code)]
struct Metadata {
    descriptions: Mapping,
    columns: Vec<String>,
    types: Vec<String>,
    products: Vec<String>,
    boards: Vec<String>,
    methods: Vec<String>,
    hosts: Vec<String>,
    guides: Vec<GuideEntry>,
}

#[derive(Deserialize, Debug)]
struct ProblemEntry {
    key: String,
    use_case: String,
    #[serde(default)]
    tools: Vec<String>,
    description: String,
    #[serde(default, alias = "related-problems")]
    related_problems: Vec<String>,
}

#[derive(Deserialize, PartialEq, Debug)]
struct Redirects {
    redirects: Option<Vec<FromTo>>,
}

#[derive(Deserialize, PartialEq, Debug)]
struct RfcEntry {
    name: String,
    title: String,
    short_description: String,
    authors: Vec<String>,
    file: String,
    area: Vec<String>,
    issue: Vec<String>,
    gerrit_change_id: Vec<String>,
    status: String,
    reviewers: Vec<String>,
    submitted: String,
    reviewed: String,
}

#[derive(Deserialize, PartialEq, Debug)]
struct RoadmapEntry {
    workstream: String,
    area: String,
    category: Vec<String>,
    bug: Option<Vec<String>>,
}

#[derive(Deserialize, Debug)]
// Dead code is used here so the names
// of the fields can be used by Deserialize
// even though there is no reading of the fields.
#[allow(dead_code)]
struct SysConfigEntry {
    name: String,
    description: String,
    architecture: String,
    #[serde(alias = "RAM")]
    ram: Option<String>,
    storage: Option<String>,
    manufacturer_link: Option<String>,
    board_driver_location: String,
}

#[derive(Deserialize, Debug)]
// Dead code is used here so the names
// of the fields can be used by Deserialize
// even though there is no reading of the fields.
#[allow(dead_code)]
struct ToolsEntry {
    name: String,
    team: String,
    links: Mapping,
    description: String,
    related: Option<Vec<String>>,
}

#[derive(Debug, Clone)]
// Represents a yaml file included in another yaml file.
// The container is the file with the reference to the
// included_file.
pub(crate) struct IncludedYaml {
    pub(crate) container: PathBuf,
    pub(crate) included_file: PathBuf,
}

#[derive(Debug)]
pub(crate) struct YamlChecker {
    root_dir: PathBuf,
    docs_folder: PathBuf,
    project: String,
    check_external_links: bool,
    allow_fuchsia_src_links: bool,
    reference_docs_root: Option<PathBuf>,
    reference_prefix: PathBuf,
    external_links: Vec<LinkReference>,
    problems: Vec<(PathBuf, ProblemEntry)>,
    tools: HashSet<String>,
    tools_entries: Vec<(PathBuf, ToolsEntry)>,
    reachability_graph: ReachabilityGraph,
    exemption_set: ExemptionSet,
    allow_unreferenced_hidden: bool,
}

impl YamlChecker {
    fn validate_problems(&self, problem_keys: &HashSet<String>, errors: &mut Vec<DocCheckError>) {
        let mut seen_problems = HashSet::new();

        // Check for duplicates
        let duplicate_errors = self
            .problems
            .iter()
            .filter(|(_, entry)| !seen_problems.insert(entry.key.as_str()))
            .map(|(file_name, entry)| {
                DocCheckError::new_error(
                    1,
                    file_name.clone(),
                    &format!("Duplicate problem key: {}", entry.key),
                )
            });
        errors.extend(duplicate_errors);

        // Validate that tools referenced in problems exist
        let tool_errors = self.problems.iter().flat_map(|(file_name, problem)| {
            problem.tools.iter().filter(|tool| !self.tools.contains(*tool)).map(move |tool| {
                DocCheckError::new_error(
                    1,
                    file_name.clone(),
                    &format!(
                        "Tool '{}' referenced in problem '{}' does not exist in _tools.yaml",
                        tool, problem.key
                    ),
                )
            })
        });
        errors.extend(tool_errors);

        // Validate related problems
        let related_errors = self.problems.iter().flat_map(|(file_name, problem)| {
            problem.related_problems.iter().filter_map(move |related| {
                if related == &problem.key {
                    Some(DocCheckError::new_error(
                        1,
                        file_name.clone(),
                        &format!(
                            "Problem '{}' cannot reference itself as a related problem",
                            problem.key
                        ),
                    ))
                } else if !problem_keys.contains(related) {
                    Some(DocCheckError::new_error(
                        1,
                        file_name.clone(),
                        &format!(
                            "Related problem '{}' referenced in problem '{}' does not exist",
                            related, problem.key
                        ),
                    ))
                } else {
                    None
                }
            })
        });
        errors.extend(related_errors);

        // Check markdown links in use_case and description
        let markdown_errors = self.problems.iter().flat_map(|(file_name, problem)| {
            let use_case_errors =
                self.check_markdown_links(file_name, &problem.use_case, problem_keys);
            let desc_errors =
                self.check_markdown_links(file_name, &problem.description, problem_keys);
            use_case_errors.into_iter().chain(desc_errors)
        });
        errors.extend(markdown_errors);
    }

    fn validate_tools(&self, problem_keys: &HashSet<String>, errors: &mut Vec<DocCheckError>) {
        let mut seen_tools = HashSet::new();
        let duplicate_tool_errors = self
            .tools_entries
            .iter()
            .filter(|(_, entry)| !seen_tools.insert(entry.name.as_str()))
            .map(|(file_name, entry)| {
                DocCheckError::new_error(
                    1,
                    file_name.clone(),
                    &format!("Duplicate tool name: {}", entry.name),
                )
            });
        errors.extend(duplicate_tool_errors);

        let tool_related_errors = self.tools_entries.iter().flat_map(|(file_name, tool_entry)| {
            tool_entry
                .related
                .iter()
                .flatten()
                .filter(|related| !self.tools.contains(*related))
                .map(move |related| {
                    DocCheckError::new_error(
                        1,
                        file_name.clone(),
                        &format!(
                            "Related tool '{}' referenced in tool '{}' does not exist",
                            related, tool_entry.name
                        ),
                    )
                })
        });
        errors.extend(tool_related_errors);

        let tool_markdown_errors = self.tools_entries.iter().flat_map(|(file_name, tool_entry)| {
            self.check_markdown_links(file_name, &tool_entry.description, problem_keys)
        });
        errors.extend(tool_markdown_errors);
    }

    // Helper to parse and check markdown links inside YAML text fields.
    // It uses pulldown_cmark to find links (inline, reference, collapsed, shortcut, anchor).
    // Reference links are validated against the known set of tools and problems.
    //
    // TODO(https://fxbug.dev/289256340): Incorporate these markdown links into the LinkChecker flow
    // to validate external links and anchors comprehensively.
    fn check_markdown_links(
        &self,
        filename: &Path,
        text: &str,
        problem_keys: &HashSet<String>,
    ) -> Vec<DocCheckError> {
        let mut errors = vec![];
        let mut callback_errors = vec![];
        let doc_line = DocLine { line_num: 1, file_name: filename.to_path_buf() };

        {
            let mut callback = |broken_link: BrokenLink<'_>| {
                let ref_label = broken_link.reference.as_ref();
                if !self.tools.contains(ref_label) && !problem_keys.contains(ref_label) {
                    callback_errors.push(DocCheckError::new_error(
                        doc_line.line_num,
                        doc_line.file_name.clone(),
                        &format!("Unknown reference link: [{}]", ref_label),
                    ));
                }
                None
            };

            let parser =
                Parser::new_with_broken_link_callback(text, Options::empty(), Some(&mut callback));

            for event in parser {
                match event {
                    Event::Start(Tag::Link { link_type, dest_url, .. })
                    | Event::Start(Tag::Image { link_type, dest_url, .. }) => match link_type {
                        LinkType::Autolink
                        | LinkType::Collapsed
                        | LinkType::Inline
                        | LinkType::Reference
                        | LinkType::Shortcut
                        | LinkType::WikiLink { .. } => {
                            if dest_url.starts_with("#") {
                                let anchor = &dest_url[1..];
                                if !self.tools.contains(anchor) && !problem_keys.contains(anchor) {
                                    errors.push(DocCheckError::new_error(
                                        doc_line.line_num,
                                        doc_line.file_name.clone(),
                                        &format!(
                                            "Invalid anchor link: {} (does not match any tool or problem)",
                                            dest_url
                                        ),
                                    ));
                                }
                            } else if let Some(err) = check_path(
                                &doc_line,
                                &self.root_dir,
                                &self.docs_folder,
                                &self.project,
                                &dest_url,
                                self.allow_fuchsia_src_links,
                                &self.reference_prefix,
                            ) {
                                errors.push(err);
                            }
                        }
                        _ => {}
                    },
                    _ => {}
                }
            }
        }

        errors.extend(callback_errors);
        errors
    }

    fn collect_reachability_roots(
        &self,
        visited: &HashMap<PathBuf, IncludedYaml>,
        markdown_files: &[PathBuf],
    ) -> HashSet<PathBuf> {
        let mut reachable: HashSet<PathBuf> = visited
            .keys()
            .filter(|path| path.extension().map_or(false, |ext| ext == "md"))
            .cloned()
            .collect();
        reachable.insert(self.root_dir.join("CODE_OF_CONDUCT.md"));
        reachable.insert(self.root_dir.join("CONTRIBUTING.md"));
        for f in markdown_files {
            if f.is_navbar_doc() || f.is_ignored_doc() || f.ends_with("gen/build_arguments.md") {
                reachable.insert(f.clone());
            }
        }
        reachable
    }

    fn traverse_reachability_graph(&self, reachable: &mut HashSet<PathBuf>) {
        let mut queue: Vec<PathBuf> = reachable.iter().cloned().collect();
        let graph = self.reachability_graph.lock().unwrap();
        while let Some(current) = queue.pop() {
            if let Some(neighbors) = graph.get(&current) {
                for neighbor in neighbors {
                    if reachable.insert(neighbor.clone()) {
                        queue.push(neighbor.clone());
                    }
                }
            }
        }
    }

    fn check_file_reachability(
        &self,
        markdown_files: &[PathBuf],
        reachable: &HashSet<PathBuf>,
        errors: &mut Vec<DocCheckError>,
    ) {
        let exemptions = self.exemption_set.lock().unwrap();
        for f in markdown_files {
            if reachable.contains(f) {
                if exemptions.contains(f) {
                    errors.push(DocCheckError::new_warning(
                        0,
                        f.clone(),
                        "Redundant exemption comment. File is reachable locally; remove '<!-- doc-checker: ignore-unused -->' to clean up.",
                    ));
                }
            } else {
                let is_hidden =
                    f.is_hidden_doc(&self.root_dir, self.reference_docs_root.as_deref());
                if is_hidden {
                    if !self.allow_unreferenced_hidden && !exemptions.contains(f) {
                        errors.push(DocCheckError::new_error(
                            0,
                            f.clone(),
                            "Unused hidden file. If this file is only referenced by external repositories, append '<!-- doc-checker: ignore-unused -->' to the file to exempt it.",
                        ));
                    }
                } else {
                    errors.push(DocCheckError::new_error(
                        0,
                        f.clone(),
                        "File not referenced in any _toc.yaml files.",
                    ));
                }
            }
        }
    }
}

#[async_trait]
impl DocYamlCheck for YamlChecker {
    fn name(&self) -> &str {
        "DocYamlCheck"
    }

    fn check<'a>(
        &mut self,
        filename: &Path,
        yaml_value: &serde_yaml::Value,
    ) -> Result<Option<Vec<DocCheckError>>> {
        if let Some(yaml_name) = filename.file_name() {
            let result = match yaml_name.to_str() {
                Some("_all_drivers_doc.yaml") => check_all_drivers_doc(filename, yaml_value),
                Some("_areas.yaml") | Some("_rfc_areas.yaml") => check_areas(filename, yaml_value),
                Some("_deprecated-docs.yaml") => check_deprecated_docs(
                    &self.root_dir,
                    &self.docs_folder,
                    &self.project,
                    filename,
                    yaml_value,
                    self.allow_fuchsia_src_links,
                    &self.reference_prefix,
                ),
                Some("_drivers_areas.yaml") => check_drivers_areas(filename, yaml_value),
                Some("_drivers_epitaphs.yaml") => check_drivers_epitaphs(filename, yaml_value),
                Some("_eng_council.yaml") => check_eng_council(filename, yaml_value),
                Some("_glossary.yaml") => check_glossary(filename, yaml_value),
                Some("_metadata.yaml") => check_metadata(
                    &self.root_dir,
                    &self.docs_folder,
                    &self.project,
                    filename,
                    yaml_value,
                    self.allow_fuchsia_src_links,
                    &mut self.external_links,
                ),
                Some("_problems.yaml") => {
                    let (entries, errors) = check_problems(filename, yaml_value);
                    if let Some(entries) = entries {
                        self.problems
                            .extend(entries.into_iter().map(|e| (filename.to_path_buf(), e)));
                    }
                    errors
                }
                Some("_redirects.yaml") => check_redirects(
                    &self.root_dir,
                    &self.docs_folder,
                    &self.project,
                    filename,
                    yaml_value,
                    self.allow_fuchsia_src_links,
                    &self.reference_prefix,
                ),
                Some("_rfcs.yaml") => check_rfcs(filename, yaml_value),
                Some("_roadmap.yaml") => check_roadmap(
                    &self.root_dir,
                    &self.docs_folder,
                    &self.project,
                    filename,
                    yaml_value,
                    self.allow_fuchsia_src_links,
                    &mut self.external_links,
                ),
                Some("_supported_cpu_architecture.yaml") => {
                    check_supported_cpu_architecture(filename, yaml_value)
                }
                Some("_supported_sys_config.yaml") => check_supported_sys_config(
                    &self.root_dir,
                    filename,
                    yaml_value,
                    &mut self.external_links,
                ),
                Some("_toc.yaml") => toc_checker::check_toc(
                    &self.root_dir,
                    &self.docs_folder,
                    &self.project,
                    filename,
                    yaml_value,
                    self.allow_fuchsia_src_links,
                    &self.reference_prefix,
                ),
                Some("_tools.yaml") => {
                    let (entries, errors) =
                        check_tools(&self.root_dir, filename, yaml_value, &mut self.external_links);
                    if let Some(entries) = entries {
                        for entry in entries {
                            self.tools.insert(entry.name.clone());
                            self.tools_entries.push((filename.to_path_buf(), entry));
                        }
                    }
                    errors
                }
                Some(name) => todo!("Need to handle {} ({:?})", name, filename),
                _ => panic!("No str avail for {:?}", filename),
            };
            Ok(result)
        } else {
            Ok(None)
        }
    }

    async fn post_check(
        &self,
        markdown_files: &[PathBuf],
        yaml_files: &[PathBuf],
    ) -> Result<Option<Vec<DocCheckError>>> {
        let mut yaml_file_set: HashSet<&PathBuf> = HashSet::from_iter(yaml_files.iter());
        let mut visited: HashMap<PathBuf, IncludedYaml> = HashMap::new();
        let mut markdown_file_set: HashSet<&PathBuf> = HashSet::from_iter(markdown_files.iter());
        let mut errors = vec![];
        let mut external_links = self.external_links.clone();

        // Some special paths that are not in the //docs dir that need to be added
        let code_of_conduct_md = self.root_dir.join("CODE_OF_CONDUCT.md");
        markdown_file_set.insert(&code_of_conduct_md);
        let contrib_md = self.root_dir.join("CONTRIBUTING.md");
        markdown_file_set.insert(&contrib_md);

        // Start with //docs/_toc.yaml
        let mut toc_stack: Vec<IncludedYaml> = vec![IncludedYaml {
            container: self.root_dir.join(&self.docs_folder).join("_toc.yaml"),
            included_file: self.root_dir.join(&self.docs_folder).join("_toc.yaml"),
        }];
        while let Some(current_yaml) = toc_stack.pop() {
            if let Some(yaml_doc) = yaml_file_set.take(&current_yaml.included_file) {
                visited.insert(yaml_doc.clone(), current_yaml.clone());
                let toc = Toc::from(yaml_doc)?;
                // remove paths to markdown
                if let Some(path_list) = toc.get_paths() {
                    for p in path_list {
                        if is_external_path(&p) {
                            if let Some(reference_root) = &self.reference_docs_root {
                                if p.starts_with("/reference") {
                                    let rel_path =
                                        p.strip_prefix("/reference/").unwrap_or(p.as_str());
                                    let mut file_path = reference_root.join(rel_path);
                                    if path_helper::is_dir(&file_path) {
                                        file_path.push("README.md");
                                    }
                                    let file_path = normalize_path(&file_path).unwrap_or(file_path);

                                    if file_path.is_hidden_doc(
                                        &self.root_dir,
                                        self.reference_docs_root.as_deref(),
                                    ) {
                                        errors.push(DocCheckError::new_error(
                                            0,
                                            yaml_doc.clone(),
                                            &format!("Cannot reference hidden file {} in _toc.yaml. Hidden files are not published by Devsite.", p),
                                        ));
                                    }

                                    if markdown_file_set.take(&file_path).is_none()
                                        && !visited.contains_key(&file_path)
                                    {
                                        errors.push(DocCheckError::new_error(
                                            0,
                                            yaml_doc.clone(),
                                            &format!("Reference to missing file: {}", p),
                                        ));
                                    } else {
                                        visited.insert(file_path, current_yaml.clone());
                                    }
                                }
                            }
                            if self.check_external_links {
                                if p.starts_with("/reference") {
                                    external_links.push(LinkReference {
                                        link: format!("https://{}{}", PUBLISHED_DOCS_HOST, p),
                                        location: DocLine {
                                            line_num: 0,
                                            file_name: current_yaml.included_file.clone(),
                                        },
                                    });
                                } else if p.starts_with("https://") || p.starts_with("http://") {
                                    external_links.push(LinkReference {
                                        link: p.to_string(),
                                        location: DocLine {
                                            line_num: 0,
                                            file_name: current_yaml.included_file.clone(),
                                        },
                                    });
                                } else if p.starts_with("//") {
                                    external_links.push(LinkReference {
                                        link: format!("https:{}", p),
                                        location: DocLine {
                                            line_num: 0,
                                            file_name: current_yaml.included_file.clone(),
                                        },
                                    });
                                }
                            }
                            continue;
                        } else {
                            let rel_path = p.strip_prefix('/').unwrap_or(p.as_str());
                            let mut file_path = self.root_dir.join(rel_path);
                            if path_helper::is_dir(&file_path) {
                                file_path.push("README.md");
                            }
                            let file_path = normalize_path(&file_path).unwrap_or(file_path);

                            if file_path
                                .is_hidden_doc(&self.root_dir, self.reference_docs_root.as_deref())
                            {
                                errors.push(DocCheckError::new_error(
                                    0,
                                    yaml_doc.clone(),
                                    &format!("Cannot reference hidden file {} in _toc.yaml. Hidden files are not published by Devsite.", p),
                                ));
                            }

                            if markdown_file_set.take(&file_path).is_none()
                                && !visited.contains_key(&file_path)
                            {
                                errors.push(DocCheckError::new_error(
                                    0,
                                    yaml_doc.clone(),
                                    &format!("Reference to missing file: {}", p),
                                ));
                            } else {
                                visited.insert(file_path, current_yaml.clone());
                            }
                        }
                    }
                }
                // follow include
                if let Some(includes) = toc.get_includes() {
                    // All includes are /docs/... so just append the root.
                    let additional_paths = includes
                        .iter()
                        //Ignoring yaml included from /reference.
                        .filter(|p| !p.starts_with("/reference"))
                        .map(|p| self.root_dir.join(p.strip_prefix('/').unwrap_or(p.as_str())))
                        .filter(|p| {
                            if p == &current_yaml.included_file {
                                errors.push(DocCheckError::new_error(
                                    0,
                                    p.clone(),
                                    &format!("YAML files cannot include themselves {p:?}"),
                                ));
                                false
                            } else {
                                true
                            }
                        });
                    toc_stack.extend(
                        additional_paths.map(|f| IncludedYaml {
                            container: yaml_doc.clone(),
                            included_file: f,
                        }),
                    );
                    // if checking reference docs, add them as well
                    if let Some(reference_root) = &self.reference_docs_root {
                        let ref_additional_paths = includes
                            .iter()
                            //Only process /reference.
                            .filter(|p| p.starts_with("/reference"))
                            .map(|p| {
                                reference_root
                                    .join(p.strip_prefix("/reference/").unwrap_or(p.as_str()))
                            })
                            .filter(|p| {
                                if p == &current_yaml.included_file {
                                    errors.push(DocCheckError::new_error(
                                        0,
                                        p.clone(),
                                        &format!("YAML files cannot include themselves {p:?}"),
                                    ));
                                    false
                                } else {
                                    true
                                }
                            });
                        let more_paths: Vec<PathBuf> = ref_additional_paths.collect();
                        toc_stack.extend(more_paths.into_iter().map(|f| IncludedYaml {
                            container: yaml_doc.clone(),
                            included_file: f,
                        }));
                    }
                }
            } else if !visited.contains_key(&current_yaml.included_file) {
                errors.push(DocCheckError::new_error(
                    0,
                    current_yaml.container.clone(),
                    &format!(
                        "Cannot find file {:?} included in {:?}",
                        &current_yaml.included_file, &current_yaml.container
                    ),
                ));
            }
        }

        let mut reachable = self.collect_reachability_roots(&visited, markdown_files);
        self.traverse_reachability_graph(&mut reachable);
        self.check_file_reachability(markdown_files, &reachable, &mut errors);

        yaml_file_set.iter().filter(|f| f.ends_with("_toc.yaml")).for_each(|&f| {
            errors.push(DocCheckError::new_error(
                0,
                f.clone(),
                "File not reachable via _toc include references.",
            ))
        });

        // Collect all problem keys as Strings for O(1) lookups and link validation
        let problem_keys: HashSet<String> =
            self.problems.iter().map(|(_, p)| p.key.clone()).collect();

        self.validate_problems(&problem_keys, &mut errors);
        self.validate_tools(&problem_keys, &mut errors);

        if self.check_external_links {
            if let Some(link_errors) = check_external_links(&external_links).await {
                for e in link_errors {
                    errors.push(e);
                }
            }
        }

        if errors.is_empty() { Ok(None) } else { Ok(Some(errors)) }
    }
}

fn is_external_path(p: &str) -> bool {
    // treat reference docs as external
    p.starts_with("/reference")
        || p.starts_with("https://")
        || p.starts_with("http://")
        || p.starts_with("//")
}

/// Checks the path property from a yaml file.
/// Returns `Some(DocCheckError)` if validation fails, or `None` if the path is valid.
// TODO: Convert check_path to return Result<(), DocCheckError> for consistency with other checker helpers.
fn check_path(
    doc_line: &DocLine,
    root_path: &Path,
    docs_folder: &Path,
    project: &str,
    path: &str,
    allow_fuchsia_src_links: bool,
    reference_prefix: &Path,
) -> Option<DocCheckError> {
    let root_dir = root_path.display().to_string();
    match do_check_link(doc_line, path, project, allow_fuchsia_src_links) {
        Ok(Some(doc_error)) => return Some(doc_error),
        Err(e) => {
            return Some(DocCheckError::new_error(
                doc_line.line_num,
                doc_line.file_name.clone(),
                &e.to_string(),
            ));
        }
        Ok(None) => {}
    };

    // These files are in the root of the project, not in the docs directory, so they need special
    // treatment.
    if ["/CONTRIBUTING.md", "/CODE_OF_CONDUCT.md"].contains(&path) {
        let filepath = root_path.join(path.strip_prefix('/').unwrap_or(path));
        if !path_helper::exists(&filepath) {
            return Some(DocCheckError::new_error(
                doc_line.line_num,
                doc_line.file_name.clone(),
                &format!("File: {:?} not found.", filepath),
            ));
        }
        return None;
    }

    match is_intree_link(project, &root_dir, docs_folder, path) {
        Ok(Some(in_tree_path)) => {
            // Handle in-tree paths that are not in the docs_folder.
            // Since this is a table of contents, all the entries need
            // to be to the docs_folder, except /reference, which is a special case.
            if !in_tree_path.starts_with(PathBuf::from("/").join(docs_folder)) {
                if in_tree_path.starts_with(reference_prefix) {
                    None
                } else {
                    Some(DocCheckError::new_error(
                        doc_line.line_num,
                        doc_line.file_name.clone(),
                        &format!(
                            "in-tree link to {} must be in '{}' or '{}' directory",
                            path,
                            PathBuf::from("/").join(docs_folder).display(),
                            reference_prefix.display()
                        ),
                    ))
                }
            } else {
                do_in_tree_check(doc_line, root_path, docs_folder, path, &in_tree_path).err()
            }
        }
        // Accept external links.
        Ok(None) if is_external_path(path) => None,
        Ok(None) => Some(DocCheckError::new_error(
            doc_line.line_num,
            doc_line.file_name.clone(),
            &format!("invalid path {}", path),
        )),
        Err(e) => Some(DocCheckError::new_error(
            doc_line.line_num,
            doc_line.file_name.clone(),
            &format!("Error checking path {}: {}", path, e),
        )),
    }
}

/// Helper function to validate a single deprecation entry.
/// Returns a list of validation errors found in the entry.
fn validate_deprecation_entry(
    entry: &FromTo,
    root_dir: &Path,
    docs_folder: &Path,
    project: &str,
    filename: &Path,
    allow_fuchsia_src_links: bool,
    docs_prefix: &Path,
    reference_prefix: &Path,
) -> Vec<DocCheckError> {
    let mut errors = vec![];
    let doc_line = DocLine { line_num: 1, file_name: filename.to_path_buf() };

    // Validate that the 'from' path is not empty.
    if entry.from.is_empty() {
        errors.push(DocCheckError::new_error(
            doc_line.line_num,
            doc_line.file_name.clone(),
            "Deprecation 'from' path cannot be empty",
        ));
        return errors;
    }

    // Validate that the 'from' path starts with '/' and is under allowed folders.
    if !entry.from.starts_with('/') {
        errors.push(DocCheckError::new_error(
            doc_line.line_num,
            doc_line.file_name.clone(),
            &format!("Deprecation 'from' path must start with '/': {}", entry.from),
        ));
    } else {
        let path_buf = PathBuf::from(&entry.from);
        if !path_buf.starts_with(docs_prefix) && !path_buf.starts_with(reference_prefix) {
            errors.push(DocCheckError::new_error(
                doc_line.line_num,
                doc_line.file_name.clone(),
                &format!(
                    "Deprecation 'from' path must start with '{}' or '{}': {}",
                    PathBuf::from("/").join(docs_folder).display(),
                    reference_prefix.display(),
                    entry.from
                ),
            ));
        }
    }

    // Validate that the 'from' path does not contain wildcards, as they are not supported.
    if entry.from.contains('*') || entry.from.contains('?') {
        errors.push(DocCheckError::new_error(
            doc_line.line_num,
            doc_line.file_name.clone(),
            &format!("Wildcards are not supported in deprecation 'from' path: {}", entry.from),
        ));
    }

    // Validate that the target 'to' document exists (using the general check_path helper).
    if let Some(err) = check_path(
        &doc_line,
        root_dir,
        docs_folder,
        project,
        &entry.to,
        allow_fuchsia_src_links,
        reference_prefix,
    ) {
        errors.push(err);
    }

    errors
}

/// Checks that all entries in `_deprecated-docs.yaml` have valid source path formatting
/// and point to a destination document that exists.
fn check_deprecated_docs(
    root_dir: &Path,
    docs_folder: &Path,
    project: &str,
    filename: &Path,
    yaml_value: &Value,
    allow_fuchsia_src_links: bool,
    reference_prefix: &Path,
) -> Option<Vec<DocCheckError>> {
    let result = serde_yaml::from_value::<Deprecations>(yaml_value.clone());
    match result {
        Ok(deprecations) => {
            // Allocate the prefix paths once outside the loop.
            let docs_prefix = PathBuf::from("/").join(docs_folder);

            let errors: Vec<DocCheckError> = deprecations
                .included
                .iter()
                .flat_map(|entry| {
                    validate_deprecation_entry(
                        entry,
                        root_dir,
                        docs_folder,
                        project,
                        filename,
                        allow_fuchsia_src_links,
                        &docs_prefix,
                        reference_prefix,
                    )
                })
                .collect();

            if errors.is_empty() { None } else { Some(errors) }
        }
        Err(e) => Some(vec![DocCheckError::new_error(
            1,
            filename.to_path_buf(),
            &format!("invalid structure {}", e),
        )]),
    }
}

fn check_redirects(
    root_dir: &Path,
    docs_folder: &Path,
    project: &str,
    filename: &Path,
    yaml_value: &Value,
    allow_fuchsia_src_links: bool,
    reference_prefix: &Path,
) -> Option<Vec<DocCheckError>> {
    let result = serde_yaml::from_value::<Redirects>(yaml_value.clone());
    match result {
        Ok(redirects) => {
            let mut errors = vec![];
            let doc_line = DocLine { line_num: 1, file_name: filename.to_path_buf() };
            for r in redirects.redirects.unwrap_or_default() {
                // Ignore wildcards ending with "..." as they are likely supported by devsite
                // but fail file existence checks.
                if r.to.ends_with("...") {
                    let parts: Vec<&str> = r.to.split("...").collect();
                    let dir_part = parts[0].trim_end_matches('/');
                    if !dir_part.is_empty() {
                        let root_dir_str = root_dir.display().to_string();
                        if let Ok(Some(in_tree_path)) =
                            is_intree_link(project, &root_dir_str, docs_folder, dir_part)
                        {
                            let abs_path = root_dir.join(
                                in_tree_path.strip_prefix("/").unwrap_or_else(|_| &in_tree_path),
                            );
                            if !path_helper::exists(&abs_path) || !path_helper::is_dir(&abs_path) {
                                errors.push(DocCheckError::new_error(
                                    doc_line.line_num,
                                    doc_line.file_name.clone(),
                                    &format!(
                                        "Directory: {:?} not found for wildcard redirect.",
                                        abs_path
                                    ),
                                ));
                            }
                        }
                    }
                } else if let Some(e) = check_path(
                    &doc_line,
                    root_dir,
                    docs_folder,
                    project,
                    &r.to,
                    allow_fuchsia_src_links,
                    reference_prefix,
                ) {
                    errors.push(e);
                }
            }
            if errors.is_empty() { None } else { Some(errors) }
        }
        Err(e) => Some(vec![DocCheckError::new_error(
            1,
            filename.to_path_buf(),
            &format!("invalid structure {}", e),
        )]),
    }
}

fn check_areas(filename: &Path, yaml_value: &Value) -> Option<Vec<DocCheckError>> {
    //TODO(https://fxbug.dev/42064921): Align _areas.yaml on same schema.
    if filename.ends_with("contribute/governance/areas/_areas.yaml") {
        let (items, errors) = parse_entries::<AreaEntry>(filename, yaml_value);
        let mut errs = errors.unwrap_or_default();
        if let Some(entries) = items {
            let mut last_name: Option<String> = None;
            let mut seen_names = std::collections::HashSet::new();
            for entry in entries {
                if entry.name.is_empty() {
                    errs.push(DocCheckError::new_error(
                        1,
                        filename.to_path_buf(),
                        "area name cannot be empty",
                    ));
                    continue;
                }
                if !seen_names.insert(entry.name.to_lowercase()) {
                    errs.push(DocCheckError::new_error(
                        1,
                        filename.to_path_buf(),
                        &format!("duplicate area name: {}", entry.name),
                    ));
                }
                if let Some(ref last) = last_name {
                    if last.to_lowercase() > entry.name.to_lowercase() {
                        errs.push(DocCheckError::new_error(
                            1,
                            filename.to_path_buf(),
                            &format!(
                                "areas are not alphabetically sorted: '{}' should be before '{}'",
                                entry.name, last
                            ),
                        ));
                    }
                }
                last_name = Some(entry.name.clone());

                if !entry.api_primary.is_empty() && !entry.api_primary.contains('@') {
                    errs.push(DocCheckError::new_error(
                        1,
                        filename.to_path_buf(),
                        &format!(
                            "invalid api_primary email: {} for area {}",
                            entry.api_primary, entry.name
                        ),
                    ));
                }
                if !entry.api_secondary.is_empty() && !entry.api_secondary.contains('@') {
                    errs.push(DocCheckError::new_error(
                        1,
                        filename.to_path_buf(),
                        &format!(
                            "invalid api_secondary email: {} for area {}",
                            entry.api_secondary, entry.name
                        ),
                    ));
                }
            }
        }
        if errs.is_empty() { None } else { Some(errs) }
    } else {
        let (items, errors) = parse_entries::<String>(filename, yaml_value);
        let mut errs = errors.unwrap_or_default();
        if let Some(entries) = items {
            let mut last_name: Option<String> = None;
            let mut seen_names = std::collections::HashSet::new();
            for entry in entries {
                if entry.is_empty() {
                    errs.push(DocCheckError::new_error(
                        1,
                        filename.to_path_buf(),
                        "area name cannot be empty",
                    ));
                    continue;
                }
                if !seen_names.insert(entry.to_lowercase()) {
                    errs.push(DocCheckError::new_error(
                        1,
                        filename.to_path_buf(),
                        &format!("duplicate area name: {}", entry),
                    ));
                }
                if let Some(ref last) = last_name {
                    if last.to_lowercase() > entry.to_lowercase() {
                        errs.push(DocCheckError::new_error(
                            1,
                            filename.to_path_buf(),
                            &format!(
                                "areas are not alphabetically sorted: '{}' should be before '{}'",
                                entry, last
                            ),
                        ));
                    }
                }
                last_name = Some(entry.clone());
            }
        }
        if errs.is_empty() { None } else { Some(errs) }
    }
}

fn check_all_drivers_doc(filename: &Path, yaml_value: &Value) -> Option<Vec<DocCheckError>> {
    let result = serde_yaml::from_value::<AllDrivers>(yaml_value.clone());
    //TODO(https://fxbug.dev/349902231): Add a check that the to: doc exists.
    match result {
        Ok(_) => None,
        Err(e) => Some(vec![DocCheckError::new_error(
            1,
            filename.to_path_buf(),
            &format!("invalid structure {}", e),
        )]),
    }
}

fn check_drivers_areas(filename: &Path, yaml_value: &Value) -> Option<Vec<DocCheckError>> {
    let result = serde_yaml::from_value::<Vec<String>>(yaml_value.clone());
    //TODO(https://fxbug.dev/42064921): Align on common _areas.yaml structure
    match result {
        Ok(_redirects) => None,
        Err(e) => Some(vec![DocCheckError::new_error(
            1,
            filename.into(),
            &format!("invalid structure for _drivers_areas {}. Data: {:?}", e, yaml_value),
        )]),
    }
}

static VALID_DRIVER_AREAS: std::sync::LazyLock<Vec<String>> = std::sync::LazyLock::new(|| {
    let s = include_str!("../../../docs/reference/hardware/_drivers_areas.yaml");
    serde_yaml::from_str(s).expect("Failed to parse driver areas")
});

// Matches the `href` attribute in HTML anchor tags (e.g., <a href="...">).
static HREF_REGEX: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
    Regex::new(r#"href="([^"]+)""#).expect("Failed to compile HREF regex")
});

fn check_drivers_epitaphs(filename: &Path, yaml_value: &Value) -> Option<Vec<DocCheckError>> {
    let (items, errors) = parse_entries::<DriverEpitaph>(filename, yaml_value);
    let mut errs = errors.unwrap_or_default();
    if let Some(epitaphs) = items {
        for epitaph in epitaphs {
            // Validate gerrit_change_id
            if epitaph.gerrit_change_id != "TBD"
                && !epitaph.gerrit_change_id.chars().all(|c| c.is_ascii_digit())
            {
                errs.push(DocCheckError::new_error(
                    1,
                    filename.to_path_buf(),
                    &format!(
                        "invalid gerrit_change_id: {}. Must be a number or 'TBD'",
                        epitaph.gerrit_change_id
                    ),
                ));
            }
            // Validate available_in_git (git hash)
            let len = epitaph.available_in_git.len();
            if (len != 40 && len != 41)
                || !epitaph.available_in_git.chars().all(|c| c.is_ascii_hexdigit())
            {
                errs.push(DocCheckError::new_error(
                    1,
                    filename.to_path_buf(),
                    &format!(
                        "invalid available_in_git hash: {}. Must be a 40 or 41-character hex string",
                        epitaph.available_in_git
                    ),
                ));
            }
            // Validate areas
            if let Some(areas) = &epitaph.areas {
                for area in areas {
                    if !VALID_DRIVER_AREAS.contains(area) {
                        errs.push(DocCheckError::new_error(
                            1,
                            filename.to_path_buf(),
                            &format!(
                                "invalid driver area: {}. Must be one of {:?}",
                                area, *VALID_DRIVER_AREAS
                            ),
                        ));
                    }
                }
            }
            // Validate path flexibly across legacy formats
            let clean_path = epitaph.path.trim_start_matches('/');
            if !clean_path.starts_with("src/")
                && !clean_path.starts_with("zircon/")
                && !clean_path.starts_with("examples/")
            {
                errs.push(DocCheckError::new_error(
                    1,
                    filename.to_path_buf(),
                    &format!(
                        "path {} must start with '/src/', '/zircon/', or '/examples/'",
                        epitaph.path
                    ),
                ));
            }
            // Validate areas
            if let Some(areas) = &epitaph.areas {
                if areas.is_empty() {
                    errs.push(DocCheckError::new_error(
                        1,
                        filename.to_path_buf(),
                        "areas list cannot be empty if provided",
                    ));
                }
            }
        }
    }
    if errs.is_empty() { None } else { Some(errs) }
}

fn check_eng_council(filename: &Path, yaml_value: &Value) -> Option<Vec<DocCheckError>> {
    let result = serde_yaml::from_value::<EngCouncil>(yaml_value.clone());
    match result {
        Ok(_redirects) => None,
        Err(e) => Some(vec![DocCheckError::new_error(
            1,
            filename.to_path_buf(),
            &format!("invalid structure for EngCouncil {}. Found {:?}", e, yaml_value),
        )]),
    }
}

fn check_glossary(filename: &Path, yaml_value: &Value) -> Option<Vec<DocCheckError>> {
    let (_items, errors) = parse_entries::<GlossaryTerm>(filename, yaml_value);
    //TODO(https://fxbug.dev/42064926): other checks for GlossaryTerm?
    errors
}

fn normalize_external_link(p: &str) -> String {
    if p.starts_with("/reference") {
        format!("https://{}{}", PUBLISHED_DOCS_HOST, p)
    } else if p.starts_with("//") {
        format!("https:{}", p)
    } else {
        p.to_string()
    }
}

fn check_metadata(
    root_dir: &Path,
    docs_folder: &Path,
    project: &str,
    filename: &Path,
    yaml_value: &Value,
    allow_fuchsia_src_links: bool,
    external_links: &mut Vec<LinkReference>,
) -> Option<Vec<DocCheckError>> {
    let result = serde_yaml::from_value::<Metadata>(yaml_value.clone());
    match result {
        Ok(metadata) => {
            let mut errors = vec![];
            let doc_line = DocLine { line_num: 1, file_name: filename.to_path_buf() };
            for guide in metadata.guides {
                if !metadata.types.contains(&guide.entry_type) {
                    errors.push(DocCheckError::new_error(
                        1,
                        filename.to_path_buf(),
                        &format!(
                            "invalid type '{}' in guide '{}'. Must be one of {:?}",
                            guide.entry_type, guide.title, metadata.types
                        ),
                    ));
                }
                if !metadata.products.contains(&guide.product) {
                    errors.push(DocCheckError::new_error(
                        1,
                        filename.to_path_buf(),
                        &format!(
                            "invalid product '{}' in guide '{}'. Must be one of {:?}",
                            guide.product, guide.title, metadata.products
                        ),
                    ));
                }
                if !metadata.boards.contains(&guide.board) {
                    errors.push(DocCheckError::new_error(
                        1,
                        filename.to_path_buf(),
                        &format!(
                            "invalid board '{}' in guide '{}'. Must be one of {:?}",
                            guide.board, guide.title, metadata.boards
                        ),
                    ));
                }
                if !metadata.methods.contains(&guide.method) {
                    errors.push(DocCheckError::new_error(
                        1,
                        filename.to_path_buf(),
                        &format!(
                            "invalid method '{}' in guide '{}'. Must be one of {:?}",
                            guide.method, guide.title, metadata.methods
                        ),
                    ));
                }
                if !metadata.hosts.contains(&guide.host) {
                    errors.push(DocCheckError::new_error(
                        1,
                        filename.to_path_buf(),
                        &format!(
                            "invalid host '{}' in guide '{}'. Must be one of {:?}",
                            guide.host, guide.title, metadata.hosts
                        ),
                    ));
                }

                // Link validation
                match do_check_link(&doc_line, &guide.url, project, allow_fuchsia_src_links) {
                    Ok(Some(err)) => {
                        errors.push(err);
                    }
                    Ok(None) => {
                        let root_dir_str = root_dir.display().to_string();
                        match is_intree_link(project, &root_dir_str, docs_folder, &guide.url) {
                            Ok(Some(in_tree_path)) => {
                                if let Err(err) = do_in_tree_check(
                                    &doc_line,
                                    root_dir,
                                    docs_folder,
                                    &guide.url,
                                    &in_tree_path,
                                ) {
                                    errors.push(err);
                                }
                            }
                            Ok(None) if is_external_path(&guide.url) => {
                                external_links.push(LinkReference {
                                    link: normalize_external_link(&guide.url),
                                    location: doc_line.clone(),
                                });
                            }
                            Ok(None) => {
                                errors.push(DocCheckError::new_error(
                                    doc_line.line_num,
                                    doc_line.file_name.clone(),
                                    &format!("invalid path {}", guide.url),
                                ));
                            }
                            Err(e) => {
                                errors.push(DocCheckError::new_error(
                                    doc_line.line_num,
                                    doc_line.file_name.clone(),
                                    &format!("Error checking path {}: {}", guide.url, e),
                                ));
                            }
                        }
                    }
                    Err(e) => {
                        errors.push(DocCheckError::new_error(
                            doc_line.line_num,
                            doc_line.file_name.clone(),
                            &e.to_string(),
                        ));
                    }
                }
            }
            if errors.is_empty() { None } else { Some(errors) }
        }
        Err(e) => Some(vec![DocCheckError::new_error(
            1,
            filename.to_path_buf(),
            &format!("invalid structure for _metadata {}. Data: {:?}", e, yaml_value),
        )]),
    }
}

fn check_problems(
    filename: &Path,
    yaml_value: &Value,
) -> (Option<Vec<ProblemEntry>>, Option<Vec<DocCheckError>>) {
    let (items, errors) = parse_entries::<ProblemEntry>(filename, yaml_value);
    let mut errs = errors.unwrap_or_default();
    if let Some(ref problems) = items {
        for problem in problems {
            if problem.key.trim().is_empty() {
                errs.push(DocCheckError::new_error(
                    1,
                    filename.to_path_buf(),
                    "problem key cannot be empty",
                ));
            }
            if problem.use_case.trim().is_empty() {
                errs.push(DocCheckError::new_error(
                    1,
                    filename.to_path_buf(),
                    &format!("use_case for problem '{}' cannot be empty", problem.key),
                ));
            }
            if problem.description.trim().is_empty() {
                errs.push(DocCheckError::new_error(
                    1,
                    filename.to_path_buf(),
                    &format!("description for problem '{}' cannot be empty", problem.key),
                ));
            }
        }
    }
    let ret_errors = if errs.is_empty() { None } else { Some(errs) };
    (items, ret_errors)
}

fn check_rfcs(filename: &Path, yaml_value: &Value) -> Option<Vec<DocCheckError>> {
    let (items, errors) = parse_entries::<RfcEntry>(filename, yaml_value);
    let mut errs = errors.unwrap_or_default();
    if let Some(rfcs) = items {
        let valid_statuses = vec![
            "Accepted",
            "Rejected",
            "Template",
            "Withdrawn",
            "Draft",
            "Obsolete",
            "Superseded",
            "Socialization",
        ];
        let mut seen_names = std::collections::HashSet::new();
        for rfc in rfcs {
            // Validate uniqueness of names
            if !seen_names.insert(rfc.name.clone()) {
                errs.push(DocCheckError::new_error(
                    1,
                    filename.to_path_buf(),
                    &format!("duplicate RFC name {}", rfc.name),
                ));
            }
            // Validate short_description is not empty
            if rfc.short_description.trim().is_empty() {
                errs.push(DocCheckError::new_error(
                    1,
                    filename.to_path_buf(),
                    "invalid RFC short_description. Must not be empty",
                ));
            }
            // Validate non-emptiness of authors/reviewers/area lists
            if rfc.authors.is_empty() && rfc.status != "Template" {
                errs.push(DocCheckError::new_error(
                    1,
                    filename.to_path_buf(),
                    "authors list must contain at least 1 entry",
                ));
            }
            if rfc.reviewers.is_empty() && rfc.status != "Template" {
                errs.push(DocCheckError::new_error(
                    1,
                    filename.to_path_buf(),
                    "reviewers list must contain at least 1 entry",
                ));
            }
            // Validate area
            if rfc.area.is_empty() {
                errs.push(DocCheckError::new_error(
                    1,
                    filename.to_path_buf(),
                    "area list must contain at least 1 entry",
                ));
            }

            // Validate name
            if !rfc.name.starts_with("RFC-") {
                errs.push(DocCheckError::new_error(
                    1,
                    filename.to_path_buf(),
                    &format!("invalid RFC name {}. Must start with 'RFC-'", rfc.name),
                ));
            }
            // Validate title
            if rfc.title.trim().is_empty() {
                errs.push(DocCheckError::new_error(
                    1,
                    filename.to_path_buf(),
                    "invalid RFC title. Must not be empty",
                ));
            }
            // Validate status
            if !valid_statuses.contains(&rfc.status.as_str()) {
                errs.push(DocCheckError::new_error(
                    1,
                    filename.to_path_buf(),
                    &format!("invalid RFC status {}", rfc.status),
                ));
            }
            // Validate dates if provided
            let mut validate_date = |d: &str| {
                if !d.is_empty() {
                    if d.len() != 10
                        || d.chars().nth(4) != Some('-')
                        || d.chars().nth(7) != Some('-')
                        || d.chars().filter(|c| c.is_ascii_digit()).count() != 8
                    {
                        errs.push(DocCheckError::new_error(
                            1,
                            filename.to_path_buf(),
                            &format!("invalid date format {}. Must be YYYY-MM-DD", d),
                        ));
                    }
                }
            };
            validate_date(&rfc.submitted);
            validate_date(&rfc.reviewed);
            // Validate gerrit change IDs are numeric
            for id in &rfc.gerrit_change_id {
                if !id.is_empty() && !id.chars().all(|c| c.is_ascii_digit()) {
                    errs.push(DocCheckError::new_error(
                        1,
                        filename.to_path_buf(),
                        &format!("invalid gerrit_change_id format {}. Must be numeric", id),
                    ));
                }
            }
            // Check that 'file' exists relative to this file's folder.
            let rfc_file_path = filename.parent().unwrap().join(&rfc.file);
            if !path_helper::exists(&rfc_file_path) {
                errs.push(DocCheckError::new_error(
                    1,
                    filename.to_path_buf(),
                    &format!("RFC file {} does not exist", rfc.file),
                ));
            }
        }
    }
    if errs.is_empty() { None } else { Some(errs) }
}

fn check_roadmap(
    root_dir: &Path,
    docs_folder: &Path,
    project: &str,
    filename: &Path,
    yaml_value: &Value,
    allow_fuchsia_src_links: bool,
    external_links: &mut Vec<LinkReference>,
) -> Option<Vec<DocCheckError>> {
    let (items, errors) = parse_entries::<RoadmapEntry>(filename, yaml_value);
    let mut errs = errors.unwrap_or_default();
    if let Some(entries) = items {
        for entry in entries {
            if entry.workstream.is_empty() {
                errs.push(DocCheckError::new_error(
                    1,
                    filename.to_path_buf(),
                    "workstream cannot be empty",
                ));
            }
            if entry.area.is_empty() {
                errs.push(DocCheckError::new_error(
                    1,
                    filename.to_path_buf(),
                    "area cannot be empty",
                ));
            }

            for cap in HREF_REGEX.captures_iter(&entry.workstream) {
                let link = &cap[1];
                let doc_line = DocLine { line_num: 1, file_name: filename.to_path_buf() };
                match do_check_link(&doc_line, link, project, allow_fuchsia_src_links) {
                    Ok(Some(err)) => errs.push(err),
                    Ok(None) => {
                        let root_dir_str = root_dir.display().to_string();
                        match is_intree_link(project, &root_dir_str, docs_folder, link) {
                            Ok(Some(in_tree_path)) => {
                                if let Err(err) = do_in_tree_check(
                                    &doc_line,
                                    root_dir,
                                    docs_folder,
                                    link,
                                    &in_tree_path,
                                ) {
                                    errs.push(err);
                                }
                            }
                            Ok(None) if is_external_path(link) => {
                                external_links.push(LinkReference {
                                    link: normalize_external_link(link),
                                    location: doc_line.clone(),
                                });
                            }
                            Ok(None) => {
                                errs.push(DocCheckError::new_error(
                                    doc_line.line_num,
                                    doc_line.file_name.clone(),
                                    &format!("invalid path {}", link),
                                ));
                            }
                            Err(e) => errs.push(DocCheckError::new_error(
                                1,
                                filename.to_path_buf(),
                                &format!("Error checking in-tree link {}: {}", link, e),
                            )),
                        }
                    }
                    Err(e) => errs.push(DocCheckError::new_error(
                        1,
                        filename.to_path_buf(),
                        &format!("Error parsing link {}: {}", link, e),
                    )),
                }
            }

            if let Some(bugs) = entry.bug {
                for bug in bugs {
                    if !bug.starts_with("https://fxbug.dev/") {
                        errs.push(DocCheckError::new_error(
                            1,
                            filename.to_path_buf(),
                            &format!(
                                "invalid bug link: {}. Must start with 'https://fxbug.dev/'",
                                bug
                            ),
                        ));
                    }
                }
            }
        }
    }
    if errs.is_empty() { None } else { Some(errs) }
}

fn check_supported_cpu_architecture(
    filename: &Path,
    yaml_value: &Value,
) -> Option<Vec<DocCheckError>> {
    let result = serde_yaml::from_value::<Vec<String>>(yaml_value.clone());
    match result {
        Ok(architectures) => {
            let mut errors = Vec::new();
            for arch in architectures {
                if !VALID_CPU_ARCHS.contains(&arch) {
                    errors.push(DocCheckError::new_error(
                        1,
                        filename.to_path_buf(),
                        &format!("invalid CPU architecture: {}", arch),
                    ));
                }
            }
            if errors.is_empty() { None } else { Some(errors) }
        }
        Err(e) => Some(vec![DocCheckError::new_error(
            1,
            filename.to_path_buf(),
            &format!(
                "invalid structure for _supported_cpu_architecture {}. Data: {:?}",
                e, yaml_value
            ),
        )]),
    }
}

static VALID_CPU_ARCHS: std::sync::LazyLock<Vec<String>> = std::sync::LazyLock::new(|| {
    let s = include_str!("../../../docs/reference/hardware/_supported_cpu_architecture.yaml");
    serde_yaml::from_str(s).expect("Failed to parse supported CPU architectures")
});

fn check_supported_sys_config(
    root_dir: &Path,
    filename: &Path,
    yaml_value: &Value,
    external_links: &mut Vec<LinkReference>,
) -> Option<Vec<DocCheckError>> {
    let (items, errors) = parse_entries::<SysConfigEntry>(filename, yaml_value);
    let mut errs = errors.unwrap_or_default();
    if let Some(configs) = items {
        let mut seen_names = std::collections::HashSet::new();
        for config in configs {
            if config.name.trim().is_empty() {
                errs.push(DocCheckError::new_error(
                    1,
                    filename.to_path_buf(),
                    "name field must not be empty",
                ));
            } else if !seen_names.insert(config.name.clone()) {
                errs.push(DocCheckError::new_error(
                    1,
                    filename.to_path_buf(),
                    &format!("duplicate system configuration name: {}", config.name),
                ));
            }
            if config.description.trim().is_empty() {
                errs.push(DocCheckError::new_error(
                    1,
                    filename.to_path_buf(),
                    "description field must not be empty",
                ));
            }
            // Check that board_driver_location exists in repo root.
            let driver_path = root_dir.join(config.board_driver_location.trim_start_matches('/'));
            if !path_helper::exists(&driver_path) {
                errs.push(DocCheckError::new_error(
                    1,
                    filename.to_path_buf(),
                    &format!(
                        "board_driver_location {} does not exist",
                        config.board_driver_location
                    ),
                ));
            }
            // Check that manufacturer_link is a valid URL if present.
            if let Some(link) = &config.manufacturer_link {
                if link.starts_with("http://") || link.starts_with("https://") {
                    external_links.push(LinkReference {
                        link: normalize_external_link(link),
                        location: DocLine { line_num: 1, file_name: filename.to_path_buf() },
                    });
                } else {
                    errs.push(DocCheckError::new_error(
                        1,
                        filename.to_path_buf(),
                        &format!("manufacturer_link {} is not a valid URL", link),
                    ));
                }
            }
            // Validate architecture
            if !VALID_CPU_ARCHS.contains(&config.architecture) {
                errs.push(DocCheckError::new_error(
                    1,
                    filename.to_path_buf(),
                    &format!(
                        "invalid architecture: {}. Must be one of the supported architectures: {:?}",
                        config.architecture, *VALID_CPU_ARCHS
                    ),
                ));
            }
        }
    }
    if errs.is_empty() { None } else { Some(errs) }
}

fn check_tools(
    root_dir: &Path,
    filename: &Path,
    yaml_value: &Value,
    external_links: &mut Vec<LinkReference>,
) -> (Option<Vec<ToolsEntry>>, Option<Vec<DocCheckError>>) {
    let (items, errors) = parse_entries::<ToolsEntry>(filename, yaml_value);
    let mut errs = errors.unwrap_or_default();
    let mut entries = vec![];
    if let Some(entries_list) = items {
        for entry in entries_list {
            for (key, value) in &entry.links {
                if let (Some(key_str), Some(link_str)) = (key.as_str(), value.as_str()) {
                    if link_str.starts_with("/docs/") {
                        let path = root_dir.join(&link_str[1..]);
                        if !path_helper::exists(&path) {
                            errs.push(DocCheckError::new_error(
                                1,
                                filename.to_path_buf(),
                                &format!(
                                    "link {}: {} in tool {} does not exist",
                                    key_str, link_str, entry.name
                                ),
                            ));
                        }
                    } else if link_str.starts_with("http://") || link_str.starts_with("https://") {
                        external_links.push(LinkReference {
                            link: normalize_external_link(link_str),
                            location: DocLine { line_num: 1, file_name: filename.to_path_buf() },
                        });
                    } else {
                        errs.push(DocCheckError::new_error(
                            1,
                            filename.to_path_buf(),
                            &format!(
                                "link {}: {} in tool {} must start with '/docs/', 'http://', or 'https://'",
                                key_str, link_str, entry.name
                            ),
                        ));
                    }
                }
            }
            entries.push(entry);
        }
    }
    let ret_entries = if entries.is_empty() { None } else { Some(entries) };
    let ret_errors = if errs.is_empty() { None } else { Some(errs) };
    (ret_entries, ret_errors)
}

/// parses the yaml_value into a list of T elements.
/// returns the items successfully parsed, and any errors encountered.
fn parse_entries<T: DeserializeOwned>(
    filename: &Path,
    yaml_value: &Value,
) -> (Option<Vec<T>>, Option<Vec<DocCheckError>>) {
    if let Some(item_list) = yaml_value.as_sequence() {
        if item_list.is_empty() {
            (
                None,
                Some(vec![DocCheckError::new_error(
                    1,
                    filename.to_path_buf(),
                    &format!("unexpected empty list for {:?} file, got {:?}", filename, yaml_value),
                )]),
            )
        } else {
            let mut errors: Vec<DocCheckError> = vec![];
            let mut items: Vec<T> = vec![];

            for item in item_list {
                let result = serde_yaml::from_value::<T>(item.clone());
                match result {
                    Ok(element) => items.push(element),
                    Err(e) => {
                        errors.push(DocCheckError::new_error(
                            1,
                            filename.to_path_buf(),
                            &format!(
                                "invalid structure for {:?} entry: {}. Data: {:?}",
                                filename, e, item
                            ),
                        ));
                    }
                };
            }
            let ret_items = if items.is_empty() { None } else { Some(items) };
            let ret_errors = if errors.is_empty() { None } else { Some(errors) };
            (ret_items, ret_errors)
        }
    } else {
        (
            None,
            Some(vec![DocCheckError::new_error(
                1,
                filename.to_path_buf(),
                &format!(
                    "unable to parse sequence for {:?} file, expected Sequence, got {:?}",
                    filename, yaml_value
                ),
            )]),
        )
    }
}

/// Called from main to register all the checks to preform which are implemented in this module.
pub fn register_yaml_checks(
    opt: &DocCheckerArgs,
    reachability_graph: ReachabilityGraph,
    exemption_set: ExemptionSet,
) -> Result<Vec<Box<dyn DocYamlCheck>>> {
    let reference_prefix = PathBuf::from("/reference");
    let checker = YamlChecker {
        root_dir: opt.root.clone(),
        docs_folder: opt.docs_folder.clone(),
        project: opt.project.clone(),
        check_external_links: opt.check_external_links,
        allow_fuchsia_src_links: opt.allow_fuchsia_src_links,
        reference_docs_root: opt.reference_docs_root.clone(),
        reference_prefix,
        external_links: vec![],
        problems: vec![],
        tools: HashSet::new(),
        tools_entries: vec![],
        reachability_graph,
        exemption_set,
        allow_unreferenced_hidden: opt.allow_unreferenced_hidden,
    };

    Ok(vec![Box::new(checker)])
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::ErrorLevel;

    #[test]
    fn test_check_rfc_areas() -> Result<()> {
        let filename = "docs/contribute/governance/rfcs/_rfc_areas.yaml";

        // Test valid sorted and unique areas
        let yaml_value: Value = serde_yaml::from_str(
            r#"
- AreaA
- AreaB
- AreaC
            "#,
        )?;
        let result = check_areas(&PathBuf::from(filename), &yaml_value);
        assert!(result.is_none());

        // Test unsorted areas
        let yaml_value: Value = serde_yaml::from_str(
            r#"
- AreaB
- AreaA
- AreaC
            "#,
        )?;
        let result = check_areas(&PathBuf::from(filename), &yaml_value);
        assert!(result.is_some());
        let errors = result.unwrap();
        assert_eq!(errors.len(), 1);
        assert_eq!(
            errors[0].message,
            "areas are not alphabetically sorted: 'AreaA' should be before 'AreaB'"
        );

        // Test duplicate areas
        let yaml_value: Value = serde_yaml::from_str(
            r#"
- AreaA
- AreaA
- AreaB
            "#,
        )?;
        let result = check_areas(&PathBuf::from(filename), &yaml_value);
        assert!(result.is_some());
        let errors = result.unwrap();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].message, "duplicate area name: AreaA");

        Ok(())
    }

    #[test]
    fn test_check_areas_sorting_and_duplicates() -> Result<()> {
        let filename = "docs/contribute/governance/areas/_areas.yaml";

        // Test duplicates and unsorted in areas struct
        let yaml_value: Value = serde_yaml::from_str(
            r#"
- name: 'AreaB'
  api_primary: 'someone@google.com'
  api_secondary: ''
- name: 'AreaA'
  api_primary: 'someone@google.com'
  api_secondary: ''
- name: 'areab'
  api_primary: 'someone@google.com'
  api_secondary: ''
            "#,
        )?;
        let result = check_areas(&PathBuf::from(filename), &yaml_value);
        assert!(result.is_some());
        let errors = result.unwrap();
        assert_eq!(errors.len(), 2);
        assert_eq!(
            errors[0].message,
            "areas are not alphabetically sorted: 'AreaA' should be before 'AreaB'"
        );
        assert_eq!(errors[1].message, "duplicate area name: areab");

        Ok(())
    }

    #[test]
    fn test_check_path() -> Result<()> {
        let doc_line = &DocLine { line_num: 1, file_name: PathBuf::from("test-check-path") };
        let checker = YamlChecker {
            root_dir: PathBuf::from("/some/root"),
            docs_folder: PathBuf::from("docs"),
            project: "fuchsia".to_string(),
            check_external_links: false,
            allow_fuchsia_src_links: false,
            reference_docs_root: None,
            reference_prefix: PathBuf::from("/reference"),
            external_links: vec![],
            problems: vec![],
            tools: HashSet::new(),
            tools_entries: vec![],
            reachability_graph: Default::default(),
            exemption_set: Default::default(),
            allow_unreferenced_hidden: false,
        };

        let test_data: [(&str, Option<DocCheckError>); 7] = [
            ("/CONTRIBUTING.md", None),
            ("/CODE_OF_CONDUCT.md", None),
            (
                "/README.md",
                Some(DocCheckError::new_error(
                    1,
                    PathBuf::from("test-check-path"),
                    "in-tree link to /README.md must be in '/docs' or '/reference' directory",
                )),
            ),
            ("https://fuchsia.dev/reference/to/something-else.md", None),
            ("/docs/are-ok.md", None),
            ("https://somewhere.com/is-ok", None),
            (
                "/src/main.cc",
                Some(DocCheckError::new_error(
                    1,
                    PathBuf::from("test-check-path"),
                    "in-tree link to /src/main.cc must be in '/docs' or '/reference' directory",
                )),
            ),
        ];

        for (test_path, expected_result) in test_data {
            let actual_result = check_path(
                doc_line,
                &checker.root_dir,
                &checker.docs_folder,
                &checker.project,
                test_path,
                checker.allow_fuchsia_src_links,
                &checker.reference_prefix,
            );
            assert_eq!(actual_result, expected_result);
        }

        Ok(())
    }

    #[test]
    fn test_check_areas() -> Result<()> {
        // Test is more complex because of todo
        //TODO(https://fxbug.dev/42064921): Align _areas.yaml on same schema.
        let filename = "docs/contribute/governance/areas/_areas.yaml";
        let yaml_value: Value = serde_yaml::from_str(
            r#"
- name: 'Area1'
  api_primary: 'someone@google.com'
  api_secondary: 'someonelese@google.com'
  description: |
          <p>
            This is an area.
          </p>
  examples:
    - fidl: 'fuchsia.docs.samples'
- name: 'Area2'
  api_primary: 'bademail'
  api_secondary: 'anotherbademail'
- name: ''
  api_primary: 'valid@google.com'
  api_secondary: ''
          "#,
        )?;

        let result = check_areas(&PathBuf::from(filename), &yaml_value);
        assert!(result.is_some());
        let errors = result.unwrap();
        assert_eq!(errors.len(), 3);
        assert_eq!(errors[0].message, "invalid api_primary email: bademail for area Area2");
        assert_eq!(
            errors[1].message,
            "invalid api_secondary email: anotherbademail for area Area2"
        );
        assert_eq!(errors[2].message, "area name cannot be empty");

        Ok(())
    }

    #[test]
    fn test_check_roadmap() -> Result<()> {
        let filename = "docs/contribute/roadmap/2022/_roadmap.yaml";
        let yaml_value: Value = serde_yaml::from_str(
            r#"
- workstream: 'Workstream1 <a href="/docs/valid.md">Link</a>'
  area: 'Area1'
  category: ['Category1']
  bug: ['https://fxbug.dev/123']
- workstream: 'Workstream2 <a href="/docs/missing.md">Invalid</a>'
  area: 'Area2'
  category: []
- workstream: ''
  area: ''
  category: []
  bug: ['https://invalid.com/123']
           "#,
        )?;

        let result = check_roadmap(
            &PathBuf::from("."),
            &PathBuf::from("docs"),
            "fuchsia",
            &PathBuf::from(filename),
            &yaml_value,
            false,
            &mut vec![],
        );
        assert!(result.is_some());
        let errors = result.unwrap();
        assert_eq!(errors.len(), 4);
        assert_eq!(
            errors[0].message,
            "in-tree link to /docs/missing.md could not be found at \"./docs/missing.md\""
        );
        assert_eq!(errors[1].message, "workstream cannot be empty");
        assert_eq!(errors[2].message, "area cannot be empty");
        assert_eq!(
            errors[3].message,
            "invalid bug link: https://invalid.com/123. Must start with 'https://fxbug.dev/'"
        );

        let external_link_yaml: Value = serde_yaml::from_str(
            r#"
- workstream: 'Workstream <a href="https://external.com/guide">Link</a>'
  area: 'Area'
  category: []
            "#,
        )?;
        let mut external_links = vec![];
        let result = check_roadmap(
            &PathBuf::from("."),
            &PathBuf::from("docs"),
            "fuchsia",
            &PathBuf::from(filename),
            &external_link_yaml,
            false,
            &mut external_links,
        );
        assert!(result.is_none());
        assert_eq!(external_links.len(), 1);
        assert_eq!(external_links[0].link, "https://external.com/guide");

        Ok(())
    }

    #[test]
    fn test_check_redirects() -> Result<()> {
        let root_dir = PathBuf::from(".");
        let docs_folder = PathBuf::from("docs");
        let project = "fuchsia";
        let allow_fuchsia_src_links = false;
        let filename = PathBuf::from("_redirects.yaml");

        let yaml_value: Value = serde_yaml::from_str(
            r#"
redirects:
- from: /docs/old.md
  to: /docs/are-ok.md
- from: /docs/old2.md
  to: /src/main.cc
- from: /docs/old3/...
  to: /docs/...
- from: /docs/old4/...
  to: /docs/nonexistent-no-extension/...
          "#,
        )?;

        let result = check_redirects(
            &root_dir,
            &docs_folder,
            project,
            &filename,
            &yaml_value,
            allow_fuchsia_src_links,
            &PathBuf::from("/reference"),
        );

        assert!(result.is_some());
        let errors = result.unwrap();
        assert_eq!(errors.len(), 2);
        assert_eq!(
            errors[0].message,
            "in-tree link to /src/main.cc must be in '/docs' or '/reference' directory"
        );
        assert_eq!(
            errors[1].message,
            "Directory: \"./docs/nonexistent-no-extension\" not found for wildcard redirect."
        );

        Ok(())
    }

    #[test]
    fn test_check_deprecated_docs() -> Result<()> {
        let checker = YamlChecker {
            root_dir: PathBuf::from("."),
            docs_folder: PathBuf::from("docs"),
            project: "fuchsia".to_string(),
            check_external_links: false,
            allow_fuchsia_src_links: false,
            reference_docs_root: None,
            reference_prefix: PathBuf::from("/reference"),
            external_links: vec![],
            problems: vec![],
            tools: HashSet::new(),
            tools_entries: vec![],
            reachability_graph: Default::default(),
            exemption_set: Default::default(),
            allow_unreferenced_hidden: false,
        };
        let filename = PathBuf::from("_deprecated-docs.yaml");

        // Test valid in-tree, external, and reference paths
        let yaml_value: Value = serde_yaml::from_str(
            r#"
included:
- from: /docs/old.md
  to: /docs/are-ok.md
- from: /docs/old2.md
  to: https://external.com/is-ok
- from: /docs/old3.md
  to: /reference/some-ref.md
            "#,
        )?;

        let result = check_deprecated_docs(
            &checker.root_dir,
            &checker.docs_folder,
            &checker.project,
            &filename,
            &yaml_value,
            checker.allow_fuchsia_src_links,
            &checker.reference_prefix,
        );
        assert!(result.is_none());

        // Test invalid `from` path (not starting with /) and invalid `to` path
        let invalid_yaml: Value = serde_yaml::from_str(
            r#"
included:
- from: docs/no-leading-slash.md
  to: /docs/are-ok.md
- from: /docs/old.md
  to: /src/main.cc
- from: /bad/from.md
  to: /docs/are-ok.md
            "#,
        )?;

        let result = check_deprecated_docs(
            &checker.root_dir,
            &checker.docs_folder,
            &checker.project,
            &filename,
            &invalid_yaml,
            checker.allow_fuchsia_src_links,
            &checker.reference_prefix,
        );
        assert!(result.is_some());
        let errors = result.unwrap();
        assert_eq!(errors.len(), 3);
        assert_eq!(
            errors[0].message,
            "Deprecation 'from' path must start with '/': docs/no-leading-slash.md"
        );
        assert_eq!(
            errors[1].message,
            "in-tree link to /src/main.cc must be in '/docs' or '/reference' directory"
        );
        assert_eq!(
            errors[2].message,
            "Deprecation 'from' path must start with '/docs' or '/reference': /bad/from.md"
        );

        // Test wildcard check in `from` path
        let wildcard_yaml: Value = serde_yaml::from_str(
            r#"
included:
- from: /docs/old/*.md
  to: /docs/are-ok.md
            "#,
        )?;

        let result = check_deprecated_docs(
            &checker.root_dir,
            &checker.docs_folder,
            &checker.project,
            &filename,
            &wildcard_yaml,
            checker.allow_fuchsia_src_links,
            &checker.reference_prefix,
        );
        assert!(result.is_some());
        let errors = result.unwrap();
        assert_eq!(errors.len(), 1);
        assert_eq!(
            errors[0].message,
            "Wildcards are not supported in deprecation 'from' path: /docs/old/*.md"
        );

        Ok(())
    }

    #[test]
    fn test_check_rfcs() -> Result<()> {
        let filename = PathBuf::from("docs/contribute/governance/rfcs/_rfcs.yaml");
        let yaml_value: Value = serde_yaml::from_str(
            r#"
- name: 'RFC-0001'
  title: 'RFC Process'
  short_description: 'The RFC process'
  authors: ['someone@google.com']
  file: '0001_rfc_process.md'
  area: ['Governance']
  issue: ['123']
  gerrit_change_id: ['456']
  status: 'Accepted'
  reviewers: ['someoneelse@google.com']
  submitted: '2020-01-01'
  reviewed: '2020-01-02'
- name: 'RFC-0002'
  title: 'Missing File'
  short_description: 'A missing file'
  authors: ['someone@google.com']
  file: 'missing.txt'
  area: ['Governance']
  issue: ['123']
  gerrit_change_id: ['456']
  status: 'Accepted'
  reviewers: ['someoneelse@google.com']
  submitted: '2020-01-01'
  reviewed: '2020-01-02'
          "#,
        )?;

        let result = check_rfcs(&filename, &yaml_value);

        assert!(result.is_some());
        let errors = result.unwrap();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].message, "RFC file missing.txt does not exist");

        Ok(())
    }

    #[test]
    fn test_check_supported_sys_config() -> Result<()> {
        let root_dir = PathBuf::from("/some/root");
        let filename = PathBuf::from("docs/reference/hardware/_supported_sys_config.yaml");
        let yaml_value: Value = serde_yaml::from_str(
            r#"
- name: 'Device1'
  description: 'A device'
  architecture: 'ARM'
  board_driver_location: '/src/devices/board/drivers/vim3.md'
- name: 'Device2'
  description: 'A device with missing driver'
  architecture: 'x64'
  board_driver_location: '/src/devices/board/drivers/missing.txt'
- name: 'Device3'
  description: 'A device with invalid URL'
  architecture: 'x64'
  board_driver_location: '/src/devices/board/drivers/vim3.md'
  manufacturer_link: 'invalid-url'
- name: 'Device4'
  description: 'A device with invalid arch'
  architecture: 'bad_arch'
  board_driver_location: '/src/devices/board/drivers/vim3.md'
          "#,
        )?;

        let result = check_supported_sys_config(&root_dir, &filename, &yaml_value, &mut vec![]);

        assert!(result.is_some());
        let errors = result.unwrap();
        assert_eq!(errors.len(), 3);
        assert_eq!(
            errors[0].message,
            "board_driver_location /src/devices/board/drivers/missing.txt does not exist"
        );
        assert_eq!(errors[1].message, "manufacturer_link invalid-url is not a valid URL");
        assert_eq!(
            errors[2].message,
            "invalid architecture: bad_arch. Must be one of the supported architectures: [\"ARM\", \"x64\"]"
        );

        let external_link_yaml: Value = serde_yaml::from_str(
            r#"
- name: 'Device'
  description: 'A device'
  architecture: 'x64'
  board_driver_location: '/src/devices/board/drivers/vim3.md'
  manufacturer_link: 'https://external.com/device'
            "#,
        )?;
        let mut external_links = vec![];
        let result = check_supported_sys_config(
            &root_dir,
            &filename,
            &external_link_yaml,
            &mut external_links,
        );
        assert!(result.is_none());
        assert_eq!(external_links.len(), 1);
        assert_eq!(external_links[0].link, "https://external.com/device");

        Ok(())
    }

    #[test]
    fn test_check_supported_cpu_architecture() -> Result<()> {
        let filename = PathBuf::from("docs/reference/hardware/_supported_cpu_architecture.yaml");
        let yaml_value: Value = serde_yaml::from_str(
            r#"
- ARM
- x64
- invalid_arch
          "#,
        )?;

        let result = check_supported_cpu_architecture(&filename, &yaml_value);

        assert!(result.is_some());
        let errors = result.unwrap();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].message, "invalid CPU architecture: invalid_arch");

        Ok(())
    }

    #[test]
    fn test_check_tools() -> Result<()> {
        let filename = PathBuf::from("docs/reference/troubleshooting/_tools.yaml");
        let yaml_value: Value = serde_yaml::from_str(
            r#"
- name: Test Tool
  team: Diagnostics
  links:
    Overview: /docs/README.md
    Missing: /docs/invalid/missing.md
    Bad: bad_prefix
  description: 'Testing'
          "#,
        )?;

        let root_dir = PathBuf::from(".");
        let (_tools, errors) = check_tools(&root_dir, &filename, &yaml_value, &mut vec![]);

        assert!(errors.is_some());
        let errors = errors.unwrap();
        assert_eq!(errors.len(), 2);
        assert_eq!(
            errors[0].message,
            "link Missing: /docs/invalid/missing.md in tool Test Tool does not exist"
        );
        assert_eq!(
            errors[1].message,
            "link Bad: bad_prefix in tool Test Tool must start with '/docs/', 'http://', or 'https://'"
        );

        let external_link_yaml: Value = serde_yaml::from_str(
            r#"
- name: Test Tool
  team: Diagnostics
  links:
    External: https://external.com/tool
  description: 'Testing'
            "#,
        )?;
        let mut external_links = vec![];
        let (_tools, errors) =
            check_tools(&root_dir, &filename, &external_link_yaml, &mut external_links);
        assert!(errors.is_none());
        assert_eq!(external_links.len(), 1);
        assert_eq!(external_links[0].link, "https://external.com/tool");

        Ok(())
    }

    #[test]
    fn test_check_drivers_epitaphs() -> Result<()> {
        let filename = PathBuf::from("docs/reference/hardware/_drivers_epitaphs.yaml");
        let yaml_value: Value = serde_yaml::from_str(
            r#"
- short_description: 'Test Driver'
  deletion_reason: 'Testing'
  gerrit_change_id: '12345'
  available_in_git: '4c6f2330ffdd30a6b1a188ed466eaa1b1bafe7fe'
  path: '/src/devices/test'
- short_description: 'Bad Driver'
  deletion_reason: 'Testing'
  gerrit_change_id: 'bad_id'
  available_in_git: 'short_hash'
  path: '/src/devices/bad'
- short_description: 'Bad Path Driver'
  deletion_reason: 'Testing'
  gerrit_change_id: '12345'
  available_in_git: '4c6f2330ffdd30a6b1a188ed466eaa1b1bafe7fe'
  path: '/bad/path'
- short_description: 'Empty Areas Driver'
  deletion_reason: 'Testing'
  gerrit_change_id: '12345'
  available_in_git: '4c6f2330ffdd30a6b1a188ed466eaa1b1bafe7fe'
  areas: []
  path: '/src/devices/test2'
          "#,
        )?;

        let result = check_drivers_epitaphs(&filename, &yaml_value);

        assert!(result.is_some());
        let errors = result.unwrap();
        assert_eq!(errors.len(), 4);
        assert_eq!(
            errors[0].message,
            "invalid gerrit_change_id: bad_id. Must be a number or 'TBD'"
        );
        assert_eq!(
            errors[1].message,
            "invalid available_in_git hash: short_hash. Must be a 40 or 41-character hex string"
        );
        assert_eq!(
            errors[2].message,
            "path /bad/path must start with '/src/', '/zircon/', or '/examples/'"
        );
        assert_eq!(errors[3].message, "areas list cannot be empty if provided");

        Ok(())
    }

    #[test]
    fn test_check_metadata() -> Result<()> {
        let root_dir = PathBuf::from(".");
        let docs_folder = PathBuf::from("docs");
        let project = "fuchsia";
        let allow_fuchsia_src_links = false;
        let filename = PathBuf::from("_metadata.yaml");

        let valid_yaml: Value = serde_yaml::from_str(
            r#"
descriptions:
  type: "Type desc"
columns:
  - "Type"
types:
  - "Custom"
products:
  - "Core"
boards:
  - "VIM"
methods:
  - "USB Cable"
hosts:
  - "Linux"
guides:
  - type: "Custom"
    product: "Core"
    board: "VIM"
    method: "USB Cable"
    host: "Linux"
    url: "/docs/are-ok.md"
    title: "Guide Title"
          "#,
        )?;

        let result = check_metadata(
            &root_dir,
            &docs_folder,
            project,
            &filename,
            &valid_yaml,
            allow_fuchsia_src_links,
            &mut vec![],
        );
        assert!(result.is_none());

        let invalid_yaml: Value = serde_yaml::from_str(
            r#"
descriptions:
  type: "Type desc"
columns:
  - "Type"
types:
  - "Custom"
products:
  - "Core"
boards:
  - "VIM"
methods:
  - "USB Cable"
hosts:
  - "Linux"
guides:
  - type: "InvalidType"
    product: "InvalidProduct"
    board: "VIM"
    method: "USB Cable"
    host: "Linux"
    url: "/docs/are-ok.md"
    title: "Guide Title"
          "#,
        )?;

        let result = check_metadata(
            &root_dir,
            &docs_folder,
            project,
            &filename,
            &invalid_yaml,
            allow_fuchsia_src_links,
            &mut vec![],
        );
        assert!(result.is_some());
        let errors = result.unwrap();
        assert_eq!(errors.len(), 2);

        let invalid_fields_yaml: Value = serde_yaml::from_str(
            r#"
descriptions:
  type: "Type desc"
columns:
  - "Type"
types:
  - "Custom"
products:
  - "Core"
boards:
  - "VIM"
methods:
  - "USB Cable"
hosts:
  - "Linux"
guides:
  - type: "Custom"
    product: "Core"
    board: "InvalidBoard"
    method: "InvalidMethod"
    host: "InvalidHost"
    url: "/docs/are-ok.md"
    title: "Guide Title"
          "#,
        )?;

        let result = check_metadata(
            &root_dir,
            &docs_folder,
            project,
            &filename,
            &invalid_fields_yaml,
            allow_fuchsia_src_links,
            &mut vec![],
        );
        assert!(result.is_some());
        let errors = result.unwrap();
        assert_eq!(errors.len(), 3);

        let invalid_path_yaml: Value = serde_yaml::from_str(
            r#"
descriptions:
  type: "Type desc"
columns:
  - "Type"
types:
  - "Custom"
products:
  - "Core"
boards:
  - "VIM"
methods:
  - "USB Cable"
hosts:
  - "Linux"
guides:
  - type: "Custom"
    product: "Core"
    board: "VIM"
    method: "USB Cable"
    host: "Linux"
    url: "custom-scheme://foo"
    title: "Guide Title"
          "#,
        )?;

        let result = check_metadata(
            &root_dir,
            &docs_folder,
            project,
            &filename,
            &invalid_path_yaml,
            allow_fuchsia_src_links,
            &mut vec![],
        );
        assert!(result.is_some());
        let errors = result.unwrap();
        assert_eq!(errors.len(), 1);
        eprintln!("ACTUAL ERROR MESSAGE: {}", errors[0].message);
        assert!(errors[0].message.contains("invalid path"));

        let external_link_yaml: Value = serde_yaml::from_str(
            r#"
descriptions:
  type: "Type desc"
columns:
  - "Type"
types:
  - "Custom"
products:
  - "Core"
boards:
  - "VIM"
methods:
  - "USB Cable"
hosts:
  - "Linux"
guides:
  - type: "Custom"
    product: "Core"
    board: "VIM"
    method: "USB Cable"
    host: "Linux"
    url: "https://external.com/guide"
    title: "Guide Title"
          "#,
        )?;

        let mut external_links = vec![];
        let result = check_metadata(
            &root_dir,
            &docs_folder,
            project,
            &filename,
            &external_link_yaml,
            allow_fuchsia_src_links,
            &mut external_links,
        );
        assert!(result.is_none());
        assert_eq!(external_links.len(), 1);
        assert_eq!(external_links[0].link, "https://external.com/guide");

        Ok(())
    }

    #[fuchsia::test]
    async fn test_check_problems_and_post_check() -> Result<()> {
        let filename = PathBuf::from("docs/reference/troubleshooting/_problems.yaml");
        let yaml_value: Value = serde_yaml::from_str(
            r#"
- key: Test Problem
  use_case: 'Testing'
  tools:
    - Test Tool
    - Missing Tool
  description: |
    This is a description with a link to [Test Tool] and [another link with `code`](https://external.com).
    Also [broken link][Missing Tool] and [really broken link][Unknown].
  related-problems: []
            "#,
        )?;

        let mut checker = YamlChecker {
            root_dir: PathBuf::from("."),
            docs_folder: PathBuf::from("docs"),
            project: "fuchsia".to_string(),
            check_external_links: false,
            allow_fuchsia_src_links: false,
            reference_docs_root: None,
            reference_prefix: PathBuf::from("/reference"),
            external_links: vec![],
            problems: vec![],
            tools: HashSet::new(),
            tools_entries: vec![],
            reachability_graph: Default::default(),
            exemption_set: Default::default(),
            allow_unreferenced_hidden: false,
        };

        let result = checker.check(&filename, &yaml_value)?;
        assert!(result.is_none());

        assert_eq!(checker.problems.len(), 1);
        assert_eq!(checker.problems[0].1.key, "Test Problem");

        // Now run post_check with defined tools
        checker.tools.insert("Test Tool".to_string());

        let post_result = checker.post_check(&[], &[]).await?;
        assert!(post_result.is_some());
        let errors = post_result.unwrap();

        let error_messages: Vec<String> = errors.iter().map(|e| e.message.clone()).collect();

        // 1. Missing Tool referenced in problems is not defined in _tools.yaml
        assert!(error_messages.contains(&"Tool 'Missing Tool' referenced in problem 'Test Problem' does not exist in _tools.yaml".to_string()));
        // 2. Unknown reference link: [Missing Tool]
        assert!(error_messages.contains(&"Unknown reference link: [Missing Tool]".to_string()));
        // 3. Unknown reference link: [Unknown]
        assert!(error_messages.contains(&"Unknown reference link: [Unknown]".to_string()));

        Ok(())
    }

    #[fuchsia::test]
    async fn test_problems_and_tools_markdown_validation() -> Result<()> {
        let root_dir = PathBuf::from(".");
        let mut checker = YamlChecker {
            root_dir: root_dir.clone(),
            docs_folder: PathBuf::from("docs"),
            project: "fuchsia".to_string(),
            check_external_links: false,
            allow_fuchsia_src_links: false,
            reference_docs_root: None,
            reference_prefix: PathBuf::from("/reference"),
            external_links: vec![],
            problems: vec![],
            tools: HashSet::new(),
            tools_entries: vec![],
            reachability_graph: Default::default(),
            exemption_set: Default::default(),
            allow_unreferenced_hidden: false,
        };

        let tools_yaml = serde_yaml::from_str(
            r#"
- name: ToolA
  team: Diagnostics
  links: {}
  description: 'Referencing [ToolB] and inline link [Doc](/docs/missing.md)'
  related:
    - ToolC
- name: ToolB
  team: Diagnostics
  links: {}
  description: 'Tool B description'
            "#,
        )?;
        let problems_yaml = serde_yaml::from_str(
            r#"
- key: Problem1
  use_case: 'I want to link to [ToolA]'
  tools:
    - ToolA
  description: 'Problem description with a broken [nonexistent_ref] and valid [ToolB]'
  related-problems: []
            "#,
        )?;

        let tools_file = PathBuf::from("docs/reference/troubleshooting/_tools.yaml");
        let problems_file = PathBuf::from("docs/reference/troubleshooting/_problems.yaml");

        checker.check(&tools_file, &tools_yaml)?;
        checker.check(&problems_file, &problems_yaml)?;

        let post_errs = checker.post_check(&[], &[]).await?;
        assert!(post_errs.is_some());
        let errors = post_errs.unwrap();
        let errors: Vec<DocCheckError> =
            errors.into_iter().filter(|e| !e.message.contains("_toc.yaml")).collect();

        let error_messages: Vec<String> = errors.iter().map(|e| e.message.clone()).collect();
        // 1. ToolC does not exist in tools:
        assert!(error_messages.contains(
            &"Related tool 'ToolC' referenced in tool 'ToolA' does not exist".to_string()
        ));
        // 2. /docs/nonexistent.md does not exist:
        assert!(error_messages.iter().any(|m| m.contains("could not be found at")));
        // 3. nonexistent_ref is not a valid tool or problem:
        assert!(error_messages.contains(&"Unknown reference link: [nonexistent_ref]".to_string()));

        Ok(())
    }

    #[fuchsia::test]
    async fn test_problems_validation_extended() -> Result<()> {
        let root_dir = PathBuf::from(".");
        let mut checker = YamlChecker {
            root_dir: root_dir.clone(),
            docs_folder: PathBuf::from("docs"),
            project: "fuchsia".to_string(),
            check_external_links: false,
            allow_fuchsia_src_links: false,
            reference_docs_root: None,
            reference_prefix: PathBuf::from("/reference"),
            external_links: vec![],
            problems: vec![],
            tools: HashSet::new(),
            tools_entries: vec![],
            reachability_graph: Default::default(),
            exemption_set: Default::default(),
            allow_unreferenced_hidden: false,
        };

        // Test empty fields
        let invalid_fields_yaml = serde_yaml::from_str(
            r#"
- key: ''
  use_case: ''
  tools: []
  description: ''
  related-problems: []
            "#,
        )?;
        let problems_file = PathBuf::from("docs/reference/troubleshooting/_problems.yaml");
        let errs = checker.check(&problems_file, &invalid_fields_yaml)?;
        assert!(errs.is_some());
        let errors = errs.unwrap();
        assert_eq!(errors.len(), 3);
        assert_eq!(errors[0].message, "problem key cannot be empty");
        assert_eq!(errors[1].message, "use_case for problem '' cannot be empty");
        assert_eq!(errors[2].message, "description for problem '' cannot be empty");

        // Test duplicate keys and self reference
        let dup_self_yaml = serde_yaml::from_str(
            r#"
- key: ProblemDup
  use_case: 'Use case'
  tools: []
  description: 'Description'
  related-problems:
    - ProblemDup
- key: ProblemDup
  use_case: 'Use case'
  tools: []
  description: 'Description'
  related-problems: []
            "#,
        )?;
        let mut checker = YamlChecker {
            root_dir: root_dir.clone(),
            docs_folder: PathBuf::from("docs"),
            project: "fuchsia".to_string(),
            check_external_links: false,
            allow_fuchsia_src_links: false,
            reference_docs_root: None,
            reference_prefix: PathBuf::from("/reference"),
            external_links: vec![],
            problems: vec![],
            tools: HashSet::new(),
            tools_entries: vec![],
            reachability_graph: Default::default(),
            exemption_set: Default::default(),
            allow_unreferenced_hidden: false,
        };
        let errs = checker.check(&problems_file, &dup_self_yaml)?;
        assert!(errs.is_none());

        let post_errs = checker.post_check(&[], &[]).await?;
        assert!(post_errs.is_some());
        let errors = post_errs.unwrap();
        let errors: Vec<DocCheckError> =
            errors.into_iter().filter(|e| !e.message.contains("_toc.yaml")).collect();
        assert_eq!(errors.len(), 2);
        let error_messages: Vec<String> = errors.iter().map(|e| e.message.clone()).collect();
        assert!(error_messages.contains(&"Duplicate problem key: ProblemDup".to_string()));
        assert!(error_messages.contains(
            &"Problem 'ProblemDup' cannot reference itself as a related problem".to_string()
        ));

        Ok(())
    }

    #[fuchsia::test]
    async fn test_problems_validation() -> Result<()> {
        let root_dir = PathBuf::from(".");
        let mut checker = YamlChecker {
            root_dir: root_dir.clone(),
            docs_folder: PathBuf::from("docs"),
            project: "fuchsia".to_string(),
            check_external_links: false,
            allow_fuchsia_src_links: false,
            reference_docs_root: None,
            reference_prefix: PathBuf::from("/reference"),
            external_links: vec![],
            problems: vec![],
            tools: HashSet::new(),
            tools_entries: vec![],
            reachability_graph: Default::default(),
            exemption_set: Default::default(),
            allow_unreferenced_hidden: false,
        };

        let tools_yaml = serde_yaml::from_str(
            r#"
- name: ToolA
  team: Diagnostics
  links: {}
  description: 'Tool A'
- name: ToolB
  team: Diagnostics
  links: {}
  description: 'Tool B'
            "#,
        )?;
        let problems_yaml = serde_yaml::from_str(
            r#"
- key: Problem1
  use_case: 'Use case 1'
  tools:
    - ToolA
    - ToolC
  description: 'Problem 1'
  related-problems:
    - Problem3
- key: Problem2
  use_case: 'Use case 2'
  tools:
    - ToolB
  description: 'Problem 2'
  related-problems:
    - Problem1
            "#,
        )?;

        let tools_file = PathBuf::from("docs/reference/troubleshooting/_tools.yaml");
        let problems_file = PathBuf::from("docs/reference/troubleshooting/_problems.yaml");

        let errs1 = checker.check(&tools_file, &tools_yaml)?;
        assert!(errs1.is_none());

        let errs2 = checker.check(&problems_file, &problems_yaml)?;
        assert!(errs2.is_none());

        let post_errs = checker.post_check(&[], &[]).await?;
        assert!(post_errs.is_some());
        let errors = post_errs.unwrap();
        let errors: Vec<DocCheckError> =
            errors.into_iter().filter(|e| !e.message.contains("_toc.yaml")).collect();
        assert_eq!(errors.len(), 2);

        let error_messages: Vec<String> = errors.iter().map(|e| e.message.clone()).collect();
        assert!(
            error_messages.contains(
                &"Tool 'ToolC' referenced in problem 'Problem1' does not exist in _tools.yaml"
                    .to_string()
            )
        );
        assert!(
            error_messages.contains(
                &"Related problem 'Problem3' referenced in problem 'Problem1' does not exist"
                    .to_string()
            )
        );

        Ok(())
    }

    #[test]
    fn test_check_file_reachability() {
        let exemption_set: ExemptionSet = Default::default();
        let mut checker = YamlChecker {
            root_dir: PathBuf::from("/workspace"),
            docs_folder: PathBuf::from("docs"),
            project: "fuchsia".to_string(),
            check_external_links: false,
            allow_fuchsia_src_links: false,
            reference_docs_root: None,
            reference_prefix: PathBuf::from("/reference"),
            external_links: vec![],
            problems: vec![],
            tools: HashSet::new(),
            tools_entries: vec![],
            reachability_graph: Default::default(),
            exemption_set: exemption_set.clone(),
            allow_unreferenced_hidden: false,
        };

        let normal_doc = PathBuf::from("/workspace/docs/normal.md");
        let hidden_doc = PathBuf::from("/workspace/docs/_common/_macro.md");

        // 1. Unreachable non-hidden doc -> missing from _toc.yaml error
        let mut errors = vec![];
        checker.check_file_reachability(&[normal_doc.clone()], &HashSet::new(), &mut errors);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].message, "File not referenced in any _toc.yaml files.");

        // 2. Unreachable hidden doc without exemption -> unused hidden file error
        errors.clear();
        checker.check_file_reachability(&[hidden_doc.clone()], &HashSet::new(), &mut errors);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.starts_with("Unused hidden file."));

        // 3. Unreachable hidden doc WITH exemption -> no error
        exemption_set.lock().unwrap().insert(hidden_doc.clone());
        errors.clear();
        checker.check_file_reachability(&[hidden_doc.clone()], &HashSet::new(), &mut errors);
        assert!(errors.is_empty());

        // 4. Unreachable hidden doc without exemption when allow_unreferenced_hidden is true -> no error
        exemption_set.lock().unwrap().clear();
        checker.allow_unreferenced_hidden = true;
        errors.clear();
        checker.check_file_reachability(&[hidden_doc.clone()], &HashSet::new(), &mut errors);
        assert!(errors.is_empty());

        // 5. Reachable hidden doc WITH exemption -> redundant exemption warning
        exemption_set.lock().unwrap().insert(hidden_doc.clone());
        checker.allow_unreferenced_hidden = false;
        let mut reachable = HashSet::new();
        reachable.insert(hidden_doc.clone());
        errors.clear();
        checker.check_file_reachability(&[hidden_doc.clone()], &reachable, &mut errors);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].level, ErrorLevel::Warning);
        assert!(errors[0].message.starts_with("Redundant exemption comment."));

        // 6. Unreachable non-hidden doc WITH exemption -> STILL missing from _toc.yaml error
        exemption_set.lock().unwrap().clear();
        exemption_set.lock().unwrap().insert(normal_doc.clone());
        errors.clear();
        checker.check_file_reachability(&[normal_doc.clone()], &HashSet::new(), &mut errors);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].message, "File not referenced in any _toc.yaml files.");

        // 7. Unreachable non-hidden doc when allow_unreferenced_hidden is true -> STILL missing from _toc.yaml error
        exemption_set.lock().unwrap().clear();
        checker.allow_unreferenced_hidden = true;
        errors.clear();
        checker.check_file_reachability(&[normal_doc.clone()], &HashSet::new(), &mut errors);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].message, "File not referenced in any _toc.yaml files.");

        // 8. Reachable non-hidden doc WITH exemption -> redundant exemption warning
        exemption_set.lock().unwrap().insert(normal_doc.clone());
        checker.allow_unreferenced_hidden = false;
        let mut reachable_normal = HashSet::new();
        reachable_normal.insert(normal_doc.clone());
        errors.clear();
        checker.check_file_reachability(&[normal_doc], &reachable_normal, &mut errors);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].level, ErrorLevel::Warning);
        assert!(errors[0].message.starts_with("Redundant exemption comment."));

        // 9. Hidden doc under reference_docs_root
        let ref_root = PathBuf::from("/reference_workspace");
        checker.reference_docs_root = Some(ref_root.clone());
        let ref_hidden = ref_root.join("_ref_macro.md");
        exemption_set.lock().unwrap().clear();
        errors.clear();
        checker.check_file_reachability(&[ref_hidden.clone()], &HashSet::new(), &mut errors);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.starts_with("Unused hidden file."));

        // With exemption -> no error
        exemption_set.lock().unwrap().insert(ref_hidden.clone());
        errors.clear();
        checker.check_file_reachability(&[ref_hidden.clone()], &HashSet::new(), &mut errors);
        assert!(errors.is_empty());

        // 10. Reachable hidden doc WITHOUT exemption -> no error, no warning (properly referenced)
        exemption_set.lock().unwrap().clear();
        let mut reachable_hidden = HashSet::new();
        reachable_hidden.insert(hidden_doc.clone());
        errors.clear();
        checker.check_file_reachability(&[hidden_doc.clone()], &reachable_hidden, &mut errors);
        assert!(errors.is_empty());

        // 11. Relative hidden doc path without exemption -> unused hidden file error
        let rel_hidden_doc = PathBuf::from("docs/_common/_macro.md");
        errors.clear();
        checker.check_file_reachability(&[rel_hidden_doc.clone()], &HashSet::new(), &mut errors);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.starts_with("Unused hidden file."));

        // 12. Reachable hidden doc under reference_docs_root WITH exemption -> redundant exemption warning
        exemption_set.lock().unwrap().clear();
        exemption_set.lock().unwrap().insert(ref_hidden.clone());
        let mut reachable_ref = HashSet::new();
        reachable_ref.insert(ref_hidden.clone());
        errors.clear();
        checker.check_file_reachability(&[ref_hidden.clone()], &reachable_ref, &mut errors);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].level, ErrorLevel::Warning);
        assert!(errors[0].message.starts_with("Redundant exemption comment."));

        // 13. Reachable hidden doc under reference_docs_root WITHOUT exemption -> no error, no warning
        exemption_set.lock().unwrap().clear();
        errors.clear();
        checker.check_file_reachability(&[ref_hidden], &reachable_ref, &mut errors);
        assert!(errors.is_empty());
    }

    #[test]
    fn test_traverse_reachability_graph() {
        let graph: ReachabilityGraph = Default::default();
        let checker = YamlChecker {
            root_dir: PathBuf::from("/workspace"),
            docs_folder: PathBuf::from("docs"),
            project: "fuchsia".to_string(),
            check_external_links: false,
            allow_fuchsia_src_links: false,
            reference_docs_root: None,
            reference_prefix: PathBuf::from("/reference"),
            external_links: vec![],
            problems: vec![],
            tools: HashSet::new(),
            tools_entries: vec![],
            reachability_graph: graph.clone(),
            exemption_set: Default::default(),
            allow_unreferenced_hidden: false,
        };

        let doc_a = PathBuf::from("/workspace/docs/a.md");
        let doc_b = PathBuf::from("/workspace/docs/b.md");
        let doc_c = PathBuf::from("/workspace/docs/c.md");
        let doc_d = PathBuf::from("/workspace/docs/d.md");
        let doc_e = PathBuf::from("/workspace/docs/_common/_e.md");
        let doc_f = PathBuf::from("/workspace/docs/f.md");
        let root_2 = PathBuf::from("/workspace/docs/root2.md");
        let isolated_1 = PathBuf::from("/workspace/docs/iso1.md");
        let isolated_2 = PathBuf::from("/workspace/docs/iso2.md");

        // Construct dependency graph:
        // A -> B -> C -> A (cycle)
        // B -> B (self loop)
        // B -> D (multi-hop branch)
        // C -> D (diamond convergence)
        // D -> _E (multi-hop to hidden doc)
        // root_2 -> F -> _E (second root converging on hidden doc)
        // isolated_1 -> isolated_2 (disconnected subgraph)
        {
            let mut g = graph.lock().unwrap();
            g.entry(doc_a.clone()).or_default().insert(doc_b.clone());
            g.entry(doc_b.clone()).or_default().insert(doc_b.clone());
            g.entry(doc_b.clone()).or_default().insert(doc_c.clone());
            g.entry(doc_b.clone()).or_default().insert(doc_d.clone());
            g.entry(doc_c.clone()).or_default().insert(doc_a.clone());
            g.entry(doc_c.clone()).or_default().insert(doc_d.clone());
            g.entry(doc_d.clone()).or_default().insert(doc_e.clone());
            g.entry(root_2.clone()).or_default().insert(doc_f.clone());
            g.entry(doc_f.clone()).or_default().insert(doc_e.clone());
            g.entry(isolated_1.clone()).or_default().insert(isolated_2.clone());
        }

        let mut reachable = HashSet::new();
        reachable.insert(doc_a.clone());
        reachable.insert(root_2.clone());

        checker.traverse_reachability_graph(&mut reachable);

        assert!(reachable.contains(&doc_a));
        assert!(reachable.contains(&doc_b));
        assert!(reachable.contains(&doc_c));
        assert!(reachable.contains(&doc_d));
        assert!(reachable.contains(&doc_e));
        assert!(reachable.contains(&root_2));
        assert!(reachable.contains(&doc_f));
        assert!(!reachable.contains(&isolated_1));
        assert!(!reachable.contains(&isolated_2));
    }
}
