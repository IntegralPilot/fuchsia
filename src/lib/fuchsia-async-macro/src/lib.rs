// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Procedural macro crate for simplifying writing async entry points
//!
//! This crate should not be depended upon directly, but should be used through
//! the fuchsia_async crate, which re-exports all these symbols.
//!
//! Program entry points, by they `main` or `#[test]` functions, that want to
//! perform asynchronous actions typically involve writing a shim such as:
//!
//! ```
//! fn main() -> Result<(), Error> {
//!     let mut executor = fasync::LocalExecutor::default();
//!     let actual_main = async {
//!         // Actual main code
//!         Ok(())
//!     };
//!     executor.run_singlethreaded(actual_main())
//! }
//! ```
//!
//! or
//!
//! ```
//! #[test]
//! fn test_foo() -> Result<(), Error> {
//!     let mut executor = fasync::LocalExecutor::default();
//!     let test = async {
//!         // Actual test code here
//!         Ok(())
//!     };
//!     let mut test_fut = test();
//!     pin_mut!(test_fut);
//!     match executor.run_until_stalled(&mut test_fut) {
//!         Poll::Pending => panic!("Test blocked"),
//!         Poll::Ready(x) => x,
//!     }
//! }
//! ```
//!
//! This crate defines attributes that allow for writing the above as just
//!
//! ```
//! #[fuchsia_async::run_singlethreaded]
//! async fn main() -> Result<(), anyhow::Error> {
//!     // Actual main code
//!     Ok(())
//! }
//! ```
//!
//! or
//!
//! ```
//!
//! #[fuchsia_async::run_until_stalled(test)]
//! async fn test_foo() -> Result<(), Error> {
//!     // Actual test code here
//!     Ok(())
//! }
//! ```
//!
//! Using the optional 'test' specifier is preferred to using `#[test]`, as spurious
//! compilation errors will be generated should `#[test]` be specified before
//! `#[fuchsia_async::run_until_stalled]`.

use proc_macro::TokenStream;
use proc_macro2::Span;
use quote::{quote, quote_spanned};
use syn::parse::{Error, Parse, ParseStream};
use syn::{Ident, parse_macro_input};

mod kw {
    syn::custom_keyword!(test);
}

fn executor_ident() -> Ident {
    Ident::new("executor", Span::call_site())
}

fn common(item: TokenStream, run_executor: TokenStream, test: bool) -> TokenStream {
    let item = parse_macro_input!(item as syn::ItemFn);
    let syn::ItemFn { attrs, sig, vis, block } = item;
    if let Err(e) = (|| {
        // Disallow const, unsafe or abi linkage, generics etc
        if let Some(c) = &sig.constness {
            return Err(Error::new(c.span, "async entry may not be 'const'"));
        }
        if let Some(u) = &sig.unsafety {
            return Err(Error::new(u.span, "async entry may not be 'unsafe'"));
        }
        if let Some(abi) = &sig.abi {
            return Err(Error::new(
                abi.extern_token.span,
                "async entry may not have custom linkage",
            ));
        }
        if !sig.generics.params.is_empty() || sig.generics.where_clause.is_some() {
            return Err(Error::new(sig.fn_token.span, "async entry may not have generics"));
        }
        if !sig.inputs.is_empty() && !test {
            return Err(Error::new(sig.paren_token.span.join(), "async entry takes no arguments"));
        }
        if let Some(dot3) = &sig.variadic {
            return Err(Error::new(dot3.dots.spans[0], "async entry may not be variadic"));
        }

        // Require the target function acknowledge it is async.
        if sig.asyncness.is_none() {
            return Err(Error::new(sig.ident.span(), "async entry must be declared as 'async'"));
        }

        // Only allow on 'main' or 'test' functions
        if sig.ident != "main" && !test && !attrs.iter().any(|a| a.path().is_ident("test")) {
            return Err(Error::new(
                sig.ident.span(),
                "async entry must be named 'main' or be a '#[test]'.",
            ));
        }
        Ok(())
    })() {
        return e.to_compile_error().into();
    }

    let adapt_func = if test && sig.inputs.is_empty() {
        quote! {let func = move |_| func();}
    } else {
        quote! {}
    };
    let test = if test {
        quote! {#[test]}
    } else {
        quote! {}
    };
    let run_executor = proc_macro2::TokenStream::from(run_executor);
    let ret_type = sig.output;
    let inputs = sig.inputs;
    let span = sig.ident.span();
    let ident = sig.ident;
    let output = quote_spanned! {span=>
        // Preserve any original attributes.
        #(#attrs)* #test
        #vis fn #ident () #ret_type {
            // Note: `ItemFn::block` includes the function body braces. Do not add
            // additional braces (will break source code coverage analysis).
            // TODO(https://fxbug.dev/42157203): Try to improve the Rust compiler to ease
            // this restriction.
            async fn func(#inputs) #ret_type #block
            #adapt_func

            #run_executor
          }
    };
    output.into()
}

/// Define an `async` function that should complete without stalling.
///
/// If the async function should stall then a `panic` will be raised. For example:
///
/// ```
/// #[fuchsia_async::run_until_stalled]
/// async fn this_will_fail_and_not_block() -> Result<(), anyhow::Error> {
///     let () = future::empty().await;
///     Ok(())
/// }
/// ```
///
/// will cause an immediate panic instead of hanging.
///
/// This is mainly intended for testing, and takes an optional `test` argument.
///
/// ```
/// #[fuchsia_async::run_until_stalled(test)]
/// async fn test_foo() {}
/// ```
#[proc_macro_attribute]
pub fn run_until_stalled(attr: TokenStream, item: TokenStream) -> TokenStream {
    let test = parse_macro_input!(attr as Option<kw::test>).is_some();
    let executor = executor_ident();
    let run_executor = if test {
        quote! {
            ::fuchsia_async::test_support::run_until_stalled_test(true, func, ::fuchsia_async::test_support::TestOptions::default())
        }
    } else {
        quote! {
            let mut #executor = ::fuchsia_async::TestExecutorBuilder::new().fake_time(true).build();
            let mut fut = ::std::pin::pin!(func());
            match #executor.run_until_stalled(&mut fut) {
                ::core::task::Poll::Ready(result) => result,
                _ => panic!("Stalled without completing. Consider using \"run_singlethreaded\", \
                             or check for a deadlock."),
            }
        }
    };
    common(item, run_executor.into(), test)
}

