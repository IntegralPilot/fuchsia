// Copyright 2024 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Implementation of TEE Internal Core API Specification 4.4 Property Access Functions
// Functions return specific error codes where applicable and panic on all other errors.
use std::cmp::min;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use tee_internal::{
    Error, Identity, PropSetHandle, Result as TeeResult, TEE_PROPSET_CURRENT_CLIENT,
    TEE_PROPSET_CURRENT_TA, TEE_PROPSET_TEE_IMPLEMENTATION, Uuid,
};
use tee_properties::{PropEnumerator, PropSet, PropertyError};

fn write_c_string_to_buffer<'a>(string_val: &str, buffer: &'a mut [u8]) -> &'a str {
    if buffer.len() == 0 {
        return std::str::from_utf8(buffer).unwrap();
    }
    // buffer.len() - 1 accounts for the NUL byte we need to write at the end.
    let bytes_to_write = min(buffer.len() - 1, string_val.len());
    buffer[..bytes_to_write].clone_from_slice(&string_val.as_bytes()[..bytes_to_write]);
    buffer[bytes_to_write] = 0;

    std::str::from_utf8(&buffer[..bytes_to_write]).unwrap()
}

// The public surface of this module calls into a PropSets singleton.

// A `PropSetHandle` input into the API can be either:
// 1. A handle to an enumerator on a property set, OR
// 2. A pseudo-handle which designates a specific property set: TEE Implementation, Client, or TA.
// This helper fn aims to easily distinguish between the two for easier processing.
pub fn is_propset_pseudo_handle(handle: PropSetHandle) -> bool {
    handle == TEE_PROPSET_TEE_IMPLEMENTATION
        || handle == TEE_PROPSET_CURRENT_CLIENT
        || handle == TEE_PROPSET_CURRENT_TA
}

pub struct GetStringError<'a> {
    pub error: Error,
    pub written: &'a str,
    pub actual_length: usize,
}

pub struct GetBinaryBlockError<'a> {
    pub error: Error,
    pub written: &'a [u8],
    pub actual_length: usize,
}

pub struct Properties {
    prop_sets: PropSets,
}

impl Properties {
    pub fn new() -> Self {
        Self { prop_sets: PropSets::load_prop_sets() }
    }

    pub fn get_property_as_string<'a>(
        &self,
        handle: PropSetHandle,
        name: &str,
        buffer: &'a mut [u8],
    ) -> Result<&'a str, GetStringError<'a>> {
        let string_val = self
            .prop_sets
            .get_string_property(handle, name)
            .map_err(|error| GetStringError { error, written: "", actual_length: 0 })?;
        let buffer_len = buffer.len();
        let string_written = write_c_string_to_buffer(string_val.as_str(), buffer);
        // We're writing a NUL byte to the buffer which is not captured in string_val.len().
        if buffer_len < string_val.len() + 1 {
            return Err(GetStringError {
                error: Error::ShortBuffer,
                written: string_written,
                actual_length: string_val.len(),
            });
        }
        Ok(string_written)
    }

    pub fn get_property_as_bool(&self, handle: PropSetHandle, name: &str) -> TeeResult<bool> {
        self.prop_sets.get_bool_property(handle, name)
    }

    pub fn get_property_as_u32(&self, handle: PropSetHandle, name: &str) -> TeeResult<u32> {
        self.prop_sets.get_uint32_property(handle, name)
    }

    pub fn get_property_as_u64(&self, handle: PropSetHandle, name: &str) -> TeeResult<u64> {
        self.prop_sets.get_uint64_property(handle, name)
    }

    pub fn get_property_as_binary_block<'a>(
        &self,
        handle: PropSetHandle,
        name: &str,
        buffer: &'a mut [u8],
    ) -> Result<&'a [u8], GetBinaryBlockError<'a>> {
        let bytes = self
            .prop_sets
            .get_binary_block_property(handle, name)
            .map_err(|error| GetBinaryBlockError { error, written: &[], actual_length: 0 })?;

        let buffer_len = buffer.len();
        let bytes_to_write = min(buffer.len(), bytes.len());
        let written = &mut buffer[..bytes_to_write];
        written.clone_from_slice(&bytes[..bytes_to_write]);

        if buffer_len < bytes.len() {
            return Err(GetBinaryBlockError {
                error: Error::ShortBuffer,
                written,
                actual_length: bytes.len(),
            });
        }
        Ok(written)
    }

    pub fn get_property_as_uuid(&self, handle: PropSetHandle, name: &str) -> TeeResult<Uuid> {
        self.prop_sets.get_uuid_property(handle, name)
    }

    pub fn get_property_as_identity(
        &self,
        handle: PropSetHandle,
        name: &str,
    ) -> TeeResult<Identity> {
        self.prop_sets.get_identity_property(handle, name)
    }

    pub fn allocate_property_enumerator(&mut self) -> PropSetHandle {
        self.prop_sets.allocate_property_enumerator()
    }

    pub fn free_property_enumerator(&mut self, handle: PropSetHandle) {
        self.prop_sets.free_property_enumerator(handle);
    }

    pub fn start_property_enumerator(&mut self, handle: PropSetHandle, prop_set: PropSetHandle) {
        self.prop_sets.start_property_enumerator(handle, prop_set);
    }

    pub fn reset_property_enumerator(&mut self, handle: PropSetHandle) {
        self.prop_sets.reset_property_enumerator(handle);
    }

    pub fn get_property_name<'a>(
        &self,
        handle: PropSetHandle,
        buffer: &'a mut [u8],
    ) -> Result<&'a str, GetStringError<'a>> {
        let string_val = self
            .prop_sets
            .get_property_name(handle)
            .map_err(|error| GetStringError { error, written: "", actual_length: 0 })?;

        let buffer_len = buffer.len();
        let string_written = write_c_string_to_buffer(string_val.as_str(), buffer);
        // We're writing a NUL byte to the buffer which is not captured in string_val.len().
        if buffer_len < string_val.len() + 1 {
            return Err(GetStringError {
                error: Error::ShortBuffer,
                written: string_written,
                actual_length: string_val.len(),
            });
        }
        Ok(string_written)
    }

    pub fn get_next_property(&mut self, handle: PropSetHandle) -> TeeResult<()> {
        self.prop_sets.get_next_property(handle)
    }
}

