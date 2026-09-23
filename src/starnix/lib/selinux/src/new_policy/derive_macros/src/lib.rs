// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use proc_macro::TokenStream;
use quote::quote;
use syn::{
    Attribute, Data, DeriveInput, Fields, Generics, Ident, Result, TypeParamBound,
    parse_macro_input,
};

fn add_trait_bounds(mut generics: Generics, bound: TypeParamBound) -> Generics {
    for param in generics.type_params_mut() {
        param.bounds.push(bound.clone());
    }
    generics
}

/// Derives `Parse` for a struct or a unit enum.
///
/// # Structs
/// For structs, it parses fields sequentially. Every field must implement `Parse`.
///
/// ```rust
/// #[derive(Parse)]
/// struct Point {
///     x: u32,
///     y: u32,
/// }
/// ```
///
/// Generates:
/// ```rust
/// impl Parse for Point {
///     fn parse(cursor: &mut PolicyCursor<'_>) -> Result<Self, ParseError> {
///         Ok(Self {
///             x: Parse::parse(cursor)?,
///             y: Parse::parse(cursor)?,
///         })
///     }
/// }
/// ```
///
/// # Enums
/// For enums, it requires the `#[policy(wire_type = <type>)]` attribute (e.g., `#[policy(wire_type = u32)]`).
/// It parses the specified integer type from the cursor and matches it against the explicit
/// discriminants of the enum variants.
///
/// All variants must be unit variants (no payloads) and must have explicit discriminants.
/// If the parsed value does not match any discriminant, it returns `ParseError::InvalidEnumValue`.
///
/// ```rust
/// #[derive(Parse)]
/// #[policy(wire_type = u32)]
/// enum Color {
///     Red = 1,
///     Blue = 2,
/// }
/// ```
///
/// Generates:
/// ```rust
/// impl Parse for Color {
///     fn parse(cursor: &mut PolicyCursor<'_>) -> Result<Self, ParseError> {
///         let value = u32::parse(cursor)?;
///         match value {
///             1 => Ok(Self::Red),
///             2 => Ok(Self::Blue),
///             _ => Err(ParseError::InvalidEnumValue { enum_name: "Color", value: value as u64 }),
///         }
///     }
/// }
/// ```
#[proc_macro_derive(Parse, attributes(policy))]
pub fn derive_parse(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = input.ident;
    let generics =
        add_trait_bounds(input.generics, syn::parse_quote!(crate::new_policy::traits::Parse));
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    let wire_type = match get_wire_type(&input.attrs) {
        Ok(t) => t,
        Err(e) => return e.to_compile_error().into(),
    };

    let parse_impl = match &input.data {
        Data::Struct(data) => {
            if wire_type.is_some() {
                return syn::Error::new_spanned(
                    &name,
                    "Structs do not support #[policy(wire_type = ...)] attribute",
                )
                .to_compile_error()
                .into();
            }
            match gen_struct_parse(data) {
                Ok(tokens) => tokens,
                Err(e) => return e.to_compile_error().into(),
            }
        }
        Data::Enum(data) => {
            let Some(wire_type) = &wire_type else {
                return syn::Error::new_spanned(
                    &name,
                    "Enums require #[policy(wire_type = ...)] attribute to derive Parse",
                )
                .to_compile_error()
                .into();
            };
            match gen_enum_parse(&name, data, wire_type) {
                Ok(tokens) => tokens,
                Err(e) => return e.to_compile_error().into(),
            }
        }
        _ => {
            return syn::Error::new_spanned(
                &name,
                "Only structs and enums are supported by Parse derive",
            )
            .to_compile_error()
            .into();
        }
    };

    let expanded = quote! {
        impl #impl_generics crate::new_policy::traits::Parse for #name #ty_generics #where_clause {
            fn parse(cursor: &mut crate::new_policy::parser::PolicyCursor<'_>) -> Result<Self, crate::new_policy::error::ParseError> {
                #parse_impl
            }
        }
    };

    expanded.into()
}

