// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// DO NOT EDIT.
// Generated from FIDL library `zither.constants` by zither, a Fuchsia platform tool.

pub const UINT8_ZERO: u8 = 0;

pub const UINT8_MAX_DEC: u8 = 255;

pub const UINT8_MAX_HEX: u8 = 0xff;

pub const INT8_ZERO: i8 = 0;

pub const INT8_MIN_DEC: i8 = -128;

pub const INT8_MIN_HEX: i8 = -0x80;

pub const INT8_MAX_DEC: i8 = 127;

pub const INT8_MAX_HEX: i8 = 0x7f;

pub const UINT16_ZERO: u16 = 0;

pub const UINT16_MAX_DEC: u16 = 65535;

pub const UINT16_MAX_HEX: u16 = 0xffff;

pub const INT16_ZERO: i16 = 0;

pub const INT16_MIN_DEC: i16 = -32768;

pub const INT16_MIN_HEX: i16 = -0x8000;

pub const INT16_MAX_DEC: i16 = 32767;

pub const INT16_MAX_HEX: i16 = 0x7fff;

pub const UINT32_ZERO: u32 = 0;

pub const UINT32_MAX_DEC: u32 = 4294967295;

pub const UINT32_MAX_HEX: u32 = 0xffffffff;

pub const INT32_ZERO: i32 = 0;

pub const INT32_MIN_DEC: i32 = -2147483648;

pub const INT32_MIN_HEX: i32 = -0x80000000;

pub const INT32_MAX_DEC: i32 = 2147483647;

pub const INT32_MAX_HEX: i32 = 0x7fffffff;

pub const UINT64_ZERO: u64 = 0;

pub const UINT64_MAX_DEC: u64 = 18446744073709551615;

pub const UINT64_MAX_HEX: u64 = 0xffffffffffffffff;

pub const INT64_ZERO: i64 = 0;

pub const INT64_MIN_DEC: i64 = -9223372036854775808;

pub const INT64_MIN_HEX: i64 = -0x8000000000000000;

pub const INT64_MAX_DEC: i64 = 9223372036854775807;

pub const INT64_MAX_HEX: i64 = 0x7fffffffffffffff;

pub const FALSE: bool = false;

pub const TRUE: bool = true;

pub const EMPTY_STRING: &str = "";

pub const BYTE_ZERO: u8 = 0;

pub const BINARY_VALUE: u8 = 0b10101111;

pub const LOWERCASE_HEX_VALUE: u64 = 0x1234abcd5678ffff;

pub const UPPERCASE_HEX_VALUE: u64 = 0x1234ABCD5678FFFF;

pub const LEADING_ZEROES_HEX_VALUE: u32 = 0x00000011;

pub const LEADING_ZEROES_DEC_VALUE: u32 = 0000000017;

pub const LEADING_ZEROES_BINARY_VALUE: u32 = 0b0000000000010001;

pub const BITWISE_OR_VALUE: u8 = 15; // 0b1000 | 0b0100 | 0b0010 | 0b0001

pub const NONEMPTY_STRING: &str = "this is a constant";

pub const DEFINITION_FROM_ANOTHER_CONSTANT: &str = NONEMPTY_STRING;

pub const BITWISE_OR_OF_OTHER_CONSTANTS: u8 = 175; // BINARY_VALUE | BITWISE_OR_VALUE | 0b1 | UINT8_ZERO

/// Constant with a one-line comment.
pub const CONSTANT_ONE_LINE_COMMENT: bool = true;

/// Constant
///
///     with
///         a
///           many-line
///             comment.
pub const CONSTANT_MANY_LINE_COMMENT: &str = "";
