// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Buffered (owned) frames and MAC data.
//!
//! This module provides types that copy buffers used by `zerocopy` types, in particular MAC frame
//! types. The `Buffered` type must be used in extractors to accept MAC frame types, because these
//! types cannot otherwise own the buffers from which they are parsed.
//!
//! For technical reasons, empty proxy types are used to name corresponding MAC frame types. For
//! example, `buffered::Buffered<buffered::MgmtFrame>` buffers a `mac::MgmtFrame`, which is
//! distinct from the empty `buffered::MgmtFrame` type. Generally, there is no need to explicitly
//! refer to the corresponding `mac` types in event handlers.

use fidl_fuchsia_wlan_tap as fidl_tap;
use std::borrow::Borrow;
use std::fmt::Debug;
use std::marker::PhantomData;
use wlan_common::mac::{
    self, ActionBody, AssocReqFrame as ParsedAssocReqFrame, AssocRespFrame as ParsedAssocRespFrame,
    AuthFrame as ParsedAuthFrame, DataFrame as ParsedDataFrame, MacFrame as ParsedMacFrame,
    MgmtFrame as ParsedMgmtFrame, NoAck, ProbeReqFrame as ParsedProbeReqFrame,
};
use zerocopy::SplitByteSlice;

use crate::event::extract::FromEvent;

pub type ParsedActionFrame<const NO_ACK: bool, B> = NoAck<NO_ACK, ActionBody<B>>;

mod sealed {
    use super::*;

    pub trait AsBuffer<T>
    where
        T: Parse,
    {
        fn as_buffer(&self) -> Option<&[u8]>;
    }
}
use sealed::AsBuffer;

pub trait Parse {
    type Output<B>
    where
        B: SplitByteSlice;

    fn parse<B>(bytes: B) -> Option<Self::Output<B>>
    where
        B: SplitByteSlice;
}

pub trait TaggedField: Parse {
    type Tag: Tag;

    fn tag<B>(parsed: &Self::Output<B>) -> Self::Tag
    where
        B: SplitByteSlice;
}

pub trait TaggedVariant<T>
where
    T: TaggedField,
{
    const TAG: T::Tag;

    fn has_tag(tag: impl Borrow<T::Tag>) -> bool {
        Self::TAG.eq(tag.borrow())
    }
}

pub trait Tag: Eq {
    fn is_supported(&self) -> bool;
}

impl Tag for mac::FrameType {
    fn is_supported(&self) -> bool {
        mac::FrameType::is_supported(self)
    }
}

impl Tag for mac::MgmtSubtype {
    fn is_supported(&self) -> bool {
        mac::MgmtSubtype::is_supported(self)
    }
}

pub enum MacFrame {}

impl AsBuffer<MacFrame> for fidl_tap::TxArgs {
    fn as_buffer(&self) -> Option<&[u8]> {
        Some(&self.packet.data)
    }
}

impl Parse for MacFrame {
    type Output<B>
        = ParsedMacFrame<B>
    where
        B: SplitByteSlice;

    fn parse<B>(bytes: B) -> Option<Self::Output<B>>
    where
        B: SplitByteSlice,
    {
        ParsedMacFrame::parse(bytes, false)
    }
}

impl TaggedField for MacFrame {
    type Tag = mac::FrameType;

    fn tag<B>(frame: &Self::Output<B>) -> Self::Tag
    where
        B: SplitByteSlice,
    {
        match frame {
            ParsedMacFrame::Ctrl { .. } => mac::FrameType::CTRL,
            ParsedMacFrame::Data { .. } => mac::FrameType::DATA,
            ParsedMacFrame::Mgmt { .. } => mac::FrameType::MGMT,
            ParsedMacFrame::Unsupported { frame_ctrl } => { *frame_ctrl }.frame_type(),
        }
    }
}

pub enum DataFrame {}

impl AsBuffer<DataFrame> for fidl_tap::TxArgs {
    fn as_buffer(&self) -> Option<&[u8]> {
        Some(&self.packet.data)
    }
}

impl Parse for DataFrame {
    type Output<B>
        = ParsedDataFrame<B>
    where
        B: SplitByteSlice;

    fn parse<B>(bytes: B) -> Option<Self::Output<B>>
    where
        B: SplitByteSlice,
    {
        ParsedDataFrame::parse(bytes, false)
    }
}

impl TaggedVariant<MacFrame> for DataFrame {
    const TAG: <MacFrame as TaggedField>::Tag = mac::FrameType::DATA;
}

pub enum MgmtFrame {}

impl AsBuffer<MgmtFrame> for fidl_tap::TxArgs {
    fn as_buffer(&self) -> Option<&[u8]> {
        Some(&self.packet.data)
    }
}

