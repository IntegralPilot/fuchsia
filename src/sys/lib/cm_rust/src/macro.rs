// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use darling::{FromDeriveInput, FromField, FromMeta, FromVariant, ast};
use proc_macro2::TokenStream;
use quote::{quote, quote_spanned};
use syn::{Ident, Type, parse_macro_input};

const PATHS_DELIM: &str = ",";

// This wrapper struct so that we can support parsing syn::Lit into Vec<syn::Path>.
// Rust compiler doesn't allow implementing a trait (FromMeta in this case)
// for a class defined externally, so we have to create a wrapper class
// in order to compile.
#[derive(Default)]
struct PathSet {
    paths: Vec<syn::Path>,
}

impl PathSet {
    pub fn new(paths: Vec<syn::Path>) -> PathSet {
        return PathSet { paths };
    }
}

impl FromMeta for PathSet {
    fn from_value(value: &syn::Lit) -> Result<PathSet, darling::Error> {
        let value = String::from_value(value)?;
        let v = value
            .split(PATHS_DELIM)
            .into_iter()
            .map(|p| syn::Path::from_string(&p))
            .collect::<Result<Vec<syn::Path>, darling::Error>>()?;
        Ok(PathSet::new(v))
    }
}

#[derive(FromVariant, Clone)]
struct EnumVariant {
    ident: Ident,
}

#[derive(FromField, Clone)]
#[darling(attributes(fidl_decl))]
struct StructField {
    ident: Option<Ident>,
    ty: Type,

    // If `#[fidl_decl(default)]` is specified and the field is `None` in FIDL,
    // the field will be set to `Default::default()` in the native type.
    //
    // NOTE: the `#[darling(default)]` means that the `default` field itself
    // should default to `false`.
    #[darling(default)]
    default: bool,

    // Same as `#[fidl_decl(default)]`, except the [NativeIntoFidl] implementation will convert
    // this field to `Option<Foo>` instead of `Some(Foo)`. Use this when you want to convert the
    // default value back to `None`.
    #[darling(default)]
    default_preserve_none: bool,
}

#[derive(Clone, Copy, Debug)]
enum SourcePathOpt {
    None,
    Dictionary,
    NameOnly,
}

impl Default for SourcePathOpt {
    fn default() -> Self {
        Self::None
    }
}

impl FromMeta for SourcePathOpt {
    fn from_value(v: &syn::Lit) -> Result<Self, darling::Error> {
        let v = String::from_value(v)?;
        let v = v.as_str();
        let v = match v {
            "none" => Self::None,
            "dictionary" => Self::Dictionary,
            "name_only" => Self::NameOnly,
            _ => return Err(darling::Error::unknown_value(v)),
        };
        Ok(v)
    }
}

#[derive(FromDeriveInput)]
#[darling(attributes(fidl_decl), supports(enum_newtype, struct_named))]
struct FidlDeclOpts {
    ident: Ident,
    data: ast::Data<EnumVariant, StructField>,
    #[darling(default)]
    fidl_table: Option<PathSet>,
    #[darling(default)]
    fidl_union: Option<PathSet>,
    #[darling(default)]
    source_path: SourcePathOpt,
}

