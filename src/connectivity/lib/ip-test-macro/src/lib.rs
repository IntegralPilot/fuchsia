// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Macros to define tests to run with both IPv4 and IPv6 from logic that
//! is parameterized over IP version.

#[macro_use]
extern crate quote;
#[macro_use]
extern crate syn;

use core::fmt::Display;

use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::ToTokens;
use syn::parse::{ParseStream, Parser};
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::visit_mut::{self, VisitMut};
use syn::{
    Attribute, Error, Expr, ExprPath, FnArg, GenericParam, Ident, Pat, PatType, Path, TypePath,
};

/// Defines tests which call the annotated function with [`net_types::ip::Ipv4`]
/// and [`net_types::ip::Ipv6`] in place of the [`net_types::ip::Ip`] type
/// parameter.
///
/// Modulo interactions with other attribute macros, a function marked with
/// `#[ip_test]` must *always* receive zero arguments.
///
/// Note that due to how expansion works, the order in which attributes are
/// anchored to a function matters. `#[ip_test]` has smarts for handling
/// `test_case` attributes due to closely related functionality, but in general
/// any macro that will also emit code that is generic over IP should be placed
/// *before* `ip_test`.
///
/// ## Arguments
///
/// `ip_test` requires the identifier that represents the IP version to be given
/// as its first argument.
///
/// Optional arguments are in the form `arg = value` and are:
///
/// * `net_types`: controls the path where the `net_types` crate is available.
///   Example: `net_types = "crate::net_types_alias"`. If omitted defaults to
///   `net_types`.
/// * `test`: controls whether `#[test]` is emitted as part of the macro.
///   Defaults to automatic behavior based on detection of an existing
///   `test_case` attribute.  Can be set as `test = false` in argument list.
///   Note
///
/// ## Example
///
/// The following code:
///
/// ```rust
/// #[ip_test(I)]
/// fn test_foo<I: Ip>() {
///    assert!(do_ip_specific_thing::<I>());
///    /* ... */
/// }
/// ```
///
/// generates the following:
///
/// ```rust
/// fn test_foo<I: Ip>() {
///    assert!(do_ip_specific_thing::<I>());
///    /* ... */
/// }
///
/// #[test]
/// fn test_foo_v4() {
///    test_foo::<Ipv4>();
/// }
///
/// #[test]
/// fn test_foo_v6() {
///    test_foo::<Ipv6>();
/// }
/// ```
#[proc_macro_attribute]
pub fn ip_test(attr: TokenStream, input: TokenStream) -> TokenStream {
    let IpTestArgs { ip_ident, net_types, emit_test } = parse_macro_input!(attr as IpTestArgs);

    let item = parse_macro_input!(input as syn::ItemFn);
    let syn::ItemFn { mut attrs, vis, sig, block } = item;
    if let Some(variadic) = &sig.variadic {
        return Error::new(variadic.dots.spans[0], format!("ip_test entry may not be variadic"))
            .to_compile_error()
            .into();
    }

    if !sig.generics.params.iter().any(|generic_param| match generic_param {
        GenericParam::Type(tp) => tp.ident == ip_ident,
        _ => false,
    }) {
        return Error::new(sig.generics.span(), format!("can't find generic parameter {ip_ident}"))
            .to_compile_error()
            .into();
    }

    if emit_test.unwrap_or_else(|| {
        !attrs.iter().any(|a| a.path().is_ident("test_case") || a.path().is_ident("test_matrix"))
    }) {
        attrs.push(Attribute {
            pound_token: Default::default(),
            style: syn::AttrStyle::Outer,
            bracket_token: Default::default(),
            meta: syn::parse_quote!(test),
        });
    }
    // borrow here because `test_attrs` is used twice in `quote_spanned!` below.
    let attrs = &attrs;

    let span = sig.ident.span();
    let output = &sig.output;
    let ident = &sig.ident;

    let arg_idents;
    {
        let mut errors = Vec::new();
        arg_idents = make_arg_idents(sig.inputs.iter(), &mut errors);
        if !errors.is_empty() {
            return Error::new(sig.inputs.span(), quote!(#(#errors)*)).to_compile_error().into();
        }
    }

    let v4_test = Ident::new(&format!("{}_v4", ident), Span::call_site());
    let v6_test = Ident::new(&format!("{}_v6", ident), Span::call_site());

    let net_types_path = |ty| {
        let mut p = net_types.clone();
        let rest = syn::parse_str::<Path>(ty).unwrap();
        p.segments.extend(rest.segments.into_iter());
        p
    };
    let ip_trait_path = net_types_path("ip::Ip");
    let ipv4_type_path = net_types_path("ip::Ipv4");
    let ipv6_type_path = net_types_path("ip::Ipv6");

    struct IpSpecializations {
        test_attrs: Vec<Attribute>,
        inputs: Vec<FnArg>,
        fn_generics: Vec<GenericParam>,
        generic_params: Vec<Path>,
    }

    let specialize = |ip_type_path: Path| {
        let test_attrs = attrs
            .iter()
            .cloned()
            .map(|mut attr| {
                let parser =
                    parse_prefix_suffix(Punctuated::<Expr, Token![,]>::parse_separated_nonempty);
                if let Ok((mut punctuated, tail)) = attr.parse_args_with(parser) {
                    let mut visit = TraitToConcreteVisit {
                        concrete: ip_type_path.clone().into(),
                        trait_path: ip_trait_path.clone(),
                        type_ident: ip_ident.clone(),
                    };

                    for expr in punctuated.iter_mut() {
                        visit.visit_expr_mut(expr);
                    }
                    let path = attr.meta.path();
                    attr.meta = parse_quote!(#path(#punctuated #tail));
                }
                attr
            })
            .collect();

        let mut input_visitor = TraitToConcreteVisit {
            concrete: ip_type_path.clone().into(),
            trait_path: ip_trait_path.clone(),
            type_ident: ip_ident.clone(),
        };
        let inputs = sig
            .inputs
            .iter()
            .cloned()
            .map(|mut a| {
                input_visitor.visit_fn_arg_mut(&mut a);
                a
            })
            .collect();

        let fn_generics = sig
            .generics
            .params
            .iter()
            .filter(|generic_param| match generic_param {
                GenericParam::Type(tp) => tp.ident != ip_ident,
                _ => true,
            })
            .cloned()
            .map(|mut generic_param| {
                input_visitor.visit_generic_param_mut(&mut generic_param);
                generic_param
            })
            .collect::<Vec<_>>();

        let generic_params = sig
            .generics
            .params
            .iter()
            .filter_map(|a| match a {
                GenericParam::Type(tp) => Some(if tp.ident == ip_ident {
                    ip_type_path.clone()
                } else {
                    tp.ident.clone().into()
                }),
                GenericParam::Lifetime(_) => None,
                GenericParam::Const(c) => Some(c.ident.clone().into()),
            })
            .collect();

        IpSpecializations { test_attrs, inputs, generic_params, fn_generics }
    };

    let IpSpecializations {
        test_attrs: ipv4_test_attrs,
        inputs: ipv4_inputs,
        fn_generics: ipv4_fn_generics,
        generic_params: ipv4_generic_params,
    } = specialize(ipv4_type_path);

    let IpSpecializations {
        test_attrs: ipv6_test_attrs,
        inputs: ipv6_inputs,
        fn_generics: ipv6_fn_generics,
        generic_params: ipv6_generic_params,
    } = specialize(ipv6_type_path);

    let (maybe_async, do_await) = if sig.asyncness.is_some() {
        (quote! {async}, quote! {.await})
    } else {
        (quote! {}, quote! {})
    };

    let output = quote_spanned! { span =>
        // Note: `ItemFn::block` includes the function body braces. Do not add
        // additional braces (will break source code coverage analysis).
        // TODO(https://fxbug.dev/42157203): Try to improve the Rust compiler to
        // ease this restriction.
        #vis #sig #block

        #(#ipv4_test_attrs)*
        #maybe_async fn #v4_test<#(#ipv4_fn_generics),*> (#(#ipv4_inputs),*) #output {
           #ident::<#(#ipv4_generic_params),*>(#(#arg_idents),*) #do_await
        }

        #(#ipv6_test_attrs)*
        #maybe_async fn #v6_test<#(#ipv6_fn_generics),*> (#(#ipv6_inputs),*) #output {
           #ident::<#(#ipv6_generic_params),*>(#(#arg_idents),*) #do_await
        }
    };
    output.into()
}

fn push_error<T: ToTokens, U: Display>(errors: &mut Vec<TokenStream2>, tokens: T, message: U) {
    errors.push(Error::new_spanned(tokens, message).to_compile_error());
}

fn make_arg_idents<'a>(
    input: impl Iterator<Item = &'a FnArg>,
    errors: &mut Vec<TokenStream2>,
) -> Vec<Ident> {
    input
        .map(|arg| match arg {
            FnArg::Receiver(_) => Ident::new("self", Span::call_site()),
            FnArg::Typed(PatType { pat, .. }) => match pat.as_ref() {
                Pat::Ident(pat) => {
                    if pat.subpat.is_some() {
                        push_error(errors, arg, "unexpected attribute argument");
                        parse_quote!(pushed_error)
                    } else {
                        pat.ident.clone()
                    }
                }
                _ => {
                    push_error(errors, arg, "patterns in function arguments not supported");
                    parse_quote!(pushed_error)
                }
            },
        })
        .collect()
}