struct PropSets {
    tee_implementation_props: Arc<PropSet>,
    ta_props: Arc<PropSet>,
    client_props: Arc<PropSet>,
    // TODO: Scope write locking to this hashmap only.
    prop_enums: HashMap<u64, PropEnumerator>,
    enumerator_handle_id_counter: AtomicU64,
}

impl PropSets {
    fn load_prop_sets() -> Self {
        let tee_impl_props_path = Path::new("/properties/system_properties");
        let tee_impl_props = PropSet::from_config_file(&tee_impl_props_path)
            .expect("TEE prop set loads successfully");

        let ta_props_path = Path::new("/pkg/data/ta_properties");
        let ta_props =
            PropSet::from_config_file(&ta_props_path).expect("TA prop set loads successfully");

        // TODO(b/369916290): Handle this properly.
        let fake_client_props_str: &str = r#"
        [
            {
                name: "gpd.client.identity",
                prop_type: "identity",
                value: "0:9cccff19-13b5-4d4c-aa9e-5c8901a52e2f",
            },
        ]
        "#;
        let client_props = PropSet::from_config_string(fake_client_props_str)
            .expect("load fake client props successfully");

        Self {
            tee_implementation_props: Arc::new(tee_impl_props),
            ta_props: Arc::new(ta_props),
            client_props: Arc::new(client_props),
            prop_enums: HashMap::new(),
            // Handles cannot be 0, as that value is reserved for TEE_HANDLE_NULL, so we start at 1.
            // There are reserved values at 0xFFFFFFFD-0xFFFFFFFF; not expecting allocations to reach them.
            enumerator_handle_id_counter: AtomicU64::new(1),
        }
    }

    fn get_prop_set_from_pseudo_handle(
        &self,
        maybe_pseudo_handle: PropSetHandle,
    ) -> Option<Arc<PropSet>> {
        match maybe_pseudo_handle {
            TEE_PROPSET_TEE_IMPLEMENTATION => Some(self.tee_implementation_props.clone()),
            TEE_PROPSET_CURRENT_CLIENT => Some(self.client_props.clone()),
            TEE_PROPSET_CURRENT_TA => Some(self.ta_props.clone()),
            _ => None,
        }
    }

