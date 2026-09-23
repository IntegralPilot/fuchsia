// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::{Fields, Ident, ItemStruct, Type, TypePath, parse_macro_input};

struct MutexField {
    ident: Ident,
    class_ident: Ident,
    class_type: proc_macro2::TokenStream,
    mutex_type: proc_macro2::TokenStream,
    custom_class: Option<Type>,
    custom_flags: Option<syn::Expr>,
    is_phantom: bool,
}

struct GuardedField {
    ident: Ident,
    mutex_ident: Ident,
    ty: Type,
    vis: syn::Visibility,
    project_as_pin: bool,
    is_konce_cell: bool,
    konce_cell_inner_ty: Option<Type>,
}

struct FieldAttrAnalysis {
    is_mutex: bool,
    is_brwlock: bool,
    guarded_by: Option<Ident>,
    is_pinned: bool,
    is_unpinned: bool,
    custom_class: Option<Type>,
    custom_flags: Option<syn::Expr>,
}

fn parse_mutex_or_brwlock_args(
    meta_list: &syn::MetaList,
    custom_class: &mut Option<Type>,
    custom_flags: &mut Option<syn::Expr>,
    errors: &mut Vec<syn::Error>,
    attr_name: &str,
) {
    let nested = match meta_list
        .parse_args_with(Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated)
    {
        Ok(nested) => nested,
        Err(_) => match meta_list.parse_args::<Type>() {
            Ok(ty) => {
                *custom_class = Some(ty);
                return;
            }
            Err(e) => {
                errors.push(e);
                return;
            }
        },
    };

    for meta in nested {
        match meta {
            syn::Meta::NameValue(nv) if nv.path.is_ident("flags") => {
                *custom_flags = Some(nv.value);
            }
            syn::Meta::Path(path) => {
                *custom_class = Some(Type::Path(TypePath { qself: None, path }));
            }
            syn::Meta::List(list) => {
                let tokens = quote! { #list };
                match syn::parse2::<Type>(tokens) {
                    Ok(ty) => *custom_class = Some(ty),
                    Err(e) => errors.push(e),
                }
            }
            other => {
                errors.push(syn::Error::new(
                    other.span(),
                    format!(
                        "#[{attr_name}] attribute accepts `LockClass`, `flags = <expr>`, or both"
                    ),
                ));
            }
        }
    }
}

fn parse_field_attributes(
    field: &mut syn::Field,
    errors: &mut Vec<syn::Error>,
) -> FieldAttrAnalysis {
    let mut analysis = FieldAttrAnalysis {
        is_mutex: false,
        is_brwlock: false,
        guarded_by: None,
        is_pinned: false,
        is_unpinned: false,
        custom_class: None,
        custom_flags: None,
    };

    field.attrs.retain(|attr| {
        if attr.path().is_ident("mutex") {
            analysis.is_mutex = true;
            if let syn::Meta::List(meta_list) = &attr.meta {
                parse_mutex_or_brwlock_args(
                    meta_list,
                    &mut analysis.custom_class,
                    &mut analysis.custom_flags,
                    errors,
                    "mutex",
                );
            } else if !matches!(attr.meta, syn::Meta::Path(_)) {
                errors.push(syn::Error::new(
                    attr.meta.span(),
                    "#[mutex] attribute must be either #[mutex], #[mutex(LockClass)], \
                     #[mutex(flags = ...)], or #[mutex(LockClass, flags = ...)].",
                ));
            }
            false
        } else if attr.path().is_ident("brwlock") {
            analysis.is_brwlock = true;
            if let syn::Meta::List(meta_list) = &attr.meta {
                parse_mutex_or_brwlock_args(
                    meta_list,
                    &mut analysis.custom_class,
                    &mut analysis.custom_flags,
                    errors,
                    "brwlock",
                );
            } else if !matches!(attr.meta, syn::Meta::Path(_)) {
                errors.push(syn::Error::new(
                    attr.meta.span(),
                    "#[brwlock] attribute must be either #[brwlock], #[brwlock(LockClass)], \
                     #[brwlock(flags = ...)], or #[brwlock(LockClass, flags = ...)].",
                ));
            }
            false
        } else if attr.path().is_ident("guarded_by") {
            if let syn::Meta::List(meta_list) = &attr.meta {
                if let Ok(ident) = meta_list.parse_args::<Ident>() {
                    analysis.guarded_by = Some(ident);
                }
            }
            false
        } else if attr.path().is_ident("pin") {
            analysis.is_pinned = true;
            true
        } else if attr.path().is_ident("ksync") {
            if let syn::Meta::List(meta_list) = &attr.meta {
                if let Ok(ident) = meta_list.parse_args::<Ident>() {
                    if ident == "unpinned" {
                        // TODO(https://fxbug.dev/536051491): Remove this workaround once
                        // WavlTree/region-alloc support safe pin projection.
                        analysis.is_unpinned = true;
                        return false;
                    }
                }
            }
            true
        } else {
            true
        }
    });

    analysis
}

fn check_unique_name(
    name: &str,
    span: proc_macro2::Span,
    field_ident: &Ident,
    kind: &str,
    generated_names: &mut std::collections::HashSet<String>,
    errors: &mut Vec<syn::Error>,
) {
    if !generated_names.insert(name.to_string()) {
        errors.push(syn::Error::new(
            span,
            format!(
                "The lock field '{field_ident}' generates the duplicate {kind} name '{name}'. \
                 Please use distinct field names."
            ),
        ));
    }
}

struct GenericsInfo<'a> {
    params_no_defaults: Punctuated<syn::GenericParam, syn::token::Comma>,
    ty_params: Vec<proc_macro2::TokenStream>,
    class_params_no_defaults: Punctuated<syn::GenericParam, syn::token::Comma>,
    class_ty_params: Vec<proc_macro2::TokenStream>,
    phantom_ty_params: Vec<&'a Ident>,
    phantom_lifetimes: Vec<&'a syn::Lifetime>,
}