struct IpTestArgs {
    ip_ident: Ident,
    net_types: Path,
    emit_test: Option<bool>,
}

impl syn::parse::Parse for IpTestArgs {
    fn parse(input: syn::parse::ParseStream<'_>) -> syn::Result<Self> {
        let ip_ident = Ident::parse(input)?;

        let mut net_types = None;
        let mut emit_test = None;
        if !input.is_empty() {
            let _ = <syn::Token![,]>::parse(input)?;
        }
        if !input.is_empty() {
            let args = Punctuated::<syn::MetaNameValue, syn::Token![,]>::parse_terminated(input)?;
            for syn::MetaNameValue { path, value, .. } in args {
                let ident = path
                    .get_ident()
                    .ok_or_else(|| Error::new(path.span(), "expecting identifier"))?;
                if ident == "test" {
                    let notest = match value {
                        syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Bool(b), .. }) => b,
                        _ => return Err(Error::new(value.span(), "expected boolean")),
                    };
                    emit_test = Some(notest.value);
                } else if ident == "net_types" {
                    let v = match value {
                        syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Str(s), .. }) => s,
                        _ => return Err(Error::new(value.span(), "extected string")),
                    };
                    net_types = Some(syn::parse_str(&v.value()).map_err(|mut e| {
                        e.combine(Error::new(v.span(), "can't parse path"));
                        e
                    })?);
                } else {
                    return Err(Error::new(path.span(), format!("unrecognized option {ident}")));
                }
            }
        }

        let net_types = net_types.unwrap_or_else(|| syn::parse_quote! { net_types });
        Ok(Self { ip_ident, emit_test, net_types })
    }
}