fn fidl_decl_derive_impl(input: syn::DeriveInput) -> TokenStream {
    let opts = match FidlDeclOpts::from_derive_input(&input) {
        Ok(opts) => opts,
        Err(e) => return e.write_errors(),
    };
    let ts = match opts.data {
        ast::Data::Enum(variants) => match (opts.fidl_union, opts.fidl_table) {
            (Some(ps), None) => {
                let mut ts: TokenStream = TokenStream::new();
                for p in ps.paths.into_iter() {
                    let t = generate_enum(
                        opts.ident.clone(),
                        Type::Path(syn::TypePath { qself: None, path: p }),
                        variants.clone(),
                    );
                    ts = quote! {
                        #ts
                        #t
                    }
                }
                ts
            }
            (Some(_), Some(_)) => {
                darling::Error::custom("only one of `fidl_union` or `fidl_table` must be set")
                    .with_span(&input)
                    .write_errors()
            }
            _ => darling::Error::custom("missing `fidl_union` attribute")
                .with_span(&input)
                .write_errors(),
        },
        ast::Data::Struct(fields) => match (opts.fidl_union, opts.fidl_table) {
            (None, Some(ps)) => {
                let mut ts: TokenStream = TokenStream::new();
                for p in ps.paths.into_iter() {
                    let t = generate_struct(
                        opts.ident.clone(),
                        Type::Path(syn::TypePath { qself: None, path: p }),
                        fields.fields.clone(),
                    );
                    ts = quote! {
                        #ts
                        #t
                    };
                }
                ts
            }
            (Some(_), Some(_)) => {
                darling::Error::custom("only one of `fidl_union` or `fidl_table` must be set")
                    .with_span(&input)
                    .write_errors()
            }
            _ => darling::Error::custom("missing `fidl_table` attribute")
                .with_span(&input)
                .write_errors(),
        },
    };
    match opts.source_path {
        SourcePathOpt::Dictionary => {
            let ident = opts.ident;
            let t = quote! {
                impl SourcePath for #ident {
                    fn source_path(&self) -> BorrowedSeparatedPath<'_> {
                        static DOT: LazyLock<RelativePath> = LazyLock::new(|| RelativePath::dot());
                        #[cfg(fuchsia_api_level_at_least = "HEAD")]
                        let dirname = &self.source_dictionary;
                        #[cfg(fuchsia_api_level_less_than = "HEAD")]
                        let dirname = &*DOT;
                        BorrowedSeparatedPath {
                            dirname,
                            basename: &self.source_name,
                        }
                    }
                }
            };
            quote! {
                #ts
                #t
            }
        }
        SourcePathOpt::NameOnly => {
            let ident = opts.ident;
            let t = quote! {
                impl SourcePath for #ident {
                    fn source_path(&self) -> BorrowedSeparatedPath<'_> {
                        static DOT: LazyLock<RelativePath> = LazyLock::new(|| RelativePath::dot());
                        BorrowedSeparatedPath {
                            dirname: &*DOT,
                            basename: &self.source_name,
                        }
                    }
                }
            };
            quote! {
                #ts
                #t
            }
        }
        SourcePathOpt::None => ts,
    }
}

fn generate_enum(
    enum_ident: Ident,
    fidl_type: syn::Type,
    variants: Vec<EnumVariant>,
) -> TokenStream {
    let fidl_into_native_lines = variants
        .iter()
        .map(|v| {
            let ident = &v.ident;
            quote_spanned! {ident.span()=>
                Self::#ident(inner) => #enum_ident::#ident(inner.fidl_into_native()),
            }
        })
        .collect::<Vec<_>>();
    let native_into_fidl_lines = variants
        .iter()
        .map(|v| {
            let ident = &v.ident;
            quote_spanned! {ident.span()=>
                Self::#ident(inner) => #fidl_type::#ident(inner.native_into_fidl()),
            }
        })
        .collect::<Vec<_>>();
    quote! {
        impl FidlIntoNative<#enum_ident> for #fidl_type {
            fn fidl_into_native(self) -> #enum_ident {
                match self {
                    #(#fidl_into_native_lines)*
                    _ => panic!("unknown FIDL variant"),
                }
            }
        }

        impl NativeIntoFidl<#fidl_type> for #enum_ident {
            fn native_into_fidl(self) -> #fidl_type {
                match self {
                    #(#native_into_fidl_lines)*
                }
            }
        }
    }
}

enum WrapperType {
    /// The type is not wrapped by anything.
    Raw,
    /// The type is Option<T>.
    Option,
    /// The type is Vec<T>.
    Vec,
    /// The type is Box<[T]>.
    BoxArray,
}

