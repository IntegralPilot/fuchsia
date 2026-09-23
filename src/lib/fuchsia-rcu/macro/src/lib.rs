// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use proc_macro::TokenStream;
use proc_macro2 as _;
use quote::quote;
use syn::{Data, DeriveInput, Fields, Type, parse_macro_input};

/// Proc macro to allow safe implementation of RcuDroppable if all of a types fields are
/// RcuDroppable.
#[proc_macro_derive(RcuDroppable)]
pub fn derive_rcu_droppable(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = input.ident;

    let field_types = match input.data {
        Data::Struct(data_struct) => collect_fields(&data_struct.fields),
        Data::Enum(data_enum) => {
            data_enum.variants.iter().flat_map(|v| collect_fields(&v.fields)).collect()
        }
        Data::Union(_) => {
            return syn::Error::new_spanned(name, "RcuDroppable cannot be derived for unions")
                .to_compile_error()
                .into();
        }
    };

    // For each field, add a `where` clause requiring it to be RcuDroppable.
    let generics = input.generics;
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    let mut where_clause = where_clause.cloned().unwrap_or_else(|| syn::WhereClause {
        where_token: Default::default(),
        predicates: Default::default(),
    });

    for ty in field_types {
        where_clause.predicates.push(syn::parse_quote!(#ty: RcuDroppable));
    }

    for param in generics.type_params() {
        let param_ident = &param.ident;
        where_clause.predicates.push(syn::parse_quote!(#param_ident: RcuDroppable));
    }

    // If each field is RcuDroppable, then the whole type is RcuDroppable.
    let expanded = quote! {
        // SAFETY: All fields implement RcuDroppable as enforced by the generated where clause
        // bounds.
        unsafe impl #impl_generics RcuDroppable for #name #ty_generics #where_clause {}
    };

    TokenStream::from(expanded)
}

fn collect_fields(fields: &Fields) -> Vec<Type> {
    match fields {
        Fields::Named(fields) => fields.named.iter().map(|f| f.ty.clone()).collect(),
        Fields::Unnamed(fields) => fields.unnamed.iter().map(|f| f.ty.clone()).collect(),
        Fields::Unit => Vec::new(),
    }
}
