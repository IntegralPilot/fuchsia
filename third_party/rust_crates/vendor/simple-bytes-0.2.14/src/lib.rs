
//! A small and easy crate to mutate or read u8 slices.
//!
//! Reads or writes any number using the byte order "big-endian".
//!
//! ## Read a slice
//!
//! ```
//! use simple_bytes::{Bytes, BytesRead};
//! 
//! let bytes: Vec<u8> = (0..255).collect();
//! let mut slice: Bytes = bytes.as_slice().into();
//!
//! assert_eq!(0, slice.read_u8());
//! assert_eq!(1, slice.read_u8());
//! assert_eq!(515, slice.read_u16());
//! ```
//!
//! ## Write to a slice
//!
//! ```
//! use simple_bytes::{BytesMut, BytesRead, BytesWrite};
//! 
//! let mut bytes = [0u8; 10];
//! let mut slice = BytesMut::from(bytes.as_mut());
//!
//! slice.write_u8(1);
//! slice.write_f32(1.234);
//! slice.write(&[1u8, 2u8]);
//! assert_eq!(3, slice.remaining().len());
//! ```

mod util;

mod cursor;
pub use cursor::Cursor;

mod offset;
pub use offset::Offset;

mod bytes;
pub use bytes::Bytes;

mod bytes_mut;
pub use bytes_mut::BytesMut;

mod bytes_owned;
pub use bytes_owned::BytesOwned;

mod bytes_array;
pub use bytes_array::BytesArray;

mod bytes_read;
pub use bytes_read::{BytesRead, ReadError, BytesReadRef};

mod bytes_write;
pub use bytes_write::{BytesWrite, WriteError};

mod bytes_seek;
pub use bytes_seek::{BytesSeek, SeekError};