// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use cm_rust::{ConfigNestedValueType, ConfigValueType};
use flex_fuchsia_component_decl as fdecl;
use flex_fuchsia_sys2 as fsys;
use std::str::FromStr;

use crate::cli::show::config_table_print;
use crate::config::{UseConfigurationOrConfigField, resolve_config_decls};
use crate::query::get_single_instance_from_query;

use super::reload_cmd;

pub async fn config_set_cmd<W: std::io::Write>(
    query: String,
    key_values: Vec<String>,
    reload: bool,
    lifecycle_controller: fsys::LifecycleControllerProxy,
    realm_query: fsys::RealmQueryProxy,
    config_override: fsys::ConfigOverrideProxy,
    writer: W,
) -> Result<()> {
    let instance = get_single_instance_from_query(&query, &realm_query).await?;
    let decls = resolve_config_decls(&realm_query, &instance.moniker).await?;
    let fields = key_values
        .into_iter()
        .map(|kv| parse_config_key_value(&kv, &decls))
        .collect::<Result<Vec<fdecl::ConfigOverride>>>()?;
    config_override
        .set_structured_config(instance.moniker.as_ref(), &fields)
        .await?
        .map_err(|e| anyhow::anyhow!("{:?}", e))?;
    if reload {
        reload_cmd(query, lifecycle_controller, realm_query, writer).await?;
    }
    Ok(())
}

pub async fn config_unset_cmd<W: std::io::Write>(
    query: Option<String>,
    reload: bool,
    lifecycle_controller: fsys::LifecycleControllerProxy,
    realm_query: fsys::RealmQueryProxy,
    config_override: fsys::ConfigOverrideProxy,
    writer: W,
) -> Result<()> {
    let (moniker, query) = match query {
        Some(q) => {
            let instance = get_single_instance_from_query(&q, &realm_query).await?;
            (instance.moniker.to_string(), q)
        }
        None => ("".to_string(), "".to_string()),
    };
    config_override
        .unset_structured_config(&moniker)
        .await?
        .map_err(|e| anyhow::anyhow!("{:?}", e))?;
    if reload && !query.is_empty() {
        reload_cmd(query, lifecycle_controller, realm_query, writer).await?;
    }
    Ok(())
}

pub async fn config_list_cmd<W: std::io::Write>(
    query: String,
    realm_query: fsys::RealmQueryProxy,
    writer: W,
) -> Result<()> {
    config_table_print(query, realm_query, writer).await
}

fn parse_config_key_value(
    kv: &str,
    decls: &Vec<UseConfigurationOrConfigField>,
) -> Result<fdecl::ConfigOverride> {
    let mut kv = kv.split("=");
    let key = kv
        .next()
        .ok_or_else(|| anyhow::anyhow!("invalid key=value formatting"))?
        .trim()
        .to_string();
    let value = kv
        .next()
        .ok_or_else(|| anyhow::anyhow!("invalid key=value formatting"))?
        .trim()
        .to_string();
    let config_type = decls
        .iter()
        .find_map(|d| match d {
            UseConfigurationOrConfigField::UseConfiguration(use_config) => {
                if key == use_config.target_name.to_string().to_lowercase() {
                    Some(use_config.type_.clone())
                } else {
                    None
                }
            }
            UseConfigurationOrConfigField::ConfigField(config_field) => {
                if key == config_field.key.to_lowercase() {
                    Some(config_field.type_.clone())
                } else {
                    None
                }
            }
        })
        .ok_or_else(|| anyhow::anyhow!("configuration capability not declared for key {key}",))?;
    let value = parse_config_value(&value, config_type)?;
    Ok(fdecl::ConfigOverride { key: Some(key), value: Some(value), ..Default::default() })
}

