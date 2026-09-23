// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{ToTokens, TokenStreamExt, quote};
use syn::parse::{Parse, ParseStream};
use syn::spanned::Spanned;
use syn::visit_mut::VisitMut;
use syn::{
    Attribute, Block, Expr, Ident, ImplItem, ItemFn, ItemImpl, LitStr, Path, ReturnType, Token,
    Type, parse_macro_input, parse_quote,
};

enum TraceItem {
    Impl(ItemImpl),
    Fn(ItemFn),
}

impl Parse for TraceItem {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        // Determine if the attribute is on a `fn` or an `impl`. Both a `fn` and an `impl`` can have
        // other attributes and `unsafe` out front. Those tokens are consumed and put back later. An
        // `impl` block should only have the `impl` token next.
        let attrs = input.call(Attribute::parse_outer)?;
        let unsafe_token =
            if input.peek(Token![unsafe]) { Some(input.parse::<Token![unsafe]>()?) } else { None };
        let trace_item = if input.peek(Token![impl]) {
            let mut item_impl = input.parse::<ItemImpl>()?;
            item_impl.attrs = attrs;
            item_impl.unsafety = unsafe_token;
            TraceItem::Impl(item_impl)
        } else {
            let mut item_fn = input.parse::<ItemFn>()?;
            item_fn.attrs = attrs;
            item_fn.sig.unsafety = unsafe_token;
            TraceItem::Fn(item_fn)
        };
        Ok(trace_item)
    }
}

struct TraceImplArgs {
    trace_all_methods: bool,
    prefix: Option<Path>,
}

impl Parse for TraceImplArgs {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let mut args = Self { trace_all_methods: false, prefix: None };
        loop {
            if input.is_empty() {
                break;
            }
            let ident: Ident = input.parse()?;
            match ident.to_string().as_ref() {
                "trace_all_methods" => args.trace_all_methods = true,
                "prefix" => {
                    input.parse::<Token![=]>()?;
                    args.prefix = Some(input.parse()?);
                }
                arg => {
                    return Err(syn::Error::new(
                        ident.span(),
                        format!("unknown argument: {}", arg),
                    ));
                }
            }
            if input.is_empty() {
                break;
            }
            input.parse::<Token![,]>()?;
        }
        Ok(args)
    }
}

#[derive(Default)]
struct TraceArgs(Vec<(LitStr, Expr)>);

impl ToTokens for TraceArgs {
    fn to_tokens(&self, tokens: &mut TokenStream2) {
        if self.0.len() > 0 {
            let arg_names = self.0.iter().map(|a| &a.0);
            let arg_values = self.0.iter().map(|a| &a.1);
            tokens.append_all(quote!(, #(#arg_names => #arg_values),*));
        }
    }
}

#[derive(Default)]
struct AttributeArgs {
    name: Option<Path>,
    trace_args: TraceArgs,
}

impl Parse for AttributeArgs {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let mut args = Self::default();
        loop {
            if input.is_empty() {
                break;
            } else if input.peek(Ident) {
                let ident: Ident = input.parse()?;
                if ident != "name" {
                    return Err(syn::Error::new(
                        ident.span(),
                        format!("unknown argument: {}", ident),
                    ));
                }
                if args.name.is_some() {
                    return Err(syn::Error::new(
                        ident.span(),
                        format!("name specified multiple times"),
                    ));
                }
                input.parse::<Token![=]>()?;
                args.name = Some(input.parse()?);
            } else if input.peek(LitStr) {
                let arg_name: LitStr = input.parse()?;
                input.parse::<Token![=>]>()?;
                let expr: Expr = input.parse()?;
                args.trace_args.0.push((arg_name, expr))
            }

            if input.is_empty() {
                break;
            } else {
                input.parse::<Token![,]>()?;
            }
        }

        Ok(args)
    }
}

#[derive(Default)]
struct TraceMethodArgs(AttributeArgs);

impl Parse for TraceMethodArgs {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        Ok(Self(input.parse()?))
    }
}

#[derive(Default)]
struct TraceFnArgs(AttributeArgs);

impl Parse for TraceFnArgs {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        Ok(Self(input.parse()?))
    }
}

