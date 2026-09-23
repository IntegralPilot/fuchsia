// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use proc_macro::TokenStream;
use quote::quote;
use std::env;
use syn::{Error, LitStr, parse_macro_input};

#[proc_macro]
pub fn envpath(input: TokenStream) -> TokenStream {
    let lit = parse_macro_input!(input as LitStr);
    let var = lit.value();
    let val = match env::var(&var) {
        Ok(val) => val,
        Err(e) => {
            return Error::new(lit.span(), e).to_compile_error().into();
        }
    };

    let mut path = match env::current_dir() {
        Ok(cwd) => cwd,
        Err(e) => {
            return Error::new(lit.span(), e).to_compile_error().into();
        }
    };
    path.push(val);

    let path_str = match path.to_str() {
        Some(path_str) => path_str,
        None => {
            return Error::new(lit.span(), "Invalid UTF-8 in path").to_compile_error().into();
        }
    };

    let expanded = quote! {
        #path_str
    };
    TokenStream::from(expanded)
}
