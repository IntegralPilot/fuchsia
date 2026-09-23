// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::header::HeaderDefinition;
use crate::ie::{Ie, IeDefinition};
use proc_macro::TokenStream;
use proc_macro2::Span;
use quote::quote;
use std::collections::HashMap;
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::{braced, parenthesized, parse_macro_input, Error, Expr, Ident, Result, Token};

macro_rules! unwrap_or_bail {
    ($x:expr) => {
        match $x {
            Err(e) => return TokenStream::from(e.to_compile_error()),
            Ok(x) => x,
        }
    };
}

const GROUP_NAME_FILL_ZEROES: &str = "fill_zeroes";
const GROUP_NAME_HEADERS: &str = "headers";
const GROUP_NAME_BODY: &str = "body";
const GROUP_NAME_IES: &str = "ies";
const GROUP_NAME_PAYLOAD: &str = "payload";

/// A set of `BufferWrite` types which can be written into a buffer.
enum Writeable {
    FillZeroes,
    Header(HeaderDefinition),
    Body(Expr),
    Ie(IeDefinition),
    Payload(Expr),
}

pub trait BufferWrite {
    fn gen_frame_len_tokens(&self) -> Result<proc_macro2::TokenStream>;
    fn gen_write_to_buf_tokens(&self) -> Result<proc_macro2::TokenStream>;
    fn gen_var_declaration_tokens(&self) -> Result<proc_macro2::TokenStream>;
}

impl BufferWrite for Writeable {
    fn gen_frame_len_tokens(&self) -> Result<proc_macro2::TokenStream> {
        match self {
            Writeable::FillZeroes => Ok(quote!()),
            Writeable::Header(x) => x.gen_frame_len_tokens(),
            Writeable::Body(_) => Ok(quote!(frame_len += body.len();)),
            Writeable::Ie(x) => x.gen_frame_len_tokens(),
            Writeable::Payload(_) => Ok(quote!(frame_len += payload.len();)),
        }
    }
    fn gen_write_to_buf_tokens(&self) -> Result<proc_macro2::TokenStream> {
        match self {
            Writeable::FillZeroes => Ok(quote!(
                frame_start_offset = w.remaining() - frame_len;
                w.append_bytes_zeroed(frame_start_offset)?;
            )),
            Writeable::Header(x) => x.gen_write_to_buf_tokens(),
            Writeable::Body(_) => Ok(quote!(w.append_value(&body[..])?;)),
            Writeable::Ie(x) => x.gen_write_to_buf_tokens(),
            Writeable::Payload(_) => Ok(quote!(w.append_value(&payload[..])?;)),
        }
    }
    fn gen_var_declaration_tokens(&self) -> Result<proc_macro2::TokenStream> {
        match self {
            Writeable::FillZeroes => Ok(quote!()),
            Writeable::Header(x) => x.gen_var_declaration_tokens(),
            Writeable::Body(x) => Ok(quote!(let body = #x;)),
            Writeable::Ie(x) => x.gen_var_declaration_tokens(),
            Writeable::Payload(x) => Ok(quote!(let payload = #x;)),
        }
    }
}
struct MacroArgs {
    buffer_source: Expr,
    write_defs: WriteDefinitions,
}

impl Parse for MacroArgs {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let buffer_source = input.parse::<Expr>()?;
        input.parse::<Token![,]>()?;
        let write_defs = input.parse::<WriteDefinitions>()?;
        Ok(MacroArgs { buffer_source, write_defs })
    }
}

struct DefaultSourceMacroArgs {
    write_defs: WriteDefinitions,
}

impl Parse for DefaultSourceMacroArgs {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let write_defs = input.parse::<WriteDefinitions>()?;
        Ok(DefaultSourceMacroArgs { write_defs })
    }
}

/// A parseable struct representing the macro's arguments.
struct WriteDefinitions {
    fill_zeroes: Option<Writeable>,
    hdrs: Vec<Writeable>,
    body: Option<Writeable>,
    fields: Vec<Writeable>,
    payload: Option<Writeable>,
}

impl Parse for WriteDefinitions {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let content;
        braced!(content in input);
        let groups = Punctuated::<GroupArgs, Token![,]>::parse_terminated(&content)?;

        let mut fill_zeroes = None;
        let mut hdrs = vec![];
        let mut fields = vec![];
        let mut body = None;
        let mut payload = None;
        for group in groups {
            match group {
                GroupArgs::FillZeroes(span) => {
                    if !hdrs.is_empty() || !fields.is_empty() || body.is_some() || payload.is_some()
                    {
                        return Err(Error::new(span, "fill_zeroes must appear first"));
                    }
                    if fill_zeroes.is_some() {
                        return Err(Error::new(span, "fill_zeroes must appear at most once"));
                    }
                    fill_zeroes.replace(Writeable::FillZeroes);
                }
                GroupArgs::Headers(data) => {
                    hdrs.extend(data.into_iter().map(|x| Writeable::Header(x)));
                }
                GroupArgs::Fields(data) => {
                    fields.extend(data.into_iter().map(|x| Writeable::Ie(x)));
                }
                GroupArgs::Payload(data) => {
                    if payload.is_some() {
                        return Err(Error::new(data.span(), "more than one payload defined"));
                    }
                    payload.replace(Writeable::Payload(data));
                }
                GroupArgs::Body(data) => {
                    if body.is_some() {
                        return Err(Error::new(data.span(), "more than one body defined"));
                    }
                    body.replace(Writeable::Body(data));
                }
            }
        }

        Ok(Self { fill_zeroes, hdrs, fields, body, payload })
    }
}

/// A parseable struct representing an individual group of definitions such as headers, IEs or
/// the buffer provider.
enum GroupArgs {
    FillZeroes(Span),
    Headers(Vec<HeaderDefinition>),
    Body(Expr),
    Fields(Vec<IeDefinition>),
    Payload(Expr),
}

impl Parse for GroupArgs {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let name: Ident = input.parse()?;
        input.parse::<Token![:]>()?;

