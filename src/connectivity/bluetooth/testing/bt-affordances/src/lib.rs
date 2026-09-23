// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::anyhow;
use bitfield::bitfield;
use fidl_fuchsia_bluetooth;
use fidl_fuchsia_bluetooth_gatt2::{
    AttributePermissions, Characteristic, CharacteristicPropertyBits, Handle, SecurityRequirements,
    ServiceHandle,
};
use fuchsia_bt_test_affordances::WorkThread;
use futures::executor::block_on;
use std::ffi::CStr;
use std::str::FromStr;
use std::sync::LazyLock;

struct State {
    worker: WorkThread,
}

impl State {
    const fn init() -> LazyLock<Self> {
        LazyLock::new(|| Self { worker: WorkThread::spawn() })
    }
}

static STATE: LazyLock<State> = State::init();

/// Stop serving Rust affordances.
///
/// Returns ZX_STATUS_INTERNAL if Rust affordances exited with an error (check logs).
#[unsafe(no_mangle)]
pub extern "C" fn stop_rust_affordances() -> i32 {
    println!("Stopping Rust affordances");
    if let Err(err) = STATE.worker.join() {
        eprintln!("stop_rust_affordances encountered error: {err}");
        return zx::Status::INTERNAL.into_raw();
    }
    zx::sys::ZX_OK
}

#[repr(C)]
pub struct UuidBytes {
    pub value: [u8; 16],
}

/// Parse a UUID from a string.
///
/// Returns a zeroed `UuidBytes` on error.
///
/// # Safety
///
/// The caller must ensure that `uuid_str` points to a valid C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uuid_from_string(uuid_str: *const core::ffi::c_char) -> UuidBytes {
    let uuid_str = unsafe { CStr::from_ptr(uuid_str) };
    let Ok(uuid_str) = uuid_str.to_str() else {
        return UuidBytes { value: [0; 16] };
    };
    match fuchsia_bluetooth::types::Uuid::from_str(uuid_str) {
        Ok(uuid) => {
            let fidl_uuid: fidl_fuchsia_bluetooth::Uuid = uuid.into();
            UuidBytes { value: fidl_uuid.value }
        }
        Err(_) => UuidBytes { value: [0; 16] },
    }
}

/// Convert a UUID to a string.
///
/// Returns ZX_STATUS_INTERNAL on error.
///
/// # Safety
///
/// The caller must ensure that `out_str` points to a valid buffer of at least 37 bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uuid_to_string(uuid: UuidBytes, out_str: *mut core::ffi::c_char) -> i32 {
    let fidl_uuid = fidl_fuchsia_bluetooth::Uuid { value: uuid.value };
    let uuid = fuchsia_bluetooth::types::Uuid::from(fidl_uuid);
    let s = uuid.to_string().to_uppercase();
    let Ok(c_str) = std::ffi::CString::new(s) else {
        return zx::Status::INTERNAL.into_raw();
    };
    let bytes = c_str.as_bytes_with_nul();
    if bytes.len() > 37 {
        eprintln!("UUID string length {} exceeds expected bound of 37", bytes.len());
        return zx::Status::INTERNAL.into_raw();
    }
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), out_str as *mut u8, bytes.len());
    }
    zx::sys::ZX_OK
}

// Copied from SL4F `GattServerFacade`.
fn permissions_from_raw(properties: u16, permissions: u16) -> AttributePermissions {
    bitfield! {
        #[derive(Clone, Copy)]
        struct Permissions(u16);
        // Bit 0 is unused
        pub read_encrypted, _: 1;
        pub read_encrypted_mitm, _: 2;
        // Bits 3, 4 unused
        pub write_encrypted, _: 5;
        pub write_encrypted_mitm, _: 6;
        pub write_signed, _: 7;
        pub write_signed_mitm, _: 8;
    }

    bitfield! {
        #[derive(Clone, Copy)]
        struct Properties(u16);
        // Bit 0 unused
        pub read, _: 1;
        // Bit 2 unused
        pub write, _: 3;
        pub notify, _: 4;
        pub indicate, _: 5;
    }

    let properties = Properties(properties);
    let permissions = Permissions(permissions);

    let read_encryption_required =
        permissions.read_encrypted() || permissions.read_encrypted_mitm();
    let read_authentication_required = permissions.read_encrypted();
    let read_authorization_required = permissions.read_encrypted();

    let write_encryption_required = permissions.write_encrypted()
        || permissions.write_encrypted_mitm()
        || permissions.write_signed_mitm();
    let write_authentication_required = permissions.write_signed_mitm();
    let write_authorization_required =
        permissions.write_signed() || permissions.write_signed_mitm();

    let update_encryption_required = permissions.read_encrypted_mitm()
        || permissions.write_encrypted()
        || permissions.write_encrypted_mitm()
        || permissions.write_signed_mitm();
    let update_authentication_required =
        permissions.write_encrypted_mitm() || permissions.write_signed_mitm();
    let update_authorization_required =
        permissions.write_encrypted_mitm() || permissions.write_signed_mitm();

    // Update Security Requirements only required if notify or indicate properties set.
    let update_sec_requirement =
        (properties.notify() || properties.indicate()).then_some(SecurityRequirements {
            encryption_required: Some(update_encryption_required),
            authentication_required: Some(update_authentication_required),
            authorization_required: Some(update_authorization_required),
            ..Default::default()
        });

    let read_sec_requirement = properties.read().then_some(SecurityRequirements {
        encryption_required: Some(read_encryption_required),
        authentication_required: Some(read_authentication_required),
        authorization_required: Some(read_authorization_required),
        ..Default::default()
    });

    let write_sec_requirement = properties.write().then_some(SecurityRequirements {
        encryption_required: Some(write_encryption_required),
        authentication_required: Some(write_authentication_required),
        authorization_required: Some(write_authorization_required),
        ..Default::default()
    });

    AttributePermissions {
        read: read_sec_requirement,
        write: write_sec_requirement,
        update: update_sec_requirement,
        ..Default::default()
    }
}

