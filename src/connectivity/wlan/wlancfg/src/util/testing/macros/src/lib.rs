// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use proc_macro::{Span, TokenStream};
use quote::{format_ident, quote};
use syn::{parse_macro_input, ItemFn};

use fidl_fuchsia_wlan_sme as fidl_sme;

fn protection_to_str(p: &fidl_sme::Protection) -> &'static str {
    match p {
        fidl_sme::Protection::Open => "open",
        fidl_sme::Protection::Wep => "wep",
        fidl_sme::Protection::Wpa1 => "wpa1",
        fidl_sme::Protection::Wpa1Wpa2PersonalTkipOnly => "wpa1wpa2_personal_tkip",
        fidl_sme::Protection::Wpa2PersonalTkipOnly => "wpa2_personal_tkip",
        fidl_sme::Protection::Wpa1Wpa2Personal => "wpa1wpa2_personal",
        fidl_sme::Protection::Wpa2Personal => "wpa2_personal",
        fidl_sme::Protection::Wpa2Wpa3Personal => "wpa2wpa3_personal",
        fidl_sme::Protection::Wpa3Personal => "wpa3_personal",
        fidl_sme::Protection::Wpa2Enterprise => "wpa2_enterprise",
        fidl_sme::Protection::Wpa3Enterprise => "wpa3_enterprise",
        _ => "unknown_variant",
    }
}

#[proc_macro_attribute]
// Generates a separate roaming test case for each enumerated origin AP protection and target AP
// protection pair.
pub fn with_roam_protection_permutations(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let template_fn = parse_macro_input!(item as ItemFn);
    let template_fn_name = &template_fn.sig.ident;
    let template_fn_body = &template_fn.block;

    const ALL_NON_ENTERPRISE_PROTECTIONS: &[fidl_sme::Protection] = &[
        fidl_sme::Protection::Open,
        fidl_sme::Protection::Wep,
        fidl_sme::Protection::Wpa1,
        fidl_sme::Protection::Wpa1Wpa2PersonalTkipOnly,
        fidl_sme::Protection::Wpa2PersonalTkipOnly,
        fidl_sme::Protection::Wpa1Wpa2Personal,
        fidl_sme::Protection::Wpa2Personal,
        fidl_sme::Protection::Wpa2Wpa3Personal,
        fidl_sme::Protection::Wpa3Personal,
    ];

    // Generate test case code given test function, origin protection, and target protection.
    let mut generated_tests = Vec::new();
    for origin_protection in ALL_NON_ENTERPRISE_PROTECTIONS {
        for target_protection in ALL_NON_ENTERPRISE_PROTECTIONS {
            let expectation =
                if origin_protection == target_protection { "allowed" } else { "not_allowed" };
            let test_name = format_ident!(
                "test_roam_requests_{}_from_{}_to_{}",
                expectation,
                protection_to_str(origin_protection),
                protection_to_str(target_protection)
            );

            let origin_ident =
                syn::Ident::new(&format!("{:?}", origin_protection), Span::call_site().into());
            let target_ident =
                syn::Ident::new(&format!("{:?}", target_protection), Span::call_site().into());
            let test_case = quote! {
                #[test]
                fn #test_name() {
                    fn #template_fn_name(
                        origin_protection: fidl_fuchsia_wlan_sme::Protection,
                        target_protection: fidl_fuchsia_wlan_sme::Protection
                    ) {
                        #template_fn_body
                    }
                    #template_fn_name(
                        fidl_fuchsia_wlan_sme::Protection::#origin_ident,
                        fidl_fuchsia_wlan_sme::Protection::#target_ident
                    );
                }
            };
            generated_tests.push(test_case);
        }
    }

    // Write generated test case code.
    let final_stream = quote! {
        #(#generated_tests)*
    };
    final_stream.into()
}
