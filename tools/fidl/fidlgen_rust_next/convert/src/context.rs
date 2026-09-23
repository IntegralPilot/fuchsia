// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_ir::{CompoundIdent, Library};
use fidlgen::{Denylist, LibraryExt as _};

pub struct Context {
    pub library: Library,
    pub rust_crate: String,
    pub rust_next_crate: String,
}

pub trait Contextual {
    fn context(&self) -> &Context;

    // Helpers

    fn library(&self) -> &Library {
        &self.context().library
    }

    fn rust_crate(&self) -> &str {
        &self.context().rust_crate
    }

    fn rust_next_crate(&self) -> &str {
        &self.context().rust_next_crate
    }

    fn denylist(&self, ident: &CompoundIdent) -> Denylist {
        self.context().library.denylist_for(ident, &["rust", "rust_next"])
    }
}

impl Contextual for Context {
    fn context(&self) -> &Context {
        self
    }
}
