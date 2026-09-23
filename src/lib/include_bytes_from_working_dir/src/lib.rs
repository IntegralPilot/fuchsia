// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use proc_macro2::{Literal, Span, TokenStream, TokenTree};
use std::env;
use std::path::Path;
use syn::{Error, LitStr, Result};

/// Imports a file's content as a reference to a byte array like [`include_bytes!`] does but looks
/// up files relative to where rustc is run from. Accepts a relative file path.
///
/// # Example
/// ```ignore
/// // Project root: If rustc is run from `/path/to/project`, and the file lives at
/// // `/path/to/project/out/gen/file.json`, then it will be included.
/// const JSON1 = include_bytes_from_working_dir_path!("out/gen/file.json");
///
/// // Build root: If rustc is run from `/path/to/project/out/build`, and the file lives at
/// // `/path/to/project/out/build/gen/file.json`, then it will be included.
/// const JSON2 = include_bytes_from_working_dir_path!("gen/file.json");
/// ```
///
/// This macro will yield an expression of type `&'static [u8; N]` which is the contents of the file.
#[proc_macro]
pub fn include_bytes_from_working_dir_path(
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    include_bytes_from_working_dir_path_impl(input.into())
        .unwrap_or_else(|err| err.to_compile_error())
        .into()
}

/// Imports a file's content as a string like [`include_bytes!`] does but looks up files relative to
/// where rustc is run from. Accepts an environment variable name.
///
/// # Example
/// ```ignore
/// // Project root: If rustc is run from `/path/to/project`, and the environment variable
/// // `JSON_PATH` contains the string "out/build/gen/file.json", then the file at
/// // `/path/to/project/out/build/gen/file.json` will be included.
/// // Note that only the env macro is supported.
/// const JSON1 = include_bytes_from_working_dir_env!("JSON_PATH");
///
/// // Build root: If rustc is run from `/path/to/project/out/build`, and the environment variable
/// // `JSON_PATH` contains the string "gen/file.json", then the file at
/// // `/path/to/project/out/build/gen/file.json` will be included.
/// const JSON2 = include_bytes_from_working_dir_env!("gen/file.json");
/// ```
#[proc_macro]
pub fn include_bytes_from_working_dir_env(
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    include_bytes_from_working_dir_env_impl(input.into())
        .unwrap_or_else(|err| err.to_compile_error())
        .into()
}

fn include_bytes_from_working_dir_path_impl(input: TokenStream) -> Result<TokenStream> {
    let lit_str: LitStr = syn::parse2(input)?;
    let path = lit_str.value();
    inner_impl(&path)
}

fn include_bytes_from_working_dir_env_impl(input: TokenStream) -> Result<TokenStream> {
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
    inner_impl(&path)
}

fn inner_impl(path: &str) -> Result<TokenStream> {
    let build_dir = env::var("FUCHSIA_BUILD_DIR").unwrap_or_else(|_| ".".to_string());
    let contents = std::fs::read(Path::new(&build_dir).join(path)).map_err(|err| {
        Error::new(Span::call_site(), format!("Unable to read file {path:?}: {err:?}"))
    })?;
    let tokens = TokenTree::Literal(Literal::byte_string(&contents)).into();
    Ok(tokens)
}
