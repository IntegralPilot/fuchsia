// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use super::handle::HandleValue;
use super::resource::{StrictValidation, validate_ranged_resource_with_strict};
use zx_status::Status;
use zx_types::{zx_handle_t, zx_rsrc_kind_t, zx_status_t};

/// Validates a resource handle against a requested range.
///
/// # Safety
///
/// This function is safe to call from C with any arguments; it does not dereference raw pointers
/// and validates all inputs through the handle table and resource validation logic.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rust_resource_validate_ranged_resource(
    handle: zx_handle_t,
    kind: zx_rsrc_kind_t,
    base: u64,
    size: usize,
    strict: bool,
) -> zx_status_t {
    let strict_validation = if strict { StrictValidation::Yes } else { StrictValidation::No };
    Status::result_into_raw(validate_ranged_resource_with_strict(
        HandleValue::new(handle),
        kind,
        base,
        size,
        strict_validation,
    ))
}
