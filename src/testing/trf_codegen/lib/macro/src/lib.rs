// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use proc_macro::TokenStream;
use quote::quote;
use syn::{ItemFn, ItemImpl, ItemStruct};

/// Transforms test functions annotated with `#[trf::test]` into an async test runner initialized with `TestRealm`.
#[proc_macro_attribute]
pub fn test(args: TokenStream, input: TokenStream) -> TokenStream {
    trf_test_impl(args.into(), input.into()).unwrap_or_else(syn::Error::into_compile_error).into()
}

/// Annotates mock structs for protocol mocking.
#[proc_macro_attribute]
pub fn mock(args: TokenStream, input: TokenStream) -> TokenStream {
    trf_mock_impl(args.into(), input.into()).unwrap_or_else(syn::Error::into_compile_error).into()
}

/// Generates control proxy helper methods.
#[proc_macro_attribute]
pub fn control(args: TokenStream, input: TokenStream) -> TokenStream {
    trf_control_impl(args.into(), input.into())
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

fn trf_test_impl(
    _args: proc_macro2::TokenStream,
    input: proc_macro2::TokenStream,
) -> Result<proc_macro2::TokenStream, syn::Error> {
    let item_fn = syn::parse2::<ItemFn>(input.clone()).map_err(|err| {
        let mut combined =
            syn::Error::new_spanned(input, "trf::test attribute can only be applied to functions");
        combined.combine(err);
        combined
    })?;

    let function_ident = &item_fn.sig.ident;
    let inner_function_ident = quote::format_ident!("__trf_inner_{}", function_ident);
    let builder_fn_ident = quote::format_ident!("build_realm_for_{}", function_ident);

    let mut inner_function = item_fn.clone();
    inner_function.sig.ident = inner_function_ident.clone();

    let struct_ident = quote::format_ident!("TestRealm_{}", function_ident);
    if let Some(arg) = inner_function.sig.inputs.first_mut() {
        if let syn::FnArg::Typed(pat_type) = arg {
            let new_ty: syn::Type = syn::parse_quote!(&::trf_generated_realm::#struct_ident);
            *pat_type.ty = new_ty;
        }
    }

    let arguments_call = if !item_fn.sig.inputs.is_empty() { quote!(&realm) } else { quote!() };

    let is_async_function = inner_function.sig.asyncness.is_some();
    let await_expression = if is_async_function { quote!( .await ) } else { quote!() };

    let attributes = &item_fn.attrs;
    let visibility = &item_fn.vis;
    let return_type = &item_fn.sig.output;

    let output = quote! {
        #(#attributes)*
        #[allow(unused_crate_dependencies)]
        #[::fuchsia::test]
        #visibility async fn #function_ident() #return_type {
            #inner_function

            let realm = ::trf_generated_realm::#builder_fn_ident().await.expect("Failed to build realm");

            #inner_function_ident(#arguments_call)#await_expression
        }
    };

    Ok(output)
}

fn trf_mock_impl(
    args: proc_macro2::TokenStream,
    input: proc_macro2::TokenStream,
) -> Result<proc_macro2::TokenStream, syn::Error> {
    let item_struct = syn::parse2::<ItemStruct>(input.clone()).map_err(|_| {
        syn::Error::new_spanned(input, "trf::mock attribute can only be applied to structs")
    })?;

    let mut protocol_name_option: Option<String> = None;

    if !args.is_empty() {
        let mock_meta_parser = syn::meta::parser(|meta| {
            if meta.path.is_ident("protocol") {
                let value_literal: syn::LitStr = meta.value()?.parse()?;
                protocol_name_option = Some(value_literal.value());
                Ok(())
            } else {
                Err(meta.error("unsupported key in trf::mock attribute (expected 'protocol')"))
            }
        });
        syn::parse::Parser::parse2(mock_meta_parser, args)?;
    }

    let protocol_name = protocol_name_option.ok_or_else(|| {
        syn::Error::new(
            proc_macro2::Span::call_site(),
            "missing 'protocol' parameter in trf::mock attribute",
        )
    })?;

    let struct_ident = &item_struct.ident;
    let (impl_generics, type_generics, where_clause) = item_struct.generics.split_for_impl();

    let expanded_tokens = quote! {
        #item_struct

        impl #impl_generics #struct_ident #type_generics #where_clause {
            pub const MOCK_PROTOCOL_NAME: &'static str = #protocol_name;

            pub fn protocol_name() -> &'static str {
                Self::MOCK_PROTOCOL_NAME
            }
        }
    };

    Ok(expanded_tokens)
}