/// Looks for a `#[trace]` attribute in `attrs`, removes the attribute, and parses the arguments.
/// Returns `Ok(Some(TraceMethodArgs))` if the attribute is present, `Ok(None)` if the attribute is
/// not present, and `Err` if parsing the attribute's arguments fails.
fn remove_trace_attribute(attrs: &mut Vec<Attribute>) -> syn::Result<Option<TraceMethodArgs>> {
    let position = attrs.iter().position(|attr| attr.path().is_ident("trace"));
    let attr = match position {
        None => {
            return Ok(None);
        }
        Some(pos) => attrs.remove(pos),
    };
    if let syn::Meta::Path(_) = attr.meta {
        return Ok(Some(TraceMethodArgs::default()));
    }
    Ok(Some(attr.parse_args::<TraceMethodArgs>()?))
}

/// Replaces `impl Trait` with `_` in `Type` objects.
struct RemoveImplTrait;
impl VisitMut for RemoveImplTrait {
    fn visit_type_mut(&mut self, node: &mut Type) {
        if let Type::ImplTrait(..) = node {
            *node = Type::Infer(syn::TypeInfer { underscore_token: Token![_](node.span()) });
        } else {
            syn::visit_mut::visit_type_mut(self, node);
        }
    }
}

type TraceName = LitStr;

fn path_to_trace_name(path: &Path) -> TraceName {
    let name = path
        .segments
        .iter()
        .map(|segment| segment.ident.to_string())
        .collect::<Vec<_>>()
        .join("::");
    LitStr::new(&name, path.span())
}