/// Derives `Serialize` for a struct or a unit enum.
///
/// # Structs
/// For structs, it serializes fields sequentially. Every field must implement `Serialize`.
///
/// ```rust
/// #[derive(Serialize)]
/// struct Point {
///     x: u32,
///     y: u32,
/// }
/// ```
///
/// Generates:
/// ```rust
/// impl Serialize for Point {
///     fn serialize(&self, writer: &mut Vec<u8>) -> Result<(), SerializeError> {
///         Serialize::serialize(&self.x, writer)?;
///         Serialize::serialize(&self.y, writer)?;
///         Ok(())
///     }
/// }
/// ```
///
/// # Enums
/// For enums, it requires the `#[policy(wire_type = <type>)]` attribute. It casts the enum
/// to the specified integer type (using `as`) and serializes it.
///
/// All variants must be unit variants and must have explicit discriminants.
///
/// ```rust
/// #[derive(Serialize)]
/// #[policy(wire_type = u32)]
/// enum Color {
///     Red = 1,
///     Blue = 2,
/// }
/// ```
///
/// Generates:
/// ```rust
/// impl Serialize for Color {
///     fn serialize(&self, writer: &mut Vec<u8>) -> Result<(), SerializeError> {
///         let value = *self as u32;
///         Serialize::serialize(&value, writer)?;
///         Ok(())
///     }
/// }
/// ```
#[proc_macro_derive(Serialize, attributes(policy))]
pub fn derive_serialize(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = input.ident;
    let generics =
        add_trait_bounds(input.generics, syn::parse_quote!(crate::new_policy::traits::Serialize));
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    let wire_type = match get_wire_type(&input.attrs) {
        Ok(t) => t,
        Err(e) => return e.to_compile_error().into(),
    };

    let serialize_impl = match &input.data {
        Data::Struct(data) => {
            if wire_type.is_some() {
                return syn::Error::new_spanned(
                    &name,
                    "Structs do not support #[policy(wire_type = ...)] attribute",
                )
                .to_compile_error()
                .into();
            }
            match gen_struct_serialize(data) {
                Ok(tokens) => tokens,
                Err(e) => return e.to_compile_error().into(),
            }
        }
        Data::Enum(data) => {
            let Some(wire_type) = &wire_type else {
                return syn::Error::new_spanned(
                    &name,
                    "Enums require #[policy(wire_type = ...)] attribute to derive Serialize",
                )
                .to_compile_error()
                .into();
            };
            match gen_enum_serialize(data, wire_type) {
                Ok(tokens) => tokens,
                Err(e) => return e.to_compile_error().into(),
            }
        }
        _ => {
            return syn::Error::new_spanned(
                &name,
                "Only structs and enums are supported by Serialize derive",
            )
            .to_compile_error()
            .into();
        }
    };

    let expanded = quote! {
        impl #impl_generics crate::new_policy::traits::Serialize for #name #ty_generics #where_clause {
            fn serialize(&self, writer: &mut crate::new_policy::parser::PolicyWriter<'_>) -> Result<(), crate::new_policy::error::SerializeError> {
                #serialize_impl
                Ok(())
            }
        }
    };

    expanded.into()
}

/// Derives `Validate` for a struct or a unit enum.
///
/// # Structs
/// For structs, it validates fields sequentially. Every field must implement `Validate`.
///
/// ```rust
/// #[derive(Validate)]
/// struct Point {
///     x: u32,
///     y: u32,
/// }
/// ```
///
/// Generates:
/// ```rust
/// impl Validate for Point {
///     fn validate(&self, policy: &NewPolicy) -> Result<(), ValidateError> {
///         Validate::validate(&self.x, policy)?;
///         Validate::validate(&self.y, policy)?;
///         Ok(())
///     }
/// }
/// ```
///
/// # Enums
/// For enums, it requires the `#[policy(wire_type = <type>)]` attribute. Since these are
/// unit enums, the generated implementation is a trivial `Ok(())`.
///
/// All variants must be unit variants.
///
/// ```rust
/// #[derive(Validate)]
/// #[policy(wire_type = u32)]
/// enum Color {
///     Red = 1,
///     Blue = 2,
/// }
/// ```
///
/// Generates:
/// ```rust
/// impl Validate for Color {
///     fn validate(&self, _policy: &NewPolicy) -> Result<(), ValidateError> {
///         Ok(())
///     }
/// }
/// ```
#[proc_macro_derive(Validate, attributes(policy))]
pub fn derive_validate(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = input.ident;
    let generics =
        add_trait_bounds(input.generics, syn::parse_quote!(crate::new_policy::traits::Validate));
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    let validate_impl = match &input.data {
        Data::Struct(data) => match gen_struct_validate(data) {
            Ok(tokens) => tokens,
            Err(e) => return e.to_compile_error().into(),
        },
        Data::Enum(data) => match gen_enum_validate(data) {
            Ok(tokens) => tokens,
            Err(e) => return e.to_compile_error().into(),
        },
        _ => {
            return syn::Error::new_spanned(
                &name,
                "Only structs and enums are supported by Validate derive",
            )
            .to_compile_error()
            .into();
        }
    };

    let expanded = quote! {
        impl #impl_generics crate::new_policy::traits::Validate for #name #ty_generics #where_clause {
            fn validate(&self, policy: &crate::new_policy::NewPolicy) -> Result<(), crate::new_policy::error::ValidateError> {
                #validate_impl
                Ok(())
            }
        }
    };

    expanded.into()
}