impl Parse for MgmtFrame {
    type Output<B>
        = ParsedMgmtFrame<B>
    where
        B: SplitByteSlice;

    fn parse<B>(bytes: B) -> Option<Self::Output<B>>
    where
        B: SplitByteSlice,
    {
        ParsedMgmtFrame::parse(bytes, false)
    }
}

impl TaggedField for MgmtFrame {
    type Tag = mac::MgmtSubtype;

    fn tag<B>(frame: &Self::Output<B>) -> Self::Tag
    where
        B: SplitByteSlice,
    {
        { frame.mgmt_hdr.frame_ctrl }.mgmt_subtype()
    }
}

impl TaggedVariant<MacFrame> for MgmtFrame {
    const TAG: <MacFrame as TaggedField>::Tag = mac::FrameType::MGMT;
}

pub enum ActionFrame<const NO_ACK: bool> {}

impl AsBuffer<ActionFrame<false>> for fidl_tap::TxArgs {
    fn as_buffer(&self) -> Option<&[u8]> {
        ParsedMgmtFrame::parse(self.packet.data.as_slice(), false).and_then(|frame| {
            ActionFrame::<false>::has_tag(MgmtFrame::tag(&frame)).then_some(frame.body)
        })
    }
}

impl AsBuffer<ActionFrame<true>> for fidl_tap::TxArgs {
    fn as_buffer(&self) -> Option<&[u8]> {
        ParsedMgmtFrame::parse(self.packet.data.as_slice(), false).and_then(|frame| {
            ActionFrame::<true>::has_tag(MgmtFrame::tag(&frame)).then_some(frame.body)
        })
    }
}

impl<const NO_ACK: bool> Parse for ActionFrame<NO_ACK> {
    type Output<B>
        = ParsedActionFrame<NO_ACK, B>
    where
        B: SplitByteSlice;

    fn parse<B>(bytes: B) -> Option<Self::Output<B>>
    where
        B: SplitByteSlice,
    {
        NoAck::<NO_ACK, ActionBody<B>>::parse(bytes)
    }
}

impl TaggedVariant<MgmtFrame> for ActionFrame<false> {
    const TAG: <MgmtFrame as TaggedField>::Tag = mac::MgmtSubtype::ACTION;
}

impl TaggedVariant<MgmtFrame> for ActionFrame<true> {
    const TAG: <MgmtFrame as TaggedField>::Tag = mac::MgmtSubtype::ACTION_NO_ACK;
}

pub enum AssocReqFrame {}

impl AsBuffer<AssocReqFrame> for fidl_tap::TxArgs {
    fn as_buffer(&self) -> Option<&[u8]> {
        ParsedMgmtFrame::parse(self.packet.data.as_slice(), false)
            .and_then(|frame| AssocReqFrame::has_tag(MgmtFrame::tag(&frame)).then_some(frame.body))
    }
}

impl Parse for AssocReqFrame {
    type Output<B>
        = ParsedAssocReqFrame<B>
    where
        B: SplitByteSlice;

    fn parse<B>(bytes: B) -> Option<Self::Output<B>>
    where
        B: SplitByteSlice,
    {
        ParsedAssocReqFrame::parse(bytes)
    }
}

impl TaggedVariant<MgmtFrame> for AssocReqFrame {
    const TAG: <MgmtFrame as TaggedField>::Tag = mac::MgmtSubtype::ASSOC_REQ;
}

pub enum AssocRespFrame {}

impl AsBuffer<AssocRespFrame> for fidl_tap::TxArgs {
    fn as_buffer(&self) -> Option<&[u8]> {
        ParsedMgmtFrame::parse(self.packet.data.as_slice(), false)
            .and_then(|frame| AssocRespFrame::has_tag(MgmtFrame::tag(&frame)).then_some(frame.body))
    }
}

impl Parse for AssocRespFrame {
    type Output<B>
        = ParsedAssocRespFrame<B>
    where
        B: SplitByteSlice;

    fn parse<B>(bytes: B) -> Option<Self::Output<B>>
    where
        B: SplitByteSlice,
    {
        ParsedAssocRespFrame::parse(bytes)
    }
}

impl TaggedVariant<MgmtFrame> for AssocRespFrame {
    const TAG: <MgmtFrame as TaggedField>::Tag = mac::MgmtSubtype::ASSOC_RESP;
}

pub enum AuthFrame {}

impl AsBuffer<AuthFrame> for fidl_tap::TxArgs {
    fn as_buffer(&self) -> Option<&[u8]> {
        ParsedMgmtFrame::parse(self.packet.data.as_slice(), false)
            .and_then(|frame| AuthFrame::has_tag(MgmtFrame::tag(&frame)).then_some(frame.body))
    }
}

