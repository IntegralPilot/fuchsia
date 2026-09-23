// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Stubs for rustversion in order to allow dependent crates to compile.
//! We can assume that we are always on the latest version of Rust, as we will
//! almost always be on a later version than the third_party crates.

extern crate proc_macro;

use proc_macro::TokenStream;

#[proc_macro_attribute]
pub fn before(_args: TokenStream, _input: TokenStream) -> TokenStream {
    TokenStream::new()
}

#[proc_macro_attribute]
pub fn since(_args: TokenStream, input: TokenStream) -> TokenStream {
    input
}

#[proc_macro_attribute]
pub fn nightly(_args: TokenStream, _input: TokenStream) -> TokenStream {
    TokenStream::new()
}

#[proc_macro_attribute]
pub fn not(args: TokenStream, input: TokenStream) -> TokenStream {
    if format!("{args}") == "nightly" {
        input
    } else {
        TokenStream::new()
    }
}
