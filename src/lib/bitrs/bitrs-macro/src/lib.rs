// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::collections::{HashMap, HashSet};

use proc_macro::TokenStream;
use proc_macro2::{Delimiter, Group, Literal, Span, TokenStream as TokenStream2};
use quote::{ToTokens, format_ident, quote};
use syn::parse::{Error, Parse, ParseStream, Result};
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::{
    Attribute, Expr, ExprLit, ExprRange, Fields, Ident, ItemStruct, Lit, Meta, Pat, PatIdent,
    RangeLimits, Stmt, Token, Type, braced, bracketed, parenthesized, parse_macro_input, token,
};

#[proc_macro_attribute]
pub fn bitfield_repr(attr: TokenStream, item: TokenStream) -> TokenStream {
    let attr: TokenStream2 = attr.into();
    let item: TokenStream2 = item.into();

    // TODO(https://fxbug.dev/525077555): Add
    // ```
    // #[zerocopy(crate = "::bitrs::__zerocopy")
    // ```
    // once https://github.com/google/zerocopy/pull/3544 lands in-tree. This
    // changes enables the derive macros themselves to use the re-exported
    // version of zerocopy in their expansions, allowing dependent crates not
    // have to explicitlty depend on zerocopy.
    quote! {
        #[repr(#attr)]
        #[derive(
            Debug,
            Eq,
            PartialEq,
            ::bitrs::__zerocopy::Immutable,
            ::bitrs::__zerocopy::IntoBytes,
            ::bitrs::__zerocopy::TryFromBytes,
        )]
        #item
    }
    .into()
}

#[proc_macro]
pub fn layout(item: TokenStream) -> TokenStream {
    parse_macro_input!(item as Layout).to_token_stream().into()
}

#[proc_macro]
pub fn multilayout(item: TokenStream) -> TokenStream {
    parse_macro_input!(item as Multilayout).to_token_stream().into()
}

//
// Parsing of the bitrs type.
//

enum BaseType {
    U8,
    U16,
    U32,
    U64,
    U128,
}

impl BaseType {
    const fn high_bit(&self) -> usize {
        match *self {
            Self::U8 => (u8::BITS - 1) as usize,
            Self::U16 => (u16::BITS - 1) as usize,
            Self::U32 => (u32::BITS - 1) as usize,
            Self::U64 => (u64::BITS - 1) as usize,
            Self::U128 => (u128::BITS - 1) as usize,
        }
    }
}

struct BaseTypeDef {
    def: Type,
    ty: BaseType,
}

impl TryFrom<Type> for BaseTypeDef {
    type Error = Error;

    fn try_from(type_def: Type) -> Result<Self> {
        const INVALID_BASE_TYPE: &str = "base type must be an unsigned integral type";
        let Type::Path(ref path_ty) = type_def else {
            return Err(Error::new_spanned(type_def, INVALID_BASE_TYPE));
        };
        let path = &path_ty.path;
        let ty = if path.is_ident("u8") {
            BaseType::U8
        } else if path.is_ident("u16") {
            BaseType::U16
        } else if path.is_ident("u32") {
            BaseType::U32
        } else if path.is_ident("u64") {
            BaseType::U64
        } else if path.is_ident("u128") {
            BaseType::U128
        } else {
            return Err(Error::new_spanned(path, INVALID_BASE_TYPE));
        };
        Ok(Self { def: type_def, ty })
    }
}

struct TypeDef {
    def: ItemStruct,
    base: BaseTypeDef,
    tags: HashSet<Ident>,
}

impl Parse for TypeDef {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let mut strct: ItemStruct = input.parse()?;

        // Pull out any `#[bitrs(tag, ...)]` attribute. It's macro-internal: it
        // declares the tag set the struct participates in for `multilayout!`'s
        // contribution-predicate evaluation, and is stripped before the struct
        // is forwarded (since `bitrs` is not a real Rust attribute).
        let mut tags: HashSet<Ident> = HashSet::new();
        for attr in &strct.attrs {
            if !attr.path().is_ident("bitrs") {
                continue;
            }
            let Meta::List(list) = &attr.meta else {
                return Err(Error::new_spanned(
                    attr,
                    "`#[bitrs(...)]` takes a parenthesized list of tag identifiers",
                ));
            };
            let parsed: Punctuated<Ident, Token![,]> =
                list.parse_args_with(Punctuated::parse_terminated)?;
            if parsed.is_empty() {
                return Err(Error::new_spanned(
                    list,
                    "`#[bitrs(...)]` requires at least one tag identifier",
                ));
            }
            for tag in parsed {
                if tag == "all" || tag == "any" || tag == "not" {
                    return Err(Error::new_spanned(
                        &tag,
                        format!("`{tag}` is reserved and cannot be used as a tag name"),
                    ));
                }
                if !tags.insert(tag.clone()) {
                    return Err(Error::new_spanned(&tag, format!("duplicate tag `{tag}`")));
                }
            }
        }
        strct.attrs.retain(|a| !a.path().is_ident("bitrs"));

        // Check for any redundant derives; all other derives are forwarded.
        for attr in &strct.attrs {
            if attr.path().is_ident("derive") {
                attr.parse_nested_meta(|meta| {
                    for t in &["Copy", "Clone", "Debug", "Default", "Eq", "PartialEq"] {
                        if meta.path.is_ident(t) {
                            return Err(Error::new_spanned(
                                meta.path,
                                format!("layout! already derives {t}"),
                            ));
                        }
                    }
                    Ok(())
                })?;
                continue;
            }
        }

