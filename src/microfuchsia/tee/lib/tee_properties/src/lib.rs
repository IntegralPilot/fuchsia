// Copyright 2024 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use indexmap::IndexMap;
use num_traits::FromPrimitive;
use serde::{Deserialize, Serialize};
use std::fs::read_to_string;
use std::path::Path;
use std::sync::Arc;
use tee_internal::{Identity, Login, Uuid};

// Maps to GlobalPlatform TEE Internal Core API Section 4.4: Property Access Functions
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PropType {
    BinaryBlock,
    UnsignedInt32,
    UnsignedInt64,
    Boolean,
    Uuid,
    Identity,
    String,
}

#[derive(Clone, Debug, thiserror::Error)]
pub enum PropertyError {
    #[error("Bad format: Unable to parse type: {prop_type:?} from value: {value}")]
    BadFormat { prop_type: PropType, value: String },
    #[error("Item not found: {name}")]
    ItemNotFound { name: String },
    // Callers following Internal Core API spec should panic on this error.
    #[error("Generic TeeProperty error: {msg}")]
    Generic { msg: String },
}

#[derive(Debug, Deserialize, Serialize)]
pub struct TeeProperty {
    name: String,
    prop_type: PropType,
    value: String,
}

pub type TeeProperties = Vec<TeeProperty>;

// Maps to GlobalPlatform TEE Internal Core API Section 4.2.4: Property Set Pseudo-Handles
// TeePropSetTeeImplementation = 0xFFFFFFFD,
// TeePropSetCurrentClient = 0xFFFFFFFE,
// TeePropsetCurrentTA = 0xFFFFFFFF,
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PropSetType {
    TeeImplementation,
    CurrentClient,
    CurrentTA,
}

pub type PropertiesMap = IndexMap<String, (PropType, String)>;

#[derive(Clone)]
pub struct PropSet {
    properties: PropertiesMap,
}

impl PropSet {
    #[cfg(test)]
    pub(crate) fn new(properties: PropertiesMap) -> Self {
        Self { properties }
    }

    pub fn from_config_file(config_path: &Path) -> Result<Self, PropertyError> {
        match read_to_string(config_path) {
            Ok(config_string) => Self::from_config_string(&config_string),
            Err(e) => Err(PropertyError::Generic { msg: e.to_string() }),
        }
    }

    pub fn from_config_string(config_string: &str) -> Result<Self, PropertyError> {
        let props: TeeProperties = match serde_json5::from_str(config_string) {
            Ok(tee_props) => tee_props,
            Err(e) => return Err(PropertyError::Generic { msg: e.to_string() }),
        };
        let mut property_map = IndexMap::new();

        for property in props {
            property_map.insert(property.name, (property.prop_type, property.value));
        }

        Ok(Self { properties: property_map })
    }

    fn get_value(&self, prop_name: String) -> Result<String, PropertyError> {
        match self.properties.get(&prop_name) {
            Some((_, val)) => Ok(val.clone()),
            None => Err(PropertyError::ItemNotFound { name: prop_name }),
        }
    }

    pub fn get_string_property(&self, prop_name: String) -> Result<String, PropertyError> {
        self.get_value(prop_name)
    }

    pub fn get_boolean_property(&self, prop_name: String) -> Result<bool, PropertyError> {
        parse_bool(self.get_value(prop_name)?)
    }

    pub fn get_uint32_property(&self, prop_name: String) -> Result<u32, PropertyError> {
        parse_uint32(self.get_value(prop_name)?)
    }

    pub fn get_uint64_property(&self, prop_name: String) -> Result<u64, PropertyError> {
        parse_uint64(self.get_value(prop_name)?)
    }

    pub fn get_binary_block_property(&self, prop_name: String) -> Result<Vec<u8>, PropertyError> {
        parse_binary_block(self.get_value(prop_name)?)
    }

    pub fn get_uuid_property(&self, prop_name: String) -> Result<Uuid, PropertyError> {
        parse_uuid(self.get_value(prop_name)?)
    }

    pub fn get_identity_property(&self, prop_name: String) -> Result<Identity, PropertyError> {
        parse_identity(self.get_value(prop_name)?)
    }

    pub fn get_property_name_at_index(&self, index: usize) -> Result<String, PropertyError> {
        match self.properties.get_index(index) {
            Some((prop_name, (_, _))) => Ok(prop_name.to_string()),
            None => Err(PropertyError::ItemNotFound { name: format!("item at index {}", index) }),
        }
    }

