// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use proc_macro2::{Ident, TokenStream};
use quote::{quote, quote_spanned};
use syn::spanned::Spanned;
use syn::{DataEnum, DeriveInput, FieldsNamed, parse_macro_input};

#[proc_macro_attribute]
pub fn assembly_container(
    attr: proc_macro::TokenStream,
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    let config_filename = attr.to_string();
    let input = parse_macro_input!(input as DeriveInput);
    let name = &input.ident;
    proc_macro::TokenStream::from(quote! {
        #input
        impl AssemblyContainer for #name {
            fn get_config_filename() -> &'static str {
                #config_filename
            }
        }
    })
}

#[proc_macro_derive(WalkPaths, attributes(walk_paths))]
pub fn walk_paths_derive(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = input.ident;

    let implementation = match &input.data {
        syn::Data::Struct(data) => match &data.fields {
            syn::Fields::Unit => quote! {Self},
            syn::Fields::Named(fields) => handle_named_struct(fields),
            syn::Fields::Unnamed(_) => unimplemented!(
                "Structs with unnamed fields are not supported in the WalkPaths derive macro."
            ),
        },
        syn::Data::Enum(data) => handle_enum(data),
        syn::Data::Union(_) => {
            unimplemented!("Unions are not supported in the WalkPaths derive macro.")
        }
    };

    proc_macro::TokenStream::from(quote! {
        impl assembly_container::WalkPaths for #name {
            fn walk_paths_with_dest<F: assembly_container::WalkPathsFn>(&mut self, found: &mut F, dest: camino::Utf8PathBuf) -> anyhow::Result<()> {
                #implementation
                Ok(())
            }
        }
    })
}

fn get_field_names(fields: &FieldsNamed) -> TokenStream {
    let mut output = Vec::new();
    for field in &fields.named {
        if !output.is_empty() {
            output.push(quote! {,});
        }
        let name = &field.ident;
        output.push(quote! {#name})
    }

    TokenStream::from_iter(output.into_iter())
}

fn get_field_indexes(fields: &syn::FieldsUnnamed) -> TokenStream {
    let mut output = Vec::new();
    for (index, field) in fields.unnamed.iter().enumerate() {
        if !output.is_empty() {
            output.push(quote! {,})
        }
        let named_index = Ident::new(&format!("field_{index}"), field.span());
        output.push(quote! { #named_index })
    }
    TokenStream::from_iter(output.into_iter())
}

fn handle_named_struct(fields: &FieldsNamed) -> TokenStream {
    TokenStream::from_iter(fields.named.iter().map(|field| {
        let name = &field.ident;
        let name_string = name.as_ref().unwrap().to_string();

        if field.attrs.iter().any(|a| a.path().is_ident("walk_paths")) {
            quote_spanned! {
                field.span() => {
                    let dest = dest.join(#name_string.to_string());
                    self.#name.walk_paths_with_dest(found, dest)?;
                }
            }
        } else {
            quote! {}
        }
    }))
}

fn handle_enum(data_enum: &DataEnum) -> TokenStream {
    let variants = TokenStream::from_iter(data_enum.variants.iter().map(|variant| {
        let variant_name = &variant.ident;
        let variant_name_string = variant_name.to_string();

        match &variant.fields {
            syn::Fields::Unit => quote! {
                Self::#variant_name => {},
            },
            syn::Fields::Named(fields) => {
                let field_names = get_field_names(fields);
                let implementation = TokenStream::from_iter(fields.named.iter().map(|field| {
                    let name = &field.ident;
                    let name_string = name.as_ref().unwrap().to_string();

                    quote_spanned! {
                        field.span() => {
                            let dest = dest.join(#variant_name_string).join(#name_string.to_string());
                            #name.walk_paths_with_dest(found, dest)?;
                        }
                    }
                }));
                quote! {
                    Self::#variant_name{#field_names} => { #implementation },
                }
            }
            syn::Fields::Unnamed(fields) => {
                let indexes = get_field_indexes(&fields);
                let implementation = TokenStream::from_iter(fields.unnamed.iter().enumerate().map(
                    |(index, field)| {
                        let named_index = Ident::new(&format!("field_{index}"), field.span());
                        let name_string = variant_name.to_string();

                        quote_spanned! {
                            field.span() => {
                                let dest = dest.join(#name_string.to_string());
                                #named_index.walk_paths_with_dest(found, dest)?;
                            }
                        }
                    },
                ));
                quote! {
                    Self::#variant_name(#indexes) => { #implementation },
                }
            }
        }
    }));

    quote! {
        match self {
            #variants
        }
    }
}
