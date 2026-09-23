// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use proc_macro::TokenStream;
use proc_macro2::Span;
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::{Expr, LitByteStr, LitStr, Token};

enum Family {
    SignedInt(char),
    UnsignedInt(char),
    String,
    CString,
    Pointer,
    Char,
    Bool,
    Float(char),
}

fn parse_spec(spec: &str) -> (String, String, String, Option<String>) {
    let mut flags = String::new();
    let mut width = String::new();
    let mut prec = String::new();

    let chars: Vec<char> = spec.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        match chars[i] {
            '<' => {
                flags.push('-');
                i += 1;
            }
            '>' => {
                i += 1;
            }
            '^' => {
                i += 1;
            }
            '+' => {
                flags.push('+');
                i += 1;
            }
            '-' => {
                flags.push('-');
                i += 1;
            }
            '0' if width.is_empty() && prec.is_empty() => {
                flags.push('0');
                i += 1;
            }
            '#' => {
                flags.push('#');
                i += 1;
            }
            ' ' => {
                flags.push(' ');
                i += 1;
            }
            _ => break,
        }
    }

    while i < chars.len() && chars[i].is_ascii_digit() {
        width.push(chars[i]);
        i += 1;
    }

    if i < chars.len() && chars[i] == '.' {
        prec.push('.');
        i += 1;
        while i < chars.len() && chars[i].is_ascii_digit() {
            prec.push(chars[i]);
            i += 1;
        }
    }

    let explicit_type = if i < chars.len() {
        let remaining: String = chars[i..].iter().collect();
        if remaining == "cs" || remaining == "z" {
            Some("cs".to_string())
        } else if let Some(&c) = chars.get(i) {
            if matches!(
                c,
                'x' | 'X' | 'o' | 'p' | 'b' | 'e' | 'E' | 'f' | 'd' | 'i' | 'u' | 's' | 'c'
            ) {
                Some(c.to_string())
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };

    (flags, width, prec, explicit_type)
}

fn infer_family(arg: &Expr) -> Family {
    match arg {
        Expr::Lit(lit) => match &lit.lit {
            syn::Lit::Str(_) => Family::String,
            syn::Lit::Char(_) | syn::Lit::Byte(_) => Family::Char,
            syn::Lit::Bool(_) => Family::Bool,
            syn::Lit::Float(_) => Family::Float('f'),
            syn::Lit::Int(int_lit) => {
                let suffix = int_lit.suffix();
                if suffix.starts_with('u') || suffix == "usize" {
                    Family::UnsignedInt('u')
                } else {
                    Family::SignedInt('d')
                }
            }
            _ => Family::SignedInt('d'),
        },
        Expr::Reference(r) => match &*r.expr {
            Expr::Lit(lit) => match &lit.lit {
                syn::Lit::Str(_) | syn::Lit::ByteStr(_) => Family::String,
                syn::Lit::Char(_) | syn::Lit::Byte(_) => Family::Char,
                _ => infer_family(&*r.expr),
            },
            _ => infer_family(&*r.expr),
        },
        Expr::Cast(cast) => {
            let ty_str = quote!(#cast.ty).to_string();
            if ty_str.contains("str") || ty_str.contains("CStr") || ty_str.contains("[u8]") {
                Family::String
            } else if ty_str.contains("* const c_char")
                || ty_str.contains("* mut c_char")
                || ty_str.contains("* const i8")
                || ty_str.contains("* mut i8")
            {
                Family::CString
            } else if ty_str.contains("char") {
                Family::Char
            } else if ty_str.contains("bool") {
                Family::Bool
            } else if ty_str.contains('*') || ty_str.contains("ptr") || ty_str.contains("c_void") {
                Family::Pointer
            } else if ty_str.contains("u8")
                || ty_str.contains("u16")
                || ty_str.contains("u32")
                || ty_str.contains("u64")
                || ty_str.contains("u128")
                || ty_str.contains("usize")
            {
                Family::UnsignedInt('u')
            } else if ty_str.contains("f32") || ty_str.contains("f64") {
                Family::Float('f')
            } else {
                Family::SignedInt('d')
            }
        }
        _ => Family::SignedInt('d'),
    }
}

fn translate_arg(
    kprint_crate: &syn::Path,
    spec: &str,
    arg: &proc_macro2::TokenStream,
    expr_for_inference: &Expr,
    extra_bindings: &mut Vec<proc_macro2::TokenStream>,
    char_counter: &mut usize,
) -> (String, Vec<proc_macro2::TokenStream>) {
    let (mut flags, width, prec, explicit_type) = parse_spec(spec);
    let family = match explicit_type.as_deref() {
        Some("d" | "i") => Family::SignedInt('d'),
        Some("u") => Family::UnsignedInt('u'),
        Some("o") => Family::UnsignedInt('o'),
        Some("x") => Family::UnsignedInt('x'),
        Some("X") => Family::UnsignedInt('X'),
        Some("s") => Family::String,
        Some("cs" | "z") => Family::CString,
        Some("p") => Family::Pointer,
        Some("c") => Family::Char,
        Some("b") => Family::Bool,
        Some("f") => Family::Float('f'),
        Some("e") => Family::Float('e'),
        Some("E") => Family::Float('E'),
        _ => infer_family(expr_for_inference),
    };

    match family {
        Family::SignedInt(c) => (
            format!("%{}{}{}{}{}", flags, width, prec, "ll", c),
            vec![quote! { #kprint_crate::backend::AsKPrintSignedInt::as_c_longlong(&#arg) }],
        ),
        Family::UnsignedInt(c) => {
            if flags.contains('#') && (c == 'x' || c == 'X') {
                flags = flags.replace('#', "");
                let prefix = if c == 'X' { "0X" } else { "0x" };
                let adjusted_width = if !width.is_empty() {
                    let w: usize = width.parse().unwrap_or(0);
                    if w > 2 { (w - 2).to_string() } else { String::new() }
                } else {
                    String::new()
                };
                (
                    format!("{}%{}{}ll{}", prefix, flags, adjusted_width, c),
                    vec![
                        quote! { #kprint_crate::backend::AsKPrintUnsignedInt::as_c_ulonglong(&#arg) },
                    ],
                )
            } else {
                (
                    format!("%{}{}{}{}{}", flags, width, prec, "ll", c),
                    vec![
                        quote! { #kprint_crate::backend::AsKPrintUnsignedInt::as_c_ulonglong(&#arg) },
                    ],
                )
            }
        }
        Family::String => {
            let prec_clamp = if !prec.is_empty() {
                if let Ok(limit) = prec[1..].parse::<usize>() {
                    let limit_lit = limit as i32;
                    quote! { core::cmp::min(#limit_lit, #kprint_crate::backend::AsKPrintStr::kprint_len(&#arg)) }
                } else {
                    quote! { #kprint_crate::backend::AsKPrintStr::kprint_len(&#arg) }
                }
            } else {
                quote! { #kprint_crate::backend::AsKPrintStr::kprint_len(&#arg) }
            };
            (
                format!("%{}{}.*s", flags, width),
                vec![prec_clamp, quote! { #kprint_crate::backend::AsKPrintStr::kprint_ptr(&#arg) }],
            )
        }
        Family::CString => (
            format!("%{}{}{}{}", flags, width, prec, "s"),
            vec![quote! { #kprint_crate::backend::AsKPrintCString::as_c_ptr(&#arg) }],
        ),
        Family::Pointer => (
            format!("%{}{}{}p", flags, width, prec),
            vec![quote! { #kprint_crate::backend::AsKPrintPointer::as_c_ptr_void(&#arg) }],
        ),
        Family::Char => {
            let char_buf_ident = quote::format_ident!("__kprint_char_buf_{}", *char_counter);
            let char_res_ident = quote::format_ident!("__kprint_char_res_{}", *char_counter);
            *char_counter += 1;
            extra_bindings.push(quote! {
                let mut #char_buf_ident = [0u8; 4];
                let #char_res_ident = #kprint_crate::backend::AsKPrintChar::kprint_encode_char(
                    &#arg,
                    &mut #char_buf_ident,
                );
            });
            (
                format!("%{}{}.*s", flags, width),
                vec![quote! { #char_res_ident.0 }, quote! { #char_res_ident.1 }],
            )
        }

        Family::Bool => (
            format!("%{}{}.*s", flags, width),
            vec![
                quote! { if #arg { 4 } else { 5 } as core::ffi::c_int },
                quote! { (if #arg { &b"true\0"[..] } else { &b"false\0"[..] }).as_ptr() as *const core::ffi::c_char },
            ],
        ),
        Family::Float(c) => (
            format!("%{}{}{}{}", flags, width, prec, c),
            vec![quote! { (#arg) as core::ffi::c_double }],
        ),
    }
}

enum ArgItem {
    Positional(Expr),
    Named(syn::Ident, Expr),
}

impl Parse for ArgItem {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        if input.peek(syn::Ident) && input.peek2(Token![=]) {
            let name: syn::Ident = input.parse()?;
            input.parse::<Token![=]>()?;
            let expr: Expr = input.parse()?;
            Ok(ArgItem::Named(name, expr))
        } else {
            let expr: Expr = input.parse()?;
            Ok(ArgItem::Positional(expr))
        }
    }
}

fn parse_fmt_str(input: ParseStream<'_>) -> syn::Result<LitStr> {
    if input.peek(LitStr) {
        return input.parse::<LitStr>();
    }
    if input.peek(syn::Ident) {
        let ident: syn::Ident = input.parse()?;
        if ident == "concat" {
            input.parse::<Token![!]>()?;
            let content;
            syn::parenthesized!(content in input);
            let mut combined = String::new();
            while !content.is_empty() {
                if content.peek(LitStr) {
                    let s: LitStr = content.parse()?;
                    combined.push_str(&s.value());
                } else if content.peek(syn::LitInt) {
                    let i: syn::LitInt = content.parse()?;
                    combined.push_str(&i.to_string());
                } else if content.peek(syn::LitBool) {
                    let b: syn::LitBool = content.parse()?;
                    combined.push_str(if b.value { "true" } else { "false" });
                } else if content.peek(syn::LitChar) {
                    let c: syn::LitChar = content.parse()?;
                    combined.push(c.value());
                } else {
                    let expr: Expr = content.parse()?;
                    return Err(syn::Error::new_spanned(
                        expr,
                        "concat! in kprint format string only supports literal tokens",
                    ));
                }
                if !content.is_empty() {
                    content.parse::<Token![,]>()?;
                }
            }
            return Ok(LitStr::new(&combined, ident.span()));
        }
    }
    Err(input.error("expected string literal or concat!(...) format string"))
}

struct ParsedCall {
    kprint_crate: syn::Path,
    buf: Option<Expr>,
    fmt: LitStr,
    args: Vec<ArgItem>,
}

fn parse_internal_kprint(input: ParseStream<'_>) -> syn::Result<ParsedCall> {
    let kprint_crate: syn::Path = input.parse()?;
    input.parse::<Token![,]>()?;
    let fmt = parse_fmt_str(input)?;
    let mut args = Vec::new();
    while !input.is_empty() {
        input.parse::<Token![,]>()?;
        if input.is_empty() {
            break;
        }
        args.push(input.parse::<ArgItem>()?);
    }
    Ok(ParsedCall { kprint_crate, buf: None, fmt, args })
}

fn parse_internal_kformat(input: ParseStream<'_>) -> syn::Result<ParsedCall> {
    let kprint_crate: syn::Path = input.parse()?;
    input.parse::<Token![,]>()?;
    let buf: Expr = input.parse()?;
    input.parse::<Token![,]>()?;
    let fmt = parse_fmt_str(input)?;
    let mut args = Vec::new();
    while !input.is_empty() {
        input.parse::<Token![,]>()?;
        if input.is_empty() {
            break;
        }
        args.push(input.parse::<ArgItem>()?);
    }
    Ok(ParsedCall { kprint_crate, buf: Some(buf), fmt, args })
}

struct GeneratedOutput {
    fmt_bytes: Vec<u8>,
    bindings: Vec<proc_macro2::TokenStream>,
    emitted_args: Vec<proc_macro2::TokenStream>,
}

fn build_output(
    kprint_crate: &syn::Path,
    prefix: &str,
    suffix: &str,
    fmt_lit: &LitStr,
    args: &[ArgItem],
) -> syn::Result<GeneratedOutput> {
    let mut bindings = Vec::new();
    let mut positional_vars = Vec::new();
    let mut named_vars = std::collections::HashMap::new();

    // Bind all supplied arguments once in lexical order to guarantee single evaluation.
    for (pos_idx, item) in args.iter().enumerate() {
        match item {
            ArgItem::Positional(e) => {
                let var_name = quote::format_ident!("__kprint_pos_{}", pos_idx);
                bindings.push(quote! {
                    let #var_name = #e;
                });
                positional_vars.push((var_name, e));
            }
            ArgItem::Named(ident, e) => {
                let var_name = quote::format_ident!("__kprint_named_{}", ident);
                bindings.push(quote! {
                    let #var_name = #e;
                });
                named_vars.insert(ident.to_string(), (var_name, e));
            }
        }
    }

    let mut emitted_args = Vec::new();
    let mut c_fmt = String::from(prefix);
    let mut next_auto_pos = 0;
    let chars: Vec<char> = fmt_lit.value().chars().collect();
    let mut i = 0;
    let mut char_counter = 0usize;

    while i < chars.len() {
        if chars[i] == '{' {
            if i + 1 < chars.len() && chars[i + 1] == '{' {
                c_fmt.push('{');
                i += 2;
                continue;
            }
            let mut j = i + 1;
            while j < chars.len() && chars[j] != '}' {
                j += 1;
            }
            if j >= chars.len() {
                return Err(syn::Error::new_spanned(fmt_lit, "Unclosed '{' in format string"));
            }
            let placeholder_content: String = chars[i + 1..j].iter().collect();
            i = j + 1;

            let (target, spec_part) = if let Some(colon_idx) = placeholder_content.find(':') {
                (&placeholder_content[..colon_idx], &placeholder_content[colon_idx + 1..])
            } else {
                (placeholder_content.as_str(), "")
            };

            let (var_token, expr_for_inference) = if target.is_empty() {
                if next_auto_pos >= positional_vars.len() {
                    return Err(syn::Error::new_spanned(
                        fmt_lit,
                        "Not enough positional arguments for format string",
                    ));
                }
                let pos = next_auto_pos;
                next_auto_pos += 1;
                let (ref var_name, orig_expr) = positional_vars[pos];
                (quote! { #var_name }, (*orig_expr).clone())
            } else if let Ok(idx) = target.parse::<usize>() {
                if idx >= positional_vars.len() {
                    return Err(syn::Error::new_spanned(
                        fmt_lit,
                        format!("Positional index {{{}}} is out of range", idx),
                    ));
                }
                let (ref var_name, orig_expr) = positional_vars[idx];
                (quote! { #var_name }, (*orig_expr).clone())
            } else if let Some((var_name, orig_expr)) = named_vars.get(target) {
                (quote! { #var_name }, (*orig_expr).clone())
            } else {
                let ident = syn::Ident::new(target, fmt_lit.span());
                let expr_path = syn::Expr::Path(syn::ExprPath {
                    attrs: Vec::new(),
                    qself: None,
                    path: syn::Path::from(ident.clone()),
                });
                (quote! { #ident }, expr_path)
            };

            let (c_spec, emitted) = translate_arg(
                kprint_crate,
                spec_part,
                &var_token,
                &expr_for_inference,
                &mut bindings,
                &mut char_counter,
            );
            c_fmt.push_str(&c_spec);
            emitted_args.extend(emitted);
        } else if chars[i] == '}' {
            if i + 1 < chars.len() && chars[i + 1] == '}' {
                c_fmt.push('}');
                i += 2;
            } else {
                return Err(syn::Error::new_spanned(fmt_lit, "Unmatched '}' in format string"));
            }
        } else if chars[i] == '%' {
            c_fmt.push_str("%%");
            i += 1;
        } else {
            c_fmt.push(chars[i]);
            i += 1;
        }
    }

    if next_auto_pos > 0 && next_auto_pos < positional_vars.len() {
        let (_, orig_expr) = &positional_vars[next_auto_pos];
        return Err(syn::Error::new_spanned(orig_expr, "argument never used in format string"));
    }

    c_fmt.push_str(suffix);
    c_fmt.push('\0');

    Ok(GeneratedOutput { fmt_bytes: c_fmt.into_bytes(), bindings, emitted_args })
}

fn emit_print_parsed(prefix: &str, suffix: &str, parsed: ParsedCall) -> TokenStream {
    let GeneratedOutput { fmt_bytes, bindings, emitted_args } =
        match build_output(&parsed.kprint_crate, prefix, suffix, &parsed.fmt, &parsed.args) {
            Ok(res) => res,
            Err(e) => return e.to_compile_error().into(),
        };

    let kprint_crate = parsed.kprint_crate;
    let fmt_lit = LitByteStr::new(&fmt_bytes, Span::call_site());

    quote! {
        {
            #(#bindings)*
            const __FMT: &[u8] = #fmt_lit;
            unsafe {
                let _ = #kprint_crate::backend::printf(
                    __FMT.as_ptr() as *const core::ffi::c_char,
                    #(#emitted_args),*
                );
            }
        }
    }
    .into()
}

fn emit_format_parsed(parsed: ParsedCall) -> TokenStream {
    let GeneratedOutput { fmt_bytes, bindings, emitted_args } =
        match build_output(&parsed.kprint_crate, "", "", &parsed.fmt, &parsed.args) {
            Ok(res) => res,
            Err(e) => return e.to_compile_error().into(),
        };

    let kprint_crate = parsed.kprint_crate;
    let fmt_lit = LitByteStr::new(&fmt_bytes, Span::call_site());
    let buf_expr = parsed.buf.expect("buffer expression required for kformat");

    quote! {
        {
            #(#bindings)*
            const __FMT: &[u8] = #fmt_lit;
            let __len = unsafe {
                #kprint_crate::backend::snprintf(
                    (#buf_expr).as_mut_ptr() as *mut core::ffi::c_char,
                    (#buf_expr).len(),
                    __FMT.as_ptr() as *const core::ffi::c_char,
                    #(#emitted_args),*
                )
            };
            let __valid = if __len < 0 {
                0
            } else {
                (__len as usize).min((#buf_expr).len().saturating_sub(1))
            };
            &(#buf_expr)[..__valid]
        }
    }
    .into()
}

#[proc_macro]
pub fn __kprint_internal(tokens: TokenStream) -> TokenStream {
    let parsed = syn::parse_macro_input!(tokens with parse_internal_kprint);
    emit_print_parsed("", "", parsed)
}

#[proc_macro]
pub fn __kprintln_internal(tokens: TokenStream) -> TokenStream {
    let parsed = syn::parse_macro_input!(tokens with parse_internal_kprint);
    emit_print_parsed("", "\n", parsed)
}

#[proc_macro]
pub fn __kformat_internal(tokens: TokenStream) -> TokenStream {
    let parsed = syn::parse_macro_input!(tokens with parse_internal_kformat);
    emit_format_parsed(parsed)
}
