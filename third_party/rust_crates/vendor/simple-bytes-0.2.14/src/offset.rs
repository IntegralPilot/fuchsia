
use crate::{
	BytesRead, ReadError, BytesWrite, WriteError, BytesSeek, SeekError, Bytes
};

/// A struct which holds a specific offset for any BytesRead,
/// BytesWrite or BytesSeek implementation.
///
/// ## Example
/// ```
/// # use simple_bytes::{Offset, BytesOwned, BytesRead, BytesWrite, BytesSeek};
/// let mut offset = Offset::new(BytesOwned::from(vec![1, 2, 3, 4]), 2);
/// assert_eq!(offset.as_slice(), &[3, 4]);
/// assert_eq!(offset.remaining(), &[3, 4]);
/// offset.write_u8(5);
/// assert_eq!(offset.as_slice(), &[5, 4]);
/// assert_eq!(offset.remaining(), &[4]);
/// offset.seek(2);
/// offset.write_u8(5);
/// assert_eq!(offset.as_slice(), &[5, 4, 5]);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Offset<T> {
	// the offset gets applied to the inner
	// position
	offset: usize,
	inner: T
}

impl<T> Offset<T> {

	/// Creates a new Cursor.
	///
	/// May panic if the offset is bigger than the inner len.
	/// Depending on inner.
	pub fn new(mut inner: T, offset: usize) -> Self
	where T: BytesRead + BytesSeek {
		inner.seek(inner.position() + offset);
		Self { inner, offset }
	}

	/// Updates the offset.
	///
	/// Maybe panic if there aren't enough bytes left.
	pub fn set_offset(&mut self, offset: usize)
	where T: BytesSeek {
		let prev_pos = self.inner.position() - self.offset;
		self.inner.seek(prev_pos + offset);
		self.offset = offset;
	}

	/// Returns the current offset.
	pub fn offset(&self) -> usize {
		self.offset
	}

	/// Returns the inner value as a reference.
	pub fn inner(&self) -> &T {
		&self.inner
	}

	/// Returns the inner value as a mutable reference.
	/// Shrinking the inner len or updating the position may lead
	/// to panics when reading or writing.
	pub fn inner_mut(&mut self) -> &mut T {
		&mut self.inner
	}

	/// Returns the inner value, discarding the offset.
	pub fn into_inner(self) -> T {
		self.inner
	}

}

impl<T> BytesRead for Offset<T>
where T: BytesRead {
	#[inline]
	fn as_slice(&self) -> &[u8] {
		&self.inner.as_slice()[self.offset..]
	}

	#[inline]
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

impl<T> BytesSeek for Offset<T>
where T: BytesSeek {
	/// Returns the internal position.
	fn position(&self) -> usize {
		self.inner.position() - self.offset
	}

	/// Sets the internal position.
	/// 
	/// ## Panics
	/// Depending on the implementation.
	fn try_seek(&mut self, pos: usize) -> Result<(), SeekError> {
		self.inner.try_seek(self.offset + pos)
	}
}

impl<T> BytesWrite for Offset<T>
where T: BytesWrite {
	fn as_mut(&mut self) -> &mut [u8] {
		&mut self.inner.as_mut()[self.offset..]
	}

	fn as_bytes(&self) -> Bytes<'_> {
		self.inner.as_bytes().inner()[self.offset..].into()
	}

	fn remaining_mut(&mut self) -> &mut [u8] {
		self.inner.remaining_mut()
	}

	fn try_write(&mut self, slice: impl AsRef<[u8]>) -> Result<(), WriteError> {
		self.inner.try_write(slice)
	}
}



#[cfg(test)]
mod tests {

	use super::*;
	use crate::Cursor;

	#[test]
	fn write() {

		let cursor = Cursor::new(vec![1, 2, 3, 4]);
		let mut offset_cursor = Offset::new(cursor, 2);
		assert_eq!(offset_cursor.remaining_mut().len(), 2);
		offset_cursor.write(&[1]);
		assert_eq!(offset_cursor.remaining_mut().len(), 1);
		offset_cursor.write(&[2]);
		assert_eq!(offset_cursor.remaining_mut().len(), 0);
		offset_cursor.write(&[1, 2]);

		assert_eq!(offset_cursor.as_mut(), &[1, 2, 1, 2]);

	}

	#[test]
	fn read() {

		let cursor = Cursor::new(vec![1, 2, 3, 4]);
		let mut offset_cursor = Offset::new(cursor, 2);
		assert_eq!(offset_cursor.position(), 0);
		offset_cursor.seek(1);
		assert_eq!(offset_cursor.position(), 1);
		assert_eq!(offset_cursor.as_slice(), &[3, 4]);
		assert_eq!(offset_cursor.remaining(), &[4]);

		offset_cursor.set_offset(1);
		assert_eq!(offset_cursor.position(), 1);
		assert_eq!(offset_cursor.as_slice(), &[2, 3, 4]);
		assert_eq!(offset_cursor.remaining(), &[3, 4]);

	}

}