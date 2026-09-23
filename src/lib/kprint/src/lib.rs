// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![no_std]

#[allow(unused_extern_crates)]
extern crate self as kprint;

pub mod backend;

#[doc(hidden)]
pub use kprint_macro::{__kformat_internal, __kprint_internal, __kprintln_internal};

/// Prints formatted text directly to standard kernel console output (via C `printf`).
#[macro_export]
macro_rules! kprint {
    ($($tt:tt)*) => {
        $crate::__kprint_internal!($crate, $($tt)*)
    };
}

/// Prints formatted text followed by a newline to standard kernel console output (via C `printf`).
#[macro_export]
macro_rules! kprintln {
    ($($tt:tt)*) => {
        $crate::__kprintln_internal!($crate, $($tt)*)
    };
}

/// Formats text into a provided buffer (via C `snprintf`) and returns a byte slice (`&[u8]`).
#[macro_export]
macro_rules! kformat {
    ($($tt:tt)*) => {
        $crate::__kformat_internal!($crate, $($tt)*)
    };
}

#[cfg(test)]
mod tests {
    #[allow(unused_imports)]
    use super::*;

    #[test]
    fn test_signed_integers() {
        let mut buf = [0u8; 256];
        assert_eq!(kformat!(&mut buf, "val: {}", -12345), b"val: -12345");
        assert_eq!(kformat!(&mut buf, "val: {:+d}", 42), b"val: +42");
        assert_eq!(kformat!(&mut buf, "val: {: d}", 42), b"val:  42");
        assert_eq!(kformat!(&mut buf, "val: '{:<8d}'", 42), b"val: '42      '");
        assert_eq!(kformat!(&mut buf, "val: '{:>8d}'", 42), b"val: '      42'");
        assert_eq!(kformat!(&mut buf, "val: {:05d}", 42), b"val: 00042");

        let val_i8: i8 = -8;
        let val_i16: i16 = -16;
        let val_i32: i32 = -32;
        let val_i64: i64 = -64;
        let val_isize: isize = -100;
        assert_eq!(
            kformat!(&mut buf, "{} {} {} {} {}", &val_i8, &val_i16, val_i32, val_i64, val_isize),
            b"-8 -16 -32 -64 -100"
        );
        assert_eq!(kformat!(&mut buf, "min i64: {}", i64::MIN), b"min i64: -9223372036854775808");
    }

    #[test]
    fn test_unsigned_and_hex() {
        let mut buf = [0u8; 256];
        assert_eq!(kformat!(&mut buf, "val: {:u}", 12345u32), b"val: 12345");
        assert_eq!(kformat!(&mut buf, "val: {:x}", 0xabcd), b"val: abcd");
        assert_eq!(kformat!(&mut buf, "val: {:X}", 0xabcd), b"val: ABCD");
        assert_eq!(kformat!(&mut buf, "val: {:#x}", 0), b"val: 0x0");
        assert_eq!(kformat!(&mut buf, "val: {:#x}", 0x2a), b"val: 0x2a");
        assert_eq!(kformat!(&mut buf, "val: {:#X}", 0x2a), b"val: 0X2A");
        assert_eq!(kformat!(&mut buf, "val: {:#08X}", 0x1A), b"val: 0X00001A");
        assert_eq!(kformat!(&mut buf, "val: {:o}", 64), b"val: 100");

        let val_u8: u8 = 8;
        let val_u16: u16 = 16;
        let val_u32: u32 = 32;
        let val_u64: u64 = 64;
        let val_usize: usize = 128;
        assert_eq!(
            kformat!(
                &mut buf,
                "{:u} {:u} {:u} {:u} {:u}",
                &val_u8,
                val_u16,
                val_u32,
                val_u64,
                val_usize
            ),
            b"8 16 32 64 128"
        );
        assert_eq!(kformat!(&mut buf, "max u64: {:u}", u64::MAX), b"max u64: 18446744073709551615");
    }

