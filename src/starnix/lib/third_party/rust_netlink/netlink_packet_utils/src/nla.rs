// SPDX-License-Identifier: MIT

use crate::DecodeError;
use crate::traits::{Emitable, Parseable};
use byteorder::{ByteOrder, NativeEndian};
use core::ops::Range;
use thiserror::Error;

/// Represent a multi-bytes field with a fixed size in a packet
type Field = Range<usize>;

/// Identify the bits that represent the "nested" flag of a netlink attribute.
pub const NLA_F_NESTED: u16 = 0x8000;
/// Identify the bits that represent the "byte order" flag of a netlink
/// attribute.
pub const NLA_F_NET_BYTEORDER: u16 = 0x4000;
/// Identify the bits that represent the type of a netlink attribute.
pub const NLA_TYPE_MASK: u16 = !(NLA_F_NET_BYTEORDER | NLA_F_NESTED);
/// NlA(RTA) align size
pub const NLA_ALIGNTO: usize = 4;
/// NlA(RTA) header size. (unsigned short rta_len) + (unsigned short rta_type)
pub const NLA_HEADER_SIZE: usize = 4;

#[derive(Debug, Clone, Error)]
pub enum NlaError {
    #[error("buffer has length {buffer_len}, but an NLA header is {} bytes", TYPE.end)]
    BufferTooSmall { buffer_len: usize },

    #[error("buffer has length: {buffer_len}, but the NLA is {nla_len} bytes")]
    LengthMismatch { buffer_len: usize, nla_len: u16 },

    #[error(
        "NLA has invalid length: {nla_len} (should be at least {} bytes", TYPE.end
    )]
    InvalidLength { nla_len: u16 },
}

#[macro_export]
macro_rules! nla_align {
    ($len: expr) => {
        ($len + NLA_ALIGNTO - 1) & !(NLA_ALIGNTO - 1)
    };
}

const LENGTH: Field = 0..2;
const TYPE: Field = 2..4;
#[allow(non_snake_case)]
fn VALUE(length: usize) -> Field {
    TYPE.end..TYPE.end + length
}

// with Copy, NlaBuffer<&'buffer T> can be copied, which turns out to be pretty
// conveninent. And since it's boils down to copying a reference it's pretty
// cheap
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct NlaBuffer<T: AsRef<[u8]>> {
    buffer: T,
}

impl<T: AsRef<[u8]>> NlaBuffer<T> {
    pub(crate) fn new_unchecked(buffer: T) -> Self {
        Self { buffer }
    }

    pub fn new(buffer: T) -> Result<NlaBuffer<T>, NlaError> {
        let buffer = Self::new_unchecked(buffer);
        buffer.check_buffer_length()?;
        Ok(buffer)
    }

    pub fn check_buffer_length(&self) -> Result<(), NlaError> {
        let len = self.buffer.as_ref().len();
        if len < TYPE.end {
            Err(NlaError::BufferTooSmall { buffer_len: len }.into())
        } else if len < self.length() as usize {
            Err(NlaError::LengthMismatch { buffer_len: len, nla_len: self.length() }.into())
        } else if (self.length() as usize) < TYPE.end {
            Err(NlaError::InvalidLength { nla_len: self.length() }.into())
        } else {
            Ok(())
        }
    }

    /// Consume the buffer, returning the underlying buffer.
    pub fn into_inner(self) -> T {
        self.buffer
    }

    /// Return a reference to the underlying buffer
    pub fn inner(&self) -> &T {
        &self.buffer
    }

    /// Return a mutable reference to the underlying buffer
    pub fn inner_mut(&mut self) -> &mut T {
        &mut self.buffer
    }

    /// Return the `type` field
    pub fn kind(&self) -> u16 {
        let data = self.buffer.as_ref();
        NativeEndian::read_u16(&data[TYPE]) & NLA_TYPE_MASK
    }

    pub fn nested_flag(&self) -> bool {
        let data = self.buffer.as_ref();
        (NativeEndian::read_u16(&data[TYPE]) & NLA_F_NESTED) != 0
    }

    pub fn network_byte_order_flag(&self) -> bool {
        let data = self.buffer.as_ref();
        (NativeEndian::read_u16(&data[TYPE]) & NLA_F_NET_BYTEORDER) != 0
    }

    /// Return the `length` field. The `length` field corresponds to the length
    /// of the nla header (type and length fields, and the value field).
    /// However, it does not account for the potential padding that follows
    /// the value field.
    pub fn length(&self) -> u16 {
        let data = self.buffer.as_ref();
        NativeEndian::read_u16(&data[LENGTH])
    }

    /// Return the length of the `value` field
    ///
    /// # Panic
    ///
    /// This panics if the length field value is less than the attribut header
    /// size.
    pub fn value_length(&self) -> usize {
        self.length() as usize - TYPE.end
    }
}

