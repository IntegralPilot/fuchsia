// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[cfg(fuchsia_api_level_less_than = "27")]
use fidl_fuchsia_diagnostics::Severity;
#[cfg(fuchsia_api_level_at_least = "27")]
use fidl_fuchsia_diagnostics_types::Severity;
use proc_macro2::TokenStream;
use quote::{quote, quote_spanned};
use syn::parse::{Parse, ParseStream, Parser};
use syn::punctuated::Punctuated;
use syn::{
    Attribute, Block, Error, Expr, Ident, ItemFn, LitBool, LitStr, Signature, Token, Visibility,
};

#[derive(Clone, Copy)]
enum FunctionType {
    Component,
    Test,
}

// How should code be executed?
#[derive(Clone)]
enum Executor {
    // Directly by calling it
    None { thread_role: Option<Box<Expr>> },
    // fasync::run_singlethreaded
    Singlethreaded { thread_role: Option<Box<Expr>>, instrumentation: bool },
    // fasync::run
    Multithreaded { threads: Box<Expr>, thread_role: Option<Box<Expr>>, instrumentation: bool },
    // #[test]
    Test,
    // fasync::run_singlethreaded(test)
    SinglethreadedTest { allow_interrupts: bool },
    // fasync::run(test)
    MultithreadedTest { threads: Box<Expr>, allow_interrupts: bool },
    // fasync::run_until_stalled
    UntilStalledTest { allow_interrupts: bool },
}

fn build_instrumentation(instrumentation: bool) -> (TokenStream, TokenStream) {
    if instrumentation {
        (
            quote! {
                let config = ::fuchsia_async_inspect::InspectTaskConfiguration::new(::fuchsia_async_inspect::default_root());
                let instrument = ::fuchsia_async_inspect::InspectTaskInstrument::new(config);
            },
            quote! { Some(instrument) },
        )
    } else {
        (quote! {}, quote! { None })
    }
}

impl Executor {
    fn is_test(&self) -> bool {
        match self {
            Executor::Test
            | Executor::SinglethreadedTest { .. }
            | Executor::MultithreadedTest { .. }
            | Executor::UntilStalledTest { .. } => true,
            Executor::None { .. }
            | Executor::Singlethreaded { .. }
            | Executor::Multithreaded { .. } => false,
        }
    }

    fn is_some(&self) -> bool {
        !matches!(self, Executor::Test | Executor::None { .. })
    }