    // TODO(https://fxbug.dev/376130726): Use &str as return value instead of String.
    fn get_string_property(
        &self,
        handle: PropSetHandle,
        name: &str,
    ) -> Result<String, tee_internal::Error> {
        assert!(!handle.is_null());
        let string_val = if let Some(prop_set) = self.get_prop_set_from_pseudo_handle(handle) {
            prop_set
                .get_string_property(name.to_string())
                .map_err(|_| tee_internal::Error::ItemNotFound)?
        } else {
            self.prop_enums
                .get(&handle)
                .expect("enumerator handle must be valid")
                .get_property_as_string()
                .map_err(|_| tee_internal::Error::ItemNotFound)?
        };
        Ok(string_val)
    }

    fn get_bool_property(
        &self,
        handle: PropSetHandle,
        name: &str,
    ) -> Result<bool, tee_internal::Error> {
        assert!(!handle.is_null());
        let bool_val = if let Some(prop_set) = self.get_prop_set_from_pseudo_handle(handle) {
            prop_set
                .get_boolean_property(name.to_string())
                .map_err(|_| tee_internal::Error::ItemNotFound)?
        } else {
            self.prop_enums
                .get(&handle)
                .expect("enumerator handle must be valid")
                .get_property_as_bool()
                .map_err(|_| tee_internal::Error::ItemNotFound)?
        };
        Ok(bool_val)
    }

    fn get_uint32_property(
        &self,
        handle: PropSetHandle,
        name: &str,
    ) -> Result<u32, tee_internal::Error> {
        assert!(!handle.is_null());
        let u32_val = if let Some(prop_set) = self.get_prop_set_from_pseudo_handle(handle) {
            prop_set
                .get_uint32_property(name.to_string())
                .map_err(|_| tee_internal::Error::ItemNotFound)?
        } else {
            self.prop_enums
                .get(&handle)
                .expect("enumerator handle must be valid")
                .get_property_as_u32()
                .map_err(|_| tee_internal::Error::ItemNotFound)?
        };
        Ok(u32_val)
    }

    fn get_uint64_property(
        &self,
        handle: PropSetHandle,
        name: &str,
    ) -> Result<u64, tee_internal::Error> {
        assert!(!handle.is_null());
        let u64_val = if let Some(prop_set) = self.get_prop_set_from_pseudo_handle(handle) {
            prop_set
                .get_uint64_property(name.to_string())
                .map_err(|_| tee_internal::Error::ItemNotFound)?
        } else {
            self.prop_enums
                .get(&handle)
                .expect("enumerator handle must be valid")
                .get_property_as_u64()
                .map_err(|_| tee_internal::Error::ItemNotFound)?
        };
        Ok(u64_val)
    }

    // TODO(https://fxbug.dev/376130726): Use &[u8] as return value instead of Vec<u8>.
    fn get_binary_block_property(
        &self,
        handle: PropSetHandle,
        name: &str,
    ) -> Result<Vec<u8>, tee_internal::Error> {
        assert!(!handle.is_null());
        let bytes = if let Some(prop_set) = self.get_prop_set_from_pseudo_handle(handle) {
            prop_set.get_binary_block_property(name.to_string()).map_err(
                |error: PropertyError| match error {
                    PropertyError::BadFormat { .. } => tee_internal::Error::BadFormat,
                    PropertyError::ItemNotFound { .. } => return tee_internal::Error::ItemNotFound,
                    PropertyError::Generic { msg } => {
                        panic!("Error getting property as binary block: {:?}", msg)
                    }
                },
            )?
        } else {
            self.prop_enums
                .get(&handle)
                .expect("enumerator handle must be valid")
                .get_property_as_binary_block()
                .map_err(|error: PropertyError| match error {
                    PropertyError::BadFormat { .. } => tee_internal::Error::BadFormat,
                    PropertyError::ItemNotFound { .. } => return tee_internal::Error::ItemNotFound,
                    PropertyError::Generic { msg } => {
                        panic!("Error getting property as binary block: {:?}", msg)
                    }
                })?
        };

        Ok(bytes)
    }

    fn get_uuid_property(
        &self,
        handle: PropSetHandle,
        name: &str,
    ) -> Result<Uuid, tee_internal::Error> {
        assert!(!handle.is_null());
        let uuid = if let Some(prop_set) = self.get_prop_set_from_pseudo_handle(handle) {
            prop_set.get_uuid_property(name.to_string()).map_err(|error| match error {
                PropertyError::BadFormat { .. } => tee_internal::Error::BadFormat,
                PropertyError::ItemNotFound { .. } => return tee_internal::Error::ItemNotFound,
                PropertyError::Generic { msg } => {
                    panic!("Error getting property as uuid: {:?}", msg)
                }
            })?
        } else {
            self.prop_enums
                .get(&handle)
                .expect("enumerator handle must be valid")
                .get_property_as_uuid()
                .map_err(|error| match error {
                    PropertyError::BadFormat { .. } => tee_internal::Error::BadFormat,
                    PropertyError::ItemNotFound { .. } => return tee_internal::Error::ItemNotFound,
                    PropertyError::Generic { msg } => {
                        panic!("Error getting property as uuid: {:?}", msg)
                    }
                })?
        };

        Ok(uuid)
    }

