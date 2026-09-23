// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use proc_macro::TokenStream;

#[cfg(not(rustflags_work))]
compile_error!("rustflags attribute not correctly passed!");

const RUSTENV_VAL: &str = env!("RUSTENV_TEST_VAR");

#[proc_macro]
pub fn make_string(_input: TokenStream) -> TokenStream {
    format!("\"{}\"", RUSTENV_VAL).parse().unwrap()
}
