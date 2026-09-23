// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The following proc macros are related to code under src/storage/fxfs/src/serialized_types.

#[macro_use]
extern crate quote;

use proc_macro::TokenStream;
use quote::ToTokens;
use std::collections::BTreeMap;
use syn::parse::{Parse, ParseStream};
use syn::{Data, Fields, Index, Result, parse_macro_input};

/// Holds an open-ended version range like `3..` meaning version 3 and up.
#[derive(Clone)]
struct PatOpenVersionRange {
    lo: syn::LitInt,
    dots: syn::token::DotDot,
}
impl PatOpenVersionRange {
    fn lo_value(&self) -> u32 {
        self.lo.base10_parse::<u32>().unwrap_or(0)
    }
}
impl std::cmp::PartialEq for PatOpenVersionRange {
    fn eq(&self, other: &Self) -> bool {
        self.lo_value() == other.lo_value()
    }
}
impl std::cmp::PartialOrd for PatOpenVersionRange {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        self.lo_value().partial_cmp(&other.lo_value())
    }
}
impl Parse for PatOpenVersionRange {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        Ok(Self { lo: input.parse()?, dots: input.parse()? })
    }
}
impl ToTokens for PatOpenVersionRange {
    fn to_tokens(&self, tokens: &mut proc_macro2::TokenStream) {
        self.lo.to_tokens(tokens);
        self.dots.to_tokens(tokens);
    }
}

/// Holds a "fat arrow" mapping from version range to type. e.g. `3.. => FooV3,`
struct Arm {
    pat: PatOpenVersionRange,
    _fat_arrow_token: syn::token::FatArrow,
    ident: syn::Ident,
}
impl Parse for Arm {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        Ok(Self { pat: input.parse()?, _fat_arrow_token: input.parse()?, ident: input.parse()? })
    }
}

struct Input {
    arms: Vec<Arm>,
}
impl Parse for Input {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        Ok(Self { arms: input.parse_terminated(Arm::parse, syn::Token![,])?.into_iter().collect() })
    }
}

