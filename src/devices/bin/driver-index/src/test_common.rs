// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_driver_framework as fdf;

#[cfg(test)]
pub fn make_accept(key: &str, value: fdf::NodePropertyValue) -> fdf::BindRule2 {
    fdf::BindRule2 { key: key.to_string(), condition: fdf::Condition::Accept, values: vec![value] }
}

#[cfg(test)]
pub fn make_accept_list(key: &str, values: Vec<fdf::NodePropertyValue>) -> fdf::BindRule2 {
    fdf::BindRule2 { key: key.to_string(), condition: fdf::Condition::Accept, values }
}

#[cfg(test)]
pub fn make_parent_spec(
    bind_rules: Vec<fdf::BindRule2>,
    properties: Vec<fdf::NodeProperty2>,
) -> fdf::ParentSpec2 {
    fdf::ParentSpec2 { bind_rules, properties }
}

#[cfg(test)]
pub fn make_composite_spec(
    spec_name: &str,
    parents: Vec<fdf::ParentSpec2>,
) -> fdf::CompositeNodeSpec {
    fdf::CompositeNodeSpec {
        name: Some(spec_name.to_string()),
        parents2: Some(parents),
        ..Default::default()
    }
}

#[cfg(test)]
pub fn make_property(key: &str, value: fdf::NodePropertyValue) -> fdf::NodeProperty2 {
    fdf::NodeProperty2 { key: key.to_string(), value }
}
