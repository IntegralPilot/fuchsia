// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use proc_macro::TokenStream;
use syn::{ItemStruct, parse_macro_input};

#[proc_macro_attribute]
pub fn ffx_command(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as ItemStruct);
    TokenStream::from(ffx_core_impl::ffx_command(input))
}
