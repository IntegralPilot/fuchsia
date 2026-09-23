// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, anyhow};

/// Unwraps the result of a FIDL call that errors out with a zx::result into a
/// Result<T, E>.
pub fn flatten_zx_error<T>(
    fidl_result: Result<Result<T, zx::sys::zx_status_t>, fidl::Error>,
) -> Result<T, Error> {
    fidl_result?
        .map_err(|zx_status| anyhow!("Server response: {}", zx::Status::err_from_raw(zx_status)))
}

/// Helper for accepting boolean values as "off" / "on" strings.
pub fn on_off_to_bool(value: &str) -> Result<bool, String> {
    match value {
        "off" => Ok(false),
        "on" => Ok(true),
        _ => Err(String::from("Unrecognized value. Possible values are \"on\" and \"off\".")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use googletest::{expect_that, gtest, matchers};

    #[gtest]
    #[fuchsia::test]
    fn flatten_zx_error_with_success() {
        let fidl_result: std::result::Result<
            std::result::Result<i32, zx::sys::zx_status_t>,
            fidl::Error,
        > = Ok(Ok(42));
        let result = flatten_zx_error(fidl_result);
        expect_that!(result, matchers::ok(matchers::eq(&42)));
    }

    #[gtest]
    #[fuchsia::test]
    fn flatten_zx_error_with_zx_error() {
        let fidl_result: std::result::Result<
            std::result::Result<i32, zx::sys::zx_status_t>,
            fidl::Error,
        > = Ok(Err(zx::sys::ZX_ERR_NOT_SUPPORTED));
        let result: std::result::Result<i32, Error> = flatten_zx_error(fidl_result);

        expect_that!(
            result,
            matchers::err(matchers::displays_as(matchers::eq("Server response: NOT_SUPPORTED")))
        );
    }

    #[gtest]
    #[fuchsia::test]
    fn flatten_zx_error_with_fidl_error() {
        let fidl_error = fidl::Error::ClientChannelClosed {
            epitaph: fidl::Epitaph::PeerClosed,
            protocol_name: "TestService",
        };
        let fidl_result: std::result::Result<
            std::result::Result<i32, zx::sys::zx_status_t>,
            fidl::Error,
        > = Err(fidl_error.clone());
        let result: std::result::Result<i32, Error> = flatten_zx_error(fidl_result);

        expect_that!(
            result,
            matchers::err(matchers::displays_as(matchers::eq(format!("{}", fidl_error))))
        );
    }
}
