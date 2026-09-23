
use std::fmt;

macro_rules! read_fn {
	($name:ident, $try_name:ident, $type:ident, $num:expr) => (
		read_fn!(
			$name, $try_name,
			$type, $num, stringify!($type), stringify!($num)
		);
	);
	($name:ident, $try_name:ident, $type:ident, $num:expr,
	$type_str:expr, $num_str:expr) => {
		#[inline]
		#[doc = "Try to read "]
		#[doc = $num_str]
		#[doc = " bytes in big-endian converting them into an `"]
		#[doc = $type_str]
		#[doc = "`."]
		fn $try_name(&mut self) -> Result<$type, ReadError> {
			self.try_read($num)?
				.try_into()
				.map($type::from_be_bytes)
				.map_err(|_| ReadError)
		}

		#[inline]
		#[track_caller]
		#[doc = "Reads "]
		#[doc = $num_str]
		#[doc = " bytes in big-endian converting them into an `"]
		#[doc = $type_str]
		#[doc = "`."]
		///
		/// ## Panics
		/// If there aren't enough bytes left.
		fn $name(&mut self) -> $type {
			self.$try_name().expect(concat!("failed to read ", $type_str))
		}
	}
}

macro_rules! read_le_fn {
	($name:ident, $try_name:ident, $type:ident, $num:expr) => (
		read_le_fn!(
			$name, $try_name,
			$type, $num, stringify!($type), stringify!($num)
		);
	);
	($name:ident, $try_name:ident, $type:ident, $num:expr,
	$type_str:expr, $num_str:expr) => {
		#[inline]
		#[doc = "Try to read "]
		#[doc = $num_str]
		#[doc = " bytes in little-endian converting them into an `"]
		#[doc = $type_str]
		#[doc = "`."]
		fn $try_name(&mut self) -> Result<$type, ReadError> {
			self.try_read($num)?
				.try_into()
				.map($type::from_le_bytes)
				.map_err(|_| ReadError)
		}

		#[inline]
		#[track_caller]
		#[doc = "Reads "]
		#[doc = $num_str]
		#[doc = " bytes in little-endian converting them into an `"]
		#[doc = $type_str]
		#[doc = "`."]
		///
		/// ## Panics
		/// If there aren't enough bytes left.
		fn $name(&mut self) -> $type {
			self.$try_name().expect(concat!("failed to read ", $type_str))
		}
	}
}

/// Get's returned when there is not enough space to read everything.
/// If this get's returned nothing was read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadError;

impl fmt::Display for ReadError {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		fmt::Debug::fmt(self, f)
	}
}

impl std::error::Error for ReadError {}

/// Read bytes or numbers.
pub trait BytesRead {
	/// Returns the entire slice.
	fn as_slice(&self) -> &[u8];

	/// Returns the length of the entire slice.
	#[inline]
	fn len(&self) -> usize {
		self.as_slice().len()
	}

	/// Returns all remaining bytes.
	fn remaining(&self) -> &[u8];

	/// Try to read a given length of bytes.
	/// 
	/// ## Failes
	/// If len exceeds `self.remaining().len()`.
	fn try_read(&mut self, len: usize) -> Result<&[u8], ReadError>;

	/// Reads a given length of bytes.
	/// 
	/// ## Panics
	/// If len exceeds `self.remaining().len()`.
	#[track_caller]
	fn read(&mut self, len: usize) -> &[u8] {
		self.try_read(len).expect("failed to read")
	}

	read_fn!(read_u8, try_read_u8, u8, 1);
	read_fn!(read_u16, try_read_u16, u16, 2);
	read_fn!(read_u32, try_read_u32, u32, 4);
	read_fn!(read_u64, try_read_u64, u64, 8);
	read_fn!(read_u128, try_read_u128, u128, 16);