impl<T: AsRef<[u8]> + AsMut<[u8]>> NlaBuffer<T> {
    /// Set the `type` field
    pub fn set_kind(&mut self, kind: u16) {
        let data = self.buffer.as_mut();
        NativeEndian::write_u16(&mut data[TYPE], kind & NLA_TYPE_MASK)
    }

    pub fn set_nested_flag(&mut self) {
        let kind = self.kind();
        let data = self.buffer.as_mut();
        NativeEndian::write_u16(&mut data[TYPE], kind | NLA_F_NESTED)
    }

    pub fn set_network_byte_order_flag(&mut self) {
        let kind = self.kind();
        let data = self.buffer.as_mut();
        NativeEndian::write_u16(&mut data[TYPE], kind | NLA_F_NET_BYTEORDER)
    }

    /// Set the `length` field
    pub fn set_length(&mut self, length: u16) {
        let data = self.buffer.as_mut();
        NativeEndian::write_u16(&mut data[LENGTH], length)
    }
}

impl<'buffer, T: AsRef<[u8]> + ?Sized> NlaBuffer<&'buffer T> {
    /// Return the `value` field
    pub fn value(&self) -> &[u8] {
        &self.buffer.as_ref()[VALUE(self.value_length())]
    }
}

impl<'buffer, T: AsRef<[u8]> + AsMut<[u8]> + ?Sized> NlaBuffer<&'buffer mut T> {
    /// Return the `value` field
    pub fn value_mut(&mut self) -> &mut [u8] {
        let length = VALUE(self.value_length());
        &mut self.buffer.as_mut()[length]
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct DefaultNla {
    kind: u16,
    value: Vec<u8>,
}

impl DefaultNla {
    pub fn new(kind: u16, value: Vec<u8>) -> Self {
        Self { kind, value }
    }
}

impl Nla for DefaultNla {
    fn value_len(&self) -> usize {
        self.value.len()
    }
    fn kind(&self) -> u16 {
        self.kind
    }
    fn emit_value(&self, buffer: &mut [u8]) {
        buffer.copy_from_slice(self.value.as_slice());
    }
}

impl<'buffer, T: AsRef<[u8]> + ?Sized> Parseable<NlaBuffer<&'buffer T>> for DefaultNla {
    type Error = DecodeError;

    fn parse(buf: &NlaBuffer<&'buffer T>) -> Result<Self, Self::Error> {
        let mut kind = buf.kind();

        if buf.network_byte_order_flag() {
            kind |= NLA_F_NET_BYTEORDER;
        }

        if buf.nested_flag() {
            kind |= NLA_F_NESTED;
        }

        Ok(DefaultNla { kind, value: buf.value().to_vec() })
    }
}

pub trait Nla {
    fn value_len(&self) -> usize;
    fn kind(&self) -> u16;
    fn emit_value(&self, buffer: &mut [u8]);

    #[inline]
    fn is_nested(&self) -> bool {
        (self.kind() & NLA_F_NESTED) != 0
    }

    #[inline]
    fn is_network_byteorder(&self) -> bool {
        (self.kind() & NLA_F_NET_BYTEORDER) != 0
    }
}

impl<T: Nla> Emitable for T {
    fn buffer_len(&self) -> usize {
        nla_align!(self.value_len()) + NLA_HEADER_SIZE
    }
    fn emit(&self, buffer: &mut [u8]) {
        let mut buffer = NlaBuffer::new_unchecked(buffer);
        buffer.set_kind(self.kind());

        if self.is_network_byteorder() {
            buffer.set_network_byte_order_flag()
        }

        if self.is_nested() {
            buffer.set_nested_flag()
        }

        // do not include the padding here, but do include the header
        buffer.set_length(self.value_len() as u16 + NLA_HEADER_SIZE as u16);

        self.emit_value(buffer.value_mut());

        let padding = nla_align!(self.value_len()) - self.value_len();
        for i in 0..padding {
            buffer.inner_mut()[NLA_HEADER_SIZE + self.value_len() + i] = 0;
        }
    }
}

// FIXME: whern specialization lands, why can actually have
//
// impl<'a, T: Nla, I: Iterator<Item=T>> Emitable for I { ...}
//
// The reason this does not work today is because it conflicts with
//
// impl<T: Nla> Emitable for T { ... }
impl<'a, T: Nla> Emitable for &'a [T] {
    fn buffer_len(&self) -> usize {
        self.iter().fold(0, |acc, nla| {
            assert_eq!(nla.buffer_len() % NLA_ALIGNTO, 0);
            acc + nla.buffer_len()
        })
    }

    fn emit(&self, buffer: &mut [u8]) {
        let mut start = 0;
        let mut end: usize;
        for nla in self.iter() {
            let attr_len = nla.buffer_len();
            assert_eq!(nla.buffer_len() % NLA_ALIGNTO, 0);
            end = start + attr_len;
            nla.emit(&mut buffer[start..end]);
            start = end;
        }
    }
}

/// An iterator that iteratates over nlas without decoding them. This is useful
/// when looking for specific nlas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NlasIterator<T> {
    position: usize,
    buffer: T,
}

impl<T> NlasIterator<T> {
    pub fn new(buffer: T) -> Self {
        NlasIterator { position: 0, buffer }
    }
}

impl<'buffer, T: AsRef<[u8]> + ?Sized + 'buffer> Iterator for NlasIterator<&'buffer T> {
    type Item = Result<NlaBuffer<&'buffer [u8]>, NlaError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.position >= self.buffer.as_ref().len() {
            return None;
        }

        match NlaBuffer::new(&self.buffer.as_ref()[self.position..]) {
            Ok(nla_buffer) => {
                self.position += nla_align!(nla_buffer.length() as usize);
                Some(Ok(nla_buffer))
            }
            Err(e) => {
                // Make sure next time we call `next()`, we return None. We
                // don't try to continue iterating after we
                // failed to return a buffer.
                self.position = self.buffer.as_ref().len();
                Some(Err(e))
            }
        }
    }
}

