//! # Note
//!	Internally there exists only one position
//! So if you read and write you should keep this in mind

use crate::{
	Bytes, Cursor,
	BytesRead, ReadError,
	BytesWrite, WriteError,
	BytesSeek, SeekError
};

use std::io;

/// A Vec wrapper that implements BytesWrite and BytesRead
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BytesOwned {
	inner: Cursor<Vec<u8>>
}

impl BytesOwned {

	/// Creates an empty Vec.
	pub fn new() -> Self {
		Self {
			inner: Cursor::new(vec![])
		}
	}

	/// Creates a new Vec with the given capacity.
	pub fn with_capacity(cap: usize) -> Self {
		Self {
			inner: Cursor::new(Vec::with_capacity(cap))
		}
	}

	/// Creates a BytesOwned struct.
	pub fn new_raw(position: usize, inner: Vec<u8>) -> Self {
		let mut cursor = Cursor::new(inner);
		cursor.seek(position);
		Self { inner: cursor }
	}

	/// Resizes the len to `new_len` allocates some more space if needed.
	pub fn resize(&mut self, new_len: usize) {
		self.inner.inner_mut().resize(new_len, 0);
		if self.inner.position() > new_len {
			self.inner.seek(new_len);
		}
	}

	/// Returns the underlying Vec mutably.
	/// 
	/// Removing items can lead to panics while
	/// reading or writing.
	#[inline]
	pub fn as_mut_vec(&mut self) -> &mut Vec<u8> {
		self.inner.inner_mut()
	}

	/// Returns the underlying Vec.
	#[inline]
	pub fn into_vec(self) -> Vec<u8> {
		self.inner.into_inner()
	}

}

impl BytesRead for BytesOwned {

	#[inline]
	fn as_slice(&self) -> &[u8] {
		self.inner.as_slice()
	}

	#[inline]
	fn len(&self) -> usize {
		self.inner.len()
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

impl io::Read for BytesOwned {
	fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
		io::Read::read(&mut self.inner, buf)
	}
}

impl BytesWrite for BytesOwned {

	#[inline]
	fn as_mut(&mut self) -> &mut [u8] {
		self.inner.as_mut()
	}

	#[inline]
	fn as_bytes(&self) -> Bytes<'_> {
		self.inner.as_bytes()
	}

	/// Returns the remaining mutable slice.
	/// 
	/// If an empty slice is returned, this does not mean
	/// you can't write anymore.
	#[inline]
	fn remaining_mut(&mut self) -> &mut [u8] {
		self.inner.remaining_mut()
	}

	/// Writes a slice. Allocates more space if the slice is
	/// bigger than the `Vec`.
	#[inline]
	fn try_write(&mut self, slice: impl AsRef<[u8]>) -> Result<(), WriteError> {
		self.inner.try_write(slice)
	}

}

impl io::Write for BytesOwned {
	fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
		io::Write::write(&mut self.inner, buf)
	}

	fn flush(&mut self) -> io::Result<()> {
		Ok(())
	}
}

impl BytesSeek for BytesOwned {
	/// Returns the internal position.
	fn position(&self) -> usize {
		self.inner.position()
	}

	/// Sets the internal position, allocating more space
	/// if the position is bigger than the `Vec`.
	fn try_seek(&mut self, pos: usize) -> Result<(), SeekError> {
		self.inner.try_seek(pos)
	}
}

impl io::Seek for BytesOwned {
	fn seek(&mut self, pos: io::SeekFrom) -> io::Result<u64> {
		io::Seek::seek(&mut self.inner, pos)
	}
}

impl From<Vec<u8>> for BytesOwned {
	fn from(b: Vec<u8>) -> Self {
		Self::new_raw(0, b)
	}
}

impl From<BytesOwned> for Vec<u8> {
	fn from(b: BytesOwned) -> Self {
		b.into_vec()
	}
}


#[cfg(test)]
mod tests {

	use super::*;


	#[test]
	fn write() {

		let mut bytes = BytesOwned::new();
		assert_eq!(bytes.len(), 0);

		let to_write: Vec<u8> = (0..10).collect();
		bytes.write(&to_write);
		bytes.write(&to_write);

		assert_eq!(bytes.len(), 20);

		assert_eq!(&bytes.as_mut()[..10], to_write.as_slice());
		assert_eq!(&bytes.as_mut()[10..20], to_write.as_slice());

		bytes.write_u8(5u8);
		bytes.write_u16(20u16);

		assert_eq!(bytes.len(), 23);

		// seek
		bytes.seek(20);
		assert_eq!(bytes.len(), 23);

		// seek
		bytes.seek(99);
		assert_eq!(bytes.len(), 99);
		// should now write to the 99 byte // this is the last byte
		// this will resize
		bytes.write_u8(5u8);
		assert_eq!(bytes.as_mut()[99], 5u8);
		assert_eq!(bytes.len(), 100);

	}

	#[test]
	fn read() {
		use crate::BytesRead;

		let bytes: Vec<u8> = (1..=255).collect();
		let mut bytes: BytesOwned = bytes.into();

		assert_eq!(bytes.as_slice(), bytes.as_slice());
		assert_eq!(bytes.len(), 255);
		assert_eq!(bytes.remaining().len(), 255);

		let to_read: Vec<u8> = (1..=10).collect();
		assert_eq!(to_read.as_slice(), bytes.read(10));
		assert_eq!(bytes.remaining().len(), 255 - 10);

		assert_eq!(11u8, bytes.read_u8());

		// peek
		let to_peek: Vec<u8> = (12..=20).collect();
		assert_eq!(to_peek.as_slice(), bytes.peek(to_peek.len()).unwrap());

		bytes.seek(255);
		assert_eq!(bytes.remaining().len(), 0);
		bytes.seek(254);
		assert_eq!(255u8, bytes.read_u8());

		bytes.seek(256);// should have allocated one byte
		bytes.seek(255);
		assert_eq!(0u8, bytes.read_u8());
	}

	#[test]
	fn test_empty() {
		let mut bytes = BytesOwned::new();
		assert_eq!(bytes.as_slice(), &[]);
		assert_eq!(bytes.len(), 0);
		bytes.seek(0);
		assert_eq!(bytes.len(), 0);
	}

	#[test]
	fn resize() {
		let mut bytes = BytesOwned::new();
		bytes.resize(4);
		assert_eq!(bytes.as_slice(), &[0, 0, 0, 0]);
		assert_eq!(bytes.position(), 0);
		bytes.write_u8(2);
		assert_eq!(bytes.as_slice(), &[2, 0, 0, 0]);
		bytes.seek(4);
		bytes.write_u8(2);
		assert_eq!(bytes.as_slice(), &[2, 0, 0, 0, 2]);
		bytes.resize(4);
		assert_eq!(bytes.as_slice(), &[2, 0, 0, 0]);
		assert!(bytes.try_read(1).is_err());
	}
}