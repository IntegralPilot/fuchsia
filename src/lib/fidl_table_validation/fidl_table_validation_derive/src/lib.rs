// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![recursion_limit = "128"]

use heck::ToUpperCamelCase;
use proc_macro2::{Span, TokenStream};
use quote::quote;

use syn::punctuated::*;
use syn::spanned::Spanned;
use syn::token::*;
use syn::*;

type Result<T> = std::result::Result<T, Error>;

const INVALID_FIDL_FIELD_ATTRIBUTE_MSG: &str = concat!(
    "fidl_field_type attribute must be required, optional, ",
    "or default = value; alternatively use fidl_field_with_default for non-literal constants"
);

#[proc_macro_derive(
    ValidFidlTable,
    attributes(
        fidl_table_src,
        fidl_field_type,
        fidl_table_validator,
        fidl_field_with_default,
        fidl_table_strict
    )
)]
pub fn validate_fidl_table(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let input: DeriveInput = syn::parse(input).unwrap();
    match input.data {
        Data::Struct(DataStruct { fields: syn::Fields::Named(fields), .. }) => {
            match impl_valid_fidl_table(&input.ident, &input.generics, fields.named, &input.attrs) {
                Ok(v) => v.into(),
                Err(e) => e.to_compile_error().into(),
            }
        }
        _ => Error::new(input.span(), "ValidateFidlTable only supports non-tuple structs!")
            .to_compile_error()
            .into(),
    }
}

fn unique_attr<'a>(attrs: &'a [Attribute], call: &str) -> Result<Option<&'a Attribute>> {
    let mut attrs = attrs.iter().filter(|attr| attr.path().is_ident(call));
    let attr = match attrs.next() {
        Some(attr) => attr,
        None => return Ok(None),
    };
    if let Some(attr) = attrs.next() {
        return Err(Error::new(
            attr.span(),
            &format!("The {} attribute should only be declared once.", call),
        ));
    }
    Ok(Some(attr))
}

fn unique_list_with_arg(attrs: &[Attribute], call: &str) -> Result<Option<Meta>> {
    let Some(attr) = unique_attr(attrs, call)? else { return Ok(None) };

    let mut nested = attr.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)?;
    if nested.len() > 1 {
        return Err(Error::new(
            attr.span(),
            &format!("The {} attribute only expects one argument", call),
        ));
    }

    return Ok(nested.pop().map(|pair| pair.into_value()));
}

