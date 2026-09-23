// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Serialization.

use std::cmp;
use std::convert::Infallible as Never;
use std::fmt::{self, Debug, Formatter};
use std::marker::PhantomData;
use std::ops::{Range, RangeBounds};

use arrayvec::ArrayVec;
use zerocopy::SplitByteSlice;

use crate::{
    AsFragmentedByteSlice, Buffer, BufferView, BufferViewMut, ContiguousBuffer, EmptyBuf,
    FragmentedBuffer, FragmentedBufferMut, FragmentedBytes, FragmentedBytesMut, GrowBuffer,
    GrowBufferMut, ParsablePacket, ParseBuffer, ParseBufferMut, ReusableBuffer, ShrinkBuffer,
    canonicalize_range,
};

/// Either of two buffers.
///
/// An `Either` wraps one of two different buffer types. It implements all of
/// the relevant traits by calling the corresponding methods on the wrapped
/// buffer.
#[derive(Copy, Clone, Debug)]
pub enum Either<A, B> {
    A(A),
    B(B),
}

impl<A, B> Either<A, B> {
    /// Maps the `A` variant of an `Either`.
    ///
    /// Given an `Either<A, B>` and a function from `A` to `AA`, `map_a`
    /// produces an `Either<AA, B>` by applying the function to the `A` variant
    /// or passing on the `B` variant unmodified.
    pub fn map_a<AA, F: FnOnce(A) -> AA>(self, f: F) -> Either<AA, B> {
        match self {
            Either::A(a) => Either::A(f(a)),
            Either::B(b) => Either::B(b),
        }
    }

    /// Maps the `B` variant of an `Either`.
    ///
    /// Given an `Either<A, B>` and a function from `B` to `BB`, `map_b`
    /// produces an `Either<A, BB>` by applying the function to the `B` variant
    /// or passing on the `A` variant unmodified.
    pub fn map_b<BB, F: FnOnce(B) -> BB>(self, f: F) -> Either<A, BB> {
        match self {
            Either::A(a) => Either::A(a),
            Either::B(b) => Either::B(f(b)),
        }
    }

    /// Returns the `A` variant in an `Either<A, B>`.
    ///
    /// # Panics
    ///
    /// Panics if this `Either<A, B>` does not hold the `A` variant.
    pub fn unwrap_a(self) -> A {
        match self {
            Either::A(x) => x,
            Either::B(_) => panic!("This `Either<A, B>` does not hold the `A` variant"),
        }
    }

    /// Returns the `B` variant in an `Either<A, B>`.
    ///
    /// # Panics
    ///
    /// Panics if this `Either<A, B>` does not hold the `B` variant.
    pub fn unwrap_b(self) -> B {
        match self {
            Either::A(_) => panic!("This `Either<A, B>` does not hold the `B` variant"),
            Either::B(x) => x,
        }
    }
}

impl<A> Either<A, A> {
    /// Returns the inner value held by this `Either` when both possible values
    /// `Either::A` and `Either::B` contain the same inner types.
    pub fn into_inner(self) -> A {
        match self {
            Either::A(x) => x,
            Either::B(x) => x,
        }
    }
}

impl<A> Either<A, Never> {
    /// Returns the `A` value in an `Either<A, Never>`.
    #[inline]
    pub fn into_a(self) -> A {
        match self {
            Either::A(a) => a,
        }
    }
}

impl<B> Either<Never, B> {
    /// Returns the `B` value in an `Either<Never, B>`.
    #[inline]
    pub fn into_b(self) -> B {
        match self {
            Either::B(b) => b,
        }
    }
}

macro_rules! call_method_on_either {
    ($val:expr, $method:ident, $($args:expr),*) => {
        match $val {
            Either::A(a) => a.$method($($args),*),
            Either::B(b) => b.$method($($args),*),
        }
    };
    ($val:expr, $method:ident) => {
        call_method_on_either!($val, $method,)
    };
}

// NOTE(joshlf): We override the default implementations of all methods for
// Either. Many of the default implementations make multiple calls to other
// Buffer methods, each of which performs a match statement to figure out which
// Either variant is present. We assume that doing this match once is more
// performant than doing it multiple times.

impl<A, B> FragmentedBuffer for Either<A, B>
where
    A: FragmentedBuffer,
    B: FragmentedBuffer,
{
    fn len(&self) -> usize {
        call_method_on_either!(self, len)
    }

    fn with_bytes<'a, R, F>(&'a self, f: F) -> R
    where
        F: for<'b> FnOnce(FragmentedBytes<'b, 'a>) -> R,
    {
        call_method_on_either!(self, with_bytes, f)
    }
}

impl<A, B> ContiguousBuffer for Either<A, B>
where
    A: ContiguousBuffer,
    B: ContiguousBuffer,
{
}

impl<A, B> ShrinkBuffer for Either<A, B>
where
    A: ShrinkBuffer,
    B: ShrinkBuffer,
{
    fn shrink<R: RangeBounds<usize>>(&mut self, range: R) {
        call_method_on_either!(self, shrink, range)
    }
    fn shrink_front(&mut self, n: usize) {
        call_method_on_either!(self, shrink_front, n)
    }
    fn shrink_back(&mut self, n: usize) {
        call_method_on_either!(self, shrink_back, n)
    }
}

impl<A, B> ParseBuffer for Either<A, B>
where
    A: ParseBuffer,
    B: ParseBuffer,
{
    fn parse<'a, P: ParsablePacket<&'a [u8], ()>>(&'a mut self) -> Result<P, P::Error> {
        call_method_on_either!(self, parse)
    }
    fn parse_with<'a, ParseArgs, P: ParsablePacket<&'a [u8], ParseArgs>>(
        &'a mut self,
        args: ParseArgs,
    ) -> Result<P, P::Error> {
        call_method_on_either!(self, parse_with, args)
    }
}

impl<A, B> FragmentedBufferMut for Either<A, B>
where
    A: FragmentedBufferMut,
    B: FragmentedBufferMut,
{
    fn with_bytes_mut<'a, R, F>(&'a mut self, f: F) -> R
    where
        F: for<'b> FnOnce(FragmentedBytesMut<'b, 'a>) -> R,
    {
        call_method_on_either!(self, with_bytes_mut, f)
    }
}

impl<A, B> ParseBufferMut for Either<A, B>
where
    A: ParseBufferMut,
    B: ParseBufferMut,
{
    fn parse_mut<'a, P: ParsablePacket<&'a mut [u8], ()>>(&'a mut self) -> Result<P, P::Error> {
        call_method_on_either!(self, parse_mut)
    }
    fn parse_with_mut<'a, ParseArgs, P: ParsablePacket<&'a mut [u8], ParseArgs>>(
        &'a mut self,
        args: ParseArgs,
    ) -> Result<P, P::Error> {
        call_method_on_either!(self, parse_with_mut, args)
    }
}

impl<A, B> GrowBuffer for Either<A, B>
where
    A: GrowBuffer,
    B: GrowBuffer,
{
    #[inline]
    fn with_parts<'a, O, F>(&'a self, f: F) -> O
    where
        F: for<'b> FnOnce(&'a [u8], FragmentedBytes<'b, 'a>, &'a [u8]) -> O,
    {
        call_method_on_either!(self, with_parts, f)
    }
    fn capacity(&self) -> usize {
        call_method_on_either!(self, capacity)
    }
    fn prefix_len(&self) -> usize {
        call_method_on_either!(self, prefix_len)
    }
    fn suffix_len(&self) -> usize {
        call_method_on_either!(self, suffix_len)
    }
    fn grow_front(&mut self, n: usize) {
        call_method_on_either!(self, grow_front, n)
    }
    fn grow_back(&mut self, n: usize) {
        call_method_on_either!(self, grow_back, n)
    }
    fn reset(&mut self) {
        call_method_on_either!(self, reset)
    }
}

impl<A, B> GrowBufferMut for Either<A, B>
where
    A: GrowBufferMut,
    B: GrowBufferMut,
{
    fn with_parts_mut<'a, O, F>(&'a mut self, f: F) -> O
    where
        F: for<'b> FnOnce(&'a mut [u8], FragmentedBytesMut<'b, 'a>, &'a mut [u8]) -> O,
    {
        call_method_on_either!(self, with_parts_mut, f)
    }

    fn with_all_contents_mut<'a, O, F>(&'a mut self, f: F) -> O
    where
        F: for<'b> FnOnce(FragmentedBytesMut<'b, 'a>) -> O,
    {
        call_method_on_either!(self, with_all_contents_mut, f)
    }

    fn serialize<C: SerializationContext, BB: PacketBuilder<C>>(
        &mut self,
        context: &mut C,
        builder: BB,
    ) {
        call_method_on_either!(self, serialize, context, builder)
    }
}

impl<A, B> Buffer for Either<A, B>
where
    A: Buffer,
    B: Buffer,
{
    fn parse_with_view<'a, ParseArgs, P: ParsablePacket<&'a [u8], ParseArgs>>(
        &'a mut self,
        args: ParseArgs,
    ) -> Result<(P, &'a [u8]), P::Error> {
        call_method_on_either!(self, parse_with_view, args)
    }
}

impl<A: AsRef<[u8]>, B: AsRef<[u8]>> AsRef<[u8]> for Either<A, B> {
    fn as_ref(&self) -> &[u8] {
        call_method_on_either!(self, as_ref)
    }
}

impl<A: AsMut<[u8]>, B: AsMut<[u8]>> AsMut<[u8]> for Either<A, B> {
    fn as_mut(&mut self) -> &mut [u8] {
        call_method_on_either!(self, as_mut)
    }
}

/// A byte slice wrapper providing buffer functionality.
///
/// A `Buf` wraps a byte slice (a type which implements `AsRef<[u8]>` or
/// `AsMut<[u8]>`) and implements various buffer traits by keeping track of
/// prefix, body, and suffix offsets within the byte slice.
#[derive(Clone, Debug)]
pub struct Buf<B> {
    buf: B,
    body: Range<usize>,
}

impl<B: AsRef<[u8]>> PartialEq for Buf<B> {
    fn eq(&self, other: &Self) -> bool {
        let self_slice = AsRef::<[u8]>::as_ref(self);
        let other_slice = AsRef::<[u8]>::as_ref(other);
        PartialEq::eq(self_slice, other_slice)
    }
}

impl<B: AsRef<[u8]>> Eq for Buf<B> {}

impl Buf<Vec<u8>> {
    /// Extracts the contained data trimmed to the buffer's range.
    pub fn into_inner(self) -> Vec<u8> {
        let Buf { mut buf, body } = self;
        let len = body.end - body.start;
        let _ = buf.drain(..body.start);
        buf.truncate(len);
        buf
    }
}

impl<B> Buf<B> {
    /// Extracts the underlying buffer and the range.
    pub fn into_parts(self) -> (B, Range<usize>) {
        let Buf { buf, body } = self;
        (buf, body)
    }
}

impl<B: AsRef<[u8]>> Buf<B> {
    /// Constructs a new `Buf`.
    ///
    /// `new` constructs a new `Buf` from a buffer and a body range. The bytes
    /// within the range will be the body, the bytes before the range will be
    /// the prefix, and the bytes after the range will be the suffix.
    ///
    /// # Panics
    ///
    /// Panics if `range` is out of bounds of `buf`, or if it is nonsensical
    /// (the end precedes the start).
    pub fn new<R: RangeBounds<usize>>(buf: B, body: R) -> Buf<B> {
        let len = buf.as_ref().len();
        Buf { buf, body: canonicalize_range(len, &body) }
    }

    /// Constructs a [`BufView`] which will be a [`BufferView`] into this `Buf`.
    pub fn buffer_view(&mut self) -> BufView<'_> {
        BufView { buf: &self.buf.as_ref()[self.body.clone()], body: &mut self.body }
    }
}

impl<B: AsRef<[u8]> + AsMut<[u8]>> Buf<B> {
    /// Constructs a [`BufViewMut`] which will be a [`BufferViewMut`] into this `Buf`.
    pub fn buffer_view_mut(&mut self) -> BufViewMut<'_> {
        BufViewMut { buf: &mut self.buf.as_mut()[self.body.clone()], body: &mut self.body }
    }
}

impl<B: AsRef<[u8]>> FragmentedBuffer for Buf<B> {
    fragmented_buffer_method_impls!();
}
impl<B: AsRef<[u8]>> ContiguousBuffer for Buf<B> {}
impl<B: AsRef<[u8]>> ShrinkBuffer for Buf<B> {
    fn shrink<R: RangeBounds<usize>>(&mut self, range: R) {
        let len = self.len();
        let mut range = canonicalize_range(len, &range);
        range.start += self.body.start;
        range.end += self.body.start;
        self.body = range;
    }

    fn shrink_front(&mut self, n: usize) {
        assert!(n <= self.len());
        self.body.start += n;
    }
    fn shrink_back(&mut self, n: usize) {
        assert!(n <= self.len());
        self.body.end -= n;
    }
}
impl<B: AsRef<[u8]>> ParseBuffer for Buf<B> {
    fn parse_with<'a, ParseArgs, P: ParsablePacket<&'a [u8], ParseArgs>>(
        &'a mut self,
        args: ParseArgs,
    ) -> Result<P, P::Error> {
        P::parse(self.buffer_view(), args)
    }
}

impl<B: AsRef<[u8]> + AsMut<[u8]>> FragmentedBufferMut for Buf<B> {
    fragmented_buffer_mut_method_impls!();
}

impl<B: AsRef<[u8]> + AsMut<[u8]>> ParseBufferMut for Buf<B> {
    fn parse_with_mut<'a, ParseArgs, P: ParsablePacket<&'a mut [u8], ParseArgs>>(
        &'a mut self,
        args: ParseArgs,
    ) -> Result<P, P::Error> {
        P::parse_mut(self.buffer_view_mut(), args)
    }
}

impl<B: AsRef<[u8]>> GrowBuffer for Buf<B> {
    fn with_parts<'a, O, F>(&'a self, f: F) -> O
    where
        F: for<'b> FnOnce(&'a [u8], FragmentedBytes<'b, 'a>, &'a [u8]) -> O,
    {
        let (prefix, buf) = self.buf.as_ref().split_at(self.body.start);
        let (body, suffix) = buf.split_at(self.body.end - self.body.start);
        let mut body = [&body[..]];
        f(prefix, body.as_fragmented_byte_slice(), suffix)
    }
    fn capacity(&self) -> usize {
        self.buf.as_ref().len()
    }
    fn prefix_len(&self) -> usize {
        self.body.start
    }
    fn suffix_len(&self) -> usize {
        self.buf.as_ref().len() - self.body.end
    }
    fn grow_front(&mut self, n: usize) {
        assert!(n <= self.body.start);
        self.body.start -= n;
    }
    fn grow_back(&mut self, n: usize) {
        assert!(n <= self.buf.as_ref().len() - self.body.end);
        self.body.end += n;
    }
}

impl<B: AsRef<[u8]> + AsMut<[u8]>> GrowBufferMut for Buf<B> {
    fn with_parts_mut<'a, O, F>(&'a mut self, f: F) -> O
    where
        F: for<'b> FnOnce(&'a mut [u8], FragmentedBytesMut<'b, 'a>, &'a mut [u8]) -> O,
    {
        let (prefix, buf) = self.buf.as_mut().split_at_mut(self.body.start);
        let (body, suffix) = buf.split_at_mut(self.body.end - self.body.start);
        let mut body = [&mut body[..]];
        f(prefix, body.as_fragmented_byte_slice(), suffix)
    }

    fn with_all_contents_mut<'a, O, F>(&'a mut self, f: F) -> O
    where
        F: for<'b> FnOnce(FragmentedBytesMut<'b, 'a>) -> O,
    {
        let mut all = [self.buf.as_mut()];
        f(all.as_fragmented_byte_slice())
    }
}

impl<B: AsRef<[u8]>> AsRef<[u8]> for Buf<B> {
    fn as_ref(&self) -> &[u8] {
        &self.buf.as_ref()[self.body.clone()]
    }
}

impl<B: AsMut<[u8]>> AsMut<[u8]> for Buf<B> {
    fn as_mut(&mut self) -> &mut [u8] {
        &mut self.buf.as_mut()[self.body.clone()]
    }
}

impl<B: AsRef<[u8]>> Buffer for Buf<B> {
    fn parse_with_view<'a, ParseArgs, P: ParsablePacket<&'a [u8], ParseArgs>>(
        &'a mut self,
        args: ParseArgs,
    ) -> Result<(P, &'a [u8]), P::Error> {
        let &mut Self { ref mut body, ref buf } = self;
        let body_before = body.clone();
        let view = BufView { buf: &buf.as_ref()[body.clone()], body };
        P::parse(view, args).map(|r| (r, &buf.as_ref()[body_before]))
    }
}

/// A [`BufferView`] into a [`Buf`].
///
/// A `BufView` is constructed by [`Buf::buffer_view`], and implements
/// `BufferView`, providing a view into the `Buf` from which it was constructed.
pub struct BufView<'a> {
    buf: &'a [u8],
    body: &'a mut Range<usize>,
}

impl<'a> BufferView<&'a [u8]> for BufView<'a> {
    fn take_front(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.len() < n {
            return None;
        }
        self.body.start += n;
        self.buf.split_off(..n)
    }

    fn take_back(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.len() < n {
            return None;
        }
        self.body.end -= n;

        let split = <[u8]>::len(self.buf).checked_sub(n).unwrap();
        self.buf.split_off(split..)
    }

    fn into_rest(self) -> &'a [u8] {
        self.buf
    }
}