/// Finds the wrapper type for the given type. This is used to find out whether the
/// type is an Option<T>, Vec<T>, Box<[T]>, or just T.
/// This is NOT fool-proof. If the type appears as std::vec::Vec<T> for instance, it
/// won't be recognized. That is one of the reasons this macro is not exported beyond
/// `cm_rust`.
fn wrapper_type(ty: &Type) -> WrapperType {
    if let Type::Path(pt) = ty {
        if pt.qself.is_none() && pt.path.leading_colon.is_none() && pt.path.segments.len() == 1 {
            let segment = &pt.path.segments[0];
            if let syn::PathArguments::AngleBracketed(params) = &segment.arguments {
                if params.args.len() == 1 {
                    if let syn::GenericArgument::Type(_) = &params.args[0] {
                        if segment.ident == "Option" {
                            return WrapperType::Option;
                        } else if segment.ident == "Vec" {
                            return WrapperType::Vec;
                        } else if segment.ident == "Box" {
                            return WrapperType::BoxArray;
                        }
                    }
                }
            }
        }
    }
    WrapperType::Raw
}

fn generate_struct(struct_ident: Ident, fidl_type: Type, fields: Vec<StructField>) -> TokenStream {
    let mut fidl_into_native_lines = Vec::new();
    let mut native_into_fidl_lines = Vec::new();
    enum DefaultOption {
        Off,
        Default,
        DefaultPreserveNone,
    }
    for field in fields {
        let default = match (field.default, field.default_preserve_none) {
            (true, true) => {
                panic!(
                    "#[fidl_decl(default)] and #[fidl_decl(default_preserve_none)] \
                    can't both be set"
                )
            }
            (true, false) => DefaultOption::Default,
            (false, true) => DefaultOption::DefaultPreserveNone,
            (false, false) => DefaultOption::Off,
        };
        let field_ident = field.ident.unwrap();
        match (wrapper_type(&field.ty), default) {
            (WrapperType::Raw, DefaultOption::Off) => {
                fidl_into_native_lines.push(quote_spanned! {field_ident.span()=>
                    #field_ident: self.#field_ident.unwrap().fidl_into_native()
                });
                native_into_fidl_lines.push(quote_spanned! {field_ident.span()=>
                    #field_ident: Some(self.#field_ident.native_into_fidl())
                });
            }
            (WrapperType::Raw, DefaultOption::Default) => {
                fidl_into_native_lines.push(quote_spanned! {field_ident.span()=>
                    #field_ident: self.#field_ident.map(FidlIntoNative::fidl_into_native).unwrap_or_default()
                });
                native_into_fidl_lines.push(quote_spanned! {field_ident.span()=>
                    #field_ident: Some(self.#field_ident.native_into_fidl())
                });
            }
            (WrapperType::Raw, DefaultOption::DefaultPreserveNone) => {
                fidl_into_native_lines.push(quote_spanned! {field_ident.span()=>
                    #field_ident: self.#field_ident.map(FidlIntoNative::fidl_into_native).unwrap_or_default()
                });
                native_into_fidl_lines.push(quote_spanned! {field_ident.span()=>
                    #field_ident: self.#field_ident.native_into_fidl()
                });
            }
            (WrapperType::Option, DefaultOption::Off) => {
                fidl_into_native_lines.push(quote_spanned! {field_ident.span()=>
                    #field_ident: self.#field_ident.map(FidlIntoNative::fidl_into_native)
                });
                native_into_fidl_lines.push(quote_spanned! {field_ident.span()=>
                    #field_ident: self.#field_ident.map(NativeIntoFidl::native_into_fidl)
                });
            }
            (WrapperType::Option, _) => {
                panic!("fidl_decl(default) attribute cannot be used with Option types")
            }
            (WrapperType::Vec, DefaultOption::Off) => {
                fidl_into_native_lines.push(quote_spanned! {field_ident.span()=>
                    #field_ident: self
                        .#field_ident
                        .into_iter()
                        .map(std::iter::IntoIterator::into_iter)
                        .flatten()
                        .map(FidlIntoNative::fidl_into_native).
                        collect()
                });
                native_into_fidl_lines.push(quote_spanned! {field_ident.span()=>
                    #field_ident: if self.#field_ident.is_empty() {
                        None
                    } else {
                        Some(self.#field_ident.into_iter().map(NativeIntoFidl::native_into_fidl).collect())
                    }
                });
            }
            (WrapperType::Vec, _) => {
                panic!("fidl_decl(default) attribute cannot be used with Vec types")
            }
            (WrapperType::BoxArray, DefaultOption::Off) => {
                fidl_into_native_lines.push(quote_spanned! {field_ident.span()=>
                    #field_ident: self
                        .#field_ident
                        .into_iter()
                        .map(std::iter::IntoIterator::into_iter)
                        .flatten()
                        .map(FidlIntoNative::fidl_into_native).
                        collect()
                });
                native_into_fidl_lines.push(quote_spanned! {field_ident.span()=>
                    #field_ident: if self.#field_ident.is_empty() {
                        None
                    } else {
                        Some(std::iter::IntoIterator::into_iter(self.#field_ident).map(NativeIntoFidl::native_into_fidl).collect())
                    }
                });
            }
            (WrapperType::BoxArray, _) => {
                panic!("fidl_decl(default) attribute cannot be used with Box<[T]> types")
            }
        }
    }
    quote! {
        impl FidlIntoNative< #struct_ident > for #fidl_type {
            fn fidl_into_native(self) -> #struct_ident {
                #struct_ident {
                    #( #fidl_into_native_lines, )*
                }
            }
        }

        impl NativeIntoFidl< #fidl_type > for #struct_ident {
            fn native_into_fidl(self) -> #fidl_type {
                #fidl_type {
                    #( #native_into_fidl_lines, )*
                    ..<#fidl_type>::default()
                }
            }
        }
    }
}

