// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{FnArg, ItemImpl, Pat, ReturnType, parse_macro_input};

struct MethodInfo {
    ident: syn::Ident,
    variant_ident: syn::Ident,
    is_mut_self: bool,
    is_async: bool,
    args: Vec<(syn::Ident, syn::Type)>,
    ret_ty: syn::Type,
    doc_and_cfg_attrs: Vec<syn::Attribute>,
    vis: syn::Visibility,
}

#[proc_macro_attribute]
pub fn rpc(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let item_impl = parse_macro_input!(item as ItemImpl);

    let server_ident = match &*item_impl.self_ty {
        syn::Type::Path(p) => {
            if let Some(seg) = p.path.segments.last() {
                seg.ident.clone()
            } else {
                return syn::Error::new_spanned(&item_impl.self_ty, "Expected valid type path")
                    .to_compile_error()
                    .into();
            }
        }
        _ => {
            return syn::Error::new_spanned(&item_impl.self_ty, "Expected struct/enum path type")
                .to_compile_error()
                .into();
        }
    };

    let request_ident = format_ident!("{}Request", server_ident);
    let response_ident = format_ident!("{}Response", server_ident);
    let rpc_ident = format_ident!("{}Rpc", server_ident);
    let client_ident = format_ident!("{}Client", server_ident);

    let (impl_generics, ty_generics, where_clause) = item_impl.generics.split_for_impl();
    let where_predicates = where_clause.map(|w| &w.predicates);

    let mut lifetime_defs = Vec::new();
    let mut lifetime_names = Vec::new();
    let mut type_const_defs = Vec::new();
    let mut type_const_names = Vec::new();

    for p in &item_impl.generics.params {
        match p {
            syn::GenericParam::Lifetime(l) => {
                lifetime_defs.push(quote! { #l });
                let lt = &l.lifetime;
                lifetime_names.push(quote! { #lt });
            }
            syn::GenericParam::Type(t) => {
                type_const_defs.push(quote! { #t });
                let id = &t.ident;
                type_const_names.push(quote! { #id });
            }
            syn::GenericParam::Const(c) => {
                type_const_defs.push(quote! { #c });
                let id = &c.ident;
                type_const_names.push(quote! { #id });
            }
        }
    }

    let phantom_types: Vec<_> = item_impl
        .generics
        .params
        .iter()
        .filter_map(|p| match p {
            syn::GenericParam::Type(t) => {
                let id = &t.ident;
                Some(quote! { #id })
            }
            syn::GenericParam::Lifetime(l) => {
                let lt = &l.lifetime;
                Some(quote! { &#lt () })
            }
            syn::GenericParam::Const(_) => None,
        })
        .collect();

    let has_phantom = !phantom_types.is_empty();

    let mut methods = Vec::new();

    for item in &item_impl.items {
        if let syn::ImplItem::Fn(method) = item {
            let Some(FnArg::Receiver(receiver)) = method.sig.inputs.first() else {
                continue;
            };

            let is_mut_self = receiver.mutability.is_some();
            let is_async = method.sig.asyncness.is_some();
            let ident = method.sig.ident.clone();
            let variant_ident = ident.clone();

            let mut args = Vec::new();
            for (idx, input) in method.sig.inputs.iter().skip(1).enumerate() {
                if let FnArg::Typed(pat_ty) = input {
                    let arg_ident = match &*pat_ty.pat {
                        Pat::Ident(pat_ident) => pat_ident.ident.clone(),
                        _ => syn::Ident::new(&format!("arg{}", idx), method.sig.ident.span()),
                    };
                    args.push((arg_ident, (*pat_ty.ty).clone()));
                }
            }

            let ret_ty = match &method.sig.output {
                ReturnType::Default => syn::parse_quote!(()),
                ReturnType::Type(_, ty) => (**ty).clone(),
            };

            let doc_and_cfg_attrs: Vec<syn::Attribute> = method
                .attrs
                .iter()
                .filter(|attr| attr.path().is_ident("doc") || attr.path().is_ident("cfg"))
                .cloned()
                .collect();

            let vis = method.vis.clone();

            methods.push(MethodInfo {
                ident,
                variant_ident,
                is_mut_self,
                is_async,
                args,
                ret_ty,
                doc_and_cfg_attrs,
                vis,
            });
        }
    }

    let any_mut_self = methods.iter().any(|m| m.is_mut_self);
    let route_self_arg = if any_mut_self { quote!(&mut self) } else { quote!(&self) };

    let req_variants = methods.iter().map(|m| {
        let v_ident = &m.variant_ident;
        let attrs = &m.doc_and_cfg_attrs;
        if m.args.is_empty() {
            quote! { #(#attrs)* #v_ident }
        } else {
            let idents = m.args.iter().map(|(id, _)| id);
            let types = m.args.iter().map(|(_, ty)| ty);
            quote! { #(#attrs)* #v_ident { #(#idents: #types),* } }
        }
    });

    let res_variants = methods.iter().map(|m| {
        let v_ident = &m.variant_ident;
        let attrs = &m.doc_and_cfg_attrs;
        let ret_ty = &m.ret_ty;
        quote! { #(#attrs)* #v_ident(#ret_ty) }
    });

    let client_methods = methods.iter().map(|m| {
        let ident = &m.ident;
        let v_ident = &m.variant_ident;
        let ret_ty = &m.ret_ty;
        let vis = &m.vis;
        let attrs = &m.doc_and_cfg_attrs;
        let arg_idents: Vec<_> = m.args.iter().map(|(id, _)| id).collect();
        let arg_types: Vec<_> = m.args.iter().map(|(_, ty)| ty).collect();

        let req_init = if m.args.is_empty() {
            quote! { #request_ident::#v_ident }
        } else {
            quote! { #request_ident::#v_ident { #(#arg_idents),* } }
        };

        quote! {
            #(#attrs)*
            #[allow(non_snake_case, dead_code)]
            #vis async fn #ident(&self, #(#arg_idents: #arg_types),*) -> ::core::result::Result<#ret_ty, ::sapphire_async::rpc::CallError> {
                let req = #req_init;
                let res = self.client.call(req).await?;
                match res {
                    #response_ident::#v_ident(val) => ::core::result::Result::Ok(val),
                    _ => ::core::unreachable!("RPC response mismatch for {}", ::core::stringify!(#ident)),
                }
            }
        }
    });

    let route_arms = methods.iter().map(|m| {
        let ident = &m.ident;
        let v_ident = &m.variant_ident;
        let arg_idents: Vec<_> = m.args.iter().map(|(id, _)| id).collect();

        let pat = if m.args.is_empty() {
            quote! { #request_ident::#v_ident }
        } else {
            quote! { #request_ident::#v_ident { #(#arg_idents),* } }
        };

        let call_expr = if m.is_async {
            quote! { self.#ident(#(#arg_idents),*).await }
        } else {
            quote! { self.#ident(#(#arg_idents),*) }
        };

        quote! {
            #pat => {
                let res = #call_expr;
                responder.respond(#response_ident::#v_ident(res));
            }
        }
    });

    let (
        req_phantom,
        res_phantom,
        rpc_tuple_field,
        client_phantom_field,
        client_phantom_init,
        wildcard_arm,
    ) = if has_phantom {
        (
            quote! { #[doc(hidden)] _Phantom(::core::marker::PhantomData<(#(#phantom_types),*)>) },
            quote! { #[doc(hidden)] _Phantom(::core::marker::PhantomData<(#(#phantom_types),*)>) },
            quote! { (pub ::core::marker::PhantomData<(#(#phantom_types),*)>) },
            quote! { _phantom: ::core::marker::PhantomData<(#(#phantom_types),*)>, },
            quote! { _phantom: ::core::marker::PhantomData, },
            quote! { _ => ::core::unreachable!("Phantom variant should not be constructed"), },
        )
    } else {
        (quote! {}, quote! {}, quote! {}, quote! {}, quote! {}, quote! {})
    };

    let route_method: syn::ImplItem = if methods.is_empty() {
        syn::parse_quote! {
            async fn route_request<Cfg, C>(
                #route_self_arg,
                request: #request_ident #ty_generics,
                _responder: ::sapphire_async::rpc::Responder<#rpc_ident #ty_generics, Cfg, C>,
            ) where
                Cfg: ::sapphire_async::rpc::RpcCfg,
                C: ::core::ops::Deref<Target = ::sapphire_async::rpc::RpcChannel<#rpc_ident #ty_generics, Cfg>>,
            {
                match request {
                    _ => ::core::unreachable!("No RPC endpoints defined"),
                }
            }
        }
    } else {
        syn::parse_quote! {
            async fn route_request<Cfg, C>(
                #route_self_arg,
                request: #request_ident #ty_generics,
                responder: ::sapphire_async::rpc::Responder<#rpc_ident #ty_generics, Cfg, C>,
            ) where
                Cfg: ::sapphire_async::rpc::RpcCfg,
                C: ::core::ops::Deref<Target = ::sapphire_async::rpc::RpcChannel<#rpc_ident #ty_generics, Cfg>>,
            {
                match request {
                    #(#route_arms)*
                    #wildcard_arm
                }
            }
        }
    };

    let self_ty = &item_impl.self_ty;

    let output = quote! {
        #[allow(non_camel_case_types, non_snake_case, dead_code)]
        pub enum #request_ident #impl_generics #where_clause {
            #(#req_variants,)*
            #req_phantom
        }

        #[allow(non_camel_case_types, non_snake_case, dead_code)]
        pub enum #response_ident #impl_generics #where_clause {
            #(#res_variants,)*
            #res_phantom
        }

        #[derive(::core::fmt::Debug, ::core::clone::Clone, ::core::marker::Copy)]
        pub struct #rpc_ident #impl_generics #rpc_tuple_field #where_clause;

        impl #impl_generics ::sapphire_async::rpc::Rpc for #rpc_ident #ty_generics #where_clause {
            type Request = #request_ident #ty_generics;
            type Response = #response_ident #ty_generics;
        }

        #[derive(::core::fmt::Debug)]
        pub struct #client_ident<#(#lifetime_defs,)* C: ::sapphire_async::rpc::RpcHandles, #(#type_const_defs),*> #where_clause {
            client: ::sapphire_async::rpc::Client<C>,
            #client_phantom_field
        }

        impl<#(#lifetime_defs,)* C: ::sapphire_async::rpc::RpcHandles, #(#type_const_defs),*> #client_ident<#(#lifetime_names,)* C, #(#type_const_names),*> #where_clause {
            pub const fn new(client: ::sapphire_async::rpc::Client<C>) -> Self {
                Self { client, #client_phantom_init }
            }
        }

        impl<#(#lifetime_defs,)* C: ::sapphire_async::rpc::RpcHandles, #(#type_const_defs),*> ::core::convert::From<::sapphire_async::rpc::Client<C>> for #client_ident<#(#lifetime_names,)* C, #(#type_const_names),*> #where_clause {
            fn from(client: ::sapphire_async::rpc::Client<C>) -> Self {
                Self::new(client)
            }
        }

        impl<#(#lifetime_defs,)* C: ::sapphire_async::rpc::RpcHandles + ::core::clone::Clone, #(#type_const_defs),*> ::core::clone::Clone for #client_ident<#(#lifetime_names,)* C, #(#type_const_names),*> #where_clause {
            fn clone(&self) -> Self {
                Self { client: self.client.clone(), #client_phantom_init }
            }
        }

        impl<#(#lifetime_defs,)* C: ::sapphire_async::rpc::RpcHandles, #(#type_const_defs),*> ::core::ops::Deref for #client_ident<#(#lifetime_names,)* C, #(#type_const_names),*> #where_clause {
            type Target = ::sapphire_async::rpc::Client<C>;
            fn deref(&self) -> &Self::Target {
                &self.client
            }
        }

        impl<#(#lifetime_defs,)* C, Cfg, #(#type_const_defs),*> #client_ident<#(#lifetime_names,)* C, #(#type_const_names),*>
        where
            C: ::sapphire_async::rpc::RpcHandles + ::core::ops::Deref<Target = ::sapphire_async::rpc::RpcChannel<#rpc_ident #ty_generics, Cfg>>,
            Cfg: ::sapphire_async::rpc::RpcCfg,
            #where_predicates
        {
            #(#client_methods)*
        }

        #item_impl

        impl #impl_generics #self_ty #where_clause {
            #route_method
        }
    };

    output.into()
}