    pub fn get_property_type_at_index(&self, index: usize) -> Result<PropType, PropertyError> {
        match self.properties.get_index(index) {
            Some((_, (prop_type, _))) => Ok(prop_type.clone()),
            None => Err(PropertyError::ItemNotFound { name: format!("item at index {}", index) }),
        }
    }

    pub fn get_number_of_props(&self) -> usize {
        self.properties.len()
    }
}

pub struct PropEnumerator {
    properties: Option<Arc<PropSet>>,
    index: usize,
}

impl PropEnumerator {
    pub fn new() -> Self {
        Self { properties: None, index: 0 }
    }

    // Spec indicates that callers should start() before doing other operations.
    // This impl doesn't need to do any functional work here since the initial state is valid.
    pub fn start(&mut self, propset: Arc<PropSet>) {
        self.properties = Some(propset);
        self.index = 0;
    }

    pub fn reset(&mut self) {
        self.index = 0;
    }

    // Spec: return ItemNotFound if enumerator not started or has reached end.
    pub fn next(&mut self) -> Result<(), PropertyError> {
        // The get_props()? call will cover the not_started case.
        let num_props = self.get_props()?.get_number_of_props();

        self.index = self.index + 1;
        if self.index >= num_props {
            return Err(PropertyError::ItemNotFound {
                name: "enumerator has reached the end of the property set".to_string(),
            });
        }
        Ok(())
    }

    pub fn get_property_name(&self) -> Result<String, PropertyError> {
        // This will return ItemNotFound when going out-of-bounds, compliant with spec.
        self.get_props()?.get_property_name_at_index(self.index)
    }

    pub fn get_property_type(&self) -> Result<PropType, PropertyError> {
        // This will return ItemNotFound when going out-of-bounds, compliant with spec.
        self.get_props()?.get_property_type_at_index(self.index)
    }

    pub fn get_property_as_string(&self) -> Result<String, PropertyError> {
        let prop_name = self.get_property_name()?;
        self.get_props()?.get_string_property(prop_name)
    }

    pub fn get_property_as_bool(&self) -> Result<bool, PropertyError> {
        let prop_name = self.get_property_name()?;
        self.get_props()?.get_boolean_property(prop_name)
    }

    pub fn get_property_as_u32(&self) -> Result<u32, PropertyError> {
        let prop_name = self.get_property_name()?;
        self.get_props()?.get_uint32_property(prop_name)
    }

    pub fn get_property_as_u64(&self) -> Result<u64, PropertyError> {
        let prop_name = self.get_property_name()?;
        self.get_props()?.get_uint64_property(prop_name)
    }

    pub fn get_property_as_binary_block(&self) -> Result<Vec<u8>, PropertyError> {
        let prop_name = self.get_property_name()?;
        self.get_props()?.get_binary_block_property(prop_name)
    }

    pub fn get_property_as_uuid(&self) -> Result<Uuid, PropertyError> {
        let prop_name = self.get_property_name()?;
        self.get_props()?.get_uuid_property(prop_name)
    }

    pub fn get_property_as_identity(&self) -> Result<Identity, PropertyError> {
        let prop_name = self.get_property_name()?;
        self.get_props()?.get_identity_property(prop_name)
    }

    fn get_props(&self) -> Result<&PropSet, PropertyError> {
        match &self.properties {
            Some(prop_set) => Ok(prop_set),
            None => Err(PropertyError::ItemNotFound {
                name: "enumerator has not been started".to_string(),
            }),
        }
    }
}

// These parsing functions define format of value strings expected in config files.
fn parse_bool(value: String) -> Result<bool, PropertyError> {
    match value.parse::<bool>() {
        Ok(val) => Ok(val),
        Err(_) => Err(PropertyError::BadFormat { prop_type: PropType::Boolean, value }),
    }
}

// TODO(https://fxbug.dev/369916290): Support the full list of integer value encodings (See 4.4 in
// the spec).
fn parse_uint32(value: String) -> Result<u32, PropertyError> {
    // TODO(b/369916290): Ensure parse matches spec-acceptable string number formats in section 4.4.
    match value.parse::<u32>() {
        Ok(val) => Ok(val),
        Err(_) => Err(PropertyError::BadFormat { prop_type: PropType::UnsignedInt32, value }),
    }
}

// TODO(https://fxbug.dev/369916290): Support the full list of integer value encodings (See 4.4 in
// the spec).
fn parse_uint64(value: String) -> Result<u64, PropertyError> {
    // TODO(b/369916290): Ensure parse matches spec-acceptable string number formats in section 4.4.
    match value.parse::<u64>() {
        Ok(val) => Ok(val),
        Err(_) => Err(PropertyError::BadFormat { prop_type: PropType::UnsignedInt64, value }),
    }
}