	read_fn!(read_i8, try_read_i8, i8, 1);
	read_fn!(read_i16, try_read_i16, i16, 2);
	read_fn!(read_i32, try_read_i32, i32, 4);
	read_fn!(read_i64, try_read_i64, i64, 8);
	read_fn!(read_i128, try_read_i128, i128, 16);

	read_fn!(read_f32, try_read_f32, f32, 4);
	read_fn!(read_f64, try_read_f64, f64, 8);

	read_le_fn!(read_le_u8, try_read_le_u8, u8, 1);
	read_le_fn!(read_le_u16, try_read_le_u16, u16, 2);
	read_le_fn!(read_le_u32, try_read_le_u32, u32, 4);
	read_le_fn!(read_le_u64, try_read_le_u64, u64, 8);
	read_le_fn!(read_le_u128, try_read_le_u128, u128, 16);

	read_le_fn!(read_le_i8, try_read_le_i8, i8, 1);
	read_le_fn!(read_le_i16, try_read_le_i16, i16, 2);
	read_le_fn!(read_le_i32, try_read_le_i32, i32, 4);
	read_le_fn!(read_le_i64, try_read_le_i64, i64, 8);
	read_le_fn!(read_le_i128, try_read_le_i128, i128, 16);

	read_le_fn!(read_le_f32, try_read_le_f32, f32, 4);
	read_le_fn!(read_le_f64, try_read_le_f64, f64, 8);

	/// Tries to read a given length without updating
	/// the internal position. Returns `None` if there are not enought
	/// bytes remaining.
	fn peek(&self, len: usize) -> Option<&[u8]>;
}

impl<R: BytesRead> BytesRead for &mut R {
	#[inline]
	fn as_slice(&self) -> &[u8] {
		(**self).as_slice()
	}

	#[inline]
	fn remaining(&self) -> &[u8] {
		(**self).remaining()
	}

	#[inline]
	fn try_read(&mut self, len: usize) -> Result<&[u8], ReadError> {
		(**self).try_read(len)
	}

	#[inline]
	fn peek(&self, len: usize) -> Option<&[u8]> {
		(**self).peek(len)
	}
}

/// Read bytes while keeping the original reference.
/// ```
/// use simple_bytes::{Bytes, BytesRead, BytesReadRef};
///
/// let mut bytes = Bytes::from("hey".as_ref());
/// let h = bytes.read_u8();
/// let ey: &'static [u8] = bytes.remaining_ref();
/// ```
pub trait BytesReadRef<'a>: BytesRead {
	/// Returns the entire slice.
	fn as_slice_ref(&self) -> &'a [u8];

	/// Returns all remaining bytes.
	fn remaining_ref(&self) -> &'a [u8];

	/// Try to read a given length of bytes.
	/// 
	/// ## Failes
	/// If len exceeds `self.remaining().len()`.
	fn try_read_ref(&mut self, len: usize) -> Result<&'a [u8], ReadError>;

	/// Reads a given length of bytes.
	/// 
	/// ## Panics
	/// If len exceeds `self.remaining().len()`.
	#[track_caller]
	fn read_ref(&mut self, len: usize) -> &'a [u8] {
		self.try_read_ref(len).expect("failed to read")
	}

	/// Tries to read a given length without updating
	/// the internal position. Returns `None` if there are not enought
	/// bytes remaining.
	fn peek_ref(&self, len: usize) -> Option<&'a [u8]>;
}

impl<'a, R: BytesReadRef<'a>> BytesReadRef<'a> for &mut R {
	#[inline]
	fn as_slice_ref(&self) -> &'a [u8] {
		(**self).as_slice_ref()
	}

	#[inline]
	fn remaining_ref(&self) -> &'a [u8] {
		(**self).remaining_ref()
	}

	#[inline]
	fn try_read_ref(&mut self, len: usize) -> Result<&'a [u8], ReadError> {
		(**self).try_read_ref(len)
	}

	#[inline]
	fn peek_ref(&self, len: usize) -> Option<&'a [u8]> {
		(**self).peek_ref(len)
	}
}