// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use futures::channel::oneshot;
use {fidl_fuchsia_driver_development as fdd, fidl_fuchsia_driver_framework as fdf};

pub type NodeBindingInfoResultCompleter = oneshot::Sender<Vec<fdd::NodeBindingInfo>>;

#[derive(Debug)]
pub struct BindResultTracker {
    expected_result_count: usize,
    currently_reported: usize,
    result_completer: Option<NodeBindingInfoResultCompleter>,
    results: Vec<fdd::NodeBindingInfo>,
}

impl BindResultTracker {
    pub fn new(
        expected_result_count: usize,
        result_completer: NodeBindingInfoResultCompleter,
    ) -> Self {
        if expected_result_count == 0 {
            // If the receiver has been dropped, we don't need to do anything.
            let _ = result_completer.send(vec![]);
            return Self {
                expected_result_count,
                currently_reported: 0,
                result_completer: None,
                results: Vec::new(),
            };
        }
        Self {
            expected_result_count,
            currently_reported: 0,
            result_completer: Some(result_completer),
            results: Vec::new(),
        }
    }

    pub fn report_no_bind(&mut self) {
        self.currently_reported += 1;
        let current = self.currently_reported;
        self.complete(current);
    }

    pub fn report_successful_bind_driver(&mut self, node_name: &str, driver: &str) {
        self.currently_reported += 1;
        self.results.push(fdd::NodeBindingInfo {
            node_name: Some(node_name.to_string()),
            driver_url: Some(driver.to_string()),
            ..Default::default()
        });
        let current = self.currently_reported;
        self.complete(current);
    }

    pub fn report_successful_bind_composite(
        &mut self,
        node_name: &str,
        composite_parents: &[fdf::CompositeParent],
    ) {
        self.currently_reported += 1;
        self.results.push(fdd::NodeBindingInfo {
            node_name: Some(node_name.to_string()),
            composite_parents: Some(composite_parents.to_vec()),
            ..Default::default()
        });
        let current = self.currently_reported;
        self.complete(current);
    }

    fn complete(&mut self, current: usize) {
        if current == self.expected_result_count
            && let Some(completer) = self.result_completer.take()
        {
            // If the receiver has been dropped, we don't need to do anything.
            let _ = completer.send(std::mem::take(&mut self.results));
        }
    }
}