impl Parse for AuthFrame {
    type Output<B>
        = ParsedAuthFrame<B>
    where
        B: SplitByteSlice;

    fn parse<B>(bytes: B) -> Option<Self::Output<B>>
    where
        B: SplitByteSlice,
    {
        ParsedAuthFrame::parse(bytes)
    }
}

impl TaggedVariant<MgmtFrame> for AuthFrame {
    const TAG: <MgmtFrame as TaggedField>::Tag = mac::MgmtSubtype::AUTH;
}

pub enum ProbeReqFrame {}

impl AsBuffer<ProbeReqFrame> for fidl_tap::TxArgs {
    fn as_buffer(&self) -> Option<&[u8]> {
        ParsedMgmtFrame::parse(self.packet.data.as_slice(), false)
            .and_then(|frame| ProbeReqFrame::has_tag(MgmtFrame::tag(&frame)).then_some(frame.body))
    }
}

impl Parse for ProbeReqFrame {
    type Output<B>
        = ParsedProbeReqFrame<B>
    where
        B: SplitByteSlice;

    fn parse<B>(bytes: B) -> Option<Self::Output<B>>
    where
        B: SplitByteSlice,
    {
        ParsedProbeReqFrame::parse(bytes)
    }
}

impl TaggedVariant<MgmtFrame> for ProbeReqFrame {
    const TAG: <MgmtFrame as TaggedField>::Tag = mac::MgmtSubtype::PROBE_REQ;
}

/// A marker type that indicates that only supported tags of the given type may be parsed.
///
/// For example, to extract any supported management frame, the `Buffered<Supported<MgmtFrame>>`
/// type can be used.
pub struct Supported<T>(PhantomData<fn() -> T>);

impl<T> Parse for Supported<T>
where
    T: Parse + TaggedField,
{
    type Output<B>
        = T::Output<B>
    where
        B: SplitByteSlice;

    fn parse<B>(bytes: B) -> Option<Self::Output<B>>
    where
        B: SplitByteSlice,
    {
        T::parse(bytes).and_then(|parsed| T::tag(&parsed).is_supported().then_some(parsed))
    }
}

impl<T, E> AsBuffer<Supported<T>> for E
where
    T: Parse + TaggedField,
    E: AsBuffer<T>,
{
    fn as_buffer(&self) -> Option<&[u8]> {
        AsBuffer::<T>::as_buffer(self)
    }
}

/// A marker type that indicates that only **un**supported tags of the given type may be parsed.
///
/// For example, to extract any unsupported management frame, the
/// `Buffered<Unsupported<MgmtFrame>>` type can be used.
pub struct Unsupported<T>(PhantomData<fn() -> T>);

impl<T> Parse for Unsupported<T>
where
    T: Parse + TaggedField,
{
    type Output<B>
        = T::Output<B>
    where
        B: SplitByteSlice;

    fn parse<B>(bytes: B) -> Option<Self::Output<B>>
    where
        B: SplitByteSlice,
    {
        T::parse(bytes).and_then(|parsed| (!T::tag(&parsed).is_supported()).then_some(parsed))
    }
}

impl<T, E> AsBuffer<Unsupported<T>> for E
where
    T: Parse + TaggedField,
    E: AsBuffer<T>,
{
    fn as_buffer(&self) -> Option<&[u8]> {
        AsBuffer::<T>::as_buffer(self)
    }
}

/// A buffered `zerocopy` type that owns a copy of its underlying buffer.
#[derive(Clone, Debug)]
pub struct Buffered<T>
where
    T: Parse,
{
    buffer: Vec<u8>,
    phantom: PhantomData<fn() -> T>,
}

impl<T> Buffered<T>
where
    T: Parse,
{
    /// Gets the parsed `zerocopy` type.
    pub fn get(&self) -> T::Output<&'_ [u8]> {
        T::parse(self.buffer.as_slice()).expect("buffered data failed to reparse")
    }
}

impl<E, T> FromEvent<E> for Buffered<T>
where
    E: AsBuffer<T>,
    T: Parse,
{
    fn from_event(event: &E) -> Option<Self> {
        // The cost of emitting static data is two-fold for `zerocopy` types: any buffer must be
        // cloned and the type must be parsed once here and likely at least one more time in
        // handlers. However, this allows handlers and extractors to be very composable and
        // flexible without exceptionally complex APIs and type bounds. See `Buffered`.
        event.as_buffer().and_then(|buffer| {
            T::parse(buffer).map(|_| Buffered { buffer: buffer.into(), phantom: PhantomData })
        })
    }
}
