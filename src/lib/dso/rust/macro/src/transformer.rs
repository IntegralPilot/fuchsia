// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_diagnostics_types::Severity;
use proc_macro2::TokenStream;
use quote::{quote, quote_spanned};
use syn::parse::{Parse, ParseStream, Parser};
use syn::punctuated::Punctuated;
use syn::{
    Attribute, Block, Error, Expr, Ident, ItemFn, LitBool, LitStr, Signature, Token, Visibility,
};

// How should code be executed?
#[derive(Clone)]
enum Executor {
    // Directly by calling it
    None { thread_role: Option<Box<Expr>> },
    // fasync::run_singlethreaded
    Singlethreaded { thread_role: Option<Box<Expr>> },
    // fasync::run
    Multithreaded { threads: Box<Expr>, thread_role: Option<Box<Expr>> },
}

impl Executor {
    fn is_some(&self) -> bool {
        !matches!(self, Executor::None { .. })
    }

    fn build_token_stream(&self, func: &syn::Ident) -> TokenStream {
        let executor_new = match self {
            Executor::None { thread_role } => {
                if let Some(role) = thread_role {
                    quote! { ::fuchsia::main_not_async_with_role(#func, #role) }
                } else {
                    quote! { ::fuchsia::main_not_async(#func) }
                }
            }
            Executor::Singlethreaded { thread_role } => {
                if let Some(role) = thread_role {
                    quote! { ::fuchsia::main_singlethreaded_with_role(#func, #role, None) }
                } else {
                    quote! { ::fuchsia::main_singlethreaded(#func, None) }
                }
            }
            Executor::Multithreaded { threads, thread_role } => {
                if let Some(role) = thread_role {
                    quote! { ::fuchsia::main_multithreaded_with_role(#func, #threads, #role, None) }
                } else {
                    quote! { ::fuchsia::main_multithreaded(#func, #threads, None) }
                }
            }
        };
        quote! {{
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

#[derive(Copy, Clone)]
enum Dso {
    Sync,
    Async,
}

pub struct Transformer {
    executor: Executor,
    attrs: Vec<Attribute>,
    vis: Visibility,
    sig: Signature,
    block: Box<Block>,
    dso_syncness: Dso,
    logging: Option<bool>,
    logging_tags: Punctuated<LitStr, Token![,]>,
    logging_include_file_line: bool,
    panic_prefix: LitStr,
    interest: Interest,
}

struct Args {
    threads: Option<Expr>,
    thread_role: Option<Expr>,
    dso_sync: bool,
    dso_async: bool,
    logging: Option<bool>,
    logging_tags: Punctuated<LitStr, Token![,]>,
    logging_include_file_line: bool,
    interest: Interest,
    panic_prefix: Option<LitStr>,
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
            dso_sync: false,
            dso_async: false,
            logging: None,
            logging_tags: Default::default(),
            logging_include_file_line: false,
            panic_prefix: None,
            interest: Interest::default(),
        };

        let arg_parser = syn::meta::parser(|meta| {
            let ident =
                meta.path.get_ident().ok_or_else(|| meta.error("arguments must have a key"))?;
            match ident.to_string().as_ref() {
                "threads" => args.threads = Some(get_arg::<Expr>(&meta.input)?),
                "thread_role" => args.thread_role = Some(get_arg::<Expr>(&meta.input)?),
                "sync" => args.dso_sync = get_bool_arg(&meta.input, true)?,
                "async" => args.dso_async = get_bool_arg(&meta.input, true)?,
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
        Self::parse(args, input)
    }

    pub fn finish(self) -> TokenStream {
        Finish::finish(self)
    }

    // Construct a new Transformer, verifying correctness.
    fn parse(args: TokenStream, input: TokenStream) -> Result<Transformer, Error> {
        let args = Args::parse(args)?;
        let ItemFn { attrs, vis, sig, block } = syn::parse2(input)?;
        let is_async = sig.asyncness.is_some();

        let err = |message| Err(Error::new(sig.ident.span(), message));

        let executor = match (args.threads, args.thread_role, is_async) {
            (None, thread_role, false) => Executor::None { thread_role: thread_role.map(Box::new) },
            (None, thread_role, true) => {
                Executor::Singlethreaded { thread_role: thread_role.map(Box::new) }
            }
            (Some(threads), thread_role, true) => Executor::Multithreaded {
                threads: Box::new(threads),
                thread_role: thread_role.map(Box::new),
            },
            (_, _, false) => {
                return err("must be async to use >1 thread");
            }
        };

        let panic_prefix =
            args.panic_prefix.unwrap_or_else(|| LitStr::new("PANIC", sig.ident.span()));

        let dso_syncness = match (args.dso_sync, args.dso_async) {
            (true, true) => {
                return err("sync and async are mutually exclusive");
            }
            (true, false) => Dso::Sync,
            (false, true) => Dso::Async,
            (false, false) => return err("either sync or async is required"),
        };

        match dso_syncness {
            Dso::Sync => {
                if is_async {
                    return err("sync requires non-async fn");
                }
            }
            Dso::Async => {
                if !is_async {
                    return err("async requires async fn");
                }
            }
        }
        Ok(Transformer {
            executor,
            attrs,
            vis,
            sig,
            block,
            dso_syncness,
            logging: args.logging,
            logging_tags: args.logging_tags,
            logging_include_file_line: args.logging_include_file_line,
            panic_prefix,
            interest: args.interest,
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
        let dso_syncness = self.dso_syncness;
        let asyncness = self.sig.asyncness;
        let block = self.block;
        let inputs = self.sig.inputs;
        let always_log_file_line = self.logging_include_file_line;
        let logging_tags = self.logging_tags;
        let panic_prefix = self.panic_prefix;
        let interest = self.interest;

        let should_panic = attrs.iter().any(|attr| {
            attr.path().segments.len() == 1 && attr.path().segments[0].ident == "should_panic"
        });
        let maybe_disable_lsan = if should_panic {
            quote! { ::fuchsia::disable_lsan_for_should_panic(); }
        } else {
            quote! {}
        };

        let inner_func_name = quote! { component_entry_point };

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
        logging_init_fn_ident.push_str("logging_for_component_");
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

        // Adapt the runner function based on the number of arguments.
        let adapt_main = match inputs.len() {
            // Main function, no arguments - no adaption needed.
            0 => quote! { #inner_func_name },
            // Main function, one argument - adapt by parsing command line arguments.
            1 => match dso_syncness {
                Dso::Async => {
                    quote! { ::fuchsia_dso::adapt_to_pass_arguments(#inner_func_name, args) }
                }
                Dso::Sync => quote! { ::fuchsia::adapt_to_parse_arguments(#inner_func_name) },
            },
            // Anything with more than one argument: error.
            n => panic!("Too many ({}) arguments to function", n),
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
        let returns_exit_code;
        let (run_executor, modified_ret_type) =
            if is_nonempty_ret_type && self.logging != Some(false) {
                returns_exit_code = true;
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
                returns_exit_code = false;
                (quote!(#tts), quote!(#ret_type))
            };

        let exit = if is_nonempty_ret_type {
            if returns_exit_code {
                quote! {
                    return match _result {
                        ::std::process::ExitCode::SUCCESS => 0,
                        ::std::process::ExitCode::FAILURE => 1,
                        _ => 255,
                    };
                }
            } else {
                quote! {
                    return match _result {
                        Ok(_) => 0,
                        Err(_) => 1,
                    };
                }
            }
        } else {
            quote! {
                return 0;
            }
        };

        let dso_start = match dso_syncness {
            Dso::Sync => {
                quote! {
                    #[repr(C)]
                    pub struct dso_sync_input {
                        handle_count: u32,
                        handle: *mut zx::sys::zx_handle_t,
                        handle_info: *mut u32,
                        name_count: u32,
                        names: *mut *const ::std::ffi::c_char,
                        argc: ::std::ffi::c_int,
                        argv: *mut *const ::std::ffi::c_char,
                        envp: *mut *const ::std::ffi::c_char,
                    }

                    #[unsafe(no_mangle)]
                    pub unsafe extern "C" fn _dso_start(
                        input: dso_sync_input,
                    ) -> ::std::ffi::c_int {
                        let dso_sync_input {
                            handle_count,
                            handle,
                            handle_info,
                            name_count,
                            names,
                            argc,
                            argv,
                            envp,
                        } = input;
                        ::fuchsia_dso::dso_init(
                            handle_count,
                            handle,
                            handle_info,
                            name_count,
                            names,
                            argc,
                            argv,
                            envp);
                        let _result = #ident();
                        ::fuchsia_dso::dso_fini();
                        #exit
                    }
                }
            }
            Dso::Async => {
                if asyncness.is_none() {
                    panic!("main function must be async if `async` is specificed");
                }
                quote! {
                    #[repr(C)]
                    pub struct dso_async_input {
                        handle_count: u32,
                        handle: *mut zx::sys::zx_handle_t,
                        handle_info: *mut u32,
                        name_count: u32,
                        names: *mut *const ::std::ffi::c_char,
                        argc: ::std::ffi::c_int,
                        argv: *mut *const ::std::ffi::c_char,
                        envp: *mut *const ::std::ffi::c_char,
                        dispatcher: *mut ::std::ffi::c_void,
                    }

                    #[unsafe(no_mangle)]
                    pub unsafe extern "C" fn _dso_start_async(
                        input: dso_async_input,
                    ) -> ::std::ffi::c_int {
                        let dso_async_input {
                            handle_count,
                            handle,
                            handle_info,
                            name_count,
                            names,
                            argc,
                            argv,
                            envp,
                            dispatcher,
                        } = input;
                        let payload = ::fuchsia_dso::DsoStartAsyncPayload {
                            handle_count,
                            handle,
                            handle_info,
                            name_count,
                            names,
                            argc,
                            argv,
                            envp,
                            dispatcher,
                        };
                        __dso_start_async(payload)
                    }

                    fn __dso_start_async(payload: ::fuchsia_dso::DsoStartAsyncPayload) -> ::std::ffi::c_int {
                        let args = ::fuchsia_dso::dso_init_async(payload);
                        let _result = #ident(args);
                        #exit
                    }
                }
            }
        };

        // Finally build output.
        let output = match dso_syncness {
            Dso::Sync => quote_spanned! {span =>
                #dso_start

                #(#attrs)*
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
            },
            Dso::Async => quote_spanned! {span =>
                #dso_start

                #(#attrs)*
                #visibility fn #ident (args: ::fuchsia_dso::DsoAsyncArgs) #modified_ret_type {
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
            },
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
