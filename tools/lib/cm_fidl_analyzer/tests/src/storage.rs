// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod tests {
    use crate::routing::RoutingTestBuilderForAnalyzer;
    use capability_source::{CapabilitySource, ComponentCapability, ComponentSource};
    use cm_fidl_analyzer::route::TargetDecl;
    use cm_rust::{CapabilityDecl, CapabilityTypeName, OfferSource, StorageDirectorySource};
    use cm_rust_testing::*;
    use component_id_index::{IndexEntry, InstanceId};
    use fidl_fuchsia_component_decl as fdecl;
    use fidl_fuchsia_io as fio;
    use moniker::Moniker;
    use routing::component_instance::ComponentInstanceInterface;
    use routing_test_helpers::component_id_index::make_index_file;
    use routing_test_helpers::storage::CommonStorageTest;
    use routing_test_helpers::{
        CheckUse, ExpectedResult, RoutingTestModel, RoutingTestModelBuilder,
    };
    use std::collections::HashSet;
    use zx_status;

    #[fuchsia::test]
    async fn storage_dir_from_cm_namespace() {
        CommonStorageTest::<RoutingTestBuilderForAnalyzer>::new()
            .test_storage_dir_from_cm_namespace()
            .await
    }

    #[fuchsia::test]
    async fn storage_and_dir_from_parent() {
        CommonStorageTest::<RoutingTestBuilderForAnalyzer>::new()
            .test_storage_and_dir_from_parent()
            .await
    }

    #[fuchsia::test]
    async fn storage_and_dir_from_parent_with_subdir() {
        CommonStorageTest::<RoutingTestBuilderForAnalyzer>::new()
            .test_storage_and_dir_from_parent_with_subdir()
            .await
    }

    #[fuchsia::test]
    async fn storage_and_dir_from_parent_rights_invalid() {
        CommonStorageTest::<RoutingTestBuilderForAnalyzer>::new()
            .test_storage_and_dir_from_parent_rights_invalid()
            .await
    }

    #[fuchsia::test]
    async fn storage_from_parent_dir_from_grandparent() {
        CommonStorageTest::<RoutingTestBuilderForAnalyzer>::new()
            .test_storage_from_parent_dir_from_grandparent()
            .await
    }

    #[fuchsia::test]
    async fn storage_from_parent_dir_from_grandparent_with_subdirs() {
        CommonStorageTest::<RoutingTestBuilderForAnalyzer>::new()
            .test_storage_from_parent_dir_from_grandparent_with_subdirs()
            .await
    }

    #[fuchsia::test]
    async fn storage_from_parent_dir_from_grandparent_with_subdir() {
        CommonStorageTest::<RoutingTestBuilderForAnalyzer>::new()
            .test_storage_from_parent_dir_from_grandparent_with_subdir()
            .await
    }

    #[fuchsia::test]
    async fn storage_and_dir_from_grandparent() {
        CommonStorageTest::<RoutingTestBuilderForAnalyzer>::new()
            .test_storage_and_dir_from_grandparent()
            .await
    }

    #[fuchsia::test]
    async fn storage_from_parent_dir_from_sibling() {
        CommonStorageTest::<RoutingTestBuilderForAnalyzer>::new()
            .test_storage_from_parent_dir_from_sibling()
            .await
    }

    #[fuchsia::test]
    async fn storage_from_parent_dir_from_sibling_with_subdir() {
        CommonStorageTest::<RoutingTestBuilderForAnalyzer>::new()
            .test_storage_from_parent_dir_from_sibling_with_subdir()
            .await
    }

    #[fuchsia::test]
    async fn storage_multiple_types() {
        CommonStorageTest::<RoutingTestBuilderForAnalyzer>::new()
            .test_storage_multiple_types()
            .await
    }

    #[fuchsia::test]
    async fn use_the_wrong_type_of_storage() {
        CommonStorageTest::<RoutingTestBuilderForAnalyzer>::new()
            .test_use_the_wrong_type_of_storage()
            .await
    }

    #[fuchsia::test]
    async fn directories_are_not_storage() {
        CommonStorageTest::<RoutingTestBuilderForAnalyzer>::new()
            .test_directories_are_not_storage()
            .await
    }

    #[fuchsia::test]
    async fn use_storage_when_not_offered() {
        CommonStorageTest::<RoutingTestBuilderForAnalyzer>::new()
            .test_use_storage_when_not_offered()
            .await
    }

    #[fuchsia::test]
    async fn dir_offered_from_nonexecutable() {
        CommonStorageTest::<RoutingTestBuilderForAnalyzer>::new()
            .test_dir_offered_from_nonexecutable()
            .await
    }

    #[fuchsia::test]
    async fn storage_dir_from_cm_namespace_prevented_by_policy() {
        CommonStorageTest::<RoutingTestBuilderForAnalyzer>::new()
            .test_storage_dir_from_cm_namespace_prevented_by_policy()
            .await
    }

    #[fuchsia::test]
    async fn instance_id_from_index() {
        CommonStorageTest::<RoutingTestBuilderForAnalyzer>::new()
            .test_instance_id_from_index()
            .await
    }

    ///   component manager's namespace
    ///    |
    ///   provider (provides storage capability, restricted to component ID index)
    ///    |
    ///   consumer (not in component ID index)
    ///
    /// Tests that consumer cannot use restricted storage as it isn't in the component ID
    /// index.
    ///
    /// This test only runs for the static model. Component Manager has a similar test that
    /// instead expects failure when a component is started, if that component uses restricted
    /// storage and is not in the component ID index.
    #[fuchsia::test]
    async fn use_restricted_storage_failure() {
        let parent_consumer_instance_id = InstanceId::new_random(&mut rand::rng());
        let index = {
            let mut index = component_id_index::Index::default();
            index
                .insert(component_id_index::IndexEntry {
                    moniker: Moniker::parse_str("parent_consumer").unwrap(),
                    instance_id: parent_consumer_instance_id.clone(),
                    ignore_duplicate_id: false,
                })
                .unwrap();
            index
        };
        let component_id_index_path = make_index_file(index).unwrap();
        let components = vec![
            (
                "provider",
                ComponentDeclBuilder::new()
                    .capability(
                        CapabilityBuilder::directory()
                            .name("data")
                            .path("/data")
                            .rights(fio::RW_STAR_DIR),
                    )
                    .capability(
                        CapabilityBuilder::storage()
                            .name("cache")
                            .backing_dir("data")
                            .source(StorageDirectorySource::Self_)
                            .storage_id(fdecl::StorageId::StaticInstanceId),
                    )
                    .offer(
                        OfferBuilder::storage()
                            .name("cache")
                            .source(OfferSource::Self_)
                            .target(offer_target_static_child("consumer")),
                    )
                    .child_default("consumer")
                    .build(),
            ),
            (
                "consumer",
                ComponentDeclBuilder::new()
                    .use_(UseBuilder::storage().name("cache").path("/storage"))
                    .build(),
            ),
        ];
        let mut builder = RoutingTestBuilderForAnalyzer::new("provider", components);
        builder.set_component_id_index_path(
            component_id_index_path.path().to_owned().try_into().unwrap(),
        );
        let model = builder.build().await;

        model
            .check_use(
                Moniker::parse_str("consumer").unwrap(),
                CheckUse::Storage {
                    path: "/storage".parse().unwrap(),
                    storage_relation: Some(Moniker::try_from(["consumer"]).unwrap()),
                    from_cm_namespace: false,
                    storage_subdir: None,
                    expected_res: ExpectedResult::Err(zx_status::Status::NOT_FOUND),
                },
            )
            .await;
    }

    /// Tests verification of a storage capability route from an unused offer. (Routes from offers
    /// are only verified if the target does not use the offered capability.)
    ///
    ///   directory_provider
    ///    |
    ///   storage_provider
    ///    |
    ///   not_consumer
    ///
    /// `directory_provider` declares a directory capability and offers it to `storage_provider`.
    /// `storage_provider` declares a storage capability with that directory as the backing dir,
    /// and offers the storage capability to `not_consumer`. `not_consumer` does not use the
    /// storage capability.
    ///
    /// Note that since the capability is not used, there is no requirement on the contents of the
    /// component ID index, even though the storage capability uses restricted storage.
    ///
    /// This test only runs for the static model, since Component Manager doesn't route capabilities
    /// from offer declarations.
    #[fuchsia::test]
    async fn route_storage_from_offer() {
        let directory_decl = CapabilityBuilder::directory()
            .name("data")
            .path("/data")
            .rights(fio::RW_STAR_DIR)
            .build();
        let offer_directory_decl = OfferBuilder::directory()
            .name("data")
            .source(OfferSource::Self_)
            .target(offer_target_static_child("storage_provider"))
            .rights(fio::RW_STAR_DIR)
            .build();
        let storage_decl = CapabilityBuilder::storage()
            .name("cache")
            .backing_dir("data")
            .source(StorageDirectorySource::Parent)
            .storage_id(fdecl::StorageId::StaticInstanceId)
            .build();
        let offer_storage_decl = OfferBuilder::storage()
            .name("cache")
            .source(OfferSource::Self_)
            .target(offer_target_static_child("not_consumer"))
            .build();
        let components = vec![
            (
                "directory_provider",
                ComponentDeclBuilder::new()
                    .capability(directory_decl.clone())
                    .offer(offer_directory_decl.clone())
                    .child_default("storage_provider")
                    .build(),
            ),
            (
                "storage_provider",
                ComponentDeclBuilder::new()
                    .capability(storage_decl.clone())
                    .offer(offer_storage_decl.clone())
                    .child_default("not_consumer")
                    .build(),
            ),
            ("not_consumer", ComponentDeclBuilder::new().build()),
        ];
        let storage_provider_instance_id = InstanceId::new_random(&mut rand::rng());
        let index = {
            let mut index = component_id_index::Index::default();
            index
                .insert(IndexEntry {
                    moniker: Moniker::parse_str("storage_provider").unwrap(),
                    instance_id: storage_provider_instance_id.clone(),
                    ignore_duplicate_id: false,
                })
                .unwrap();
            index
        };
        let component_id_index_path = make_index_file(index).unwrap();
        let mut builder = RoutingTestBuilderForAnalyzer::new("directory_provider", components);
        builder.set_component_id_index_path(
            component_id_index_path.path().to_owned().try_into().unwrap(),
        );
        let test = builder.build().await;
        let root = test.look_up_instance(&Moniker::root()).await.expect("root instance");
        let storage_provider = test
            .look_up_instance(&Moniker::parse_str("/storage_provider").unwrap())
            .await
            .expect("storage_provider instance");

        let route_maps = test
            .model
            .check_routes_for_instance(
                &storage_provider,
                &HashSet::from_iter(vec![CapabilityTypeName::Storage].into_iter()),
            )
            .await;
        assert_eq!(route_maps.len(), 1);

        let storage = route_maps
            .get(&CapabilityTypeName::Storage)
            .expect("expected a storage capability route");

        assert_eq!(storage.len(), 1);
        for result in storage {
            assert_eq!(result.using_node, Moniker::parse_str("/storage_provider").unwrap());
            assert_eq!(result.target_decl, TargetDecl::Offer(offer_storage_decl.clone()));
            assert_eq!(result.capability, Some("cache".parse().unwrap()));
            assert!(result.error.is_none());
            assert_eq!(
                result.source,
                Some(CapabilitySource::Component(ComponentSource {
                    capability: ComponentCapability::Directory(match directory_decl.clone() {
                        CapabilityDecl::Directory(decl) => decl,
                        _ => panic!("unexpected capability variant"),
                    }),
                    moniker: root.moniker().clone(),
                }))
            );
        }
    }
}