fn parse_config_value(value: &str, config_type: ConfigValueType) -> Result<fdecl::ConfigValue> {
    let result =
        match config_type {
            ConfigValueType::Bool => {
                fdecl::ConfigValue::Single(fdecl::ConfigSingleValue::Bool(bool::from_str(value)?))
            }
            ConfigValueType::Uint8 => {
                fdecl::ConfigValue::Single(fdecl::ConfigSingleValue::Uint8(u8::from_str(value)?))
            }
            ConfigValueType::Uint16 => {
                fdecl::ConfigValue::Single(fdecl::ConfigSingleValue::Uint16(u16::from_str(value)?))
            }
            ConfigValueType::Uint32 => {
                fdecl::ConfigValue::Single(fdecl::ConfigSingleValue::Uint32(u32::from_str(value)?))
            }
            ConfigValueType::Uint64 => {
                fdecl::ConfigValue::Single(fdecl::ConfigSingleValue::Uint64(u64::from_str(value)?))
            }
            ConfigValueType::Int8 => {
                fdecl::ConfigValue::Single(fdecl::ConfigSingleValue::Int8(i8::from_str(value)?))
            }
            ConfigValueType::Int16 => {
                fdecl::ConfigValue::Single(fdecl::ConfigSingleValue::Int16(i16::from_str(value)?))
            }
            ConfigValueType::Int32 => {
                fdecl::ConfigValue::Single(fdecl::ConfigSingleValue::Int32(i32::from_str(value)?))
            }
            ConfigValueType::Int64 => {
                fdecl::ConfigValue::Single(fdecl::ConfigSingleValue::Int64(i64::from_str(value)?))
            }
            ConfigValueType::String { max_size: _ } => {
                fdecl::ConfigValue::Single(fdecl::ConfigSingleValue::String(String::from(value)))
            }
            ConfigValueType::Vector { nested_type, max_count: _ } => match nested_type {
                ConfigNestedValueType::Bool => fdecl::ConfigValue::Vector(
                    fdecl::ConfigVectorValue::BoolVector(try_parse_vec::<bool>(value)?),
                ),
                ConfigNestedValueType::Uint8 => fdecl::ConfigValue::Vector(
                    fdecl::ConfigVectorValue::Uint8Vector(try_parse_vec::<u8>(value)?),
                ),
                ConfigNestedValueType::Uint16 => fdecl::ConfigValue::Vector(
                    fdecl::ConfigVectorValue::Uint16Vector(try_parse_vec::<u16>(value)?),
                ),
                ConfigNestedValueType::Uint32 => fdecl::ConfigValue::Vector(
                    fdecl::ConfigVectorValue::Uint32Vector(try_parse_vec::<u32>(value)?),
                ),
                ConfigNestedValueType::Uint64 => fdecl::ConfigValue::Vector(
                    fdecl::ConfigVectorValue::Uint64Vector(try_parse_vec::<u64>(value)?),
                ),
                ConfigNestedValueType::Int8 => fdecl::ConfigValue::Vector(
                    fdecl::ConfigVectorValue::Int8Vector(try_parse_vec::<i8>(value)?),
                ),
                ConfigNestedValueType::Int16 => fdecl::ConfigValue::Vector(
                    fdecl::ConfigVectorValue::Int16Vector(try_parse_vec::<i16>(value)?),
                ),
                ConfigNestedValueType::Int32 => fdecl::ConfigValue::Vector(
                    fdecl::ConfigVectorValue::Int32Vector(try_parse_vec::<i32>(value)?),
                ),
                ConfigNestedValueType::Int64 => fdecl::ConfigValue::Vector(
                    fdecl::ConfigVectorValue::Int64Vector(try_parse_vec::<i64>(value)?),
                ),
                ConfigNestedValueType::String { max_size: _ } => fdecl::ConfigValue::Vector(
                    fdecl::ConfigVectorValue::StringVector(try_parse_vec::<String>(value)?),
                ),
            },
        };
    Ok(result)
}

fn try_parse_vec<T: FromStr>(value: &str) -> Result<Vec<T>, T::Err> {
    value.split(",").map(str::trim).map(|v| T::from_str(v)).collect()
}

#[cfg(test)]
mod test {

    use super::*;
    use std::collections::HashMap;

    use crate::test_utils::{serve_lifecycle_controller, serve_realm_query};
    use cm_rust::FidlIntoNative;
    use futures::TryStreamExt;
    use moniker::Moniker;
    use test_case::test_case;

    fn serve_config_override(
        expected_moniker: &'static str,
        expected_kvs: Vec<cm_rust::ConfigOverride>,
    ) -> fsys::ConfigOverrideProxy {
        let (config_override, mut stream) =
            fidl::endpoints::create_proxy_and_stream::<fsys::ConfigOverrideMarker>();

        fuchsia_async::Task::local(async move {
            loop {
                match stream.try_next().await.unwrap().unwrap() {
                    fsys::ConfigOverrideRequest::SetStructuredConfig {
                        moniker,
                        fields,
                        responder,
                    } => {
                        eprintln!("SetStructuredConfig call for {moniker} with {fields:?}");
                        assert_eq!(
                            Moniker::parse_str(expected_moniker),
                            Moniker::parse_str(&moniker)
                        );
                        let result = fields
                            .into_iter()
                            .map(FidlIntoNative::fidl_into_native)
                            .map(|field| {
                                if expected_kvs.contains(&field) {
                                    Ok(())
                                } else {
                                    Err(fsys::ConfigOverrideError::KeyNotFound)
                                }
                            })
                            .collect::<Result<Vec<()>, fsys::ConfigOverrideError>>();
                        match result {
                            Ok(_) => responder.send(Ok(())).unwrap(),
                            Err(e) => responder.send(Err(e)).unwrap(),
                        };
                    }
                    fsys::ConfigOverrideRequest::UnsetStructuredConfig { moniker, responder } => {
                        eprintln!("UnsetStructuredConfig call for {moniker}");
                        if !moniker.is_empty() {
                            assert_eq!(
                                Moniker::parse_str(expected_moniker),
                                Moniker::parse_str(&moniker)
                            );
                        }
                        responder.send(Ok(())).unwrap();
                    }
                    fsys::ConfigOverrideRequest::_UnknownMethod { ordinal, control_handle: _, method_type, .. } => {
                        eprintln!("_UnknownMethod call with ordinal {ordinal} and method type {method_type:?}");
                        break;
                    }
                }
            }
        })
        .detach();
        config_override
    }