impl<'a> AsRef<[u8]> for BufView<'a> {
    fn as_ref(&self) -> &[u8] {
        self.buf
    }
}

/// A [`BufferViewMut`] into a [`Buf`].
///
/// A `BufViewMut` is constructed by [`Buf::buffer_view_mut`], and implements
/// `BufferViewMut`, providing a mutable view into the `Buf` from which it was
/// constructed.
pub struct BufViewMut<'a> {
    buf: &'a mut [u8],
    body: &'a mut Range<usize>,
}

impl<'a> BufferView<&'a mut [u8]> for BufViewMut<'a> {
    fn take_front(&mut self, n: usize) -> Option<&'a mut [u8]> {
        if self.len() < n {
            return None;
        }
        self.body.start += n;
        self.buf.split_off_mut(..n)
    }

    fn take_back(&mut self, n: usize) -> Option<&'a mut [u8]> {
        if self.len() < n {
            return None;
        }
        self.body.end -= n;

        let split = <[u8]>::len(self.buf).checked_sub(n)?;
        Some(self.buf.split_off_mut(split..)?)
    }

    fn into_rest(self) -> &'a mut [u8] {
        self.buf
    }
}

impl<'a> BufferViewMut<&'a mut [u8]> for BufViewMut<'a> {}

impl<'a> AsRef<[u8]> for BufViewMut<'a> {
    fn as_ref(&self) -> &[u8] {
        self.buf
    }
}

impl<'a> AsMut<[u8]> for BufViewMut<'a> {
    fn as_mut(&mut self) -> &mut [u8] {
        self.buf
    }
}

/// The constraints required by a [`PacketBuilder`].
///
/// `PacketConstraints` represents the constraints that must be satisfied in
/// order to serialize a `PacketBuilder`.
///
/// A `PacketConstraints`, `c`, guarantees two properties:
/// - `c.max_body_len() >= c.min_body_len()`
/// - `c.header_len() + c.min_body_len() + c.footer_len()` does not overflow
///   `usize`
///
/// It is not possible (using safe code) to obtain a `PacketConstraints` which
/// violates these properties, so code may rely for its correctness on the
/// assumption that these properties hold.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct PacketConstraints {
    header_len: usize,
    footer_len: usize,
    min_body_len: usize,
    max_body_len: usize,
}

impl PacketConstraints {
    /// A no-op `PacketConstraints` which does not add any constraints - there
    /// is no header, footer, minimum body length requirement, or maximum body
    /// length requirement.
    pub const UNCONSTRAINED: Self =
        Self { header_len: 0, footer_len: 0, min_body_len: 0, max_body_len: usize::MAX };

    /// Constructs a new `PacketConstraints`.
    ///
    /// # Panics
    ///
    /// `new` panics if the arguments violate the validity properties of
    /// `PacketConstraints` - if `max_body_len < min_body_len`, or if
    /// `header_len + min_body_len + footer_len` overflows `usize`.
    #[inline]
    pub fn new(
        header_len: usize,
        footer_len: usize,
        min_body_len: usize,
        max_body_len: usize,
    ) -> PacketConstraints {
        PacketConstraints::try_new(header_len, footer_len, min_body_len, max_body_len).expect(
            "max_body_len < min_body_len or header_len + min_body_len + footer_len overflows usize",
        )
    }

    /// Tries to construct a new `PacketConstraints`.
    ///
    /// `new` returns `None` if the provided values violate the validity
    /// properties of `PacketConstraints` - if `max_body_len < min_body_len`, or
    /// if `header_len + min_body_len + footer_len` overflows `usize`.
    #[inline]
    pub fn try_new(
        header_len: usize,
        footer_len: usize,
        min_body_len: usize,
        max_body_len: usize,
    ) -> Option<PacketConstraints> {
        // Test case 3 in test_packet_constraints
        let header_min_body_footer_overflows = header_len
            .checked_add(min_body_len)
            .and_then(|sum| sum.checked_add(footer_len))
            .is_none();
        // Test case 5 in test_packet_constraints
        let max_less_than_min = max_body_len < min_body_len;
        if max_less_than_min || header_min_body_footer_overflows {
            return None;
        }
        Some(PacketConstraints { header_len, footer_len, min_body_len, max_body_len })
    }

    /// Constructs a new `PacketConstraints` with a given `max_body_len`.
    ///
    /// The `header_len`, `footer_len`, and `min_body_len` are all `0`.
    #[inline]
    pub fn with_max_body_len(max_body_len: usize) -> PacketConstraints {
        // SAFETY:
        // - `max_body_len >= min_body_len` by construction
        // - `header_len + min_body_len + footer_len` is 0 and thus does not
        //   overflow `usize`
        PacketConstraints { header_len: 0, footer_len: 0, min_body_len: 0, max_body_len }
    }

    /// The number of bytes in this packet's header.
    #[inline]
    pub fn header_len(&self) -> usize {
        self.header_len
    }

    /// The number of bytes in this packet's footer.
    #[inline]
    pub fn footer_len(&self) -> usize {
        self.footer_len
    }

    /// The minimum body length (in bytes) required by this packet in order to
    /// avoid adding padding.
    ///
    /// `min_body_len` returns the minimum number of body bytes required in
    /// order to avoid adding padding. Note that, if padding bytes are required,
    /// they may not necessarily belong immediately following the body,
    /// depending on which packet layer imposes the minimum. In particular, in a
    /// nested packet, padding goes after the body of the layer which imposes
    /// the minimum. This means that, if the layer that imposes the minimum is
    /// not the innermost one, then padding must be added not after the
    /// innermost body, but instead in between footers.
    /// [`NestedPacketBuilder::serialize_into`] is responsible for inserting
    /// padding when serializing nested packets.
    ///
    /// If there is no minimum body length, this returns 0.
    #[inline]
    pub fn min_body_len(&self) -> usize {
        self.min_body_len
    }

    /// The maximum length (in bytes) of a body allowed by this packet.
    ///
    /// If there is no maximum body length, this returns [`core::usize::MAX`].
    #[inline]
    pub fn max_body_len(&self) -> usize {
        self.max_body_len
    }

    /// Attempts to encapsulate `self` in `outer`.
    ///
    /// Upon success, `try_encapsulate` returns a `PacketConstraints` which
    /// represents the encapsulation of `self` in `outer`. Its header length,
    /// footer length, minimum body length, and maximum body length are set
    /// accordingly.
    ///
    /// This is probably not the method you want to use; consider
    /// [`Serializer::encapsulate`] instead.
    pub fn try_encapsulate(&self, outer: &Self) -> Option<PacketConstraints> {
        let inner = self;
        // Test case 1 in test_packet_constraints
        let header_len = inner.header_len.checked_add(outer.header_len)?;
        // Test case 2 in test_packet_constraints
        let footer_len = inner.footer_len.checked_add(outer.footer_len)?;
        // This is guaranteed not to overflow by the invariants on
        // PacketConstraint.
        let inner_header_footer_len = inner.header_len + inner.footer_len;
        // Note the saturating_sub here - it's OK if the inner PacketBuilder
        // more than satisfies the outer PacketBuilder's minimum body length
        // requirement.
        let min_body_len = cmp::max(
            outer.min_body_len.saturating_sub(inner_header_footer_len),
            inner.min_body_len,
        );
        // Note the checked_sub here - it's NOT OK if the inner PacketBuilder
        // exceeds the outer PacketBuilder's maximum body length requirement.
        //
        // Test case 4 in test_packet_constraints
        let max_body_len =
            cmp::min(outer.max_body_len.checked_sub(inner_header_footer_len)?, inner.max_body_len);
        // It's still possible that `min_body_len > max_body_len` or that
        // `header_len + min_body_len + footer_len` overflows `usize`; `try_new`
        // checks those constraints for us.
        PacketConstraints::try_new(header_len, footer_len, min_body_len, max_body_len)
    }
}

/// The target buffers into which [`PacketBuilder::serialize`] serializes its
/// header and footer.
pub struct SerializeTarget<'a> {
    #[allow(missing_docs)]
    pub header: &'a mut [u8],
    #[allow(missing_docs)]
    pub footer: &'a mut [u8],
}

/// A builder capable of serializing a packet's headers and footers.
///
/// A `PacketBuilder` describes a packet's headers and footers, and is capable
/// of serializing the header and the footer into an existing buffer via the
/// `serialize` method. A `PacketBuilder` never describes a body.
/// [`NestablePacketBuilder::wrap_body`] must be used to create a packet
/// serializer for a whole packet.
///
/// `()` may be used as an "empty" `PacketBuilder` with no header, footer,
/// minimum body length requirement, or maximum body length requirement.
pub trait PacketBuilder<C: SerializationContext>: NestablePacketBuilder + Sized {
    /// Gets the packet-specific state to use with the [`SerializationContext`].
    fn context_state(&self) -> C::ContextState {
        C::ContextState::default()
    }

    /// Serializes this packet into an existing buffer.
    ///
    /// *This method is usually called by this crate during the serialization of
    /// a [`Serializer`], not directly by the user.*
    ///
    /// # Preconditions
    ///
    /// The caller is responsible for initializing `body` with the body to be
    /// encapsulated, and for ensuring that the body satisfies both the minimum
    /// and maximum body length requirements, possibly by adding padding or by
    /// truncating the body.
    ///
    /// # Postconditions
    ///
    /// `serialize` is responsible for serializing its header and footer into
    /// `target.header` and `target.footer` respectively.
    ///
    /// # Security
    ///
    /// `serialize` must initialize the bytes of the header and footer, even if
    /// only to zero, in order to avoid leaking the contents of packets
    /// previously stored in the same buffer.
    ///
    /// # Panics
    ///
    /// May panic if the `target.header` or `target.footer` are not large enough
    /// to fit the packet's header and footer respectively, or if the body does
    /// not satisfy the minimum or maximum body length requirements.
    fn serialize(
        &self,
        context: &mut C,
        target: &mut SerializeTarget<'_>,
        body: FragmentedBytesMut<'_, '_>,
    );
}

pub trait NestablePacketBuilder: Sized {
    /// Gets the constraints for this `PacketBuilder`.
    fn constraints(&self) -> PacketConstraints;

    /// Wraps given packet `body` in this packet.
    ///
    /// Consumes the [`PacketBuilder`] and the `body`. If the `body` implements
    /// `Serializer` then the result implement `Serializer` as well.
    #[inline]
    fn wrap_body<B>(self, body: B) -> Nested<B, Self> {
        Nested { inner: body, outer: self }
    }
}

impl<'a, B: NestablePacketBuilder> NestablePacketBuilder for &'a B {
    #[inline]
    fn constraints(&self) -> PacketConstraints {
        B::constraints(self)
    }
}

impl<'a, C: SerializationContext, B: PacketBuilder<C>> PacketBuilder<C> for &'a B {
    #[inline]
    fn context_state(&self) -> C::ContextState {
        B::context_state(self)
    }
    #[inline]
    fn serialize(
        &self,
        context: &mut C,
        target: &mut SerializeTarget<'_>,
        body: FragmentedBytesMut<'_, '_>,
    ) {
        B::serialize(self, context, target, body)
    }
}

impl<'a, B: NestablePacketBuilder> NestablePacketBuilder for &'a mut B {
    #[inline]
    fn constraints(&self) -> PacketConstraints {
        B::constraints(self)
    }
}

impl<'a, C: SerializationContext, B: PacketBuilder<C>> PacketBuilder<C> for &'a mut B {
    #[inline]
    fn context_state(&self) -> C::ContextState {
        B::context_state(self)
    }
    #[inline]
    fn serialize(
        &self,
        context: &mut C,
        target: &mut SerializeTarget<'_>,
        body: FragmentedBytesMut<'_, '_>,
    ) {
        B::serialize(self, context, target, body)
    }
}

impl NestablePacketBuilder for () {
    #[inline]
    fn constraints(&self) -> PacketConstraints {
        PacketConstraints::UNCONSTRAINED
    }
}

impl<C: SerializationContext> PacketBuilder<C> for () {
    #[inline]
    fn serialize(
        &self,
        _context: &mut C,
        _target: &mut SerializeTarget<'_>,
        _body: FragmentedBytesMut<'_, '_>,
    ) {
    }
}

impl NestablePacketBuilder for Never {
    fn constraints(&self) -> PacketConstraints {
        match *self {}
    }
}

impl<C: SerializationContext> PacketBuilder<C> for Never {
    fn serialize(
        &self,
        _context: &mut C,
        _target: &mut SerializeTarget<'_>,
        _body: FragmentedBytesMut<'_, '_>,
    ) {
    }
}

/// One object encapsulated in another one.
///
/// `Nested`s are constructed using the [`PacketBuilder::wrap_body`] and
/// [`Serializer::wrap_in`] methods.
///
/// When `I: Serializer` and `O: PacketBuilder`, `Nested<I, O>` implements
/// [`Serializer`].
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct Nested<I, O> {
    inner: I,
    outer: O,
}

impl<I, O> Nested<I, O> {
    /// Consumes this `Nested` and returns the inner object, discarding the
    /// outer one.
    #[inline]
    pub fn into_inner(self) -> I {
        self.inner
    }

    /// Consumes this `Nested` and returns the outer object, discarding the
    /// inner one.
    #[inline]
    pub fn into_outer(self) -> O {
        self.outer
    }

    #[inline]
    pub fn inner(&self) -> &I {
        &self.inner
    }

    #[inline]
    pub fn inner_mut(&mut self) -> &mut I {
        &mut self.inner
    }

    #[inline]
    pub fn outer(&self) -> &O {
        &self.outer
    }

    #[inline]
    pub fn outer_mut(&mut self) -> &mut O {
        &mut self.outer
    }
}

/// A [`PacketBuilder`] which has no header or footer, but which imposes a
/// maximum body length constraint.
///
/// `LimitedSizePacketBuilder`s are constructed using the
/// [`Serializer::with_size_limit`] method.
#[derive(Copy, Clone, Debug)]
#[cfg_attr(test, derive(Eq, PartialEq))]
pub struct LimitedSizePacketBuilder {
    /// The maximum body length.
    pub limit: usize,
}

impl NestablePacketBuilder for LimitedSizePacketBuilder {
    fn constraints(&self) -> PacketConstraints {
        PacketConstraints::with_max_body_len(self.limit)
    }
}

impl<C: SerializationContext> PacketBuilder<C> for LimitedSizePacketBuilder {
    fn serialize(
        &self,
        _context: &mut C,
        _target: &mut SerializeTarget<'_>,
        _body: FragmentedBytesMut<'_, '_>,
    ) {
    }
}

/// A builder capable of serializing packets - which do not encapsulate other
/// packets - into an existing buffer.
///
/// An `InnerPacketBuilder` describes a packet, and is capable of serializing
/// that packet into an existing buffer via the `serialize` method. Unlike the
/// [`PacketBuilder`] trait, it describes a packet which does not encapsulate
/// other packets.
///
/// # Notable implementations
///
/// `InnerPacketBuilder` is implemented for `&[u8]`, `&mut [u8]`, and `Vec<u8>`
/// by treating the contents of the slice/`Vec` as the contents of the packet to
/// be serialized.
pub trait InnerPacketBuilder {
    /// The number of bytes consumed by this packet.
    fn bytes_len(&self) -> usize;

    /// Serializes this packet into an existing buffer.
    ///
    /// `serialize` is called with a buffer of length `self.bytes_len()`, and is
    /// responsible for serializing the packet into the buffer.
    ///
    /// # Security
    ///
    /// All of the bytes of the buffer should be initialized, even if only to
    /// zero, in order to avoid leaking the contents of packets previously
    /// stored in the same buffer.
    ///
    /// # Panics
    ///
    /// May panic if `buffer.len() != self.bytes_len()`.
    fn serialize(&self, buffer: &mut [u8]);

    /// Converts this `InnerPacketBuilder` into a [`Serializer`].
    ///
    /// `into_serializer` is like [`into_serializer_with`], except that no
    /// buffer is provided for reuse in serialization.
    ///
    /// [`into_serializer_with`]: InnerPacketBuilder::into_serializer_with
    #[inline]
    fn into_serializer(self) -> InnerSerializer<Self, EmptyBuf>
    where
        Self: Sized,
    {
        self.into_serializer_with(EmptyBuf)
    }

    /// Converts this `InnerPacketBuilder` into a [`Serializer`] with a buffer
    /// that can be used for serialization.
    ///
    /// `into_serializer_with` consumes a buffer and converts `self` into a type
    /// which implements `Serialize` by treating it as the innermost body to be
    /// contained within any encapsulating [`PacketBuilder`]s. During
    /// serialization, `buffer` will be provided to the [`BufferProvider`],
    /// allowing it to reuse the buffer for serialization and avoid allocating a
    /// new one if possible.
    ///
    /// `buffer` will have its body shrunk to be zero bytes before the
    /// `InnerSerializer` is constructed.
    fn into_serializer_with<B: ShrinkBuffer>(self, mut buffer: B) -> InnerSerializer<Self, B>
    where
        Self: Sized,
    {
        buffer.shrink_back_to(0);
        InnerSerializer { inner: self, buffer }
    }
}

