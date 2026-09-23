// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use proc_macro::{Delimiter, TokenStream, TokenTree};

/// A procedural macro attribute that generates the C++ mangled symbol name for the
/// given string literal, and attaches it as `#[unsafe(export_name = "...")]` to the
/// target static variable.
///
/// Expects a single double-quoted string literal as the attribute argument, e.g.
/// `#[interned_string_export_name("hello")]`.
#[proc_macro_attribute]
pub fn interned_string_export_name(attr: TokenStream, item: TokenStream) -> TokenStream {
    generate_export_name_attr(attr, item, mangle_cpp_interned_string)
}

/// A procedural macro attribute that generates the C++ mangled symbol name for the
/// given category string literal, and attaches it as `#[unsafe(export_name = "...")]`
/// to the target static variable.
#[proc_macro_attribute]
pub fn interned_category_export_name(attr: TokenStream, item: TokenStream) -> TokenStream {
    generate_export_name_attr(attr, item, mangle_cpp_interned_category)
}

// Common helper to parse the attribute string, run the mangler, and generate the
// `export_name` attribute.
fn generate_export_name_attr<F>(attr: TokenStream, item: TokenStream, mangle_fn: F) -> TokenStream
where
    F: FnOnce(&str) -> String,
{
    let mut iter = attr.into_iter();
    let tt = match iter.next() {
        Some(tt) => tt,
        None => panic!("Expected string literal as attribute argument"),
    };
    let actual_str = extract_string_literal(tt, "attribute argument");

    let cpp_symbol = mangle_fn(&actual_str);

    let mut result = TokenStream::new();
    let export_name_attr = format!("#[unsafe(export_name = \"{}\")]", cpp_symbol);
    result.extend(export_name_attr.parse::<TokenStream>().unwrap());
    result.extend(item);
    result
}

// Extracts and validates the raw string content from a double-quoted string literal token tree,
// recursively unwrapping invisible `Delimiter::None` groups that the compiler wraps around
// macro arguments to preserve spans and boundaries.
fn extract_string_literal(mut tt: TokenTree, context: &str) -> String {
    while let TokenTree::Group(group) = &tt {
        if group.delimiter() == Delimiter::None {
            let mut iter = group.stream().into_iter();
            if let Some(first) = iter.next() {
                if iter.next().is_none() {
                    tt = first;
                    continue;
                }
            }
        }
        break;
    }

    let lit = match tt {
        TokenTree::Literal(lit) => lit,
        _ => panic!("Expected string literal as {}, got: {}", context, tt),
    };

    let str_val = lit.to_string();
    if !str_val.starts_with('"') || !str_val.ends_with('"') {
        panic!("{} must be a double-quoted string literal", context);
    }
    str_val[1..str_val.len() - 1].to_string()
}

// Generates the Itanium C++ ABI mangled name for the C++ template instantiation
// `fxt::internal::InternedStringStorage<chars...>::interned_string`.
//
// Under the Itanium C++ ABI:
// - `_ZN` starts a nested name sequence.
// - `3fxt` is namespace `fxt`.
// - `8internal` is namespace `internal`.
// - `21InternedStringStorage` is the class name.
// - `IJ` starts the template parameter pack.
// - Each character `c` of the string is mangled as an integral non-type template
//   parameter of type `char` (mangled type code `c`): `Lc` + [decimal value] + `E`.
// - `EE` closes the template parameters list.
// - `15interned_string` is the static member variable name.
// - `E` closes the nested name sequence.
//
// For example, the string `"foo"` is mangled to:
// `_ZN3fxt8internal21InternedStringStorageIJLc102ELc111ELc111EEE15interned_stringE`
fn mangle_cpp_interned_string(s: &str) -> String {
    let mut result = String::from("_ZN3fxt8internal21InternedStringStorageIJ");
    for c in s.chars() {
        result.push_str(&format!("Lc{}E", c as u32));
    }
    result.push_str("EE15interned_stringE");
    result
}

// Generates the Itanium C++ ABI mangled name for the C++ template instantiation
// `fxt::internal::InternedCategoryStorage<chars...>::interned_category`.
fn mangle_cpp_interned_category(s: &str) -> String {
    let mut result = String::from("_ZN3fxt8internal23InternedCategoryStorageIJ");
    for c in s.chars() {
        result.push_str(&format!("Lc{}E", c as u32));
    }
    result.push_str("EE17interned_categoryE");
    result
}

/// A function-like procedural macro that generates an `unsafe extern "C"` block importing
/// the C++ mangled category symbol for the given string literal.
///
/// Usage:
/// `import_category!(VAR_NAME, "category.name");`
#[proc_macro]
pub fn import_category(input: TokenStream) -> TokenStream {
    generate_import_block(
        input,
        "::kstring::interned_category::InternedCategory",
        mangle_cpp_interned_category,
    )
}

/// A function-like procedural macro that generates an `unsafe extern "C"` block importing
/// the C++ mangled string symbol for the given string literal.
///
/// Usage:
/// `import_string!(VAR_NAME, "string.value");`
#[proc_macro]
pub fn import_string(input: TokenStream) -> TokenStream {
    generate_import_block(
        input,
        "::kstring::interned_string::InternedString",
        mangle_cpp_interned_string,
    )
}

// Common helper to parse the macro inputs (an identifier and a string literal),
// run the mangling function, and generate the `unsafe extern "C"` block with the type.
fn generate_import_block<F>(input: TokenStream, type_path: &str, mangle_fn: F) -> TokenStream
where
    F: FnOnce(&str) -> String,
{
    let mut iter = input.into_iter();

    // Parse the identifier
    let var_name = match iter.next() {
        Some(TokenTree::Ident(ident)) => ident,
        _ => panic!("Expected identifier as first argument to import macro"),
    };

    // Parse the comma separator
    match iter.next() {
        Some(TokenTree::Punct(punct)) if punct.as_char() == ',' => {}
        _ => panic!("Expected comma separator between identifier and string literal"),
    };

    // Parse and extract the string literal
    let str_lit_tt = match iter.next() {
        Some(tt) => tt,
        None => panic!("Expected string literal as second argument to import macro"),
    };
    let actual_str = extract_string_literal(str_lit_tt, "second argument to import macro");

    let cpp_symbol = mangle_fn(&actual_str);

    let expanded = format!(
        "unsafe extern \"C\" {{
            #[link_name = \"{}\"]
            pub static {}: {};
        }}",
        cpp_symbol, var_name, type_path
    );

    expanded.parse::<TokenStream>().unwrap()
}