    fn setup() -> (fsys::LifecycleControllerProxy, fsys::ConfigOverrideProxy, fsys::RealmQueryProxy)
    {
        let lifecycle_controller = serve_lifecycle_controller("./my_foo");
        let config_override = serve_config_override(
            "./my_foo",
            vec![
                cm_rust::ConfigOverride {
                    key: "foo".into(),
                    value: cm_rust::ConfigValue::Single(cm_rust::ConfigSingleValue::Bool(true)),
                },
                cm_rust::ConfigOverride {
                    key: "bar".into(),
                    value: cm_rust::ConfigValue::Single(cm_rust::ConfigSingleValue::Uint64(42)),
                },
            ],
        );
        let instances = vec![fsys::Instance {
            moniker: Some("./my_foo".to_string()),
            url: Some("fuchsia-pkg://fuchsia.com/foo#meta/foo.cm".to_string()),
            ..Default::default()
        }];
        let manifests = HashMap::from([(
            "./my_foo".to_string(),
            fdecl::Component {
                uses: Some(vec![fdecl::Use::Config(fdecl::UseConfiguration {
                    source: Some(fdecl::Ref::Parent(fdecl::ParentRef)),
                    source_name: Some("fuchsia.foo".to_string()),
                    target_name: Some("foo".to_string()),
                    type_: Some(fdecl::ConfigType {
                        layout: fdecl::ConfigTypeLayout::Bool,
                        constraints: Vec::new(),
                        parameters: None,
                    }),
                    ..Default::default()
                })]),
                config: Some(fdecl::ConfigSchema {
                    fields: Some(vec![fdecl::ConfigField {
                        key: Some("bar".to_string()),
                        type_: Some(fdecl::ConfigType {
                            layout: fdecl::ConfigTypeLayout::Uint64,
                            constraints: Vec::new(),
                            parameters: None,
                        }),
                        ..Default::default()
                    }]),
                    checksum: Some(fdecl::ConfigChecksum::Sha256([0; 32])),
                    value_source: Some(fdecl::ConfigValueSource::PackagePath("".to_string())),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )]);
        let configs = HashMap::from([(
            "./my_foo".to_string(),
            fdecl::ResolvedConfig {
                fields: vec![fdecl::ResolvedConfigField {
                    key: "foo".to_string(),
                    value: fdecl::ConfigValue::Single(fdecl::ConfigSingleValue::Bool(false)),
                }],
                checksum: fdecl::ConfigChecksum::Sha256([0; 32]),
            },
        )]);
        let realm_query = serve_realm_query(instances, manifests, configs, HashMap::new());
        (lifecycle_controller, config_override, realm_query)
    }

    #[test_case(vec!["foo=true".to_string()], false, true; "no reload succeeds")]
    #[test_case(vec!["foo=true".to_string()], true, true; "reload succeeds")]
    #[test_case(vec!["foo=42".to_string()], false, false; "wrong type fails")]
    #[test_case(vec!["bar=42".to_string()], false, true; "structured config override succeeds")]
    #[fuchsia::test]
    async fn config_set(key_values: Vec<String>, reload: bool, succeeds: bool) {
        let (lifecycle_controller, config_override, realm_query) = setup();
        let writer = Vec::new();
        assert_eq!(
            config_set_cmd(
                "my_foo".to_string(),
                key_values,
                reload,
                lifecycle_controller,
                realm_query,
                config_override,
                writer,
            )
            .await
            .is_ok(),
            succeeds
        );
    }

    #[test_case(Some("my_foo".to_string()), false, true; "no reload succeeds")]
    #[test_case(Some("my_foo".to_string()), true, true; "reload succeeds")]
    #[test_case(Some("my_bar".to_string()), false, false; "unknown query fails")]
    #[test_case(None, false, true; "empty moniker succeeds")]
    #[fuchsia::test]
    async fn config_unset(query: Option<String>, reload: bool, succeeds: bool) {
        let (lifecycle_controller, config_override, realm_query) = setup();
        let writer = Vec::new();
        assert_eq!(
            config_unset_cmd(
                query,
                reload,
                lifecycle_controller,
                realm_query,
                config_override,
                writer,
            )
            .await
            .is_ok(),
            succeeds
        );
    }

    #[test_case("true", ConfigValueType::Bool, fdecl::ConfigValue::Single(fdecl::ConfigSingleValue::Bool(true)); "single bool")]
    #[test_case("42", ConfigValueType::Uint8, fdecl::ConfigValue::Single(fdecl::ConfigSingleValue::Uint8(42)); "single u8")]
    #[test_case("42", ConfigValueType::Uint16, fdecl::ConfigValue::Single(fdecl::ConfigSingleValue::Uint16(42)); "single u16")]
    #[test_case("42", ConfigValueType::Uint32, fdecl::ConfigValue::Single(fdecl::ConfigSingleValue::Uint32(42)); "single u32")]
    #[test_case("42", ConfigValueType::Uint64, fdecl::ConfigValue::Single(fdecl::ConfigSingleValue::Uint64(42)); "single u64")]
    #[test_case("42", ConfigValueType::Int8, fdecl::ConfigValue::Single(fdecl::ConfigSingleValue::Int8(42)); "single i8")]
    #[test_case("42", ConfigValueType::Int16, fdecl::ConfigValue::Single(fdecl::ConfigSingleValue::Int16(42)); "single i16")]
    #[test_case("42", ConfigValueType::Int32, fdecl::ConfigValue::Single(fdecl::ConfigSingleValue::Int32(42)); "single i32")]
    #[test_case("42", ConfigValueType::Int64, fdecl::ConfigValue::Single(fdecl::ConfigSingleValue::Int64(42)); "single i64")]
    #[test_case("a string", ConfigValueType::String { max_size: 1024}, fdecl::ConfigValue::Single(fdecl::ConfigSingleValue::String(String::from("a string"))); "single String")]
    #[test_case("true, false", ConfigValueType::Vector { nested_type: ConfigNestedValueType::Bool, max_count: 32 }, fdecl::ConfigValue::Vector(fdecl::ConfigVectorValue::BoolVector(vec![true, false])); "bool vector")]
    #[test_case("42, 24", ConfigValueType::Vector { nested_type: ConfigNestedValueType::Uint8, max_count: 32 }, fdecl::ConfigValue::Vector(fdecl::ConfigVectorValue::Uint8Vector(vec![42, 24])); "u8 vector")]
    #[test_case("42, 24", ConfigValueType::Vector { nested_type: ConfigNestedValueType::Uint16, max_count: 32 }, fdecl::ConfigValue::Vector(fdecl::ConfigVectorValue::Uint16Vector(vec![42, 24])); "u16 vector")]
    #[test_case("42, 24", ConfigValueType::Vector { nested_type: ConfigNestedValueType::Uint32, max_count: 32 }, fdecl::ConfigValue::Vector(fdecl::ConfigVectorValue::Uint32Vector(vec![42, 24])); "u32 vector")]
    #[test_case("42, 24", ConfigValueType::Vector { nested_type: ConfigNestedValueType::Uint64, max_count: 32 }, fdecl::ConfigValue::Vector(fdecl::ConfigVectorValue::Uint64Vector(vec![42, 24])); "u64 vector")]
    #[test_case("42, 24", ConfigValueType::Vector { nested_type: ConfigNestedValueType::Int8, max_count: 32 }, fdecl::ConfigValue::Vector(fdecl::ConfigVectorValue::Int8Vector(vec![42, 24])); "i8 vector")]
    #[test_case("42, 24", ConfigValueType::Vector { nested_type: ConfigNestedValueType::Int16, max_count: 32 }, fdecl::ConfigValue::Vector(fdecl::ConfigVectorValue::Int16Vector(vec![42, 24])); "i16 vector")]
    #[test_case("42, 24", ConfigValueType::Vector { nested_type: ConfigNestedValueType::Int32, max_count: 32 }, fdecl::ConfigValue::Vector(fdecl::ConfigVectorValue::Int32Vector(vec![42, 24])); "i32 vector")]
    #[test_case("42, 24", ConfigValueType::Vector { nested_type: ConfigNestedValueType::Int64, max_count: 32 }, fdecl::ConfigValue::Vector(fdecl::ConfigVectorValue::Int64Vector(vec![42, 24])); "i64 vector")]
    #[test_case("person, camera, tv", ConfigValueType::Vector { nested_type: ConfigNestedValueType::String { max_size: 1024 }, max_count: 32 }, fdecl::ConfigValue::Vector(fdecl::ConfigVectorValue::StringVector(vec![String::from("person"), String::from("camera"), String::from("tv")])); "string vector")]
    #[fuchsia::test]
    fn test_parse_config_value(
        value: &str,
        config_type: ConfigValueType,
        expected: fdecl::ConfigValue,
    ) {
        let cv = parse_config_value(value, config_type).unwrap();
        assert_eq!(cv, expected);
    }
}