/// This macro generates an implementation of `HasName` which provides access to a field named `name` via the `name()` method.
///
/// Requires the struct to have a field named `name`.
#[proc_macro_derive(HasName)]
pub fn derive_has_name(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    let fields = match &input.data {
        Data::Struct(data) => &data.fields,
        _ => {
            return syn::Error::new_spanned(&name, "HasName can only be derived for structs")
                .to_compile_error()
                .into();
        }
    };

    let has_name_field =
        fields.iter().any(|f| f.ident.as_ref().map_or(false, |ident| ident == "name"));

    if !has_name_field {
        return syn::Error::new_spanned(&name, "HasName derive requires a field named 'name'")
            .to_compile_error()
            .into();
    }

    let expanded = quote! {
        impl #impl_generics crate::new_policy::traits::HasName for #name #ty_generics #where_clause {
            fn name(&self) -> &[u8] {
                &self.name
            }
        }
    };

    expanded.into()
}

/// This macro generates an implementation of `HasPolicyId` which provides access to a field named `id` via the `id()` method.
///
/// Requires the struct to have a field named `id`, and uses its type as the associated `Id` type.
#[proc_macro_derive(HasPolicyId)]
pub fn derive_has_policy_id(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    let fields = match &input.data {
        Data::Struct(data) => &data.fields,
        _ => {
            return syn::Error::new_spanned(&name, "HasPolicyId can only be derived for structs")
                .to_compile_error()
                .into();
        }
    };

    let id_field_type = fields.iter().find_map(|f| {
        if f.ident.as_ref().map_or(false, |ident| ident == "id") { Some(&f.ty) } else { None }
    });

    let Some(id_field_type) = id_field_type else {
        return syn::Error::new_spanned(&name, "HasPolicyId derive requires a field named 'id'")
            .to_compile_error()
            .into();
    };

    let expanded = quote! {
        impl #impl_generics crate::new_policy::traits::HasPolicyId for #name #ty_generics #where_clause {
            type Id = #id_field_type;
            fn id(&self) -> Self::Id {
                self.id
            }
        }
    };

    expanded.into()
}

/// Target type name parsed from `#[policy(wire_type = ...)]` attribute, if present.
fn get_wire_type(attrs: &[Attribute]) -> Result<Option<Ident>> {
    let mut wire_type = None;
    for attr in attrs {
        if attr.path().is_ident("policy") {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("wire_type") {
                    let value = meta.value()?;
                    let ident: Ident = value.parse()?;
                    wire_type = Some(ident);
                    Ok(())
                } else {
                    Err(meta.error("unrecognized policy attribute"))
                }
            })?;
        }
    }
    Ok(wire_type)
}

/// Validator ensuring enum contains only unit variants with explicit discriminants.
fn validate_wire_enum(data_enum: &syn::DataEnum) -> Result<()> {
    for variant in &data_enum.variants {
        if !matches!(variant.fields, Fields::Unit) {
            return Err(syn::Error::new_spanned(
                variant,
                "Only unit enums (without fields) are supported by wire enum derive",
            ));
        }
        if variant.discriminant.is_none() {
            return Err(syn::Error::new_spanned(
                variant,
                "All variants of a wire enum must have explicit discriminants",
            ));
        }
    }
    Ok(())
}

