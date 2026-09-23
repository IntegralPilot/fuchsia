
use crate::{
	BytesRead, ReadError,
	BytesWrite, WriteError,
	BytesSeek, SeekError, Bytes
};
use crate::util::{io_eof, seek_from_to_n_pos, write_or_alloc};

use std::io;

/// A generic struct implementing BytesRead, BytesWrite and BytesSeek
/// for different types.
/// 
/// In the background Bytes, BytesMut and
/// BytesOwned use this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Cursor<T> {
	/// always points to the next position
	position: usize,
	inner: T
}

impl<T> Cursor<T> {
	/// Creates a new Cursor.
	pub fn new(inner: T) -> Self {
		Self {
			position: 0,
			inner
		}
	}

	/// Returns the inner value as a reference.
	pub fn inner(&self) -> &T {
		&self.inner
	}

	/// Returns the inner value as a mutable reference.
	/// Shrinking the inner len may lead to panics when reading
	/// or writing.
	pub fn inner_mut(&mut self) -> &mut T {
		&mut self.inner
	}

	/// Returns the inner value, discarding how many bytes
	/// were read or written.
	pub fn into_inner(self) -> T {
		self.inner
	}
}

impl<T> BytesRead for Cursor<T>
where T: AsRef<[u8]> {
	#[inline]
	fn as_slice(&self) -> &[u8] {
		self.inner.as_ref()
	}

	#[inline]
	fn remaining(&self) -> &[u8] {
		&self.as_slice()[self.position..]
	}

	#[inline]
	fn try_read(&mut self, len: usize) -> Result<&[u8], ReadError> {
		let slice = &self.inner.as_ref()[self.position..].get(..len)
			.ok_or(ReadError)?;
		self.position += len;

		Ok(slice)
	}

	fn peek(&self, len: usize) -> Option<&[u8]> {
		self.remaining().get(..len)
	}
}

impl<T> io::Read for Cursor<T>
where T: AsRef<[u8]> {
	fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
		let len = buf.len().min(self.remaining().len());
		buf[..len].copy_from_slice(BytesRead::read(self, len));

		Ok(len)
	}
}

impl<'a> BytesSeek for Cursor<&'a [u8]> {
	#[inline]
	fn position(&self) -> usize {
		self.position
	}

	/// Sets the internal position.
	/// 
	/// ## Fails
	/// If the position exceeds the slice.
	fn try_seek(&mut self, pos: usize) -> Result<(), SeekError> {
		let len = self.inner.len();
		if self.position + len >= pos {
			self.position = pos;
			Ok(())
		} else {
			Err(SeekError(len))
		}
	}
}



impl<'a> io::Seek for Cursor<&'a [u8]> {
	fn seek(&mut self, pos: io::SeekFrom) -> io::Result<u64> {
		let n_pos = seek_from_to_n_pos(self.inner.len(), self.position, pos)?;

		self.try_seek(n_pos)
			.map(|_| n_pos as u64)
			.map_err(io_eof)
	}
}


impl<'a> BytesWrite for Cursor<&'a mut [u8]> {
	fn as_mut(&mut self) -> &mut [u8] {
		self.inner
	}

	fn as_bytes(&self) -> Bytes<'_> {
		Bytes::new(0, &*self.inner)
	}

	fn remaining_mut(&mut self) -> &mut [u8] {
		&mut self.inner[self.position..]
	}

	fn try_write(&mut self, slice: impl AsRef<[u8]>) -> Result<(), WriteError> {
		let slice = slice.as_ref();
		self.remaining_mut().get_mut(..slice.len())
			.ok_or(WriteError)?
			.copy_from_slice(slice);
		self.position += slice.len();

		Ok(())
	}
}

impl<'a> io::Write for Cursor<&'a mut [u8]> {
	fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
		self.try_write(buf)
			.map_err(io_eof)?;
		Ok(buf.len())
	}

	fn flush(&mut self) -> io::Result<()> {
		Ok(())
	}
}

impl<'a> BytesSeek for Cursor<&'a mut [u8]> {
	fn position(&self) -> usize {
		self.position
	}

	/// Sets the internal position.
	/// 
	/// ## Fails
	/// If the position exceeds the slice.
	fn try_seek(&mut self, pos: usize) -> Result<(), SeekError> {
		let len = self.inner.len();
		if self.position + len >= pos {
			self.position = pos;
			Ok(())
		} else {
			Err(SeekError(len))
		}
	}
}

impl<'a> io::Seek for Cursor<&'a mut [u8]> {
	fn seek(&mut self, pos: io::SeekFrom) -> io::Result<u64> {
		let n_pos = seek_from_to_n_pos(self.inner.len(), self.position, pos)?;

		self.try_seek(n_pos)
			.map(|_| n_pos as u64)
			.map_err(io_eof)
	}
}


impl<const L: usize> BytesWrite for Cursor<[u8; L]> {
	fn as_mut(&mut self) -> &mut [u8] {
		&mut self.inner
	}