    fn get_identity_property(
        &self,
        handle: PropSetHandle,
        name: &str,
    ) -> Result<Identity, tee_internal::Error> {
        assert!(!handle.is_null());
        let identity = if let Some(prop_set) = self.get_prop_set_from_pseudo_handle(handle) {
            prop_set.get_identity_property(name.to_string()).map_err(|error: PropertyError| {
                match error {
                    PropertyError::BadFormat { .. } => tee_internal::Error::BadFormat,
                    PropertyError::ItemNotFound { .. } => return tee_internal::Error::ItemNotFound,
                    PropertyError::Generic { msg } => {
                        panic!("Error getting property as identity: {:?}", msg)
                    }
                }
            })?
        } else {
            self.prop_enums
                .get(&handle)
                .expect("enumerator handle must be valid")
                .get_property_as_identity()
                .map_err(|error| match error {
                    PropertyError::BadFormat { .. } => tee_internal::Error::BadFormat,
                    PropertyError::ItemNotFound { .. } => return tee_internal::Error::ItemNotFound,
                    PropertyError::Generic { msg } => {
                        panic!("Error getting property as identity: {:?}", msg)
                    }
                })?
        };

        Ok(identity)
    }

    fn new_enumerator_handle(&mut self) -> u64 {
        assert!(
            self.enumerator_handle_id_counter.load(Ordering::Relaxed)
                <= *TEE_PROPSET_TEE_IMPLEMENTATION
        );
        self.enumerator_handle_id_counter.fetch_add(1, Ordering::Relaxed)
    }

    fn allocate_property_enumerator(&mut self) -> PropSetHandle {
        // TODO(b/369916290): Check for out-of-memory conditions to return TEE_ERROR_OUT_OF_MEMORY.
        let handle = self.new_enumerator_handle();
        let _ = self.prop_enums.insert(handle, PropEnumerator::new());
        PropSetHandle::from_value(handle)
    }

    fn free_property_enumerator(&mut self, handle: PropSetHandle) {
        assert!(!handle.is_null());
        let _ = self.prop_enums.remove_entry(&handle);
    }

    fn start_property_enumerator(&mut self, handle: PropSetHandle, prop_set: PropSetHandle) {
        assert!(!handle.is_null());
        let Some(prop_set) = self.get_prop_set_from_pseudo_handle(prop_set) else {
            panic!("Invalid TEE_PropSetHandle pseudo-handle provided");
        };

        let Some(prop_enumerator) = self.prop_enums.get_mut(&handle) else {
            panic!("Invalid enumerator handle provided");
        };

        prop_enumerator.start(prop_set);
    }

    fn reset_property_enumerator(&mut self, handle: PropSetHandle) {
        assert!(!handle.is_null());
        let Some(prop_enumerator) = self.prop_enums.get_mut(&handle) else {
            panic!("Invalid enumerator handle provided");
        };
        prop_enumerator.reset();
    }

    fn get_property_name(&self, handle: PropSetHandle) -> Result<String, tee_internal::Error> {
        assert!(!handle.is_null());
        let Some(prop_enumerator) = self.prop_enums.get(&handle) else {
            panic!("Invalid enumerator handle provided");
        };
        prop_enumerator.get_property_name().map_err(|_| tee_internal::Error::ItemNotFound)
    }

