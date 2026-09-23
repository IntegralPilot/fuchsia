// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use proc_macro::TokenStream;
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::{FnArg, Ident, ItemFn, Pat, parse_macro_input};

struct Syscall {
    fn_item: ItemFn,
    base_name: Ident,
}

impl Parse for Syscall {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let fn_item: ItemFn = input.parse()?;

        let fn_name = &fn_item.sig.ident;
        let fn_name_str = fn_name.to_string();

        if !fn_name_str.starts_with("sys_") {
            return Err(syn::Error::new_spanned(
                fn_name,
                "#[syscall] function names must start with 'sys_' prefix",
            ));
        }

        let syscall_name_str = fn_name_str.strip_prefix("sys_").unwrap();
        let base_name = Ident::new(syscall_name_str, fn_name.span());

        for arg in &fn_item.sig.inputs {
            match arg {
                FnArg::Typed(pat_type) => match &*pat_type.pat {
                    Pat::Ident(_) => {}
                    _ => {
                        return Err(syn::Error::new_spanned(
                            &pat_type.pat,
                            "#[syscall] only supports identifier patterns for arguments",
                        ));
                    }
                },
                FnArg::Receiver(rec) => {
                    return Err(syn::Error::new_spanned(
                        rec,
                        "#[syscall] functions cannot take `self`",
                    ));
                }
            }
        }

        Ok(Syscall { fn_item, base_name })
    }
}

#[proc_macro_attribute]
pub fn syscall(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let Syscall { fn_item, base_name } = parse_macro_input!(item as Syscall);
    let vis = &fn_item.vis;
    let syn::Signature { inputs, output, .. } = &fn_item.sig;
    let fn_name = &fn_item.sig.ident;
    let block = &fn_item.block;

    quote! {
        #[allow(improper_ctypes_definitions)]
        #[unsafe(no_mangle)]
        #vis extern "C" fn #fn_name(#inputs) #output #block

        const _: crate::syscalls_rs::syscall_signatures::#base_name = #fn_name;
    }
    .into()
}