fn fidl_table_strict(attrs: &[Attribute]) -> Result<Option<Vec<Ident>>> {
    let Some(attr) = unique_attr(attrs, "fidl_table_strict")? else {
        return Ok(None);
    };
    let list = match &attr.meta {
        Meta::Path(_) => return Ok(Some(vec![])),
        Meta::NameValue(_) => {
            return Err(Error::new(
                attr.span(),
                &format!(
                    "fidl_table_strict expects either no arguments or a list of \
                    fields from the FIDL table that are omitted from the validated type"
                ),
            ));
        }
        Meta::List(list) => list,
    };
    let ignored = list.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)?;
    let idents = ignored
        .into_iter()
        .map(|meta| {
            match &meta {
                Meta::Path(p) => p.get_ident().cloned(),
                _ => None,
            }
            .ok_or_else(move || {
                Error::new(meta.span(), "fidl_table_strict expects list of identifiers")
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Some(idents))
}

fn fidl_table_path(span: Span, attrs: &[Attribute]) -> Result<Path> {
    match unique_list_with_arg(attrs, "fidl_table_src")? {
        Some(meta) => match meta {
            Meta::Path(fidl_table_path) => Ok(fidl_table_path),
            _ => Err(Error::new(
                span,
                concat!(
                    "The #[fidl_table_src(FidlTableType)] attribute ",
                    "takes only one argument, a type name."
                ),
            )),
        },
        _ => Err(Error::new(
            span,
            concat!(
                "To derive ValidFidlTable, struct needs ",
                "#[fidl_table_src(FidlTableType)] attribute to mark ",
                "source Fidl type."
            ),
        )),
    }
}

fn fidl_table_validator(span: Span, attrs: &[Attribute]) -> Result<Option<Ident>> {
    unique_list_with_arg(attrs, "fidl_table_validator")?
        .map(|meta| match meta {
            Meta::Path(fidl_table_validator) => fidl_table_validator
                .get_ident()
                .cloned()
                .ok_or_else(|| Error::new(fidl_table_validator.span(), "Invalid Identifier")),
            _ => Err(Error::new(
                span,
                concat!(
                    "The #[fidl_table(FidlTableType)] attribute takes ",
                    "only one argument, a type name."
                ),
            )),
        })
        .transpose()
}

#[derive(Clone, Debug)]
struct FidlField {
    ident: Ident,
    #[allow(unused)]
    in_vec: bool,
    kind: FidlFieldKind,
}

impl TryFrom<Field> for FidlField {
    type Error = Error;
    fn try_from(src: Field) -> Result<Self> {
        let span = src.span();
        let ident = match src.ident {
            Some(ident) => Ok(ident),
            None => {
                Err(Error::new(span, "ValidFidlTable can only be derived for non-tuple structs."))
            }
        }?;

        let kind = FidlFieldKind::try_from((span, src.attrs.as_slice()))?;

        let in_vec = match src.ty {
            syn::Type::Path(path) => {
                let first_segment = path.path.segments.iter().next();
                let second_segment = match first_segment {
                    Some(PathSegment {
                        arguments:
                            syn::PathArguments::AngleBracketed(AngleBracketedGenericArguments {
                                args,
                                ..
                            }),
                        ..
                    }) => match args.first() {
                        Some(GenericArgument::Type(syn::Type::Path(path))) => {
                            path.path.segments.iter().next()
                        }
                        _ => None,
                    },
                    _ => None,
                };

                let extract_name = |segment: &PathSegment| format!("{}", segment.ident);
                let first_segment = first_segment.map(&extract_name);
                let second_segment = second_segment.map(&extract_name);

                match (kind.clone(), first_segment, second_segment) {
                    (FidlFieldKind::Required, Some(segment), _) if segment == "Vec" => true,
                    (FidlFieldKind::Optional, _, Some(segment)) if segment == "Vec" => true,
                    _ => false,
                }
            }
            _ => false,
        };

        Ok(FidlField { ident, in_vec, kind })
    }
}

impl FidlField {
    fn camel_case(&self) -> Ident {
        let name = self.ident.to_string().to_upper_camel_case();
        Ident::new(&name, Span::call_site())
    }

    fn src_ident(&self) -> Ident {
        Ident::new(&format!("src_{}", self.ident), Span::call_site())
    }

    fn generate_try_from(&self, missing_field_error_type: &Ident) -> TokenStream {
        let ident = &self.ident;
        let src_ident = self.src_ident();
        match &self.kind {
            FidlFieldKind::Required => {
                let camel_case = self.camel_case();
                match self.in_vec {
                    true => quote!(
                        #ident: {
                            let src_vec = #src_ident.ok_or(#missing_field_error_type::#camel_case)?;
                            src_vec
                                .into_iter()
                                .map(std::convert::TryFrom::try_from)
                                .map(|r| r.map_err(::fidl_table_validation::anyhow::Error::from))
                                .collect::<std::result::Result<_, ::fidl_table_validation::anyhow::Error>>()?
                        },
                    ),
                    false => quote!(
                        #ident: std::convert::TryFrom::try_from(
                            #src_ident.ok_or(#missing_field_error_type::#camel_case)?
                        ).map_err(::fidl_table_validation::anyhow::Error::from)?,
                    ),
                }
            }
            FidlFieldKind::Optional => match self.in_vec {
                true => quote!(
                    #ident: if let Some(src_vec) = #src_ident {
                        Some(
                            src_vec
                                .into_iter()
                                .map(std::convert::TryFrom::try_from)
                                .map(|r| r.map_err(::fidl_table_validation::anyhow::Error::from))
                                .collect::<std::result::Result<_, ::fidl_table_validation::anyhow::Error>>()?
                        )
                    } else {
                        None
                    },
                ),
                false => quote!(
                    #ident: if let Some(field) = #src_ident {
                        Some(
                            std::convert::TryFrom::try_from(
                                field
                            ).map_err(::fidl_table_validation::anyhow::Error::from)?
                        )
                    } else {
                        None
                    },
                ),
            },
            FidlFieldKind::Default => quote!(
                #ident: #src_ident
                    .map(std::convert::TryFrom::try_from)
                    .transpose()
                    .map_err(::fidl_table_validation::anyhow::Error::from)?
                    .unwrap_or_default(),
            ),
            FidlFieldKind::ExprDefault(default_ident) => quote!(
                #ident: #src_ident
                    .map(std::convert::TryFrom::try_from)
                    .transpose()
                    .map_err(::fidl_table_validation::anyhow::Error::from)?
                    .unwrap_or(#default_ident),
            ),
            FidlFieldKind::HasDefault(value) => quote!(
                #ident: #src_ident
                    .map(std::convert::TryFrom::try_from)
                    .transpose()
                    .map_err(::fidl_table_validation::anyhow::Error::from)?
                    .unwrap_or(#value),
            ),
            FidlFieldKind::Converter(path) => {
                let camel_case = self.camel_case();
                quote!(
                    #ident: <#path as ::fidl_table_validation::Converter>::try_from_fidl(
                        #src_ident.ok_or(#missing_field_error_type::#camel_case)?
                    )
                    .map_err(::fidl_table_validation::anyhow::Error::from)?,
                )
            }
            FidlFieldKind::OptionalConverter(path) => {
                quote!(
                    #ident: <#path as ::fidl_table_validation::Converter>::try_from_fidl(
                        #src_ident
                    )
                    .map_err(::fidl_table_validation::anyhow::Error::from)?,
                )
            }
        }
    }
}

