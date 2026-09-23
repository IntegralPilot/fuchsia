// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_driver_framework as fdf;

// Note: We use `to_...` functions instead of implementing `Into` or `From` traits
// because of Rust's orphan rule. Since `fdf` types are not defined in the current crate,
// we cannot implement foreign traits for foreign types. A newtype wrapper could be used to
// work around this, but simple conversion functions suffice for now.

pub fn to_property2(property: &fdf::NodeProperty) -> fdf::NodeProperty2 {
    let key = match &property.key {
        fdf::NodePropertyKey::StringValue(s) => s.clone(),
        _ => panic!("Integer keys are deprecated"),
    };
    fdf::NodeProperty2 { key, value: property.value.clone() }
}

pub fn to_deprecated_property(property: &fdf::NodeProperty2) -> fdf::NodeProperty {
    fdf::NodeProperty {
        key: fdf::NodePropertyKey::StringValue(property.key.clone()),
        value: property.value.clone(),
    }
}

pub fn to_bind_rule2(bind_rule: &fdf::BindRule) -> fdf::BindRule2 {
    let key = match &bind_rule.key {
        fdf::NodePropertyKey::StringValue(s) => s.clone(),
        _ => panic!("Integer keys are deprecated"),
    };
    fdf::BindRule2 { key, condition: bind_rule.condition, values: bind_rule.values.clone() }
}