/// A VisitMut that replaces accesses of an associated type or constant with
/// type a different type qualified with a trait name. Given a `type_ident` of
/// `I`, `concrete` of `Ipv4` and `trait_path` of `Ip`, `I::Addr` would be
/// replaced with `<Ipv4 as Ip>::Addr`.
struct TraitToConcreteVisit {
    type_ident: Ident,
    concrete: Path,
    trait_path: Path,
}

impl TraitToConcreteVisit {
    fn update_type_path(&self, qself: &mut Option<syn::QSelf>, path: &mut Path) {
        let Self { type_ident, concrete, trait_path } = self;
        if qself == &None {
            let mut segments = path.segments.iter();
            if segments.next().is_some_and(|p| &p.ident == type_ident) {
                let remaining_path = segments.cloned().collect::<Vec<_>>();
                let TypePath { path: new_path, qself: new_qself } = if remaining_path.is_empty() {
                    parse_quote!(#concrete)
                } else {
                    parse_quote!(<#concrete as #trait_path>::#(#remaining_path)::*)
                };
                *path = new_path;
                *qself = new_qself;
            }
        }
    }
}

impl VisitMut for TraitToConcreteVisit {
    fn visit_expr_path_mut(&mut self, i: &mut ExprPath) {
        let ExprPath { attrs: _, qself, path } = i;
        self.update_type_path(qself, path);

        visit_mut::visit_expr_path_mut(self, i);
    }

    fn visit_type_path_mut(&mut self, i: &mut TypePath) {
        let TypePath { qself, path } = i;
        self.update_type_path(qself, path);

        visit_mut::visit_type_path_mut(self, i)
    }
}

/// Constructs a parser that eagerly parses with the provided function.
///
/// The parser returns two things: the prefix of the input that was
/// successfully parsed by the provided function, and the rest of the input.
/// This is useful for adapting `P` for [`syn::parse`], which returns an error
/// if any tokens are left unconsumed.
fn parse_prefix_suffix<P>(
    parser: for<'a> fn(ParseStream<'a>) -> syn::Result<P>,
) -> impl Parser<Output = (P, TokenStream2)> {
    fn consume_input<'a>(input: ParseStream<'a>) -> TokenStream2 {
        input.parse::<TokenStream2>().unwrap()
    }

    move |input: ParseStream<'_>| parser(input).map(|p| (p, consume_input(input)))
}

#[cfg(test)]
mod tests {
    use assert_matches::assert_matches;

    use super::*;

    #[test]
    fn parse_prefix_suffix_test_case() {
        let test_case = quote!(arg1, arg2; "name");

        let parser = parse_prefix_suffix(Punctuated::<Expr, Token![,]>::parse_separated_nonempty);

        let (arguments, tail) = parser.parse2(test_case).expect("parse succeeds");

        let arguments = arguments
            .iter()
            .map(|e| assert_matches!(e, Expr::Path(p) => &p.path))
            .collect::<Vec<_>>();
        let (arg1, arg2) = assert_matches!(arguments.as_slice(), &[arg1, arg2] => (arg1, arg2));
        assert!(arg1.is_ident("arg1"));
        assert!(arg2.is_ident("arg2"));

        let tail = tail.into_iter().map(|t| t.to_string()).collect::<Vec<_>>();
        assert_eq!(tail.as_slice(), &[";", "\"name\""]);
    }
}
