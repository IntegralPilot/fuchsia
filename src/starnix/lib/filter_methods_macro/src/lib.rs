// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use proc_macro::TokenStream;

enum Filter {
    RoMethod,
    RwMethod,
}

impl Filter {
    fn matches(&self, function: &syn::ItemFn) -> bool {
        let signature_has_mutable_self =
            if let Some(syn::FnArg::Receiver(argument)) = function.sig.inputs.first() {
                argument.mutability.is_some()
            } else {
                false
            };
        let expect_mutable_self = matches!(self, Self::RwMethod);
        signature_has_mutable_self == expect_mutable_self
    }
}

impl syn::parse::Parse for Filter {
    fn parse(input: syn::parse::ParseStream<'_>) -> syn::Result<Self> {
        let ident: syn::Ident = input.parse()?;
        match ident.to_string().as_str() {
            "RoMethod" => Ok(Filter::RoMethod),
            "RwMethod" => Ok(Filter::RwMethod),
            _ => Err(syn::Error::new(ident.span(), "Expected `RoMethod` or `RwMethod`")),
        }
    }
}

struct FilterMethods {
    filter: Filter,
    methods: Vec<syn::ItemFn>,
}

impl syn::parse::Parse for FilterMethods {
    fn parse(input: syn::parse::ParseStream<'_>) -> syn::Result<Self> {
        let filter: Filter = input.parse()?;
        input.parse::<syn::Token![,]>()?;
        let mut methods = vec![];
        while !input.is_empty() {
            let function: syn::ItemFn = input.parse()?;
            methods.push(function);
        }
        Ok(Self { filter, methods })
    }
}

/// The `filter_methods` macro. Requires an input of the form:
///
/// identifier, method*
///
/// where identifier is either RoMethod or RwMethod and method* is a sequence of method
/// definitions.
/// The macro will filter the sequence depending on the identifier.
/// If the identifier is RwMethod, it will only keep the method that have a mutable reference self
/// as parameter.
/// If the identifier is RoMethod, it will keep all other methods.
#[proc_macro]
pub fn filter_methods(input: TokenStream) -> TokenStream {
    let FilterMethods { filter, methods } = syn::parse_macro_input!(input as FilterMethods);

    let mut result = proc_macro2::TokenStream::new();
    for method in methods.into_iter().filter(|f| filter.matches(f)) {
        result.extend(quote::quote! {
            #method
        });
    }
    result.into()
}