fn parse_binary_block(value: String) -> Result<Vec<u8>, PropertyError> {
    // The string is expected to be base64 encoded.
    match STANDARD.decode(value.clone()) {
        Ok(bytes) => Ok(Vec::from(bytes)),
        Err(_) => Err(PropertyError::BadFormat { prop_type: PropType::BinaryBlock, value }),
    }
}

fn parse_uuid(value: String) -> Result<Uuid, PropertyError> {
    match uuid::Uuid::parse_str(&value) {
        Ok(uuid) => {
            let (time_low, time_mid, time_hi_and_version, clock_seq_and_node) = uuid.as_fields();
            Ok(Uuid {
                time_low: time_low,
                time_mid: time_mid,
                time_hi_and_version: time_hi_and_version,
                clock_seq_and_node: *clock_seq_and_node,
            })
        }
        Err(_) => Err(PropertyError::BadFormat { prop_type: PropType::Uuid, value }),
    }
}

fn parse_identity(value: String) -> Result<Identity, PropertyError> {
    // Encoded as `integer (':' uuid)?`.
    let (login_str, uuid) = match value.split_once(':') {
        Some((login_str, uuid_str)) => (login_str, parse_uuid(uuid_str.to_string())?),
        None => (value.as_str(), Uuid::default()),
    };
    let login_val = match login_str.parse::<u32>() {
        Ok(val) => val,
        Err(_) => return Err(PropertyError::BadFormat { prop_type: PropType::Identity, value }),
    };
    let login = Login::from_u32(login_val)
        .ok_or(PropertyError::BadFormat { prop_type: PropType::Identity, value })?;
    Ok(Identity { login, uuid })
}

// TODO(b/366015756): Parsing logic should be fuzzed for production-hardening.
#[cfg(test)]
pub mod tests {
    use super::*;

    const TEST_PROP_NAME_STRING: &str = "gpd.tee.test.string";
    const TEST_PROP_NAME_BOOL: &str = "gpd.tee.test.bool";
    const TEST_PROP_NAME_U32: &str = "gpd.tee.test.u32";
    const TEST_PROP_NAME_U64: &str = "gpd.tee.test.u64";
    const TEST_PROP_NAME_BINARY_BLOCK: &str = "gpd.tee.test.binaryBlock";
    const TEST_PROP_NAME_UUID: &str = "gpd.tee.test.uuid";
    const TEST_PROP_NAME_IDENTITY: &str = "gpd.tee.test.identity";

    const TEST_PROP_VAL_STRING: &str = "asdf";
    const TEST_PROP_VAL_BOOL: &str = "true";
    const TEST_PROP_VAL_U32: &str = "57";
    const TEST_PROP_VAL_U64: &str = "4294967296"; // U32::MAX + 1
    const TEST_PROP_VAL_BINARY_BLOCK: &str = "ZnVjaHNpYQ=="; // base64 encoding of "fuchsia"

    // Randomly generated UUID for testing.
    const TEST_PROP_VAL_UUID: &str = "9cccff19-13b5-4d4c-aa9e-5c8901a52e2f";
    const TEST_PROP_UUID: Uuid = Uuid {
        time_low: 0x9cccff19,
        time_mid: 0x13b5,
        time_hi_and_version: 0x4d4c,
        clock_seq_and_node: [0xaa, 0x9e, 0x5c, 0x89, 0x01, 0xa5, 0x2e, 0x2f],
    };

    // TODO(https://fxbug.dev/369916290): Spell as 0xf0000000 when hex encodings are supported.
    const TEST_PROP_VAL_IDENTITY: &str = "4026531840:9cccff19-13b5-4d4c-aa9e-5c8901a52e2f";
    const TEST_PROP_IDENTITY: Identity =
        Identity { login: Login::TrustedApp, uuid: TEST_PROP_UUID };

