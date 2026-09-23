// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! This crate provides an attribute macro to help instrument test functions
//! with setup and teardown logic.

use proc_macro::TokenStream;
use quote::quote;
use syn::spanned::Spanned as _;

fn fixture_inner(wrapper_fn: syn::Path, input: syn::ItemFn) -> Result<TokenStream, TokenStream> {
    let syn::ItemFn { attrs, sig, block, vis: _ } = input;
    let syn::Signature {
        ident: test_name,
        inputs,
        output,
        asyncness,
        constness,
        generics,
        unsafety: _,
        abi: _,
        fn_token: _,
        paren_token: _,
        variadic: _,
    } = sig;
    if let Some(constness) = constness {
        return Err(syn::Error::new(constness.span(), "test function cannot be const")
            .to_compile_error()
            .into());
    }
    let syn::Generics { lt_token: _, params, gt_token: _, where_clause: _ } = &generics;
    if !params.is_empty() {
        return Err(syn::Error::new(generics.span(), "test function cannot be generic")
            .to_compile_error()
            .into());
    }
    let (maybe_move, maybe_await) =
        if asyncness.is_some() { (Some(quote!(move)), Some(quote!(.await))) } else { (None, None) };

    let args = inputs
        .into_iter()
        .map(|input| match input {
            syn::FnArg::Receiver(receiver) => {
                Err(syn::Error::new(receiver.span(), "test function signature cannot contain self")
                    .to_compile_error()
                    .into())
            }
            syn::FnArg::Typed(arg) => Ok(arg),
        })
        .collect::<Result<Vec<_>, TokenStream>>()?;

    // TODO(https://fxbug.dev/42155980): make passing `test_name` to the wrapper function an optional
    // parameter on the #[fixture] macro.
    let result = quote! {
        #(#attrs)*
        #asyncness fn #test_name () #output {
            #wrapper_fn (stringify!(#test_name), |#( #args ),*| #asyncness #maybe_move
                #block
            ) #maybe_await
        }
    };
    Ok(result.into())
}

/// Wraps the body of a test `fn` in a closure, and passes it to a specified
/// helper `fn` to be run by the helper with any relevant setup and inputs that
/// it requires.
///
/// Example:
///
/// ```
/// # struct Foo;
///
/// async fn setup<F, Fut>(test_name: &str, test: F)
/// where
///     F: FnOnce(Foo) -> Fut,
///     Fut: std::future::Future<Output = ()>,
/// {
///     // setup foo
/// #   let foo = Foo{};
///     test(foo).await
/// }
///
/// #[fixture::fixture(setup)]
/// async fn test_foo(input: Foo) {
///     // test body using `input`
/// }
/// ```
///
/// Expands to:
///
/// ```
/// # struct Foo;
///
/// async fn setup<F, Fut>(test_name: &str, test: F)
/// # where
/// #     F: FnOnce(Foo) -> Fut,
/// #     Fut: std::future::Future<Output = ()>,
/// # {
/// #     // setup foo
/// #     let foo = Foo{};
/// #     test(foo).await
/// # }
///
/// async fn test_foo() {
///     setup("test_foo", |input| async move {
///         // test body using `input`
///     }).await
/// }
/// ```
///
/// This macro supports async as well as non-async functions, and any number of
/// inputs to the test body. For a non-async test, the wrapper function could
/// have a signature like this:
///
/// ```
/// fn setup(test_name: &str, test: impl FnOnce()) { /* ... */ }
/// ```
#[proc_macro_attribute]
pub fn fixture(attrs: TokenStream, input: TokenStream) -> TokenStream {
    let wrapper_fn = syn::parse_macro_input!(attrs as syn::Path);
    let input = syn::parse_macro_input!(input as syn::ItemFn);
    match fixture_inner(wrapper_fn, input) {
        Ok(token_stream) => token_stream,
        Err(token_stream) => token_stream,
    }
}

mod kw {
    syn::custom_keyword!(noasync);
}

struct TeardownArgs {
    teardown_fn: syn::Path,
    noasync: bool,
}

impl syn::parse::Parse for TeardownArgs {
    fn parse(input: syn::parse::ParseStream<'_>) -> syn::parse::Result<Self> {
        let teardown_fn = input.parse::<syn::Path>()?;
        let comma = input.parse::<Option<syn::token::Comma>>()?;
        let noasync = comma
            .and_then(|_| input.parse::<Option<kw::noasync>>().transpose())
            .transpose()?
            .is_some();
        Ok(Self { teardown_fn, noasync })
    }
}

fn teardown_inner(args: TeardownArgs, input: syn::ItemFn) -> Result<TokenStream, TokenStream> {
    let TeardownArgs { teardown_fn, noasync } = args;
    let syn::ItemFn { attrs, sig, block, vis } = input;

    // Use function asyncness for teardown or force sync through option.
    let add_await = sig.asyncness.is_some() && !noasync;

    let await_tokens = add_await.then(|| quote! {.await});

    let result = quote! {
        #(#attrs)*
        #vis #sig {
            let teardown_arg = #block;
            #teardown_fn (teardown_arg) #await_tokens
        }
    };
    Ok(result.into())
}

/// Runs a teardown function after a target function execution, giving the
/// teardown function the target function's return value.
///
/// Especially useful to perform teardown in asynchronous test functions.
///
///  Example:
///
/// ```
/// struct Foo{}
/// async fn teardown(foo: Foo) {
///     futures::future::ready(foo).await;
/// }
///
/// #[fixture::teardown(teardown)]
/// async fn foo_checker() {
///     Foo {}
/// }
/// ```
///
/// Expands to
///
/// ```
/// # struct Foo{}
/// async fn teardown(foo: Foo) {
/// #   futures::future::ready(foo).await;
/// }
///
/// async fn foo_checker() {
///     let teardown_result = {
///         Foo{}
///     };
///     teardown(teardown_result).await
/// }
/// ```
///
/// If the target function is `async`, the teardown function is assumed to be
/// async as well and thus a `.await` is added. This behavior can be overridden
/// by adding `, noasync` to the macro invocation.
///
/// Example:
///
/// ```
/// fn teardown(){}
/// #[fixture::teardown(teardown, noasync)]
/// async fn foo_checker() {
/// }
/// ```
#[proc_macro_attribute]
pub fn teardown(attrs: TokenStream, input: TokenStream) -> TokenStream {
    let args = syn::parse_macro_input!(attrs as TeardownArgs);
    let input = syn::parse_macro_input!(input as syn::ItemFn);
    match teardown_inner(args, input) {
        Ok(token_stream) => token_stream,
        Err(token_stream) => token_stream,
    }
}
