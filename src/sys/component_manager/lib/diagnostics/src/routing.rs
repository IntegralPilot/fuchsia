// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_sync::Mutex;
use moniker::Moniker;
use {fuchsia_inspect as inspect, fuchsia_inspect_contrib as inspect_contrib};

const MAX_NUMBER_OF_ROUTING_ERRORS: usize = 100;

/// Holds the `BoundedListNode` for routing errors.
pub struct RoutingErrors {
    _node: inspect::Node,
    errors: Mutex<inspect_contrib::nodes::BoundedListNode>,
}

impl RoutingErrors {
    pub fn new(node: inspect::Node) -> Self {
        let errors = inspect_contrib::nodes::BoundedListNode::new(
            node.create_child("errors"),
            MAX_NUMBER_OF_ROUTING_ERRORS,
        );
        Self { _node: node, errors: Mutex::new(errors) }
    }

    pub fn record(
        &self,
        moniker: &Moniker,
        capability_name: &str,
        error: &str,
        availability: cm_types::Availability,
    ) {
        self.errors.lock().add_entry(|node| {
            node.record_string("moniker", moniker.to_string());
            node.record_string("capability_name", capability_name);
            node.record_string("error", error);
            node.record_string("availability", availability.to_string());
        });
    }
}
