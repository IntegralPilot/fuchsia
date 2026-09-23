// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_ir::Ident;
use fidlgen::rust::{RustIdent, is_rust_keyword};

pub trait CompatRustIdent {
    fn compat_camel(&self) -> String;
    #[expect(unused)]
    fn compat_snake(&self) -> String;
}

impl CompatRustIdent for Ident {
    fn compat_camel(&self) -> String {
        let mut result = self.camel();
        if !result.ends_with('_') && is_reserved(self.non_canonical()) {
            result.push('_');
        }
        result
    }

    fn compat_snake(&self) -> String {
        let mut result = self.snake();
        if !result.ends_with('_') && is_reserved(self.non_canonical()) {
            result.push('_');
        }
        result
    }
}

fn is_reserved(name: &str) -> bool {
    is_rust_keyword(name) || RESERVED_SUFFIX_LIST.iter().any(|suffix| name.ends_with(suffix))
}

const RESERVED_SUFFIX_LIST: &[&str] =
    &["Impl", "Marker", "Proxy", "ProxyProtocol", "ControlHandle", "Responder", "Server"];
