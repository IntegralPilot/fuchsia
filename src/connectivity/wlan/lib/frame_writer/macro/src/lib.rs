// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A procedural macro for writing frames into buffers.

mod frame_writer;
mod header;
mod ie;

/// Allocates a buffer in a new `fdf::Arena` and writes a frame into the buffer.
///
/// This happens in three steps:
///
///   1. Compute the frame's length.
///   2. Allocate a buffer an `fdf::Arena`.
///   3. Write the frame into the buffer.
///
/// # Headers
///
/// Headers must derive zerocopy's various traits and be declared as packed & C-compatible.
/// Headers are declared through a qualified path to a type followed by an expression evaluating
/// to a reference of the same type. For example:
///
/// ```
/// headers: {
///     mac::EthernetIIHdr: &mac::EthernetIIHdr {
///         da: dst_addr,
///         sa: src_addr,
///         ether_type: llc_frame.hdr.protocol_id,
///     },
/// },
/// ```
///
/// # Body
///
/// An arbitrary body can be written to the end of the buffer's header section.
/// The body's type must be a slice of a compatible type.
///
/// # IEs
///
/// ## Supported IEs
///
/// * ssid: a byte slice
/// * supported_rates: a byte slice
/// * dsss_param_set: wlan_common::ie::DsssParamSet
/// * extended_supported_rates: a byte slice **OR** `{}`
///                   If `{}` is used the rates supplied in the `supported_rates` IE will be
///                   continued in this one if they exceed the maximum amount of allowed supported
///                   rates.
///                   Note: `extended_supported_rates` cannot be used without also declaring
///                         `supported_rates` first. `extended_supported_rates` may not directly
///                         follow the `supported_rates` IE.
/// * tim: wlan_common::ie::TimView
/// * ht_caps: wlan_common::ie::HtCapabilities
/// * vht_caps: wlan_common::ie::VhtCapabilities
/// * rsne: wlan_common::ie::rsn::rsne::Rsne
/// * bss_max_idle_period: wlan_common::ie::BssMaxIdlePeriod
/// * wsc: wlan_common::ie::Wsc
/// * wpa1: wlan_common::ie::Wpa1
///
/// ## IE values
///
/// IE values can be any expression that evaluates to the corresponding IE type.
/// For example the following expressions are valid:
///
/// ```
/// ssid: vec![4u8; 12],
/// ssid: SSID_CONSTANT,
/// ssid: b"foobar",
/// ssid: self.generate_ssid(),
/// ssid: local_var,
/// ssid: match x { 0 => b"foo", _ => b"bar" },
/// ssid: if x { b"foo" } else { b"bar" },
/// ```
///
/// ## Optional IEs
///
/// IEs may be optional (see examples below). Optional IEs are declared through an `?` token.
/// Value expressions in optional IEs must evaluate to a value of type `Option<V>` with `V` being
/// a compatible value type. If the value is declared through an if-statement and **NO** else branch
/// was defined, the then-branch's value must be of type `V` and will automatically be wrapped with
/// `Some(_)` if the condition was met. If the condition was not met, `None` will be used.
/// If an else-brach was defined both branches must return an `Option<V>` value.
///
/// ## Emit IE offset
///
/// One can emit the offset at which a particular IE is written at.
/// ```
/// let mut offset = 0;
/// let buffer = write_frame!({
///     ...
///     ies: {
///         ssid: ssid,
///         supported_rates: &rates,
///         offset @ extended_supported_rates: {/* continue rates */},
///     },
///     ...
/// });
/// ```
///
/// # Payload
///
/// An arbitrary payload can be written to the end of the buffer. The payload's type must be a slice
/// of a compatible type.
///
/// # Return type
///
/// The macro returns a `Result<ArenaStaticBuffer<[u8]>, Error>`.
///
/// # Constraints
///
/// The macro early returns with an error if the writing to the buffer failed, thus, the caller must
/// carry a return type of `Result<(), Error>`. This macro is heavily dependent on the `wlan_common`
/// crates and assumes the caller to depend on those.
///
/// # Examples
///
/// ```
/// let buffer = write_frame!({
///     headers: {
///         mac::MgmtHdr: &mac::MgmtHdr {
///             frame_ctrl: mac::FrameControl(0)
///                 .with_frame_type(mac::FrameType::MGMT)
///                 .with_mgmt_subtype(mac::MgmtSubtype::PROBE_REQ),
///             duration: 0,
///             addr1: mac::BCAST_ADDR,
///             addr2: self.iface_mac,
///             addr3: mac::BCAST_ADDR,
///             seq_ctrl: mac::SequenceControl(0)
///                 .with_seq_num(self.ctx.seq_mgr.next_sns1(&mac::BCAST_ADDR) as u16),
///         },
///     },
///     ies: {
///         ssid: ssid,
///         supported_rates: &rates,
///         offset @ extended_supported_rates: {/* continue rates */},
///         ht_cap?: if band_info.ht_supported {
///             band_info.ht_caps.into()
///         },
///         vht_cap?: if band_info.vht_supported {
///             band_info.vht_caps.into()
///         }
///     },
///     payload: vec![42u8; 5],
/// });
/// ```
#[proc_macro]
pub fn write_frame(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    frame_writer::process_with_default_source(input)
}

/// Writes a frame into a fixed sized slice.
///
/// This macro writes the frame into the fixed sized buffer in the
/// first argument and returns a `Result<(usize, usize), Error>` where
/// the values in the returned tuple are the beginning and ending indices
/// of the frame, respectively.
///
/// The second argument of this macro is the specification of the
/// frame defined in the documentation for `write_frame` with the
/// addition of an optional `fill_zeroes` field.
///
/// # Fill with zeroes
///
/// The caller may optionally specify `fill_zeroes: ()` at the
/// beginning of the frame specification. The effect is that the frame
/// will be written so that the end of the frame ends at the end of
/// the buffer. The beginning of the buffer will be filled with zeroes
/// up to the start of the frame. For example,
///
/// ```
/// let buffer = [0u8; 10];
/// let (frame_start, frame_end) = write_frame_with_fixed_slice!(&mut buffer[..], {
///     fill_zeroes: (),
///     ies: { ssid: &b"foobar"[..] },
/// });
/// assert_eq!(&[0, 6, 102, 111, 111, 98, 97, 114, 0, 0][..], &buffer[frame_start..frame_end]);
/// ```
#[proc_macro]
pub fn write_frame_with_fixed_slice(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    frame_writer::process_with_fixed_slice(input)
}

/// Appends a frame to a buffer that implements `wlan_common::append::Append`.
///
/// This macro mutates the dynamically sized buffer in the first
/// argument and returns a `Result<B, Error>` where `B` is the type of
/// the dynamically sized buffer. The second argument of this macro is
/// the specification of the frame defined in the documentation for
/// `write_frame`.
#[proc_macro]
pub fn append_frame_to(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    frame_writer::process_with_cursor(input)
}

/// Writes a frame to a Vec<u8>
///
/// This macro returns a `Result<Vec<u8>, Error>` containing the frame
/// specified according to the documentation for `write_frame`.
#[proc_macro]
pub fn write_frame_to_vec(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let input = proc_macro2::TokenStream::from(input);
    let out = proc_macro2::TokenStream::from(frame_writer::process_with_cursor(
        proc_macro::TokenStream::from(quote::quote!(
            wlan_frame_writer::__wlan_common::append::VecCursor::new(),
            #input
        )),
    ));
    proc_macro::TokenStream::from(quote::quote!(
        #out.map(|buffer| Vec::from(buffer))
    ))
}
