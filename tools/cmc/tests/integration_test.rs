// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use difference::Changeset;
use fidl::unpersist;
use fidl_fuchsia_component_decl::*;
use fidl_fuchsia_data as fdata;
use fidl_fuchsia_io as fio;
use std::fmt::Debug;
use std::fs::File;
use std::io::Read;
use std::path::PathBuf;

#[track_caller]
fn fancy_assert_eq<T>(actual: &T, expected: &T)
where
    T: Debug + PartialEq,
{
    assert_eq!(
        actual,
        expected,
        "Apply the following changes to expectation object: {}",
        Changeset::new(&format!("{:#?}", expected), &format!("{:#?}", actual), "\n")
    );
}

#[fuchsia::test]
fn example_cml_integration_test() {
    // Load the compiled version of //tools/cmc/meta/example.cml
    let mut cm_decl = read_cm("/pkg/meta/example.cm").expect("could not read cm file");

    // profile variant injects this protocol.
    if let Some(uses) = &mut cm_decl.uses {
        uses.retain(|u| match u {
            Use::Protocol(decl) => {
                if decl.source_name == Some("fuchsia.debugdata.Publisher".to_owned()) {
                    let protocol = decl.source_name.clone().unwrap();
                    let target_path = format!("/svc/{}", protocol);
                    fancy_assert_eq(
                        decl,
                        &UseProtocol {
                            dependency_type: Some(DependencyType::Strong),
                            source: Some(Ref::Debug(DebugRef {})),
                            source_name: Some(protocol),
                            target_path: Some(target_path),
                            availability: Some(Availability::Transitional),
                            ..Default::default()
                        },
                    );
                    return false;
                }
                return true;
            }
            _ => true,
        })
    }

    let program = Program {
        runner: Some("elf".to_string()),
        info: Some(fdata::Dictionary {
            entries: Some(vec![
                fdata::DictionaryEntry {
                    key: "binary".to_string(),
                    value: Some(Box::new(fdata::DictionaryValue::Str("bin/example".to_string()))),
                },
                fdata::DictionaryEntry {
                    key: "lifecycle.stop_event".to_string(),
                    value: Some(Box::new(fdata::DictionaryValue::Str("notify".to_string()))),
                },
            ]),
            ..Default::default()
        }),
        ..Default::default()
    };
    fancy_assert_eq(&cm_decl.program.as_ref(), &Some(&program));

    let uses = vec![
        Use::EventStream(UseEventStream {
            source_name: Some("events".to_string()),
            source: Some(Ref::Parent(ParentRef {})),
            target_path: Some("/testdir/my_stream".to_string()),
            scope: Some(vec![Ref::Child(ChildRef {
                collection: None,
                name: "logger".to_string(),
            })]),
            availability: Some(Availability::Required),
            ..Default::default()
        }),
        Use::EventStream(UseEventStream {
            source_name: Some("other".to_string()),
            source: Some(Ref::Parent(ParentRef {})),
            target_path: Some("/testdir/my_stream".to_string()),
            scope: Some(vec![Ref::Child(ChildRef {
                collection: None,
                name: "logger".to_string(),
            })]),
            availability: Some(Availability::Required),
            ..Default::default()
        }),
        Use::EventStream(UseEventStream {
            source_name: Some("some".to_string()),
            source: Some(Ref::Parent(ParentRef {})),
            target_path: Some("/testdir/my_stream".to_string()),
            scope: Some(vec![Ref::Child(ChildRef {
                collection: None,
                name: "logger".to_string(),
            })]),
            availability: Some(Availability::Required),
            ..Default::default()
        }),
        Use::EventStream(UseEventStream {
            source_name: Some("filtered".to_string()),
            source: Some(Ref::Parent(ParentRef {})),
            target_path: Some("/svc/fuchsia.component.EventStream".to_string()),
            availability: Some(Availability::Required),
            ..Default::default()
        }),
        Use::Protocol(UseProtocol {
            dependency_type: Some(DependencyType::Strong),
            source: Some(Ref::Parent(ParentRef {})),
            source_name: Some("fuchsia.fonts.LegacyProvider".to_string()),
            target_path: Some("/svc/fuchsia.fonts.OldProvider".to_string()),
            availability: Some(Availability::Optional),
            ..Default::default()
        }),
        Use::Protocol(UseProtocol {
            dependency_type: Some(DependencyType::Strong),
            source: Some(Ref::Debug(DebugRef {})),
            source_name: Some("fuchsia.log.LegacyLog".to_string()),
            target_path: Some("/svc/fuchsia.log.LegacyLog".to_string()),
            availability: Some(Availability::Required),
            ..Default::default()
        }),
        Use::Protocol(UseProtocol {
            dependency_type: Some(DependencyType::Strong),
            source: Some(Ref::Parent(ParentRef {})),
            source_name: Some("fuchsia.logger.LogSink".to_string()),
            target_path: Some("/svc/fuchsia.logger.LogSink".to_string()),
            source_dictionary: Some("diagnostics".to_string()),
            availability: Some(Availability::Required),
            ..Default::default()
        }),
        Use::Protocol(UseProtocol {
            dependency_type: Some(DependencyType::Strong),
            source: Some(Ref::Parent(ParentRef {})),
            source_name: Some("fuchsia.logger.LogSink".to_string()),
            numbered_handle: Some(0x3D),
            source_dictionary: Some("diagnostics".to_string()),
            availability: Some(Availability::Required),
            ..Default::default()
        }),
        Use::Service(UseService {
            dependency_type: Some(DependencyType::Strong),
            source: Some(Ref::Parent(ParentRef {})),
            source_name: Some("fuchsia.fonts.Provider".to_string()),
            target_path: Some("/svc/fuchsia.fonts.Provider".to_string()),
            availability: Some(Availability::Required),
            ..Default::default()
        }),
    ];
    fancy_assert_eq(&cm_decl.uses.as_ref(), &Some(&uses));

    let exposes = vec![
        Expose::Directory(ExposeDirectory {
            source: Some(Ref::Self_(SelfRef {})),
            source_name: Some("blobfs".to_string()),
            target_name: Some("blobfs".to_string()),
            target: Some(Ref::Parent(ParentRef {})),
            rights: None,
            subdir: Some("blob".to_string()),
            availability: Some(Availability::Required),
            ..Default::default()
        }),
        Expose::Directory(ExposeDirectory {
            source: Some(Ref::Self_(SelfRef)),
            source_name: Some("blobfs".to_string()),
            target: Some(Ref::Parent(ParentRef)),
            target_name: Some("optional_dir".to_string()),
            rights: None,
            subdir: Some("blob".to_string()),
            availability: Some(Availability::Optional),
            ..Default::default()
        }),
        Expose::Protocol(ExposeProtocol {
            source: Some(Ref::Child(ChildRef { name: "logger".to_string(), collection: None })),
            source_name: Some("fuchsia.logger.LegacyLog".to_string()),
            target_name: Some("fuchsia.logger.OldLog".to_string()),
            target: Some(Ref::Parent(ParentRef {})),
            availability: Some(Availability::Required),
            ..Default::default()
        }),
        Expose::Protocol(ExposeProtocol {
            source: Some(Ref::Child(ChildRef { name: "logger".to_string(), collection: None })),
            source_name: Some("fuchsia.logger.LegacyLog".to_string()),
            target: Some(Ref::Parent(ParentRef)),
            target_name: Some("fuchsia.logger.OldLog_same_as_target".to_string()),
            availability: Some(Availability::SameAsTarget),
            ..Default::default()
        }),
        Expose::Protocol(ExposeProtocol {
            source: Some(Ref::VoidType(VoidRef)),
            source_name: Some("fuchsia.logger.LegacyLog".to_string()),
            target: Some(Ref::Parent(ParentRef)),
            target_name: Some("fuchsia.logger.OldLog_void".to_string()),
            availability: Some(Availability::Optional),
            ..Default::default()
        }),
        Expose::Protocol(ExposeProtocol {
            source: Some(Ref::VoidType(VoidRef)),
            source_name: Some("fuchsia.logger.LegacyLog".to_string()),
            target: Some(Ref::Parent(ParentRef)),
            target_name: Some("fuchsia.logger.OldLog_absent_child".to_string()),
            availability: Some(Availability::Optional),
            ..Default::default()
        }),
        Expose::Service(ExposeService {
            source: Some(Ref::Child(ChildRef { name: "logger".to_string(), collection: None })),
            source_name: Some("fuchsia.logger.Log".to_string()),
            target_name: Some("fuchsia.logger.Log".to_string()),
            target: Some(Ref::Parent(ParentRef {})),
            availability: Some(Availability::Required),
            ..Default::default()
        }),
    ];
    fancy_assert_eq(&cm_decl.exposes.as_ref(), &Some(&exposes));

    let offers = vec![
        Offer::EventStream(OfferEventStream {
            source_name: Some("started".to_string()),
            source: Some(Ref::Parent(ParentRef {})),
            target: Some(Ref::Child(ChildRef { name: "logger".to_string(), collection: None })),
            scope: Some(vec![Ref::Child(ChildRef {
                name: "logger".to_string(),
                collection: None,
            })]),
            target_name: Some("started".to_string()),
            availability: Some(Availability::Required),
            ..Default::default()
        }),
        Offer::EventStream(OfferEventStream {
            source_name: Some("stopped".to_string()),
            source: Some(Ref::Parent(ParentRef {})),
            target: Some(Ref::Child(ChildRef { name: "logger".to_string(), collection: None })),
            scope: Some(vec![Ref::Child(ChildRef {
                name: "logger".to_string(),
                collection: None,
            })]),
            target_name: Some("stopped".to_string()),
            availability: Some(Availability::Required),
            ..Default::default()
        }),
        Offer::Protocol(OfferProtocol {
            source: Some(Ref::Child(ChildRef { name: "logger".to_string(), collection: None })),
            source_name: Some("fuchsia.logger.LegacyLog".to_string()),
            target: Some(Ref::Collection(CollectionRef { name: "modular".to_string() })),
            target_name: Some("fuchsia.logger.OldLog".to_string()),
            dependency_type: Some(DependencyType::Strong),
            availability: Some(Availability::Required),
            ..Default::default()
        }),
        Offer::Protocol(OfferProtocol {
            source: Some(Ref::VoidType(VoidRef {})),
            source_name: Some("fuchsia.logger.LegacyLog2".to_string()),
            target: Some(Ref::Collection(CollectionRef { name: "modular".to_string() })),
            target_name: Some("fuchsia.logger.OldLog2".to_string()),
            dependency_type: Some(DependencyType::Strong),
            availability: Some(Availability::Optional),
            ..Default::default()
        }),
        Offer::Protocol(OfferProtocol {
            source: Some(Ref::Child(ChildRef { name: "logger".to_string(), collection: None })),
            source_name: Some("fuchsia.logger.LegacyLog3".to_string()),
            target: Some(Ref::Collection(CollectionRef { name: "modular".to_string() })),
            target_name: Some("fuchsia.logger.OldLog3".to_string()),
            dependency_type: Some(DependencyType::Strong),
            availability: Some(Availability::Required),
            ..Default::default()
        }),
        Offer::Protocol(OfferProtocol {
            source: Some(Ref::Parent(ParentRef {})),
            source_name: Some("fuchsia.logger.LegacyLog4".to_string()),
            target: Some(Ref::Collection(CollectionRef { name: "modular".to_string() })),
            target_name: Some("fuchsia.logger.OldLog4".to_string()),
            dependency_type: Some(DependencyType::Strong),
            availability: Some(Availability::Optional),
            ..Default::default()
        }),
        Offer::Service(OfferService {
            source: Some(Ref::Child(ChildRef { name: "logger".to_string(), collection: None })),
            source_name: Some("fuchsia.logger.Log".to_string()),
            target: Some(Ref::Collection(CollectionRef { name: "modular".to_string() })),
            target_name: Some("fuchsia.logger.Log".to_string()),
            availability: Some(Availability::Required),
            dependency_type: Some(DependencyType::Strong),
            ..Default::default()
        }),
        Offer::Dictionary(OfferDictionary {
            source: Some(Ref::Parent(ParentRef {})),
            source_name: Some("diagnostics".to_string()),
            target: Some(Ref::Child(ChildRef { name: "logger".to_string(), collection: None })),
            target_name: Some("diagnostics".to_string()),
            dependency_type: Some(DependencyType::Strong),
            availability: Some(Availability::Required),
            source_dictionary: None,
            ..Default::default()
        }),
        Offer::Dictionary(OfferDictionary {
            source: Some(Ref::Parent(ParentRef)),
            source_name: Some("diagnostics".to_string()),
            target: Some(Ref::Collection(CollectionRef { name: "explicit_dynamic".to_string() })),
            target_name: Some("diagnostics".to_string()),
            dependency_type: Some(DependencyType::Strong),
            availability: Some(Availability::Required),
            source_dictionary: None,
            ..Default::default()
        }),
        Offer::Dictionary(OfferDictionary {
            source: Some(Ref::Parent(ParentRef {})),
            source_name: Some("diagnostics".to_string()),
            target: Some(Ref::Collection(CollectionRef { name: "explicit_static".to_string() })),
            target_name: Some("diagnostics".to_string()),
            dependency_type: Some(DependencyType::Strong),
            availability: Some(Availability::Required),
            source_dictionary: None,
            ..Default::default()
        }),
        Offer::Dictionary(OfferDictionary {
            source: Some(Ref::Parent(ParentRef {})),
            source_name: Some("diagnostics".to_string()),
            target: Some(Ref::Collection(CollectionRef { name: "long_child_names".to_string() })),
            target_name: Some("diagnostics".to_string()),
            dependency_type: Some(DependencyType::Strong),
            availability: Some(Availability::Required),
            source_dictionary: None,
            ..Default::default()
        }),
        Offer::Dictionary(OfferDictionary {
            source: Some(Ref::Parent(ParentRef {})),
            source_name: Some("diagnostics".to_string()),
            target: Some(Ref::Collection(CollectionRef { name: "modular".to_string() })),
            target_name: Some("diagnostics".to_string()),
            dependency_type: Some(DependencyType::Strong),
            availability: Some(Availability::Required),
            source_dictionary: None,
            ..Default::default()
        }),
    ];
    fancy_assert_eq(&cm_decl.offers.as_ref(), &Some(&offers));

    let capabilities = vec![
        Capability::Directory(Directory {
            name: Some("blobfs".to_string()),
            source_path: Some("/volumes/blobfs".to_string()),
            rights: Some(
                fio::Operations::CONNECT
                    | fio::Operations::READ_BYTES
                    | fio::Operations::WRITE_BYTES
                    | fio::Operations::GET_ATTRIBUTES
                    | fio::Operations::UPDATE_ATTRIBUTES
                    | fio::Operations::ENUMERATE
                    | fio::Operations::TRAVERSE
                    | fio::Operations::MODIFY_DIRECTORY,
            ),
            ..Default::default()
        }),
        Capability::Protocol(Protocol {
            name: Some("fuchsia.logger.Log2".to_string()),
            source_path: Some("/svc/fuchsia.logger.Log2".to_string()),
            ..Default::default()
        }),
        Capability::Resolver(Resolver {
            name: Some("pkg_resolver".to_string()),
            source_path: Some("/svc/fuchsia.pkg.Resolver".to_string()),
            ..Default::default()
        }),
        Capability::Runner(Runner {
            name: Some("dart_runner".to_string()),
            source_path: Some("/svc/fuchsia.sys2.Runner".to_string()),
            ..Default::default()
        }),
        Capability::Service(Service {
            name: Some("fuchsia.logger.Log".to_string()),
            source_path: Some("/svc/fuchsia.logger.Log".to_string()),
            ..Default::default()
        }),
        Capability::Storage(Storage {
            name: Some("minfs".to_string()),
            source: Some(Ref::Parent(ParentRef {})),
            backing_dir: Some("data".to_string()),
            subdir: None,
            storage_id: Some(StorageId::StaticInstanceIdOrMoniker),
            ..Default::default()
        }),
    ];
    fancy_assert_eq(&cm_decl.capabilities.as_ref(), &Some(&capabilities));

    let children = vec![Child {
        name: Some("logger".to_string()),
        url: Some("fuchsia-pkg://fuchsia.com/logger/stable#meta/logger.cm".to_string()),
        startup: Some(StartupMode::Lazy),
        environment: Some("env_one".to_string()),
        ..Default::default()
    }];
    fancy_assert_eq(&cm_decl.children.as_ref(), &Some(&children));

    let collections = vec![
        Collection {
            name: Some("explicit_dynamic".to_string()),
            durability: Some(Durability::Transient),
            environment: None,
            allowed_offers: Some(AllowedOffers::StaticAndDynamic),
            allow_long_names: None,
            persistent_storage: None,
            ..Default::default()
        },
        Collection {
            name: Some("explicit_static".to_string()),
            durability: Some(Durability::Transient),
            environment: None,
            allowed_offers: Some(AllowedOffers::StaticOnly),
            allow_long_names: None,
            persistent_storage: None,
            ..Default::default()
        },
        Collection {
            name: Some("long_child_names".to_string()),
            durability: Some(Durability::Transient),
            environment: None,
            allowed_offers: None,
            allow_long_names: Some(true),
            persistent_storage: None,
            ..Default::default()
        },
        Collection {
            name: Some("modular".to_string()),
            durability: Some(Durability::Transient),
            environment: None,
            allowed_offers: None,
            allow_long_names: None,
            persistent_storage: None,
            ..Default::default()
        },
    ];
    fancy_assert_eq(&cm_decl.collections.as_ref(), &Some(&collections));

    let facets = fdata::Dictionary {
        entries: Some(vec![
            fdata::DictionaryEntry {
                key: "author".to_string(),
                value: Some(Box::new(fdata::DictionaryValue::Str("Fuchsia".to_string()))),
            },
            fdata::DictionaryEntry {
                key: "metadata.publisher".to_string(),
                value: Some(Box::new(fdata::DictionaryValue::Str(
                    "The Books Publisher".to_string(),
                ))),
            },
            fdata::DictionaryEntry {
                key: "year".to_string(),
                value: Some(Box::new(fdata::DictionaryValue::Str("2018".to_string()))),
            },
        ]),
        ..Default::default()
    };
    fancy_assert_eq(&cm_decl.facets.as_ref(), &Some(&facets));

    let envs = vec![
        Environment {
            name: Some("env_one".to_string()),
            extends: Some(EnvironmentExtends::None),
            stop_timeout_ms: Some(1337),
            runners: None,
            resolvers: None,
            debug_capabilities: None,
            ..Default::default()
        },
        Environment {
            name: Some("env_two".to_string()),
            extends: Some(EnvironmentExtends::Realm),
            stop_timeout_ms: None,
            runners: None,
            resolvers: None,
            debug_capabilities: Some(vec![
                DebugRegistration::Protocol(DebugProtocolRegistration {
                    source_name: Some("fuchsia.logger.LegacyLog".to_string()),
                    source: Some(Ref::Child(ChildRef {
                        name: "logger".to_string(),
                        collection: None,
                    })),
                    target_name: Some("fuchsia.logger.LegacyLog".to_string()),
                    ..Default::default()
                }),
                DebugRegistration::Protocol(DebugProtocolRegistration {
                    source_name: Some("fuchsia.logger.OtherLog".to_string()),
                    source: Some(Ref::Parent(ParentRef {})),
                    target_name: Some("fuchsia.logger.OtherLog".to_string()),
                    ..Default::default()
                }),
                DebugRegistration::Protocol(DebugProtocolRegistration {
                    source_name: Some("fuchsia.logger.Log2".to_string()),
                    source: Some(Ref::Self_(SelfRef {})),
                    target_name: Some("fuchsia.logger.Log2".to_string()),
                    ..Default::default()
                }),
            ]),
            ..Default::default()
        },
    ];
    fancy_assert_eq(&cm_decl.environments.as_ref(), &Some(&envs));

    let config = ConfigSchema {
        fields: Some(vec![
            ConfigField {
                key: Some("my_flag".to_string()),
                type_: Some(ConfigType {
                    layout: ConfigTypeLayout::Bool,
                    parameters: Some(vec![]),
                    constraints: vec![],
                }),
                mutability: Some(ConfigMutability::empty()),
                ..Default::default()
            },
            ConfigField {
                key: Some("my_string".to_string()),
                type_: Some(ConfigType {
                    layout: ConfigTypeLayout::String,
                    constraints: vec![LayoutConstraint::MaxSize(100)],
                    parameters: Some(vec![]),
                }),
                mutability: Some(ConfigMutability::empty()),
                ..Default::default()
            },
            ConfigField {
                key: Some("my_uint8".to_string()),
                type_: Some(ConfigType {
                    layout: ConfigTypeLayout::Uint8,
                    parameters: Some(vec![]),
                    constraints: vec![],
                }),
                mutability: Some(ConfigMutability::empty()),
                ..Default::default()
            },
            ConfigField {
                key: Some("my_vector_of_string".to_string()),
                type_: Some(ConfigType {
                    layout: ConfigTypeLayout::Vector,
                    constraints: vec![LayoutConstraint::MaxSize(100)],
                    parameters: Some(vec![LayoutParameter::NestedType(ConfigType {
                        layout: ConfigTypeLayout::String,
                        constraints: vec![LayoutConstraint::MaxSize(50)],
                        parameters: Some(vec![]),
                    })]),
                }),
                mutability: Some(ConfigMutability::empty()),
                ..Default::default()
            },
        ]),
        checksum: Some(ConfigChecksum::Sha256([
            227, 126, 183, 151, 1, 15, 67, 110, 144, 49, 222, 176, 221, 111, 19, 165, 34, 204, 240,
            41, 165, 95, 117, 57, 203, 42, 186, 167, 84, 26, 25, 231,
        ])),
        value_source: Some(ConfigValueSource::PackagePath("meta/example.cvf".to_string())),
        ..Default::default()
    };
    fancy_assert_eq(&cm_decl.config.as_ref(), &Some(&config));

    // Coverage bots apply a merge to the source.
    if let Some(di) = &mut cm_decl.debug_info {
        if let Some(ms) = &mut di.manifest_sources {
            if let Some(entry) = ms.get_mut(0) {
                *entry = entry.replace(
                    "obj/tools/cmc/example_component_manifest_merge_merge.cml",
                    "tools/cmc/meta/example.cml",
                );
            }
        }
    }

    // Assert equality of any missing fields.
    let expected_decl = Component {
        program: Some(program),
        uses: Some(uses),
        exposes: Some(exposes),
        offers: Some(offers),
        capabilities: Some(capabilities),
        children: Some(children),
        collections: Some(collections),
        facets: Some(facets),
        environments: Some(envs),
        config: Some(config),
        debug_info: Some(DebugInfo {
            manifest_sources: Some(vec![
                "tools/cmc/meta/example.cml".to_string(),
                "sdk/lib/inspect/offer.shard.cml".to_string(),
                "sdk/lib/sys/testing/coverage.shard.cml".to_string(),
                "sdk/lib/syslog/client.shard.cml".to_string(),
                "sdk/lib/syslog/offer.shard.cml".to_string(),
                "sdk/lib/syslog/use.shard.cml".to_string(),
            ]),
            ..Default::default()
        }),
        ..Default::default()
    };
    fancy_assert_eq(&cm_decl, &expected_decl);
}

fn read_cm(file: &str) -> Result<Component, Error> {
    let mut buffer = Vec::new();
    let path = PathBuf::from(file);
    File::open(&path)?.read_to_end(&mut buffer)?;
    Ok(unpersist(&buffer)?)
}