    fn create_test_prop_map() -> PropertiesMap {
        let mut props: IndexMap<String, (PropType, String)> = IndexMap::new();
        props.insert(
            TEST_PROP_NAME_STRING.to_string(),
            (PropType::String, TEST_PROP_VAL_STRING.to_string()),
        );
        props.insert(
            TEST_PROP_NAME_BOOL.to_string(),
            (PropType::Boolean, TEST_PROP_VAL_BOOL.to_string()),
        );
        props.insert(
            TEST_PROP_NAME_U32.to_string(),
            (PropType::UnsignedInt32, TEST_PROP_VAL_U32.to_string()),
        );
        props.insert(
            TEST_PROP_NAME_U64.to_string(),
            (PropType::UnsignedInt64, TEST_PROP_VAL_U64.to_string()),
        );
        props.insert(
            TEST_PROP_NAME_BINARY_BLOCK.to_string(),
            (PropType::BinaryBlock, TEST_PROP_VAL_BINARY_BLOCK.to_string()),
        );
        props.insert(
            TEST_PROP_NAME_UUID.to_string(),
            (PropType::Uuid, TEST_PROP_VAL_UUID.to_string()),
        );
        props.insert(
            TEST_PROP_NAME_IDENTITY.to_string(),
            (PropType::Identity, TEST_PROP_VAL_IDENTITY.to_string()),
        );
        props
    }

    fn create_test_prop_set() -> PropSet {
        PropSet::new(create_test_prop_map())
    }

    #[test]
    pub fn test_load_config_from_string() {
        let config_json = r#"[
            {
                "name": "gpd.tee.asdf",
                "prop_type": "boolean",
                "value": "true"
            },
            {
                "name": "gpd.tee.other",
                "prop_type": "binary_block",
                "value": "testingzz"
            }
        ]
        "#;

        let prop_set: PropSet = PropSet::from_config_string(config_json).expect("loading config");

