
use crate::{BytesRead, ReadError, BytesReadRef, BytesSeek, SeekError, Cursor};

use std::io;

/// A slice wrapper that implements BytesRead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Bytes<'a> {
	inner: Cursor<&'a [u8]>
}

impl<'a> Bytes<'a> {
	/// You should probably use:
	///
	/// ```
	/// # use simple_bytes::Bytes;
	/// # let slice = &[1u8, 2u8][..];
	/// let bytes: Bytes = slice.into();
	/// // or
	/// let bytes = Bytes::from(slice);
	/// ```
	///
	pub fn new(position: usize, inner: &'a [u8]) -> Self {
		let mut cursor = Cursor::new(inner);
		cursor.seek(position);
		Self { inner: cursor }
	}

	/// Returns the inner slice with the original reference.
	pub fn inner(&self) -> &'a [u8] {
		self.as_slice_ref()
	}
}

impl BytesRead for Bytes<'_> {
	// returns the full slice
	#[inline]
	fn as_slice(&self) -> &[u8] {
		self.inner.as_slice()
	}

	#[inline]
	fn remaining(&self) -> &[u8] {
		self.inner.remaining()
	}

	#[inline]
	fn try_read(&mut self, len: usize) -> Result<&[u8], ReadError> {
		self.inner.try_read(len)
	}

	#[inline]
	fn peek(&self, len: usize) -> Option<&[u8]> {
		self.inner.peek(len)
	}
}

impl io::Read for Bytes<'_> {
	fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
		io::Read::read(&mut self.inner, buf)
	}
}

impl<'a> BytesReadRef<'a> for Bytes<'a> {

	// returns the full slice
	#[inline]
	fn as_slice_ref(&self) -> &'a [u8] {
		*self.inner.inner()
	}

	#[inline]
	fn remaining_ref(&self) -> &'a [u8] {
		&self.as_slice_ref()[self.position()..]
	}

	#[inline]
	fn try_read_ref(&mut self, len: usize) -> Result<&'a [u8], ReadError> {
		let slice = &self.as_slice_ref()[self.position()..].get(..len)
			.ok_or(ReadError)?;
		// the previous line did not panic
		// so let's increase our position
		self.seek(self.position() + len);

		Ok(slice)
	}

	#[inline]
	fn peek_ref(&self, len: usize) -> Option<&'a [u8]> {
		self.remaining_ref().get(..len)
	}

}

impl BytesSeek for Bytes<'_> {
	/// Returns the internal position.
	fn position(&self) -> usize {
		self.inner.position()
	}

	/// Sets the internal position.
	/// 
	/// ## Fails
	/// If the position exceeds the slice.
	fn try_seek(&mut self, pos: usize) -> Result<(), SeekError> {
		self.inner.try_seek(pos)
	}
}

impl io::Seek for Bytes<'_> {
	fn seek(&mut self, pos: io::SeekFrom) -> io::Result<u64> {
		io::Seek::seek(&mut self.inner, pos)
	}
}

impl<'a> From<&'a [u8]> for Bytes<'a> {
	fn from(s: &'a [u8]) -> Self {
		Self::new(0, s)
	}
}

#[cfg(test)]
mod tests {

	use super::*;

	#[test]
	fn read() {

		let bytes: Vec<u8> = (0..=255).collect();
		let mut bytes = Bytes::from(bytes.as_slice());
		assert_eq!(bytes.as_slice(), bytes.as_slice());
		assert_eq!(bytes.len(), 256);
		assert_eq!(bytes.remaining().len(), 256);

		let to_read: Vec<u8> = (0..10).collect();
		assert_eq!(to_read.as_slice(), bytes.read(10));
		assert_eq!(bytes.remaining().len(), 256 - 10);

		assert_eq!(10u8, bytes.read_u8());

		// peek
		let to_peek: Vec<u8> = (11..=20).collect();
		assert_eq!(to_peek.as_slice(), bytes.peek(10).unwrap());

		bytes.seek(255);
		assert_eq!(255u8, bytes.read_u8());

		assert_eq!(bytes.position(), 256);
		bytes.seek(256);
	}

	#[test]
	fn read_le() {

		let b = u16::MAX - 20;
		println!("be: {:?}", b.to_be_bytes());
		let bytes = b.to_le_bytes();
		println!("le: {:?}", bytes);
		let mut bytes = Bytes::from(&bytes[..]);
		assert_ne!(bytes.read_u16(), b);
		bytes.seek(0);
		println!("buffer: {:?}", bytes.as_slice());
		assert_eq!(bytes.read_le_u16(), b);

	}

	#[test]
	fn test_empty() {
		let mut bytes = Bytes::from(&[][..]);
		assert_eq!(bytes.as_slice(), &[]);
		assert_eq!(bytes.len(), 0);
		bytes.seek(0);
	}

	#[test]
	#[should_panic]
	fn test_seek_empty() {
		let mut bytes = Bytes::from(&[][..]);
		bytes.seek(1);
	}

	#[test]
	#[should_panic]
	fn read_out_of_bound() {

		let bytes = [0u8; 100];
		let mut bytes = Bytes::from(&bytes[..]);

		bytes.seek(100);

		let _ = bytes.read_u8();
	}

	#[test]
	#[should_panic]
	fn seek_out_of_bound() {

		let bytes = [0u8; 100];
		let mut bytes = Bytes::from(&bytes[..]);

		bytes.seek(101);
	}

}