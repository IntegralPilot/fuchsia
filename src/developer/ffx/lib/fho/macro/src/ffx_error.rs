// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use proc_macro2::{Span, TokenStream};
use quote::quote;
use syn::spanned::Spanned;
use syn::{Data, DeriveInput, Fields};

pub fn generate_error_impl(ast: &DeriveInput) -> Result<TokenStream, crate::errors::ParseError> {
    let name = &ast.ident;
    let (impl_generics, ty_generics, where_clause) = ast.generics.split_for_impl();

    let variants = match &ast.data {
        Data::Enum(de) => &de.variants,
        _ => {
            return Ok(syn::Error::new(
                ast.span(),
                "#[derive(FfxError)] can only be applied to enums",
            )
            .to_compile_error());
        }
    };

    let mut match_arms = Vec::new();

    for variant in variants {
        let var_name = &variant.ident;

        let mut unexpected = false;
        let mut user = false;
        let mut transparent = false;
        let mut exit_code = None;

        for attr in &variant.attrs {
            if attr.path().is_ident("unexpected") {
                unexpected = true;
            } else if attr.path().is_ident("user") {
                user = true;
            } else if attr.path().is_ident("transparent") {
                transparent = true;
            } else if attr.path().is_ident("exit_with_code") {
                let code: syn::LitInt = attr.parse_args().map_err(|e| {
                    crate::errors::ParseError::InvalidTargetAttr(
                        attr.span(),
                        format!("Invalid #[exit_with_code] argument: {e}"),
                    )
                })?;
                exit_code = Some(code);
            }
        }

        let count =
            (unexpected as u8) + (user as u8) + (transparent as u8) + (exit_code.is_some() as u8);
        if count != 1 {
            return Ok(syn::Error::new(
                variant.span(),
                "Each variant must have exactly one of #[unexpected], #[user], #[transparent] or #[exit_with_code(code)] attributes",
            )
            .to_compile_error());
        }

        let (pat, constructor_args) = match &variant.fields {
            Fields::Unit => (quote! {}, quote! {}),
            Fields::Unnamed(fields) => {
                let bindings = (0..fields.unnamed.len())
                    .map(|i| syn::Ident::new(&format!("x{i}"), Span::call_site()))
                    .collect::<Vec<_>>();
                (quote! { ( #(#bindings),* ) }, quote! { ( #(#bindings),* ) })
            }
            Fields::Named(fields) => {
                let bindings =
                    fields.named.iter().map(|f| f.ident.as_ref().unwrap()).collect::<Vec<_>>();
                (quote! { { #(#bindings),* } }, quote! { { #(#bindings),* } })
            }
        };

        let arm = if unexpected {
            quote! {
                #name::#var_name #pat => {
                    fho::macro_deps::fho::Error::Unexpected(
                        fho::macro_deps::anyhow::Error::new(#name::#var_name #constructor_args)
                    )
                }
            }
        } else if user {
            quote! {
                #name::#var_name #pat => {
                    fho::macro_deps::fho::Error::User(
                        fho::macro_deps::anyhow::Error::new(#name::#var_name #constructor_args)
                    )
                }
            }
        } else if transparent {
            if variant.fields.len() != 1 {
                return Ok(syn::Error::new(
                    variant.span(),
                    "#[transparent] variant must have exactly one field",
                )
                .to_compile_error());
            }
            quote! {
                #name::#var_name (x0) => {
                    fho::macro_deps::fho::Error::from(x0)
                }
            }
        } else {
            let code = exit_code.unwrap();
            quote! {
                #name::#var_name #pat => {
                    fho::macro_deps::fho::Error::from(
                        fho::macro_deps::anyhow::Error::from(
                            errors::FfxError::Error(
                                fho::macro_deps::anyhow::Error::new(#name::#var_name #constructor_args),
                                #code,
                            )
                        )
                    )
                }
            }
        };

        match_arms.push(arm);
    }

    Ok(quote! {
        impl #impl_generics From<#name #ty_generics> for fho::macro_deps::fho::Error #where_clause {
            fn from(e: #name #ty_generics) -> Self {
                match e {
                    #(#match_arms)*
                }
            }
        }
    })
}