fn extract_generics_info<'a>(generics: &'a syn::Generics) -> GenericsInfo<'a> {
    let mut ty_params = Vec::new();
    let mut class_ty_params = Vec::new();
    let mut class_params_no_defaults = Punctuated::new();
    let mut phantom_ty_params = Vec::new();
    let mut phantom_lifetimes = Vec::new();

    for param in &generics.params {
        match param {
            syn::GenericParam::Type(type_param) => {
                let ident = &type_param.ident;
                ty_params.push(quote! { #ident });
                class_ty_params.push(quote! { #ident });
                phantom_ty_params.push(ident);

                let mut p = type_param.clone();
                p.default = None;
                class_params_no_defaults.push(syn::GenericParam::Type(p));
            }
            syn::GenericParam::Const(const_param) => {
                let ident = &const_param.ident;
                ty_params.push(quote! { #ident });
            }
            syn::GenericParam::Lifetime(lifetime_param) => {
                let lifetime = &lifetime_param.lifetime;
                ty_params.push(quote! { #lifetime });
                class_ty_params.push(quote! { #lifetime });
                phantom_lifetimes.push(lifetime);

                class_params_no_defaults.push(syn::GenericParam::Lifetime(lifetime_param.clone()));
            }
        }
    }

    let mut params_no_defaults = generics.params.clone();
    for param in &mut params_no_defaults {
        match param {
            syn::GenericParam::Type(type_param) => type_param.default = None,
            syn::GenericParam::Const(const_param) => const_param.default = None,
            syn::GenericParam::Lifetime(_) => {}
        }
    }

    GenericsInfo {
        params_no_defaults,
        ty_params,
        class_params_no_defaults,
        class_ty_params,
        phantom_ty_params,
        phantom_lifetimes,
    }
}

#[derive(Default)]
struct FieldCode {
    read_token_guard_accessors: proc_macro2::TokenStream,
    write_token_guard_accessors: proc_macro2::TokenStream,
    read_guard_accessors: proc_macro2::TokenStream,
    write_guard_accessors: proc_macro2::TokenStream,
    read_fields_decl: proc_macro2::TokenStream,
    write_fields_decl: proc_macro2::TokenStream,
    read_fields_init: proc_macro2::TokenStream,
    write_fields_init: proc_macro2::TokenStream,
}

fn generate_field_code(
    guarded_fields: &[&GuardedField],
    class_type: &proc_macro2::TokenStream,
) -> FieldCode {
    let mut code = FieldCode::default();

    for f in guarded_fields {
        let f_ident = &f.ident;
        let f_ty = &f.ty;
        let f_vis = &f.vis;
        let f_mut_ident = format_ident!("{}_mut", f_ident);
        let f_cell_ident = format_ident!("{}_cell", f_ident);

        // Read accessors and fields
        if f.is_konce_cell {
            let inner_ty = f.konce_cell_inner_ty.as_ref().unwrap();

            code.read_token_guard_accessors.extend(quote! {
                #[inline]
                #f_vis fn #f_ident(&self) -> ::core::option::Option<&#inner_ty> {
                    // SAFETY: The lock token proves that the lock protecting this cell is held.
                    unsafe { self.parent.#f_ident.get(self.token) }
                }
            });

            code.read_guard_accessors.extend(quote! {
                #[inline]
                #f_vis fn #f_ident(&self) -> ::core::option::Option<&#inner_ty> {
                    // SAFETY: The lock token proves that the lock protecting this cell is held.
                    unsafe { self.parent.#f_ident.get(self.inner.token()) }
                }
            });

            code.read_fields_decl.extend(quote! {
                #[allow(dead_code)]
                #f_vis #f_ident: ::core::option::Option<&'b #inner_ty>,
            });

            code.read_fields_init.extend(quote! {
                // SAFETY: The guard token proves shared access to the cell.
                #f_ident: unsafe { me.parent.#f_ident.get(token) },
            });
        } else {
            code.read_token_guard_accessors.extend(quote! {
                #[inline]
                #f_vis fn #f_ident(&self) -> &#f_ty {
                    // SAFETY: The lock token proves that the lock protecting this cell is held.
                    unsafe { self.parent.#f_ident.get(self.token) }
                }
            });

            code.read_guard_accessors.extend(quote! {
                #[inline]
                #f_vis fn #f_ident(&self) -> &#f_ty {
                    // SAFETY: The lock token proves that the lock protecting this cell is held.
                    unsafe { self.parent.#f_ident.get(self.inner.token()) }
                }
            });

            code.read_fields_decl.extend(quote! {
                #[allow(dead_code)]
                #f_vis #f_ident: &'b #f_ty,
            });

            code.read_fields_init.extend(quote! {
                // SAFETY: The guard token proves shared access to the cell.
                #f_ident: unsafe { me.parent.#f_ident.get(token) },
            });
        }

        // Write accessors and fields
        if f.is_konce_cell {
            let inner_ty = f.konce_cell_inner_ty.as_ref().unwrap();

            code.write_token_guard_accessors.extend(quote! {
                #[inline]
                #f_vis fn #f_ident(&self) -> ::core::option::Option<&#inner_ty> {
                    // SAFETY: The lock token proves that the lock protecting this cell is held.
                    unsafe { self.parent.#f_ident.get(&*self.token) }
                }

                #[inline]
                #f_vis fn #f_mut_ident(&mut self) -> ::core::option::Option<&mut #inner_ty> {
                    // SAFETY: The lock token proves that the lock protecting this cell is held.
                    unsafe { self.parent.#f_ident.get_mut(&mut *self.token) }
                }

                #[inline]
                #f_vis fn #f_cell_ident(&mut self) -> ::ksync::KOnceCellGuard<'_, #inner_ty, #class_type> {
                    // SAFETY: We hold an exclusive mutable reference to the guard token.
                    unsafe { self.parent.#f_ident.guard(&mut *self.token) }
                }
            });

            code.write_guard_accessors.extend(quote! {
                #[inline]
                #f_vis fn #f_ident(&self) -> ::core::option::Option<&#inner_ty> {
                    // SAFETY: The lock token proves that the lock protecting this cell is held.
                    unsafe { self.parent.#f_ident.get(self.inner.token()) }
                }

                #[inline]
                #f_vis fn #f_mut_ident(self: ::core::pin::Pin<&mut Self>) -> ::core::option::Option<&mut #inner_ty> {
                    let me = unsafe { self.get_unchecked_mut() };
                    let inner_pin = unsafe { ::core::pin::Pin::new_unchecked(&mut me.inner) };
                    // SAFETY: We hold an exclusive mutable reference to the guard token.
                    unsafe { me.parent.#f_ident.get_mut(inner_pin.token_mut()) }
                }

                #[inline]
                #f_vis fn #f_cell_ident(self: ::core::pin::Pin<&mut Self>) -> ::ksync::KOnceCellGuard<'_, #inner_ty, #class_type> {
                    let me = unsafe { self.get_unchecked_mut() };
                    let inner_pin = unsafe { ::core::pin::Pin::new_unchecked(&mut me.inner) };
                    // SAFETY: We hold an exclusive mutable reference to the guard token.
                    unsafe { me.parent.#f_ident.guard(inner_pin.token_mut()) }
                }
            });

            code.write_fields_decl.extend(quote! {
                #[allow(dead_code)]
                #f_vis #f_ident: ::core::option::Option<&'b mut #inner_ty>,
            });

            code.write_fields_init.extend(quote! {
                // SAFETY: We hold exclusive access to the guard token and each field cell
                // is disjoint.
                #f_ident: unsafe { me.parent.#f_ident.get_mut_unchecked() },
            });
        } else if f.project_as_pin {
            code.write_token_guard_accessors.extend(quote! {
                #[inline]
                #f_vis fn #f_ident(&self) -> &#f_ty {
                    // SAFETY: The lock token proves that the lock protecting this cell is held.
                    unsafe { self.parent.#f_ident.get(&*self.token) }
                }

                #[inline]
                #f_vis fn #f_mut_ident(&mut self) -> ::core::pin::Pin<&mut #f_ty> {
                    // SAFETY: We hold an exclusive mutable reference to the guard token,
                    // and the parent struct is pinned so structurally pinned fields remain pinned.
                    unsafe { ::core::pin::Pin::new_unchecked(self.parent.#f_ident.get_mut(&mut *self.token)) }
                }
            });

            code.write_guard_accessors.extend(quote! {
                #[inline]
                #f_vis fn #f_ident(&self) -> &#f_ty {
                    // SAFETY: The lock token proves that the lock protecting this cell is held.
                    unsafe { self.parent.#f_ident.get(self.inner.token()) }
                }

                #[inline]
                #f_vis fn #f_mut_ident(self: ::core::pin::Pin<&mut Self>) -> ::core::pin::Pin<&mut #f_ty> {
                    // SAFETY: Safe projection to obtain pinned reference to self
                    // without moving fields.
                    let me = unsafe { self.get_unchecked_mut() };
                    // SAFETY: `inner` is structurally pinned inside the pinned guard `self`.
                    let inner_pin = unsafe { ::core::pin::Pin::new_unchecked(&mut me.inner) };
                    // SAFETY: We hold an exclusive mutable reference to the guard token,
                    // and the parent is pinned.
                    unsafe { ::core::pin::Pin::new_unchecked(me.parent.#f_ident.get_mut(inner_pin.token_mut())) }
                }
            });

            code.write_fields_decl.extend(quote! {
                #[allow(dead_code)]
                #f_vis #f_ident: ::core::pin::Pin<&'b mut #f_ty>,
            });

            code.write_fields_init.extend(quote! {
                // SAFETY: We hold exclusive access to the guard token, each field cell
                // is disjoint, and the parent struct is pinned so structurally pinned
                // fields remain pinned.
                #f_ident: unsafe { ::core::pin::Pin::new_unchecked(&mut *me.parent.#f_ident.as_mut_ptr(token)) },
            });
        } else {
            code.write_token_guard_accessors.extend(quote! {
                #[inline]
                #f_vis fn #f_ident(&self) -> &#f_ty {
                    // SAFETY: The lock token proves that the lock protecting this cell is held.
                    unsafe { self.parent.#f_ident.get(&*self.token) }
                }

                #[inline]
                #f_vis fn #f_mut_ident(&mut self) -> &mut #f_ty {
                    // SAFETY: We hold an exclusive mutable reference to the guard token.
                    unsafe { self.parent.#f_ident.get_mut(&mut *self.token) }
                }
            });

            code.write_guard_accessors.extend(quote! {
                #[inline]
                #f_vis fn #f_ident(&self) -> &#f_ty {
                    // SAFETY: The lock token proves that the lock protecting this cell is held.
                    unsafe { self.parent.#f_ident.get(self.inner.token()) }
                }

                #[inline]
                #f_vis fn #f_mut_ident(self: ::core::pin::Pin<&mut Self>) -> &mut #f_ty {
                    // SAFETY: Safe projection to obtain unpinned reference to self
                    // without moving fields.
                    let me = unsafe { self.get_unchecked_mut() };
                    // SAFETY: `inner` is structurally pinned inside the pinned guard `self`.
                    let inner_pin = unsafe { ::core::pin::Pin::new_unchecked(&mut me.inner) };
                    // SAFETY: We hold an exclusive mutable reference to the guard token.
                    unsafe { me.parent.#f_ident.get_mut(inner_pin.token_mut()) }
                }
            });

            code.write_fields_decl.extend(quote! {
                #[allow(dead_code)]
                #f_vis #f_ident: &'b mut #f_ty,
            });

            code.write_fields_init.extend(quote! {
                // SAFETY: We hold exclusive access to the guard token and each field cell
                // is disjoint.
                #f_ident: unsafe { &mut *me.parent.#f_ident.as_mut_ptr(token) },
            });
        }
    }

    code
}

fn generate_lock_class_registration(
    struct_ident: &Ident,
    lock_ident: &Ident,
    mu_camel: &str,
    class_ident: &Ident,
    class_impl_generics: &proc_macro2::TokenStream,
    class_ty_generics: &proc_macro2::TokenStream,
    where_clause: Option<&syn::WhereClause>,
    reg_init: proc_macro2::TokenStream,
) -> proc_macro2::TokenStream {
    let struct_upper = struct_ident.to_string().to_ascii_uppercase();
    let mu_upper = mu_camel.to_ascii_uppercase();
    let reg_ident = format_ident!("{}_{}_REGISTRATION", struct_upper, mu_upper);
    let string_reg_ident = format_ident!("{}_{}_STRING_REG", struct_upper, mu_upper);
    let path_name = format!("{}::{}", struct_ident, lock_ident);

    quote! {
        ::ksync::declare_interned_string!(#string_reg_ident, #path_name);

        #[unsafe(link_section = "rust_lock_classes")]
        #[used]
        static #reg_ident: ::ksync::LockClassRegistration = #reg_init;

        impl #class_impl_generics ::ksync::LockClass for #class_ident #class_ty_generics #where_clause {
            const ID: *mut ::core::ffi::c_void = #reg_ident.get();
        }
    }
}

fn generate_fields_struct(
    struct_vis: &syn::Visibility,
    fields_ident: &Ident,
    fields_decl_generics: &proc_macro2::TokenStream,
    where_clause: Option<&syn::WhereClause>,
    fields_decl: &proc_macro2::TokenStream,
    phantom_ty_params: &[&Ident],
) -> proc_macro2::TokenStream {
    quote! {
        #[allow(dead_code)]
        #struct_vis struct #fields_ident #fields_decl_generics #where_clause {
            #fields_decl
            _marker: ::core::marker::PhantomData<(&'b (), #(#phantom_ty_params),*)>,
        }
    }
}

fn generate_token_guard(
    struct_vis: &syn::Visibility,
    struct_ident: &Ident,
    ty_generics: &impl quote::ToTokens,
    where_clause: Option<&syn::WhereClause>,
    class_type: &proc_macro2::TokenStream,
    token_guard_ident: &Ident,
    fields_ident: &Ident,
    token_guard_decl_generics: &proc_macro2::TokenStream,
    token_guard_impl_generics: &proc_macro2::TokenStream,
    token_guard_ty_generics: &proc_macro2::TokenStream,
    fields_ret_ty_generics: &proc_macro2::TokenStream,
    accessors: &proc_macro2::TokenStream,
    fields_init: &proc_macro2::TokenStream,
) -> proc_macro2::TokenStream {
    quote! {
        #[allow(dead_code)]
        #struct_vis struct #token_guard_ident #token_guard_decl_generics #where_clause {
            parent: &'b #struct_ident #ty_generics,
            token: &'b ::ksync::LockToken<'a, #class_type>,
        }

        impl #token_guard_impl_generics #token_guard_ident #token_guard_ty_generics #where_clause {
            #accessors

            #[inline]
            #struct_vis fn token(&self) -> &::ksync::LockToken<'a, #class_type> {
                self.token
            }

            #[inline]
            #struct_vis fn fields<'f>(&'f self) -> #fields_ident #fields_ret_ty_generics {
                let me = self;
                let token = me.token;
                #fields_ident {
                    #fields_init
                    _marker: ::core::marker::PhantomData,
                }
            }
        }
    }
}

fn generate_token_guard_mut(
    struct_vis: &syn::Visibility,
    struct_ident: &Ident,
    ty_generics: &impl quote::ToTokens,
    where_clause: Option<&syn::WhereClause>,
    class_type: &proc_macro2::TokenStream,
    token_guard_mut_ident: &Ident,
    read_fields_ident: &Ident,
    write_fields_ident: &Ident,
    token_guard_decl_generics: &proc_macro2::TokenStream,
    token_guard_impl_generics: &proc_macro2::TokenStream,
    token_guard_ty_generics: &proc_macro2::TokenStream,
    fields_ret_ty_generics: &proc_macro2::TokenStream,
    accessors: &proc_macro2::TokenStream,
    read_fields_init: &proc_macro2::TokenStream,
    write_fields_init: &proc_macro2::TokenStream,
) -> proc_macro2::TokenStream {
    quote! {
        #[allow(dead_code)]
        #struct_vis struct #token_guard_mut_ident #token_guard_decl_generics #where_clause {
            parent: &'b #struct_ident #ty_generics,
            token: &'b mut ::ksync::LockToken<'a, #class_type>,
        }

        impl #token_guard_impl_generics #token_guard_mut_ident #token_guard_ty_generics #where_clause {
            #accessors

            #[inline]
            #struct_vis fn token(&self) -> &::ksync::LockToken<'a, #class_type> {
                &*self.token
            }

            #[inline]
            #struct_vis fn token_mut(&mut self) -> &mut ::ksync::LockToken<'a, #class_type> {
                &mut *self.token
            }

            #[inline]
            #struct_vis fn fields<'f>(&'f self) -> #read_fields_ident #fields_ret_ty_generics {
                let me = self;
                let token = &*me.token;
                #read_fields_ident {
                    #read_fields_init
                    _marker: ::core::marker::PhantomData,
                }
            }

            #[inline]
            #struct_vis fn fields_mut<'f>(&'f mut self) -> #write_fields_ident #fields_ret_ty_generics {
                let me = self;
                let token = &mut *me.token;
                #write_fields_ident {
                    #write_fields_init
                    _marker: ::core::marker::PhantomData,
                }
            }
        }
    }
}