    fn get_next_property(&mut self, handle: PropSetHandle) -> Result<(), tee_internal::Error> {
        assert!(!handle.is_null());
        let Some(prop_enumerator) = self.prop_enums.get_mut(&handle) else {
            panic!("Invalid enumerator handle provided");
        };

        prop_enumerator.next().map_err(|_| tee_internal::Error::ItemNotFound)
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    // TODO: Re-use test values from //src/microfuchsia/tee/lib/tee_properties tests to avoid duplicating these.
    const TEST_TEE_IMPL_PROPS_STR: &str = r#"[
        {
            name: "gpd.tee.test.string",
            prop_type: "string",
            value: "asdf",
        },
        {
            name: "gpd.tee.test.bool",
            prop_type: "boolean",
            value: "true",
        },
        {
            name: "gpd.tee.test.u32",
            prop_type: "unsigned_int32",
            value: "57",
        },
        {
            name: "gpd.tee.test.u64",
            prop_type: "unsigned_int64",
            value: "4294967296",
        },
        {
            name: "gpd.tee.test.binaryBlock",
            prop_type: "binary_block",
            value: "ZnVjaHNpYQ==",
        },
        {
            name: "gpd.tee.test.uuid",
            prop_type: "uuid",
            value: "9cccff19-13b5-4d4c-aa9e-5c8901a52e2f",
        },
        {
            name: "gpd.tee.test.identity",
            prop_type: "identity",
            value: "4026531840:9cccff19-13b5-4d4c-aa9e-5c8901a52e2f",
        },
    ]
    "#;

