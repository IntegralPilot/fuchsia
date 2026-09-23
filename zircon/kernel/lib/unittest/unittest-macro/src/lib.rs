// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{ToTokens, quote};
use syn::parse::{Parse, ParseStream};
use syn::{
    Attribute, Block, Error, Expr, Ident, Item, ItemFn, ItemMod, Lit, LitStr, Meta, ReturnType,
    Type, parse_macro_input,
};

const SECTION_NAME: &str = ".data.rel.ro.unittest_testcases";

struct TestSuiteArgs {
    name: Option<String>,
}

impl Parse for TestSuiteArgs {
    fn parse(input: ParseStream<'_>) -> Result<Self, Error> {
        if input.is_empty() {
            return Ok(TestSuiteArgs { name: None });
        }

        let ident: Ident = input.parse()?;
        if ident != "name" {
            return Err(Error::new(ident.span(), "expected 'name'"));
        }

        input.parse::<syn::Token![=]>()?;
        let lit: LitStr = input.parse()?;

        Ok(TestSuiteArgs { name: Some(lit.value()) })
    }
}

#[proc_macro_attribute]
pub fn suite(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(attr as TestSuiteArgs);
    let mut suite = parse_macro_input!(item as TestSuite);
    suite.custom_name = args.name;
    quote!(#suite).into()
}

// Represents a test function, as defined within a #[suite]-annotated
// module.
struct TestCase {
    // The name of the test.
    ident: Ident,
    // A brief description of the test.
    docstring: String,
    // The test function's body.
    block: Block,
    // Whether the test is ignored.
    ignored: bool,
}

impl TestCase {
    fn from_item_fn(item_fn: ItemFn) -> Result<Self, Error> {
        if !item_fn.sig.generics.params.is_empty() {
            return Err(Error::new_spanned(
                &item_fn.sig.generics,
                "a test function cannot be generic",
            ));
        }

        if !item_fn.sig.inputs.is_empty() {
            return Err(Error::new_spanned(
                &item_fn.sig.inputs,
                "a test function must take no arguments",
            ));
        }

        match &item_fn.sig.output {
            ReturnType::Default => {}
            ReturnType::Type(_, ty) => {
                let unit_type = if let Type::Tuple(type_tuple) = &**ty {
                    type_tuple.elems.is_empty()
                } else {
                    false
                };
                if !unit_type {
                    return Err(Error::new_spanned(ty, "a test function must return ()"));
                }
            }
        }

        let docstring =
            get_single_line_docstring(&item_fn.sig.ident, &item_fn.attrs, "test function")?;

        let ignored = item_fn.attrs.iter().any(|attr| attr.path().is_ident("ignore"));

        Ok(Self { ident: item_fn.sig.ident, docstring, block: *item_fn.block, ignored })
    }
}

impl ToTokens for TestCase {
    fn to_tokens(&self, tokens: &mut TokenStream2) {
        let ident = &self.ident;
        let block = &self.block;
        let doc_comment = format!(" {}", self.docstring);
        let maybe_allow_dead_code = if self.ignored {
            quote! { #[allow(dead_code)] }
        } else {
            quote! {}
        };

        // Declarative macros can't reference variables not defined in their
        // scope, but they can reference other macros, so we introduce
        // record_failure!() for use in the assert/expect macros.
        tokens.extend(quote! {
            #[doc = #doc_comment]
            #maybe_allow_dead_code
            pub extern "C" fn #ident() -> bool {
                #[allow(unused_mut)]
                let mut all_ok = true;

                #[allow(unused_macros)]
                macro_rules! record_failure {
                    () => {
                        #[allow(unused_assignments)]
                        {
                            all_ok = false;
                        }
                    };
                }

                #block
                all_ok
            }
        });
    }
}

struct TestSuite {
    ident: Ident,
    docstring: String,
    custom_name: Option<String>,
    non_test_items: Vec<Item>,
    cases: Vec<TestCase>,
}

impl Parse for TestSuite {
    fn parse(input: ParseStream<'_>) -> Result<Self, Error> {
        let input_mod: ItemMod = input.parse()?;
        let mod_ident = &input_mod.ident;

        let docstring =
            get_single_line_docstring(mod_ident, &input_mod.attrs, "test suite module")?;

        let content = input_mod.content.map(|(_, items)| items).unwrap_or(Vec::new());
        let mut cases = Vec::new();
        let mut non_test_items = Vec::new();
        for mut item in content {
            if let Item::Fn(item_fn) = &mut item {
                let test_attr_idx =
                    item_fn.attrs.iter().position(|attr| attr.path().is_ident("test"));
                if let Some(idx) = test_attr_idx {
                    item_fn.attrs.remove(idx);
                    cases.push(TestCase::from_item_fn(item_fn.clone())?);
                    continue;
                }
            }
            non_test_items.push(item);
        }

        if cases.is_empty() {
            return Err(Error::new_spanned(
                mod_ident,
                "a test suite module must contain at least one test function",
            ));
        }

        Ok(Self { ident: mod_ident.clone(), docstring, custom_name: None, non_test_items, cases })
    }
}

impl ToTokens for TestSuite {
    fn to_tokens(&self, tokens: &mut TokenStream2) {
        let mod_ident = &self.ident;
        let suite_name = match &self.custom_name {
            Some(name) => name.clone(),
            None => mod_ident.to_string(),
        };
        let suite_name_c_str = format!("{suite_name}\0");
        let suite_desc_c_str = format!("{}\0", self.docstring);
        let doc_comment = format!(" {}", self.docstring);

        let non_test_items = &self.non_test_items;
        let cases = &self.cases;
        let enabled_cases: Vec<&TestCase> = cases.iter().filter(|c| !c.ignored).collect();
        let reg_entries = enabled_cases.iter().map(|case| {
            let ident = &case.ident;
            let case_name_c_str = format!("{ident}\0");
            quote! {
                ::unittest::TestCaseRegistration {
                    name: #case_name_c_str.as_ptr() as *const core::ffi::c_char,
                    fn_: #ident,
                }
            }
        });

        let enabled_test_count = enabled_cases.len();
        tokens.extend(quote! {
            #[doc = #doc_comment]
            pub mod #mod_ident {
                #[cfg(not(ktest))]
                compile_error!("#[suite] may only be used in a cfg(ktest) context");

                use super::*;

                #( #non_test_items )*

                #( #cases )*

                const _: () = {
                    static TESTS: [::unittest::TestCaseRegistration; #enabled_test_count] = [
                        #( #reg_entries ),*
                    ];

                    #[unsafe(link_section = #SECTION_NAME)]
                    #[used]
                    static SUITE: ::unittest::TestSuiteRegistration = ::unittest::TestSuiteRegistration {
                        name: #suite_name_c_str.as_ptr() as *const core::ffi::c_char,
                        desc: #suite_desc_c_str.as_ptr() as *const core::ffi::c_char,
                        tests: TESTS.as_ptr(),
                        test_cnt: #enabled_test_count,
                    };
                };
            }
        });
    }
}

// Gets an item's docstring, if present, and enforces that it be a single line
// the sake of terminal printing brevity.
fn get_single_line_docstring(
    item: &Ident,
    attrs: &[Attribute],
    what: &str,
) -> Result<String, Error> {
    let mut doc_attr = None;
    for attr in attrs {
        if !attr.path().is_ident("doc") {
            continue;
        }
        if doc_attr.is_some() {
            return Err(Error::new_spanned(
                item,
                format!("a {what}'s docstring must be a single line. Keep it brief!"),
            ));
        }
        doc_attr = Some(attr);
    }
    let Some(doc_attr) = doc_attr else {
        return Err(Error::new_spanned(
            item,
            format!("a {what} must have a docstring description"),
        ));
    };

    if let Meta::NameValue(meta_name_value) = &doc_attr.meta {
        if let Expr::Lit(expr_lit) = &meta_name_value.value {
            if let Lit::Str(lit_str) = &expr_lit.lit {
                let val = lit_str.value().trim().to_string();
                if !val.is_empty() {
                    return Ok(val);
                }
            }
        }
    }
    Err(Error::new_spanned(item, format!("a {what} docstring cannot be empty")))
}
