[![CI](https://github.com/soerenmeier/simple-bytes/actions/workflows/ci.yaml/badge.svg)](https://github.com/soerenmeier/simple-bytes/actions/workflows/ci.yaml)
[![crates.io](https://img.shields.io/crates/v/simple-bytes)](https://crates.io/crates/simple-bytes)
[![docs.rs](https://img.shields.io/docsrs/simple-bytes)](https://docs.rs/simple-bytes)

A small and easy crate to mutate or read u8 slices.

Reads or writes any number using the byte order "big-endian".

## Read a slice

```rust
use simple_bytes::{Bytes, BytesRead};

let bytes: Vec<u8> = (0..255).collect();
let mut slice: Bytes = bytes.as_slice().into();

assert_eq!(0, slice.read_u8());
assert_eq!(1, slice.read_u8());
assert_eq!(515, slice.read_u16());
```

## Write to a slice

```rust
use simple_bytes::{BytesMut, BytesRead, BytesWrite};

let mut bytes = [0u8; 10];
let mut slice = BytesMut::from(bytes.as_mut());

slice.write_u8(1);
slice.write_f32(1.234);
slice.write(&[1u8, 2u8]);
assert_eq!(3, slice.remaining().len());
```