impl<'a, I: InnerPacketBuilder> InnerPacketBuilder for &'a I {
    #[inline]
    fn bytes_len(&self) -> usize {
        I::bytes_len(self)
    }
    #[inline]
    fn serialize(&self, buffer: &mut [u8]) {
        I::serialize(self, buffer)
    }
}
impl<'a, I: InnerPacketBuilder> InnerPacketBuilder for &'a mut I {
    #[inline]
    fn bytes_len(&self) -> usize {
        I::bytes_len(self)
    }
    #[inline]
    fn serialize(&self, buffer: &mut [u8]) {
        I::serialize(self, buffer)
    }
}
impl<'a> InnerPacketBuilder for &'a [u8] {
    #[inline]
    fn bytes_len(&self) -> usize {
        self.len()
    }
    #[inline]
    fn serialize(&self, buffer: &mut [u8]) {
        buffer.copy_from_slice(self);
    }
}
impl<'a> InnerPacketBuilder for &'a mut [u8] {
    #[inline]
    fn bytes_len(&self) -> usize {
        self.len()
    }
    #[inline]
    fn serialize(&self, buffer: &mut [u8]) {
        buffer.copy_from_slice(self);
    }
}
impl<'a> InnerPacketBuilder for Vec<u8> {
    #[inline]
    fn bytes_len(&self) -> usize {
        self.len()
    }
    #[inline]
    fn serialize(&self, buffer: &mut [u8]) {
        buffer.copy_from_slice(self.as_slice());
    }
}
impl<const N: usize> InnerPacketBuilder for ArrayVec<u8, N> {
    fn bytes_len(&self) -> usize {
        self.as_slice().bytes_len()
    }
    fn serialize(&self, buffer: &mut [u8]) {
        self.as_slice().serialize(buffer);
    }
}

/// An [`InnerPacketBuilder`] created from any [`B: SplitByteSlice`].
///
/// `ByteSliceInnerPacketBuilder<B>` implements `InnerPacketBuilder` so long as
/// `B: SplitByteSlice`.
///
/// [`B: SplitByteSlice`]: zerocopy::SplitByteSlice
pub struct ByteSliceInnerPacketBuilder<B>(pub B);

impl<B: SplitByteSlice> InnerPacketBuilder for ByteSliceInnerPacketBuilder<B> {
    fn bytes_len(&self) -> usize {
        self.0.deref().bytes_len()
    }
    fn serialize(&self, buffer: &mut [u8]) {
        self.0.deref().serialize(buffer)
    }
}

impl<B: SplitByteSlice> Debug for ByteSliceInnerPacketBuilder<B> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "ByteSliceInnerPacketBuilder({:?})", self.0.as_ref())
    }
}

/// An error in serializing a packet.
///
/// `SerializeError` is the type of errors returned from methods on the
/// [`Serializer`] trait. The `Alloc` variant indicates that a new buffer could
/// not be allocated, while the `SizeLimitExceeded` variant indicates that a
/// size limit constraint was exceeded.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum SerializeError<A> {
    /// A new buffer could not be allocated.
    Alloc(A),
    /// The size limit constraint was exceeded.
    SizeLimitExceeded,
}

impl<A> SerializeError<A> {
    /// Is this `SerializeError::Alloc`?
    #[inline]
    pub fn is_alloc(&self) -> bool {
        match self {
            SerializeError::Alloc(_) => true,
            SerializeError::SizeLimitExceeded => false,
        }
    }

    /// Is this `SerializeError::SizeLimitExceeded`?
    #[inline]
    pub fn is_size_limit_exceeded(&self) -> bool {
        match self {
            SerializeError::Alloc(_) => false,
            SerializeError::SizeLimitExceeded => true,
        }
    }

    /// Maps the [`SerializeError::Alloc`] error type.
    pub fn map_alloc<T, F: FnOnce(A) -> T>(self, f: F) -> SerializeError<T> {
        match self {
            SerializeError::Alloc(a) => SerializeError::Alloc(f(a)),
            SerializeError::SizeLimitExceeded => SerializeError::SizeLimitExceeded,
        }
    }
}

impl<A> From<A> for SerializeError<A> {
    fn from(a: A) -> SerializeError<A> {
        SerializeError::Alloc(a)
    }
}

/// The error returned when a buffer is too short to hold a serialized packet,
/// and the [`BufferProvider`] is incapable of allocating a new one.
///
/// `BufferTooShortError` is returned by the [`Serializer`] methods
/// [`serialize_no_alloc`] and [`serialize_no_alloc_outer`].
///
/// [`serialize_no_alloc`]: Serializer::serialize_no_alloc
/// [`serialize_no_alloc_outer`]: Serializer::serialize_no_alloc_outer
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct BufferTooShortError;

/// An object capable of providing buffers which satisfy certain constraints.
///
/// A `BufferProvider<Input, Output>` is an object which is capable of consuming
/// a buffer of type `Input` and, either by reusing it or by allocating a new
/// one and copying the input buffer's body into it, producing a buffer of type
/// `Output` which meets certain prefix and suffix length constraints.
///
/// A `BufferProvider` must always be provided when serializing a
/// [`Serializer`].
///
/// Implementors may find the helper function [`try_reuse_buffer`] useful.
///
/// For clients who don't need the full expressive power of this trait, the
/// simpler [`BufferAlloc`] trait is provided. It only defines how to allocate
/// new buffers, and two blanket impls of `BufferProvider` are provided for all
/// `BufferAlloc` types.
pub trait BufferProvider<Input, Output> {
    /// The type of errors returned from [`reuse_or_realloc`].
    ///
    /// [`reuse_or_realloc`]: BufferProvider::reuse_or_realloc
    type Error;

    /// Attempts to produce an output buffer with the given constraints by
    /// allocating a new one.
    ///
    /// `alloc_no_reuse` produces a new buffer with the following invariants:
    /// - The output buffer must have at least `prefix` bytes of prefix
    /// - The output buffer must have at least `suffix` bytes of suffix
    /// - The output buffer must have a body of length `body` bytes.
    ///
    /// If these requirements cannot be met, then an error is returned.
    fn alloc_no_reuse(
        self,
        prefix: usize,
        body: usize,
        suffix: usize,
    ) -> Result<Output, Self::Error>;

    /// Consumes an input buffer and attempts to produce an output buffer with
    /// the given constraints, either by reusing the input buffer or by
    /// allocating a new one and copying the body into it.
    ///
    /// `reuse_or_realloc` consumes a buffer by value, and produces a new buffer
    /// with the following invariants:
    /// - The output buffer must have at least `prefix` bytes of prefix
    /// - The output buffer must have at least `suffix` bytes of suffix
    /// - The output buffer must have the same body as the input buffer
    ///
    /// If these requirements cannot be met, then an error is returned along
    /// with the input buffer, which is unmodified.
    fn reuse_or_realloc(
        self,
        buffer: Input,
        prefix: usize,
        suffix: usize,
    ) -> Result<Output, (Self::Error, Input)>;
}

/// An object capable of allocating new buffers.
///
/// A `BufferAlloc<Output>` is an object which is capable of allocating new
/// buffers of type `Output`.
///
/// [Two blanket implementations] of [`BufferProvider`] are given for any type
/// which implements `BufferAlloc<O>`. One blanket implementation works for any
/// input buffer type, `I`, and produces buffers of type `Either<I, O>` as
/// output. One blanket implementation works only when the input and output
/// buffer types are the same, and produces buffers of that type. See the
/// documentation on those impls for more details.
///
/// The following implementations of `BufferAlloc` are provided:
/// - Any `FnOnce(usize) -> Result<O, E>` implements `BufferAlloc<O, Error = E>`
/// - `()` implements `BufferAlloc<Never, Error = ()>` (an allocator which
///   always fails)
/// - [`new_buf_vec`] implements `BufferAlloc<Buf<Vec<u8>>, Error = Never>` (an
///   allocator which infallibly heap-allocates `Vec`s)
///
/// [Two blanket implementations]: trait.BufferProvider.html#implementors
pub trait BufferAlloc<Output> {
    /// The type of errors returned from [`alloc`].
    ///
    /// [`alloc`]: BufferAlloc::alloc
    type Error;

    /// Attempts to allocate a new buffer of size `len`.
    fn alloc(self, len: usize) -> Result<Output, Self::Error>;
}

impl<O, E, F: FnOnce(usize) -> Result<O, E>> BufferAlloc<O> for F {
    type Error = E;

    #[inline]
    fn alloc(self, len: usize) -> Result<O, E> {
        self(len)
    }
}

impl BufferAlloc<Never> for () {
    type Error = ();

    #[inline]
    fn alloc(self, _len: usize) -> Result<Never, ()> {
        Err(())
    }
}

/// Allocates a new `Buf<Vec<u8>>`.
///
/// `new_buf_vec(len)` is shorthand for `Ok(Buf::new(vec![0; len], ..))`. It
/// implements [`BufferAlloc<Buf<Vec<u8>>, Error = Never>`], and, thanks to a
/// blanket impl, [`BufferProvider<I, Either<I, Buf<Vec<u8>>>, Error = Never>`]
/// for all `I: BufferMut`, and `BufferProvider<Buf<Vec<u8>>, Buf<Vec<u8>>,
/// Error = Never>`.
///
/// [`BufferAlloc<Buf<Vec<u8>>, Error = Never>`]: BufferAlloc
/// [`BufferProvider<I, Either<I, Buf<Vec<u8>>>, Error = Never>`]: BufferProvider
pub fn new_buf_vec(len: usize) -> Result<Buf<Vec<u8>>, Never> {
    Ok(Buf::new(vec![0; len], ..))
}

/// A variant of [`BufferAlloc`] that allocates buffers with the necessary
/// prefix, body, suffix layout.
pub trait LayoutBufferAlloc<O> {
    /// The type of errors returned from [`layout_alloc`].
    ///
    /// [`layout_alloc`]: LayoutBufferAlloc::layout_alloc
    type Error;

    /// Like [`BufferAlloc::layout_alloc`], but the returned buffer has reserved
    /// `prefix` and `suffix` bytes around `body`.
    fn layout_alloc(self, prefix: usize, body: usize, suffix: usize) -> Result<O, Self::Error>;
}

impl<O: ShrinkBuffer, E, F: FnOnce(usize) -> Result<O, E>> LayoutBufferAlloc<O> for F {
    type Error = E;

    #[inline]
    fn layout_alloc(self, prefix: usize, body: usize, suffix: usize) -> Result<O, E> {
        let mut b = self(prefix + body + suffix)?;
        b.shrink_front(prefix);
        b.shrink_back(suffix);
        Ok(b)
    }
}

impl LayoutBufferAlloc<Never> for () {
    type Error = ();

    #[inline]
    fn layout_alloc(self, _prefix: usize, _body: usize, _suffix: usize) -> Result<Never, ()> {
        Err(())
    }
}

/// Attempts to reuse a buffer for the purposes of implementing
/// [`BufferProvider::reuse_or_realloc`].
///
/// `try_reuse_buffer` attempts to reuse an existing buffer to satisfy the given
/// prefix and suffix constraints. If it succeeds, it returns `Ok` containing a
/// buffer with the same body as the input, and with at least `prefix` prefix
/// bytes and at least `suffix` suffix bytes. Otherwise, it returns `Err`
/// containing the original, unmodified input buffer.
///
/// Concretely, `try_reuse_buffer` has the following behavior:
/// - If the prefix and suffix constraints are already met, it returns `Ok` with
///   the input unmodified
/// - If the prefix and suffix constraints are not yet met, then...
///   - If there is enough capacity to meet the constraints and the body is not
///     larger than `max_copy_bytes`, the body will be moved within the buffer
///     in order to meet the constraints, and it will be returned
///   - Otherwise, if there is not enough capacity or the body is larger than
///     `max_copy_bytes`, it returns `Err` with the input unmodified
///
/// `max_copy_bytes` is meant to be an estimate of how many bytes can be copied
/// before allocating a new buffer will be cheaper than copying.
#[inline]
pub fn try_reuse_buffer<B: GrowBufferMut + ShrinkBuffer>(
    mut buffer: B,
    prefix: usize,
    suffix: usize,
    max_copy_bytes: usize,
) -> Result<B, B> {
    let need_prefix = prefix;
    let need_suffix = suffix;
    let have_prefix = buffer.prefix_len();
    let have_body = buffer.len();
    let have_suffix = buffer.suffix_len();
    let need_capacity = need_prefix + have_body + need_suffix;

    if have_prefix >= need_prefix && have_suffix >= need_suffix {
        // We already satisfy the prefix and suffix requirements.
        Ok(buffer)
    } else if buffer.capacity() >= need_capacity && have_body <= max_copy_bytes {
        // The buffer is large enough, but the body is currently too far
        // forward or too far backwards to satisfy the prefix or suffix
        // requirements, so we need to move the body within the buffer.
        buffer.reset();

        // Copy the original body range to a point starting immediatley
        // after `prefix`. This satisfies the `prefix` constraint by
        // definition, and satisfies the `suffix` constraint since we know
        // that the total buffer capacity is sufficient to hold the total
        // length of the prefix, body, and suffix.
        buffer.copy_within(have_prefix..(have_prefix + have_body), need_prefix);
        buffer.shrink(need_prefix..(need_prefix + have_body));
        debug_assert_eq!(buffer.prefix_len(), need_prefix);
        debug_assert!(buffer.suffix_len() >= need_suffix);
        debug_assert_eq!(buffer.len(), have_body);
        Ok(buffer)
    } else {
        Err(buffer)
    }
}

/// Provides an implementation of [`BufferProvider`] from a [`BufferAlloc`] `A`
/// that attempts to reuse the input buffer and falls back to the allocator if
/// the input buffer can't be reused.
pub struct MaybeReuseBufferProvider<A>(pub A);

impl<I: ReusableBuffer, O: ReusableBuffer, A: BufferAlloc<O>> BufferProvider<I, Either<I, O>>
    for MaybeReuseBufferProvider<A>
{
    type Error = A::Error;

    fn alloc_no_reuse(
        self,
        prefix: usize,
        body: usize,
        suffix: usize,
    ) -> Result<Either<I, O>, Self::Error> {
        let Self(alloc) = self;
        let need_capacity = prefix + body + suffix;
        BufferAlloc::alloc(alloc, need_capacity).map(|mut buf| {
            buf.shrink(prefix..(prefix + body));
            Either::B(buf)
        })
    }

    /// If `buffer` has enough capacity to store `need_prefix + need_suffix +
    /// buffer.len()` bytes, then reuse `buffer`. Otherwise, allocate a new
    /// buffer using `A`'s [`BufferAlloc`] implementation.
    ///
    /// If there is enough capacity, but the body is too far forwards or
    /// backwards in the buffer to satisfy the prefix and suffix constraints,
    /// the body will be moved within the buffer in order to satisfy the
    /// constraints. This operation is linear in the length of the body.
    #[inline]
    fn reuse_or_realloc(
        self,
        buffer: I,
        need_prefix: usize,
        need_suffix: usize,
    ) -> Result<Either<I, O>, (A::Error, I)> {
        // TODO(joshlf): Maybe it's worth coming up with a heuristic for when
        // moving the body is likely to be more expensive than allocating
        // (rather than just using `usize::MAX`)? This will be tough since we
        // don't know anything about the performance of `A::alloc`.
        match try_reuse_buffer(buffer, need_prefix, need_suffix, usize::MAX) {
            Ok(buffer) => Ok(Either::A(buffer)),
            Err(buffer) => {
                let have_body = buffer.len();
                let mut buf = match BufferProvider::<I, Either<I, O>>::alloc_no_reuse(
                    self,
                    need_prefix,
                    have_body,
                    need_suffix,
                ) {
                    Ok(buf) => buf,
                    Err(err) => return Err((err, buffer)),
                };

                buf.copy_from(&buffer);
                debug_assert_eq!(buf.prefix_len(), need_prefix);
                debug_assert!(buf.suffix_len() >= need_suffix);
                debug_assert_eq!(buf.len(), have_body);
                Ok(buf)
            }
        }
    }
}

impl<B: ReusableBuffer, A: BufferAlloc<B>> BufferProvider<B, B> for MaybeReuseBufferProvider<A> {
    type Error = A::Error;

    fn alloc_no_reuse(self, prefix: usize, body: usize, suffix: usize) -> Result<B, Self::Error> {
        BufferProvider::<B, Either<B, B>>::alloc_no_reuse(self, prefix, body, suffix)
            .map(Either::into_inner)
    }

    /// If `buffer` has enough capacity to store `need_prefix + need_suffix +
    /// buffer.len()` bytes, then reuse `buffer`. Otherwise, allocate a new
    /// buffer using `A`'s [`BufferAlloc`] implementation.
    ///
    /// If there is enough capacity, but the body is too far forwards or
    /// backwards in the buffer to satisfy the prefix and suffix constraints,
    /// the body will be moved within the buffer in order to satisfy the
    /// constraints. This operation is linear in the length of the body.
    #[inline]
    fn reuse_or_realloc(self, buffer: B, prefix: usize, suffix: usize) -> Result<B, (A::Error, B)> {
        BufferProvider::<B, Either<B, B>>::reuse_or_realloc(self, buffer, prefix, suffix)
            .map(Either::into_inner)
    }
}