        let base_type = if let Fields::Unnamed(fields) = &strct.fields {
            if fields.unnamed.is_empty() {
                return Err(Error::new_spanned(&fields.unnamed, "no base type provided"));
            }
            if fields.unnamed.len() > 1 {
                return Err(Error::new_spanned(
                    &fields.unnamed,
                    "too many tuple fields; only the base type should be provided",
                ));
            }
            BaseTypeDef::try_from(fields.unnamed.first().unwrap().ty.clone())?
        } else {
            return Err(Error::new_spanned(
                &strct.fields,
                "bitrs type must be defined as a tuple struct",
            ));
        };

        if !strct.generics.params.is_empty() {
            return Err(Error::new_spanned(
                &strct.generics,
                "generic parameters are not supported",
            ));
        }
        if let Some(where_clause) = &strct.generics.where_clause {
            return Err(Error::new_spanned(where_clause, "generic parameters are not supported"));
        }

        Ok(Self { def: strct, base: base_type, tags })
    }
}

//
// Parsing and binding for an individual bitfield.
//

#[derive(Clone)]
struct Bitfield {
    span: Span,
    name: Option<Ident>,
    high_bit: usize,
    low_bit: usize,
    repr: Option<Type>,
    doc_attrs: Vec<Attribute>,
    unshifted: bool,
    default: Option<Box<Expr>>,
}

impl Bitfield {
    const fn is_reserved(&self) -> bool {
        self.name.is_none()
    }

    fn display_name(&self) -> String {
        match &self.name {
            Some(name) => format!("`{name}`"),
            None => "reserved".to_string(),
        }
    }

    fn display_kind(&self) -> &'static str {
        if self.bit_width() == 1 { "bit" } else { "field" }
    }

    fn display_range(&self) -> String {
        if self.bit_width() == 1 {
            format!("{}", self.low_bit)
        } else {
            format!("[{}:{}]", self.high_bit, self.low_bit)
        }
    }

    const fn bit_width(&self) -> usize {
        self.high_bit - self.low_bit + 1
    }

    fn minimum_width_integral_type(&self) -> TokenStream2 {
        match self.bit_width() {
            2..=8 => quote! {u8},
            9..=16 => quote! {u16},
            17..=32 => quote! {u32},
            33..=64 => quote! {u64},
            65..=128 => quote! {u128},
            width => panic!("unexpected integral bit width: {width}"),
        }
    }

    fn getter_and_setter(&self, ty: &TypeDef) -> TokenStream2 {
        debug_assert!(!self.is_reserved());
        if self.unshifted {
            debug_assert!(self.repr.is_none());
        }

        let doc_attrs = &self.doc_attrs;
        let name = self.name.as_ref().unwrap();
        let setter_name = format_ident!("set_{}", name);
        let type_name = &ty.def.ident;

        let base_type = &ty.base.def;
        let high_bit = self.high_bit;
        let low_bit = self.low_bit;
        let shifted = !self.unshifted;

        let (get_doc, set_doc) = {
            let range = self.display_range();
            let qualifier = if shifted { "" } else { "unshifted " };
            (
                format!("The {qualifier}value of `{type_name}{range}`."),
                format!("Sets the {qualifier}value of `{type_name}{range}`."),
            )
        };

        if self.bit_width() == 1 && shifted {
            return quote! {
                #(#doc_attrs)*
                #[doc = #get_doc]
                #[inline]
                pub const fn #name(&self) -> bool {
                    ::bitrs::get_bit!(self.0, #low_bit)
                }

                #(#doc_attrs)*
                #[doc = #set_doc]
                #[inline]
                pub const fn #setter_name(&mut self, value: bool) -> &mut Self {
                    ::bitrs::set_bit!(self.0, #low_bit, value);
                    self
                }
            };
        }

        let clamped_type: TokenStream2 = if shifted {
            self.minimum_width_integral_type()
        } else {
            quote! { #base_type }
        };

        let get_clamped = quote! {
            ::bitrs::get_field!(
                #base_type,
                #clamped_type,
                #high_bit,
                #low_bit,
                #shifted,
                self.0
            )
        };

        let set_clamped = quote! {
            ::bitrs::set_field!(
                #base_type,
                #high_bit,
                #low_bit,
                #shifted,
                self.0,
                value
            )
        };

        // TODO: Custom-repr getter/setter can't be `const fn` until
        // `zerocopy::TryFromBytes` / `IntoBytes` have const methods.
        // Tracking upstream: https://github.com/google/zerocopy/issues/115.
        let getter = if let Some(repr) = &self.repr {
            let try_name = format_ident!("try_{}", name);
            let try_get_doc = format!("Fallible variant of [`Self::{name}`].");
            let panic_doc = format!(
                "# Panics\n\n\
                 Panics if the field's bit pattern is not a valid value of \
                 the custom representation. Use [`Self::{try_name}`] to \
                 recover from invalid bit patterns."
            );
            quote! {
                #(#doc_attrs)*
                #[doc = #get_doc]
                ///
                #[doc = #panic_doc]
                #[inline]
                pub fn #name(&self) -> #repr
                where
                    #repr: ::bitrs::__zerocopy::TryFromBytes,
                {
                    self.#try_name().unwrap()
                }

                #[doc = #try_get_doc]
                #[inline]
                pub fn #try_name(&self)
                    -> ::core::result::Result<#repr, ::bitrs::InvalidBits<#clamped_type>>
                where
                    #repr: ::bitrs::__zerocopy::TryFromBytes,
                {
                    use ::bitrs::__zerocopy::IntoBytes;
                    use ::bitrs::__zerocopy::TryFromBytes;
                    let value = #get_clamped ;
                    #repr::try_read_from_bytes(value.as_bytes())
                        .map_err(|_| ::bitrs::InvalidBits(value))
                }
            }
        } else {
            quote! {
                #(#doc_attrs)*
                #[doc = #get_doc]
                #[inline]
                pub const fn #name(&self) -> #clamped_type {
                    #get_clamped
                }
            }
        };

        let setter = if let Some(repr) = &self.repr {
            quote! {
                #[doc = #set_doc]
                #[inline]
                pub fn #setter_name(&mut self, value: #repr) -> &mut Self
                where
                    #repr: ::bitrs::__zerocopy::IntoBytes + ::bitrs::__zerocopy::Immutable
                 {
                    use ::bitrs::__zerocopy::IntoBytes;
                    use ::bitrs::__zerocopy::FromBytes;
                    const { assert!(::core::mem::size_of::<#repr>() == ::core::mem::size_of::<#clamped_type>()) }
                    let value = #clamped_type::read_from_bytes(value.as_bytes()).unwrap() as #base_type;
                    #set_clamped ;
                    self
                }
            }
        } else {
            quote! {
                #[doc = #set_doc]
                #[inline]
                pub const fn #setter_name(&mut self, value: #clamped_type) -> &mut Self {
                    let value = value as #base_type;
                    #set_clamped ;
                    self
                }
            }
        };

        quote! {
            #getter
            #setter
        }
    }
}