fn generate_pinned_guard_struct(
    struct_vis: &syn::Visibility,
    struct_ident: &Ident,
    guard_ident: &Ident,
    guard_decl_generics: &proc_macro2::TokenStream,
    guard_impl_generics: &proc_macro2::TokenStream,
    guard_ty_generics: &proc_macro2::TokenStream,
    ty_generics: &impl quote::ToTokens,
    where_clause: Option<&syn::WhereClause>,
    inner_type: &proc_macro2::TokenStream,
) -> proc_macro2::TokenStream {
    quote! {
        #[pin_init::pin_data(PinnedDrop)]
        #struct_vis struct #guard_ident #guard_decl_generics #where_clause {
            parent: &'a #struct_ident #ty_generics,
            #[pin]
            inner: #inner_type,
        }

        #[pin_init::pinned_drop]
        impl #guard_impl_generics pin_init::PinnedDrop for #guard_ident #guard_ty_generics #where_clause {
            fn drop(self: ::core::pin::Pin<&mut Self>) {}
        }
    }
}

#[proc_macro_attribute]
pub fn guarded(_args: TokenStream, input: TokenStream) -> TokenStream {
    let mut input_struct = parse_macro_input!(input as ItemStruct);

    let mut mutex_fields = Vec::new();
    let mut brwlock_fields = Vec::new();
    let mut guarded_fields = Vec::new();
    let mut errors = Vec::new();

    if let Fields::Named(ref mut fields) = input_struct.fields {
        for field in fields.named.iter_mut() {
            let analysis = parse_field_attributes(field, &mut errors);

            if analysis.is_unpinned && !is_wavltree_type(&field.ty) {
                errors.push(syn::Error::new(
                    field.ty.span(),
                    "The #[ksync(unpinned)] attribute is only allowed on WavlTree fields",
                ));
            }

            if analysis.is_mutex {
                if !is_kmutex_type(&field.ty) {
                    errors.push(syn::Error::new(
                        field.ty.span(),
                        "Mutex field must be of type KMutex",
                    ));
                }
                let mut mutex_type = quote! { ::ksync::RawMutex };
                match extract_lock_type(&field.ty) {
                    Ok(Some(ty)) => mutex_type = ty,
                    Ok(None) => {}
                    Err(e) => errors.push(e),
                }
                field.attrs.push(syn::parse_quote!(#[pin]));
                mutex_fields.push((
                    field.clone(),
                    mutex_type,
                    analysis.custom_class,
                    analysis.custom_flags,
                ));
            } else if analysis.is_brwlock {
                if !is_brwlock_type(&field.ty) {
                    errors.push(syn::Error::new(
                        field.ty.span(),
                        "Brwlock field must be of type BrwLockPi",
                    ));
                }
                field.attrs.push(syn::parse_quote!(#[pin]));
                brwlock_fields.push((field.clone(), analysis.custom_flags));
            } else if let Some(mutex_ident) = analysis.guarded_by {
                let konce_cell_inner_ty = if is_konce_cell_type(&field.ty) {
                    match extract_konce_cell_inner_type(&field.ty) {
                        Some(ty) => Some(ty),
                        None => {
                            errors.push(syn::Error::new(
                                field.ty.span(),
                                "KOnceCell field must specify its value type, e.g., KOnceCell<T>",
                            ));
                            None
                        }
                    }
                } else {
                    None
                };
                let is_konce_cell = konce_cell_inner_ty.is_some();

                if is_konce_cell && analysis.is_pinned {
                    errors.push(syn::Error::new(
                        field.ident.as_ref().unwrap().span(),
                        "KOnceCell fields cannot be structurally pinned with #[pin]",
                    ));
                }

                guarded_fields.push(GuardedField {
                    ident: field.ident.clone().unwrap(),
                    mutex_ident,
                    ty: field.ty.clone(),
                    vis: field.vis.clone(),
                    project_as_pin: analysis.is_pinned && !analysis.is_unpinned,
                    is_konce_cell,
                    konce_cell_inner_ty,
                });
            }
        }
    }

    if !errors.is_empty() {
        let compile_errors = errors.iter().map(|e| e.to_compile_error());
        return quote! { #(#compile_errors)* }.into();
    }

    // Automatically apply #[::pin_init::pin_data] if not already present
    let has_pin_data = input_struct
        .attrs
        .iter()
        .any(|attr| attr.path().segments.iter().any(|seg| seg.ident == "pin_data"));
    if !has_pin_data {
        input_struct.attrs.push(syn::parse_quote!(#[::pin_init::pin_data]));
    }

    let struct_ident = &input_struct.ident;
    let struct_vis = &input_struct.vis;

    let generics_info = extract_generics_info(&input_struct.generics);

    let mut mutex_fields_processed = Vec::new();
    let mut brwlock_fields_processed = Vec::new();
    let mut generated_names = std::collections::HashSet::new();

    for (field, mutex_type, custom_class, custom_flags) in mutex_fields {
        let field_ident = field.ident.clone().unwrap();
        let mu_camel = to_camel_case(&field_ident.to_string());
        let guard_name = format!("{struct_ident}{mu_camel}Guard");

        check_unique_name(
            &guard_name,
            field_ident.span(),
            &field_ident,
            "guard",
            &mut generated_names,
            &mut errors,
        );

        let (class_ident, class_type) = match custom_class {
            Some(ref ty) => {
                let ident = if let syn::Type::Path(syn::TypePath { path, .. }) = ty {
                    path.segments
                        .last()
                        .map(|s| s.ident.clone())
                        .unwrap_or_else(|| format_ident!("CustomClass"))
                } else {
                    format_ident!("CustomClass")
                };
                (ident, quote! { #ty })
            }
            None => {
                let class_name = format!("{struct_ident}{mu_camel}Class");
                check_unique_name(
                    &class_name,
                    field_ident.span(),
                    &field_ident,
                    "class",
                    &mut generated_names,
                    &mut errors,
                );
                let ident = format_ident!("{class_name}");
                let ty = if generics_info.class_ty_params.is_empty() {
                    quote! { #ident }
                } else {
                    let class_ty_params = &generics_info.class_ty_params;
                    quote! { #ident <#(#class_ty_params),*> }
                };
                (ident, ty)
            }
        };

        let is_phantom = is_phantom_mutex_type(&field.ty);

        mutex_fields_processed.push(MutexField {
            ident: field_ident,
            class_ident,
            class_type,
            mutex_type,
            custom_class,
            custom_flags,
            is_phantom,
        });
    }

    for (field, custom_flags) in brwlock_fields {
        let field_ident = field.ident.unwrap();
        let mu_camel = to_camel_case(&field_ident.to_string());

        let class_name = format!("{struct_ident}{mu_camel}Class");
        let read_guard_name = format!("{struct_ident}{mu_camel}ReadGuard");
        let write_guard_name = format!("{struct_ident}{mu_camel}WriteGuard");

        check_unique_name(
            &class_name,
            field_ident.span(),
            &field_ident,
            "class",
            &mut generated_names,
            &mut errors,
        );
        check_unique_name(
            &read_guard_name,
            field_ident.span(),
            &field_ident,
            "guard",
            &mut generated_names,
            &mut errors,
        );
        check_unique_name(
            &write_guard_name,
            field_ident.span(),
            &field_ident,
            "guard",
            &mut generated_names,
            &mut errors,
        );

        let class_ident = format_ident!("{class_name}");
        let class_type = if generics_info.class_ty_params.is_empty() {
            quote! { #class_ident }
        } else {
            let class_ty_params = &generics_info.class_ty_params;
            quote! { #class_ident <#(#class_ty_params),*> }
        };

        brwlock_fields_processed.push(MutexField {
            ident: field_ident,
            class_ident,
            class_type,
            mutex_type: quote! { ::ksync::RawBrwLockPi },
            custom_class: None,
            custom_flags,
            is_phantom: false,
        });
    }

    if !errors.is_empty() {
        let compile_errors = errors.iter().map(|e| e.to_compile_error());
        return quote! { #(#compile_errors)* }.into();
    }

    // Rewrite fields in struct to KMutex / BrwLockPi and KCell
    if let Fields::Named(ref mut fields) = input_struct.fields {
        for field in fields.named.iter_mut() {
            let field_ident = field.ident.as_ref().unwrap();

            if let Some(mutex_field) =
                mutex_fields_processed.iter().find(|m| m.ident == *field_ident)
            {
                let class_type = &mutex_field.class_type;
                let mutex_type = &mutex_field.mutex_type;
                if let Type::Path(ref mut type_path) = field.ty {
                    if let Some(last_segment) = type_path.path.segments.last_mut() {
                        last_segment.ident = format_ident!("KMutex");
                        last_segment.arguments = syn::PathArguments::AngleBracketed(
                            syn::parse2(quote! { <#class_type, #mutex_type> }).unwrap(),
                        );
                    }
                }
            } else if let Some(brwlock_field) =
                brwlock_fields_processed.iter().find(|m| m.ident == *field_ident)
            {
                let class_type = &brwlock_field.class_type;
                if let Type::Path(ref mut type_path) = field.ty {
                    if let Some(last_segment) = type_path.path.segments.last_mut() {
                        last_segment.ident = format_ident!("BrwLockPi");
                        last_segment.arguments = syn::PathArguments::AngleBracketed(
                            syn::parse2(quote! { <#class_type> }).unwrap(),
                        );
                    }
                }
            } else if let Some(guarded_field) =
                guarded_fields.iter().find(|f| f.ident == *field_ident)
            {
                let class_type_opt = mutex_fields_processed
                    .iter()
                    .chain(brwlock_fields_processed.iter())
                    .find(|m| m.ident == guarded_field.mutex_ident)
                    .map(|m| &m.class_type);

                if let Some(class_type) = class_type_opt {
                    if guarded_field.is_konce_cell {
                        let inner_ty = guarded_field.konce_cell_inner_ty.as_ref().unwrap();
                        field.ty =
                            syn::parse2(quote! { ::ksync::KOnceCell<#inner_ty, #class_type> })
                                .unwrap();
                    } else {
                        let original_ty = &field.ty;
                        field.ty =
                            syn::parse2(quote! { ::ksync::KCell<#original_ty, #class_type> })
                                .unwrap();
                    }
                    field.attrs.push(syn::parse_quote!(#[allow(dead_code)]));
                } else {
                    errors.push(syn::Error::new(
                        guarded_field.mutex_ident.span(),
                        format!(
                            "Guarded field '{}' references non-existent lock '{}'",
                            field_ident, guarded_field.mutex_ident
                        ),
                    ));
                }
            }
        }
    }

    if !errors.is_empty() {
        let compile_errors = errors.iter().map(|e| e.to_compile_error());
        return quote! { #(#compile_errors)* }.into();
    }

    let (impl_generics, ty_generics, where_clause) = input_struct.generics.split_for_impl();

    let params_with_bounds = &generics_info.params_no_defaults;
    let ty_params = &generics_info.ty_params;
    let phantom_ty_params = &generics_info.phantom_ty_params;
    let phantom_lifetimes = &generics_info.phantom_lifetimes;

    let fields_decl_generics = quote! { <'b, #params_with_bounds> };
    let fields_ty_generics = quote! { <'b, #(#ty_params),*> };
    let fields_ret_ty_generics = quote! { <'f, #(#ty_params),*> };
    let token_guard_decl_generics = quote! { <'b, 'a, #params_with_bounds> };
    let token_guard_impl_generics = quote! { <'b, 'a, #params_with_bounds> };
    let token_guard_ty_generics = quote! { <'b, 'a, #(#ty_params),*> };

    let (class_decl_generics, class_impl_generics, class_ty_generics) =
        if generics_info.class_ty_params.is_empty() {
            (quote! {}, quote! {}, quote! {})
        } else {
            let class_params_no_defaults = &generics_info.class_params_no_defaults;
            let class_ty_params = &generics_info.class_ty_params;
            (
                quote! { <#class_params_no_defaults> },
                quote! { <#class_params_no_defaults> },
                quote! { <#(#class_ty_params),*> },
            )
        };

    let marker_phantom = quote! {
        ::core::marker::PhantomData<(
            #(& #phantom_lifetimes (),)*
            fn() -> (*const (#(#phantom_ty_params),*)),
        )>
    };

    // Marker structs for custom classes
    let mut marker_structs = quote! {};
    for lock in mutex_fields_processed.iter().chain(brwlock_fields_processed.iter()) {
        if lock.custom_class.is_none() {
            let class_ident = &lock.class_ident;
            if input_struct.generics.params.is_empty() {
                marker_structs.extend(quote! {
                    #[allow(non_camel_case_types)]
                    #[derive(Default, Debug, Clone, Copy)]
                    #struct_vis struct #class_ident;
                });
            } else {
                marker_structs.extend(quote! {
                    #[allow(non_camel_case_types)]
                    #[derive(Default, Debug, Clone, Copy)]
                    #struct_vis struct #class_ident #class_decl_generics (
                        pub #marker_phantom
                    );
                });
            }
        }
    }

    let mut generated_code = quote! {};

    for mutex in &mutex_fields_processed {
        let mu_ident = &mutex.ident;
        let class_ident = &mutex.class_ident;
        let class_type = &mutex.class_type;
        let mutex_type = &mutex.mutex_type;

        let mu_camel = to_camel_case(&mu_ident.to_string());
        let guard_ident = format_ident!("{struct_ident}{mu_camel}Guard");
        let fields_ident = format_ident!("{struct_ident}{mu_camel}Fields");
        let fields_mut_ident = format_ident!("{struct_ident}{mu_camel}FieldsMut");
        let token_guard_ident = format_ident!("{struct_ident}{mu_camel}TokenGuard");
        let token_guard_mut_ident = format_ident!("{struct_ident}{mu_camel}TokenGuardMut");

        let lock_method_ident = format_ident!("lock_{mu_ident}", span = mu_ident.span());
        let lock_policy_method_ident =
            format_ident!("lock_{mu_ident}_policy", span = mu_ident.span());
        let guard_method_ident = format_ident!("guard_{mu_ident}", span = mu_ident.span());
        let guard_mut_method_ident = format_ident!("guard_{mu_ident}_mut", span = mu_ident.span());

        let this_guarded_fields: Vec<&GuardedField> =
            guarded_fields.iter().filter(|f| f.mutex_ident == *mu_ident).collect();

        let field_code = generate_field_code(&this_guarded_fields, class_type);

        let fields_struct = generate_fields_struct(
            struct_vis,
            &fields_ident,
            &fields_decl_generics,
            where_clause,
            &field_code.read_fields_decl,
            phantom_ty_params,
        );

        let fields_mut_struct = generate_fields_struct(
            struct_vis,
            &fields_mut_ident,
            &fields_decl_generics,
            where_clause,
            &field_code.write_fields_decl,
            phantom_ty_params,
        );

        let class_registration_code = if mutex.custom_class.is_none() {
            let flags_expr = match mutex.custom_flags.as_ref() {
                Some(flags) => {
                    quote! { (<#mutex_type as ::ksync::RawLock>::LOCK_FLAGS | (#flags)) }
                }
                None => quote! { <#mutex_type as ::ksync::RawLock>::LOCK_FLAGS },
            };
            let string_reg_ident = format_ident!(
                "{}_{}_STRING_REG",
                struct_ident.to_string().to_ascii_uppercase(),
                mu_camel.to_ascii_uppercase()
            );
            generate_lock_class_registration(
                struct_ident,
                mu_ident,
                &mu_camel,
                class_ident,
                &class_impl_generics,
                &class_ty_generics,
                where_clause,
                quote! { ::ksync::LockClassRegistration::with_flags(#string_reg_ident, #flags_expr) },
            )
        } else {
            quote! {}
        };

        let guard_struct_generics = if mutex.is_phantom {
            let mut params = params_with_bounds.clone();
            params.push(syn::parse_quote!(M: ::ksync::RawLock = ::ksync::RawMutex));
            quote! { <'a, #params> }
        } else {
            let mut params = params_with_bounds.clone();
            params.push(syn::parse_quote!(P: ::ksync::LockPolicy<#mutex_type> = <#mutex_type as ::ksync::RawLock>::DefaultPolicy));
            quote! { <'a, #params>  }
        };
        let guard_impl_generics = if mutex.is_phantom {
            let mut params = params_with_bounds.clone();
            params.push(syn::parse_quote!(M: ::ksync::RawLock));
            quote! { <'a, #params> }
        } else {
            let mut params = params_with_bounds.clone();
            params.push(syn::parse_quote!(P: ::ksync::LockPolicy<#mutex_type>));
            quote! { <'a, #params> }
        };
        let guard_ty_generics = if mutex.is_phantom {
            let mut args = ty_params.clone();
            args.push(quote! { M });
            quote! { <'a, #(#args),*> }
        } else {
            let mut args = ty_params.clone();
            args.push(quote! { P });
            quote! { <'a, #(#args),*> }
        };
        let guard_inner_type = if mutex.is_phantom {
            quote! { ::ksync::KMutexGuard<'a, #class_type, M> }
        } else {
            quote! { ::ksync::KMutexGuard<'a, #class_type, #mutex_type, P> }
        };

        let lock_aliased_method_ident =
            format_ident!("lock_{mu_ident}_aliased", span = mu_ident.span());
        let lock_aliased_policy_method_ident =
            format_ident!("lock_{mu_ident}_aliased_policy", span = mu_ident.span());

        let lock_method_def = if mutex.is_phantom {
            quote! {
                #[inline]
                #struct_vis fn #lock_method_ident<'a, M: ::ksync::RawLock>(
                    &'a self,
                    real_mutex: &'a ::ksync::KMutex<#class_type, M>,
                ) -> impl pin_init::PinInit<#guard_ident #guard_ty_generics, ::core::convert::Infallible>
                where
                    <M as ::ksync::RawLock>::DefaultPolicy: ::ksync::LockPolicy<M, AcquireArgs = ()>,
                {
                    pin_init::pin_init!(#guard_ident {
                        parent: self,
                        inner <- ::ksync::KMutexGuard::new(real_mutex),
                    })
                }
            }
        } else if is_monitored_spinlock_type(mutex_type) {
            let return_ty_generics = quote! { <'_, #(#ty_params),*> };
            let mut policy_args = ty_params.clone();
            policy_args.push(quote! { P });
            let policy_return_ty_generics = quote! { <'_, #(#policy_args),*> };
            quote! {
                #[inline]
                #struct_vis fn #lock_method_ident(
                    &self,
                    tag: ::ksync::SourceTag,
                ) -> impl pin_init::PinInit<#guard_ident #return_ty_generics, ::core::convert::Infallible> {
                    pin_init::pin_init!(#guard_ident {
                        parent: self,
                        inner <- ::ksync::KMutexGuard::new_with_args(&self.#mu_ident, tag),
                    })
                }
                #[inline]
                #struct_vis fn #lock_policy_method_ident<P: ::ksync::LockPolicy<#mutex_type>>(
                    &self,
                    args: P::AcquireArgs,
                ) -> impl pin_init::PinInit<#guard_ident #policy_return_ty_generics, ::core::convert::Infallible> {
                    pin_init::pin_init!(#guard_ident {
                        parent: self,
                        inner <- ::ksync::KMutexGuard::new_with_args(&self.#mu_ident, args),
                    })
                }
                #[inline]
                #struct_vis fn #lock_aliased_method_ident<
                    'a,
                    AliasClass: ::ksync::LockClass,
                    M2: ::ksync::RawLock,
                >(
                    &'a self,
                    alias: &'a ::ksync::KMutex<AliasClass, M2>,
                    tag: ::ksync::SourceTag,
                ) -> impl pin_init::PinInit<
                    ::ksync::KMutexAliasedGuard<'a, #class_type, AliasClass, #mutex_type, M2>,
                    ::core::convert::Infallible,
                > {
                    ::ksync::KMutexAliasedGuard::new_with_args(&self.#mu_ident, alias, tag)
                }
                #[inline]
                #struct_vis fn #lock_aliased_policy_method_ident<
                    'a,
                    AliasClass: ::ksync::LockClass,
                    M2: ::ksync::RawLock,
                    P: ::ksync::LockPolicy<#mutex_type>,
                >(
                    &'a self,
                    alias: &'a ::ksync::KMutex<AliasClass, M2>,
                    args: P::AcquireArgs,
                ) -> impl pin_init::PinInit<
                    ::ksync::KMutexAliasedGuard<'a, #class_type, AliasClass, #mutex_type, M2, P>,
                    ::core::convert::Infallible,
                > {
                    ::ksync::KMutexAliasedGuard::new_with_args(&self.#mu_ident, alias, args)
                }
            }
        } else {
            let return_ty_generics = quote! { <'_, #(#ty_params),*> };
            let mut policy_args = ty_params.clone();
            policy_args.push(quote! { P });
            let policy_return_ty_generics = quote! { <'_, #(#policy_args),*> };
            quote! {
                #[inline]
                #struct_vis fn #lock_method_ident(&self) -> impl pin_init::PinInit<#guard_ident #return_ty_generics, ::core::convert::Infallible> {
                    pin_init::pin_init!(#guard_ident {
                        parent: self,
                        inner <- ::ksync::KMutexGuard::new(&self.#mu_ident),
                    })
                }
                #[inline]
                #struct_vis fn #lock_policy_method_ident<P: ::ksync::LockPolicy<#mutex_type, AcquireArgs = ()>>(&self)
                    -> impl pin_init::PinInit<#guard_ident #policy_return_ty_generics, ::core::convert::Infallible> {
                    pin_init::pin_init!(#guard_ident {
                        parent: self,
                        inner <- ::ksync::KMutexGuard::new(&self.#mu_ident),
                    })
                }
                #[inline]
                #struct_vis fn #lock_aliased_method_ident<
                    'a,
                    AliasClass: ::ksync::LockClass,
                    M2: ::ksync::RawLock,
                >(
                    &'a self,
                    alias: &'a ::ksync::KMutex<AliasClass, M2>,
                ) -> impl pin_init::PinInit<
                    ::ksync::KMutexAliasedGuard<'a, #class_type, AliasClass, #mutex_type, M2>,
                    ::core::convert::Infallible,
                > {
                    ::ksync::KMutexAliasedGuard::new(&self.#mu_ident, alias)
                }
                #[inline]
                #struct_vis fn #lock_aliased_policy_method_ident<
                    'a,
                    AliasClass: ::ksync::LockClass,
                    M2: ::ksync::RawLock,
                    P: ::ksync::LockPolicy<#mutex_type, AcquireArgs = ()>,
                >(
                    &'a self,
                    alias: &'a ::ksync::KMutex<AliasClass, M2>,
                ) -> impl pin_init::PinInit<
                    ::ksync::KMutexAliasedGuard<'a, #class_type, AliasClass, #mutex_type, M2, P>,
                    ::core::convert::Infallible,
                > {
                    ::ksync::KMutexAliasedGuard::new(&self.#mu_ident, alias)
                }
            }
        };

        let guard_method_def = quote! {
            #[inline]
            #struct_vis fn #guard_method_ident<'b, 'a>(
                &'b self,
                token: &'b ::ksync::LockToken<'a, #class_type>,
            ) -> #token_guard_ident #token_guard_ty_generics {
                #token_guard_ident { parent: self, token }
            }

            #[inline]
            #struct_vis fn #guard_mut_method_ident<'b, 'a>(
                &'b self,
                token: &'b mut ::ksync::LockToken<'a, #class_type>,
            ) -> #token_guard_mut_ident #token_guard_ty_generics {
                #token_guard_mut_ident { parent: self, token }
            }
        };

        let guard_accessors = &field_code.write_guard_accessors;
        let fields_init = &field_code.read_fields_init;
        let fields_mut_init = &field_code.write_fields_init;

        let token_guard = generate_token_guard(
            struct_vis,
            struct_ident,
            &ty_generics,
            where_clause,
            class_type,
            &token_guard_ident,
            &fields_ident,
            &token_guard_decl_generics,
            &token_guard_impl_generics,
            &token_guard_ty_generics,
            &fields_ret_ty_generics,
            &field_code.read_token_guard_accessors,
            &field_code.read_fields_init,
        );

        let token_guard_mut = generate_token_guard_mut(
            struct_vis,
            struct_ident,
            &ty_generics,
            where_clause,
            class_type,
            &token_guard_mut_ident,
            &fields_ident,
            &fields_mut_ident,
            &token_guard_decl_generics,
            &token_guard_impl_generics,
            &token_guard_ty_generics,
            &fields_ret_ty_generics,
            &field_code.write_token_guard_accessors,
            &field_code.read_fields_init,
            &field_code.write_fields_init,
        );

        let guard_struct = generate_pinned_guard_struct(
            struct_vis,
            struct_ident,
            &guard_ident,
            &guard_struct_generics,
            &guard_impl_generics,
            &guard_ty_generics,
            &ty_generics,
            where_clause,
            &guard_inner_type,
        );

        generated_code.extend(quote! {
            #fields_struct
            #fields_mut_struct
            #class_registration_code
            #guard_struct

            impl #guard_impl_generics #guard_ident #guard_ty_generics #where_clause {
                #guard_accessors

                #[inline]
                #struct_vis fn token(&self) -> &::ksync::LockToken<'a, #class_type> {
                    self.inner.token()
                }

                #[inline]
                #struct_vis fn token_mut(self: ::core::pin::Pin<&mut Self>) -> &mut ::ksync::LockToken<'a, #class_type> {
                    let me = unsafe { self.get_unchecked_mut() };
                    let inner_pin = unsafe { ::core::pin::Pin::new_unchecked(&mut me.inner) };
                    inner_pin.token_mut()
                }

                #[inline]
                #struct_vis fn call_unlocked<R, F: FnOnce() -> R>(
                    self: ::core::pin::Pin<&mut Self>,
                    f: F,
                ) -> R {
                    let me = unsafe { self.get_unchecked_mut() };
                    let inner_pin = unsafe { ::core::pin::Pin::new_unchecked(&mut me.inner) };
                    inner_pin.call_unlocked(f)
                }

                #[inline]
                #struct_vis fn call_untracked<R, F: FnOnce(&mut ::ksync::LockToken<'a, #class_type>) -> R>(
                    self: ::core::pin::Pin<&mut Self>,
                    f: F,
                ) -> R {
                    let me = unsafe { self.get_unchecked_mut() };
                    let inner_pin = unsafe { ::core::pin::Pin::new_unchecked(&mut me.inner) };
                    inner_pin.call_untracked(f)
                }

                #[inline]
                #struct_vis fn fields<'b>(&'b self) -> #fields_ident #fields_ty_generics {
                    let me = self;
                    let token = me.inner.token();
                    #fields_ident {
                        #fields_init
                        _marker: ::core::marker::PhantomData,
                    }
                }

                #[inline]
                #struct_vis fn fields_mut<'b>(self: ::core::pin::Pin<&'b mut Self>) -> #fields_mut_ident #fields_ty_generics {
                    let me = unsafe { self.get_unchecked_mut() };
                    let inner_pin = unsafe { ::core::pin::Pin::new_unchecked(&mut me.inner) };
                    let token = inner_pin.token_mut();
                    #fields_mut_ident {
                        #fields_mut_init
                        _marker: ::core::marker::PhantomData,
                    }
                }
            }

            #token_guard
            #token_guard_mut

            impl #impl_generics #struct_ident #ty_generics #where_clause {
                #lock_method_def
                #guard_method_def
            }
        });
    }

    for brwlock in &brwlock_fields_processed {
        let lock_ident = &brwlock.ident;
        let class_ident = &brwlock.class_ident;
        let class_type = &brwlock.class_type;

        let mu_camel = to_camel_case(&lock_ident.to_string());
        let read_guard_ident = format_ident!("{struct_ident}{mu_camel}ReadGuard");
        let write_guard_ident = format_ident!("{struct_ident}{mu_camel}WriteGuard");
        let read_fields_ident = format_ident!("{struct_ident}{mu_camel}ReadFields");
        let write_fields_ident = format_ident!("{struct_ident}{mu_camel}WriteFields");

        let read_token_guard_ident = format_ident!("{struct_ident}{mu_camel}ReadTokenGuard");
        let write_token_guard_ident = format_ident!("{struct_ident}{mu_camel}WriteTokenGuard");

        let read_lock_method_ident = format_ident!("read_{lock_ident}", span = lock_ident.span());
        let write_lock_method_ident = format_ident!("write_{lock_ident}", span = lock_ident.span());
        let guard_read_method_ident =
            format_ident!("guard_read_{lock_ident}", span = lock_ident.span());
        let guard_write_method_ident =
            format_ident!("guard_write_{lock_ident}", span = lock_ident.span());

        let this_guarded_fields: Vec<&GuardedField> =
            guarded_fields.iter().filter(|f| f.mutex_ident == *lock_ident).collect();

        let field_code = generate_field_code(&this_guarded_fields, class_type);

        let read_fields_struct = generate_fields_struct(
            struct_vis,
            &read_fields_ident,
            &fields_decl_generics,
            where_clause,
            &field_code.read_fields_decl,
            phantom_ty_params,
        );

        let write_fields_struct = generate_fields_struct(
            struct_vis,
            &write_fields_ident,
            &fields_decl_generics,
            where_clause,
            &field_code.write_fields_decl,
            phantom_ty_params,
        );

        let class_registration_code = if brwlock.custom_class.is_none() {
            let string_reg_ident = format_ident!(
                "{}_{}_STRING_REG",
                struct_ident.to_string().to_ascii_uppercase(),
                mu_camel.to_ascii_uppercase()
            );
            let flags_expr = match brwlock.custom_flags.as_ref() {
                Some(flags) => quote! { (::ksync::RawBrwLockPi::LOCK_FLAGS | (#flags)) },
                None => quote! { ::ksync::RawBrwLockPi::LOCK_FLAGS },
            };
            generate_lock_class_registration(
                struct_ident,
                lock_ident,
                &mu_camel,
                class_ident,
                &class_impl_generics,
                &class_ty_generics,
                where_clause,
                quote! { ::ksync::LockClassRegistration::with_flags(#string_reg_ident, #flags_expr) },
            )
        } else {
            quote! {}
        };

        let read_guard_accessors = &field_code.read_guard_accessors;
        let write_guard_accessors = &field_code.write_guard_accessors;
        let read_fields_init = &field_code.read_fields_init;
        let write_fields_init = &field_code.write_fields_init;

        let guard_decl_generics = quote! { <'a, #params_with_bounds> };
        let guard_ty_generics = quote! { <'a, #(#ty_params),*> };
        let return_ty_generics = quote! { <'_, #(#ty_params),*> };

        let read_token_guard = generate_token_guard(
            struct_vis,
            struct_ident,
            &ty_generics,
            where_clause,
            class_type,
            &read_token_guard_ident,
            &read_fields_ident,
            &token_guard_decl_generics,
            &token_guard_impl_generics,
            &token_guard_ty_generics,
            &fields_ret_ty_generics,
            &field_code.read_token_guard_accessors,
            &field_code.read_fields_init,
        );

        let write_token_guard = generate_token_guard_mut(
            struct_vis,
            struct_ident,
            &ty_generics,
            where_clause,
            class_type,
            &write_token_guard_ident,
            &read_fields_ident,
            &write_fields_ident,
            &token_guard_decl_generics,
            &token_guard_impl_generics,
            &token_guard_ty_generics,
            &fields_ret_ty_generics,
            &field_code.write_token_guard_accessors,
            &field_code.read_fields_init,
            &field_code.write_fields_init,
        );

        let read_guard_inner = quote! { ::ksync::BrwLockPiReadGuard<'a, #class_type> };
        let read_guard_struct = generate_pinned_guard_struct(
            struct_vis,
            struct_ident,
            &read_guard_ident,
            &guard_decl_generics,
            &guard_decl_generics,
            &guard_ty_generics,
            &ty_generics,
            where_clause,
            &read_guard_inner,
        );

        let write_guard_inner = quote! { ::ksync::BrwLockPiWriteGuard<'a, #class_type> };
        let write_guard_struct = generate_pinned_guard_struct(
            struct_vis,
            struct_ident,
            &write_guard_ident,
            &guard_decl_generics,
            &guard_decl_generics,
            &guard_ty_generics,
            &ty_generics,
            where_clause,
            &write_guard_inner,
        );

        generated_code.extend(quote! {
            #read_fields_struct
            #write_fields_struct
            #class_registration_code
            #read_guard_struct

            impl #guard_decl_generics #read_guard_ident #guard_ty_generics #where_clause {
                #read_guard_accessors

                #[inline]
                #struct_vis fn call_unlocked<R, F: FnOnce() -> R>(
                    self: ::core::pin::Pin<&mut Self>,
                    f: F,
                ) -> R {
                    let me = unsafe { self.get_unchecked_mut() };
                    let inner_pin = unsafe { ::core::pin::Pin::new_unchecked(&mut me.inner) };
                    inner_pin.call_unlocked(f)
                }

                #[inline]
                #struct_vis fn fields<'b>(&'b self) -> #read_fields_ident #fields_ty_generics {
                    let me = self;
                    let token = me.inner.token();
                    #read_fields_ident {
                        #read_fields_init
                        _marker: ::core::marker::PhantomData,
                    }
                }
            }

            #write_guard_struct

            impl #guard_decl_generics #write_guard_ident #guard_ty_generics #where_clause {
                #write_guard_accessors

                #[inline]
                #struct_vis fn call_unlocked<R, F: FnOnce() -> R>(
                    self: ::core::pin::Pin<&mut Self>,
                    f: F,
                ) -> R {
                    let me = unsafe { self.get_unchecked_mut() };
                    let inner_pin = unsafe { ::core::pin::Pin::new_unchecked(&mut me.inner) };
                    inner_pin.call_unlocked(f)
                }

                #[inline]
                #struct_vis fn fields<'b>(&'b self) -> #read_fields_ident #fields_ty_generics {
                    let me = self;
                    let token = me.inner.token();
                    #read_fields_ident {
                        #read_fields_init
                        _marker: ::core::marker::PhantomData,
                    }
                }

                #[inline]
                #struct_vis fn fields_mut<'b>(self: ::core::pin::Pin<&'b mut Self>) -> #write_fields_ident #fields_ty_generics {
                    let me = unsafe { self.get_unchecked_mut() };
                    let inner_pin = unsafe { ::core::pin::Pin::new_unchecked(&mut me.inner) };
                    let token = inner_pin.token_mut();
                    #write_fields_ident {
                        #write_fields_init
                        _marker: ::core::marker::PhantomData,
                    }
                }
            }

            #read_token_guard
            #write_token_guard

            impl #impl_generics #struct_ident #ty_generics #where_clause {
                #[inline]
                #struct_vis fn #read_lock_method_ident(&self) -> impl pin_init::PinInit<#read_guard_ident #return_ty_generics, ::core::convert::Infallible> {
                    pin_init::pin_init!(#read_guard_ident {
                        parent: self,
                        inner <- ::ksync::BrwLockPiReadGuard::new(&self.#lock_ident),
                    })
                }

                #[inline]
                #struct_vis fn #write_lock_method_ident(&self) -> impl pin_init::PinInit<#write_guard_ident #return_ty_generics, ::core::convert::Infallible> {
                    pin_init::pin_init!(#write_guard_ident {
                        parent: self,
                        inner <- ::ksync::BrwLockPiWriteGuard::new(&self.#lock_ident),
                    })
                }

                #[inline]
                #struct_vis fn #guard_read_method_ident<'b, 'a>(
                    &'b self,
                    token: &'b ::ksync::LockToken<'a, #class_type>,
                ) -> #read_token_guard_ident #token_guard_ty_generics {
                    #read_token_guard_ident { parent: self, token }
                }

                #[inline]
                #struct_vis fn #guard_write_method_ident<'b, 'a>(
                    &'b self,
                    token: &'b mut ::ksync::LockToken<'a, #class_type>,
                ) -> #write_token_guard_ident #token_guard_ty_generics {
                    #write_token_guard_ident { parent: self, token }
                }
            }
        });
    }

    let expanded = quote! {
        #marker_structs
        #input_struct
        #generated_code
    };

    TokenStream::from(expanded)
}

fn to_camel_case(s: &str) -> String {
    let mut camel = String::new();
    let mut capitalize = true;
    for c in s.chars() {
        if c == '_' {
            capitalize = true;
        } else if capitalize {
            camel.push(c.to_ascii_uppercase());
            capitalize = false;
        } else {
            camel.push(c);
        }
    }
    camel
}

fn is_type_named(ty: &Type, name: &str) -> bool {
    matches!(ty, Type::Path(TypePath { path, .. }) if path.segments.iter().any(|seg| seg.ident == name))
}

fn is_wavltree_type(ty: &Type) -> bool {
    is_type_named(ty, "WavlTree")
}

fn is_kmutex_type(ty: &Type) -> bool {
    is_type_named(ty, "KMutex")
        || is_type_named(ty, "KSpinlock")
        || is_type_named(ty, "KMonitoredSpinlock")
}

fn is_monitored_spinlock_type(tokens: &proc_macro2::TokenStream) -> bool {
    if let Ok(ty) = syn::parse2::<Type>(tokens.clone()) {
        is_type_named(&ty, "RawMonitoredSpinlock") || is_type_named(&ty, "KMonitoredSpinlock")
    } else {
        false
    }
}

fn is_brwlock_type(ty: &Type) -> bool {
    is_type_named(ty, "BrwLockPi")
}

fn is_konce_cell_type(ty: &Type) -> bool {
    is_type_named(ty, "KOnceCell")
}

fn extract_konce_cell_inner_type(ty: &Type) -> Option<Type> {
    let Type::Path(TypePath { path, .. }) = ty else { return None };
    let seg = path.segments.iter().find(|s| s.ident == "KOnceCell")?;
    let syn::PathArguments::AngleBracketed(args) = &seg.arguments else { return None };
    let syn::GenericArgument::Type(inner_ty) = args.args.first()? else { return None };
    Some(inner_ty.clone())
}

fn is_phantom_mutex_type(ty: &Type) -> bool {
    let Type::Path(TypePath { path, .. }) = ty else { return false };
    let Some(seg) = path.segments.last() else { return false };
    if seg.ident != "KMutex" {
        return false;
    }
    let syn::PathArguments::AngleBracketed(args) = &seg.arguments else { return false };
    let Some(syn::GenericArgument::Type(arg_ty)) = args.args.first() else { return false };
    is_type_named(arg_ty, "PhantomMutex")
}

fn extract_lock_type(ty: &Type) -> Result<Option<proc_macro2::TokenStream>, syn::Error> {
    let Type::Path(type_path) = ty else { return Ok(None) };
    let Some(last_segment) = type_path.path.segments.last() else { return Ok(None) };
    if last_segment.ident == "KSpinlock" {
        return Ok(Some(quote! { ::ksync::RawSpinlock }));
    }
    if last_segment.ident == "KMonitoredSpinlock" {
        return Ok(Some(quote! { ::ksync::RawMonitoredSpinlock }));
    }
    if last_segment.ident != "KMutex" {
        return Ok(None);
    }

    match &last_segment.arguments {
        syn::PathArguments::None => Ok(None),
        syn::PathArguments::AngleBracketed(args) => match args.args.len() {
            0 => Ok(None),
            1 => {
                let first_arg = &args.args[0];
                Ok(Some(quote! { #first_arg }))
            }
            _ => Err(syn::Error::new(
                args.span(),
                "KMutex expects at most 1 generic argument for the lock type in struct definition",
            )),
        },
        syn::PathArguments::Parenthesized(args) => Err(syn::Error::new(
            args.span(),
            "KMutex does not support parenthesized generic arguments",
        )),
    }
}

struct SingletonLockInput {
    attrs: Vec<syn::Attribute>,
    vis: syn::Visibility,
    _struct_token: Option<syn::Token![struct]>,
    name: Ident,
    lock_type: Option<Type>,
}

impl syn::parse::Parse for SingletonLockInput {
    fn parse(input: syn::parse::ParseStream<'_>) -> syn::Result<Self> {
        let attrs = input.call(syn::Attribute::parse_outer)?;
        let vis = input.parse::<syn::Visibility>()?;
        let _struct_token = if input.peek(syn::Token![struct]) {
            Some(input.parse::<syn::Token![struct]>()?)
        } else {
            None
        };
        let name = input.parse::<Ident>()?;
        let lock_type = if input.peek(syn::Token![,]) {
            input.parse::<syn::Token![,]>()?;
            Some(input.parse::<Type>()?)
        } else {
            None
        };
        Ok(SingletonLockInput { attrs, vis, _struct_token, name, lock_type })
    }
}

#[proc_macro]
pub fn declare_singleton_lock(input: TokenStream) -> TokenStream {
    let SingletonLockInput { attrs, vis, name, lock_type, .. } =
        parse_macro_input!(input as SingletonLockInput);

    let raw_lock = lock_type.unwrap_or_else(|| syn::parse_quote!(::ksync::RawMutex));
    let name_str = name.to_string();
    let name_upper = name_str.to_ascii_uppercase();
    let string_reg_ident = format_ident!("{}_STRING_REG", name_upper);
    let reg_ident = format_ident!("{}_REGISTRATION", name_upper);

    let lock_methods = if is_monitored_spinlock_type(&quote! { #raw_lock }) {
        quote! {
            /// Acquires the singleton monitored spinlock using the default policy and the given source tag.
            #[inline]
            pub fn lock(
                tag: ::ksync::SourceTag,
            ) -> impl ::ksync::pin_init::PinInit<
                ::ksync::KMutexGuard<'static, #name, #raw_lock>,
                ::core::convert::Infallible,
            > {
                Self::get().lock_with(tag)
            }

            /// Acquires the singleton monitored spinlock using the specified lock policy and arguments.
            #[inline]
            pub fn lock_policy<P: ::ksync::LockPolicy<#raw_lock>>(
                args: P::AcquireArgs,
            ) -> impl ::ksync::pin_init::PinInit<
                ::ksync::KMutexGuard<'static, #name, #raw_lock, P>,
                ::core::convert::Infallible,
            > {
                Self::get().lock_policy_with::<P>(args)
            }
        }
    } else {
        quote! {
            /// Acquires the singleton mutex using the default policy.
            #[inline]
            pub fn lock() -> impl ::ksync::pin_init::PinInit<
                ::ksync::KMutexGuard<'static, #name, #raw_lock>,
                ::core::convert::Infallible,
            > {
                Self::get().lock()
            }

            /// Acquires the singleton mutex using the specified lock policy.
            #[inline]
            pub fn lock_policy<
                P: ::ksync::LockPolicy<#raw_lock, AcquireArgs = ()>,
            >() -> impl ::ksync::pin_init::PinInit<
                ::ksync::KMutexGuard<'static, #name, #raw_lock, P>,
                ::core::convert::Infallible,
            > {
                Self::get().lock_policy::<P>()
            }
        }
    };

    quote! {
        #(#attrs)*
        #[derive(Debug, Copy, Clone)]
        #vis struct #name;

        const _: () = {
            ::ksync::declare_interned_string!(#string_reg_ident, #name_str);

            #[unsafe(link_section = "rust_lock_classes")]
            #[used]
            static #reg_ident: ::ksync::LockClassRegistration =
                ::ksync::LockClassRegistration::with_flags(
                    &#string_reg_ident,
                    ::ksync::LOCK_FLAGS_SINGLETON_LOCK,
                );

            impl ::ksync::LockClass for #name {
                const ID: *mut ::core::ffi::c_void = #reg_ident.get();
            }
        };

        impl #name {
            /// Returns a reference to the global singleton mutex.
            #[inline]
            pub fn Get() -> &'static ::ksync::KMutex<#name, #raw_lock> {
                Self::get()
            }

            /// Returns a reference to the global singleton mutex.
            #[inline]
            pub fn get() -> &'static ::ksync::KMutex<#name, #raw_lock> {
                static SINGLETON: ::ksync::KMutex<#name, #raw_lock> =
                    ::ksync::KMutex::new(<#raw_lock>::const_init(<#name as ::ksync::LockClass>::ID));
                &SINGLETON
            }

            #lock_methods
        }
    }
    .into()
}