/// Provides an implementation of [`BufferProvider`] from a [`BufferAlloc`] `A`
/// that never attempts to reuse the input buffer, and always create a new
/// buffer from the allocator `A`.
pub struct NoReuseBufferProvider<A>(pub A);

impl<I: FragmentedBuffer, O: ReusableBuffer, A: BufferAlloc<O>> BufferProvider<I, O>
    for NoReuseBufferProvider<A>
{
    type Error = A::Error;

    fn alloc_no_reuse(self, prefix: usize, body: usize, suffix: usize) -> Result<O, A::Error> {
        let Self(alloc) = self;
        alloc.alloc(prefix + body + suffix).map(|mut b| {
            b.shrink(prefix..prefix + body);
            b
        })
    }

    fn reuse_or_realloc(self, buffer: I, prefix: usize, suffix: usize) -> Result<O, (A::Error, I)> {
        BufferProvider::<I, O>::alloc_no_reuse(self, prefix, buffer.len(), suffix)
            .map(|mut b| {
                b.copy_from(&buffer);
                b
            })
            .map_err(|e| (e, buffer))
    }
}

/// Context in which packet serialization is performed.
pub trait SerializationContext: Sized {
    /// The packet-specific state required by this serialization context.
    type ContextState: Default;

    /// Performs nested serialization within the context of an outer
    /// [`PacketBuilder`].
    ///
    /// The provided `serialize_fn` is expected to serialize the packet body and
    /// then the packet header and/or footer, each with the provided context.
    ///
    /// The implementor is expected to call `serialize_fn` and return the result
    /// without modification, but it may update its internal state before and/or
    /// after doing so if necessary.
    fn serialize_nested<O: PacketBuilder<Self>, R>(
        &mut self,
        _outer: &O,
        constraints: PacketConstraints,
        serialize_fn: impl FnOnce(&mut Self, PacketConstraints) -> R,
    ) -> R {
        serialize_fn(self, constraints)
    }
}

// An empty serialization context.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct NoOpSerializationContext;

impl SerializationContext for NoOpSerializationContext {
    type ContextState = ();

    // No need to override the serialization method; the default implementation
    // does what we want.
}

pub trait Serializer<C: SerializationContext>: NestableSerializer + Sized {
    /// The type of buffers returned from serialization methods on this trait.
    type Buffer;

    /// Serializes this `Serializer`, producing a buffer.
    ///
    /// As `Serializer`s can be nested using the [`Nested`] type (constructed
    /// using [`NestablePacketBuilder::wrap_body`] and
    /// [`NestableSerializer::wrap_in`]), the `serialize` method is recursive -
    /// calling it on a `Nested` will recurse into the inner `Serializer`, which
    /// might itself be a `Nested`, and so on. `Nested` ensures that the
    /// serialization `context` is passed down the stack of recursive calls.
    /// When the innermost `Serializer` is reached, the contained buffer is
    /// passed to the `provider`, allowing it to decide how to produce a buffer
    /// which is large enough to fit the entire packet - either by reusing the
    /// existing buffer, or by discarding it and allocating a new one.
    /// `constraints` specifies [`PacketConstraints`] for the outer parts of the
    /// packet (header and footer).
    fn serialize<B: GrowBufferMut, P: BufferProvider<Self::Buffer, B>>(
        self,
        context: &mut C,
        constraints: PacketConstraints,
        provider: P,
    ) -> Result<B, (SerializeError<P::Error>, Self)>;

    /// Serializes the data into a new buffer without consuming `self`.
    ///
    /// Creates a new buffer using `alloc` and serializes the data into that
    /// that new buffer. Unlike all other serialize methods,
    /// `serialize_new_buf` takes `self` by reference. This allows to use the
    /// same `Serializer` to serialize the data more than once.
    fn serialize_new_buf<B: GrowBufferMut, A: LayoutBufferAlloc<B>>(
        &self,
        context: &mut C,
        constraints: PacketConstraints,
        alloc: A,
    ) -> Result<B, SerializeError<A::Error>>;

    /// Serializes this `Serializer`, allocating a [`Buf<Vec<u8>>`] if the
    /// contained buffer isn't large enough.
    ///
    /// `serialize_vec` is like [`serialize`], except that, if the contained
    /// buffer isn't large enough to contain the packet, a new `Vec<u8>` is
    /// allocated and wrapped in a [`Buf`]. If the buffer is large enough, but
    /// the body is too far forwards or backwards to fit the encapsulating
    /// headers or footers, the body will be moved within the buffer (this
    /// operation's cost is linear in the size of the body).
    ///
    /// `serialize_vec` is equivalent to calling `serialize` with
    /// [`new_buf_vec`] as the [`BufferProvider`].
    ///
    /// [`Buf<Vec<u8>>`]: Buf
    /// [`serialize`]: Serializer::serialize
    #[inline]
    #[allow(clippy::type_complexity)]
    fn serialize_vec(
        self,
        context: &mut C,
        constraints: PacketConstraints,
    ) -> Result<Either<Self::Buffer, Buf<Vec<u8>>>, (SerializeError<Never>, Self)>
    where
        Self::Buffer: ReusableBuffer,
    {
        self.serialize(context, constraints, MaybeReuseBufferProvider(new_buf_vec))
    }

    /// Serializes this `Serializer`, failing if the existing buffer is not
    /// large enough.
    ///
    /// `serialize_no_alloc` is like [`serialize`], except that it will fail if
    /// the existing buffer isn't large enough. If the buffer is large enough,
    /// but the body is too far forwards or backwards to fit the encapsulating
    /// headers or footers, the body will be moved within the buffer (this
    /// operation's cost is linear in the size of the body).
    ///
    /// `serialize_no_alloc` is equivalent to calling `serialize` with a
    /// `BufferProvider` which cannot allocate a new buffer (such as `()`).
    ///
    /// [`serialize`]: Serializer::serialize
    #[inline]
    fn serialize_no_alloc(
        self,
        context: &mut C,
        constraints: PacketConstraints,
    ) -> Result<Self::Buffer, (SerializeError<BufferTooShortError>, Self)>
    where
        Self::Buffer: ReusableBuffer,
    {
        self.serialize(context, constraints, MaybeReuseBufferProvider(()))
            .map(Either::into_a)
            .map_err(|(err, slf)| {
                (
                    match err {
                        SerializeError::Alloc(()) => BufferTooShortError.into(),
                        SerializeError::SizeLimitExceeded => SerializeError::SizeLimitExceeded,
                    },
                    slf,
                )
            })
    }

    /// Serializes this `Serializer` as the outermost packet.
    ///
    /// `serialize_outer` is like [`serialize`], except that it is called when
    /// this `Serializer` describes the outermost packet, not encapsulated in
    /// any other packets. It is equivalent to calling `serialize` with an empty
    /// [`PacketBuilder`] (such as `()`).
    ///
    /// [`serialize`]: Serializer::serialize
    #[inline]
    fn serialize_outer<B: GrowBufferMut, P: BufferProvider<Self::Buffer, B>>(
        self,
        context: &mut C,
        provider: P,
    ) -> Result<B, (SerializeError<P::Error>, Self)> {
        self.serialize(context, PacketConstraints::UNCONSTRAINED, provider)
    }

    /// Serializes this `Serializer` as the outermost packet, allocating a
    /// [`Buf<Vec<u8>>`] if the contained buffer isn't large enough.
    ///
    /// `serialize_vec_outer` is like [`serialize_vec`], except that it is
    /// called when this `Serializer` describes the outermost packet, not
    /// encapsulated in any other packets. It is equivalent to calling
    /// `serialize_vec` with an empty [`PacketBuilder`] (such as `()`).
    ///
    /// [`Buf<Vec<u8>>`]: Buf
    /// [`serialize_vec`]: Serializer::serialize_vec
    #[inline]
    #[allow(clippy::type_complexity)]
    fn serialize_vec_outer(
        self,
        context: &mut C,
    ) -> Result<Either<Self::Buffer, Buf<Vec<u8>>>, (SerializeError<Never>, Self)>
    where
        Self::Buffer: ReusableBuffer,
    {
        self.serialize_vec(context, PacketConstraints::UNCONSTRAINED)
    }

    /// Serializes this `Serializer` as the outermost packet, failing if the
    /// existing buffer is not large enough.
    ///
    /// `serialize_no_alloc_outer` is like [`serialize_no_alloc`], except that
    /// it is called when this `Serializer` describes the outermost packet, not
    /// encapsulated in any other packets. It is equivalent to calling
    /// `serialize_no_alloc` with an empty [`PacketBuilder`] (such as `()`).
    ///
    /// [`serialize_no_alloc`]: Serializer::serialize_no_alloc
    #[inline]
    fn serialize_no_alloc_outer(
        self,
        context: &mut C,
    ) -> Result<Self::Buffer, (SerializeError<BufferTooShortError>, Self)>
    where
        Self::Buffer: ReusableBuffer,
    {
        self.serialize_no_alloc(context, PacketConstraints::UNCONSTRAINED)
    }

    /// Like [`Serializer::serialize_vec_outer`], but never attempts to reuse
    /// the underlying buffer.
    #[inline]
    fn serialize_vec_outer_no_reuse(
        &self,
        context: &mut C,
    ) -> Result<Buf<Vec<u8>>, SerializeError<Never>> {
        self.serialize_new_buf(context, PacketConstraints::UNCONSTRAINED, new_buf_vec)
    }
}

/// Extension trait for `Serializer` that allows for composition of serializers
/// without type hints by deferring the resolution of the concrete serialization
/// context type.
pub trait NestableSerializer: Sized {
    /// Encapsulates this `Serializer` in a packet, producing a new
    /// `Serializer`.
    ///
    /// `wrap_in()` consumes this `Serializer` and a [`PacketBuilder`], and
    /// produces a new `Serializer` which describes encapsulating this one in
    /// the packet described by `outer`.
    #[inline]
    fn wrap_in<B: NestablePacketBuilder>(self, outer: B) -> Nested<Self, B> {
        outer.wrap_body(self)
    }

    /// Creates a new `Serializer` which will enforce a size limit.
    ///
    /// `with_size_limit` consumes this `Serializer` and limit, and produces a
    /// new `Serializer` which will enforce the given limit on all serialization
    /// requests. Note that the given limit will be enforced at this layer -
    /// serialization requests will be rejected if the body produced by the
    /// request at this layer would exceed the limit. It has no effect on
    /// headers or footers added by encapsulating layers outside of this one.
    #[inline]
    fn with_size_limit(self, limit: usize) -> Nested<Self, LimitedSizePacketBuilder> {
        self.wrap_in(LimitedSizePacketBuilder { limit })
    }
}

/// A [`Serializer`] constructed from an [`InnerPacketBuilder`].
///
/// An `InnerSerializer` wraps an `InnerPacketBuilder` and a buffer, and
/// implements the `Serializer` trait. When a serialization is requested, it
/// either reuses the stored buffer or allocates a new one large enough to hold
/// itself and all outer `PacketBuilder`s.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct InnerSerializer<I, B> {
    inner: I,
    // The buffer's length must be zero since we encapsulate the buffer in a
    // PacketBuilder. If the length were non-zero, that would have the effect of
    // retaining the contents of the buffer when serializing, and putting them
    // immediately after the bytes of `inner`.
    buffer: B,
}

impl<I, B> InnerSerializer<I, B> {
    pub fn inner(&self) -> &I {
        &self.inner
    }
}

/// A wrapper for `InnerPacketBuilders` which implements `PacketBuilder` by
/// treating the entire `InnerPacketBuilder` as the header of the
/// `PacketBuilder`. This allows us to compose our InnerPacketBuilder with
/// the outer `PacketBuilders` into a single, large `PacketBuilder`, and then
/// serialize it using `self.buffer`.
struct InnerPacketBuilderWrapper<I>(I);

impl<I: InnerPacketBuilder> NestablePacketBuilder for InnerPacketBuilderWrapper<I> {
    fn constraints(&self) -> PacketConstraints {
        let Self(wrapped) = self;
        PacketConstraints::new(wrapped.bytes_len(), 0, 0, usize::MAX)
    }
}

impl<C: SerializationContext, I: InnerPacketBuilder> PacketBuilder<C>
    for InnerPacketBuilderWrapper<I>
{
    fn serialize(
        &self,
        _context: &mut C,
        target: &mut SerializeTarget<'_>,
        _body: FragmentedBytesMut<'_, '_>,
    ) {
        let Self(wrapped) = self;

        // Note that the body might be non-empty if an outer
        // PacketBuilder added a minimum body length constraint that
        // required padding.
        debug_assert_eq!(target.header.len(), wrapped.bytes_len());
        debug_assert_eq!(target.footer.len(), 0);

        InnerPacketBuilder::serialize(wrapped, target.header);
    }
}

impl<C: SerializationContext, I: InnerPacketBuilder, B: GrowBuffer + ShrinkBuffer> Serializer<C>
    for InnerSerializer<I, B>
{
    type Buffer = B;

    #[inline]
    fn serialize<BB: GrowBufferMut, P: BufferProvider<B, BB>>(
        self,
        context: &mut C,
        constraints: PacketConstraints,
        provider: P,
    ) -> Result<BB, (SerializeError<P::Error>, InnerSerializer<I, B>)> {
        debug_assert_eq!(self.buffer.len(), 0);
        InnerPacketBuilderWrapper(self.inner)
            .wrap_body(self.buffer)
            .serialize(context, constraints, provider)
            .map_err(|(err, Nested { inner: buffer, outer: pb })| {
                (err, InnerSerializer { inner: pb.0, buffer })
            })
    }

    #[inline]
    fn serialize_new_buf<BB: GrowBufferMut, A: LayoutBufferAlloc<BB>>(
        &self,
        context: &mut C,
        outer: PacketConstraints,
        alloc: A,
    ) -> Result<BB, SerializeError<A::Error>> {
        InnerPacketBuilderWrapper(&self.inner)
            .wrap_body(EmptyBuf)
            .serialize_new_buf(context, outer, alloc)
    }
}

impl<I: InnerPacketBuilder, B: GrowBuffer + ShrinkBuffer> NestableSerializer
    for InnerSerializer<I, B>
{
}

impl<C: SerializationContext, B: GrowBuffer + ShrinkBuffer> Serializer<C> for B {
    type Buffer = B;

    #[inline]
    fn serialize<BB: GrowBufferMut, P: BufferProvider<Self::Buffer, BB>>(
        self,
        context: &mut C,
        constraints: PacketConstraints,
        provider: P,
    ) -> Result<BB, (SerializeError<P::Error>, Self)> {
        TruncatingSerializer::new(self, TruncateDirection::NoTruncating)
            .serialize(context, constraints, provider)
            .map_err(|(err, ser)| (err, ser.buffer))
    }

    fn serialize_new_buf<BB: GrowBufferMut, A: LayoutBufferAlloc<BB>>(
        &self,
        _context: &mut C,
        constraints: PacketConstraints,
        alloc: A,
    ) -> Result<BB, SerializeError<A::Error>> {
        if self.len() > constraints.max_body_len() {
            return Err(SerializeError::SizeLimitExceeded);
        }

        let padding = constraints.min_body_len().saturating_sub(self.len());
        let tail_size = padding + constraints.footer_len();
        let mut buffer = alloc.layout_alloc(constraints.header_len(), self.len(), tail_size)?;
        buffer.copy_from(self);
        buffer.grow_back_zero(padding);
        Ok(buffer)
    }
}

impl<B: GrowBuffer + ShrinkBuffer> NestableSerializer for B {}

/// Either of two serializers.
///
/// An `EitherSerializer` wraps one of two different serializer types.
pub enum EitherSerializer<A, B> {
    A(A),
    B(B),
}

impl<C: SerializationContext, A: Serializer<C>, B: Serializer<C, Buffer = A::Buffer>> Serializer<C>
    for EitherSerializer<A, B>
{
    type Buffer = A::Buffer;

    fn serialize<TB: GrowBufferMut, P: BufferProvider<Self::Buffer, TB>>(
        self,
        context: &mut C,
        constraints: PacketConstraints,
        provider: P,
    ) -> Result<TB, (SerializeError<P::Error>, Self)> {
        match self {
            EitherSerializer::A(s) => s
                .serialize(context, constraints, provider)
                .map_err(|(err, s)| (err, EitherSerializer::A(s))),
            EitherSerializer::B(s) => s
                .serialize(context, constraints, provider)
                .map_err(|(err, s)| (err, EitherSerializer::B(s))),
        }
    }

    fn serialize_new_buf<TB: GrowBufferMut, BA: LayoutBufferAlloc<TB>>(
        &self,
        context: &mut C,
        outer: PacketConstraints,
        alloc: BA,
    ) -> Result<TB, SerializeError<BA::Error>> {
        match self {
            EitherSerializer::A(s) => s.serialize_new_buf(context, outer, alloc),
            EitherSerializer::B(s) => s.serialize_new_buf(context, outer, alloc),
        }
    }
}

impl<A: NestableSerializer, B: NestableSerializer> NestableSerializer for EitherSerializer<A, B> {}

/// The direction a buffer's body should be truncated from to force
/// it to fit within a size limit.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum TruncateDirection {
    /// If a buffer cannot fit within a limit, discard bytes from the
    /// front of the body.
    DiscardFront,
    /// If a buffer cannot fit within a limit, discard bytes from the
    /// end of the body.
    DiscardBack,
    /// Do not attempt to truncate a buffer to make it fit within a limit.
    NoTruncating,
}