impl Parse for Bitfield {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        const INVALID_BITFIELD_DECL_FORM: &str = "bitfield declaration should take one of the following forms:\n\
            * `let $name @ $bit (= $default)?;`\n\
            * `let $name @ $high..$low (: $repr)? (= $default)?;`\n\
            * `let __ @ $bit (= $value)?;`\n\
            * `let __ @ $high..$low (= $value)?;`";
        let err = |spanned: &dyn ToTokens| Error::new_spanned(spanned, INVALID_BITFIELD_DECL_FORM);
        let wildcard_err = |spanned: &dyn ToTokens| {
            Error::new_spanned(
                spanned,
                "use `__` for reserved fields; bare `_` is not a valid \
                 identifier in this position",
            )
        };

        // `_ @ ...` is not valid Rust grammar (`@` requires an identifier on
        // its LHS), so syn would fail with a confusing "expected `;`" pointing
        // at `@`. Intercept that sequence here to issue the right error. Look
        // past any leading attributes (`#[unshifted]`, doc comments) on a fork
        // so the diagnostic still fires when the let is attribute-prefixed.
        let fork = input.fork();
        let _ = fork.call(Attribute::parse_outer)?;
        if fork.peek(Token![let]) && fork.peek2(Token![_]) && fork.peek3(Token![@]) {
            let _: Vec<Attribute> = input.call(Attribute::parse_outer)?;
            let _: Token![let] = input.parse()?;
            let underscore: Token![_] = input.parse()?;
            return Err(wildcard_err(&underscore));
        }

        let stmt = input.parse::<Stmt>()?;
        let Stmt::Local(ref local) = stmt else {
            return Err(err(&stmt));
        };

        let mut doc_attrs = Vec::new();
        let mut unshifted = false;
        for attr in &local.attrs {
            if attr.path().is_ident("doc") {
                doc_attrs.push(attr.clone());
            } else if attr.path().is_ident("unshifted") {
                if unshifted {
                    return Err(Error::new_spanned(attr, "duplicate `#[unshifted]` attribute"));
                }
                unshifted = true;
            } else {
                return Err(Error::new_spanned(
                    attr,
                    "attributes are not permitted on individual fields",
                ));
            }
        }

        // syn parks the `: Type` annotation in a `Pat::Type` wrapping the
        // inner pattern, so a custom repr surfaces here rather than on the
        // local itself.
        let (pat_ident, repr): (&PatIdent, Option<Type>) = match &local.pat {
            // `let foo @ ...;`
            Pat::Ident(binding) => (binding, None),
            Pat::Type(typed) => match &*typed.pat {
                // `let foo @ ...: Repr;`
                Pat::Ident(binding) => (binding, Some((*typed.ty).clone())),
                // `let _: T ...;`
                Pat::Wild(_) => return Err(wildcard_err(&typed.pat)),
                _ => return Err(err(&local.pat)),
            },
            // `let _ = ...;` or `let _;`
            Pat::Wild(_) => return Err(wildcard_err(&local.pat)),
            _ => return Err(err(&local.pat)),
        };

        if let Some(by_ref) = &pat_ident.by_ref {
            return Err(Error::new_spanned(
                by_ref,
                "`ref` is not permitted on bitfield declarations",
            ));
        }
        if let Some(mutability) = &pat_ident.mutability {
            return Err(Error::new_spanned(
                mutability,
                "`mut` is not permitted on bitfield declarations",
            ));
        }