    fn build_token_stream(&self, func: &syn::Ident) -> TokenStream {
        let (instrumentation, executor_new) = match self {
            Executor::None { thread_role } => {
                if let Some(role) = thread_role {
                    (quote! {}, quote! { ::fuchsia::main_not_async_with_role(#func, #role) })
                } else {
                    (quote! {}, quote! { ::fuchsia::main_not_async(#func) })
                }
            }
            Executor::Test => (quote! {}, quote! { ::fuchsia::test_not_async(#func) }),
            Executor::Singlethreaded { thread_role, instrumentation } => {
                let (instrumentation_code, instrument_arg) =
                    build_instrumentation(*instrumentation);
                if let Some(role) = thread_role {
                    (
                        instrumentation_code,
                        quote! { ::fuchsia::main_singlethreaded_with_role(#func, #role, #instrument_arg) },
                    )
                } else {
                    (
                        instrumentation_code,
                        quote! { ::fuchsia::main_singlethreaded(#func, #instrument_arg) },
                    )
                }
            }
            Executor::Multithreaded { threads, thread_role, instrumentation } => {
                let (instrumentation_code, instrument_arg) =
                    build_instrumentation(*instrumentation);
                if let Some(role) = thread_role {
                    (
                        instrumentation_code,
                        quote! { ::fuchsia::main_multithreaded_with_role(#func, #threads, #role, #instrument_arg) },
                    )
                } else {
                    (
                        instrumentation_code,
                        quote! { ::fuchsia::main_multithreaded(#func, #threads, #instrument_arg) },
                    )
                }
            }
            Executor::SinglethreadedTest { allow_interrupts } => (
                quote! {},
                quote! {
                    ::fuchsia::test_singlethreaded(
                        #func,
                        ::fuchsia::TestOptions { allow_interrupts: #allow_interrupts },
                    )
                },
            ),
            Executor::MultithreadedTest { threads, allow_interrupts } => (
                quote! {},
                quote! {
                    ::fuchsia::test_multithreaded(
                        #func,
                        #threads,
                        ::fuchsia::TestOptions { allow_interrupts: #allow_interrupts },
                    )
                },
            ),
            Executor::UntilStalledTest { allow_interrupts } => (
                quote! {},
                quote! {
                    ::fuchsia::test_until_stalled(
                        #func,
                        ::fuchsia::TestOptions { allow_interrupts: #allow_interrupts },
                    )
                },
            ),
        };
        quote! {{
            #instrumentation
            #executor_new
        }}
    }
}

// Helper trait for things that can generate the final token stream
pub trait Finish {
    fn finish(self) -> TokenStream
    where
        Self: Sized;
}

pub struct Transformer {
    executor: Executor,
    attrs: Vec<Attribute>,
    vis: Visibility,
    sig: Signature,
    block: Box<Block>,
    logging: Option<bool>,
    logging_tags: Punctuated<LitStr, Token![,]>,
    logging_include_file_line: bool,
    panic_prefix: LitStr,
    interest: Interest,
    add_test_attr: bool,
}

struct Args {
    threads: Option<Expr>,
    thread_role: Option<Expr>,
    allow_stalls: Option<bool>,
    logging: Option<bool>,
    logging_tags: Punctuated<LitStr, Token![,]>,
    logging_include_file_line: bool,
    interest: Interest,
    panic_prefix: Option<LitStr>,
    add_test_attr: bool,
    instrumentation: bool,
    allow_interrupts: bool,
}

#[derive(Default)]
struct Interest {
    min_severity: Option<Severity>,
}

impl Parse for Interest {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let str_token = input.parse::<LitStr>()?;
        let min_severity = match str_token.value().to_lowercase().as_str() {
            "trace" => Severity::Trace,
            "debug" => Severity::Debug,
            "info" => Severity::Info,
            "warn" => Severity::Warn,
            "error" => Severity::Error,
            "fatal" => Severity::Fatal,
            other => {
                return Err(syn::Error::new(
                    str_token.span(),
                    format!("invalid severity: {}", other),
                ));
            }
        };
        Ok(Interest { min_severity: Some(min_severity) })
    }
}

impl quote::ToTokens for Interest {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        tokens.extend(match self.min_severity {
            None => quote! { ::fuchsia::Interest::default() },
            Some(severity) => {
                let severity_tok = match severity {
                    Severity::Trace => quote!(::fuchsia::Severity::Trace),
                    Severity::Debug => quote!(::fuchsia::Severity::Debug),
                    Severity::Info => quote!(::fuchsia::Severity::Info),
                    Severity::Warn => quote!(::fuchsia::Severity::Warn),
                    Severity::Error => quote!(::fuchsia::Severity::Error),
                    Severity::Fatal => quote!(::fuchsia::Severity::Fatal),
                    #[cfg(fuchsia_api_level_at_least = "27")]
                    Severity::__SourceBreaking { unknown_ordinal: o } => {
                        panic!("unknown severity type with ordinal: {o:?}")
                    }
                };
                quote! {
                    ::fuchsia::Interest {
                        min_severity: Some(#severity_tok),
                        ..Default::default()
                    }
                }
            }
        });
    }
}

fn get_arg<T: Parse>(p: &ParseStream<'_>) -> syn::Result<T> {
    p.parse::<Token![=]>()?;
    p.parse()
}

fn get_bool_arg(p: &ParseStream<'_>, if_present: bool) -> syn::Result<bool> {
    if p.peek(Token![=]) { Ok(get_arg::<LitBool>(p)?.value) } else { Ok(if_present) }
}

fn get_logging_tags(p: &ParseStream<'_>) -> syn::Result<Punctuated<LitStr, Token![,]>> {
    p.parse::<Token![=]>()?;
    let content;
    syn::bracketed!(content in p);
    Punctuated::parse_terminated(&content)
}

fn get_interest_arg(input: &ParseStream<'_>) -> syn::Result<Interest> {
    input.parse::<Token![=]>()?;
    input.parse::<Interest>()
}

impl Args {
    fn parse(input: TokenStream) -> syn::Result<Self> {
        let mut args = Self {
            threads: None,
            thread_role: None,
            allow_stalls: None,
            logging: None,
            logging_tags: Default::default(),
            logging_include_file_line: false,
            panic_prefix: None,
            interest: Interest::default(),
            add_test_attr: true,
            instrumentation: false,
            allow_interrupts: false,
        };

        let arg_parser = syn::meta::parser(|meta| {
            let ident =
                meta.path.get_ident().ok_or_else(|| meta.error("arguments must have a key"))?;
            match ident.to_string().as_ref() {
                "threads" => args.threads = Some(get_arg::<Expr>(&meta.input)?),
                "thread_role" => args.thread_role = Some(get_arg::<Expr>(&meta.input)?),
                "allow_stalls" => args.allow_stalls = Some(get_bool_arg(&meta.input, true)?),
                "logging" => args.logging = Some(get_bool_arg(&meta.input, true)?),
                "logging_tags" => {
                    args.logging = Some(true);
                    args.logging_tags = get_logging_tags(&meta.input)?;
                }
                "always_log_file_line" => {
                    args.logging = Some(true);
                    args.logging_include_file_line = get_bool_arg(&meta.input, true)?;
                }
                "logging_minimum_severity" => {
                    args.logging = Some(true);
                    args.interest = get_interest_arg(&meta.input)?;
                }
                "logging_panic_prefix" => {
                    args.logging = Some(true);
                    args.panic_prefix = Some(get_arg(&meta.input)?);
                }
                "add_test_attr" => args.add_test_attr = get_bool_arg(&meta.input, true)?,
                "instrumentation" => args.instrumentation = get_bool_arg(&meta.input, true)?,
                "allow_interrupts" => args.allow_interrupts = get_bool_arg(&meta.input, true)?,
                _ => return Err(meta.error("unrecognized argument")),
            }

            Ok(())
        });

        arg_parser.parse2(input)?;
        Ok(args)
    }
}

impl Transformer {
    pub fn parse_main(args: TokenStream, input: TokenStream) -> Result<Self, Error> {
        Self::parse(FunctionType::Component, args, input)
    }

    pub fn parse_test(args: TokenStream, input: TokenStream) -> Result<Self, Error> {
        Self::parse(FunctionType::Test, args, input)
    }

    pub fn finish(self) -> TokenStream {
        Finish::finish(self)
    }

    // Construct a new Transformer, verifying correctness.
    fn parse(
        function_type: FunctionType,
        args: TokenStream,
        input: TokenStream,
    ) -> Result<Transformer, Error> {
        let args = Args::parse(args)?;
        let ItemFn { attrs, vis, sig, block } = syn::parse2(input)?;
        let is_async = sig.asyncness.is_some();

        let err = |message| Err(Error::new(sig.ident.span(), message));

        if matches!(function_type, FunctionType::Component) && args.allow_interrupts {
            return err("allow_interrupts only applies to tests");
        }
        let allow_interrupts = args.allow_interrupts;
        if !is_async && allow_interrupts {
            return err("allow_interrupts only applies to async tests");
        }

        let executor =
            match (args.threads, args.allow_stalls, args.thread_role, is_async, function_type) {
                (_, _, Some(_), _, FunctionType::Test) => {
                    return err("thread_role cannot be applied to tests");
                }
                (_, Some(_), _, _, FunctionType::Component) => {
                    return err("allow_stalls only applies to tests");
                }
                (None, _, thread_role, false, FunctionType::Component) => {
                    Executor::None { thread_role: thread_role.map(Box::new) }
                }
                (None, None, thread_role, true, FunctionType::Component) => {
                    Executor::Singlethreaded {
                        thread_role: thread_role.map(Box::new),
                        instrumentation: args.instrumentation,
                    }
                }
                (Some(threads), None, thread_role, true, FunctionType::Component) => {
                    Executor::Multithreaded {
                        threads: Box::new(threads),
                        thread_role: thread_role.map(Box::new),
                        instrumentation: args.instrumentation,
                    }
                }
                (None, Some(_), _, false, FunctionType::Test) => {
                    return err("allow_stalls only applies to async tests");
                }
                (None, None, _, false, FunctionType::Test) => Executor::Test,
                (None, Some(true) | None, _, true, FunctionType::Test) => {
                    Executor::SinglethreadedTest { allow_interrupts }
                }
                (Some(threads), Some(true) | None, _, true, FunctionType::Test) => {
                    Executor::MultithreadedTest { threads: Box::new(threads), allow_interrupts }
                }
                (None, Some(false), _, true, FunctionType::Test) => {
                    Executor::UntilStalledTest { allow_interrupts }
                }
                (_, Some(false), _, _, FunctionType::Test) => {
                    return err("allow_stalls=false tests must be single threaded");
                }
                (_, Some(true) | None, _, false, _) => {
                    return err("must be async to use >1 thread");
                }
            };

        let panic_prefix =
            args.panic_prefix.unwrap_or_else(|| LitStr::new("PANIC", sig.ident.span()));
        Ok(Transformer {
            executor,
            attrs,
            vis,
            sig,
            block,
            logging: args.logging,
            logging_tags: args.logging_tags,
            logging_include_file_line: args.logging_include_file_line,
            panic_prefix,
            interest: args.interest,
            add_test_attr: args.add_test_attr,
        })
    }
}

impl Finish for Transformer {
    // Build the transformed code, knowing that everything is ok because we proved that in parse.
    fn finish(self) -> TokenStream {
        let ident = self.sig.ident;
        let span = ident.span();
        let ret_type = self.sig.output;
        let attrs = self.attrs;
        let visibility = self.vis;
        let asyncness = self.sig.asyncness;
        let block = self.block;
        let inputs = self.sig.inputs;
        let always_log_file_line = self.logging_include_file_line;
        let mut logging_tags = self.logging_tags;
        let panic_prefix = self.panic_prefix;
        let interest = self.interest;

        let mut func_attrs = Vec::new();

        let should_panic = attrs.iter().any(|attr| {
            attr.path().segments.len() == 1 && attr.path().segments[0].ident == "should_panic"
        });
        let maybe_disable_lsan = if should_panic {
            quote! { ::fuchsia::disable_lsan_for_should_panic(); }
        } else {
            quote! {}
        };

        let inner_func_name = if self.executor.is_test() {
            quote! { test_entry_point }
        } else {
            quote! { component_entry_point }
        };

        // Using a unique, unambiguous variable name here avoids the macro hygiene issue
        // that occurs when this proc-macro is invoked from within a declarative macro.
        // The repeated shadowing of `let func = ...` can fail to resolve in that context.
        let func_to_run_ident =
            syn::Ident::new("__internal_func_to_run", proc_macro2::Span::mixed_site());

        let mut logging_init_fn_ident = String::from("init_");
        if let Some(logging) = self.logging {
            if !logging {
                logging_init_fn_ident.push_str("noop_");
            }
        } else {
            logging_init_fn_ident.push_str("default_");
        }
        if self.executor.is_test() {
            logging_tags.insert(0, LitStr::new(&ident.to_string(), ident.span()));
            logging_init_fn_ident.push_str("logging_for_test_");
        } else {
            logging_init_fn_ident.push_str("logging_for_component_");
        }
        if self.executor.is_some() {
            logging_init_fn_ident.push_str("with_executor");
        } else {
            logging_init_fn_ident.push_str("with_threads");
        }
        let logging_init_fn = Ident::new(&logging_init_fn_ident, proc_macro2::Span::call_site());
        let init_logging = quote! {
            ::fuchsia::#logging_init_fn(
                #func_to_run_ident,
                ::fuchsia::LoggingOptions {
                    interest: #interest,
                    always_log_file_line: #always_log_file_line,
                    tags: &[#logging_tags],
                    panic_prefix: #panic_prefix,
                }
            )
        };

        if self.executor.is_test() && self.add_test_attr {
            // Add test attribute to outer function.
            func_attrs.push(quote!(#[test]));
        }

        // Adapt the runner function based on whether it's a test and argument count
        // by providing needed arguments.
        let adapt_main = match (self.executor.is_test(), inputs.len()) {
            // Main function, no arguments - no adaption needed.
            (false, 0) => quote! { #inner_func_name },
            // Main function, one arguemnt - adapt by parsing command line arguments.
            (false, 1) => quote! { ::fuchsia::adapt_to_parse_arguments(#inner_func_name) },
            // Test function, no arguments - adapt by taking the run number and discarding it.
            (true, 0) => quote! { ::fuchsia::adapt_to_take_test_run_number(#inner_func_name) },
            // Test function, one argument - no adaption needed.
            (true, 1) => quote! { #inner_func_name },
            // Anything with more than one argument: error.
            (_, n) => panic!("Too many ({}) arguments to function", n),
        };

        let tts = self.executor.build_token_stream(&func_to_run_ident);
        let is_nonempty_ret_type = match &ret_type {
            syn::ReturnType::Default => false,
            syn::ReturnType::Type(_, ty) => match &**ty {
                // Treat a `-> ()` return as not having any return type at all.
                syn::Type::Tuple(tuple) => !tuple.elems.is_empty(),
                _ => true,
            },
        };

        // Select executor
        let (run_executor, modified_ret_type) =
            if is_nonempty_ret_type && self.logging != Some(false) {
                (
                    quote! {
                        let result = #tts;
                        match result {
                            std::result::Result::Ok(val) => {
                                use std::process::Termination;
                                val.report()
                            },
                            std::result::Result::Err(err) => {
                                ::fuchsia::error!("{err:?}");
                                std::process::ExitCode::FAILURE
                            }
                        }
                    },
                    quote!(-> std::process::ExitCode),
                )
            } else {
                (quote!(#tts), quote!(#ret_type))
            };

        // Finally build output.
        let output = quote_spanned! {span =>
            #(#attrs)* #(#func_attrs)*
            #visibility fn #ident () #modified_ret_type {
                // Note: `ItemFn::block` includes the function body braces. Do
                // not add additional braces (will break source code coverage
                // analysis).
                // TODO(https://fxbug.dev/42157203): Try to improve the Rust compiler to
                // ease this restriction.
                #asyncness fn #inner_func_name(#inputs) #ret_type #block
                #maybe_disable_lsan
                let #func_to_run_ident = #adapt_main;
                let #func_to_run_ident = #init_logging;
                #run_executor
            }
        };
        output.into()
    }
}

impl<R: Finish> Finish for Result<R, Error> {
    fn finish(self) -> TokenStream {
        match self {
            Ok(r) => r.finish(),
            Err(e) => e.to_compile_error(),
        }
    }
}