    #[test]
    fn test_strings_and_cstrings() {
        let mut buf = [0u8; 256];
        assert_eq!(kformat!(&mut buf, "hello {}", "world"), b"hello world");
        let s = "fuchsia";
        assert_eq!(kformat!(&mut buf, "os: {:s}", s), b"os: fuchsia");
        let sub = "pigweed kernel";
        assert_eq!(kformat!(&mut buf, "sub: {:s}", &sub[0..7]), b"sub: pigweed");
        assert_eq!(kformat!(&mut buf, "prec: {:.5s}", "123456789"), b"prec: 12345");

        let byte_slice: &[u8] = b"bytes";
        assert_eq!(kformat!(&mut buf, "raw: {:s}", byte_slice), b"raw: bytes");

        let fixed_array: [u8; 4] = [b'a', b'b', b'c', b'd'];
        assert_eq!(kformat!(&mut buf, "arr: {:s}", fixed_array), b"arr: abcd");

        let c_str = c"zircon-cstr";
        assert_eq!(kformat!(&mut buf, "cstr: {:s}", c_str), b"cstr: zircon-cstr");

        let raw_c_ptr = c"raw-c-ptr".as_ptr();
        assert_eq!(kformat!(&mut buf, "raw_ptr: {:cs}", raw_c_ptr), b"raw_ptr: raw-c-ptr");
        assert_eq!(kformat!(&mut buf, "aligned: {:<12cs}!", raw_c_ptr), b"aligned: raw-c-ptr   !");
    }

    #[test]
    fn test_pointers() {
        let mut buf = [0u8; 256];
        let val: i32 = 42;
        let ptr = &val as *const i32;
        let res = kformat!(&mut buf, "ptr: {:p}", ptr);
        assert!(res.starts_with(b"ptr: 0x") || res.starts_with(b"ptr: (nil)"));

        let usize_addr: usize = 0x12345678;
        let res2 = kformat!(&mut buf, "addr: {:p}", usize_addr);
        let res2_str = core::str::from_utf8(res2).unwrap();
        assert!(res2_str.contains("12345678"));

        let null_ptr: *const core::ffi::c_void = core::ptr::null();
        let res_null = kformat!(&mut buf, "null: {:p}", null_ptr);
        let res_null_str = core::str::from_utf8(res_null).unwrap();
        assert!(
            res_null_str.contains("0x0")
                || res_null_str.contains("(nil)")
                || res_null_str.contains("00000000")
                || res_null_str.contains("0")
        );
    }

    #[test]
    fn test_single_evaluation() {
        let mut buf = [0u8; 256];
        let mut eval_count = 0;
        let mut side_effect_fn = || {
            eval_count += 1;
            "single_eval"
        };
        let res = kformat!(&mut buf, "result: {:s}", side_effect_fn());
        assert_eq!(res, b"result: single_eval");
        assert_eq!(eval_count, 1);

        let mut bool_eval_count = 0;
        let mut bool_side_effect_fn = || {
            bool_eval_count += 1;
            true
        };
        let res_b = kformat!(&mut buf, "flag: {:b}", bool_side_effect_fn());
        assert_eq!(res_b, b"flag: true");
        assert_eq!(bool_eval_count, 1);

        let mut multi_eval_count = 0;
        let mut multi_side_effect_fn = || {
            multi_eval_count += 1;
            100
        };
        let res_multi = kformat!(&mut buf, "{0} + {0} = 200", multi_side_effect_fn());
        assert_eq!(res_multi, b"100 + 100 = 200");
        assert_eq!(multi_eval_count, 1);
    }

    #[test]
    fn test_concat_format_string() {
        let mut buf = [0u8; 256];
        let val = 42;
        assert_eq!(
            kformat!(&mut buf, concat!("prefix_", "status: ", "{}"), val),
            b"prefix_status: 42"
        );
        assert_eq!(
            kformat!(&mut buf, concat!("test_", 1, "_", true, "_", 'c', "={}"), 99),
            b"test_1_true_c=99"
        );
    }

