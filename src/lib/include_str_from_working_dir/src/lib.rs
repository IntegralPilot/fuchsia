// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use proc_macro2::{Span, TokenStream};
use quote::quote;
use std::path::Path;
use std::{env, fs};
use syn::{Error, LitStr, Result};

/// Imports a file's content as a string like [`include_str!`] does but looks up files relative to
/// where rustc is run from. Accepts a relative file path.
///
/// # Example
/// ```ignore
/// // Project root: If rustc is run from `/path/to/project`, and the file lives at
/// // `/path/to/project/out/gen/file.json`, then it will be included.
/// const JSON1: &str = include_str_from_working_dir_path!("out/gen/file.json");
///
/// // Build root: If rustc is run from `/path/to/project/out/build`, and the file lives at
/// // `/path/to/project/out/build/gen/file.json`, then it will be included.
/// const JSON2: &str = include_str_from_working_dir_path!("gen/file.json");
/// ```
#[proc_macro]
pub fn include_str_from_working_dir_path(
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    include_str_from_working_dir_path_impl(input.into())
        .unwrap_or_else(|err| err.to_compile_error())
        .into()
}

/// Imports a file's content as a string like [`include_str!`] does but looks up files relative to
/// where rustc is run from. Accepts an environment variable name.
///
/// # Example
/// ```ignore
/// // Project root: If rustc is run from `/path/to/project`, and the environment variable
/// // `JSON_PATH` contains the string "out/build/gen/file.json", then the file at
/// // `/path/to/project/out/build/gen/file.json` will be included.
/// // Note that only the env macro is supported.
/// const JSON1: &str = include_str_from_working_dir_env!("JSON_PATH");
///
/// // Build root: If rustc is run from `/path/to/project/out/build`, and the environment variable
/// // `JSON_PATH` contains the string "gen/file.json", then the file at
/// // `/path/to/project/out/build/gen/file.json` will be included.
/// const JSON2: &str = include_str_from_working_dir_env!("JSON_PATH");
/// ```
#[proc_macro]
pub fn include_str_from_working_dir_env(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    include_str_from_working_dir_env_impl(input.into())
        .unwrap_or_else(|err| err.to_compile_error())
        .into()
}

fn read_file(path: &str) -> Result<TokenStream> {
    let build_dir = env::var("FUCHSIA_BUILD_DIR").unwrap_or_else(|_| ".".to_string());
    let contents = fs::read_to_string(Path::new(&build_dir).join(path)).map_err(|err| {
        Error::new(Span::call_site(), format!("Unable to read file {path:?}: {err:?}"))
    })?;
    Ok(quote! { #contents })
}

fn include_str_from_working_dir_path_impl(input: TokenStream) -> Result<TokenStream> {
    let lit_str: LitStr = syn::parse2(input)?;
    let path = lit_str.value();
    let contents = read_file(&path)?;
    Ok(quote! { #contents })
}

fn include_str_from_working_dir_env_impl(input: TokenStream) -> Result<TokenStream> {
    let env_var: LitStr = syn::parse2(input)?;
    let path = match env::var(env_var.value()) {
        Ok(path) => path,
        Err(err) => {
            return Err(Error::new(
                env_var.span(),
                format!("Invalid env var {:?}: {err:?}", env_var.value()),
            ));
        }
    };
    let contents = read_file(&path)?;
    Ok(quote! { #contents })
}