        let ident_str = pat_ident.ident.to_string();
        let name: Option<Ident> = if ident_str == "__" {
            None
        } else if ident_str.starts_with('_') {
            return Err(Error::new_spanned(
                &pat_ident.ident,
                "leading-underscore identifiers are not permitted on \
                 bitfields; use `__` for reserved fields, or rename to a \
                 non-`_`-prefixed identifier",
            ));
        } else {
            Some(pat_ident.ident.clone())
        };

        let Some((_, subpat)) = &pat_ident.subpat else {
            return Err(Error::new_spanned(local, "missing `@ BIT_RANGE` in bitfield declaration"));
        };

        let int_lit_from_expr = |e: &Expr| -> Result<usize> {
            match e {
                Expr::Lit(ExprLit { lit: Lit::Int(i), .. }) => i.base10_parse(),
                _ => Err(err(e)),
            }
        };

        let (high_bit, low_bit) = match &**subpat {
            Pat::Lit(ExprLit { lit: Lit::Int(i), .. }) => {
                let n: usize = i.base10_parse()?;
                (n, n)
            }
            Pat::Range(range @ ExprRange { start, end, limits, .. }) => {
                if let RangeLimits::Closed(eq) = limits {
                    return Err(Error::new_spanned(
                        eq,
                        "bit ranges use the exclusive `..` token, but both \
                         endpoints are treated as inclusive bit indices; \
                         write `H..L`, not `H..=L`",
                    ));
                }
                let (Some(start), Some(end)) = (start, end) else {
                    return Err(Error::new_spanned(range, "bit range requires both endpoints"));
                };
                let high = int_lit_from_expr(start)?;
                let low = int_lit_from_expr(end)?;
                if high < low {
                    return Err(Error::new_spanned(range, "first high bit, then low"));
                }
                (high, low)
            }
            _ => return Err(err(subpat)),
        };

        let default_or_value = if let Some(ref init) = local.init {
            if init.diverge.is_some() {
                return Err(err(local));
            }
            Some(init.expr.clone())
        } else {
            None
        };

        if !doc_attrs.is_empty() && name.is_none() {
            return Err(Error::new_spanned(
                &doc_attrs[0],
                "doc comments are not permitted on reserved fields",
            ));
        }

        if let Some(repr) = &repr {
            if name.is_none() {
                return Err(Error::new_spanned(
                    repr,
                    "custom representations are not permitted for reserved fields",
                ));
            }
            if high_bit == low_bit {
                return Err(Error::new_spanned(
                    repr,
                    "custom representations are not permitted for bits",
                ));
            }
        }

        if unshifted && name.is_none() {
            return Err(Error::new_spanned(
                &local.pat,
                "`#[unshifted]` is not permitted on reserved fields",
            ));
        }
        if unshifted && repr.is_some() {
            return Err(Error::new_spanned(
                repr,
                "`#[unshifted]` is not permitted on fields with custom representations",
            ));
        }

        Ok(Bitfield {
            span: stmt.span(),
            name,
            high_bit,
            low_bit,
            repr,
            doc_attrs,
            unshifted,
            default: default_or_value,
        })
    }
}

struct Layout {
    ty: TypeDef,
    named: Vec<Bitfield>,
    reserved: Vec<Bitfield>,
}