/// Describes how to handle errors when parsing attributes.
pub enum NlaParseMode {
    /// Any attribute parsing errors result in a failure of the entire message
    /// parsing operation. Corresponds to the `NETLINK_GET_STRICT_CHK` option.
    Strict,
    /// Attribute parsing errors do not impact the success of message parsing.
    /// Attributes that failed to parse are ignored.
    Relaxed,
}

/// A type that can iterate over and parse Netlink attributes.
pub trait HasNlas {
    /// Returns an iterator over the Netlink attribute buffers in `self`, or
    /// errors where the attribute length is invalid.
    fn attributes(&self) -> impl Iterator<Item = Result<NlaBuffer<&[u8]>, NlaError>>;

    /// Parses the Netlink attributes from `self`, using `parse_fn` to parse
    /// each attribute from an attribute buffer.
    fn parse_attributes<'a, A, E>(
        &'a self,
        mode: NlaParseMode,
        mut parse_fn: impl FnMut(&NlaBuffer<&'a [u8]>) -> Result<A, E>,
    ) -> Result<Vec<A>, E>
    where
        E: From<NlaError>,
    {
        self.attributes()
            .filter_map(|nla_buf| {
                match nla_buf.map_err(|e| e.into()).map(|b| parse_fn(&b)).flatten() {
                    Ok(attr) => Some(Ok(attr)),
                    Err(e) => match mode {
                        NlaParseMode::Strict => Some(Err(e)),
                        NlaParseMode::Relaxed => None,
                    },
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use assert_matches::assert_matches;

    #[test]
    fn network_byteorder() {
        // The IPSET_ATTR_TIMEOUT attribute should have the network byte order
        // flag set. IPSET_ATTR_TIMEOUT(3600)
        static TEST_ATTRIBUTE: &[u8] = &[0x08, 0x00, 0x06, 0x40, 0x00, 0x00, 0x0e, 0x10];
        let buffer = NlaBuffer::new(TEST_ATTRIBUTE).unwrap();
        let buffer_is_net = buffer.network_byte_order_flag();
        let buffer_is_nest = buffer.nested_flag();

        let nla = DefaultNla::parse(&buffer).unwrap();
        let mut emitted_buffer = vec![0; nla.buffer_len()];

        nla.emit(&mut emitted_buffer);

        let attr_is_net = nla.is_network_byteorder();
        let attr_is_nest = nla.is_nested();

        let emit = NlaBuffer::new(emitted_buffer).unwrap();
        let emit_is_net = emit.network_byte_order_flag();
        let emit_is_nest = emit.nested_flag();

        assert_eq!([buffer_is_net, buffer_is_nest], [attr_is_net, attr_is_nest]);
        assert_eq!([attr_is_net, attr_is_nest], [emit_is_net, emit_is_nest]);
    }

    fn get_len() -> usize {
        // usize::MAX
        18446744073709551615
    }

    #[test]
    fn test_align() {
        assert_eq!(nla_align!(13), 16);
        assert_eq!(nla_align!(16), 16);
        assert_eq!(nla_align!(0), 0);
        assert_eq!(nla_align!(1), 4);
        assert_eq!(nla_align!(get_len() - 4), usize::MAX - 3);
    }
    #[test]
    #[should_panic]
    fn test_align_overflow() {
        assert_eq!(nla_align!(get_len() - 3), usize::MAX);
    }

    struct TestHasNlas(Vec<Result<Vec<u8>, NlaError>>);

    impl HasNlas for TestHasNlas {
        fn attributes(&self) -> impl Iterator<Item = Result<NlaBuffer<&[u8]>, NlaError>> {
            let Self(buffers) = self;
            buffers.iter().map(|res| match res {
                Ok(bytes) => Ok(NlaBuffer::new_unchecked(bytes.as_slice())),
                Err(e) => Err(e.clone()),
            })
        }
    }

    #[derive(Debug, PartialEq, Eq)]
    struct TestAttr(Vec<u8>);

    fn parse_test_attr(buf: &NlaBuffer<&[u8]>) -> Result<TestAttr, NlaError> {
        Ok(TestAttr(buf.inner().to_vec()))
    }

    #[test]
    fn parse_attributes_strict_fail_first() {
        let nlas =
            TestHasNlas(vec![Err(NlaError::InvalidLength { nla_len: 0 }), Ok(vec![1, 2, 3])]);

        assert_matches!(
            nlas.parse_attributes(NlaParseMode::Strict, parse_test_attr),
            Err(NlaError::InvalidLength { .. })
        );
    }

    #[test]
    fn parse_attributes_strict_fail_middle() {
        let nlas = TestHasNlas(vec![
            Ok(vec![1, 2, 3]),
            Err(NlaError::InvalidLength { nla_len: 0 }),
            Ok(vec![4, 5, 6]),
        ]);

        assert_matches!(
            nlas.parse_attributes(NlaParseMode::Strict, parse_test_attr),
            Err(NlaError::InvalidLength { .. })
        );
    }

    #[test]
    fn parse_attributes_strict_fail_last() {
        let nlas =
            TestHasNlas(vec![Ok(vec![1, 2, 3]), Err(NlaError::InvalidLength { nla_len: 0 })]);

        assert_matches!(
            nlas.parse_attributes(NlaParseMode::Strict, parse_test_attr),
            Err(NlaError::InvalidLength { .. })
        );
    }

    #[test]
    fn parse_attributes_strict_inner_fn_fails() {
        let nlas = TestHasNlas(vec![Ok(vec![1, 2, 3]), Ok(vec![]), Ok(vec![4, 5, 6])]);

        let res = nlas.parse_attributes(NlaParseMode::Strict, |buf| {
            if buf.inner().is_empty() {
                return Err(NlaError::BufferTooSmall { buffer_len: 0 });
            }
            Ok(TestAttr(buf.inner().to_vec()))
        });
        assert_matches!(res, Err(NlaError::BufferTooSmall { .. }));
    }

    #[test]
    fn parse_attributes_relaxed_fail_first() {
        let nlas =
            TestHasNlas(vec![Err(NlaError::InvalidLength { nla_len: 0 }), Ok(vec![1, 2, 3])]);

        assert_eq!(
            nlas.parse_attributes(NlaParseMode::Relaxed, parse_test_attr).unwrap(),
            vec![TestAttr(vec![1, 2, 3])]
        );
    }

    #[test]
    fn parse_attributes_relaxed_fail_middle() {
        let nlas = TestHasNlas(vec![
            Ok(vec![1, 2, 3]),
            Err(NlaError::InvalidLength { nla_len: 0 }),
            Ok(vec![4, 5, 6]),
        ]);

        assert_eq!(
            nlas.parse_attributes(NlaParseMode::Relaxed, parse_test_attr).unwrap(),
            vec![TestAttr(vec![1, 2, 3]), TestAttr(vec![4, 5, 6])]
        );
    }

    #[test]
    fn parse_attributes_relaxed_fail_last() {
        let nlas =
            TestHasNlas(vec![Ok(vec![1, 2, 3]), Err(NlaError::InvalidLength { nla_len: 0 })]);

        assert_eq!(
            nlas.parse_attributes(NlaParseMode::Relaxed, parse_test_attr).unwrap(),
            vec![TestAttr(vec![1, 2, 3])]
        );
    }

    #[test]
    fn parse_attributes_relaxed_inner_fn_fails() {
        let nlas = TestHasNlas(vec![Ok(vec![1, 2, 3]), Ok(vec![]), Ok(vec![4, 5, 6])]);

        assert_eq!(
            nlas.parse_attributes(NlaParseMode::Relaxed, |buf| {
                if buf.inner().is_empty() {
                    return Err(NlaError::BufferTooSmall { buffer_len: 0 });
                }
                Ok(TestAttr(buf.inner().to_vec()))
            })
            .unwrap(),
            vec![TestAttr(vec![1, 2, 3]), TestAttr(vec![4, 5, 6])]
        );
    }
}