fn add_tracing_to_async_block(
    return_type: &ReturnType,
    block: &mut Block,
    name: TraceName,
    args: TraceArgs,
) {
    let return_type = if let ReturnType::Type(_, return_type) = return_type {
        let mut return_type = *return_type.clone();
        // `impl Trait` can't appear in the type of a variable binding. Replace all `impl Trait`s
        // with `_`.
        RemoveImplTrait.visit_type_mut(&mut return_type);
        quote!( #return_type )
    } else {
        quote!(())
    };

    // Rust uses the type of the first `return` statement to determine the output type of the future
    // created by the `async move {}' block. If the function returns `Box<dyn Trait>` and has
    // multiple `return` points with different implementations of `Trait` then the generated future
    // will have the type of the first `return` and compilation may fail on the other `return`
    // statements. Placing an unreachable `return` statement with the correct `return` type at the
    // top of the `async move {}` block gives the generated future the correct return type.
    let type_inference_fix = quote! {
        #[allow(unreachable_code)]
        if false {
            let _type_inference_fix: #return_type = unreachable!();
            return _type_inference_fix;
        }
    };

    // The trace args are created before the future because the future may move variables that are
    // referenced in the trace args. The trace args should make copies of the data so they won't
    // move any variables used in the future.
    let stmts = &block.stmts;
    block.stmts = parse_quote!(
        let __trace_args = ::fxfs_trace::trace_future_args!(#name #args);
        ::fxfs_trace::TraceFutureExt::trace(async move {
            #type_inference_fix
            #(#stmts)*
        }, __trace_args).await
    );
}

fn add_tracing_to_sync_block(block: &mut Block, name: TraceName, args: TraceArgs) {
    let stmts = &block.stmts;
    block.stmts = parse_quote!(
        ::fxfs_trace::duration!(#name #args);
        #(#stmts)*
    );
}

fn trace_prefix_from_type(impl_type: &Type) -> Option<Path> {
    if let Type::Path(path) = impl_type {
        if let Some(segment) = path.path.segments.last() {
            return Some(segment.ident.clone().into());
        }
    }
    None
}

fn add_tracing_to_impl(mut item_impl: ItemImpl, args: TraceImplArgs) -> syn::Result<TokenStream2> {
    let prefix = if let Some(prefix) = args.prefix {
        prefix
    } else if let Some(prefix) = trace_prefix_from_type(&*item_impl.self_ty) {
        prefix
    } else {
        return Err(syn::Error::new(
            item_impl.self_ty.span(),
            "Failed to determine prefix from type name. Explicit prefix required.",
        ));
    };

    for item in &mut item_impl.items {
        if let ImplItem::Fn(func) = item {
            let trace_fn_args = remove_trace_attribute(&mut func.attrs)?.or_else(|| {
                if args.trace_all_methods { Some(TraceMethodArgs::default()) } else { None }
            });
            let Some(trace_fn_args) = trace_fn_args else {
                continue;
            };
            let trace_name_path = if let Some(name) = trace_fn_args.0.name {
                name
            } else {
                let ident = func.sig.ident.clone();
                parse_quote!(#prefix :: #ident)
            };
            let trace_name = path_to_trace_name(&trace_name_path);
            if func.sig.asyncness.is_some() {
                add_tracing_to_async_block(
                    &func.sig.output,
                    &mut func.block,
                    trace_name,
                    trace_fn_args.0.trace_args,
                );
            } else {
                add_tracing_to_sync_block(&mut func.block, trace_name, trace_fn_args.0.trace_args);
            }
        }
    }
    Ok(quote!(#item_impl))
}

fn add_tracing_to_fn(mut item_fn: ItemFn, args: TraceFnArgs) -> syn::Result<TokenStream2> {
    let trace_name_path =
        if let Some(name) = args.0.name { name } else { item_fn.sig.ident.clone().into() };
    let trace_name = path_to_trace_name(&trace_name_path);
    if item_fn.sig.asyncness.is_some() {
        add_tracing_to_async_block(
            &item_fn.sig.output,
            &mut item_fn.block,
            trace_name,
            args.0.trace_args,
        );
    } else {
        add_tracing_to_sync_block(&mut item_fn.block, trace_name, args.0.trace_args);
    }
    Ok(quote!(#item_fn))
}

/// Adds tracing to functions and methods.
///
/// ## Method Tracing
/// When this attribute is present on an `impl`, methods marked with `#[trace]` will have tracing
/// added to them. The name of the trace events default to `<type-name>::<method-name>` but this can
/// be changed with arguments.
///
/// Arguments:
///  - `trace_all_methods` - boolean toggle for whether to add tracing to all methods in the `impl`
///                          even if methods aren't marked with `#[trace]`. Defaults to false.
///  - `prefix`            - string to appear before the method name in the trace events. Defaults
///                          to the name of the type. This argument may be required if the name of
///                          the type can't be determined.
///
/// `#[trace]` Arguments:
///  - `name` - string to use for the name of the trace event. Defaults to
///             `<prefix>::<method-name>`.
///
/// Example:
/// ```
/// struct Foo;
/// #[fxfs_trace::trace]
/// impl Foo {
///     #[trace]
///     async fn bar(&self) {
///         ...
///     }
/// }
/// ```
///
/// If `async_trait` is also present on the `impl` then `fxfs_trace::trace` should come first.
/// `async_trait` desugars async methods into sync methods that return a BoxFuture. If `fxfs_trace`
/// runs after `async_trait` then `fxfs_trace` will see async methods as sync methods and apply the
/// wrong tracing.
///
/// ## Function Tracing
/// When this attribute is present on a function, the function will have tracing added to it.
///
/// Arguments:
///  - `name` - string to use for the name of the trace event. Defaults to the name of the function.
///
/// ## Synchronous Functions/Methods
/// `fxfs_trace::duration!` is added to the start of the function/method.
///
/// Example:
/// ```
/// #[fxfs_trace::trace]
/// fn example_function() {
///     ...
/// }
/// // Expands to:
/// fn example_function() {
///     fxfs_trace::duration!("example_function");
///     ...
/// }
/// ```
///
/// ## Async Functions/Methods
/// The body of function/method is wrapped in a `fuchsia_tace::TraceFuture`.
///
/// Example:
/// ```
/// #[fxfs_trace::trace]
/// async fn example_function() {
///     ...
/// }
/// // Expands to:
/// async fn example_function() {
///     let trace_future_args = fxfs_trace::trace_future_args!("example_function");
///     async move {
///         ...
///     }.trace(trace_future_args).await
/// }
/// ```
#[proc_macro_attribute]
pub fn trace(args: TokenStream, input: TokenStream) -> TokenStream {
    let trace_item = parse_macro_input!(input as TraceItem);
    let result = match trace_item {
        TraceItem::Impl(item_impl) => {
            let args = parse_macro_input!(args as TraceImplArgs);
            add_tracing_to_impl(item_impl, args)
        }
        TraceItem::Fn(item_fn) => {
            let args = parse_macro_input!(args as TraceFnArgs);
            add_tracing_to_fn(item_fn, args)
        }
    };
    result.unwrap_or_else(|e| e.to_compile_error()).into()
}