/// A [`Serializer`] that truncates its body if it would exceed a size limit.
///
/// `TruncatingSerializer` wraps a buffer, and implements `Serializer`. Unlike
/// the blanket impl of `Serializer` for `B: GrowBuffer + ShrinkBuffer`, if the
/// buffer's body exceeds the size limit constraint passed to
/// `Serializer::serialize`, the body is truncated to fit.
///
/// Note that this does not guarantee that size limit exceeded errors will not
/// occur. The size limit may be small enough that the encapsulating headers
/// alone exceed the size limit.  There may also be a minimum body length
/// constraint which is larger than the size limit.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct TruncatingSerializer<B> {
    buffer: B,
    direction: TruncateDirection,
}

impl<B> TruncatingSerializer<B> {
    /// Constructs a new `TruncatingSerializer`.
    pub fn new(buffer: B, direction: TruncateDirection) -> TruncatingSerializer<B> {
        TruncatingSerializer { buffer, direction }
    }

    /// Provides shared access to the inner buffer.
    pub fn buffer(&self) -> &B {
        &self.buffer
    }

    /// Provides mutable access to the inner buffer.
    pub fn buffer_mut(&mut self) -> &mut B {
        &mut self.buffer
    }
}

impl<C: SerializationContext, B: GrowBuffer + ShrinkBuffer> Serializer<C>
    for TruncatingSerializer<B>
{
    type Buffer = B;

    fn serialize<BB: GrowBufferMut, P: BufferProvider<B, BB>>(
        mut self,
        _context: &mut C,
        constraints: PacketConstraints,
        provider: P,
    ) -> Result<BB, (SerializeError<P::Error>, Self)> {
        let original_len = self.buffer.len();
        let excess_bytes = if original_len > constraints.max_body_len() {
            Some(original_len - constraints.max_body_len())
        } else {
            None
        };
        if let Some(excess_bytes) = excess_bytes {
            match self.direction {
                TruncateDirection::DiscardFront => self.buffer.shrink_front(excess_bytes),
                TruncateDirection::DiscardBack => self.buffer.shrink_back(excess_bytes),
                TruncateDirection::NoTruncating => {
                    return Err((SerializeError::SizeLimitExceeded, self));
                }
            }
        }

        let padding = constraints.min_body_len().saturating_sub(self.buffer.len());

        // At this point, the body and padding MUST fit within the limit. Note
        // that PacketConstraints guarantees that min_body_len <= max_body_len,
        // so the padding can't cause this assertion to fail.
        debug_assert!(self.buffer.len() + padding <= constraints.max_body_len());
        match provider.reuse_or_realloc(
            self.buffer,
            constraints.header_len(),
            padding + constraints.footer_len(),
        ) {
            Ok(buffer) => Ok(buffer),
            Err((err, mut buffer)) => {
                // Undo the effects of shrinking the buffer so that the buffer
                // we return is unmodified from its original (which is required
                // by the contract of this method).
                if let Some(excess_bytes) = excess_bytes {
                    match self.direction {
                        TruncateDirection::DiscardFront => buffer.grow_front(excess_bytes),
                        TruncateDirection::DiscardBack => buffer.grow_back(excess_bytes),
                        TruncateDirection::NoTruncating => unreachable!(),
                    }
                }

                Err((
                    SerializeError::Alloc(err),
                    TruncatingSerializer { buffer, direction: self.direction },
                ))
            }
        }
    }

    fn serialize_new_buf<BB: GrowBufferMut, A: LayoutBufferAlloc<BB>>(
        &self,
        _context: &mut C,
        outer: PacketConstraints,
        alloc: A,
    ) -> Result<BB, SerializeError<A::Error>> {
        let truncated_size = cmp::min(self.buffer.len(), outer.max_body_len());
        let discarded_bytes = self.buffer.len() - truncated_size;
        let padding = outer.min_body_len().saturating_sub(truncated_size);
        let tail_size = padding + outer.footer_len();
        let mut buffer = alloc.layout_alloc(outer.header_len(), truncated_size, tail_size)?;
        buffer.with_bytes_mut(|mut dst| {
            self.buffer.with_bytes(|src| {
                let src = match (discarded_bytes > 0, self.direction) {
                    (false, _) => src,
                    (true, TruncateDirection::DiscardFront) => src.slice(discarded_bytes..),
                    (true, TruncateDirection::DiscardBack) => src.slice(..truncated_size),
                    (true, TruncateDirection::NoTruncating) => {
                        return Err(SerializeError::SizeLimitExceeded);
                    }
                };
                dst.copy_from(&src);
                Ok(())
            })
        })?;
        buffer.grow_back_zero(padding);
        Ok(buffer)
    }
}

impl<B: GrowBuffer + ShrinkBuffer> NestableSerializer for TruncatingSerializer<B> {}

impl<C: SerializationContext, I: Serializer<C>, O: PacketBuilder<C>> Serializer<C>
    for Nested<I, O>
{
    type Buffer = I::Buffer;

    #[inline]
    fn serialize<B: GrowBufferMut, P: BufferProvider<I::Buffer, B>>(
        self,
        context: &mut C,
        constraints: PacketConstraints,
        provider: P,
    ) -> Result<B, (SerializeError<P::Error>, Self)> {
        context
            .serialize_nested(&self.outer, constraints, |context, constraints| {
                let Some(constraints) = self.outer.constraints().try_encapsulate(&constraints)
                else {
                    return Err((SerializeError::SizeLimitExceeded, self.inner));
                };
                self.inner.serialize(context, constraints, provider).map(|mut buf| {
                    buf.serialize(context, &self.outer);
                    buf
                })
            })
            .map_err(|(err, inner)| (err, self.outer.wrap_body(inner)))
    }

    #[inline]
    fn serialize_new_buf<B: GrowBufferMut, A: LayoutBufferAlloc<B>>(
        &self,
        context: &mut C,
        constraints: PacketConstraints,
        alloc: A,
    ) -> Result<B, SerializeError<A::Error>> {
        context.serialize_nested(&self.outer, constraints, |context, constraints| {
            let Some(constraints) = self.outer.constraints().try_encapsulate(&constraints) else {
                return Err(SerializeError::SizeLimitExceeded);
            };
            self.inner.serialize_new_buf(context, constraints, alloc).map(|mut buf| {
                buf.serialize(context, &self.outer);
                buf
            })
        })
    }
}

impl<I: NestableSerializer, O: NestablePacketBuilder> NestableSerializer for Nested<I, O> {}

/// A packet builder used for partial packet serialization.
pub trait PartialPacketBuilder<C: SerializationContext>: PacketBuilder<C> {
    /// Serializes the header to the specified `buffer`.
    ///
    /// Checksums (if any) should not calculated. The corresponding fields
    /// should be set to 0.
    ///
    /// `body_len` specifies size of the packet body wrapped by this
    /// `PacketBuilder`. It is supplied so the correct packet size can be
    /// written in the header.
    fn partial_serialize(&self, context: &mut C, body_len: usize, buffer: &mut [u8]);
}

impl<C: SerializationContext> PartialPacketBuilder<C> for () {
    fn partial_serialize(&self, _context: &mut C, _body_len: usize, _buffer: &mut [u8]) {}
}

/// Result returned by `PartialSerializer::partial_serialize`.
#[derive(Debug, Eq, PartialEq)]
pub enum PartialSerializeResult<'a, B> {
    Slice(&'a [u8]),
    NewBuffer { buffer: B, total_size: usize },
}

/// A serializer that supports partial serialization.
///
/// Partial serialization allows to serialize only packet headers without
/// calculating packet checksums (if any).
pub trait PartialSerializer<C: SerializationContext> {
    /// If the packet is already serialized then returns the whole seialized
    /// packet as `PartialSerializeResult::Slice`. Otherwise serializes the
    /// headers into a new buffer returned as `PartialSerializeResult::NewBuffer`.
    fn partial_serialize<B: GrowBufferMut + ContiguousBuffer, A: LayoutBufferAlloc<B>>(
        &self,
        context: &mut C,
        alloc: A,
    ) -> Result<PartialSerializeResult<'_, B>, SerializeError<A::Error>> {
        let (buffer, total_size) =
            self.partial_serialize_new_buf(context, PacketConstraints::UNCONSTRAINED, alloc)?;
        Ok(PartialSerializeResult::NewBuffer { buffer, total_size })
    }

    /// Serializes the headers into a new buffer allocated used `alloc`.
    ///
    /// Returns a buffer with serialized headers. If the serializer doesn't
    /// serialize any headers then an empty buffer is returned. In either case,
    /// the buffer is guaranteed to contain exactly `constraints.header_len()`
    /// at the head.
    ///
    /// The second returned value indicates total number of bytes in the
    /// packet, including the headers.
    fn partial_serialize_new_buf<B: GrowBufferMut, A: LayoutBufferAlloc<B>>(
        &self,
        context: &mut C,
        constraints: PacketConstraints,
        alloc: A,
    ) -> Result<(B, usize), SerializeError<A::Error>>;
}

impl<C, B> PartialSerializer<C> for B
where
    C: SerializationContext,
    B: GrowBuffer + ShrinkBuffer,
{
    fn partial_serialize_new_buf<BB: GrowBufferMut, A: LayoutBufferAlloc<BB>>(
        &self,
        _context: &mut C,
        constraints: PacketConstraints,
        alloc: A,
    ) -> Result<(BB, usize), SerializeError<A::Error>> {
        let buffer = alloc.layout_alloc(constraints.header_len(), 0, 0)?;
        Ok((buffer, self.len()))
    }
}

impl<C, B> PartialSerializer<C> for TruncatingSerializer<B>
where
    C: SerializationContext,
    B: GrowBuffer + ShrinkBuffer,
{
    fn partial_serialize_new_buf<BB: GrowBufferMut, A: LayoutBufferAlloc<BB>>(
        &self,
        _context: &mut C,
        constraints: PacketConstraints,
        alloc: A,
    ) -> Result<(BB, usize), SerializeError<A::Error>> {
        let total_size = cmp::max(
            constraints.min_body_len(),
            cmp::min(self.buffer().len(), constraints.max_body_len()),
        );
        let buffer = alloc.layout_alloc(constraints.header_len(), 0, 0)?;
        Ok((buffer, total_size))
    }
}

impl<C, I, B> PartialSerializer<C> for InnerSerializer<I, B>
where
    C: SerializationContext,
    I: InnerPacketBuilder,
    B: GrowBuffer + ShrinkBuffer,
{
    fn partial_serialize_new_buf<BB: GrowBufferMut, A: LayoutBufferAlloc<BB>>(
        &self,
        _context: &mut C,
        constraints: PacketConstraints,
        alloc: A,
    ) -> Result<(BB, usize), SerializeError<A::Error>> {
        let total_size = cmp::max(self.inner().bytes_len(), constraints.min_body_len());
        let buffer = alloc.layout_alloc(constraints.header_len(), 0, 0)?;
        Ok((buffer, total_size))
    }
}

impl<C, A, B> PartialSerializer<C> for EitherSerializer<A, B>
where
    C: SerializationContext,
    A: PartialSerializer<C>,
    B: PartialSerializer<C>,
{
    fn partial_serialize_new_buf<BB: GrowBufferMut, AA: LayoutBufferAlloc<BB>>(
        &self,
        context: &mut C,
        constraints: PacketConstraints,
        alloc: AA,
    ) -> Result<(BB, usize), SerializeError<AA::Error>> {
        match self {
            EitherSerializer::A(s) => s.partial_serialize_new_buf(context, constraints, alloc),
            EitherSerializer::B(s) => s.partial_serialize_new_buf(context, constraints, alloc),
        }
    }
}

impl<C, I, O> PartialSerializer<C> for Nested<I, O>
where
    C: SerializationContext,
    I: PartialSerializer<C>,
    O: PartialPacketBuilder<C>,
{
    fn partial_serialize_new_buf<B: GrowBufferMut, A: LayoutBufferAlloc<B>>(
        &self,
        context: &mut C,
        constraints: PacketConstraints,
        alloc: A,
    ) -> Result<(B, usize), SerializeError<A::Error>> {
        context.serialize_nested(&self.outer, constraints, |context, constraints| {
            let header_constraints = self.outer.constraints();
            let Some(constraints) = header_constraints.try_encapsulate(&constraints) else {
                return Err(SerializeError::SizeLimitExceeded);
            };
            let header_len = header_constraints.header_len();
            let (mut buffer, mut total_size) =
                self.inner.partial_serialize_new_buf(context, constraints, alloc)?;
            buffer.with_parts_mut(|prefix, _body, _suffix| {
                let header_offset = prefix.len() - header_len;
                let header = &mut prefix[header_offset..];
                self.outer.partial_serialize(context, total_size, header);
            });
            buffer.grow_front(header_len);
            total_size += header_len + header_constraints.footer_len();
            Ok((buffer, total_size))
        })
    }
}

mod sealed {
    use super::*;

    /// The inner workings of [`DynamicSerializer`].
    ///
    /// This trait is sealed because we don't want it to be implementable
    /// outside this crate or for its methods to be callable.
    pub trait DynamicSerializerInner<C: SerializationContext> {
        /// Serializes this serializer using a dyn borrow to an allocator.
        ///
        /// This method behaves much like [`Serializer::serialize_new_buf`], but
        /// with a specific shape allowing for dynamic dispatch.
        ///
        /// The target buffer is allocated via [`DynamicBufferAlloc`] and,
        /// instead of returning an owned buffer, it returns the total number of
        /// bytes in `prefix`, `suffix` that the buffer taken from the allocator
        /// _must have_ after having serialized this entity.
        fn serialize_dyn_alloc(
            &self,
            context: &mut C,
            outer: PacketConstraints,
            alloc: &mut dyn DynamicBufferAlloc,
        ) -> Result<(usize, usize), SerializeError<DynAllocError>>;
    }

    pub trait DynamicPartialSerializerInner<C: SerializationContext> {
        fn partial_serialize_dyn_alloc(
            &self,
            context: &mut C,
            constraints: PacketConstraints,
            alloc: &mut dyn DynamicBufferAlloc,
        ) -> Result<usize, SerializeError<DynAllocError>>;
    }

    /// Type-erased allocator allowing dynamic serializers through
    /// [`DynamicSerializerInner`].
    ///
    /// This has roughly the same shape as [`LayoutBufferAlloc`], but with
    /// dynamic dispatch capabilities.
    pub trait DynamicBufferAlloc {
        /// Allocates a buffer with `prefix`, `body`, `suffix` bytes, like
        /// [`LayoutBufferAlloc::layout_alloc`].
        ///
        /// Note that the returned buffer has a tied lifetime to the allocator.
        /// The type erasure here is achieved by storing the buffer within the
        /// allocator itself, which can then be extracted to fulfill the
        /// `Serializer` trait. See the `Adapter` implementations supporting
        /// [`DynamicSerializerInner`] for details.
        ///
        /// This trait is sealed because we don't want it to be implementable
        /// outside this crate or for its methods to be callable.
        ///
        /// `alloc` may only be called once per instance of
        /// `DynamicBufferAlloc`. It reflects the single-use nature of
        /// [`LayoutBufferAlloc`], but methods taking `self` is not dyn
        /// compatible. Implementors may panic if called more than once on the
        /// same instance.
        fn alloc(
            &mut self,
            prefix: usize,
            body: usize,
            suffix: usize,
        ) -> Result<Buf<&mut [u8]>, DynAllocError>;
    }

    /// The temporary errors returned by dynamic helpers in
    /// [`DynamicSerializerInner`] and [`DynamicBufferAlloc`].
    pub struct DynAllocError;
}

use sealed::{
    DynAllocError, DynamicBufferAlloc, DynamicPartialSerializerInner, DynamicSerializerInner,
};

/// `DynamicBufferAlloc` implementation that wraps an `LayoutBufferAlloc`
enum DynBufferAlloc<A: LayoutBufferAlloc<B>, B> {
    Empty,
    Alloc(A),
    Buffer(B),
    Error(A::Error),
}

impl<A: LayoutBufferAlloc<B>, B> DynBufferAlloc<A, B> {
    fn take_buffer(self) -> B {
        let DynBufferAlloc::Buffer(b) = self else { unreachable!("unexpected alloc state") };
        b
    }

    fn take_error(self) -> A::Error {
        let DynBufferAlloc::Error(e) = self else { unreachable!("unexpected alloc state") };
        e
    }
}

impl<A: LayoutBufferAlloc<B>, B: GrowBufferMut> DynamicBufferAlloc for DynBufferAlloc<A, B> {
    fn alloc(
        &mut self,
        prefix: usize,
        body: usize,
        suffix: usize,
    ) -> Result<Buf<&mut [u8]>, DynAllocError> {
        let alloc = match core::mem::replace(self, Self::Empty) {
            Self::Alloc(a) => a,
            _ => panic!("unexpected alloc state"),
        };

        let buffer = match alloc.layout_alloc(prefix, body, suffix) {
            Ok(b) => b,
            Err(e) => {
                *self = Self::Error(e);
                return Err(DynAllocError);
            }
        };
        *self = Self::Buffer(buffer);
        let buffer = match self {
            Self::Buffer(b) => b.with_all_contents_mut(|b| match b.try_into_contiguous() {
                Ok(b) => b,
                Err(_) => {
                    todo!("https://fxbug.dev/428952155: support dyn serialize fragmented buffers")
                }
            }),
            // We just set buffer above.
            _ => unreachable!(),
        };
        Ok(Buf::new(buffer, prefix..(buffer.len() - suffix)))
    }
}