// TODO(https://fxbug.dev/396500079): Support more features as necessary to pass PTS-bot tests.
//
/// Publish a local GATT service with one characteristic. GATT requests to the service are logged.
///
/// Returns ZX_STATUS_INVALID_ARGS if UUID or `characteristic_properties` are invalid (check logs).
/// Returns ZX_STATUS_INTERNAL on error in bt-affordances (check logs).
///
/// # Safety
///
/// The caller must ensure that UUIDs are validly encoded as C strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn publish_service(
    handle: u64,
    uuid: *const core::ffi::c_char,
    characteristic_handle: u64,
    characteristic_uuid: *const core::ffi::c_char,
    characteristic_properties: u16,
    characteristic_permissions: u16,
) -> i32 {
    let parse_uuid =
        |uuid: *const core::ffi::c_char| -> Result<fidl_fuchsia_bluetooth::Uuid, anyhow::Error> {
            // Caller is responsible for providing a valid pointer to a C string. Encoding errors
            // are logged safely.
            fuchsia_bluetooth::types::Uuid::from_str(unsafe {
                CStr::from_ptr(uuid).to_str().map_err(|utf8_err| anyhow!("utf8 error: {utf8_err}"))
            }?)
            .map(|uuid| uuid.into())
            .map_err(|bt_error| anyhow!("{bt_error}"))
        };
    let uuid = match parse_uuid(uuid) {
        Ok(uuid) => uuid,
        Err(err) => {
            eprintln!("Error parsing UUID: {err}");
            return zx::Status::INVALID_ARGS.into_raw();
        }
    };
    let characteristic_uuid = match parse_uuid(characteristic_uuid) {
        Ok(uuid) => uuid,
        Err(err) => {
            eprintln!("Error parsing characteristic UUID: {err}");
            return zx::Status::INVALID_ARGS.into_raw();
        }
    };

    let properties = match CharacteristicPropertyBits::from_bits(characteristic_properties) {
        Some(properties) => properties,
        None => {
            eprintln!("Error parsing characteristic properties");
            return zx::Status::INVALID_ARGS.into_raw();
        }
    };

    let characteristic = Characteristic {
        handle: Some(Handle { value: characteristic_handle }),
        type_: Some(characteristic_uuid),
        properties: Some(properties),
        permissions: Some(permissions_from_raw(
            characteristic_properties,
            characteristic_permissions,
        )),
        ..Default::default()
    };

    if let Err(err) = block_on(STATE.worker.publish_service(
        uuid,
        ServiceHandle { value: handle },
        vec![characteristic],
    )) {
        eprintln!("publish_service encountered error: {err}");
        return zx::Status::INTERNAL.into_raw();
    }

    zx::sys::ZX_OK
}

#[repr(C)]
pub struct ReadCharacteristicResult {
    pub handle: u64,
    pub value: [u8; 512],
    pub value_len: usize,
    pub maybe_truncated: bool,
}

/// Read the value of a GATT characteristic on the remote peer identified with the given handles.
///
/// Returns ZX_STATUS_INTERNAL on error (check logs).
///
/// # Safety
///
/// The caller must ensure that `result` points to a valid `ReadCharacteristicResult` struct.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn read_characteristic(
    service_handle: u64,
    characteristic_handle: u64,
    result: *mut ReadCharacteristicResult,
) -> i32 {
    let service_handle = ServiceHandle { value: service_handle };
    let characteristic_handle = Handle { value: characteristic_handle };

    match block_on(STATE.worker.read_characteristic(service_handle, characteristic_handle)) {
        Ok(read_value) => unsafe {
            (*result).handle = read_value.handle.unwrap().value;
            let value = read_value.value.unwrap();
            (*result).value_len = value.len();
            (&mut (*result).value)[..value.len()].copy_from_slice(&value);
            (*result).maybe_truncated = read_value.maybe_truncated.unwrap();
        },
        Err(err) => {
            eprintln!("read_characteristic encountered error: {err}");
            return zx::Status::INTERNAL.into_raw();
        }
    }
    zx::sys::ZX_OK
}

/// Enable notifications/indications on the GATT characteristic with the given handles.
///
/// Returns ZX_STATUS_INTERNAL on error (check logs).
#[unsafe(no_mangle)]
pub extern "C" fn register_characteristic_notifier(
    service_handle: u64,
    characteristic_handle: u64,
) -> i32 {
    let service_handle = ServiceHandle { value: service_handle };
    let characteristic_handle = Handle { value: characteristic_handle };

    if let Err(err) = block_on(
        STATE.worker.register_characteristic_notifier(service_handle, characteristic_handle),
    ) {
        eprintln!("register_characteristic_notifier encountered error: {err}");
        return zx::Status::INTERNAL.into_raw();
    }
    zx::sys::ZX_OK
}