#[derive(Clone, Debug)]
enum FidlFieldKind {
    Required,
    Optional,
    Default,
    ExprDefault(Ident),
    HasDefault(Lit),
    Converter(Path),
    OptionalConverter(Path),
}

impl TryFrom<(Span, &[Attribute])> for FidlFieldKind {
    type Error = Error;
    fn try_from((span, attrs): (Span, &[Attribute])) -> Result<Self> {
        if let Some(kind) = match unique_list_with_arg(attrs, "fidl_field_type")? {
            Some(Meta::Path(field_type)) => {
                field_type.get_ident().and_then(|i| match i.to_string().as_str() {
                    "required" => Some(FidlFieldKind::Required),
                    "optional" => Some(FidlFieldKind::Optional),
                    "default" => Some(FidlFieldKind::Default),
                    _ => None,
                })
            }
            Some(Meta::NameValue(ref default_value)) if default_value.path.is_ident("default") => {
                if let Expr::Lit(ExprLit { ref lit, .. }) = default_value.value {
                    Some(FidlFieldKind::HasDefault(lit.clone()))
                } else {
                    return Err(Error::new(default_value.span(), "expected literal value"));
                }
            }
            Some(Meta::NameValue(ref converter)) if converter.path.is_ident("converter") => {
                if let Expr::Path(ExprPath { ref path, .. }) = converter.value {
                    Some(FidlFieldKind::Converter(path.clone()))
                } else {
                    return Err(Error::new(converter.span(), "expected path"));
                }
            }
            Some(Meta::NameValue(ref converter))
                if converter.path.is_ident("optional_converter") =>
            {
                if let Expr::Path(ExprPath { ref path, .. }) = converter.value {
                    Some(FidlFieldKind::OptionalConverter(path.clone()))
                } else {
                    return Err(Error::new(converter.span(), "expected path"));
                }
            }
            _ => None,
        } {
            return Ok(kind);
        }

        let error = Err(Error::new(span, INVALID_FIDL_FIELD_ATTRIBUTE_MSG));
        match unique_list_with_arg(attrs, "fidl_field_with_default")? {
            Some(Meta::Path(field_type)) => match field_type.get_ident() {
                Some(ident) => Ok(FidlFieldKind::ExprDefault(ident.clone())),
                _ => error,
            },
            _ => Ok(FidlFieldKind::Required),
        }
    }
}

