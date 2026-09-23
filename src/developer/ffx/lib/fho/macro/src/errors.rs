// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use proc_macro2::{Span, TokenStream};
use quote::{ToTokens, quote_spanned};

#[derive(Debug)]
pub enum ParseError {
    DuplicateAttr(Span),
    CommandRequired(Span),
    OnlyStructsSupported(Span),
    OnlyNamedFieldStructsSupported(Span),
    InvalidCheckAttr(Span),
    InvalidWithAttr(Span),
    InvalidAttr(Span),
    UnexpectedAttr(String, Span),
    InvalidTargetAttr(Span, String),
}

impl ToTokens for ParseError {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        match self {
            ParseError::DuplicateAttr(span) => tokens.extend(
                        quote_spanned! {*span=>
                            std::compile_error!("Duplicate attribute found. Can only have one attribute kind per field");
                        }
                    ),
            ParseError::CommandRequired(span) => tokens.extend(
                        quote_spanned! {*span=>
                            std::compile_error!("An ffx tool must have exactly one field denoted with the `#[command]` attribute");
                        }
                    ),
            ParseError::OnlyStructsSupported(span) => tokens.extend(
                        quote_spanned! {*span=>
                            std::compile_error!("`#[derive(FfxTool)]` can only be applied to structs.");
                        }
                    ),
            ParseError::OnlyNamedFieldStructsSupported(span) => tokens.extend(
                        quote_spanned! {*span=>
                            std::compile_error!("`#[derive(FfxTool)]` does not support unit or tuple structs");
                        }
                    ),
            ParseError::InvalidCheckAttr(span) => tokens.extend(
                        quote_spanned! {*span=>
                            std::compile_error!("`#[check()]` attribute contents must be a call expression");
                        }
                    ),
            ParseError::InvalidWithAttr(span) => tokens.extend(
                        quote_spanned! {*span=>
                            std::compile_error!("`#[with()]` attribute contents must be a call expression");
                        }
                    ),
            ParseError::UnexpectedAttr(name, span) => tokens.extend(
                        quote_spanned! {*span=>
                            std::compile_error!("`#[{}]` unexpected here", #name);
                        }
                    ),
            ParseError::InvalidAttr(span) => tokens.extend(
                        quote_spanned! {*span=>
                            std::compile_error!("Invalid or unknown attribute for FfxTool");
                        }
                    ),
            ParseError::InvalidTargetAttr(span, msg) => tokens.extend(
                        quote_spanned! {*span =>
                             std::compile_error!(#msg);
                         }
                     ),
        }
    }
}
