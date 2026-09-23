// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use proc_macro::TokenStream;
use transformer::{Finish, Transformer};

/// Define a fuchsia DSO main function.
///
/// This attribute should be applied to the process `main` function.
/// It will take care of setting up various Fuchsia crates for the component.
///
/// Arguments:
///  - `sync` - boolean toggle for whether to use the synchronous DSO entry point.
///  - `async` - boolean toggle for whether to use the asynchronous DSO entry point.
///
/// The main function can return either `()` or a `Result<(), E>` where `E` is an error type.
#[proc_macro_attribute]
pub fn main(args: TokenStream, input: TokenStream) -> TokenStream {
    Transformer::parse_main(args.into(), input.into()).finish().into()
}
