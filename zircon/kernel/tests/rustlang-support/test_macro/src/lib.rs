// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use proc_macro::TokenStream;

#[proc_macro]
pub fn plus_one(input: TokenStream) -> TokenStream {
    let mut tokens = input;
    let plus1: TokenStream = "+ 1".parse().unwrap();
    tokens.extend(plus1);
    tokens
}