fn dyn_serialize_new_buf<C: SerializationContext, B: GrowBufferMut, A: LayoutBufferAlloc<B>>(
    serializer: &dyn DynamicSerializerInner<C>,
    context: &mut C,
    outer: PacketConstraints,
    alloc: A,
) -> Result<B, SerializeError<A::Error>> {
    let mut alloc = DynBufferAlloc::Alloc(alloc);
    let (prefix, suffix) = match serializer.serialize_dyn_alloc(context, outer, &mut alloc) {
        Ok(result) => result,
        Err(SerializeError::SizeLimitExceeded) => return Err(SerializeError::SizeLimitExceeded),
        Err(SerializeError::Alloc(DynAllocError)) => {
            return Err(SerializeError::Alloc(alloc.take_error()));
        }
    };

    let mut buffer = alloc.take_buffer();
    buffer.grow_front(buffer.prefix_len().checked_sub(prefix).unwrap_or_else(|| {
        panic!("failed to grow buffer front; want: {} got: {}", prefix, buffer.prefix_len())
    }));
    buffer.grow_back(buffer.suffix_len().checked_sub(suffix).unwrap_or_else(|| {
        panic!("failed to grow buffer back; want: {} got: {}", suffix, buffer.suffix_len())
    }));
    Ok(buffer)
}

/// A type that provides [`Serializer`] via dynamic dispatch.
///
/// See discussion on [`DynamicSerializer`] for when dynamically dispatched
/// serializers can be beneficial.
#[derive(Copy, Clone)]
pub struct DynSerializer<'a, C: SerializationContext>(&'a dyn DynamicSerializerInner<C>);

impl<'a, C: SerializationContext> DynSerializer<'a, C> {
    /// Creates a new `DynSerializer` from a borrow to a concrete serializer.
    pub fn new<S: Serializer<C>>(s: &'a S) -> Self {
        Self::new_dyn(s)
    }

    /// Creates a new `DynSerializer` from a fat `DynamicSerializer` pointer.
    pub fn new_dyn(s: &'a dyn DynamicSerializer<C>) -> Self {
        Self(s)
    }
}

impl<C: SerializationContext> Serializer<C> for DynSerializer<'_, C> {
    type Buffer = EmptyBuf;

    fn serialize<B: GrowBufferMut, P: BufferProvider<Self::Buffer, B>>(
        self,
        context: &mut C,
        constraints: PacketConstraints,
        provider: P,
    ) -> Result<B, (SerializeError<P::Error>, Self)> {
        struct Adapter<S, P>(P, PhantomData<S>);

        impl<S, B, P> LayoutBufferAlloc<B> for Adapter<S, P>
        where
            P: BufferProvider<S, B>,
        {
            type Error = P::Error;

            fn layout_alloc(
                self,
                prefix: usize,
                body: usize,
                suffix: usize,
            ) -> Result<B, Self::Error> {
                let Self(provider, PhantomData) = self;
                provider.alloc_no_reuse(prefix, body, suffix)
            }
        }

        let Self(serializer) = self;
        match dyn_serialize_new_buf(
            serializer,
            context,
            constraints,
            Adapter(provider, PhantomData),
        ) {
            Ok(b) => Ok(b),
            Err(e) => Err((e, self)),
        }
    }

    fn serialize_new_buf<B: GrowBufferMut, A: LayoutBufferAlloc<B>>(
        &self,
        context: &mut C,
        constraints: PacketConstraints,
        alloc: A,
    ) -> Result<B, SerializeError<A::Error>> {
        let Self(serializer) = self;
        dyn_serialize_new_buf(*serializer, context, constraints, alloc)
    }
}

// `LayoutBufferAlloc` implementation that wraps a `DynamicBufferAlloc`.
struct DynamicBufferAllocAdapter<'a>(&'a mut dyn DynamicBufferAlloc);
impl<'a> LayoutBufferAlloc<Buf<&'a mut [u8]>> for DynamicBufferAllocAdapter<'a> {
    type Error = DynAllocError;

    fn layout_alloc(
        self,
        prefix: usize,
        body: usize,
        suffix: usize,
    ) -> Result<Buf<&'a mut [u8]>, Self::Error> {
        let Self(inner) = self;
        inner.alloc(prefix, body, suffix)
    }
}

impl<C: SerializationContext> NestableSerializer for DynSerializer<'_, C> {}

impl<C: SerializationContext, O: Serializer<C>> DynamicSerializerInner<C> for O {
    fn serialize_dyn_alloc(
        &self,
        context: &mut C,
        outer: PacketConstraints,
        alloc: &mut dyn DynamicBufferAlloc,
    ) -> Result<(usize, usize), SerializeError<DynAllocError>> {
        self.serialize_new_buf(context, outer, DynamicBufferAllocAdapter(alloc))
            .map(|buffer| (buffer.prefix_len(), buffer.suffix_len()))
    }
}

/// A marker trait that is used as an attestation of dynamic serialization
/// capabilities.
///
/// Use [`DynSerializer`] to create instances of dynamic serializers.
///
/// # Discussion
///
/// If serializers are passed deep down the call stack, causing local
/// instantiation of multiple functions, it might be beneficial to consider
/// using a dynamically dispatched serializer instead. The hit taken during code
/// generation (and compilation times) might not be worth it, depending on the
/// task at hand. As an example, slow-path protocols might not derive much
/// benefit from deep compiler optimization which tips the scales in favor of
/// using a dynamically dispatched serializer instead.
pub trait DynamicSerializer<C: SerializationContext>: DynamicSerializerInner<C> {}
impl<C: SerializationContext, O: DynamicSerializerInner<C>> DynamicSerializer<C> for O {}

#[derive(Copy, Clone)]
pub struct DynPartialSerializer<'a, C: SerializationContext>(
    &'a dyn DynamicPartialSerializerInner<C>,
);

impl<'a, C: SerializationContext> DynPartialSerializer<'a, C> {
    pub fn new_dyn(s: &'a dyn DynamicPartialSerializerInner<C>) -> Self {
        Self(s)
    }
}

impl<'a, C: SerializationContext> PartialSerializer<C> for DynPartialSerializer<'a, C> {
    fn partial_serialize_new_buf<B: GrowBufferMut, A: LayoutBufferAlloc<B>>(
        &self,
        context: &mut C,
        constraints: PacketConstraints,
        alloc: A,
    ) -> Result<(B, usize), SerializeError<A::Error>> {
        let Self(inner) = self;
        let mut alloc = DynBufferAlloc::Alloc(alloc);
        let total_size = match inner.partial_serialize_dyn_alloc(context, constraints, &mut alloc) {
            Ok(result) => result,
            Err(SerializeError::SizeLimitExceeded) => {
                return Err(SerializeError::SizeLimitExceeded);
            }
            Err(SerializeError::Alloc(DynAllocError)) => {
                return Err(SerializeError::Alloc(alloc.take_error()));
            }
        };

        let mut buffer = alloc.take_buffer();
        buffer.grow_front(
            buffer.prefix_len().checked_sub(constraints.header_len()).unwrap_or_else(|| {
                panic!(
                    "failed to grow buffer front; want: {} got: {}",
                    constraints.header_len(),
                    buffer.prefix_len()
                )
            }),
        );
        Ok((buffer, total_size))
    }
}

impl<C: SerializationContext, O: PartialSerializer<C>> DynamicPartialSerializerInner<C> for O {
    fn partial_serialize_dyn_alloc(
        &self,
        context: &mut C,
        constraints: PacketConstraints,
        alloc: &mut dyn DynamicBufferAlloc,
    ) -> Result<usize, SerializeError<DynAllocError>> {
        self.partial_serialize_new_buf(context, constraints, DynamicBufferAllocAdapter(alloc))
            .map(|(_buf, total_size)| total_size)
    }
}
pub trait DynamicPartialSerializer<C: SerializationContext>:
    DynamicPartialSerializerInner<C>
{
}

