// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#![allow(unused_crate_dependencies, unused_extern_crates)]
#![crate_type = "proc-macro"]
extern crate charlie;
extern crate proc_macro;
use proc_macro::TokenStream;
#[proc_macro]
pub fn make_constant(_item: TokenStream) -> TokenStream {
    format!("pub {} NOVEMBER: () = ();", charlie::CHARLIE).parse().unwrap()
}