/// A derive-macro that generates implementations of `NativeIntoFidl` and `FidlIntoNative`.
///
/// The macro supports the following top-level attributes:
///
/// - `#[fidl_decl(fidl_table = "path::to::fidl::Table")]`: Generates implementations to convert
///   between this struct and a FIDL table of the given name.
/// - `#[fidl_decl(fidl_union = "path::to::fidl::Union")]`: Generates implementations to convert
///   between this enum and a FIDL union of the given name.
///
/// The names of the fields/variants are mapped to those of the FIDL table/union, so they must be
/// the same.
///
/// A field/variant can be omitted, in which case the conversion impl panics when the FIDL
/// table/union has that field/variant present.
#[proc_macro_derive(FidlDecl, attributes(fidl_decl))]
pub fn fidl_decl_derive(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    fidl_decl_derive_impl(parse_macro_input!(input)).into()
}

#[derive(FromDeriveInput)]
#[darling(supports(struct_named))]
struct DeclCommonOpts {
    ident: Ident,
}

fn use_decl_common_derive_impl(input: syn::DeriveInput) -> TokenStream {
    let struct_ident = match DeclCommonOpts::from_derive_input(&input) {
        Ok(opts) => opts.ident,
        Err(e) => return e.write_errors(),
    };

    quote! {
        impl SourceName for #struct_ident {
            fn source_name(&self) -> &Name {
                &self.source_name
            }
        }

        impl UseDeclCommon for #struct_ident {
            fn source(&self) -> &UseSource {
                &self.source
            }
            fn availability(&self) -> &Availability {
                &self.availability
            }
        }
    }
}

/// A derive-macro that generates an implementation of `UseDeclCommon`. Use this for
/// the inner structs of each variant of `UseDecl`.
#[proc_macro_derive(UseDeclCommon)]
pub fn use_decl_common_derive(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    use_decl_common_derive_impl(parse_macro_input!(input)).into()
}

fn offer_decl_common_derive_impl(input: syn::DeriveInput) -> TokenStream {
    let struct_ident = match DeclCommonOpts::from_derive_input(&input) {
        Ok(opts) => opts.ident,
        Err(e) => return e.write_errors(),
    };

    quote! {
        impl SourceName for #struct_ident {
            fn source_name(&self) -> &Name {
                &self.source_name
            }
        }

        impl OfferDeclCommon for #struct_ident {
            fn target_name(&self) -> &Name {
                &self.target_name
            }

            fn source(&self) -> &OfferSource {
                &self.source
            }

            fn target(&self) -> &OfferTarget {
                &self.target
            }

            fn availability(&self) -> &Availability {
                &self.availability
            }
        }
    }
}

