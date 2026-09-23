// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[doc(hidden)]
#[macro_export]
macro_rules! __assert_parses_exactly {
    ($buf:expr => $parser:path => $expected:expr $(,)?) => {{
        use std::fmt::Write;
        const TEST_SENTINEL: [u8; 4] = [0xDE, 0xAD, 0xBE, 0xEF];

        let mut with_sentinel = $buf.to_owned();
        with_sentinel.extend(TEST_SENTINEL);
        let (trailing, observed) = match $parser(&with_sentinel) {
            Ok(p) => p,
            Err(e) => {
                let parser = stringify!($parser);
                let mut message = format!("{parser} failed to parse: {e:?}. buffer contained [");
                for chunk in with_sentinel.chunks(8) {
                    message.push_str("\n    ");
                    for byte in chunk {
                        write!(&mut message, "{byte:#04X}, ").unwrap();
                    }
                }
                message.push_str("\n]");
                panic!("{message}");
            }
        };
        assert_eq!(observed, $expected);
        assert_eq!(
            trailing, TEST_SENTINEL,
            "parser returned the right value but the wrong trailing bytes",
        );
    }};
}

#[macro_export]
macro_rules! assert_parses_to_record {
    ($buf:expr, $expected:expr $(,)?) => {{
        let buf = $buf.to_owned();
        __assert_parses_exactly!(buf => $crate::RawTraceRecord::parse =>
            $crate::ParsedWithOriginalBytes { parsed: $expected, bytes: &buf[..] });
    }};
}

#[macro_export]
macro_rules! assert_parses_to_arg {
    ($buf:expr, $expected:expr $(,)?) => {{
        __assert_parses_exactly!($buf => crate::args::RawArg::parse => $expected);
    }}
}
