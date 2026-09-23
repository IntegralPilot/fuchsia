// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_driver_framework as fdf;

#[derive(Debug)]
pub enum BindResult {
    NotBound,
    Driver(String),
    Composite(Vec<fdf::CompositeParent>),
}

impl BindResult {
    pub fn is_bound(&self) -> bool {
        !matches!(self, BindResult::NotBound)
    }

    pub fn driver_url(&self) -> Option<&str> {
        if let BindResult::Driver(url) = self { Some(url) } else { None }
    }

    pub fn composite_parents(&self) -> Option<&[fdf::CompositeParent]> {
        if let BindResult::Composite(parents) = self { Some(parents) } else { None }
    }
}
