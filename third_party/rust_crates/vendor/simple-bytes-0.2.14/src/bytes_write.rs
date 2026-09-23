
use crate::Bytes;

use std::fmt;

macro_rules! write_fn {
	($name:ident, $try_name:ident, $type:ident) => (
		write_fn!($name, $try_name, $type, stringify!($type));
	);
	($name:ident, $try_name:ident, $type:ident, $type_str:expr) => {
		#[inline]
		#[doc = "Try to write "]
		#[doc = $type_str]
		#[doc = " in big-endian.`"]
		fn $try_name(&mut self, num: $type) -> Result<(), WriteError> {
			self.try_write(num.to_be_bytes())
		}

		#[inline]
		#[track_caller]
		#[doc = "Writes an `"]
		#[doc = $type_str]
		#[doc = "` in big-endian."]
		/// 
		/// ## Panics
		/// If there aren't enough remaining bytes left.
		fn $name(&mut self, num: $type) {
			self.$try_name(num).expect("failed to write")
		}
	}
}

macro_rules! write_le_fn {
	($name:ident, $try_name:ident, $type:ident) => (
		write_le_fn!($name, $try_name, $type, stringify!($type));
	);
	($name:ident, $try_name:ident, $type:ident, $type_str:expr) => {
		#[inline]
		#[doc = "Try to write "]
		#[doc = $type_str]
		#[doc = " in little-endian.`"]
		fn $try_name(&mut self, num: $type) -> Result<(), WriteError> {
			self.try_write(num.to_le_bytes())
		}

		#[inline]
		#[track_caller]
		#[doc = "Writes an `"]
		#[doc = $type_str]
		#[doc = "` in little-endian."]
		/// 
		/// ## Panics
		/// If there aren't enough remaining bytes left.
		fn $name(&mut self, num: $type) {
			self.$try_name(num).expect("failed to write")
		}
	}
}

/// Get's returned when there is not enough space to write everything.
/// If this get's returned nothing should be written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriteError;

impl fmt::Display for WriteError {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		fmt::Debug::fmt(self, f)
	}
}

impl std::error::Error for WriteError {}

/// Write bytes or numbers.
pub trait BytesWrite {
	/// Returns the entire slice mutably.
	fn as_mut(&mut self) -> &mut [u8];

	/// Returns the entire slice as a bytes struct
	/// setting the position of the new Bytes to `0`.
	fn as_bytes(&self) -> Bytes<'_>;

	/// Returns the remaining bytes mutably.
	fn remaining_mut(&mut self) -> &mut [u8];

	/// Writes a slice.
	fn try_write(&mut self, slice: impl AsRef<[u8]>) -> Result<(), WriteError>;

	/// Writes a slice.
	/// 
	/// ## Panics
	/// If there aren't enough remaining bytes left.
	#[track_caller]
	fn write(&mut self, slice: impl AsRef<[u8]>) {
		self.try_write(slice).expect("failed to write")
	}

	write_fn!(write_u8, try_write_u8, u8);
	write_fn!(write_u16, try_write_u16, u16);
	write_fn!(write_u32, try_write_u32, u32);
	write_fn!(write_u64, try_write_u64, u64);
	write_fn!(write_u128, try_write_u128, u128);

	write_fn!(write_i8, try_write_i8, i8);
	write_fn!(write_i16, try_write_i16, i16);
	write_fn!(write_i32, try_write_i32, i32);
	write_fn!(write_i64, try_write_i64, i64);
	write_fn!(write_i128, try_write_i128, i128);

	write_fn!(write_f32, try_write_f32, f32);
	write_fn!(write_f64, try_write_f64, f64);

	write_le_fn!(write_le_u8, try_write_le_u8, u8);
	write_le_fn!(write_le_u16, try_write_le_u16, u16);
	write_le_fn!(write_le_u32, try_write_le_u32, u32);
	write_le_fn!(write_le_u64, try_write_le_u64, u64);
	write_le_fn!(write_le_u128, try_write_le_u128, u128);

	write_le_fn!(write_le_i8, try_write_le_i8, i8);
	write_le_fn!(write_le_i16, try_write_le_i16, i16);
	write_le_fn!(write_le_i32, try_write_le_i32, i32);
	write_le_fn!(write_le_i64, try_write_le_i64, i64);
	write_le_fn!(write_le_i128, try_write_le_i128, i128);

	write_le_fn!(write_le_f32, try_write_le_f32, f32);
	write_le_fn!(write_le_f64, try_write_le_f64, f64);
}

impl<W: BytesWrite> BytesWrite for &mut W {
	#[inline]
	fn as_mut(&mut self) -> &mut [u8] {
		(**self).as_mut()
	}

	#[inline]
	fn as_bytes(&self) -> Bytes<'_> {
		(**self).as_bytes()
	}

	#[inline]
	fn remaining_mut(&mut self) -> &mut [u8] {
		(**self).remaining_mut()
	}

	#[inline]
	fn try_write(&mut self, slice: impl AsRef<[u8]>) -> Result<(), WriteError> {
		(**self).try_write(slice)
	}
}