    #[test]
    fn test_captured_and_named_variables() {
        let mut buf = [0u8; 256];
        let user = "alice";
        let score = 100;
        assert_eq!(
            kformat!(&mut buf, "player {user:s} has score {score}"),
            b"player alice has score 100"
        );
        assert_eq!(
            kformat!(&mut buf, "{greeting:s}, {name:s}!", greeting = "hello", name = "world"),
            b"hello, world!"
        );
    }

    #[test]
    fn test_positional_arguments() {
        let mut buf = [0u8; 256];
        assert_eq!(kformat!(&mut buf, "{0} + {1} = {0}", "a", "b"), b"a + b = a");
        assert_eq!(kformat!(&mut buf, "{1} {0} {1}", 10, 20), b"20 10 20");
    }

    #[test]
    fn test_escapes_and_percent() {
        let mut buf = [0u8; 256];
        assert_eq!(kformat!(&mut buf, "{{hello}}"), b"{hello}");
        assert_eq!(kformat!(&mut buf, "100% completed"), b"100% completed");
        assert_eq!(kformat!(&mut buf, "{}% done", 75), b"75% done");
        assert_eq!(kformat!(&mut buf, "{{{}%}}", 50), b"{50%}");
    }

    #[test]
    fn test_chars() {
        let mut buf = [0u8; 256];
        assert_eq!(kformat!(&mut buf, "char: {}", 'Z'), b"char: Z");
        let c = '?';
        assert_eq!(kformat!(&mut buf, "char: {:c}", c), b"char: ?");
        assert_eq!(kformat!(&mut buf, "ref char: {:c}", &c), b"ref char: ?");

        // u8 and byte literals with {:c}
        let byte: u8 = b'A';
        assert_eq!(kformat!(&mut buf, "byte: {:c}", byte), b"byte: A");
        assert_eq!(kformat!(&mut buf, "ref byte: {:c}", &byte), b"ref byte: A");
        assert_eq!(kformat!(&mut buf, "byte lit: {:c}", b'K'), b"byte lit: K");
        assert_eq!(kformat!(&mut buf, "byte lit auto: {}", b'X'), b"byte lit auto: X");

        // Non-ASCII and multi-byte Unicode characters
        assert_eq!(kformat!(&mut buf, "crab: {}", '🦀'), "crab: 🦀".as_bytes());
        assert_eq!(kformat!(&mut buf, "accent: {:c}", 'é'), "accent: é".as_bytes());
    }

    #[test]
    fn test_booleans() {
        let mut buf = [0u8; 256];
        assert_eq!(kformat!(&mut buf, "flag: {}", true), b"flag: true");
        assert_eq!(kformat!(&mut buf, "flag: {:b}", false), b"flag: false");
    }

    #[test]
    fn test_floats() {
        let mut buf = [0u8; 256];
        assert_eq!(kformat!(&mut buf, "pi: {:.2f}", core::f64::consts::PI), b"pi: 3.14");
        assert_eq!(kformat!(&mut buf, "float: {:.4f}", 1.23456f32), b"float: 1.2346");
        let f = 1000.0;
        let res_e = kformat!(&mut buf, "sci: {:.1e}", f);
        assert_eq!(res_e, b"sci: 1.0e+03");
    }

    #[test]
    fn test_buffer_truncation() {
        let mut small_buf = [0u8; 8];
        let res = kformat!(&mut small_buf, "1234567890");
        assert_eq!(res, b"1234567");
        assert_eq!(res.len(), 7);
    }

    #[test]
    fn test_macro_wrapper_hygiene() {
        macro_rules! wrapped_kformat {
            ($buf:expr, $($tt:tt)*) => {
                kformat!($buf, $($tt)*)
            };
        }
        let mut buf = [0u8; 64];
        let x = 123;
        assert_eq!(wrapped_kformat!(&mut buf, "val: {}", x), b"val: 123");
    }

    #[test]
    fn test_print_macros() {
        kprint!("test printf {}", 123);
        kprint!("100%");
        kprintln!(" line2");
        kprintln!("empty without args");
        kprintln!("{:#x}", 0);
        kprintln!("crab: {}", '🦀');
    }
}
