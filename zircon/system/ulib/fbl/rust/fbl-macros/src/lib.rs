// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use proc_macro::TokenStream;
use quote::{ToTokens, quote};
use syn::{DeriveInput, Fields, ItemStruct, parse_macro_input};

/// Attribute macro to make a struct reference counted.
///
/// Adds a `ref_count: fbl::RefCounted` field and a `_guard: RefCountedGuard` as the first and
/// second fields. Implements `fbl::HasRefCount` and `fbl::Recyclable` for the struct.
#[proc_macro_attribute]
pub fn ref_counted(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let mut input = parse_macro_input!(item as ItemStruct);
    let name = &input.ident;

    // Check for #[repr(C)]
    let has_repr_c = input.attrs.iter().any(|attr| {
        if !attr.path().is_ident("repr") {
            return false;
        }
        if let Ok(ident) = attr.parse_args::<syn::Ident>() {
            return ident == "C";
        }
        if let Ok(list) = attr.parse_args_with(
            syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated,
        ) {
            return list.iter().any(|meta| {
                if let syn::Meta::Path(path) = meta {
                    return path.is_ident("C");
                }
                false
            });
        }
        false
    });

    if !has_repr_c {
        panic!("Structs using #[ref_counted] must also be marked with #[repr(C)]");
    }

    if let Fields::Named(ref mut fields) = input.fields {
        // Add ref_count field as the first field.
        fields.named.insert(
            0,
            syn::parse_quote! {
                ref_count: ::fbl::RefCounted
            },
        );
        // Add _guard field to prevent manual allocation.
        fields.named.push(syn::parse_quote! {
            __fbl_ref_counted_guard: ()
        });
    } else {
        panic!("ref_counted attribute only supports structs with named fields");
    }

    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    let expanded = quote! {
        #input

        impl #impl_generics ::fbl::HasRefCount for #name #ty_generics #where_clause {
            fn ref_count(&self) -> &::fbl::RefCounted {
                // Compile-time validation that ref_count is at offset 0
                const {
                    assert!(core::mem::offset_of!(Self, ref_count) == 0);
                }
                &self.ref_count
            }
        }
    };

    TokenStream::from(expanded)
}

/// Derive macro to implement `Recyclable` for a struct using `kalloc::Box`.
///
/// This assumes the object was allocated using `kalloc::Box` (e.g., via `UniquePtr::try_new`).
#[proc_macro_derive(Recyclable)]
pub fn derive_recyclable(item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as DeriveInput);
    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    let expanded = quote! {
        unsafe impl #impl_generics ::fbl::Recyclable for #name #ty_generics #where_clause {
            unsafe fn recycle(ptr: ::core::ptr::NonNull<Self>) {
                // SAFETY: The caller of `recycle` must ensure that `ptr` was allocated
                // by a mechanism compatible with `kalloc::Box` (e.g., `UniquePtr::try_new`).
                unsafe {
                    let _ = ::kalloc::Box::from_non_null(ptr);
                }
            }

            fn allocate(value: Self) -> Result<::core::ptr::NonNull<Self>, ::kalloc::AllocError> {
                let boxed = ::kalloc::Box::try_new(value)?;
                let raw = ::kalloc::Box::into_raw(boxed);
                // SAFETY: Box::into_raw returns a valid, non-null pointer.
                unsafe { Ok(::core::ptr::NonNull::new_unchecked(raw)) }
            }
        }

        unsafe impl #impl_generics ::fbl::UninitRecyclable for #name #ty_generics #where_clause {
            unsafe fn recycle_uninit(ptr: ::core::ptr::NonNull<::core::mem::MaybeUninit<Self>>) {
                // SAFETY: The caller of `recycle_uninit` must ensure that `ptr` was allocated
                // by a mechanism compatible with `kalloc::Box`.
                unsafe {
                    let _ = ::kalloc::Box::from_non_null(ptr);
                }
            }

            fn allocate_uninit() -> Result<::core::ptr::NonNull<::core::mem::MaybeUninit<Self>>, ::kalloc::AllocError> {
                let boxed = ::kalloc::Box::try_new_uninit()?;
                let raw = ::kalloc::Box::into_raw(boxed);
                // SAFETY: Box::into_raw returns a valid, non-null pointer.
                unsafe { Ok(::core::ptr::NonNull::new_unchecked(raw)) }
            }
        }
    };

    TokenStream::from(expanded)
}

