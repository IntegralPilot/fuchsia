// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Data, DataEnum, DataStruct, DeriveInput, Fields, Generics, Ident, parse_macro_input};

#[proc_macro_derive(TypeFingerprint, attributes(serde))]
pub fn derive_type_fingerprint(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let result = match &input.data {
        Data::Struct(data) => handle_struct(&input.ident, &input.generics, data),
        Data::Enum(data) => handle_enum(&input.ident, &input.generics, data),
        Data::Union(_) => Err(syn::Error::new_spanned(&input, "unions are not supported")),
    };
    match result {
        Ok(ts) => ts.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

/// Parses and returns the path in `#[serde(with = "module")]` if present, ignoring other
/// attributes.
fn get_serde_with_attribute(attrs: &[syn::Attribute]) -> syn::Result<Option<syn::Path>> {
    let mut module_path = None;
    for attr in attrs {
        if attr.path().is_ident("serde") {
            let res = attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("with") {
                    let value = meta.value()?;
                    let s: syn::LitStr = value.parse()?;
                    let path = s.parse::<syn::Path>()?;
                    module_path = Some(path);
                } else {
                    // Fully consume other nested serde attributes from the input.
                    if meta.input.peek(syn::Token![=]) {
                        let _value = meta.value()?;
                        let _: syn::Expr = _value.parse()?;
                    } else if meta.input.peek(syn::token::Paren) {
                        let content;
                        syn::parenthesized!(content in meta.input);
                        let _: TokenStream = content.parse()?;
                    }
                }
                Ok(())
            });
            res?;
        }
    }
    Ok(module_path)
}

/// Returns the fingerprint expression for a field, delegating to its `serde(with)` module if
/// annotated.
fn get_fingerprint_expr(field: &syn::Field) -> syn::Result<TokenStream> {
    let field_type = &field.ty;
    if let Some(module_path) = get_serde_with_attribute(&field.attrs)? {
        Ok(quote! { #module_path::fingerprint::<#field_type>() })
    } else {
        Ok(quote! { <#field_type as TypeFingerprint>::fingerprint() })
    }
}

fn handle_fields(fields: &syn::Fields) -> syn::Result<Vec<TokenStream>> {
    match &fields {
        Fields::Unit => Ok(vec![]),
        Fields::Named(fields) => fields
            .named
            .iter()
            .map(|field| {
                let fingerprint_expr = get_fingerprint_expr(field)?;
                if let Some(field_name) = &field.ident {
                    let field_name = field_name.to_string();
                    Ok(quote! { #field_name + ":" + &(#fingerprint_expr) })
                } else {
                    Ok(quote! { &(#fingerprint_expr) })
                }
            })
            .collect(),
        Fields::Unnamed(fields) => fields
            .unnamed
            .iter()
            .map(|field| {
                let fingerprint_expr = get_fingerprint_expr(field)?;
                Ok(quote! { &(#fingerprint_expr) })
            })
            .collect(),
    }
}

fn handle_struct(
    ident: &Ident,
    _generics: &Generics,
    data: &DataStruct,
) -> syn::Result<TokenStream> {
    let fields = handle_fields(&data.fields)?;
    let mut out = quote! { "" };
    for (i, field) in fields.into_iter().enumerate() {
        if i != 0 {
            out = quote! { #out + "," }
        }
        out = quote! { #out + #field }
    }
    Ok(quote! {
        impl TypeFingerprint for #ident {
            fn fingerprint() -> String {
                "struct {".to_owned() + #out + "}"
            }
        }
    })
}

fn handle_enum(ident: &Ident, _generics: &Generics, data: &DataEnum) -> syn::Result<TokenStream> {
    let variants = data
        .variants
        .iter()
        .map(|variant| {
            let name = variant.ident.to_string();
            let fields = handle_fields(&variant.fields)?;
            if fields.is_empty() {
                Ok(quote! { #name })
            } else {
                let mut out = quote! { #name + "(" };
                for (i, field) in fields.into_iter().enumerate() {
                    if i != 0 {
                        out = quote! { #out + "," }
                    }
                    out = quote! { #out + #field }
                }
                Ok(quote! { #out + ")" })
            }
        })
        .collect::<syn::Result<Vec<_>>>()?;
    let mut out = quote! { "" };
    for (i, variant) in variants.into_iter().enumerate() {
        if i != 0 {
            out = quote! { #out + "," }
        }
        out = quote! { #out + #variant }
    }
    Ok(quote! {
        impl TypeFingerprint for #ident {
            fn fingerprint() -> String {
                    "enum {".to_owned() + #out + "}"
            }
        }
    })
}
