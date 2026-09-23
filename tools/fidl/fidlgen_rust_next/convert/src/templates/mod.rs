// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod bits;
mod r#enum;

use askama::Template;
use fidl_ir::{Bits, DeclType, Enum};
use fidlgen::Denylist;

use self::bits::BitsTemplate;
use self::r#enum::EnumTemplate;
use crate::context::{Context, Contextual};

#[derive(Template)]
#[template(path = "library.askama")]
pub struct LibraryTemplate<'a> {
    context: &'a Context,
}

impl<'a> LibraryTemplate<'a> {
    pub fn new(context: &'a Context) -> Self {
        Self { context }
    }

    fn bits(&self, bits: &'a Bits) -> BitsTemplate<'a> {
        BitsTemplate::new(bits, self.context)
    }

    fn enm(&self, enm: &'a Enum) -> EnumTemplate<'a> {
        EnumTemplate::new(enm, self.context)
    }
}

impl Contextual for LibraryTemplate<'_> {
    fn context(&self) -> &Context {
        self.context
    }
}
