// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_BLUETOOTH_TESTING_BT_AFFORDANCES_FFI_C_BINDINGS_H_
#define SRC_CONNECTIVITY_BLUETOOTH_TESTING_BT_AFFORDANCES_FFI_C_BINDINGS_H_

#include <cstdarg>
#include <cstdint>
#include <cstdlib>
#include <new>
#include <ostream>

struct UuidBytes {
  uint8_t value[16];
};

struct ReadCharacteristicResult {
  uint64_t handle;
  uint8_t value[512];
  uintptr_t value_len;
  bool maybe_truncated;
};

extern "C" {

/// Stop serving Rust affordances.
///
/// Returns ZX_STATUS_INTERNAL if Rust affordances exited with an error (check logs).
int32_t stop_rust_affordances();

/// Parse a UUID from a string.
///
/// Returns a zeroed `UuidBytes` on error.
///
/// # Safety
///
/// The caller must ensure that `uuid_str` points to a valid C string.
UuidBytes uuid_from_string(const char *uuid_str);

/// Convert a UUID to a string.
///
/// Returns ZX_STATUS_INTERNAL on error.
///
/// # Safety
///
/// The caller must ensure that `out_str` points to a valid buffer of at least 37 bytes.
int32_t uuid_to_string(UuidBytes uuid, char *out_str);

/// Publish a local GATT service with one characteristic. GATT requests to the service are logged.
///
/// Returns ZX_STATUS_INVALID_ARGS if UUID or `characteristic_properties` are invalid (check logs).
/// Returns ZX_STATUS_INTERNAL on error in bt-affordances (check logs).
///
/// # Safety
///
/// The caller must ensure that UUIDs are validly encoded as C strings.
int32_t publish_service(uint64_t handle, const char *uuid, uint64_t characteristic_handle,
                        const char *characteristic_uuid, uint16_t characteristic_properties,
                        uint16_t characteristic_permissions);

/// Read the value of a GATT characteristic on the remote peer identified with the given handles.
///
/// Returns ZX_STATUS_INTERNAL on error (check logs).
///
/// # Safety
///
/// The caller must ensure that `result` points to a valid `ReadCharacteristicResult` struct.
int32_t read_characteristic(uint64_t service_handle, uint64_t characteristic_handle,
                            ReadCharacteristicResult *result);

/// Enable notifications/indications on the GATT characteristic with the given handles.
///
/// Returns ZX_STATUS_INTERNAL on error (check logs).
int32_t register_characteristic_notifier(uint64_t service_handle, uint64_t characteristic_handle);

}  // extern "C"

#endif  // SRC_CONNECTIVITY_BLUETOOTH_TESTING_BT_AFFORDANCES_FFI_C_BINDINGS_H_