/// Implements traits for versioned structures.
/// Namely:
///   * [Versioned] for all versions.
///   * [VersionedLatest] for the most recent version.
///   * Transitive [From] for any version to a newer version.
#[proc_macro]
pub fn versioned_type(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as Input);
    let arms = input.arms;

    let versions: BTreeMap<u32, syn::Ident> =
        arms.iter().map(|x| (x.pat.lo_value(), x.ident.clone())).collect();
    assert_eq!(arms.len(), versions.len(), "Duplicate version range found.");

    let mut out = quote! {};

    // The latest version should implement VersionedLatest.
    if let Some((major, ident)) = versions.iter().last() {
        // Generate some static assertions up front to check the versions are sensible.
        let assertions = arms.iter().map(|x| x.pat.lo_value()).map(|v| {
            quote! {
                static_assertions::const_assert!(#v <= LATEST_VERSION.major);
            }
        });
        // We order versions in their original match order.
        let match_iter = arms.iter().map(|x| (x.pat.lo_value(), x.ident.clone())).map(|(v, i)| {
            quote! { #v.. => Ok(#i::deserialize_from(reader, version)?.into()), }
        });
        out = quote! {
            #out
            impl VersionedLatest for #ident {
                fn deserialize_from_version<R>(
                    reader: &mut R, version: Version) -> anyhow::Result<Self>
                where R: std::io::Read, Self: Sized {
                    #(#assertions)*
                    assert!(#major <= LATEST_VERSION.major,
                        "Found version > LATEST_VERSION for {}.", stringify!(#ident));
                    const FUTURE_VER : u32 = LATEST_VERSION.major + 1;
                    match version.major {
                        FUTURE_VER.. => anyhow::bail!(
                                "Invalid future version {} > {} deserializing {}.",
                                version, LATEST_VERSION, stringify!(#ident)),
                        #(#match_iter)*
                        x => anyhow::bail!(
                                "Unsupported version {} for {}.", x, stringify!(#ident)),
                    }
                }
            }
        };
    }

    // The [From] ladder is a little tricky because users are free to repeat types
    // in their version mapping. We use our sequence of versions.
    let mut idents: Vec<&syn::Ident> = versions.values().collect();
    idents.dedup();
    let last_ident = idents.pop().unwrap();

    for i in 0..idents.len().saturating_sub(1) {
        let ident = &idents[i];
        let next_ident = &idents[i + 1];
        out = quote! {
            #out
            impl From<#ident> for #last_ident {
                fn from(item: #ident) -> Self {
                    let tmp: #next_ident = item.into();
                    tmp.into()
                }
            }
        }
    }
    TokenStream::from(out)
}

/// This is just a shorthand for `impl Versioned for Foo {}` which is required for all top-level
/// versioned struct/enum in Fxfs.
#[proc_macro_derive(Versioned)]
pub fn derive_versioned(input: TokenStream) -> TokenStream {
    let syn::DeriveInput { ident, .. } = parse_macro_input!(input);
    TokenStream::from(quote! { impl Versioned for #ident {} })
}

/// This does nothing by itself, but with the Migrate derive macro, it signals that the struct being
/// migrated to, does not require a Default implementation, since all the fields are available in
/// both.
#[proc_macro_attribute]
pub fn migrate_nodefault(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}

/// This does nothing by itself, but with the Migrate derive macro, it defines the target to
/// migrate to. Without this, Migrate will make a From impl for IdentVX to Ident.
#[proc_macro_attribute]
pub fn migrate_to_version(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}

/// Adds a From implementation to help migrate structs or enums.  This will support the following
/// migrations:
///
///   1. Adding a new member to a struct where the member has a Default implementation.
///   2. Changing the type of a member with the same name, where all the fields implement From.
///      Default is not needed in this case by including the the migrate_nodefault attribute.
///   3. Changing an enum variant's fields where the all the fields implement From.
///
/// This will also work if a new variant is added to an enum, but if that's the only change, and
/// it's added to the end of the existing variants, it will automatically be backward compatible
/// because it won't affect how bincode serializes the old variants.  There are some other changes
/// that can be made that are safe with bincode serialization, such as converting from Option to
/// Vec.
#[proc_macro_derive(Migrate)]
pub fn derive_migrate(input: TokenStream) -> TokenStream {
    let input: syn::DeriveInput = parse_macro_input!(input);

    let ident = input.ident;
    let target = if let Some(attr) = input
        .attrs
        .iter()
        .find(|a| a.style == syn::AttrStyle::Outer && a.path().is_ident("migrate_to_version"))
    {
        match attr.parse_args::<syn::Type>() {
            Ok(ident) => ident.into_token_stream(),
            Err(error) => error.into_compile_error(),
        }
    } else {
        match format!("{}", ident).rsplit_once('V') {
            Some((ident, _)) => format_ident!("{}", ident).into_token_stream(),
            None => syn::Error::new(
                ident.span(),
                "struct or enum must have V in the name to be migrated.",
            )
            .into_compile_error(),
        }
    };

    let out = match input.data {
        Data::Enum(e) => {
            let mut arms = quote! {};
            for variant in e.variants {
                let var_ident = variant.ident;
                let (fields, result) = match variant.fields {
                    Fields::Named(fields) => {
                        let field_names: Vec<_> = fields.named.iter().map(|f| &f.ident).collect();
                        (
                            quote! { { #(#field_names),* } },
                            quote! {
                                #target::#var_ident { #(#field_names: #field_names.into()),* }
                            },
                        )
                    }
                    Fields::Unnamed(fields) => {
                        let len = fields.unnamed.len();
                        let field_names: Vec<_> =
                            (0..len).map(|i| format_ident!("f{}", i)).collect();
                        (
                            quote! { (#(#field_names),*) },
                            quote! { #target::#var_ident(#(#field_names.into()),*) },
                        )
                    }
                    Fields::Unit => (quote! {}, quote! { #target::#var_ident }),
                };

                arms.extend(quote! {
                    #ident::#var_ident #fields => #result,
                });
            }

            quote! {
                impl From<#ident> for #target {
                    fn from(from: #ident) -> Self {
                        match from {
                            #arms
                        }
                    }
                }
            }
        }
        Data::Struct(s) => {
            let body = match s.fields {
                Fields::Named(fields) => {
                    let fields = fields.named.iter().map(|f| {
                        let name = &f.ident;
                        match f.ty.to_token_stream().into_iter().next() {
                            Some(proc_macro2::TokenTree::Ident(ident)) if ident == "Option" => {
                                quote! { #name: from.#name.map(Into::into) }
                            }
                            _ => quote! { #name: from.#name.into() },
                        }
                    });

                    let default_string = if input
                        .attrs
                        .iter()
                        .filter(|a| {
                            a.style == syn::AttrStyle::Outer
                                && a.path().is_ident("migrate_nodefault")
                        })
                        .next()
                        .is_none()
                    {
                        quote! {
                            ..Default::default()
                        }
                    } else {
                        quote! {}
                    };

                    quote! {
                        #target {
                            #(#fields),*,
                            #default_string
                        }
                    }
                }
                Fields::Unnamed(fields) => {
                    let fields = fields.unnamed.iter().enumerate().map(|(i, f)| {
                        let i = Index::from(i);
                        match f.ty.to_token_stream().into_iter().next() {
                            Some(proc_macro2::TokenTree::Ident(ident)) if ident == "Option" => {
                                quote! { from.#i.map(Into::into) }
                            }
                            _ => quote! { from.#i.into() },
                        }
                    });
                    quote! {
                        #target (
                            #(#fields),*,
                        )
                    }
                }
                Fields::Unit => quote! {},
            };

            quote! {
                impl From<#ident> for #target {
                    fn from(from: #ident) -> Self {
                        #body
                    }
                }
            }
        }
        _ => unimplemented!(),
    };

    // eprintln!("TOKENS: {}", out);

    out.into()
}

#[proc_macro_derive(ToWeakNode)]
pub fn derive_to_weak_node(input: TokenStream) -> TokenStream {
    let input: syn::DeriveInput = parse_macro_input!(input);
    let node = input.ident;
    quote! {
        impl crate::node::ToWeakNode for #node {
            fn to_weak_node(self: Arc<Self>) -> crate::node::WeakNode {
                use {
                    crate::node::{FxNode, ToWeakNode, WeakNode, WeakNodeVTable},
                    std::{
                        any::TypeId,
                        mem::ManuallyDrop,
                        sync::{Arc, Weak},
                    },
                };

                unsafe fn drop(ptr: *const ()) {
                    Weak::from_raw(ptr as *const #node);
                }

                unsafe fn type_id() -> TypeId {
                    TypeId::of::<#node>()
                }

                unsafe fn upgrade(ptr: *const ()) -> Option<Arc<dyn FxNode>> {
                    ManuallyDrop::new(Weak::from_raw(ptr as *const #node))
                        .upgrade()
                        .map(|a| a as Arc<dyn FxNode>)
                }

                static VTABLE: WeakNodeVTable = WeakNodeVTable::new(drop, type_id, upgrade);

                // SAFETY: We've implemented `drop`, `type_id` and `upgrade` correctly.  The code in
                // `node.rs` requires us to use `Weak::into_raw` here.
                unsafe { WeakNode::new(&VTABLE, Arc::downgrade(&self).into_raw() as *const ()) }
            }
        }
    }
    .into()
}

/// Implements a trivial FuzzyHash for non-range-based keys.
#[proc_macro_derive(FuzzyHash)]
pub fn derive_fuzzy_hash(input: TokenStream) -> TokenStream {
    let syn::DeriveInput { ident, .. } = parse_macro_input!(input);
    TokenStream::from(quote! {
        impl FuzzyHash for #ident {
            fn fuzzy_hash(&self) -> impl std::iter::ExactSizeIterator<Item = u64> {
                let hash = crate::stable_hash::stable_hash(self);
                std::iter::once(hash)
            }
        }
    })
}

#[proc_macro]
pub fn impl_fuzzy_hash(input: TokenStream) -> TokenStream {
    let ident: syn::Type = parse_macro_input!(input);
    TokenStream::from(quote! {
        impl FuzzyHash for #ident {
            fn fuzzy_hash(&self) -> impl std::iter::ExactSizeIterator<Item = u64> {
                let hash = crate::stable_hash::stable_hash(self);
                std::iter::once(hash)
            }
        }
    })
}

#[proc_macro_derive(SerializeKey)]
pub fn derive_serialize_key(input: TokenStream) -> TokenStream {
    let input: syn::DeriveInput = parse_macro_input!(input);
    let ident = input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    let serialize_body = match &input.data {
        Data::Struct(s) => match &s.fields {
            Fields::Named(fields) => {
                let fields = fields.named.iter().map(|f| {
                    let name = &f.ident;
                    quote! { self.#name.serialize_key_to(serializer); }
                });
                quote! { #(#fields)* }
            }
            Fields::Unnamed(fields) => {
                let fields = fields.unnamed.iter().enumerate().map(|(i, _)| {
                    let i = Index::from(i);
                    quote! { self.#i.serialize_key_to(serializer); }
                });
                quote! { #(#fields)* }
            }
            Fields::Unit => quote! {},
        },
        Data::Enum(e) => {
            let arms = e.variants.iter().enumerate().map(|(i, variant)| {
                let var_ident = &variant.ident;
                let idx = i as u8;
                match &variant.fields {
                    Fields::Named(fields) => {
                        let field_names: Vec<_> = fields.named.iter().map(|f| &f.ident).collect();
                        quote! {
                            Self::#var_ident { #(#field_names),* } => {
                                serializer.write_bytes(&#idx.to_be_bytes());
                                #(#field_names.serialize_key_to(serializer);)*
                            }
                        }
                    }
                    Fields::Unnamed(fields) => {
                        let field_names: Vec<_> = (0..fields.unnamed.len())
                            .map(|i| quote::format_ident!("f{}", i))
                            .collect();
                        quote! {
                            Self::#var_ident(#(#field_names),*) => {
                                serializer.write_bytes(&#idx.to_be_bytes());
                                #(#field_names.serialize_key_to(serializer);)*
                            }
                        }
                    }
                    Fields::Unit => {
                        quote! {
                            Self::#var_ident => {
                                serializer.write_bytes(&#idx.to_be_bytes());
                            }
                        }
                    }
                }
            });
            quote! {
                match self {
                    #(#arms)*
                }
            }
        }
        _ => unimplemented!("Unions not supported"),
    };

    let deserialize_body = match &input.data {
        Data::Struct(s) => match &s.fields {
            Fields::Named(fields) => {
                let field_names = fields.named.iter().map(|f| &f.ident);
                let field_tys = fields.named.iter().map(|f| &f.ty);
                quote! {
                    Ok(Self {
                        #( #field_names: <#field_tys as crate::serialized_types::serialized_key::SerializeKey>::deserialize_key_from(deserializer)? ),*
                    })
                }
            }
            Fields::Unnamed(fields) => {
                let field_tys = fields.unnamed.iter().map(|f| &f.ty);
                quote! {
                    Ok(Self (
                        #( <#field_tys as crate::serialized_types::serialized_key::SerializeKey>::deserialize_key_from(deserializer)? ),*
                    ))
                }
            }
            Fields::Unit => quote! { Ok(Self) },
        },
        Data::Enum(e) => {
            let arms = e.variants.iter().enumerate().map(|(i, variant)| {
                let var_ident = &variant.ident;
                let idx = i as u8;
                match &variant.fields {
                    Fields::Named(fields) => {
                        let field_names = fields.named.iter().map(|f| &f.ident);
                        let field_tys = fields.named.iter().map(|f| &f.ty);
                        quote! {
                            #idx => Ok(Self::#var_ident {
                                #( #field_names: <#field_tys as crate::serialized_types::serialized_key::SerializeKey>::deserialize_key_from(deserializer)? ),*
                            }),
                        }
                    }
                    Fields::Unnamed(fields) => {
                        let field_tys = fields.unnamed.iter().map(|f| &f.ty);
                        quote! {
                            #idx => Ok(Self::#var_ident (
                                #( <#field_tys as crate::serialized_types::serialized_key::SerializeKey>::deserialize_key_from(deserializer)? ),*
                            )),
                        }
                    }
                    Fields::Unit => {
                        quote! {
                            #idx => Ok(Self::#var_ident),
                        }
                    }
                }
            });
            quote! {
                let tag = <u8 as crate::serialized_types::serialized_key::SerializeKey>::deserialize_key_from(deserializer)?;
                match tag {
                    #(#arms)*
                    _ => anyhow::bail!("Invalid enum tag {} for {}", tag, stringify!(#ident)),
                }
            }
        }
        _ => unimplemented!("Unions not supported"),
    };

    TokenStream::from(quote! {
        impl #impl_generics crate::serialized_types::serialized_key::SerializeKey for #ident #ty_generics #where_clause {
            fn serialize_key_to<B: crate::serialized_types::varint::Buffer>(&self, serializer: &mut crate::serialized_types::serialized_key::KeySerializer<'_, B>) {
                use crate::serialized_types::serialized_key::SerializeKey as _;
                #serialize_body
            }
            fn deserialize_key_from(deserializer: &mut crate::serialized_types::serialized_key::KeyDeserializer<'_>) -> anyhow::Result<Self> {
                use crate::serialized_types::serialized_key::SerializeKey as _;
                #deserialize_body
            }
        }
    })
}