        let bool_prop_value =
            prop_set.get_boolean_property("gpd.tee.asdf".to_string()).expect("getting bool prop");
        assert_eq!(true, bool_prop_value)
    }

    #[test]
    pub fn test_load_config_from_string_failure() {
        // Invalid json (missing opening `{` for first object)
        let config_json = r#"[
                "name": "gpd.tee.asdf",
                "prop_type": "boolean",
                "value": "true"
            },
            {
                "name": "gpd.tee.other",
                "prop_type": "binary_block",
                "value": "testingzz"
            }
        ]
        "#;

        let res = PropSet::from_config_string(config_json);

        match res.err() {
            Some(PropertyError::Generic { .. }) => (),
            _ => assert!(false, "Unexpected error type"),
        }
    }

    // *** Enumerator Tests ***
    #[test]
    pub fn test_enumerator_query_prop_types_success() {
        let mut enumerator = PropEnumerator::new();

        // Enumerate in the order of test prop set, starting with string.
        enumerator.start(Arc::new(create_test_prop_set()));

        let prop_name = enumerator.get_property_name().expect("getting prop name");
        assert_eq!(TEST_PROP_NAME_STRING.to_string(), prop_name);

        let prop_type = enumerator.get_property_type().expect("getting prop type");
        assert_eq!(PropType::String, prop_type);

        let prop_val = enumerator.get_property_as_string().expect("getting prop as string");
        assert_eq!(TEST_PROP_VAL_STRING.to_string(), prop_val);

        // bool.
        enumerator.next().expect("moving enumerator to next prop");

        let prop_name = enumerator.get_property_name().expect("getting prop name");
        assert_eq!(TEST_PROP_NAME_BOOL.to_string(), prop_name);

        let prop_type = enumerator.get_property_type().expect("getting prop type");
        assert_eq!(PropType::Boolean, prop_type);

        let prop_val = enumerator.get_property_as_bool().expect("getting prop as bool");
        assert_eq!(true, prop_val);

        // u32.
        enumerator.next().expect("moving enumerator to next prop");

        let prop_name = enumerator.get_property_name().expect("getting prop name");
        assert_eq!(TEST_PROP_NAME_U32.to_string(), prop_name);

        let prop_type = enumerator.get_property_type().expect("getting prop type");
        assert_eq!(PropType::UnsignedInt32, prop_type);

        let prop_val = enumerator.get_property_as_u32().expect("getting prop as u32");
        assert_eq!(57, prop_val);

        // u64.
        enumerator.next().expect("moving enumerator to next prop");

        let prop_name = enumerator.get_property_name().expect("getting prop name");
        assert_eq!(TEST_PROP_NAME_U64.to_string(), prop_name);

        let prop_type = enumerator.get_property_type().expect("getting prop type");
        assert_eq!(PropType::UnsignedInt64, prop_type);

        let prop_val = enumerator.get_property_as_u64().expect("getting prop as u64");
        assert_eq!(4294967296, prop_val);

        // Binary block.
        enumerator.next().expect("moving enumerator to next prop");

        let prop_name = enumerator.get_property_name().expect("getting prop name");
        assert_eq!(TEST_PROP_NAME_BINARY_BLOCK.to_string(), prop_name);

        let prop_type = enumerator.get_property_type().expect("getting prop type");
        assert_eq!(PropType::BinaryBlock, prop_type);

        let prop_val =
            enumerator.get_property_as_binary_block().expect("getting prop as binary block");
        let bytes_expected = STANDARD.decode("ZnVjaHNpYQ==").expect("decoding binary block string");
        assert_eq!(bytes_expected, prop_val);

        // UUID.
        enumerator.next().expect("moving enumerator to next prop");

        let prop_name = enumerator.get_property_name().expect("getting prop name");
        assert_eq!(TEST_PROP_NAME_UUID.to_string(), prop_name);

        let prop_type = enumerator.get_property_type().expect("getting prop type");
        assert_eq!(PropType::Uuid, prop_type);

        let prop_val: Uuid = enumerator.get_property_as_uuid().expect("getting prop as uuid");
        assert_eq!(TEST_PROP_UUID.time_low, prop_val.time_low);
        assert_eq!(TEST_PROP_UUID.time_mid, prop_val.time_mid);
        assert_eq!(TEST_PROP_UUID.time_hi_and_version, prop_val.time_hi_and_version);
        assert_eq!(TEST_PROP_UUID.clock_seq_and_node, prop_val.clock_seq_and_node);

        // Identity.
        enumerator.next().expect("moving enumerator to next prop");

        let prop_name = enumerator.get_property_name().expect("getting prop name");
        assert_eq!(TEST_PROP_NAME_IDENTITY.to_string(), prop_name);

        let prop_type = enumerator.get_property_type().expect("getting prop type");
        assert_eq!(PropType::Identity, prop_type);

        let prop_val: Identity =
            enumerator.get_property_as_identity().expect("getting prop as identity");
        assert_eq!(TEST_PROP_IDENTITY.login, prop_val.login);
        // This should have the same test UUID; can reuse parsed expected uuid fields.
        assert_eq!(TEST_PROP_IDENTITY.uuid.time_low, prop_val.uuid.time_low);
        assert_eq!(TEST_PROP_IDENTITY.uuid.time_mid, prop_val.uuid.time_mid);
        assert_eq!(TEST_PROP_IDENTITY.uuid.time_hi_and_version, prop_val.uuid.time_hi_and_version);
        assert_eq!(TEST_PROP_IDENTITY.uuid.clock_seq_and_node, prop_val.uuid.clock_seq_and_node);

        // Test error upon going out of bounds via next().
        let res = enumerator.next();
        match res.err() {
            Some(PropertyError::ItemNotFound { .. }) => (),
            _ => assert!(false, "Unexpected error type"),
        }
    }

    #[test]
    pub fn test_enumerator_reset() {
        let mut enumerator = PropEnumerator::new();
        enumerator.start(Arc::new(create_test_prop_set()));

        let prop_name = enumerator.get_property_name().expect("getting prop name");
        assert_eq!(TEST_PROP_NAME_STRING.to_string(), prop_name);

        enumerator.next().expect("moving enumerator to next prop");

        let prop_name = enumerator.get_property_name().expect("getting prop name");
        assert_eq!(TEST_PROP_NAME_BOOL.to_string(), prop_name);

        enumerator.reset();

        let prop_name = enumerator.get_property_name().expect("getting prop name");
        assert_eq!(TEST_PROP_NAME_STRING.to_string(), prop_name);
    }

    #[test]
    pub fn test_enumerator_wrong_prop_type_error() {
        let mut enumerator = PropEnumerator::new();
        enumerator.start(Arc::new(create_test_prop_set()));

        // First value is string type and not interpretable as bool, should error.
        let res = enumerator.get_property_as_bool();
        match res.err() {
            Some(PropertyError::BadFormat { .. }) => (),
            _ => assert!(false, "Unexpected error type"),
        }

        enumerator.next().expect("moving enumerator to next prop");

        // Second value is bool, not interpretable as identity, should error.
        let res = enumerator.get_property_as_identity();
        match res.err() {
            Some(PropertyError::BadFormat { .. }) => (),
            _ => assert!(false, "Unexpected error type"),
        }

        // Getting the same bool value as bool should still work.
        let res = enumerator.get_property_as_bool();
        assert!(res.is_ok());
    }

    #[test]
    pub fn test_enumerator_not_started_error() {
        // This enumerator functionally starts in a usable initial state, with internal index = 0.
        // The spec says callers should call start() first though, so we error if it isn't called.
        let enumerator = PropEnumerator::new();

        let res = enumerator.get_property_name();

        // Return code should map to item not found error per spec.
        match res.err() {
            Some(PropertyError::ItemNotFound { .. }) => (),
            _ => assert!(false, "Unexpected error type"),
        }
    }

    // *** PropSet Getter Tests ***
    #[test]
    pub fn test_propset_get_not_found() {
        let prop_set = create_test_prop_set();

        let res = prop_set.get_boolean_property("name.that.isnt.there".to_string());

        match res.err() {
            Some(PropertyError::ItemNotFound { .. }) => (),
            _ => assert!(false, "Unexpected error type"),
        }
    }

    #[test]
    pub fn test_propset_get_string_success() {
        let prop_set = create_test_prop_set();

        let test_str_val = prop_set
            .get_string_property(TEST_PROP_NAME_STRING.to_string())
            .expect("getting str prop val");
        assert_eq!(TEST_PROP_VAL_STRING.to_string(), test_str_val);

        // Spec indicates that any value can be represented as string, even if it doesn't match the type.
        let test_bool_val_as_str = prop_set
            .get_string_property(TEST_PROP_NAME_BOOL.to_string())
            .expect("getting bool prop val as string");
        assert_eq!(TEST_PROP_VAL_BOOL.to_string(), test_bool_val_as_str);
    }

    #[test]
    pub fn test_propset_get_bool_success() {
        let prop_set = create_test_prop_set();

        let val = prop_set
            .get_boolean_property(TEST_PROP_NAME_BOOL.to_string())
            .expect("getting bool prop val");
        assert_eq!(true, val);
    }

    #[test]
    pub fn test_propset_get_bool_wrong_type() {
        let prop_set = create_test_prop_set();

        let res = prop_set.get_boolean_property(TEST_PROP_NAME_UUID.to_string());
        match res.err() {
            Some(PropertyError::BadFormat { .. }) => (),
            _ => assert!(false, "Unexpected error type"),
        }
    }

    #[test]
    pub fn test_propset_get_u32_success() {
        let prop_set = create_test_prop_set();

        let val = prop_set
            .get_uint32_property(TEST_PROP_NAME_U32.to_string())
            .expect("getting u32 prop val");
        assert_eq!(57, val);
    }

    #[test]
    pub fn test_propset_get_u32_wrong_type() {
        let prop_set = create_test_prop_set();

        let res = prop_set.get_uint32_property(TEST_PROP_NAME_BOOL.to_string());
        match res.err() {
            Some(PropertyError::BadFormat { .. }) => (),
            _ => assert!(false, "Unexpected error type"),
        }
    }

    #[test]
    pub fn test_propset_get_u64_success() {
        let prop_set = create_test_prop_set();

        let val = prop_set
            .get_uint64_property(TEST_PROP_NAME_U64.to_string())
            .expect("getting u64 prop val");
        assert_eq!(4294967296, val);
    }

    #[test]
    pub fn test_propset_get_u64_wrong_type() {
        let prop_set = create_test_prop_set();

        let res = prop_set.get_uint64_property(TEST_PROP_NAME_BOOL.to_string());
        match res.err() {
            Some(PropertyError::BadFormat { .. }) => (),
            _ => assert!(false, "Unexpected error type"),
        }
    }

    #[test]
    pub fn test_propset_get_binary_block_success() {
        let prop_set = create_test_prop_set();

        let val = prop_set
            .get_binary_block_property(TEST_PROP_NAME_BINARY_BLOCK.to_string())
            .expect("getting binary block prop val");

        let expected_bytes = STANDARD
            .decode(TEST_PROP_VAL_BINARY_BLOCK)
            .expect("decoding expected binary block bytes");
        assert_eq!(expected_bytes, val);
    }

    #[test]
    pub fn test_propset_get_binary_block_wrong_type() {
        let prop_set = create_test_prop_set();

        // Technically most of the test values (bool, u32, u64, string) are valid base64 strings.
        // Use the serialized identity string to trigger parse failure since it has invalid chars.
        let res = prop_set.get_binary_block_property(TEST_PROP_NAME_IDENTITY.to_string());
        match res.err() {
            Some(PropertyError::BadFormat { .. }) => (),
            _ => assert!(false, "Unexpected error type"),
        }
    }

    #[test]
    pub fn test_propset_get_uuid_success() {
        let prop_set = create_test_prop_set();

        let val = prop_set
            .get_uuid_property(TEST_PROP_NAME_UUID.to_string())
            .expect("getting uuid prop val");

        assert_eq!(TEST_PROP_UUID.time_low, val.time_low);
        assert_eq!(TEST_PROP_UUID.time_mid, val.time_mid);
        assert_eq!(TEST_PROP_UUID.time_hi_and_version, val.time_hi_and_version);
        assert_eq!(TEST_PROP_UUID.clock_seq_and_node, val.clock_seq_and_node);
    }

    #[test]
    pub fn test_propset_get_uuid_wrong_type() {
        let prop_set = create_test_prop_set();

        let res = prop_set.get_uuid_property(TEST_PROP_NAME_BOOL.to_string());
        match res.err() {
            Some(PropertyError::BadFormat { .. }) => (),
            _ => assert!(false, "Unexpected error type"),
        }
    }

    #[test]
    pub fn test_propset_get_identity_success() {
        let prop_set = create_test_prop_set();

        let val: Identity = prop_set
            .get_identity_property(TEST_PROP_NAME_IDENTITY.to_string())
            .expect("getting identity prop val");

        assert_eq!(TEST_PROP_IDENTITY.login, val.login);
        assert_eq!(TEST_PROP_IDENTITY.uuid.time_low, val.uuid.time_low);
        assert_eq!(TEST_PROP_IDENTITY.uuid.time_mid, val.uuid.time_mid);
        assert_eq!(TEST_PROP_IDENTITY.uuid.time_hi_and_version, val.uuid.time_hi_and_version);
        assert_eq!(TEST_PROP_IDENTITY.uuid.clock_seq_and_node, val.uuid.clock_seq_and_node);
    }

    #[test]
    pub fn test_propset_get_identity_wrong_type() {
        let prop_set = create_test_prop_set();

        let res = prop_set.get_identity_property(TEST_PROP_NAME_BOOL.to_string());
        match res.err() {
            Some(PropertyError::BadFormat { .. }) => (),
            _ => assert!(false, "Unexpected error type"),
        }
    }

    // *** Parsing Function Tests ***
    #[test]
    pub fn test_parse_bool_success() {
        let val_true = "true".to_string();
        let val_false = "false".to_string();

        let res_true = parse_bool(val_true).expect("parsing true");
        let res_false = parse_bool(val_false).expect("parsing false");

        assert!(res_true);
        assert!(!res_false);
    }

    #[test]
    pub fn test_parse_bool_empty_string() {
        let val = "".to_string();

        let res = parse_bool(val);

        match res.err() {
            Some(PropertyError::BadFormat { .. }) => (),
            _ => assert!(false, "Unexpected error type"),
        }
    }

    #[test]
    pub fn test_parse_bool_bad_format() {
        let val = "asdf".to_string();

        let res = parse_bool(val);

        match res.err() {
            Some(PropertyError::BadFormat { .. }) => (),
            _ => assert!(false, "Unexpected error type"),
        }
    }

    #[test]
    pub fn test_parse_bool_caps_not_accepted() {
        let val = "TRUE".to_string();

        let res = parse_bool(val);

        match res.err() {
            Some(PropertyError::BadFormat { .. }) => (),
            _ => assert!(false, "Unexpected error type"),
        }
    }

    #[test]
    pub fn test_parse_u32_success() {
        let val = "15".to_string();

        let res = parse_uint32(val).expect("parsing 15");

        assert_eq!(res, 15);
    }

    #[test]
    pub fn test_parse_u32_empty_string() {
        let val = "".to_string();

        let res = parse_uint32(val);

        match res.err() {
            Some(PropertyError::BadFormat { .. }) => (),
            _ => assert!(false, "Unexpected error type"),
        }
    }

    #[test]
    pub fn test_parse_u32_negative_value() {
        let val = "-15".to_string();

        let res = parse_uint32(val);

        match res.err() {
            Some(PropertyError::BadFormat { .. }) => (),
            _ => assert!(false, "Unexpected error type"),
        }
    }

    #[test]
    pub fn test_parse_u32_too_large() {
        // u32::MAX = 4_294_967_295u32
        let val = "4294967296".to_string();

        let res = parse_uint32(val);

        match res.err() {
            Some(PropertyError::BadFormat { .. }) => (),
            _ => assert!(false, "Unexpected error type"),
        }
    }

    #[test]
    pub fn test_parse_u32_bad_format() {
        let val = "text".to_string();

        let res = parse_uint32(val);

        match res.err() {
            Some(PropertyError::BadFormat { .. }) => (),
            _ => assert!(false, "Unexpected error type"),
        }
    }

    #[test]
    pub fn test_parse_u64_success() {
        let val = "4294967296".to_string();

        let res = parse_uint64(val).expect("parsing 4294967296");

        assert_eq!(res, 4294967296);
    }

    #[test]
    pub fn test_parse_u64_empty_string() {
        let val = "".to_string();

        let res = parse_uint64(val);

        match res.err() {
            Some(PropertyError::BadFormat { .. }) => (),
            _ => assert!(false, "Unexpected error type"),
        }
    }

    #[test]
    pub fn test_parse_u64_negative_value() {
        let val = "-15".to_string();

        let res = parse_uint64(val);

        match res.err() {
            Some(PropertyError::BadFormat { .. }) => (),
            _ => assert!(false, "Unexpected error type"),
        }
    }

    #[test]
    pub fn test_parse_u64_too_large() {
        // u64::MAX = 18_446_744_073_709_551_615u64
        let val = "18446744073709551616".to_string();

        let res = parse_uint64(val);

        match res.err() {
            Some(PropertyError::BadFormat { .. }) => (),
            _ => assert!(false, "Unexpected error type"),
        }
    }

    #[test]
    pub fn test_parse_u64_bad_format() {
        let val = "text".to_string();

        let res = parse_uint64(val);

        match res.err() {
            Some(PropertyError::BadFormat { .. }) => (),
            _ => assert!(false, "Unexpected error type"),
        }
    }

    #[test]
    pub fn test_parse_binary_block_success() {
        let bytes: [u8; 6] = [1, 2, 3, 4, 5, 6];
        let encoded = STANDARD.encode(bytes);

        let res = parse_binary_block(encoded).expect("parsing binary block");

        assert_eq!(bytes.to_vec(), res);
    }

    #[test]
    pub fn test_parse_binary_empty_string() {
        let res = parse_binary_block("".to_string());

        assert!(res.is_ok());
    }

    #[test]
    pub fn test_parse_binary_block_invalid_base64() {
        // Include characters outside of standard base64 set.
        let bad_val = "asdf&^%@".to_string();

        let res = parse_binary_block(bad_val);

        match res.err() {
            Some(PropertyError::BadFormat { .. }) => (),
            _ => assert!(false, "Unexpected error type"),
        }
    }

    #[test]
    pub fn test_parse_binary_block_invalid_padding_chars_only() {
        // Include characters outside of standard base64 set.
        let bad_val = "====".to_string();

        let res = parse_binary_block(bad_val);

        match res.err() {
            Some(PropertyError::BadFormat { .. }) => (),
            _ => assert!(false, "Unexpected error type"),
        }
    }

    #[test]
    pub fn test_parse_uuid_success() {
        let valid_uuid = "9cccff19-13b5-4d4c-aa9e-5c8901a52e2f".to_string();

        let res = parse_uuid(valid_uuid);

        assert!(res.is_ok());
    }

    #[test]
    pub fn test_parse_uuid_empty_string() {
        let res = parse_uuid("".to_string());

        match res.err() {
            Some(PropertyError::BadFormat { .. }) => (),
            _ => assert!(false, "Unexpected error type"),
        }
    }

    #[test]
    pub fn test_parse_uuid_invalid_uuid() {
        let invalid_uuid = "asdf".to_string();

        let res = parse_uuid(invalid_uuid);

        match res.err() {
            Some(PropertyError::BadFormat { .. }) => (),
            _ => assert!(false, "Unexpected error type"),
        }
    }

    #[test]
    pub fn test_parse_identity_success() {
        let res = parse_identity(TEST_PROP_VAL_IDENTITY.to_string());
        assert!(res.is_ok());
    }

    #[test]
    pub fn test_parse_identity_empty_uuid() {
        // TODO(https://fxbug.dev/369916290): Spell as 0xf0000000 when hex encodings are supported.
        let res = parse_identity("4026531840".to_string());
        assert!(res.is_ok());
        let id = res.unwrap();
        assert_eq!(Uuid::default(), id.uuid);
    }

    #[test]
    pub fn test_parse_identity_invalid_login_type() {
        // 0xefffffff is an implementation-reserved value.
        //
        // TODO(https://fxbug.dev/369916290): Spell as 0xefffffff when hex encodings are supported.
        let res = parse_identity("4026531839:9cccff19-13b5-4d4c-aa9e-5c8901a52e2f".to_string());
        match res.err() {
            Some(PropertyError::BadFormat { .. }) => (),
            _ => assert!(false, "Unexpected error type"),
        }
    }
}
