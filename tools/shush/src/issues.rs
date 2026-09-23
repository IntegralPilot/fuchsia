// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use once_cell::sync::OnceCell;
use serde::{Deserialize, Serialize};

use std::collections::HashMap;
use std::fmt::Write;

use crate::api::{Api, ComponentId, CreateIssue, IssueId, Status, UpdateIssue};
use crate::lint::{Lint, LintFile};
use crate::owners::FileOwnership;

struct ComponentDefs {
    defs: HashMap<String, ComponentId>,
}

impl ComponentDefs {
    fn load(api: &mut (impl Api + ?Sized)) -> Result<Self> {
        let mut defs = HashMap::new();
        for component in api.list_components()? {
            let path = component
                .path
                .iter()
                .map(|x| x.replace(' ', ""))
                .reduce(|acc, x| format!("{acc}>{x}"))
                .unwrap_or_default();
            defs.insert(path, component.id);
        }
        Ok(Self { defs })
    }

    fn get(&self, component: &str) -> Option<ComponentId> {
        self.defs.get(component).copied()
    }
}

pub struct IssueTemplate<'a> {
    // Settings
    filter: &'a [String],
    codesearch_ref: Option<&'a str>,
    template: Option<String>,
    blocking_issue: Option<&'a str>,
    max_cc_users: usize,

    // Cache
    component_defs: OnceCell<ComponentDefs>,
}

impl<'a> IssueTemplate<'a> {
    pub fn new(
        filter: &'a [String],
        codesearch_ref: Option<&'a str>,
        template: Option<String>,
        blocking_issue: Option<&'a str>,
        max_cc_users: usize,
    ) -> Self {
        Self {
            filter,
            codesearch_ref,
            template,
            blocking_issue,
            max_cc_users,

            component_defs: OnceCell::new(),
        }
    }

    fn format_lints(&self, mut description: String, file: &LintFile) -> String {
        let mut insertions =
            file.lints.iter().map(|l| self.annotation_msg(l, &file.path)).collect::<Vec<_>>();
        insertions.sort();

        write!(
            &mut description,
            "\n[{}]({})\n{}\n",
            file.path,
            self.codesearch_url(&file.path, None),
            insertions.join("\n"),
        )
        .unwrap();

        description
    }

    fn annotation_msg(&self, l: &Lint, path: &str) -> String {
        let lints_url = "https://rust-lang.github.io/rust-clippy/master#".to_owned();
        let cs_url = self.codesearch_url(path, Some(l.span.start.line));

        // format both clippy and normal rustc lints in markdown
        if let Some(name) = l.name.strip_prefix("clippy::") {
            format!(
                "- [{name}]({}) on [line {}]({})",
                (lints_url + name),
                l.span.start.line,
                cs_url
            )
        } else {
            format!("- {} on [line {}]({})", l.name, l.span.start.line, cs_url)
        }
    }

    fn codesearch_url(&self, path: &str, line: Option<usize>) -> String {
        let mut link = format!(
            "https://cs.opensource.google/fuchsia/fuchsia/+/{}:{}",
            self.codesearch_ref.unwrap_or("main"),
            path
        );
        if let Some(line) = line {
            link.push_str(&format!(";l={}", line))
        }
        link
    }

    fn get_component_defs(
        &self,
        api: &mut (impl Api + ?Sized),
        verbose: bool,
    ) -> Result<&ComponentDefs> {
        self.component_defs.get_or_try_init(|| {
            if verbose {
                println!("Loading component definitions, this may take a bit...")
            };
            ComponentDefs::load(api)
        })
    }

    pub fn create(
        &self,
        api: &mut dyn Api,
        ownership: &FileOwnership,
        files: &[&LintFile],
        holding_component_name: &str,
        verbose: bool,
    ) -> Result<Issue> {
        let component_defs = self.get_component_defs(api, verbose)?;

        let mut title = "Please inspect ".to_string();
        match self.filter {
            [a] => write!(title, "these {}", a)?,
            [a, b] => write!(title, "these {} and {}", a, b)?,
            _ => write!(title, "multiple new")?,
        }
        write!(title, " lints")?;

        let mut component = None;
        let mut owner = None;
        let mut cc_users = None;
        let mut blocking_issues = Vec::new();

        if let Some(component_name) = &ownership.component {
            write!(title, " in {}", component_name)?;
            if let Some(c) = component_defs.get(component_name) {
                component = Some(c);
            } else {
                eprintln!(
                    "could not find component '{component_name}' in components map, skipping"
                );
            }
        }

        if !ownership.owners.is_empty() {
            // A small price to pay for salvation
            owner = Some(ownership.owners[0].clone());

            if ownership.component.is_none() && ownership.owners.len() > 1 {
                cc_users = Some(
                    ownership
                        .owners
                        .iter()
                        // skip the owner
                        .skip(1)
                        .take(self.max_cc_users)
                        .cloned()
                        .collect(),
                );
            }
        }

        let details = files.iter().fold(String::new(), |d, f| self.format_lints(d, f));
        let description = if let Some(ref template) = self.template {
            template.replace("INSERT_DETAILS_HERE", &details)
        } else {
            details
        };

        if let Some(i) = self.blocking_issue {
            blocking_issues.push(IssueId::new(i.parse()?));
        }

        let holding_component = component_defs
            .get(holding_component_name)
            .expect("couldn't find holding component definition");
        let request = CreateIssue {
            title,
            description,
            status: Status::New,
            component: holding_component,
            blocking_issues,
        };

        let id = api.create_issue(request)?;

        Ok(Issue { id, component, owner, cc_users })
    }
}

#[derive(Deserialize, Serialize)]
pub struct Issue {
    pub id: IssueId,
    pub component: Option<ComponentId>,
    pub owner: Option<String>,
    pub cc_users: Option<Vec<String>>,
}

impl Issue {
    pub fn rollout(
        mut issues: Vec<Self>,
        comment: Option<String>,
        api: &mut (impl Api + ?Sized),
        verbose: bool,
    ) -> Result<()> {
        let issues_len = issues.len();
        for (i, issue) in issues.drain(..).enumerate() {
            if verbose {
                println!("[{}/{issues_len}] Rolling out https://fxbug.dev/{}", i + 1, issue.id);
            }

            api.update_issue(UpdateIssue {
                id: issue.id,
                status: Some(Status::Assigned),
                owner: issue.owner,
                cc_users: issue.cc_users,
                component: issue.component,
                comment: comment.clone(),
            })?;
        }

        Ok(())
    }
}
