// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use assert_matches::assert_matches;
use proc_macro::TokenStream;
use proc_macro2::{TokenStream as TokenStream2, TokenTree};
use quote::{ToTokens as _, quote};
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::{
    AngleBracketedGenericArguments, GenericArgument, GenericParam, Generics, Ident, Token, Type,
    TypeParam, TypeParamBound, TypePath, parse_quote,
};

/// Implements a derive macro for [`net_types::ip::GenericOverIp`].
/// Requires that #[derive(GenericOverIp)] invocations explicitly specify
/// which type parameter is the generic-over-ip one with the
/// `#[generic_over_ip]` attribute, rather than inferring it from the bounds
/// on the struct generics.
///
/// Consider the following example:
///
/// ```
///  #[derive(GenericOverIp)]
///  #[generic_over_ip(<ARGUMENTS EXPLAINED BELOW>)]
///  struct Foo<T>(T);
/// ```
///
/// `#[generic_over_ip(T, Ip)]` specifies that the GenericOverIp impl
/// should be written treating `T` as an `Ip` implementor (either `Ipv4` or
/// `Ipv6`).
///
/// `#[generic_over_ip(T, IpAddress)]` specifies that `T` is an `IpAddress`
/// implementor (`Ipv4Addr` or `Ipv6Addr`).
///
/// `#[generic_over_ip(T, GenericOverIp)]` specifies that `T` implements
/// `GenericOverIp<I>` for all `I: Ip`. (Notably, we'd like to use this case
/// for the Ip and IpAddress cases above, but we cannot due to issues
/// with conflicting blanket impls.)
///
/// `#[generic_over_ip()]` specifies that `Foo` is IP-invariant.
#[proc_macro_derive(GenericOverIp, attributes(generic_over_ip))]
pub fn derive_generic_over_ip(input: TokenStream) -> TokenStream {
    let ast = syn::parse(input).unwrap();

    impl_derive_generic_over_ip(&ast).into()
}