	fn as_bytes(&self) -> Bytes<'_> {
		Bytes::new(0, &self.inner)
	}

	fn remaining_mut(&mut self) -> &mut [u8] {
		&mut self.inner[self.position..]
	}

	fn try_write(&mut self, slice: impl AsRef<[u8]>) -> Result<(), WriteError> {
		let slice = slice.as_ref();
		self.remaining_mut().get_mut(..slice.len())
			.ok_or(WriteError)?
			.copy_from_slice(slice);
		self.position += slice.len();

		Ok(())
	}
}

impl<const L: usize> io::Write for Cursor<[u8; L]> {
	fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
		self.try_write(buf)
			.map_err(io_eof)?;
		Ok(buf.len())
	}

	fn flush(&mut self) -> io::Result<()> {
		Ok(())
	}
}

impl<const L: usize> BytesSeek for Cursor<[u8; L]> {
	fn position(&self) -> usize {
		self.position
	}

	/// Sets the internal position.
	/// 
	/// ## Fails
	/// If the position exceeds the slice.
	fn try_seek(&mut self, pos: usize) -> Result<(), SeekError> {
		let len = self.inner.len();
		if self.position + len >= pos {
			self.position = pos;
			Ok(())
		} else {
			Err(SeekError(len))
		}
	}
}

impl<const L: usize> io::Seek for Cursor<[u8; L]> {
	fn seek(&mut self, pos: io::SeekFrom) -> io::Result<u64> {
		let n_pos = seek_from_to_n_pos(self.inner.len(), self.position, pos)?;

		self.try_seek(n_pos)
			.map(|_| n_pos as u64)
			.map_err(io_eof)
	}
}


impl BytesWrite for Cursor<&mut Vec<u8>> {
	fn as_mut(&mut self) -> &mut [u8] {
		&mut self.inner
	}

	fn as_bytes(&self) -> Bytes<'_> {
		Bytes::new(0, &self.inner)
	}

	/// Returns the remaining mutable slice.
	/// 
	/// If an empty slice is returned, this does not mean
	/// you can't write anymore.
	fn remaining_mut(&mut self) -> &mut [u8] {
		&mut self.inner[self.position..]
	}

	/// Write a slice. Allocates more space if the slice is
	/// bigger than the `Vec`.
	fn try_write(&mut self, slice: impl AsRef<[u8]>) -> Result<(), WriteError> {
		self.position = write_or_alloc(
			self.inner,
			self.position,
			slice.as_ref()
		);

		Ok(())
	}
}

impl io::Write for Cursor<&mut Vec<u8>> {
	fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
		self.try_write(buf)
			.map_err(io_eof)?;
		Ok(buf.len())
	}

	fn flush(&mut self) -> io::Result<()> {
		Ok(())
	}
}

impl BytesSeek for Cursor<&mut Vec<u8>> {
	fn position(&self) -> usize {
		self.position
	}

	/// Sets the internal position, allocating more space
	/// if the position is bigger than the `Vec`.
	fn try_seek(&mut self, pos: usize) -> Result<(), SeekError> {
		self.position = pos;
		let n_len = self.position;
		if self.inner.len() < n_len {
			self.inner.resize(n_len, 0u8);
		}

		Ok(())
	}
}

impl io::Seek for Cursor<&mut Vec<u8>> {
	fn seek(&mut self, pos: io::SeekFrom) -> io::Result<u64> {
		let n_pos = seek_from_to_n_pos(self.inner.len(), self.position, pos)?;

		self.try_seek(n_pos)
			.map(|_| n_pos as u64)
			.map_err(io_eof)
	}
}


impl BytesWrite for Cursor<Vec<u8>> {
	fn as_mut(&mut self) -> &mut [u8] {
		&mut self.inner
	}

	fn as_bytes(&self) -> Bytes<'_> {
		Bytes::new(0, &self.inner)
	}

	/// Returns the remaining mutable slice.
	/// 
	/// If an empty slice is returned, this does not mean
	/// you can't write anymore.
	fn remaining_mut(&mut self) -> &mut [u8] {
		&mut self.inner[self.position..]
	}

	/// Write a slice. Allocates more space if the slice is
	/// bigger than the `Vec`.
	fn try_write(&mut self, slice: impl AsRef<[u8]>) -> Result<(), WriteError> {
		self.position = write_or_alloc(
			&mut self.inner,
			self.position,
			slice.as_ref()
		);

		Ok(())
	}
}

impl io::Write for Cursor<Vec<u8>> {
	fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
		self.try_write(buf)
			.map_err(io_eof)?;
		Ok(buf.len())
	}

	fn flush(&mut self) -> io::Result<()> {
		Ok(())
	}
}

impl BytesSeek for Cursor<Vec<u8>> {
	fn position(&self) -> usize {
		self.position
	}

	/// Sets the internal position, allocating more space
	/// if the position is bigger than the `Vec`.
	fn try_seek(&mut self, pos: usize) -> Result<(), SeekError> {
		self.position = pos;
		let n_len = self.position;
		if self.inner.len() < n_len {
			self.inner.resize(n_len, 0u8);
		}

		Ok(())
	}
}

impl io::Seek for Cursor<Vec<u8>> {
	fn seek(&mut self, pos: io::SeekFrom) -> io::Result<u64> {
		let n_pos = seek_from_to_n_pos(self.inner.len(), self.position, pos)?;

		self.try_seek(n_pos)
			.map(|_| n_pos as u64)
			.map_err(io_eof)
	}
}