/// Helper to map across all fields of a struct (named or unnamed).
fn map_struct_fields<F>(data: &syn::DataStruct, mut gen_field: F) -> Vec<proc_macro2::TokenStream>
where
    F: FnMut(&syn::Type, &syn::Member) -> proc_macro2::TokenStream,
{
    match &data.fields {
        Fields::Named(fields) => fields
            .named
            .iter()
            .map(|f| {
                let member = syn::Member::Named(f.ident.clone().unwrap());
                gen_field(&f.ty, &member)
            })
            .collect(),
        Fields::Unnamed(fields) => fields
            .unnamed
            .iter()
            .enumerate()
            .map(|(i, f)| {
                let member = syn::Member::Unnamed(syn::Index::from(i));
                gen_field(&f.ty, &member)
            })
            .collect(),
        Fields::Unit => Vec::new(),
    }
}

/// Helper to generate sequential `<FieldType as Trait>::method(&self.field, arg)?;` calls for all struct fields.
fn gen_struct_visitor_calls(
    data: &syn::DataStruct,
    trait_path: &proc_macro2::TokenStream,
    method_name: &Ident,
    arg: &proc_macro2::TokenStream,
) -> Result<proc_macro2::TokenStream> {
    let calls = map_struct_fields(data, |ty, member| {
        quote! {
            <#ty as #trait_path>::#method_name(&self.#member, #arg)?;
        }
    });
    Ok(quote! { #(#calls)* })
}

/// Code generator for parsing struct fields sequentially.
fn gen_struct_parse(data: &syn::DataStruct) -> Result<proc_macro2::TokenStream> {
    match &data.fields {
        Fields::Named(_) => {
            let fields = map_struct_fields(data, |ty, member| {
                quote! {
                    #member: <#ty as crate::new_policy::traits::Parse>::parse(cursor)?,
                }
            });
            Ok(quote! {
                Ok(Self {
                    #(#fields)*
                })
            })
        }
        Fields::Unnamed(_) => {
            let fields = map_struct_fields(data, |ty, _| {
                quote! {
                    <#ty as crate::new_policy::traits::Parse>::parse(cursor)?,
                }
            });
            Ok(quote! {
                Ok(Self (
                    #(#fields)*
                ))
            })
        }
        Fields::Unit => Ok(quote! { Ok(Self) }),
    }
}

/// Code generator for parsing unit enum from its wire representation.
fn gen_enum_parse(
    name: &Ident,
    data_enum: &syn::DataEnum,
    wire_type: &Ident,
) -> Result<proc_macro2::TokenStream> {
    validate_wire_enum(data_enum)?;
    let match_arms = data_enum.variants.iter().map(|variant| {
        let variant_ident = &variant.ident;
        let (_, expr) = variant.discriminant.as_ref().unwrap();
        quote! {
            #expr => Ok(Self::#variant_ident),
        }
    });

    Ok(quote! {
        let value = <#wire_type as crate::new_policy::traits::Parse>::parse(cursor)?;
        match value {
            #(#match_arms)*
            _ => Err(crate::new_policy::error::ParseError::InvalidEnumValue {
                enum_name: stringify!(#name),
                value: value as u64,
            }),
        }
    })
}

/// Code generator for serializing struct fields sequentially.
fn gen_struct_serialize(data: &syn::DataStruct) -> Result<proc_macro2::TokenStream> {
    gen_struct_visitor_calls(
        data,
        &quote!(crate::new_policy::traits::Serialize),
        &syn::parse_quote!(serialize),
        &quote!(writer),
    )
}

/// Code generator for serializing unit enum to its wire representation.
fn gen_enum_serialize(
    data_enum: &syn::DataEnum,
    wire_type: &Ident,
) -> Result<proc_macro2::TokenStream> {
    validate_wire_enum(data_enum)?;
    Ok(quote! {
        let value = *self as #wire_type;
        crate::new_policy::traits::Serialize::serialize(&value, writer)?;
    })
}

/// Code generator for validating struct fields sequentially.
fn gen_struct_validate(data: &syn::DataStruct) -> Result<proc_macro2::TokenStream> {
    gen_struct_visitor_calls(
        data,
        &quote!(crate::new_policy::traits::Validate),
        &syn::parse_quote!(validate),
        &quote!(policy),
    )
}

/// Code generator for validating unit enum (always succeeds trivially).
fn gen_enum_validate(data_enum: &syn::DataEnum) -> Result<proc_macro2::TokenStream> {
    validate_wire_enum(data_enum)?;
    Ok(quote! {})
}
