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


/// A array wrapper that implements BytesWrite and BytesRead
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BytesArray<const N: usize> {
	inner: Cursor<[u8; N]>
}

impl<const N: usize> BytesArray<N> {
	/// You should probably use:
	///
	/// ```
	/// # use simple_bytes::BytesArray;
	/// # let arr = [1u8, 2u8];
	/// let bytes: BytesArray<2> = arr.into();
	/// // or
	/// let bytes = BytesArray::from(arr);
	/// ```
	///
	pub fn new(position: usize, inner: [u8; N]) -> Self {
		let mut cursor = Cursor::new(inner);
		cursor.seek(position);
		Self { inner: cursor }
	}

	/// Returns the underlying array mutably.
	#[inline]
	pub fn as_mut_array(&mut self) -> &mut [u8; N] {
		self.inner.inner_mut()
	}

	/// Returns the underlying Array.
	#[inline]
	pub fn into_array(self) -> [u8; N] {
		self.inner.into_inner()
	}

}

impl<const N: usize> BytesRead for BytesArray<N> {
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

impl<const N: usize> io::Read for BytesArray<N> {
	fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
		io::Read::read(&mut self.inner, buf)
	}
}

impl<const N: usize> BytesWrite for BytesArray<N> {
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

impl<const N: usize> io::Write for BytesArray<N> {
	fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
		io::Write::write(&mut self.inner, buf)
	}

	fn flush(&mut self) -> io::Result<()> {
		Ok(())
	}
}

impl<const N: usize> BytesSeek for BytesArray<N> {
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

impl<const N: usize> io::Seek for BytesArray<N> {
	fn seek(&mut self, pos: io::SeekFrom) -> io::Result<u64> {
		io::Seek::seek(&mut self.inner, pos)
	}
}

impl<const N: usize> From<[u8; N]> for BytesArray<N> {
	fn from(b: [u8; N]) -> Self {
		Self::new(0, b)
	}
}

impl<const N: usize> From<BytesArray<N>> for [u8; N] {
	fn from(b: BytesArray<N>) -> Self {
		b.into_array()
	}
}


#[cfg(test)]
mod tests {
	use super::*;
	use crate::BytesRead;

	#[test]
	fn write() {
		let mut bytes = BytesArray::from([0u8; 100]);
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
		let mut bytes = BytesArray::from([0u8; 2]);
		bytes.write_le_u16(b);
		assert_eq!(bytes.as_slice(), le);
	}

	#[test]
	fn test_empty() {
		let mut bytes = BytesArray::from([0u8; 0]);
		assert_eq!(bytes.as_slice(), &[]);
		assert_eq!(bytes.len(), 0);
		bytes.seek(0);
	}

	#[test]
	#[should_panic]
	fn write_overflow() {
		let mut bytes = BytesArray::from([0u8; 100]);

		// seek
		bytes.seek(100);

		bytes.write_u8(5u8);
	}
}