        match name.to_string().as_str() {
            GROUP_NAME_FILL_ZEROES => {
                let content;
                parenthesized!(content in input);
                if content.is_empty() {
                    Ok(GroupArgs::FillZeroes(name.span()))
                } else {
                    Err(Error::new(name.span(), "fill_zeroes only supports the unit type: ()"))
                }
            }
            GROUP_NAME_HEADERS => {
                let content;
                braced!(content in input);
                let hdrs: Punctuated<HeaderDefinition, Token![,]> =
                    Punctuated::parse_terminated(&content)?;
                Ok(GroupArgs::Headers(hdrs.into_iter().collect()))
            }
            GROUP_NAME_IES => {
                let content;
                braced!(content in input);
                let ies: Punctuated<IeDefinition, Token![,]> =
                    Punctuated::parse_terminated(&content)?;
                let ies = ies.into_iter().collect::<Vec<_>>();

                // Error if a IE was defined more than once.
                let mut map = HashMap::new();
                for ie in &ies {
                    if let Some(_) = map.insert(ie.type_, ie) {
                        return Err(Error::new(ie.name.span(), "IE defined twice"));
                    }
                }

                let rates_presence =
                    map.iter().fold((false, None), |acc, (type_, ie)| match type_ {
                        Ie::ExtendedRates { .. } => (acc.0, Some(ie)),
                        Ie::Rates => (true, None),
                        _ => acc,
                    });
                if let (false, Some(ie)) = rates_presence {
                    return Err(Error::new(
                        ie.name.span(),
                        "`extended_supported_rates` IE specified without `supported_rates` IE",
                    ));
                }

                Ok(GroupArgs::Fields(ies))
            }
            GROUP_NAME_BODY => Ok(GroupArgs::Body(input.parse::<Expr>()?)),
            GROUP_NAME_PAYLOAD => Ok(GroupArgs::Payload(input.parse::<Expr>()?)),
            unknown => Err(Error::new(name.span(), format!("unknown group: '{}'", unknown))),
        }
    }
}