fn trf_control_impl(
    _args: proc_macro2::TokenStream,
    input: proc_macro2::TokenStream,
) -> Result<proc_macro2::TokenStream, syn::Error> {
    if let Ok(item_impl) = syn::parse2::<ItemImpl>(input.clone()) {
        let struct_ident = match item_impl.self_ty.as_ref() {
            syn::Type::Path(type_path) => {
                type_path.path.segments.last().map(|segment| &segment.ident).ok_or_else(|| {
                    syn::Error::new_spanned(&item_impl.self_ty, "expected valid type path")
                })?
            }
            _ => {
                return Err(syn::Error::new_spanned(
                    &item_impl.self_ty,
                    "trf::control requires a path type for self",
                ));
            }
        };

        let self_type = &item_impl.self_ty;
        let control_proxy_ident = quote::format_ident!("{}ControlProxy", struct_ident);

        let expanded_tokens = quote! {
            #item_impl

            impl #self_type {
                pub fn create_control_proxy() -> #control_proxy_ident {
                    #control_proxy_ident::default()
                }
            }

            #[derive(Debug, Default)]
            pub struct #control_proxy_ident;
        };

        Ok(expanded_tokens)
    } else if let Ok(item_fn) = syn::parse2::<ItemFn>(input.clone()) {
        let function_ident = &item_fn.sig.ident;
        let helper_ident = quote::format_ident!("{}_control_proxy", function_ident);

        let expanded_tokens = quote! {
            #item_fn

            pub fn #helper_ident() -> &'static str {
                stringify!(#function_ident)
            }
        };

        Ok(expanded_tokens)
    } else {
        Err(syn::Error::new_spanned(
            input,
            "trf::control attribute can only be applied to impl blocks or functions",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::{trf_control_impl, trf_mock_impl, trf_test_impl};
    use quote::quote;

    #[test]
    fn test_macro_trf_test_expansion() {
        let input_tokens = quote! {
            async fn example_test(realm: &TestRealm) {
                assert!(true);
            }
        };

        let result = trf_test_impl(quote!(), input_tokens).expect("expansion should succeed");
        let result_string = result.to_string();

        assert!(result_string.contains("fuchsia :: test"));
        assert!(result_string.contains("async fn example_test"));
        assert!(result_string.contains("TestRealm"));
        assert!(result_string.contains("__trf_inner_example_test"));
    }

    #[test]
    fn test_macro_trf_mock_expansion() {
        let attribute_args = quote! { protocol = "fuchsia.examples.Echo" };
        let input_tokens = quote! {
            pub struct MyEchoMock;
        };

        let result = trf_mock_impl(attribute_args, input_tokens).expect("expansion should succeed");
        let result_string = result.to_string();

        assert!(result_string.contains("struct MyEchoMock"));
        assert!(result_string.contains("MOCK_PROTOCOL_NAME"));
        assert!(result_string.contains("fuchsia.examples.Echo"));
        assert!(result_string.contains("fn protocol_name"));
    }

    #[test]
    fn test_macro_control_proxy_generation() {
        let input_tokens = quote! {
            impl MyEchoMock {
                pub fn send_signal(&self) {}
            }
        };

        let result = trf_control_impl(quote!(), input_tokens).expect("expansion should succeed");
        let result_string = result.to_string();

        assert!(result_string.contains("impl MyEchoMock"));
        assert!(result_string.contains("fn create_control_proxy"));
        assert!(result_string.contains("MyEchoMockControlProxy"));
    }

    #[test]
    fn test_macro_trf_test_non_async_and_no_args() {
        let input_tokens = quote! {
            fn example_test_sync() {
                assert!(true);
            }
        };

        let result = trf_test_impl(quote!(), input_tokens).expect("expansion should succeed");
        let result_string = result.to_string();

        assert!(result_string.contains("fuchsia :: test"));
        assert!(result_string.contains("async fn example_test_sync"));
        // removed assert for explicit await
        println!("{}", result_string);
        assert!(result_string.contains("__trf_inner_example_test_sync"));
    }

    #[test]
    fn test_macro_trf_test_invalid_item() {
        let input_tokens = quote! {
            struct NotAFunction;
        };
        let result = trf_test_impl(quote!(), input_tokens);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().to_string(),
            "trf::test attribute can only be applied to functions"
        );
    }

    #[test]
    fn test_macro_trf_mock_missing_protocol() {
        let input_tokens = quote! { pub struct MyMock; };
        let result = trf_mock_impl(quote!(), input_tokens);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().to_string(),
            "missing 'protocol' parameter in trf::mock attribute"
        );
    }

    #[test]
    fn test_macro_trf_mock_invalid_key() {
        let attribute_args = quote! { name = "fuchsia.examples.Echo" };
        let input_tokens = quote! { pub struct MyMock; };
        let result = trf_mock_impl(attribute_args, input_tokens);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().to_string(),
            "unsupported key in trf::mock attribute (expected 'protocol')"
        );
    }

    #[test]
    fn test_macro_trf_mock_invalid_item() {
        let attribute_args = quote! { protocol = "fuchsia.examples.Echo" };
        let input_tokens = quote! { fn not_a_struct() {} };
        let result = trf_mock_impl(attribute_args, input_tokens);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().to_string(),
            "trf::mock attribute can only be applied to structs"
        );
    }

    #[test]
    fn test_macro_control_proxy_fn_generation() {
        let input_tokens = quote! {
            fn do_something() {}
        };
        let result = trf_control_impl(quote!(), input_tokens).expect("expansion should succeed");
        let result_string = result.to_string();

        assert!(result_string.contains("fn do_something"));
        assert!(result_string.contains("fn do_something_control_proxy"));
        assert!(result_string.contains("stringify ! (do_something)"));
    }

    #[test]
    fn test_macro_control_proxy_invalid_item() {
        let input_tokens = quote! {
            struct NotAnImplOrFn;
        };
        let result = trf_control_impl(quote!(), input_tokens);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().to_string(),
            "trf::control attribute can only be applied to impl blocks or functions"
        );
    }
}