impl<C: SerializationContext, O: DynamicPartialSerializerInner<C>> DynamicPartialSerializer<C>
    for O
{
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BufferMut;
    use assert_matches::assert_matches;
    use std::fmt::Debug;
    use test_case::test_case;
    use test_util::{assert_geq, assert_leq};

    fn dirty_buf_alloc(len: usize) -> Result<Buf<Vec<u8>>, Never> {
        Ok(Buf::new(vec![0xAA; len], ..))
    }

    // DummyPacketBuilder:
    // - Implements PacketBuilder with the stored constraints; it fills the
    //   header with header_byte and the footer with footer_byte
    // - Implements InnerPacketBuilder by consuming a `header_len`-bytes body,
    //   and filling it with header_byte
    #[derive(Copy, Clone, Debug, Eq, PartialEq)]
    struct DummyPacketBuilder {
        header_len: usize,
        footer_len: usize,
        min_body_len: usize,
        max_body_len: usize,
        header_byte: u8,
        footer_byte: u8,
    }

    impl DummyPacketBuilder {
        fn new(
            header_len: usize,
            footer_len: usize,
            min_body_len: usize,
            max_body_len: usize,
        ) -> DummyPacketBuilder {
            DummyPacketBuilder {
                header_len,
                footer_len,
                min_body_len,
                max_body_len,
                header_byte: 0xFF,
                footer_byte: 0xFE,
            }
        }
    }

    impl NestablePacketBuilder for DummyPacketBuilder {
        fn constraints(&self) -> PacketConstraints {
            PacketConstraints::new(
                self.header_len,
                self.footer_len,
                self.min_body_len,
                self.max_body_len,
            )
        }
    }

    impl<C: SerializationContext> PacketBuilder<C> for DummyPacketBuilder {
        fn serialize(
            &self,
            _context: &mut C,
            target: &mut SerializeTarget<'_>,
            body: FragmentedBytesMut<'_, '_>,
        ) {
            assert_eq!(target.header.len(), self.header_len);
            assert_eq!(target.footer.len(), self.footer_len);
            assert!(body.len() >= self.min_body_len);
            assert!(body.len() <= self.max_body_len);
            target.header.fill(self.header_byte);
            target.footer.fill(self.footer_byte);
        }
    }

    impl<C: SerializationContext> PartialPacketBuilder<C> for DummyPacketBuilder {
        fn partial_serialize(&self, _context: &mut C, _body_len: usize, buffer: &mut [u8]) {
            buffer.fill(self.header_byte)
        }
    }

    impl InnerPacketBuilder for DummyPacketBuilder {
        fn bytes_len(&self) -> usize {
            self.header_len
        }

        fn serialize(&self, buffer: &mut [u8]) {
            assert_eq!(buffer.len(), self.header_len);
            buffer.fill(self.header_byte);
        }
    }

    // Helper for `VerifyingSerializer` used to verify the serialization result.
    #[derive(Copy, Clone, Debug, Eq, PartialEq)]
    struct SerializerVerifier {
        // Total size if the inner body if not truncated or `None` if
        // serialization is expected to fail due to size limit.
        inner_len: Option<usize>,

        // Is the inner Serializer truncating (a TruncatingSerializer with
        // TruncateDirection::DiscardFront or DiscardBack)?
        truncating: bool,
    }

    impl SerializerVerifier {
        fn new<S: Serializer<NoOpSerializationContext>>(serializer: &S, truncating: bool) -> Self {
            let inner_len = serializer
                .serialize_new_buf(
                    &mut NoOpSerializationContext,
                    PacketConstraints::UNCONSTRAINED,
                    new_buf_vec,
                )
                .map(|buf| buf.len())
                .inspect_err(|err| assert!(err.is_size_limit_exceeded()))
                .ok();
            Self { inner_len, truncating }
        }

        fn verify_result<B: GrowBufferMut, A>(
            &self,
            result: Result<&B, &SerializeError<A>>,
            outer: PacketConstraints,
        ) {
            let should_exceed_size_limit = match self.inner_len {
                Some(inner_len) => outer.max_body_len() < inner_len && !self.truncating,
                None => true,
            };

            match result {
                Ok(buf) => {
                    assert_geq!(buf.prefix_len(), outer.header_len());
                    assert_geq!(buf.suffix_len(), outer.footer_len());
                    assert_leq!(buf.len(), outer.max_body_len());

                    // It is `Serialize::serialize()`'s responsibility to ensure that there
                    // is enough suffix room to fit any post-body padding and the footer,
                    // but it is the caller's responsibility to actually add that padding
                    // (ie, move it from the suffix to the body).
                    let padding = outer.min_body_len().saturating_sub(buf.len());
                    assert_leq!(padding + outer.footer_len(), buf.suffix_len());

                    assert!(!should_exceed_size_limit);
                }
                Err(err) => {
                    // If we shouldn't fail as a result of a size limit exceeded
                    // error, we might still fail as a result of allocation.
                    if should_exceed_size_limit {
                        assert!(err.is_size_limit_exceeded());
                    } else {
                        assert!(err.is_alloc());
                    }
                }
            }
        }
    }

    // A Serializer that verifies certain invariants while operating. In
    // particular:
    // - If serialization fails, the original Serializer is returned unmodified.
    // - If `outer.try_constraints()` returns `None`, serialization fails.
    // - If the size limit is exceeded and truncation is disabled, serialization
    //   fails.
    // - If serialization succeeds, the body has the correct length, including
    //   taking into account `outer`'s minimum body length requirement
    #[derive(Copy, Clone, Debug, Eq, PartialEq)]
    struct VerifyingSerializer<S> {
        ser: S,
        verifier: SerializerVerifier,
    }

    impl<S: Serializer<NoOpSerializationContext> + Debug + Clone + Eq>
        Serializer<NoOpSerializationContext> for VerifyingSerializer<S>
    where
        S::Buffer: ReusableBuffer,
    {
        type Buffer = S::Buffer;

        fn serialize<B: GrowBufferMut, P: BufferProvider<Self::Buffer, B>>(
            self,
            context: &mut NoOpSerializationContext,
            constraints: PacketConstraints,
            provider: P,
        ) -> Result<B, (SerializeError<P::Error>, Self)> {
            let Self { ser, verifier } = self;
            let orig = ser.clone();

            let result = ser.serialize(context, constraints, provider).map_err(|(err, ser)| {
                // If serialization fails, the original Serializer should be
                // unmodified.
                assert_eq!(ser, orig);
                (err, Self { ser, verifier })
            });

            verifier.verify_result(result.as_ref().map_err(|(err, _ser)| err), constraints);

            result
        }

        fn serialize_new_buf<B: GrowBufferMut, A: LayoutBufferAlloc<B>>(
            &self,
            context: &mut NoOpSerializationContext,
            outer: PacketConstraints,
            alloc: A,
        ) -> Result<B, SerializeError<A::Error>> {
            let res = self.ser.serialize_new_buf(context, outer, alloc);
            self.verifier.verify_result(res.as_ref(), outer);
            res
        }
    }

    impl<S> NestableSerializer for VerifyingSerializer<S> {}

    trait SerializerExt: Serializer<NoOpSerializationContext> {
        fn into_verifying(self, truncating: bool) -> VerifyingSerializer<Self>
        where
            Self::Buffer: ReusableBuffer,
        {
            let verifier = SerializerVerifier::new(&self, truncating);
            VerifyingSerializer { ser: self, verifier }
        }

        fn wrap_in_verifying<B: PacketBuilder<NoOpSerializationContext>>(
            self,
            outer: B,
            truncating: bool,
        ) -> VerifyingSerializer<Nested<Self, B>>
        where
            Self::Buffer: ReusableBuffer,
        {
            self.wrap_in(outer).into_verifying(truncating)
        }

        fn with_size_limit_verifying(
            self,
            limit: usize,
            truncating: bool,
        ) -> VerifyingSerializer<Nested<Self, LimitedSizePacketBuilder>>
        where
            Self::Buffer: ReusableBuffer,
        {
            self.with_size_limit(limit).into_verifying(truncating)
        }
    }

    impl<S: Serializer<NoOpSerializationContext>> SerializerExt for S {}

    #[test]
    fn test_either_into_inner() {
        fn ret_either(a: u32, b: u32, c: bool) -> Either<u32, u32> {
            if c { Either::A(a) } else { Either::B(b) }
        }

        assert_eq!(ret_either(1, 2, true).into_inner(), 1);
        assert_eq!(ret_either(1, 2, false).into_inner(), 2);
    }

    #[test]
    fn test_either_unwrap_success() {
        assert_eq!(Either::<u16, u32>::A(5).unwrap_a(), 5);
        assert_eq!(Either::<u16, u32>::B(10).unwrap_b(), 10);
    }

    #[test]
    #[should_panic]
    fn test_either_unwrap_a_panic() {
        let _: u16 = Either::<u16, u32>::B(10).unwrap_a();
    }

    #[test]
    #[should_panic]
    fn test_either_unwrap_b_panic() {
        let _: u32 = Either::<u16, u32>::A(5).unwrap_b();
    }

    #[test_case(Buf::new((0..100).collect(), ..); "entire buf")]
    #[test_case(Buf::new((0..100).collect(), 0..0); "empty range")]
    #[test_case(Buf::new((0..100).collect(), ..50); "prefix")]
    #[test_case(Buf::new((0..100).collect(), 50..); "suffix")]
    #[test_case(Buf::new((0..100).collect(), 25..75); "middle")]
    fn test_buf_into_inner(buf: Buf<Vec<u8>>) {
        assert_eq!(buf.clone().as_ref(), buf.into_inner());
    }

    #[test]
    fn test_packet_constraints() {
        use PacketConstraints as PC;

        // Test try_new

        // Sanity check.
        assert!(PC::try_new(0, 0, 0, 0).is_some());
        // header_len + min_body_len + footer_len doesn't overflow usize
        assert!(PC::try_new(usize::MAX / 2, usize::MAX / 2, 0, 0).is_some());
        // header_len + min_body_len + footer_len overflows usize
        assert_eq!(PC::try_new(usize::MAX, 1, 0, 0), None);
        // min_body_len > max_body_len
        assert_eq!(PC::try_new(0, 0, 1, 0), None);

        // Test PacketConstraints::try_encapsulate

        // Sanity check.
        let pc = PC::new(10, 10, 0, usize::MAX);
        assert_eq!(pc.try_encapsulate(&pc).unwrap(), PC::new(20, 20, 0, usize::MAX - 20));

        let pc = PC::new(10, 10, 0, usize::MAX);
        assert_eq!(pc.try_encapsulate(&pc).unwrap(), PC::new(20, 20, 0, usize::MAX - 20));

        // Starting here, each failure test case corresponds to one check in
        // either PacketConstraints::try_encapsulate or PacketConstraints::new
        // (which is called from PacketConstraints::try_encapsulate). Each test
        // case is labeled "Test case N", and a corresponding comment in either
        // of those two functions identifies which line is being tested.

        // The outer PC's minimum body length requirement of 10 is more than
        // satisfied by the inner PC's combined 20 bytes of header and footer.
        // The resulting PC has its minimum body length requirement saturated to
        // 0.
        let inner = PC::new(10, 10, 0, usize::MAX);
        let outer = PC::new(0, 0, 10, usize::MAX);
        assert_eq!(inner.try_encapsulate(&outer).unwrap(), PC::new(10, 10, 0, usize::MAX - 20));

        // Test case 1
        //
        // The sum of the inner and outer header lengths overflows `usize`.
        let inner = PC::new(usize::MAX, 0, 0, usize::MAX);
        let outer = PC::new(1, 0, 0, usize::MAX);
        assert_eq!(inner.try_encapsulate(&outer), None);

        // Test case 2
        //
        // The sum of the inner and outer footer lengths overflows `usize`.
        let inner = PC::new(0, usize::MAX, 0, usize::MAX);
        let outer = PC::new(0, 1, 0, usize::MAX);
        assert_eq!(inner.try_encapsulate(&outer), None);

        // Test case 3
        //
        // The sum of the resulting header, footer, and minimum body lengths
        // overflows `usize`. We use usize::MAX / 5 + 1 as the constant so that
        // none of the intermediate additions overflow, so we make sure to test
        // that an overflow in the final addition will be caught.
        let one_fifth_max = (usize::MAX / 5) + 1;
        let inner = PC::new(one_fifth_max, one_fifth_max, one_fifth_max, usize::MAX);
        let outer = PC::new(one_fifth_max, one_fifth_max, 0, usize::MAX);
        assert_eq!(inner.try_encapsulate(&outer), None);

        // Test case 4
        //
        // The header and footer of the inner PC exceed the maximum body length
        // requirement of the outer PC.
        let inner = PC::new(10, 10, 0, usize::MAX);
        let outer = PC::new(0, 0, 0, 10);
        assert_eq!(inner.try_encapsulate(&outer), None);

        // Test case 5
        //
        // The resulting minimum body length (thanks to the inner
        // PacketBuilder's minimum body length) is larger than the resulting
        // maximum body length.
        let inner = PC::new(0, 0, 10, usize::MAX);
        let outer = PC::new(0, 0, 0, 5);
        assert_eq!(inner.try_encapsulate(&outer), None);
    }

    #[test]
    fn test_inner_serializer() {
        const INNER: &[u8] = &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9];

        fn concat<'a, I: IntoIterator<Item = &'a &'a [u8]>>(slices: I) -> Vec<u8> {
            let mut v = Vec::new();
            for slc in slices.into_iter() {
                v.extend_from_slice(slc);
            }
            v
        }

        // Sanity check.
        let buf =
            INNER.into_serializer().serialize_vec_outer(&mut NoOpSerializationContext).unwrap();
        assert_eq!(buf.as_ref(), INNER);

        // A larger minimum body length requirement will cause padding to be
        // added.
        let buf = INNER
            .into_serializer()
            .into_verifying(false)
            .wrap_in(DummyPacketBuilder::new(0, 0, 20, usize::MAX))
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .unwrap();
        assert_eq!(buf.as_ref(), concat(&[INNER, vec![0; 10].as_ref()]).as_slice());

        // Headers and footers are added as appropriate (note that
        // DummyPacketBuilder fills its header with 0xFF and its footer with
        // 0xFE).
        let buf = INNER
            .into_serializer()
            .into_verifying(false)
            .wrap_in(DummyPacketBuilder::new(10, 10, 0, usize::MAX))
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .unwrap();
        assert_eq!(
            buf.as_ref(),
            concat(&[vec![0xFF; 10].as_ref(), INNER, vec![0xFE; 10].as_ref()]).as_slice()
        );

        // An exceeded maximum body size is rejected.
        assert_eq!(
            INNER
                .into_serializer()
                .into_verifying(false)
                .wrap_in(DummyPacketBuilder::new(0, 0, 0, 9))
                .serialize_vec_outer(&mut NoOpSerializationContext)
                .unwrap_err()
                .0,
            SerializeError::SizeLimitExceeded
        );

        // `into_serializer_with` truncates the buffer's body to zero before
        // returning, so those body bytes are not included in the serialized
        // output.
        assert_eq!(
            INNER
                .into_serializer_with(Buf::new(vec![0xFF], ..))
                .into_verifying(false)
                .serialize_vec_outer(&mut NoOpSerializationContext)
                .unwrap()
                .as_ref(),
            INNER
        );
    }

    #[test]
    fn test_buffer_serializer_and_inner_serializer() {
        fn verify_buffer_serializer<B: BufferMut + Debug>(
            buffer: B,
            header_len: usize,
            footer_len: usize,
            min_body_len: usize,
        ) {
            let old_body = buffer.to_flattened_vec();
            let serializer =
                DummyPacketBuilder::new(header_len, footer_len, min_body_len, usize::MAX)
                    .wrap_body(buffer);

            let buffer0 = serializer
                .serialize_new_buf(
                    &mut NoOpSerializationContext,
                    PacketConstraints::UNCONSTRAINED,
                    dirty_buf_alloc,
                )
                .unwrap();
            verify(buffer0, &old_body, header_len, footer_len, min_body_len);

            let buffer = serializer.serialize_vec_outer(&mut NoOpSerializationContext).unwrap();
            verify(buffer, &old_body, header_len, footer_len, min_body_len);
        }

        fn verify_inner_packet_builder_serializer(
            body: &[u8],
            header_len: usize,
            footer_len: usize,
            min_body_len: usize,
        ) {
            let buffer = DummyPacketBuilder::new(header_len, footer_len, min_body_len, usize::MAX)
                .wrap_body(body.into_serializer())
                .serialize_vec_outer(&mut NoOpSerializationContext)
                .unwrap();
            verify(buffer, body, header_len, footer_len, min_body_len);
        }

        fn verify<B: Buffer>(
            buffer: B,
            body: &[u8],
            header_len: usize,
            footer_len: usize,
            min_body_len: usize,
        ) {
            let flat = buffer.to_flattened_vec();
            let header_bytes = &flat[..header_len];
            let body_bytes = &flat[header_len..header_len + body.len()];
            let padding_len = min_body_len.saturating_sub(body.len());
            let padding_bytes =
                &flat[header_len + body.len()..header_len + body.len() + padding_len];
            let total_body_len = body.len() + padding_len;
            let footer_bytes = &flat[header_len + total_body_len..];
            assert_eq!(
                buffer.len() - total_body_len,
                header_len + footer_len,
                "buffer.len()({}) - total_body_len({}) != header_len({}) + footer_len({})",
                buffer.len(),
                header_len,
                footer_len,
                min_body_len,
            );

            // DummyPacketBuilder fills its header with 0xFF
            assert!(
                header_bytes.iter().all(|b| *b == 0xFF),
                "header_bytes {:?} are not filled with 0xFF's",
                header_bytes,
            );
            assert_eq!(body_bytes, body);
            // Padding bytes must be initialized to zero
            assert!(
                padding_bytes.iter().all(|b| *b == 0),
                "padding_bytes {:?} are not filled with 0s",
                padding_bytes,
            );
            // DummyPacketBuilder fills its footer with 0xFE
            assert!(
                footer_bytes.iter().all(|b| *b == 0xFE),
                "footer_bytes {:?} are not filled with 0xFE's",
                footer_bytes,
            );
        }

        // Test for every valid combination of buf_len, range_start, range_end,
        // prefix, suffix, and min_body within [0, 8).
        for buf_len in 0..8 {
            for range_start in 0..buf_len {
                for range_end in range_start..buf_len {
                    for prefix in 0..8 {
                        for suffix in 0..8 {
                            for min_body in 0..8 {
                                let mut vec = vec![0; buf_len];
                                // Initialize the vector with values 0, 1, 2,
                                // ... so that we can check to make sure that
                                // the range bytes have been properly copied if
                                // the buffer is reallocated.
                                #[allow(clippy::needless_range_loop)]
                                for i in 0..vec.len() {
                                    vec[i] = i as u8;
                                }
                                verify_buffer_serializer(
                                    Buf::new(vec.as_mut_slice(), range_start..range_end),
                                    prefix,
                                    suffix,
                                    min_body,
                                );
                                if range_start == 0 {
                                    // Unlike verify_buffer_serializer, this
                                    // test doesn't make use of the prefix or
                                    // suffix. In order to avoid running the
                                    // exact same test multiple times, we only
                                    // run this when `range_start == 0`, which
                                    // has the effect of reducing the number of
                                    // times that this test is run by roughly a
                                    // factor of 8.
                                    verify_inner_packet_builder_serializer(
                                        &vec.as_slice()[range_start..range_end],
                                        prefix,
                                        suffix,
                                        min_body,
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn test_min_body_len() {
        // Test that padding is added after the body of the packet whose minimum
        // body length constraint requires it. A previous version of this code
        // had a bug where padding was always added after the innermost body.

        let body = &[1, 2];

        // 4 bytes of header and footer for a total of 6 bytes (including the
        // body).
        let inner = DummyPacketBuilder::new(2, 2, 0, usize::MAX);
        // Minimum body length of 8 will require 2 bytes of padding.
        let outer = DummyPacketBuilder::new(2, 2, 8, usize::MAX);
        let buf = body
            .into_serializer()
            .into_verifying(false)
            .wrap_in_verifying(inner, false)
            .wrap_in_verifying(outer, false)
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .unwrap();
        assert_eq!(buf.prefix_len(), 0);
        assert_eq!(buf.suffix_len(), 0);
        assert_eq!(
            buf.as_ref(),
            &[
                0xFF, 0xFF, // Outer header
                0xFF, 0xFF, // Inner header
                1, 2, // Inner body
                0xFE, 0xFE, // Inner footer
                0, 0, // Padding to satisfy outer minimum body length requirement
                0xFE, 0xFE // Outer footer
            ]
        );
    }

    #[test]
    fn test_size_limit() {
        // ser is a Serializer that will consume 1 byte of buffer space
        fn test<S: Serializer<NoOpSerializationContext> + Clone + Debug + Eq>(ser: S)
        where
            S::Buffer: ReusableBuffer,
        {
            // Each of these tests encapsulates ser in a DummyPacketBuilder
            // which consumes 1 byte for the header and one byte for the footer.
            // Thus, the inner serializer will consume 1 byte, while the
            // DummyPacketBuilder will consume 2 bytes, for a total of 3 bytes.

            let pb = DummyPacketBuilder::new(1, 1, 0, usize::MAX);

            // Test that a size limit of 3 is OK. Note that this is an important
            // test since it tests the case when the size limit is exactly
            // sufficient. A previous version of this code had a bug where a
            // packet which fit the size limit exactly would be rejected.
            assert!(
                ser.clone()
                    .wrap_in_verifying(pb, false)
                    .with_size_limit_verifying(3, false)
                    .serialize_vec_outer(&mut NoOpSerializationContext)
                    .is_ok()
            );
            // Test that a more-than-large-enough size limit of 4 is OK.
            assert!(
                ser.clone()
                    .wrap_in_verifying(pb, false)
                    .with_size_limit_verifying(4, false)
                    .serialize_vec_outer(&mut NoOpSerializationContext)
                    .is_ok()
            );
            // Test that the inner size limit of 1 only applies to the inner
            // serializer, and so is still OK even though the outer serializer
            // consumes 3 bytes total.
            assert!(
                ser.clone()
                    .with_size_limit_verifying(1, false)
                    .wrap_in_verifying(pb, false)
                    .with_size_limit_verifying(3, false)
                    .serialize_vec_outer(&mut NoOpSerializationContext)
                    .is_ok()
            );
            // Test that the inner size limit of 0 is exceeded by the inner
            // serializer's 1 byte length.
            assert!(
                ser.clone()
                    .with_size_limit_verifying(0, false)
                    .wrap_in_verifying(pb, false)
                    .serialize_vec_outer(&mut NoOpSerializationContext)
                    .is_err()
            );
            // Test that a size limit which would be exceeded by the
            // encapsulating layer is rejected by Nested's implementation. If
            // this doesn't work properly, then the size limit should underflow,
            // resulting in a panic (see the Nested implementation of
            // Serialize).
            assert!(
                ser.clone()
                    .wrap_in_verifying(pb, false)
                    .with_size_limit_verifying(1, false)
                    .serialize_vec_outer(&mut NoOpSerializationContext)
                    .is_err()
            );
        }

        // We use this as an InnerPacketBuilder which consumes 1 byte of body.
        test(DummyPacketBuilder::new(1, 0, 0, usize::MAX).into_serializer().into_verifying(false));
        test(Buf::new(vec![0], ..).into_verifying(false));
    }

    #[test]
    fn test_truncating_serializer() {
        fn verify_result<S: Serializer<NoOpSerializationContext> + Debug>(ser: S, expected: &[u8])
        where
            S::Buffer: ReusableBuffer + AsRef<[u8]>,
        {
            let buf = ser
                .serialize_new_buf(
                    &mut NoOpSerializationContext,
                    PacketConstraints::UNCONSTRAINED,
                    new_buf_vec,
                )
                .unwrap();
            assert_eq!(buf.as_ref(), &expected[..]);
            let buf = ser.serialize_vec_outer(&mut NoOpSerializationContext).unwrap();
            assert_eq!(buf.as_ref(), &expected[..]);
        }

        // Test truncate front.
        let body = vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9];
        let ser =
            TruncatingSerializer::new(Buf::new(body.clone(), ..), TruncateDirection::DiscardFront)
                .into_verifying(true)
                .with_size_limit_verifying(4, true);
        verify_result(ser, &[6, 7, 8, 9]);

        // Test truncate back.
        let ser =
            TruncatingSerializer::new(Buf::new(body.clone(), ..), TruncateDirection::DiscardBack)
                .into_verifying(true)
                .with_size_limit_verifying(7, true);
        verify_result(ser, &[0, 1, 2, 3, 4, 5, 6]);

        // Test no truncating (default/original case).
        let ser =
            TruncatingSerializer::new(Buf::new(body.clone(), ..), TruncateDirection::NoTruncating)
                .into_verifying(false)
                .with_size_limit_verifying(5, true);
        assert!(ser.clone().serialize_vec_outer(&mut NoOpSerializationContext).is_err());
        assert!(
            ser.serialize_new_buf(
                &mut NoOpSerializationContext,
                PacketConstraints::UNCONSTRAINED,
                new_buf_vec
            )
            .is_err()
        );
        assert!(ser.serialize_vec_outer(&mut NoOpSerializationContext).is_err());

        // Test that, when serialization fails, any truncation is undone.

        // `ser` has a body of `[1, 2]` and no prefix or suffix
        fn test_serialization_failure<
            S: Serializer<NoOpSerializationContext> + Clone + Eq + Debug,
        >(
            ser: S,
            err: SerializeError<BufferTooShortError>,
        ) where
            S::Buffer: ReusableBuffer + Debug,
        {
            // Serialize with a PacketBuilder with a size limit of 1 so that the
            // body (of length 2) is too large. If `ser` is configured not to
            // truncate, it should result in a size limit error. If it is
            // configured to truncate, the 2 + 2 = 4 combined bytes of header
            // and footer will cause allocating a new buffer to fail, and it
            // should result in an allocation failure. Even if the body was
            // truncated, it should be returned to its original un-truncated
            // state before being returned from `serialize`.
            let (e, new_ser) = DummyPacketBuilder::new(2, 2, 0, 1)
                .wrap_body(ser.clone())
                .serialize_no_alloc_outer(&mut NoOpSerializationContext)
                .unwrap_err();
            assert_eq!(err, e);
            assert_eq!(new_ser.into_inner(), ser);
        }

        let body = Buf::new(vec![1, 2], ..);
        test_serialization_failure(
            TruncatingSerializer::new(body.clone(), TruncateDirection::DiscardFront)
                .into_verifying(true),
            SerializeError::Alloc(BufferTooShortError),
        );
        test_serialization_failure(
            TruncatingSerializer::new(body.clone(), TruncateDirection::DiscardFront)
                .into_verifying(true),
            SerializeError::Alloc(BufferTooShortError),
        );
        test_serialization_failure(
            TruncatingSerializer::new(body.clone(), TruncateDirection::NoTruncating)
                .into_verifying(false),
            SerializeError::SizeLimitExceeded,
        );
    }

    // Regression test for a bug in the directionality of constraints
    // encapsulation in nested partial serialization: an outer packet should be
    // able to encapsulate an inner packet whose constraints it would violate if
    // the encapsulation were reversed.
    #[test]
    fn nested_partial_serialize_constraints() {
        const BODY: &[u8] = &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9];
        const INNER_PACKET_MAX_BODY: usize = 10;
        const EXPECTED_HEADER: &[u8] = &[0xFF; 20];

        let packet = BODY
            .into_serializer()
            .wrap_in(DummyPacketBuilder::new(0, 0, 0, INNER_PACKET_MAX_BODY))
            .wrap_in(DummyPacketBuilder::new(2 * INNER_PACKET_MAX_BODY, 0, 0, usize::MAX));
        let result = packet.partial_serialize(&mut NoOpSerializationContext, new_buf_vec);
        let buffer = assert_matches!(
            result,
            Ok(PartialSerializeResult::NewBuffer { buffer, total_size: 30 }) => buffer);
        assert_eq!(buffer.as_ref(), EXPECTED_HEADER);
    }

    #[test]
    fn test_try_reuse_buffer() {
        fn test_expect_success(
            body_range: Range<usize>,
            prefix: usize,
            suffix: usize,
            max_copy_bytes: usize,
        ) {
            let mut bytes = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9];
            let buffer = Buf::new(&mut bytes[..], body_range);
            let body = buffer.as_ref().to_vec();
            let buffer = try_reuse_buffer(buffer, prefix, suffix, max_copy_bytes).unwrap();
            assert_eq!(buffer.as_ref(), body.as_slice());
            assert!(buffer.prefix_len() >= prefix);
            assert!(buffer.suffix_len() >= suffix);
        }

        fn test_expect_failure(
            body_range: Range<usize>,
            prefix: usize,
            suffix: usize,
            max_copy_bytes: usize,
        ) {
            let mut bytes = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9];
            let buffer = Buf::new(&mut bytes[..], body_range.clone());
            let mut bytes = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9];
            let orig = Buf::new(&mut bytes[..], body_range.clone());
            let buffer = try_reuse_buffer(buffer, prefix, suffix, max_copy_bytes).unwrap_err();
            assert_eq!(buffer, orig);
        }

        // No prefix or suffix trivially succeeds.
        test_expect_success(0..10, 0, 0, 0);
        // If we have enough prefix/suffix, it succeeds.
        test_expect_success(1..9, 1, 1, 0);
        // If we don't have enough prefix/suffix, but we have enough capacity to
        // move the buffer within the body, it succeeds...
        test_expect_success(0..9, 1, 0, 9);
        test_expect_success(1..10, 0, 1, 9);
        // ...but if we don't provide a large enough max_copy_bytes, it will fail.
        test_expect_failure(0..9, 1, 0, 8);
        test_expect_failure(1..10, 0, 1, 8);
    }

    #[test]
    fn test_maybe_reuse_buffer_provider() {
        fn test_expect(body_range: Range<usize>, prefix: usize, suffix: usize, expect_a: bool) {
            let mut bytes = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9];
            let buffer = Buf::new(&mut bytes[..], body_range);
            let body = buffer.as_ref().to_vec();
            let buffer = BufferProvider::reuse_or_realloc(
                MaybeReuseBufferProvider(new_buf_vec),
                buffer,
                prefix,
                suffix,
            )
            .unwrap();
            match &buffer {
                Either::A(_) if expect_a => {}
                Either::B(_) if !expect_a => {}
                Either::A(_) => panic!("expected Eitehr::B variant"),
                Either::B(_) => panic!("expected Eitehr::A variant"),
            }
            let bytes: &[u8] = buffer.as_ref();
            assert_eq!(bytes, body.as_slice());
            assert!(buffer.prefix_len() >= prefix);
            assert!(buffer.suffix_len() >= suffix);
        }

        // Expect that we'll be able to reuse the existing buffer.
        fn test_expect_reuse(body_range: Range<usize>, prefix: usize, suffix: usize) {
            test_expect(body_range, prefix, suffix, true);
        }

        // Expect that we'll need to allocate a new buffer.
        fn test_expect_realloc(body_range: Range<usize>, prefix: usize, suffix: usize) {
            test_expect(body_range, prefix, suffix, false);
        }

        // No prefix or suffix trivially succeeds.
        test_expect_reuse(0..10, 0, 0);
        // If we have enough prefix/suffix, it succeeds.
        test_expect_reuse(1..9, 1, 1);
        // If we don't have enough prefix/suffix, but we have enough capacity to
        // move the buffer within the body, it succeeds.
        test_expect_reuse(0..9, 1, 0);
        test_expect_reuse(1..10, 0, 1);
        // If we don't have enough capacity, it fails and must realloc.
        test_expect_realloc(0..9, 1, 1);
        test_expect_realloc(1..10, 1, 1);
    }

    #[test]
    fn test_no_reuse_buffer_provider() {
        #[track_caller]
        fn test_expect(body_range: Range<usize>, prefix: usize, suffix: usize) {
            let mut bytes = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9];
            // The buffer that will not be reused.
            let internal_buffer: Buf<&mut [u8]> = Buf::new(&mut bytes[..], body_range);
            let body = internal_buffer.as_ref().to_vec();
            // The newly allocated buffer, note the type is different from
            // internal_buffer.
            let buffer: Buf<Vec<u8>> = BufferProvider::reuse_or_realloc(
                NoReuseBufferProvider(new_buf_vec),
                internal_buffer,
                prefix,
                suffix,
            )
            .unwrap();
            let bytes: &[u8] = buffer.as_ref();
            assert_eq!(bytes, body.as_slice());
            assert_eq!(buffer.prefix_len(), prefix);
            assert_eq!(buffer.suffix_len(), suffix);
        }
        // No prefix or suffix trivially succeeds, reuse opportunity is ignored.
        test_expect(0..10, 0, 0);
        // If we have enough prefix/suffix, reuse opportunity is ignored.
        test_expect(1..9, 1, 1);
        // Prefix and suffix and properly allocated and the body is copied.
        test_expect(0..9, 10, 10);
        test_expect(1..10, 15, 15);
    }

    /// Simple Vec-backed buffer to test fragmented buffers implementation.
    ///
    /// `ScatterGatherBuf` keeps:
    /// - an inner buffer `inner`, which is always part of its body.
    /// - extra backing memory in `data`.
    ///
    /// `data` has two "root" regions, marked by the midpoint `mid`. Everything
    /// left of `mid` is this buffer's prefix, and after `mid` is this buffer's
    /// suffix.
    ///
    /// The `range` field keeps the range in `data` that contains *filled*
    /// prefix and suffix information. `range.start` is always less than or
    /// equal to `mid` and `range.end` is always greater than or equal to `mid`,
    /// such that growing the front of the buffer means decrementing
    /// `range.start` and growing the back of the buffer means incrementing
    /// `range.end`.
    ///
    ///  At any time this buffer's parts are:
    /// - Free prefix data in range `0..range.start`.
    /// - Used prefix data (now part of body) in range `range.start..mid`.
    /// - Inner buffer body in `inner`.
    /// - Used suffix data (now part of body) in range `mid..range.end`.
    /// - Free suffix data in range `range.end..`
    struct ScatterGatherBuf<B> {
        data: Vec<u8>,
        mid: usize,
        range: Range<usize>,
        inner: B,
    }

    impl<B: BufferMut> FragmentedBuffer for ScatterGatherBuf<B> {
        fn len(&self) -> usize {
            self.inner.len() + (self.range.end - self.range.start)
        }

        fn with_bytes<'a, R, F>(&'a self, f: F) -> R
        where
            F: for<'b> FnOnce(FragmentedBytes<'b, 'a>) -> R,
        {
            let (_, rest) = self.data.split_at(self.range.start);
            let (prefix_b, rest) = rest.split_at(self.mid - self.range.start);
            let (suffix_b, _) = rest.split_at(self.range.end - self.mid);
            let mut bytes = [prefix_b, self.inner.as_ref(), suffix_b];
            f(FragmentedBytes::new(&mut bytes[..]))
        }
    }

    impl<B: BufferMut> FragmentedBufferMut for ScatterGatherBuf<B> {
        fn with_bytes_mut<'a, R, F>(&'a mut self, f: F) -> R
        where
            F: for<'b> FnOnce(FragmentedBytesMut<'b, 'a>) -> R,
        {
            let (_, rest) = self.data.split_at_mut(self.range.start);
            let (prefix_b, rest) = rest.split_at_mut(self.mid - self.range.start);
            let (suffix_b, _) = rest.split_at_mut(self.range.end - self.mid);
            let mut bytes = [prefix_b, self.inner.as_mut(), suffix_b];
            f(FragmentedBytesMut::new(&mut bytes[..]))
        }
    }

    impl<B: BufferMut> GrowBuffer for ScatterGatherBuf<B> {
        fn with_parts<'a, O, F>(&'a self, f: F) -> O
        where
            F: for<'b> FnOnce(&'a [u8], FragmentedBytes<'b, 'a>, &'a [u8]) -> O,
        {
            let (prefix, rest) = self.data.split_at(self.range.start);
            let (prefix_b, rest) = rest.split_at(self.mid - self.range.start);
            let (suffix_b, suffix) = rest.split_at(self.range.end - self.mid);
            let mut bytes = [prefix_b, self.inner.as_ref(), suffix_b];
            f(prefix, bytes.as_fragmented_byte_slice(), suffix)
        }
        fn prefix_len(&self) -> usize {
            self.range.start
        }

        fn suffix_len(&self) -> usize {
            self.data.len() - self.range.end
        }

        fn grow_front(&mut self, n: usize) {
            self.range.start -= n;
        }

        fn grow_back(&mut self, n: usize) {
            self.range.end += n;
            assert!(self.range.end <= self.data.len());
        }
    }

    impl<B: BufferMut> GrowBufferMut for ScatterGatherBuf<B> {
        fn with_parts_mut<'a, O, F>(&'a mut self, f: F) -> O
        where
            F: for<'b> FnOnce(&'a mut [u8], FragmentedBytesMut<'b, 'a>, &'a mut [u8]) -> O,
        {
            let (prefix, rest) = self.data.split_at_mut(self.range.start);
            let (prefix_b, rest) = rest.split_at_mut(self.mid - self.range.start);
            let (suffix_b, suffix) = rest.split_at_mut(self.range.end - self.mid);
            let mut bytes = [prefix_b, self.inner.as_mut(), suffix_b];
            f(prefix, bytes.as_fragmented_byte_slice(), suffix)
        }

        fn with_all_contents_mut<'a, O, F>(&'a mut self, _f: F) -> O
        where
            F: for<'b> FnOnce(FragmentedBytesMut<'b, 'a>) -> O,
        {
            unimplemented!()
        }
    }

    struct ScatterGatherProvider;

    impl<B: BufferMut> BufferProvider<B, ScatterGatherBuf<B>> for ScatterGatherProvider {
        type Error = Never;

        fn alloc_no_reuse(
            self,
            _prefix: usize,
            _body: usize,
            _suffix: usize,
        ) -> Result<ScatterGatherBuf<B>, Self::Error> {
            unimplemented!("not used in tests")
        }

        fn reuse_or_realloc(
            self,
            buffer: B,
            prefix: usize,
            suffix: usize,
        ) -> Result<ScatterGatherBuf<B>, (Self::Error, B)> {
            let inner = buffer;
            let data = vec![0; prefix + suffix];
            let range = Range { start: prefix, end: prefix };
            let mid = prefix;
            Ok(ScatterGatherBuf { inner, data, range, mid })
        }
    }

    #[test]
    fn test_scatter_gather_serialize() {
        // Assert that a buffer composed of different allocations can be used as
        // a serialization target, while reusing an internal body buffer.
        let buf = Buf::new(vec![10, 20, 30, 40, 50], ..);
        let pb = DummyPacketBuilder::new(3, 2, 0, usize::MAX);
        let ser = pb.wrap_body(buf);
        let result =
            ser.serialize_outer(&mut NoOpSerializationContext, ScatterGatherProvider {}).unwrap();
        let flattened = result.to_flattened_vec();
        assert_eq!(&flattened[..], &[0xFF, 0xFF, 0xFF, 10, 20, 30, 40, 50, 0xFE, 0xFE]);
    }

    #[test]
    fn dyn_serialize() {
        let body = Buf::new(vec![10, 20, 30, 40, 50], ..);
        let header1 = DummyPacketBuilder {
            header_len: 5,
            footer_len: 0,
            min_body_len: 0,
            max_body_len: usize::MAX,
            header_byte: 0xAA,
            footer_byte: 0xBB,
        };
        let header2 = DummyPacketBuilder {
            header_len: 3,
            footer_len: 2,
            min_body_len: 0,
            max_body_len: usize::MAX,
            header_byte: 0xCC,
            footer_byte: 0xDD,
        };
        // A reference serializer.
        let ser1 = body.clone().wrap_in(header1).wrap_in(header2);
        // A nested dynamic serializer.
        let ser2 = body.wrap_in(header1);
        let ser2 = DynSerializer::new(&ser2).wrap_in(header2);
        // An outer dynamic serializer.
        let ser3 = ser1.clone();
        let ser3 = DynSerializer::new(&ser3);
        // Two levels of dynamic serializer.
        let ser4 = DynSerializer::new(&ser2);

        fn serialize(
            s: impl Serializer<NoOpSerializationContext, Buffer: ReusableBuffer>,
        ) -> Vec<u8> {
            s.serialize_vec(&mut NoOpSerializationContext, PacketConstraints::UNCONSTRAINED)
                .map_err(|(e, _)| e)
                .unwrap()
                .unwrap_b()
                .into_inner()
        }

        fn serialize_new(s: impl Serializer<NoOpSerializationContext>) -> Vec<u8> {
            s.serialize_new_buf(
                &mut NoOpSerializationContext,
                PacketConstraints::UNCONSTRAINED,
                new_buf_vec,
            )
            .unwrap()
            .into_inner()
        }

        let expect = serialize(ser1.clone());
        assert_eq!(serialize(ser2), expect);
        assert_eq!(serialize(ser3), expect);
        assert_eq!(serialize(ser4), expect);
        assert_eq!(serialize_new(ser1), expect);
        assert_eq!(serialize_new(ser2), expect);
        assert_eq!(serialize_new(ser3), expect);
        assert_eq!(serialize_new(ser4), expect);
    }

    /// SerializationContext that tracks the `header_len()` of the outer
    /// `PacketBuilder`s it sees.
    struct TrackingSerializationContext {
        history: Vec<usize>,
    }

    impl SerializationContext for TrackingSerializationContext {
        type ContextState = ();

        fn serialize_nested<O: PacketBuilder<Self>, R>(
            &mut self,
            outer: &O,
            constraints: PacketConstraints,
            serialize_fn: impl FnOnce(&mut Self, PacketConstraints) -> R,
        ) -> R {
            self.history.push(outer.constraints().header_len());
            serialize_fn(self, constraints)
        }
    }

    #[test]
    fn nested_serializer_context_aware() {
        let body = Buf::new(vec![0; 10], ..);

        let outer = DummyPacketBuilder::new(3, 0, 0, usize::MAX);
        let middle = DummyPacketBuilder::new(2, 0, 0, usize::MAX);
        let inner = DummyPacketBuilder::new(1, 0, 0, usize::MAX);

        let serializer = body.wrap_in(inner).wrap_in(middle).wrap_in(outer);

        let mut context = TrackingSerializationContext { history: Vec::new() };
        let _buf = serializer.clone().serialize_vec_outer(&mut context).unwrap();
        assert_eq!(context.history, vec![3, 2, 1]);

        let mut context = TrackingSerializationContext { history: Vec::new() };
        let _buf = serializer
            .serialize_new_buf(&mut context, PacketConstraints::UNCONSTRAINED, new_buf_vec)
            .unwrap();
        assert_eq!(context.history, vec![3, 2, 1]);
    }

    #[test]
    fn nested_partial_serializer_context_aware() {
        let body = Buf::new(vec![0; 10], ..);

        let outer = DummyPacketBuilder::new(3, 0, 0, usize::MAX);
        let middle = DummyPacketBuilder::new(2, 0, 0, usize::MAX);
        let inner = DummyPacketBuilder::new(1, 0, 0, usize::MAX);

        let serializer = body.wrap_in(inner).wrap_in(middle).wrap_in(outer);

        let mut context = TrackingSerializationContext { history: Vec::new() };
        let _result = serializer.partial_serialize(&mut context, new_buf_vec).unwrap();
        assert_eq!(context.history, vec![3, 2, 1]);
    }
}
