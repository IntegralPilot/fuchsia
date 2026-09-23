// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::fmt;

use fidl_ir::Attributes;

pub struct DocStringTemplate<'a> {
    attributes: &'a Attributes,
}

impl<'a> DocStringTemplate<'a> {
    pub fn new(attributes: &'a Attributes) -> Self {
        Self { attributes }
    }
}

impl fmt::Display for DocStringTemplate<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(doc) = self.attributes.get_value("doc") {
            write!(f, "#[doc = \"{}\"]", doc.escape_default())?;
        }

        Ok(())
    }
}