impl Layout {
    fn constants(&self) -> TokenStream2 {
        let base = &self.ty.base.def;

        let mut field_constants = Vec::new();
        let mut field_metadata = Vec::new();
        let mut checks = Vec::new();
        let mut default_stmts = Vec::new();

        for field in &self.named {
            let name_lower = field.name.as_ref().unwrap().to_string();
            let name_upper = name_lower.to_uppercase();
            let high_bit = field.high_bit;
            let low_bit = Literal::usize_unsuffixed(field.low_bit);
            let shifted_mask = quote! {::bitrs::shifted_mask!(#base, #high_bit, #low_bit) };

            let mask_name = format_ident!("{name_upper}_MASK");
            let mask_doc = format!("Unshifted bitmask of `{name_lower}`.");
            let shift_name = format_ident!("{name_upper}_SHIFT");
            let shift_doc = format!("Bit shift (i.e., the low bit) of `{name_lower}`.");

            field_constants.push(quote! {
                #[doc = #mask_doc]
                pub const #mask_name: #base = (#shifted_mask << #low_bit);
                #[doc = #shift_doc]
                pub const #shift_name: usize = #low_bit;
            });

            if let Some(default) = &field.default {
                let default_name = format_ident!("{name_upper}_DEFAULT");
                let doc = format!("Pre-shifted default value of the `{name_lower}` field.",);
                field_constants.push(quote! {
                    #[doc = #doc]
                    pub const #default_name: #base = ((#default) as #base) << #low_bit;
                });
                checks.push(quote! {
                    const { assert!(((#default) as #base) << #low_bit & !(#shifted_mask << #low_bit) == 0) }
                });
                default_stmts.push(quote! {
                    { v |= Self::#default_name; }
                });
            }

            let high_bit = Literal::usize_unsuffixed(field.high_bit);
            let default = if let Some(default) = &field.default {
                quote! { #default }
            } else {
                quote! { 0 }
            };
            field_metadata.push(quote! {
                ::bitrs::FieldMetadata::<#base>{
                    name: #name_lower,
                    high_bit: #high_bit,
                    low_bit: #low_bit,
                    default: #default as #base,
                },
            });
        }

        let num_fields = self.named.len();

        let mut rsvd1_stmts = Vec::new();
        let mut rsvd0_stmts = Vec::new();
        for rsvd in &self.reserved {
            let rsvd_value = rsvd.default.as_ref().unwrap();
            let high_bit = rsvd.high_bit;
            let low_bit = Literal::usize_unsuffixed(rsvd.low_bit);
            let shifted_mask = quote! {::bitrs::shifted_mask!(#base, #high_bit, #low_bit) };
            let name = format_ident!("RSVD_{}_{}", rsvd.high_bit, rsvd.low_bit);

            field_constants.push(quote! {
                const #name: #base = (#rsvd_value as #base) << #low_bit;
            });
            checks.push(quote! {
                const { assert!((#rsvd_value as #base) << #low_bit & !(#shifted_mask << #low_bit) == 0) }
            });
            rsvd1_stmts.push(quote! {
                { v |= Self::#name; }
            });
            rsvd0_stmts.push(quote! {
                { v |= !Self::#name & (#shifted_mask << #low_bit); }
            });
        }
        field_constants.push(quote! {
            #[doc(hidden)]
            const NUM_FIELDS: usize = #num_fields;
            /// Metadata of all named fields in the layout.
            pub const FIELDS: [::bitrs::FieldMetadata::<#base>; #num_fields] = [
                #(#field_metadata)*
            ];
        });

        let check_fn = if checks.is_empty() {
            quote! {}
        } else {
            let checks = checks.into_iter();
            quote! {
                #[forbid(overflowing_literals)]
                const fn check_defaults() -> () {
                    #(#checks)*
                }
            }
        };

        quote! {
            /// Mask of all reserved-as-1 bits.
            pub const RSVD1_MASK: #base = {
                let mut v: #base = 0;
                #(#rsvd1_stmts)*
                v
            };
            /// Mask of all reserved-as-0 bits.
            pub const RSVD0_MASK: #base = {
                let mut v: #base = 0;
                #(#rsvd0_stmts)*
                v
            };
            /// The default value of the layout, combining all field
            /// defaults and reserved-as values.
            pub const DEFAULT: #base = {
                let mut v: #base = Self::RSVD1_MASK;
                #(#default_stmts)*
                v
            };

            #(#field_constants)*

            #check_fn
        }
    }

    fn iter_impl(&self) -> TokenStream2 {
        let ty = &self.ty.def.ident;
        let base = &self.ty.base.def;
        let iter_type = format_ident!("{}Iter", ty);
        let vis = &self.ty.def.vis;

        quote! {
            #[doc(hidden)]
            #vis struct #iter_type(#base, usize, usize);

            impl ::core::iter::Iterator for #iter_type {
                type Item = (&'static ::bitrs::FieldMetadata<#base>, #base);

                fn next(&mut self) -> Option<Self::Item> {
                    if self.1 >= self.2 {
                        return None;
                    }
                    let metadata = &#ty::FIELDS[self.1];
                    let shifted_mask = (1 << (metadata.high_bit - metadata.low_bit + 1)) - 1;
                    let value = (self.0 >> metadata.low_bit) & shifted_mask;
                    self.1 += 1;
                    Some((metadata, value))
                }
            }

            impl ::core::iter::DoubleEndedIterator for #iter_type {
                fn next_back(&mut self) -> Option<Self::Item> {
                    if self.1 >= self.2 {
                        return None;
                    }
                    self.2 -= 1;
                    let metadata = &#ty::FIELDS[self.2];
                    let shifted_mask = (1 << (metadata.high_bit - metadata.low_bit + 1)) - 1;
                    let value = (self.0 >> metadata.low_bit) & shifted_mask;
                    Some((metadata, value))
                }
            }

            impl #ty {
                /// Returns an iterator over
                /// ([metadata][`bitrs::FieldMetadata`], value) pairs for each
                /// field.
                pub fn iter(&self) -> #iter_type {
                    #iter_type(self.0, 0, Self::NUM_FIELDS)
                }
            }

            impl ::core::iter::IntoIterator for #ty {
                type Item = (&'static ::bitrs::FieldMetadata<#base>, #base);
                type IntoIter = #iter_type;

                fn into_iter(self) -> Self::IntoIter { #iter_type(self.0, 0, Self::NUM_FIELDS) }
            }

            impl<'a> ::core::iter::IntoIterator for &'a #ty {
                type Item = (&'static ::bitrs::FieldMetadata<#base>, #base);
                type IntoIter = #iter_type;

                fn into_iter(self) -> Self::IntoIter { #iter_type(self.0, 0, #ty::NUM_FIELDS) }
            }
        }
    }

    fn getters_and_setters(&self) -> impl Iterator<Item = TokenStream2> + '_ {
        self.named.iter().map(|field| field.getter_and_setter(&self.ty))
    }

    fn fmt_fn(&self, integral_specifier: &str) -> TokenStream2 {
        let ty_str = &self.ty.def.ident.to_string();

        let mut custom_repr_fields =
            self.named.iter().filter(|field| field.repr.is_some()).peekable();

        let where_clause = if custom_repr_fields.peek().is_some() {
            let bounds = custom_repr_fields.map(|field| {
                let repr = field.repr.as_ref().unwrap();
                quote! {#repr: ::core::fmt::Debug,}
            });
            quote! {
                where
                    #(#bounds)*
            }
        } else {
            quote! {}
        };

        let fmt_fields = self.named.iter().map(|field| {
            let name = &field.name;
            let name_str = name.as_ref().unwrap().to_string();
            let default_specifier = if field.bit_width() == 1 { "" } else { integral_specifier };
            if field.repr.is_some() {
                let try_name = format_ident!("try_{}", name.as_ref().unwrap());
                let ok_format_string = format!("{{indent}}{name_str}: {{:#?}},{{sep}}");
                let ok_format_string = Literal::string(&ok_format_string);
                let err_format_string =
                    format!("{{indent}}{name_str}: InvalidBits({{{default_specifier}}}),{{sep}}");
                let err_format_string = Literal::string(&err_format_string);
                quote! {
                    {
                        match self.#try_name() {
                            Ok(value) => write!(f, #ok_format_string, value),
                            Err(invalid) => write!(f, #err_format_string, invalid.0),
                        }?;
                    }
                }
            } else {
                let format_string =
                    format!("{{indent}}{name_str}: {{{default_specifier}}},{{sep}}");
                let format_string = Literal::string(&format_string);
                quote! {
                    { write!(f, #format_string, self.#name())?; }
                }
            }
        });

        quote! {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result
            #where_clause
            {
                let (sep, indent) = if f.alternate() {
                    ('\n', "    ")
                } else {
                    (' ', "")
                };
                write!(f, "{} {{{sep}", #ty_str)?;
                #(#fmt_fields)*
                write!(f, "}}")
            }
        }
    }

    fn fmt_impls(&self) -> TokenStream2 {
        let ty = &self.ty.def.ident;
        let lower_hex_fmt = self.fmt_fn(":#x");
        let upper_hex_fmt = self.fmt_fn(":#X");
        let binary_fmt = self.fmt_fn(":#b");
        let octal_fmt = self.fmt_fn(":#o");
        quote! {
            impl ::core::fmt::Debug for #ty {
                fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                    ::core::fmt::LowerHex::fmt(self, f)
                }
            }

            impl ::core::fmt::Binary for #ty {
                #binary_fmt
            }

            impl ::core::fmt::LowerHex for #ty {
                #lower_hex_fmt
            }

            impl ::core::fmt::UpperHex for #ty {
                #upper_hex_fmt
            }

            impl ::core::fmt::Octal for #ty {
                #octal_fmt
            }
        }
    }
}

impl Layout {
    /// Builds a `Layout` from a pre-parsed type and a flat field list. Runs
    /// the within-layout invariants (sort, overlap check, high-bit bound),
    /// returning the first violation as an error, then splits fields into
    /// named/reserved.
    fn from_parts(ty: TypeDef, mut fields: Vec<Bitfield>) -> Result<Self> {
        fields.sort_by_key(|field| field.low_bit);

        // TODO(https://github.com/rust-lang/rust/issues/54725): For the
        // overlap diagnostic, it would be nice to Span::join() the two
        // spans, but that's still experimental.
        let mut seen: HashMap<&Ident, &Bitfield> = HashMap::new();
        let mut prev: Option<&Bitfield> = None;
        for field in &fields {
            if let Some(prev) = prev
                && prev.high_bit >= field.low_bit
            {
                return Err(Error::new(
                    field.span,
                    format!(
                        "{} ({} {}) overlaps with {} ({} {})",
                        field.display_name(),
                        field.display_kind(),
                        field.display_range(),
                        prev.display_name(),
                        prev.display_kind(),
                        prev.display_range(),
                    ),
                ));
            }
            if let Some(name) = &field.name {
                if let Some(prev_named) = seen.get(name) {
                    return Err(Error::new(
                        field.span,
                        format!(
                            "field `{name}` declared twice ({} {} and {} {})",
                            prev_named.display_kind(),
                            prev_named.display_range(),
                            field.display_kind(),
                            field.display_range(),
                        ),
                    ));
                }
                seen.insert(name, field);
            }
            prev = Some(field);
        }

        let highest_possible = ty.base.ty.high_bit();
        if let Some(highest) = fields.last()
            && highest.high_bit > highest_possible
        {
            return Err(Error::new(
                highest.span,
                format!(
                    "high bit {} exceeds the highest possible value \
                     of {highest_possible}",
                    highest.high_bit
                ),
            ));
        }

        // Drain in reverse so `named` and `reserved` end up in descending
        // bit order (high bit first). The codegen doesn't depend on a
        // specific order, but the metadata does, which is downstream of this
        // ordering.
        let mut layout = Self { ty, named: vec![], reserved: vec![] };
        for field in fields.into_iter().rev() {
            if field.is_reserved() {
                if field.default.is_some() {
                    layout.reserved.push(field);
                }
            } else {
                layout.named.push(field);
            }
        }
        Ok(layout)
    }
}

impl Parse for Layout {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let input = {
            let content;
            braced!(content in input);
            content
        };

        let ty = input.parse::<TypeDef>()?;
        if let Some(tag) = ty.tags.iter().next() {
            return Err(Error::new_spanned(
                tag,
                "`#[bitrs(...)]` tags are only meaningful in `multilayout!`",
            ));
        }

        let set = input.parse::<BitfieldSet>()?;
        if let Some(pred) = set.predicate {
            return Err(Error::new(
                pred.span(),
                "tag predicates are only meaningful in `multilayout!`",
            ));
        }
        Self::from_parts(ty, set.fields)
    }
}

impl ToTokens for Layout {
    fn to_tokens(&self, tokens: &mut TokenStream2) {
        let type_def = &self.ty.def;
        let type_name = &type_def.ident;
        let base = &self.ty.base.def;

        let constants = self.constants();
        let getters_and_setters = self.getters_and_setters();
        let iter_impl = self.iter_impl();
        let fmt_impls = self.fmt_impls();
        quote! {
            #[derive(Copy, Clone, Eq, PartialEq)]
            #type_def

            impl #type_name {
                #constants

                /// Creates a new instance with reserved-as-1 bits set and
                /// all other bits zeroed (i.e., with a value of
                /// [`Self::RSVD1_MASK`]).
                pub const fn new() -> Self {
                    Self(Self::RSVD1_MASK)
                }

                pub const fn bits(self) -> #base {
                    self.0
                }

                #(#getters_and_setters)*
            }

            impl ::core::default::Default for #type_name {
                /// Returns an instance with the default bits set (i.e,. with a
                /// value of [`Self::DEFAULT`].
                fn default() -> Self {
                    Self(Self::DEFAULT)
                }
            }

            impl ::core::convert::From<#base> for #type_name {
                // `RSVD{0,1}_MASK` may be zero, in which case the following
                // mask conditions might be trivially true.
                #[allow(clippy::bad_bit_mask)]
                fn from(value: #base) -> Self {
                    debug_assert!(
                        value & Self::RSVD1_MASK == Self::RSVD1_MASK,
                        "from(): Invalid base value ({value:#x}) has reserved-as-1 bits ({:#x}) unset",
                        Self::RSVD1_MASK,
                    );
                    debug_assert!(
                        !value & Self::RSVD0_MASK == Self::RSVD0_MASK,
                        "from(): Invalid base value ({value:#x}) has reserved-as-0 bits ({:#x}) set",
                        Self::RSVD0_MASK,
                    );
                    Self(value)
                }
            }

            impl ::core::convert::From<#type_name> for #base {
                fn from(value: #type_name) -> Self {
                    value.bits()
                }
            }

            #iter_impl

            #fmt_impls
        }
        .to_tokens(tokens);
    }
}

//
// `multilayout!`: a family of layouts in one place. Parses one or more struct
// heads — each optionally annotated with `#[bitrs(tag, ...)]` to declare its
// tag set — followed by a sequence of bitfield sets. Each set is
// either bare (`{ ... }` — applies to every struct) or predicated
// (`#[<predicate>] { ... }`). A predicate is a boolean expression over the
// declared tags: a bare tag identifier, `all(<predicate>, ...)`,
// `any(<predicate>, ...)`, or `not(<predicate>)`, freely nested. A
// bitfield set applies to a struct iff its predicate evaluates true against
// that struct's tag set; an unannotated set always applies. Set order is
// irrelevant; each set's fields union into the matching per-struct
// field lists. Emits one `Layout`-equivalent stream per declared struct.
//

#[derive(Clone)]
enum Predicate {
    Tag(Ident),
    All(Vec<Predicate>, Span),
    Any(Vec<Predicate>, Span),
    Not(Box<Predicate>, Span),
}

impl Predicate {
    fn span(&self) -> Span {
        match self {
            Predicate::Tag(tag) => tag.span(),
            Predicate::All(_, span) | Predicate::Any(_, span) | Predicate::Not(_, span) => *span,
        }
    }

    /// Validates that any tag in the predicate exists in `all_tags`.
    fn validate(&self, all_tags: &HashSet<Ident>) -> Result<()> {
        match self {
            Predicate::Tag(tag) => {
                if !all_tags.contains(tag) {
                    return Err(Error::new_spanned(tag, format!("unknown tag `{tag}`")));
                }
                Ok(())
            }
            Predicate::All(preds, _) | Predicate::Any(preds, _) => {
                for pred in preds {
                    pred.validate(all_tags)?;
                }
                Ok(())
            }
            Predicate::Not(pred, _) => pred.validate(all_tags),
        }
    }

    /// Evaluates the predicate against a struct's `tags`.
    fn eval(&self, tags: &HashSet<Ident>) -> bool {
        match self {
            Predicate::Tag(tag) => tags.contains(tag),
            Predicate::All(preds, _) => preds.iter().all(|pred| pred.eval(tags)),
            Predicate::Any(preds, _) => preds.iter().any(|pred| pred.eval(tags)),
            Predicate::Not(pred, _) => !pred.eval(tags),
        }
    }
}

impl Parse for Predicate {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let ident: Ident = input.parse()?;
        let has_paren = input.peek(token::Paren);
        if ident == "all" || ident == "any" {
            if !has_paren {
                return Err(Error::new_spanned(
                    &ident,
                    format!("`{ident}` requires a parenthesized argument list"),
                ));
            }
            let content;
            parenthesized!(content in input);
            let preds: Punctuated<Predicate, Token![,]> = Punctuated::parse_terminated(&content)?;
            let preds = preds.into_iter().collect();
            let span = ident.span();
            if ident == "all" {
                return Ok(Predicate::All(preds, span));
            }
            return Ok(Predicate::Any(preds, span));
        }

        if ident == "not" {
            if !has_paren {
                return Err(Error::new_spanned(&ident, "`not` requires a parenthesized argument"));
            }
            let content;
            parenthesized!(content in input);
            let inner: Predicate = content.parse()?;
            let _ = content.parse::<Option<Token![,]>>()?;
            if !content.is_empty() {
                return Err(Error::new(content.span(), "`not(...)` takes exactly one argument"));
            }
            return Ok(Predicate::Not(Box::new(inner), ident.span()));
        }
        if has_paren {
            return Err(Error::new_spanned(
                &ident,
                format!(
                    "unknown predicate function `{ident}`; expected \
                         `all`, `any`, or `not`"
                ),
            ));
        }
        Ok(Predicate::Tag(ident))
    }
}

/// A block of bitfields, optionally gated by a predicate attribute.
struct BitfieldSet {
    predicate: Option<Predicate>,
    fields: Vec<Bitfield>,
}

impl Parse for BitfieldSet {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let mut predicate: Option<Predicate> = None;
        while input.peek(Token![#]) {
            let hash: Token![#] = input.parse()?;
            let content;
            bracketed!(content in input);
            if predicate.is_some() {
                return Err(Error::new(
                    hash.span,
                    "only one predicate attribute is permitted per \
                     bitfield set; combine with `all(...)` / \
                     `any(...)` if you need a compound predicate",
                ));
            }
            let pred: Predicate = content.parse()?;
            if !content.is_empty() {
                return Err(Error::new(
                    content.span(),
                    "invalid predicate attribute; expected a bare tag identifier, \
                     `all(...)`, `any(...)`, or `not(...)`",
                ));
            }
            predicate = Some(pred);
        }

        let block;
        braced!(block in input);
        let mut fields = Vec::new();
        while !block.is_empty() {
            fields.push(block.parse::<Bitfield>()?);
        }

        Ok(Self { predicate, fields })
    }
}

struct Multilayout {
    layouts: Vec<Layout>,
}

/// Returns `true` if the next item in `input` looks like a bitfield set (i.e.,
/// a bare `{ ... }` or one prefixed by predicate attributes). Pure lookahead:
/// peels any outer attributes on a fork and reports whether the next
/// significant token is `{`. If the fork's attribute parse fails (malformed
/// attribute), we report "not a bitfield set" and let the main parser surface
/// the syntax error through `TypeDef::parse`.
fn looks_like_bitfield_set(input: ParseStream<'_>) -> bool {
    if input.peek(token::Brace) {
        return true;
    }
    if !input.peek(Token![#]) {
        return false;
    }
    let fork = input.fork();
    while fork.peek(Token![#]) {
        let _ = fork.parse::<Token![#]>();
        let Ok(group) = fork.parse::<Group>() else {
            return false;
        };
        if group.delimiter() != Delimiter::Bracket {
            return false;
        }
    }
    fork.peek(token::Brace)
}

impl Parse for Multilayout {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let input = {
            let content;
            braced!(content in input);
            content
        };

        // Phase 1: struct heads.
        let mut types: Vec<TypeDef> = Vec::new();
        while !looks_like_bitfield_set(&input) {
            if input.is_empty() {
                return Err(Error::new(
                    input.span(),
                    "expected at least one bitfield set after the struct declarations",
                ));
            }
            types.push(input.parse::<TypeDef>()?);
        }
        if types.is_empty() {
            return Err(Error::new(
                input.span(),
                "expected at least one `struct Name(base);` declaration",
            ));
        }

        // The union of all declared tags.
        let mut all_tags: HashSet<Ident> = HashSet::new();
        for ty in &types {
            for t in &ty.tags {
                all_tags.insert(t.clone());
            }
        }

        // Phase 2: bitfield sets.
        let mut bitfield_sets: Vec<BitfieldSet> = Vec::new();
        while !input.is_empty() {
            bitfield_sets.push(input.parse::<BitfieldSet>()?);
        }

        // Validate that all predicates reference declared tags.
        for set in &bitfield_sets {
            if let Some(pred) = &set.predicate {
                pred.validate(&all_tags)?;
            }
        }

        // Fan out: per declared struct, union every bitfield set whose
        // predicate is satisfied by the struct's tag set and build its
        // Layout. Set coverage is validated below.
        let mut matched = vec![false; bitfield_sets.len()];
        let mut layouts: Vec<Layout> = Vec::new();
        for ty in types {
            let mut fields: Vec<Bitfield> = Vec::new();
            for (i, set) in bitfield_sets.iter().enumerate() {
                let belongs = match &set.predicate {
                    None => true,
                    Some(pred) => {
                        let belongs = pred.eval(&ty.tags);
                        if belongs {
                            matched[i] = true;
                        }
                        belongs
                    }
                };
                if belongs {
                    fields.extend(set.fields.iter().cloned());
                }
            }
            let ident = ty.def.ident.clone();
            let layout = Layout::from_parts(ty, fields)
                .map_err(|err| Error::new(err.span(), format!("in struct `{ident}`: {err}")))?;
            layouts.push(layout);
        }

        // Every bitfield set must match at least one declared struct.
        for (set, matched) in bitfield_sets.iter().zip(matched) {
            if let Some(pred) = &set.predicate {
                if !matched {
                    return Err(Error::new(
                        pred.span(),
                        "bitfield set predicate matches no declared structs",
                    ));
                }
            }
        }

        Ok(Self { layouts })
    }
}

impl ToTokens for Multilayout {
    fn to_tokens(&self, tokens: &mut TokenStream2) {
        for layout in &self.layouts {
            layout.to_tokens(tokens);
        }
    }
}
