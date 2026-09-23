// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::ffi::CStr;
use tpm2_tss_sys as tss_sys;

/// The string returned if we are unable to decode the `TSS2_RC`.
const DEFAULT_RC_ERROR: &'static str = "Unable to decode TSS return code.";

/// Converts a `TSS2_RC` to a `&str` representation.
#[allow(dead_code)]
pub fn decode_return_code(return_code: tss_sys::TSS2_RC) -> &'static str {
    let decoded_str = unsafe { tss_sys::Tss2_RC_Decode(return_code) };
    if decoded_str.is_null() {
        return DEFAULT_RC_ERROR;
    }
    let c_str = unsafe { CStr::from_ptr(decoded_str) };
    if let Ok(final_str) = c_str.to_str() {
        final_str
    } else {
        DEFAULT_RC_ERROR
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decode_return_code_with_success_return_code() {
        assert_eq!(decode_return_code(0), "tpm:success");
    }
}