fn impl_derive_generic_over_ip(ast: &syn::DeriveInput) -> TokenStream2 {
    let (impl_generics, type_generics, where_clause) = ast.generics.split_for_impl();
    if where_clause.is_some() {
        return quote! {
            compile_error!("deriving GenericOverIp for types with 'where' clauses is unsupported")
        }
        .into();
    }

    let name = &ast.ident;

    let specified_generic_over_ip = match find_generic_over_ip_attr(ast) {
        Ok(param) => param,
        Err(e) => return e.into_compile_error().into(),
    };

    let extra_bounds = match &specified_generic_over_ip {
        None => Vec::new(),
        Some((ident, _)) => match collect_bounds(ident, ast.generics.type_params()) {
            Some(bounds) => bounds,
            None => {
                return syn::Error::new(
                    ast.generics.span(),
                    format!(
                        "found no type parameter named {ident:?} as specified in \
                            the generic_over_ip attribute"
                    ),
                )
                .into_compile_error()
                .into();
            }
        },
    };

    // Drop the first and last tokens, which should be '<' and '>', and the
    // trailing comma if there is one.
    let mut impl_generics = impl_generics.into_token_stream().into_iter();

    let expect_trailing_angle_bracket = impl_generics.next().is_some_and(|first| {
        assert_matches!(first, TokenTree::Punct(p) if p.as_char() == '<');
        true
    });
    let mut impl_generics: Vec<_> = impl_generics.collect();
    if expect_trailing_angle_bracket {
        assert_matches!(impl_generics.pop(), Some(TokenTree::Punct(p)) if p.as_char() == '>');
    }

    // Add a trailing comma if `impl_generics` is non-empty and doesn't have one.
    match impl_generics.last() {
        Some(TokenTree::Punct(p)) if p.as_char() == ',' => {}
        None => {}
        Some(_) => {
            impl_generics.push(parse_quote! { , });
        }
    }

    let impl_generics = impl_generics.into_iter().collect::<TokenStream2>();

    match specified_generic_over_ip.clone() {
        Some((ident, param_type)) => {
            // Emit an impl that substitutes the identified type parameter
            // to produce the new GenericOverIp::Type.
            let generic_ip_name: Ident = parse_quote!(IpType);
            let extra_bounds_target: TypePath = match param_type {
                IpGenericParamType::IpVersion => parse_quote!(#generic_ip_name),
                IpGenericParamType::IpAddress => parse_quote!(#generic_ip_name::Addr),
                IpGenericParamType::GenericOverIp => parse_quote! {
                    <#ident as GenericOverIp<IpType>>::Type
                },
            };

            let bound_if_generic_over_ip = match param_type {
                IpGenericParamType::IpVersion | IpGenericParamType::IpAddress => None,
                IpGenericParamType::GenericOverIp => Some(quote! {
                    #ident: GenericOverIp<IpType>,
                }),
            };

            let generic_bounds = with_type_param_replaced(
                &ast.generics,
                &ident,
                parse_quote! {
                    #extra_bounds_target
                },
            );

            quote! {
                impl <#impl_generics #generic_ip_name: Ip>
                GenericOverIp<IpType> for #name #type_generics
                where #bound_if_generic_over_ip #extra_bounds_target: #(#extra_bounds)+*, {
                    type Type = #name #generic_bounds;
                }
            }
        }
        None => {
            // The type is IP-invariant so `GenericOverIp::Type` is always Self.`
            quote! {
                impl <#impl_generics IpType: Ip> GenericOverIp<IpType> for #name #type_generics {
                    type Type = Self;
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
enum IpGenericParamType {
    IpVersion,
    IpAddress,
    GenericOverIp,
}

impl Parse for IpGenericParamType {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let bound: Ident = input.parse()?;
        let bound_error_message = "the bound passed to generic_over_ip \
                                   must be Ip, IpAddress, or GenericOverIp";

        Ok(match bound.to_string().as_str() {
            "Ip" => IpGenericParamType::IpVersion,
            "IpAddress" => IpGenericParamType::IpAddress,
            "GenericOverIp" => IpGenericParamType::GenericOverIp,
            _ => return Err(syn::Error::new(bound.span(), bound_error_message)),
        })
    }
}

struct GenericOverIpArgs {
    ident: Ident,
    _comma_token: Token![,],
    bound: IpGenericParamType,
}

impl Parse for GenericOverIpArgs {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        Ok(GenericOverIpArgs {
            ident: input.parse()?,
            _comma_token: input.parse()?,
            bound: input.parse()?,
        })
    }
}

fn find_generic_over_ip_attr(
    ast: &syn::DeriveInput,
) -> Result<Option<(Ident, IpGenericParamType)>, syn::Error> {
    let mut attrs = ast
        .attrs
        .iter()
        .filter(|attr| attr.path().get_ident().map(|i| i == "generic_over_ip").unwrap_or(false));
    let attr = attrs.next().ok_or_else(|| {
        syn::Error::new(
            ast.ident.span(),
            "derive(GenericOverIp) cannot be used without the generic_over_ip attribute",
        )
    })?;
    match attrs.next() {
        None => {}
        Some(attr) => {
            return Err(syn::Error::new(
                attr.span(),
                "derive(GenericOverIp) cannot be used with multiple generic_over_ip attributes",
            ));
        }
    }

    let list = match &attr.meta {
        syn::Meta::Path(_) | syn::Meta::NameValue(_) => {
            return Err(syn::Error::new(
                attr.meta.span(),
                "generic_over_ip must be passed at most one\
                    type parameter identifier",
            ));
        }
        syn::Meta::List(list) => list,
    };

    if list.tokens.is_empty() {
        return Ok(None);
    }

    let args: GenericOverIpArgs = syn::parse(list.tokens.clone().into()).map_err(|e| {
        syn::Error::new(
            list.tokens.span(),
            format!(
                "generic_over_ip must be either be passed no \
                        arguments, or one type parameter identifier and its \
                        trait bound (Ip, IpAddress, or GenericOverIp),: {e:?}"
            ),
        )
    })?;

    Ok(Some((args.ident, args.bound)))
}

fn collect_bounds<'a>(
    ident: &'a Ident,
    mut generics: impl Iterator<Item = &'a TypeParam>,
) -> Option<Vec<&'a TypeParamBound>> {
    generics.find_map(|t| if &t.ident == ident { Some(t.bounds.iter().collect()) } else { None })
}

fn with_type_param_replaced(
    generics: &Generics,
    to_find: &Ident,
    replacement: TypePath,
) -> Option<AngleBracketedGenericArguments> {
    let args: Punctuated<_, _> = generics
        .params
        .iter()
        .map(|g| match g {
            GenericParam::Const(c) => GenericArgument::Const(parse_quote!(#c.ident)),
            GenericParam::Lifetime(l) => GenericArgument::Lifetime(l.lifetime.clone()),
            GenericParam::Type(t) => {
                if &t.ident == to_find {
                    GenericArgument::Type(Type::Path(replacement.clone()))
                } else {
                    GenericArgument::Type(Type::Path(TypePath {
                        path: t.ident.clone().into(),
                        qself: None,
                    }))
                }
            }
        })
        .collect();
    (args.len() != 0).then(|| AngleBracketedGenericArguments {
        args,
        colon2_token: None,
        lt_token: Default::default(),
        gt_token: Default::default(),
    })
}