/// A derive-macro that generates an implementation of `OfferDeclCommon`. Use this for
/// the inner structs of each variant of `OfferDecl`.
#[proc_macro_derive(OfferDeclCommon)]
pub fn offer_decl_common_derive(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    offer_decl_common_derive_impl(parse_macro_input!(input)).into()
}

fn offer_decl_common_derive_availability_required_impl(input: syn::DeriveInput) -> TokenStream {
    let struct_ident = match DeclCommonOpts::from_derive_input(&input) {
        Ok(opts) => opts.ident,
        Err(e) => return e.write_errors(),
    };

    quote! {
        impl SourceName for #struct_ident {
            fn source_name(&self) -> &Name {
                &self.source_name
            }
        }

        impl OfferDeclCommon for #struct_ident {
            fn target_name(&self) -> &Name {
                &self.target_name
            }

            fn source(&self) -> &OfferSource {
                &self.source
            }

            fn target(&self) -> &OfferTarget {
                &self.target
            }

            fn availability(&self) -> &Availability {
                &Availability::Required
            }
        }
    }
}

/// A derive-macro that generates an implementation of `OfferDeclCommon`. Use this for
/// the inner structs of each variant of `OfferDecl` that do not have an `availability` field.
#[proc_macro_derive(OfferDeclCommonNoAvailability)]
pub fn offer_decl_common_derive_availability_required(
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    offer_decl_common_derive_availability_required_impl(parse_macro_input!(input)).into()
}

fn expose_decl_common_derive_impl(input: syn::DeriveInput) -> TokenStream {
    let struct_ident = match DeclCommonOpts::from_derive_input(&input) {
        Ok(opts) => opts.ident,
        Err(e) => return e.write_errors(),
    };

    quote! {
        impl SourceName for #struct_ident {
            fn source_name(&self) -> &Name {
                &self.source_name
            }
        }

        impl ExposeDeclCommon for #struct_ident {
            fn target_name(&self) -> &Name {
                &self.target_name
            }

            fn source(&self) -> &ExposeSource {
                &self.source
            }

            fn target(&self) -> &ExposeTarget {
                &self.target
            }

            fn availability(&self) -> &Availability {
                &self.availability
            }
        }
    }
}

/// A derive-macro that generates an implementation of `ExposeDeclCommon`. Use this
/// for the inner structs of each variant of `ExposeDecl` that supports specifying availability.
#[proc_macro_derive(ExposeDeclCommon)]
pub fn expose_decl_common_derive(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    expose_decl_common_derive_impl(parse_macro_input!(input)).into()
}

fn expose_decl_common_availability_always_required_derive_impl(
    input: syn::DeriveInput,
) -> TokenStream {
    let struct_ident = match DeclCommonOpts::from_derive_input(&input) {
        Ok(opts) => opts.ident,
        Err(e) => return e.write_errors(),
    };

    quote! {
        impl SourceName for #struct_ident {
            fn source_name(&self) -> &Name {
                &self.source_name
            }
        }

        impl ExposeDeclCommon for #struct_ident {
            fn target_name(&self) -> &Name {
                &self.target_name
            }

            fn source(&self) -> &ExposeSource {
                &self.source
            }

            fn target(&self) -> &ExposeTarget {
                &self.target
            }

            fn availability(&self) -> &Availability {
                &Availability::Required
            }
        }
    }
}

/// A derive-macro that generates an implementation of `ExposeDeclCommon`. Use this
/// for inner structs of each variant of `ExposeDecl` whose availability is always required.
#[proc_macro_derive(ExposeDeclCommonAlwaysRequired)]
pub fn expose_decl_common_with_availability_derive(
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    expose_decl_common_availability_always_required_derive_impl(parse_macro_input!(input)).into()
}
