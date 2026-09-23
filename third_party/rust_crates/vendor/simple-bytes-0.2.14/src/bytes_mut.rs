
use crate::{
	Bytes, Cursor,
	BytesRead, ReadError,
	BytesWrite, WriteError,
	BytesSeek, SeekError
};

use std::io;

/// A mutable slice wrapper that implements BytesWrite
#[derive(Debug, PartialEq, Eq, Hash)]
pub struct BytesMut<'a> {
	inner: Cursor<&'a mut [u8]>
}

impl<'a> BytesMut<'a> {
	/// You should probably use:
	///
	/// ```
	/// # use simple_bytes::BytesMut;
	/// # let slice = &mut [1u8, 2u8][..];
	/// let bytes: BytesMut = slice.into();
	/// // or
	/// let bytes = BytesMut::from(slice);
	/// ```
	///
	pub fn new(position: usize, inner: &'a mut [u8]) -> Self {
		let mut cursor = Cursor::new(inner);
		cursor.seek(position);
		Self { inner: cursor }
	}
}

impl BytesRead for BytesMut<'_> {
	#[inline]
	fn as_slice(&self) -> &[u8] {
		self.inner.as_slice()
	}

	fn remaining(&self) -> &[u8] {
		self.inner.remaining()
	}

	fn try_read(&mut self, len: usize) -> Result<&[u8], ReadError> {
		self.inner.try_read(len)
	}

	fn peek(&self, len: usize) -> Option<&[u8]> {
		self.inner.peek(len)
	}
}

impl io::Read for BytesMut<'_> {
	fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
		io::Read::read(&mut self.inner, buf)
	}
}

impl BytesWrite for BytesMut<'_> {
	#[inline]
	fn as_mut(&mut self) -> &mut [u8] {
		self.inner.as_mut()
	}

	#[inline]
	fn as_bytes(&self) -> Bytes<'_> {
		self.inner.as_bytes()
	}

	#[inline]
	fn remaining_mut(&mut self) -> &mut [u8] {
		self.inner.remaining_mut()
	}

	#[inline]
	fn try_write(&mut self, slice: impl AsRef<[u8]>) -> Result<(), WriteError> {
		self.inner.try_write(slice)
	}
}

impl io::Write for BytesMut<'_> {
	fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
		io::Write::write(&mut self.inner, buf)
	}

	fn flush(&mut self) -> io::Result<()> {
		Ok(())
	}
}

impl BytesSeek for BytesMut<'_> {
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

impl io::Seek for BytesMut<'_> {
	fn seek(&mut self, pos: io::SeekFrom) -> io::Result<u64> {
		io::Seek::seek(&mut self.inner, pos)
	}
}

impl<'a> From<&'a mut [u8]> for BytesMut<'a> {
	fn from(s: &'a mut [u8]) -> Self {
		Self::new(0, s)
	}
}


#[cfg(test)]
mod tests {

	use super::*;
	use crate::BytesRead;

	#[test]
	fn write() {

		let mut bytes = [0u8; 100];
		let mut bytes = BytesMut::from(&mut bytes[..]);
		assert_eq!(bytes.len(), 100);

		let to_write: Vec<u8> = (0..10).collect();
		bytes.write(&to_write);
		bytes.write(&to_write);

		assert_eq!(bytes.remaining().len(), 100 - 20);
		assert_eq!(bytes.remaining().len(), bytes.remaining_mut().len());

		assert_eq!(&bytes.as_mut()[..10], to_write.as_slice());
		assert_eq!(&bytes.as_bytes().peek(20).unwrap()[10..], to_write.as_slice());

		bytes.write_u8(5u8);
		bytes.write_u16(20u16);

		assert_eq!(bytes.remaining_mut().len(), 100 - 23);

		// seek
		bytes.seek(99);
		// should now write to the 99 byte // this is the last byte
		bytes.write_u8(5u8);
		assert_eq!(bytes.remaining_mut().len(), 0);
		assert_eq!(bytes.as_mut()[99], 5u8);

	}

	#[test]
	fn write_le() {
		let b = u16::MAX - 20;
		let le = b.to_le_bytes();
		let mut bytes = [0u8; 2];
		let mut bytes = BytesMut::from(bytes.as_mut());
		bytes.write_le_u16(b);
		assert_eq!(bytes.as_slice(), le);
	}

	#[test]
	fn test_empty() {
		let mut bytes = BytesMut::from(&mut [][..]);
		assert_eq!(bytes.as_slice(), &[]);
		assert_eq!(bytes.len(), 0);
		bytes.seek(0);
	}

	#[test]
	#[should_panic]
	fn write_overflow() {

		let mut bytes = [0u8; 100];
		let mut bytes = BytesMut::from(&mut bytes[..]);

		// seek
		bytes.seek(100);

		bytes.write_u8(5u8);
	}

}