fn impl_valid_fidl_table(
    name: &Ident,
    generics: &Generics,
    fields: Punctuated<Field, Comma>,
    attrs: &[Attribute],
) -> Result<TokenStream> {
    let fidl_table_path = fidl_table_path(name.span(), attrs)?;
    let fidl_table_strict = fidl_table_strict(attrs)?;
    let fidl_table_type = match fidl_table_path.segments.last() {
        Some(segment) => segment.ident.clone(),
        None => {
            return Err(Error::new(
                name.span(),
                concat!(
                    "The #[fidl_table_src(FidlTableType)] attribute ",
                    "takes only one argument, a type name."
                ),
            ));
        }
    };

    let missing_field_error_type = {
        let mut error_type_name = fidl_table_type.to_string();
        error_type_name.push_str("MissingFieldError");
        Ident::new(&error_type_name, Span::call_site())
    };

    let error_type_name = {
        let mut error_type_name = fidl_table_type.to_string();
        error_type_name.push_str("ValidationError");
        Ident::new(&error_type_name, Span::call_site())
    };

    let custom_validator = fidl_table_validator(name.span(), attrs)?;
    let custom_validator_error = custom_validator.as_ref().map(|validator| {
        quote!(
            /// Custom validator error.
            Logical(<#validator as Validate<#name>>::Error),
        )
    });
    let custom_validator_call =
        custom_validator.as_ref().map(|validator| quote!(#validator::validate(&maybe_valid)?;));
    let custom_validator_error_from_impl = custom_validator.map(|validator| {
        quote!(
            impl From<<#validator as Validate<#name>>::Error> for #error_type_name {
                fn from(src: <#validator as Validate<#name>>::Error) -> Self {
                    #error_type_name::Logical(src)
                }
            }
        )
    });

    let fields: Vec<FidlField> = fields
        .into_pairs()
        .map(Pair::into_value)
        .map(FidlField::try_from)
        .collect::<Result<Vec<FidlField>>>()?;

    let trailing_destructure = fidl_table_strict
        .as_ref()
        .map(|ignored| {
            itertools::Either::Left(
                ignored
                    .iter()
                    .map(|ident| quote!(#ident: _,))
                    .chain(std::iter::once(quote!(__source_breaking: _,))),
            )
        })
        .unwrap_or_else(|| itertools::Either::Right(std::iter::once(quote!(..))));

    let fidl_table_destructure = fields
        .iter()
        .map(|field| {
            let ident = &field.ident;
            let src_ident = field.src_ident();
            quote!(#ident: #src_ident,)
        })
        .chain(trailing_destructure)
        .collect::<TokenStream>();
    let fidl_table_construct_trailing = fidl_table_strict
        .as_ref()
        .map(|ignored| {
            ignored
                .iter()
                .map(|ident| quote!(#ident: Default::default(),))
                .chain(std::iter::once(quote!(__source_breaking: Default::default(),)))
                .collect::<TokenStream>()
        })
        .unwrap_or_else(|| quote!(..Default::default()));

    let mut field_validations = TokenStream::new();
    field_validations.extend(fields.iter().map(|f| f.generate_try_from(&missing_field_error_type)));

    let mut field_intos = TokenStream::new();
    field_intos.extend(fields.iter().map(|field| {
        let ident = &field.ident;
        match &field.kind {
            FidlFieldKind::Optional => match field.in_vec {
                true => quote!(
                    #ident: if let Some(field) = src.#ident {
                        Some(field.into_iter().map(Into::into).collect())
                    } else {
                        None
                    },
                ),
                false => quote!(
                    #ident: if let Some(field) = src.#ident {
                        Some(field.into())
                    } else {
                        None
                    },
                ),
            },
            FidlFieldKind::Converter(path) => {
                quote!(#ident: Some(
                    <#path as ::fidl_table_validation::Converter>::from_validated(src.#ident)
                ),)
            }
            FidlFieldKind::OptionalConverter(path) => {
                quote!(#ident:
                    <#path as ::fidl_table_validation::Converter>::from_validated(src.#ident),
                )
            }
            _ => match field.in_vec {
                true => quote!(
                    #ident: Some(
                        src.#ident.into_iter().map(Into::into).collect()
                    ),
                ),
                false => quote!(
                    #ident: Some(
                        src.#ident.into()
                    ),
                ),
            },
        }
    }));

    let mut field_errors = TokenStream::new();
    field_errors.extend(
        fields
            .iter()
            .filter(|field| match field.kind {
                FidlFieldKind::Required | FidlFieldKind::Converter(_) => true,
                _ => false,
            })
            .map(|field| {
                let doc = format!("`{}` is missing.", field.ident);
                let camel_case = FidlField::camel_case(field);
                quote!(
                    #[doc = #doc]
                    #camel_case,
                )
            }),
    );

    let missing_error_doc = format!("Missing fields in `{}`.", fidl_table_type);
    let error_doc = format!("Errors validating `{}`.", fidl_table_type);
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();
    Ok(quote!(
        #[doc = #missing_error_doc]
        #[derive(Debug, Clone, Copy, PartialEq)]
        pub enum #missing_field_error_type {
            #field_errors
        }

        #[doc = #error_doc]
        #[derive(Debug)]
        pub enum #error_type_name {
            /// Missing Field.
            MissingField(#missing_field_error_type),
            /// Invalid Field.
            InvalidField(::fidl_table_validation::anyhow::Error),
            #custom_validator_error
        }

        impl std::fmt::Display for #error_type_name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "Validation error: {:?}", self)
            }
        }

        impl std::error::Error for #error_type_name {}

        impl From<::fidl_table_validation::anyhow::Error> for #error_type_name {
            fn from(src: ::fidl_table_validation::anyhow::Error) -> Self {
                #error_type_name::InvalidField(src)
            }
        }

        impl From<#missing_field_error_type> for #error_type_name {
            fn from(src: #missing_field_error_type) -> Self {
                #error_type_name::MissingField(src)
            }
        }

        #custom_validator_error_from_impl

        impl #impl_generics std::convert::TryFrom<#fidl_table_path> for #name #ty_generics #where_clause {
            type Error = #error_type_name;
            fn try_from(src: #fidl_table_path) -> std::result::Result<Self, Self::Error> {
                use ::fidl_table_validation::Validate;
                let #fidl_table_path { #fidl_table_destructure } = src;
                let maybe_valid = Self {
                    #field_validations
                };
                #custom_validator_call
                Ok(maybe_valid)
            }
        }

        impl #impl_generics std::convert::From<#name #ty_generics> for #fidl_table_path #where_clause {
            fn from(src: #name #ty_generics) -> #fidl_table_path {
                Self {
                    #field_intos
                    #fidl_table_construct_trailing
                }
            }
        }
    ))
}
