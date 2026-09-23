// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! This crate defines attribute proc macros for writing tests with its sibling test-harness crate.
//! Specifically, marking fns which take `test-harness::TestHarness` with the macros in this crate
//! enables them to run with the standard Rust test runner. While `run_singlethreaded_test` is
//! currently the only attribute available, further attributes could be defined for other execution
//! environments.

use proc_macro::TokenStream;
use quote::{quote, quote_spanned};
use syn::{parse_macro_input, Error, Lit, Visibility};

fn validate_item_fn(sig: &syn::Signature, vis: &syn::Visibility) -> Result<(), syn::Error> {
    // Disallow const, unsafe or abi linkage, generics etc
    if let Some(c) = &sig.constness {
        return Err(Error::new(c.span, "test-harness tests may not be 'const'"));
    }
    if let Some(u) = &sig.unsafety {
        return Err(Error::new(u.span, "test-harness tests may not be 'unsafe'"));
    }
    if let Some(abi) = &sig.abi {
        return Err(Error::new(
            abi.extern_token.span,
            "test-harness test may not have custom linkage",
        ));
    }
    if !sig.generics.params.is_empty() || sig.generics.where_clause.is_some() {
        return Err(Error::new(sig.fn_token.span, "test-harness tests may not have generics"));
    }
    if sig.inputs.len() != 1 {
        return Err(Error::new(
            sig.paren_token.span.join(),
            "test-harness tests take exactly one argument, which must `impl TestHarness`",
        ));
    }
    if let Some(dot3) = &sig.variadic {
        return Err(Error::new(dot3.dots.spans[0], "test-harness tests may not be variadic"));
    }
    // Require the target function acknowledge it is async.
    if sig.asyncness.is_none() {
        return Err(Error::new(sig.ident.span(), "test-harness tests must be declared as 'async'"));
    }
    // The attributes defined in this crate purposefully mangle the names and visibility of the fns
    // to which they are applied. As such, they should not be applied to `pub` fns, which would
    // indicate that the client plans to use them elsewhere.
    if let Some(token_span) = match vis {
        Visibility::Public(pub_token) => Some(pub_token.span),
        Visibility::Restricted(restricted_vis) => Some(restricted_vis.pub_token.span),
        Visibility::Inherited => None,
    } {
        return Err(Error::new(token_span, "test-harness tests cannot be called elsewhere, so they must have inherited (i.e. non-pub) visibility"));
    }
    Ok(())
}

/// Used to run tests that require TestHarness types as inputs on a singlethreaded executor. This
/// attribute should be used instead of `#[test]`, not in addition to it.
///
/// e.g.
///
///     ```
///     impl TestHarness for SomeHarness {..}
///
///     #[test_harness::run_singlethreaded_test]
///     async fn test_foo(harness: SomeHarness) {
///         // use harness
///     }
///     ```
#[proc_macro_attribute]
pub fn run_singlethreaded_test(args: TokenStream, item: TokenStream) -> TokenStream {
    let mut test_component: Option<String> = None;
    let component_parser = syn::meta::parser(|meta| {
        if meta.path.is_ident("test_component") {
            if let Ok(value) = meta.value() {
                if let Ok(Lit::Str(lit_str)) = value.parse() {
                    test_component = Some(lit_str.value());
                }
            }
        }
        Ok(())
    });
    parse_macro_input!(args with component_parser);
    let test_component = match test_component {
        Some(component) => quote! {
            Some(String::from(#component))
        },
        None => quote! {
            None
        },
    };

    let item = parse_macro_input!(item as syn::ItemFn);
    let syn::ItemFn { attrs, sig, vis, block } = item;

    if let Err(e) = validate_item_fn(&sig, &vis) {
        return e.to_compile_error().into();
    }

    let inputs = sig.inputs;
    let span = sig.ident.span();
    let ident = sig.ident;
    let output = quote_spanned! {span=>
        // Preserve any original attributes.
        #(#attrs)* #[test]
        fn #ident () {
            // Note: `ItemFn::block` includes the function body braces. Do not
            // add additional braces (will break source code coverage analysis).
            // TODO(https://fxbug.dev/42157203): Try to improve the Rust compiler to ease
            // this restriction.
            async fn func(#inputs) #block
            let func = move |_| { ::test_harness::run_with_harness(func, #test_component) };
            ::test_harness::run_singlethreaded_test(func)
          }
    };
    output.into()
}
