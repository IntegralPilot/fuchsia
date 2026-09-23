// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::fmt;

use anyhow::Result;
use serde::{Deserialize, Serialize};

pub trait Api {
    fn create_issue(&mut self, request: CreateIssue) -> Result<IssueId>;
    fn update_issue(&mut self, request: UpdateIssue) -> Result<()>;
    fn list_components(&mut self) -> Result<Vec<Component>>;
}

impl Api for &mut dyn Api {
    fn create_issue(&mut self, request: CreateIssue) -> Result<IssueId> {
        (*self).create_issue(request)
    }

    fn update_issue(&mut self, request: UpdateIssue) -> Result<()> {
        (*self).update_issue(request)
    }

    fn list_components(&mut self) -> Result<Vec<Component>> {
        (*self).list_components()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct IssueId(usize);

impl IssueId {
    pub fn new(id: usize) -> Self {
        Self(id)
    }
}

impl fmt::Display for IssueId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[allow(dead_code)]
#[derive(Clone, Copy, Debug)]
pub enum Status {
    New,
    Assigned,
    Accepted,
    Fixed,
    Verified,
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::New => "NEW",
            Self::Assigned => "ASSIGNED",
            Self::Accepted => "ACCEPTED",
            Self::Fixed => "FIXED",
            Self::Verified => "Verified",
        };
        write!(f, "{s}")
    }
}

#[derive(Debug)]
pub struct CreateIssue {
    pub title: String,
    pub description: String,
    pub status: Status,
    pub component: ComponentId,
    pub blocking_issues: Vec<IssueId>,
}

impl CreateIssue {
    pub fn to_bugspec(&self) -> String {
        use core::fmt::Write;

        let mut result = String::new();

        writeln!(&mut result, "{}\n", self.title).unwrap();
        writeln!(&mut result, "{}\n", self.description).unwrap();
        writeln!(&mut result, "STATUS={}", self.status).unwrap();
        writeln!(&mut result, "COMPONENT={}", self.component).unwrap();
        if !self.blocking_issues.is_empty() {
            write!(&mut result, "BLOCKED_BY+={}", self.blocking_issues.first().unwrap()).unwrap();
            for issue in self.blocking_issues.iter().skip(1) {
                write!(&mut result, ",{}", issue).unwrap();
            }
            writeln!(&mut result).unwrap();
        }

        result
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct ComponentId(usize);

impl ComponentId {
    pub fn new(id: usize) -> Self {
        Self(id)
    }
}

impl fmt::Display for ComponentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug)]
pub struct Component {
    pub id: ComponentId,
    pub path: Vec<String>,
}

#[derive(Debug)]
pub struct UpdateIssue {
    pub id: IssueId,
    pub status: Option<Status>,
    pub owner: Option<String>,
    pub cc_users: Option<Vec<String>>,
    pub component: Option<ComponentId>,
    pub comment: Option<String>,
}

impl UpdateIssue {
    pub fn to_bugspec(&self) -> String {
        use core::fmt::Write;

        let mut result = String::new();

        if let Some(comment) = &self.comment {
            writeln!(&mut result, "{}\n", comment).unwrap();
        }

        if let Some(status) = &self.status {
            writeln!(&mut result, "STATUS={}", status).unwrap();
        }
        if let Some(owner) = &self.owner {
            writeln!(&mut result, "ASSIGNEE={}", owner).unwrap();
        }
        if let Some(cc_users) = &self.cc_users
            && !cc_users.is_empty()
        {
            write!(&mut result, "CC+={}", cc_users.iter().next().unwrap()).unwrap();
            for cc_user in cc_users.iter().skip(1) {
                write!(&mut result, ",{}", cc_user).unwrap();
            }
            writeln!(&mut result).unwrap();
        }
        if let Some(component) = &self.component {
            writeln!(&mut result, "COMPONENT={}", component).unwrap();
        }

        result
    }
}
