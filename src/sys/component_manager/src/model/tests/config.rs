// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod tests {

    use crate::model::component::StartReason;
    use crate::model::testing::routing_test_helpers::*;
    use cm_rust::*;
    use cm_rust_testing::*;
    use moniker::Moniker;
    use std::sync::Arc;

    async fn start_component_get_config(
        test: &RoutingTest,
        component_moniker: &str,
    ) -> Arc<config_encoder::ConfigFields> {
        let test_moniker = Moniker::parse_str(component_moniker).unwrap();
        let root =
            test.start_and_get_instance(&test_moniker, StartReason::Eager, true).await.unwrap().0;
        let resolved_state = root.lock_resolved_state().await.unwrap();
        resolved_state.resolved_component.config.as_ref().unwrap().clone()
    }

    #[fuchsia::test]
    async fn capability_overrides_resolver() {
        let good_value = cm_rust::ConfigSingleValue::Int8(12);
        let package_value = cm_rust::ConfigSingleValue::Int8(5);
        let components = vec![
            ("root", ComponentDeclBuilder::new().child_default("child").build()),
            (
                "child",
                ComponentDeclBuilder::new()
                    .capability(
                        CapabilityBuilder::config()
                            .name("fuchsia.MyConfig")
                            .value(good_value.clone().into()),
                    )
                    .use_(
                        UseBuilder::config()
                            .source(cm_rust::UseSource::Self_)
                            .name("fuchsia.MyConfig")
                            .target_name("my_config")
                            .config_type(cm_rust::ConfigValueType::Int8),
                    )
                    .config(cm_rust::ConfigDecl {
                        fields: Box::from([cm_rust::ConfigField {
                            key: "my_config".into(),
                            type_: cm_rust::ConfigValueType::Int8,
                            mutability: Default::default(),
                        }]),
                        checksum: cm_rust::ConfigChecksum::Sha256([0; 32]),
                        value_source: cm_rust::ConfigValueSource::PackagePath("myfile".into()),
                    })
                    .build(),
            ),
        ];

        let test = RoutingTestBuilder::new("root", components)
            .add_config(
                "myfile",
                ConfigValuesData {
                    values: Box::from([cm_rust::ConfigValueSpec { value: package_value.into() }]),
                    checksum: cm_rust::ConfigChecksum::Sha256([0; 32]),
                },
            )
            .build()
            .await;
        let config = start_component_get_config(&test, "/child").await;
        assert_eq!(
            config.fields,
            vec![config_encoder::ConfigField {
                key: "my_config".into(),
                value: good_value.into(),
                mutability: Default::default(),
            }]
        );
    }

    #[fuchsia::test]
    async fn config_from_resolver() {
        let package_value = cm_rust::ConfigSingleValue::Int8(5);
        let components = vec![
            ("root", ComponentDeclBuilder::new().child_default("child").build()),
            (
                "child",
                ComponentDeclBuilder::new()
                    .config(cm_rust::ConfigDecl {
                        fields: Box::from([cm_rust::ConfigField {
                            key: "my_config".into(),
                            type_: cm_rust::ConfigValueType::Int8,
                            mutability: Default::default(),
                        }]),
                        checksum: cm_rust::ConfigChecksum::Sha256([0; 32]),
                        value_source: cm_rust::ConfigValueSource::PackagePath("myfile".into()),
                    })
                    .build(),
            ),
        ];

        let test = RoutingTestBuilder::new("root", components)
            .add_config(
                "myfile",
                ConfigValuesData {
                    values: Box::from([cm_rust::ConfigValueSpec {
                        value: package_value.clone().into(),
                    }]),
                    checksum: cm_rust::ConfigChecksum::Sha256([0; 32]),
                },
            )
            .build()
            .await;
        let config = start_component_get_config(&test, "/child").await;
        assert_eq!(
            config.fields,
            vec![config_encoder::ConfigField {
                key: "my_config".into(),
                value: package_value.into(),
                mutability: Default::default(),
            }]
        );
    }

    #[fuchsia::test]
    async fn config_from_resolver_and_capability() {
        let cap_value = cm_rust::ConfigSingleValue::Int8(12);
        let package_value = cm_rust::ConfigSingleValue::Int8(5);
        let components = vec![
            ("root", ComponentDeclBuilder::new().child_default("child").build()),
            (
                "child",
                ComponentDeclBuilder::new()
                    .capability(
                        CapabilityBuilder::config()
                            .name("fuchsia.MyConfig")
                            .value(cap_value.clone().into()),
                    )
                    .use_(
                        UseBuilder::config()
                            .source(cm_rust::UseSource::Self_)
                            .name("fuchsia.MyConfig")
                            .target_name("from_cap")
                            .config_type(cm_rust::ConfigValueType::Int8),
                    )
                    .config(cm_rust::ConfigDecl {
                        fields: Box::from([
                            cm_rust::ConfigField {
                                key: "from_resolver".into(),
                                type_: cm_rust::ConfigValueType::Int8,
                                mutability: Default::default(),
                            },
                            cm_rust::ConfigField {
                                key: "from_cap".into(),
                                type_: cm_rust::ConfigValueType::Int8,
                                mutability: Default::default(),
                            },
                        ]),
                        checksum: cm_rust::ConfigChecksum::Sha256([0; 32]),
                        value_source: cm_rust::ConfigValueSource::PackagePath("myfile".into()),
                    })
                    .build(),
            ),
        ];

        let test = RoutingTestBuilder::new("root", components)
            .add_config(
                "myfile",
                ConfigValuesData {
                    values: Box::from([
                        cm_rust::ConfigValueSpec { value: package_value.clone().into() },
                        cm_rust::ConfigValueSpec { value: package_value.clone().into() },
                    ]),
                    checksum: cm_rust::ConfigChecksum::Sha256([0; 32]),
                },
            )
            .build()
            .await;
        let config = start_component_get_config(&test, "/child").await;
        assert_eq!(
            &*config.fields,
            &[
                config_encoder::ConfigField {
                    key: "from_resolver".into(),
                    value: package_value.into(),
                    mutability: Default::default(),
                },
                config_encoder::ConfigField {
                    key: "from_cap".into(),
                    value: cap_value.into(),
                    mutability: Default::default(),
                }
            ]
        );
    }
}