    const TEST_TA_PROPS_STR: &str = r#"[
        {
            name: "gpd.ta.appID",
            prop_type: "uuid",
            value: "9cccff19-13b5-4d4c-aa9e-5c8901a52e2f",
        },
        {
            name: "gpd.ta.singleInstance",
            prop_type: "boolean",
            value: "true",
        },
        {
            name: "gpd.ta.multiSession",
            prop_type: "boolean",
            value: "true",
        },
        {
            name: "gpd.ta.instanceKeepAlive",
            prop_type: "boolean",
            value: "false",
        },
    ]
    "#;

    const TEST_CLIENT_PROPS_STR: &str = r#"
    [
        {
            name: "gpd.client.identity",
            prop_type: "identity",
            value: "0:9cccff19-13b5-4d4c-aa9e-5c8901a52e2f",
        },
    ]
    "#;

    fn init_test_tee_impl_props() -> PropSets {
        let test_props = PropSet::from_config_string(TEST_TEE_IMPL_PROPS_STR)
            .expect("load fake tee props successfully");
        let ta_props = PropSet::from_config_string(TEST_TA_PROPS_STR)
            .expect("load fake ta props successfully");
        let client_props = PropSet::from_config_string(TEST_CLIENT_PROPS_STR)
            .expect("load fake client props successfully");

        PropSets {
            tee_implementation_props: Arc::new(test_props),
            ta_props: Arc::new(ta_props),
            client_props: Arc::new(client_props),
            prop_enums: HashMap::new(),
            enumerator_handle_id_counter: AtomicU64::new(1),
        }
    }

    #[fuchsia::test]
    pub fn test_get_string_property() {
        let prop_sets = init_test_tee_impl_props();
        let value = prop_sets
            .get_string_property(TEE_PROPSET_TEE_IMPLEMENTATION, "gpd.tee.test.string")
            .unwrap();
        assert_eq!("asdf".to_string(), value);
    }

    #[fuchsia::test]
    pub fn test_get_string_property_not_found() {
        let prop_sets = init_test_tee_impl_props();
        let value =
            prop_sets.get_string_property(TEE_PROPSET_TEE_IMPLEMENTATION, "name_not_in_set");
        assert_eq!(tee_internal::Error::ItemNotFound, value.err().unwrap());
    }

    #[fuchsia::test]
    pub fn test_get_bool_property() {
        let prop_sets = init_test_tee_impl_props();
        let value = prop_sets
            .get_bool_property(TEE_PROPSET_TEE_IMPLEMENTATION, "gpd.tee.test.bool")
            .unwrap();
        assert!(value);
    }

    #[fuchsia::test]
    pub fn test_get_bool_property_not_found() {
        let prop_sets = init_test_tee_impl_props();
        let value = prop_sets.get_bool_property(TEE_PROPSET_TEE_IMPLEMENTATION, "name_not_in_set");
        assert_eq!(tee_internal::Error::ItemNotFound, value.err().unwrap());
    }

    #[fuchsia::test]
    pub fn test_get_u32_property() {
        let prop_sets = init_test_tee_impl_props();
        let value = prop_sets
            .get_uint32_property(TEE_PROPSET_TEE_IMPLEMENTATION, "gpd.tee.test.u32")
            .unwrap();
        assert_eq!(57, value);
    }

    #[fuchsia::test]
    pub fn test_get_u32_property_not_found() {
        let prop_sets = init_test_tee_impl_props();
        let value =
            prop_sets.get_uint32_property(TEE_PROPSET_TEE_IMPLEMENTATION, "name_not_in_set");
        assert_eq!(tee_internal::Error::ItemNotFound, value.err().unwrap());
    }

    #[fuchsia::test]
    pub fn test_get_u64_property() {
        let prop_sets = init_test_tee_impl_props();
        let value = prop_sets
            .get_uint64_property(TEE_PROPSET_TEE_IMPLEMENTATION, "gpd.tee.test.u64")
            .unwrap();
        assert_eq!(4294967296, value);
    }

    #[fuchsia::test]
    pub fn test_get_u64_property_not_found() {
        let prop_sets = init_test_tee_impl_props();
        let value =
            prop_sets.get_uint64_property(TEE_PROPSET_TEE_IMPLEMENTATION, "name_not_in_set");
        assert_eq!(tee_internal::Error::ItemNotFound, value.err().unwrap());
    }

    #[fuchsia::test]
    pub fn test_get_binary_block_property() {
        let prop_sets = init_test_tee_impl_props();
        let value = prop_sets
            .get_binary_block_property(TEE_PROPSET_TEE_IMPLEMENTATION, "gpd.tee.test.binaryBlock")
            .unwrap();

        // base64 string ZnVjaHNpYQ== resolves to hex [66 75 63 68 73 69 61], which is len 7.
        // This maps to [102, 117, 99, 104, 115, 105, 97] as u8 bytes.
        let expected_val_as_u8: Vec<u8> = vec![102, 117, 99, 104, 115, 105, 97];
        assert_eq!(7, value.len());
        assert_eq!(expected_val_as_u8, value);
    }

    #[fuchsia::test]
    pub fn test_get_binary_block_property_not_found() {
        let prop_sets = init_test_tee_impl_props();
        let value =
            prop_sets.get_binary_block_property(TEE_PROPSET_TEE_IMPLEMENTATION, "name_not_in_set");
        assert_eq!(tee_internal::Error::ItemNotFound, value.err().unwrap());
    }

    #[fuchsia::test]
    pub fn test_get_binary_block_property_bad_format() {
        let prop_sets = init_test_tee_impl_props();
        let value =
            prop_sets.get_binary_block_property(TEE_PROPSET_TEE_IMPLEMENTATION, "gpd.tee.test.u32");
        assert_eq!(tee_internal::Error::BadFormat, value.err().unwrap());
    }

    #[fuchsia::test]
    pub fn test_get_uuid_property() {
        let prop_sets = init_test_tee_impl_props();
        let value = prop_sets
            .get_uuid_property(TEE_PROPSET_TEE_IMPLEMENTATION, "gpd.tee.test.uuid")
            .unwrap();

        assert_eq!(0x9cccff19, value.time_low);
        assert_eq!(0x13b5, value.time_mid);
        assert_eq!(0x4d4c, value.time_hi_and_version);
        assert_eq!([0xaa, 0x9e, 0x5c, 0x89, 0x01, 0xa5, 0x2e, 0x2f], value.clock_seq_and_node);
    }

    #[fuchsia::test]
    pub fn test_get_uuid_property_not_found() {
        let prop_sets = init_test_tee_impl_props();
        let value = prop_sets.get_uuid_property(TEE_PROPSET_TEE_IMPLEMENTATION, "name_not_in_set");
        assert_eq!(tee_internal::Error::ItemNotFound, value.err().unwrap());
    }

    #[fuchsia::test]
    pub fn test_get_uuid_property_bad_format() {
        let prop_sets = init_test_tee_impl_props();
        let value = prop_sets.get_uuid_property(TEE_PROPSET_TEE_IMPLEMENTATION, "gpd.tee.test.u32");
        assert_eq!(tee_internal::Error::BadFormat, value.err().unwrap());
    }

    #[fuchsia::test]
    pub fn test_get_identity_property() {
        let prop_sets = init_test_tee_impl_props();
        let value = prop_sets
            .get_identity_property(TEE_PROPSET_TEE_IMPLEMENTATION, "gpd.tee.test.identity")
            .unwrap();

        // Login::TrustedApp = TEE_LOGIN_TRUSTED_APP = 4026531840, as encoded in TEST_TEE_IMPL_PROPS_STR.
        assert_eq!(tee_internal::Login::TrustedApp, value.login);

        assert_eq!(0x9cccff19, value.uuid.time_low);
        assert_eq!(0x13b5, value.uuid.time_mid);
        assert_eq!(0x4d4c, value.uuid.time_hi_and_version);
        assert_eq!([0xaa, 0x9e, 0x5c, 0x89, 0x01, 0xa5, 0x2e, 0x2f], value.uuid.clock_seq_and_node);
    }

    #[fuchsia::test]
    pub fn test_get_identity_property_not_found() {
        let prop_sets = init_test_tee_impl_props();
        let value =
            prop_sets.get_identity_property(TEE_PROPSET_TEE_IMPLEMENTATION, "name_not_in_set");
        assert_eq!(tee_internal::Error::ItemNotFound, value.err().unwrap());
    }

    #[fuchsia::test]
    pub fn test_get_identity_property_bad_format() {
        let prop_sets = init_test_tee_impl_props();
        let value =
            prop_sets.get_identity_property(TEE_PROPSET_TEE_IMPLEMENTATION, "gpd.tee.test.uuid");
        assert_eq!(tee_internal::Error::BadFormat, value.err().unwrap());
    }

    #[fuchsia::test]
    pub fn test_enumerator_allocate() {
        let mut prop_sets = init_test_tee_impl_props();
        assert_eq!(0, prop_sets.prop_enums.len());
        let handle = prop_sets.allocate_property_enumerator();
        assert!(prop_sets.prop_enums.get(&handle).is_some());
    }

    #[fuchsia::test]
    pub fn test_enumerator_free() {
        let mut prop_sets = init_test_tee_impl_props();
        assert_eq!(0, prop_sets.prop_enums.len());
        let handle = prop_sets.allocate_property_enumerator();
        assert!(prop_sets.prop_enums.get(&handle).is_some());
        prop_sets.free_property_enumerator(handle);
        assert_eq!(0, prop_sets.prop_enums.len());
    }

    #[fuchsia::test]
    pub fn test_enumerator_start() {
        let mut prop_sets = init_test_tee_impl_props();
        let handle = prop_sets.allocate_property_enumerator();
        // We can't inspect the internal state of the enumerator, so this tests not panicking.
        prop_sets.start_property_enumerator(handle, TEE_PROPSET_TEE_IMPLEMENTATION);
    }

    #[fuchsia::test]
    #[should_panic]
    pub fn test_enumerator_start_bad_handle() {
        let mut prop_sets = init_test_tee_impl_props();
        let _handle = prop_sets.allocate_property_enumerator();
        // Handle value of 0 corresponds to TEE_HANDLE_NULL, which is not valid for this API.
        let bad_handle = PropSetHandle::from_value(0);
        prop_sets.start_property_enumerator(bad_handle, TEE_PROPSET_TEE_IMPLEMENTATION);
    }

    #[fuchsia::test]
    #[should_panic(expected = "Invalid TEE_PropSetHandle pseudo-handle provided")]
    pub fn test_enumerator_start_bad_prop_set_pseudo_handle() {
        let mut prop_sets = init_test_tee_impl_props();
        let handle = prop_sets.allocate_property_enumerator();
        // Pseudo handle is not one of the 3 reserved values.
        prop_sets.start_property_enumerator(handle, PropSetHandle::from_value(0));
    }

    #[fuchsia::test]
    pub fn test_enumerator_get_name() {
        let mut prop_sets = init_test_tee_impl_props();
        let handle = prop_sets.allocate_property_enumerator();

        prop_sets.start_property_enumerator(handle, TEE_PROPSET_TEE_IMPLEMENTATION);
        let name = prop_sets.get_property_name(handle).unwrap();
        assert_eq!("gpd.tee.test.string".to_string(), name);
    }

    #[fuchsia::test]
    pub fn test_enumerator_next_property() {
        let mut prop_sets = init_test_tee_impl_props();
        let handle = prop_sets.allocate_property_enumerator();

        prop_sets.start_property_enumerator(handle, TEE_PROPSET_TEE_IMPLEMENTATION);
        let name = prop_sets.get_property_name(handle).unwrap();
        assert_eq!("gpd.tee.test.string".to_string(), name);

        prop_sets.get_next_property(handle).unwrap();
        let name = prop_sets.get_property_name(handle).unwrap();
        assert_eq!("gpd.tee.test.bool".to_string(), name);
    }

    #[fuchsia::test]
    pub fn test_enumerator_reset() {
        let mut prop_sets = init_test_tee_impl_props();
        let handle = prop_sets.allocate_property_enumerator();

        prop_sets.start_property_enumerator(handle, TEE_PROPSET_TEE_IMPLEMENTATION);
        let name = prop_sets.get_property_name(handle).unwrap();
        assert_eq!("gpd.tee.test.string".to_string(), name);

        prop_sets.get_next_property(handle).unwrap();
        let name = prop_sets.get_property_name(handle).unwrap();
        assert_eq!("gpd.tee.test.bool".to_string(), name);

        prop_sets.reset_property_enumerator(handle);
        let name = prop_sets.get_property_name(handle).unwrap();
        assert_eq!("gpd.tee.test.string".to_string(), name);
    }

    #[fuchsia::test]
    pub fn test_enumerator_get_name_ta_prop_set() {
        let mut prop_sets = init_test_tee_impl_props();
        let handle = prop_sets.allocate_property_enumerator();

        prop_sets.start_property_enumerator(handle, TEE_PROPSET_CURRENT_TA);
        let name = prop_sets.get_property_name(handle).unwrap();
        assert_eq!("gpd.ta.appID".to_string(), name);
    }

    #[fuchsia::test]
    pub fn test_enumerator_get_name_client_prop_set() {
        let mut prop_sets = init_test_tee_impl_props();
        let handle = prop_sets.allocate_property_enumerator();

        prop_sets.start_property_enumerator(handle, TEE_PROPSET_CURRENT_CLIENT);
        let name = prop_sets.get_property_name(handle).unwrap();
        assert_eq!("gpd.client.identity".to_string(), name);
    }

    #[fuchsia::test]
    pub fn test_enumerator_all_prop_types() {
        // This test uses the enumerator to run through all the test types in the following order:
        // string -> bool -> u32 -> u64 -> binary block -> uuid -> identity
        let mut prop_sets = init_test_tee_impl_props();
        let handle = prop_sets.allocate_property_enumerator();

        prop_sets.start_property_enumerator(handle, TEE_PROPSET_TEE_IMPLEMENTATION);
        let string_val = prop_sets.get_string_property(handle, "").unwrap();
        assert_eq!("asdf".to_string(), string_val);

        prop_sets.get_next_property(handle).unwrap();
        let bool_val = prop_sets.get_bool_property(handle, "").unwrap();
        assert!(bool_val);

        prop_sets.get_next_property(handle).unwrap();
        let u32_val = prop_sets.get_uint32_property(handle, "").unwrap();
        assert_eq!(57, u32_val);

        prop_sets.get_next_property(handle).unwrap();
        let u64_val = prop_sets.get_uint64_property(handle, "").unwrap();
        assert_eq!(4294967296, u64_val);

        prop_sets.get_next_property(handle).unwrap();
        let binary_block = prop_sets.get_binary_block_property(handle, "").unwrap();
        let expected_val_as_u8: Vec<u8> = vec![102, 117, 99, 104, 115, 105, 97];
        assert_eq!(7, binary_block.len());
        assert_eq!(expected_val_as_u8, binary_block);

        prop_sets.get_next_property(handle).unwrap();
        let uuid_val = prop_sets.get_uuid_property(handle, "").unwrap();
        assert_eq!(0x9cccff19, uuid_val.time_low);
        assert_eq!(0x13b5, uuid_val.time_mid);
        assert_eq!(0x4d4c, uuid_val.time_hi_and_version);
        assert_eq!([0xaa, 0x9e, 0x5c, 0x89, 0x01, 0xa5, 0x2e, 0x2f], uuid_val.clock_seq_and_node);

        prop_sets.get_next_property(handle).unwrap();
        let identity_val = prop_sets.get_identity_property(handle, "").unwrap();
        assert_eq!(tee_internal::Login::TrustedApp, identity_val.login);
        assert_eq!(0x9cccff19, identity_val.uuid.time_low);
        assert_eq!(0x13b5, identity_val.uuid.time_mid);
        assert_eq!(0x4d4c, identity_val.uuid.time_hi_and_version);
        assert_eq!(
            [0xaa, 0x9e, 0x5c, 0x89, 0x01, 0xa5, 0x2e, 0x2f],
            identity_val.uuid.clock_seq_and_node
        );

        let res = prop_sets.get_next_property(handle);
        assert_eq!(tee_internal::Error::ItemNotFound, res.err().unwrap())
    }
}