/// Derive macro to implement `SinglyLinkedListContainable` for a struct.
///
/// Mark fields that are nodes with `#[sll_node]`. To support multiple lists, use
/// `#[sll_node(tag = MyTag)]`.
#[proc_macro_derive(SinglyLinkedListContainable, attributes(sll_node))]
pub fn derive_singly_linked_list_containable(item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as DeriveInput);
    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    let fields = match input.data {
        syn::Data::Struct(s) => s.fields,
        _ => panic!("SinglyLinkedListContainable derive only supports structs"),
    };

    let mut impls = Vec::new();

    for field in fields {
        for attr in &field.attrs {
            if attr.path().is_ident("sll_node") {
                let field_name = field.ident.as_ref().unwrap();
                let mut tag = None;

                let _ = attr.parse_nested_meta(|meta| {
                    if meta.path.is_ident("tag") {
                        let value = meta.value()?;
                        let parsed_tag: syn::Type = value.parse()?;
                        tag = Some(parsed_tag);
                        Ok(())
                    } else {
                        let path_str = meta.path.to_token_stream().to_string();
                        panic!("unsupported attribute: {}", path_str)
                    }
                });

                let tag_type = tag.unwrap_or_else(|| syn::parse_quote! { ::fbl::DefaultObjectTag });

                impls.push(quote! {
                    impl #impl_generics ::fbl::SinglyLinkedListContainable<#name #ty_generics, #tag_type> for #name #ty_generics #where_clause {
                        fn get_node(&self) -> &::fbl::SinglyLinkedListNode<#name #ty_generics> {
                            &self.#field_name
                        }
                    }
                });
            }
        }
    }

    if impls.is_empty() {
        panic!("At least one field must be marked with #[sll_node]");
    }

    let expanded = quote! {
        #(#impls)*
    };

    TokenStream::from(expanded)
}

/// Derive macro to implement `DoublyLinkedListContainable` for a struct.
///
/// Mark fields that are nodes with `#[dll_node]`. To support multiple lists, use
/// `#[dll_node(tag = MyTag)]`.
#[proc_macro_derive(DoublyLinkedListContainable, attributes(dll_node))]
pub fn derive_doubly_linked_list_containable(item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as DeriveInput);
    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    let fields = match input.data {
        syn::Data::Struct(s) => s.fields,
        _ => panic!("DoublyLinkedListContainable derive only supports structs"),
    };

    let mut impls = Vec::new();

    for field in fields {
        for attr in &field.attrs {
            if attr.path().is_ident("dll_node") {
                let field_name = field.ident.as_ref().unwrap();
                let mut tag = None;

                let _ = attr.parse_nested_meta(|meta| {
                    if meta.path.is_ident("tag") {
                        let value = meta.value()?;
                        let parsed_tag: syn::Type = value.parse()?;
                        tag = Some(parsed_tag);
                        Ok(())
                    } else {
                        Err(meta.error("unsupported attribute"))
                    }
                });

                let tag_type = tag.unwrap_or_else(|| syn::parse_quote! { ::fbl::DefaultObjectTag });

                impls.push(quote! {
                    impl #impl_generics ::fbl::DoublyLinkedListContainable<#name #ty_generics, #tag_type> for #name #ty_generics #where_clause {
                        fn get_node(&self) -> &::fbl::DoublyLinkedListNode<#name #ty_generics> {
                            &self.#field_name
                        }
                    }
                });
            }
        }
    }

    if impls.is_empty() {
        panic!("At least one field must be marked with #[dll_node]");
    }

    let expanded = quote! {
        #(#impls)*
    };

    TokenStream::from(expanded)
}

/// Derive macro to implement `WavlTreeContainable` for a struct.
///
/// Mark fields that are nodes with `#[wavl_node]`. To support multiple trees, use
/// `#[wavl_node(tag = MyTag)]`.
#[proc_macro_derive(WavlTreeContainable, attributes(wavl_node))]
pub fn derive_wavl_tree_containable(item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as DeriveInput);
    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    let fields = match input.data {
        syn::Data::Struct(s) => s.fields,
        _ => panic!("WavlTreeContainable derive only supports structs"),
    };

    let mut impls = Vec::new();

    for field in fields {
        for attr in &field.attrs {
            if attr.path().is_ident("wavl_node") {
                let field_name = field.ident.as_ref().unwrap();
                let mut tag = None;
                let mut rank = None;

                let _ = attr.parse_nested_meta(|meta| {
                    if meta.path.is_ident("tag") {
                        let value = meta.value()?;
                        let parsed_tag: syn::Type = value.parse()?;
                        tag = Some(parsed_tag);
                        Ok(())
                    } else if meta.path.is_ident("rank") {
                        let value = meta.value()?;
                        let parsed_rank: syn::Type = value.parse()?;
                        rank = Some(parsed_rank);
                        Ok(())
                    } else {
                        let path_str = meta.path.to_token_stream().to_string();
                        panic!("unsupported attribute: {}", path_str)
                    }
                });

                let tag_type = tag.unwrap_or_else(|| syn::parse_quote! { ::fbl::DefaultObjectTag });
                let rank_type = rank.unwrap_or_else(|| syn::parse_quote! { bool });

                impls.push(quote! {
                    impl #impl_generics ::fbl::WavlTreeContainable<#name #ty_generics, #tag_type> for #name #ty_generics #where_clause {
                        type Rank = #rank_type;
                        fn get_node(&self) -> &::fbl::WavlTreeNode<#name #ty_generics, Self::Rank> {
                            &self.#field_name
                        }
                    }
                });
            }
        }
    }

    if impls.is_empty() {
        panic!("At least one field must be marked with #[wavl_node]");
    }

    let expanded = quote! {
        #(#impls)*
    };

    TokenStream::from(expanded)
}
