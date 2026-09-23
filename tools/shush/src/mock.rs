// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;

use crate::api::{Api, Component, CreateIssue, IssueId, UpdateIssue};

pub struct Mock {
    next_issue_id: usize,
    log_api: bool,
    read_only: Option<Box<dyn Api>>,
}

impl Mock {
    pub fn new(log_api: bool, read_only: Option<Box<dyn Api>>) -> Self {
        Self { next_issue_id: 0, log_api, read_only }
    }
}

impl Api for Mock {
    fn create_issue(&mut self, request: CreateIssue) -> Result<IssueId> {
        if self.log_api {
            println!("[mock] Filing issue {}:\n{:#?}", self.next_issue_id, request);
        }

        let id = IssueId::new(self.next_issue_id);
        self.next_issue_id += 1;

        Ok(id)
    }

    fn update_issue(&mut self, request: UpdateIssue) -> Result<()> {
        if self.log_api {
            println!("[mock] Updating issues:\n{:#?}", request);
        }

        Ok(())
    }

    fn list_components(&mut self) -> Result<Vec<Component>> {
        if self.log_api {
            println!("[mock] Listing component defs");
        }

        if let Some(read_only) = self.read_only.as_mut() {
            let defs = read_only.list_components()?;

            if self.log_api {
                println!("[mock] Readonly API returned component defs:");
                println!("{defs:?}");
            }

            Ok(defs)
        } else {
            Ok(Vec::new())
        }
    }
}
