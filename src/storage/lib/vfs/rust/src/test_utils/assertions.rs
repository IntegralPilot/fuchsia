// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Assertions for unit tests for some very common operations that would otherwise be mildly
//! painful to read and write.
//!
//! Any macros in this file should shy away from trying to do too much, as it obscures the behavior
//! being tested. If it's too complicated to write in a test, it's too complicated to write in real
//! code, and should be handled by a support library.

#[doc(hidden)]
pub mod reexport {
    pub use flex_fuchsia_io as fio;
    pub use zx_status::Status;
}

#[macro_export]
macro_rules! assert_read {
    ($proxy:expr, $expected:expr) => {{
        use $crate::test_utils::assertions::reexport::Status;

        let content = $proxy
            .read($expected.len() as u64)
            .await
            .expect("read failed")
            .map_err(Status::err_from_raw)
            .expect("read error");

        assert_eq!(content.as_slice(), $expected.as_bytes());
    }};
}

#[macro_export]
macro_rules! assert_read_at {
    ($proxy:expr, $offset:expr, $expected:expr) => {{
        use $crate::test_utils::assertions::reexport::Status;

        let content = $proxy
            .read_at($expected.len() as u64, $offset)
            .await
            .expect("read failed")
            .map_err(Status::err_from_raw)
            .expect("read error");

        assert_eq!(content.as_slice(), $expected.as_bytes());
    }};
}

#[macro_export]
macro_rules! assert_write {
    ($proxy:expr, $content:expr) => {{
        use $crate::test_utils::assertions::reexport::Status;

        let len_written = $proxy
            .write($content.as_bytes())
            .await
            .expect("write failed")
            .map_err(Status::err_from_raw)
            .expect("write error");

        assert_eq!(len_written, $content.len() as u64);
    }};
}

#[macro_export]
macro_rules! assert_write_err {
    ($proxy:expr, $content:expr, $expected_status:expr) => {{
        use $crate::test_utils::assertions::reexport::Status;

        let result = $proxy
            .write($content.as_bytes())
            .await
            .expect("write failed")
            .map_err(Status::err_from_raw);

        assert_eq!(result, Err($expected_status));
    }};
}

#[macro_export]
macro_rules! assert_seek {
    ($proxy:expr, $pos:expr, $start:ident, $expected:expr) => {{
        use $crate::test_utils::assertions::reexport::{Status, fio};

        let actual = $proxy
            .seek(fio::SeekOrigin::$start, $pos)
            .await
            .expect("seek failed")
            .map_err(Status::err_from_raw);

        assert_eq!(actual, $expected);
    }};
    ($proxy:expr, $pos:expr, Start) => {
        assert_seek!($proxy, $pos, Start, Ok($pos as u64))
    };
}

#[macro_export]
macro_rules! assert_truncate {
    ($proxy:expr, $length:expr) => {{
        use $crate::test_utils::assertions::reexport::Status;

        let () = $proxy
            .resize($length)
            .await
            .expect("resize failed")
            .map_err(Status::err_from_raw)
            .expect("resize error");
    }};
}

#[macro_export]
macro_rules! assert_truncate_err {
    ($proxy:expr, $length:expr, $expected_status:expr) => {{
        use $crate::test_utils::assertions::reexport::Status;

        let result =
            $proxy.resize($length).await.expect("resize failed").map_err(Status::err_from_raw);

        assert_eq!(result, Err($expected_status));
    }};
}

#[macro_export]
macro_rules! assert_get_attr {
    ($proxy:expr, $expected:expr) => {{
        let (status, attrs) = $proxy.deprecated_get_attr().await.expect("get_attr failed");

        assert_eq!(status, zx_status::sys::ZX_OK);
        assert_eq!(attrs, $expected);
    }};
}

#[macro_export]
macro_rules! assert_query {
    ($proxy:expr, $expected:expr) => {
        let protocol = $proxy.query().await.expect("describe failed");
        assert_eq!(protocol, $expected.as_bytes());
    };
}

#[macro_export]
macro_rules! assert_close {
    ($proxy:expr) => {{
        use $crate::test_utils::assertions::reexport::Status;

        let () = $proxy
            .close()
            .await
            .expect("close failed")
            .map_err(Status::err_from_raw)
            .expect("close error");
    }};
}

#[macro_export]
macro_rules! assert_read_dirents {
    ($proxy:expr, $max_bytes:expr, $expected:expr) => {{
        let expected = $expected as Vec<u8>;

        let (status, entries) = $proxy.read_dirents($max_bytes).await.expect("read_dirents failed");

        assert_eq!(status, zx_status::sys::ZX_OK);
        assert!(
            &entries[..] == &expected[..],
            "Read entries do not match the expectation.\n\
             Expected entries: {:?}\n\
             Actual entries:   {:?}\n\
             Expected as UTF-8 lossy: {:?}\n\
             Received as UTF-8 lossy: {:?}",
            expected,
            &entries[..],
            String::from_utf8_lossy(&expected),
            String::from_utf8_lossy(&entries),
        );
    }};
}

#[macro_export]
macro_rules! assert_get_attributes {
    ($proxy:expr, $requested_attributes:expr, $expected:expr) => {{
        use $crate::test_utils::assertions::reexport::Status;

        let (mutable_attributes, immutable_attributes) = $proxy
            .get_attributes($requested_attributes)
            .await
            .expect("get_attributes failed")
            .map_err(Status::err_from_raw)
            .expect("get_attributes error");

        assert_eq!(mutable_attributes, $expected.mutable_attributes);
        assert_eq!(immutable_attributes, $expected.immutable_attributes);
    }};
}