/// Define an `async` function that should run to completion on a single thread.
///
/// Takes an optional `test` argument.
///
/// Tests written using this macro can be run repeatedly.
/// The environment variable `FASYNC_TEST_REPEAT_COUNT` sets the number of repetitions each test
/// will be run for, while the environment variable `FASYNC_TEST_MAX_CONCURRENCY` bounds the
/// maximum concurrent invocations of each test. If `FASYNC_TEST_MAX_CONCURRENCY` is set to 0 (the
/// default) no bounds to concurrency are applied.
/// If FASYNC_TEST_TIMEOUT_SECONDS is set, it specifies the maximum duration for one repetition of
/// a test.
/// Multiple threads will be spawned to execute the tests concurrently (to save wall time), up to a maximum of
/// FASYNC_TEST_MAX_THREADS.
///
/// ```
/// #[fuchsia_async::run_singlethreaded(test)]
/// async fn test_foo() {}
/// ```
///
/// The test can optionally take a usize argument which specifies which repetition of the test is
/// being performed:
///
/// ```
/// #[fuchsia_async::run_singlethreaded(test)]
/// async fn test_foo(test_run: usize) {
///   println!("Test repetition #{}", test_run);
/// }
/// ```
#[proc_macro_attribute]
pub fn run_singlethreaded(attr: TokenStream, item: TokenStream) -> TokenStream {
    let test = parse_macro_input!(attr as Option<kw::test>).is_some();
    let run_executor = if test {
        quote! {
            ::fuchsia_async::test_support::run_singlethreaded_test(func, ::fuchsia_async::test_support::TestOptions::default())
        }
    } else {
        quote! {
            ::fuchsia_async::LocalExecutorBuilder::new().build().run_singlethreaded(func())
        }
    };
    common(item, run_executor.into(), test)
}

struct RunAttributes {
    threads: u8,
    test: bool,
}

impl Parse for RunAttributes {
    fn parse(input: ParseStream<'_>) -> syn::parse::Result<Self> {
        let threads = input.parse::<syn::LitInt>()?.base10_parse::<u8>()?;
        let comma = input.parse::<Option<syn::Token![,]>>()?.is_some();
        let test = if comma { input.parse::<Option<kw::test>>()?.is_some() } else { false };
        Ok(RunAttributes { threads, test })
    }
}

/// Define an `async` function that should run to completion on `N` threads.
///
/// Number of threads is configured by `#[fuchsia_async::run(N)]`, and can also
/// take an optional `test` argument.
///
/// Tests written using this macro can be run repeatedly.
/// The environment variable `FASYNC_TEST_REPEAT_COUNT` sets the number of repetitions each test
/// will be run for, while the environment variable `FASYNC_TEST_MAX_CONCURRENCY` bounds the
/// maximum concurrent invocations of each test. If `FASYNC_TEST_MAX_CONCURRENCY` is set to 0 (the
/// default) no bounds to concurrency are applied.
/// If FASYNC_TEST_TIMEOUT_SECONDS is set, it specifies the maximum duration for one repetition of
/// a test.
/// When running tests concurrently the thread pool size will be scaled up by the expected maximum concurrent
/// test executions (to save wall time) - this pool size can be capped with FASYNC_TEST_MAX_THREADS.
///
/// ```
/// #[fuchsia_async::run(4, test)]
/// async fn test_foo() {}
/// ```
///
/// The test can optionally take a usize argument which specifies which repetition of the test is
/// being performed:
///
/// ```
/// #[fuchsia_async::run(4, test)]
/// async fn test_foo(test_run: usize) {
///   println!("Test repetition #{}", test_run);
/// }
/// ```
#[proc_macro_attribute]
pub fn run(attr: TokenStream, item: TokenStream) -> TokenStream {
    let RunAttributes { threads, test } = parse_macro_input!(attr as RunAttributes);

    let run_executor = if test {
        quote! {
            ::fuchsia_async::test_support::run_test(func, #threads, ::fuchsia_async::test_support::TestOptions::default())
        }
    } else {
        quote! {
            ::fuchsia_async::SendExecutorBuilder::new().num_threads(#threads).build().run(func())
        }
    };
    common(item, run_executor.into(), test)
}