fn process_write_definitions(
    write_defs: WriteDefinitions,
    make_buf_tokens: proc_macro2::TokenStream,
    return_buf_tokens: proc_macro2::TokenStream,
    allow_fill_zeroes: bool,
) -> TokenStream {
    let mut declare_var_tokens = quote!();
    let mut write_to_buf_tokens = quote!();
    let mut frame_len_tokens = quote!(let mut frame_len = 0;);

    // Order writable pieces: FillZeroes + Hdr + Body + Fields + Payload
    let mut writables = vec![];
    if let Some(x) = write_defs.fill_zeroes {
        if !allow_fill_zeroes {
            panic!("fill_zeroes only allowed in write_frame_with_fixed_slice");
        }
        writables.push(x)
    }
    writables.extend(write_defs.hdrs);
    if let Some(x) = write_defs.body {
        writables.push(x);
    }
    writables.extend(write_defs.fields);
    if let Some(x) = write_defs.payload {
        writables.push(x);
    }

    for x in writables {
        let tokens = unwrap_or_bail!(x.gen_write_to_buf_tokens());
        write_to_buf_tokens = quote!(#write_to_buf_tokens #tokens);

        let tokens = unwrap_or_bail!(x.gen_frame_len_tokens());
        frame_len_tokens = quote!(#frame_len_tokens #tokens);

        let tokens = unwrap_or_bail!(x.gen_var_declaration_tokens());
        declare_var_tokens = quote!(#declare_var_tokens #tokens);
    }

    TokenStream::from(quote! {
        {
            use {
                wlan_frame_writer::__wlan_common::{
                    append::{Append, TrackedAppend},
                    buffer_writer::BufferWriter,
                    error::FrameWriteError,
                    ie::{self, IE_PREFIX_LEN, SUPPORTED_RATES_MAX_LEN},
                },
                wlan_frame_writer::__zerocopy::IntoBytes,
                std::convert::AsRef,
                std::mem::size_of,
            };


            || -> Result<_, FrameWriteError> {
                #declare_var_tokens
                #frame_len_tokens

                #make_buf_tokens

                {
                    #write_to_buf_tokens
                }

                #return_buf_tokens
            }()
        }
    })
}

pub fn process_with_default_source(input: TokenStream) -> TokenStream {
    let macro_args = parse_macro_input!(input as DefaultSourceMacroArgs);
    let buf_tokens = quote!(
        let arena = wlan_frame_writer::__Arena::new();
        let mut buffer = arena.insert_default_slice::<u8>(frame_len);
        let mut w = BufferWriter::new(&mut buffer[..]);
    );
    let return_buf_tokens = quote!(
        if w.bytes_appended() != frame_len {
            return Err(FrameWriteError::BadWrite(
                format!("bytes_appended does not match frame length: {} != {}",
                    w.bytes_appended(), frame_len)
            ));
        }
        Ok(arena.make_static(buffer))
    );
    process_write_definitions(macro_args.write_defs, buf_tokens, return_buf_tokens, false)
}

pub fn process_with_cursor(input: TokenStream) -> TokenStream {
    let macro_args = parse_macro_input!(input as MacroArgs);
    let buffer_source = macro_args.buffer_source;
    let buf_tokens = quote!(
        let mut w = #buffer_source;
    );
    let return_buf_tokens = quote!(Ok(w));
    process_write_definitions(macro_args.write_defs, buf_tokens, return_buf_tokens, false)
}

pub fn process_with_fixed_slice(input: TokenStream) -> TokenStream {
    let macro_args = parse_macro_input!(input as MacroArgs);
    let buffer_source = macro_args.buffer_source;
    let buf_tokens = quote!(
        let mut w = BufferWriter::new(#buffer_source);
        let mut frame_start_offset = 0;
    );
    let return_buf_tokens = quote!(
        // offset is the start of the frame because it is either the
        // beginning of the buffer or the first byte of the frame
        // after filling the front of the buffer with zeroes.
        let frame_start = frame_start_offset;
        // w.bytes_appended() is always the end of the frame because it
        // already counts the zeroes that might be written before the
        // appended
        let frame_end = w.bytes_appended();
        Ok((frame_start, frame_end))
    );
    process_write_definitions(macro_args.write_defs, buf_tokens, return_buf_tokens, true)
}
