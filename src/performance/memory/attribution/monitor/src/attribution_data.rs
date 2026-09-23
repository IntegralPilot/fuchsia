// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::attribution_client::AttributionClient;
use crate::common::PrincipalIdMap;
use crate::resources::{Job, KernelResources};
use attribution_processing::{
    Attribution, AttributionData, AttributionDataProvider, Principal, PrincipalDescription,
    ResourceReference,
};
use fuchsia_sync::Mutex;
use fuchsia_trace::duration;
use std::sync::Arc;
use traces::CATEGORY_MEMORY_CAPTURE;

pub struct AttributionDataProviderImpl {
    root_job: Arc<Mutex<dyn Job>>,
    attribution_client: Arc<dyn AttributionClient>,
    muted_principal: Option<PrincipalDescription>,
}

impl AttributionDataProviderImpl {
    /// Create a new [AttributionDataProviderImpl]. `attribution_client` exposes attribution
    /// information from the memory attribution protocol, and `root_job` is used to retrieve memory
    /// usage of kernel objects.
    pub fn new(
        attribution_client: Arc<dyn AttributionClient>,
        root_job: Arc<Mutex<dyn Job>>,
    ) -> Arc<AttributionDataProviderImpl> {
        Arc::new(AttributionDataProviderImpl {
            root_job,
            attribution_client,
            muted_principal: None,
        })
    }
    pub fn with_muted_principal(&self, muted_principal: Option<PrincipalDescription>) -> Arc<Self> {
        Arc::new(AttributionDataProviderImpl {
            root_job: self.root_job.clone(),
            attribution_client: self.attribution_client.clone(),
            muted_principal,
        })
    }
}

impl AttributionDataProvider for AttributionDataProviderImpl {
    fn get_attribution_data(&self) -> Result<AttributionData, anyhow::Error> {
        let attribution_state = self.attribution_client.get_attributions();

        let kernel_resources = KernelResources::get_resources(
            &*self.root_job.lock(),
            &attribution_state,
            &self.muted_principal,
        )?;

        duration!(CATEGORY_MEMORY_CAPTURE, "AttributionSnapshot::new");
        // Compute the capacity needed for |principals| and |attributions| to avoid
        // reallocations as we fill these vectors.
        let (num_principals, num_attributions) = attribution_state
            .0
            .values()
            .map(|provider| (provider.definitions.len(), provider.resources.len()))
            .fold((0, 0), |(acc_p, acc_a), (p, a)| (acc_p + p, acc_a + a));

        let mut principals = Vec::with_capacity(num_principals);
        let mut attributions = Vec::with_capacity(num_attributions);

        for (provider_identifier, attribution_provider) in attribution_state.0 {
            // fuchsia.memory.attribution.Providers protocol identifiers locally unique
            // for a given `Provider`. `local_to_global` maps those to global identifiers.
            let mut local_to_global = PrincipalIdMap::default();
            for (local_id, definition) in attribution_provider.definitions {
                local_to_global.insert(local_id, definition.id);
                principals.push(Principal {
                    identifier: definition.id.into(),
                    description: definition.description,
                    principal_type: definition.principal_type,
                    parent: definition
                        .attributor
                        .as_ref()
                        .map(|&principal_identifier| principal_identifier.into()),
                });
            }
            for (subject_identifier, resources) in attribution_provider.resources {
                attributions.push(Attribution {
                    source: provider_identifier.into(),
                    subject: local_to_global.get(subject_identifier, provider_identifier).into(),
                    resources: resources
                        .into_iter()
                        .map(|r| match r {
                            fidl_fuchsia_memory_attribution::Resource::KernelObject(koid) => {
                                ResourceReference::KernelObject(koid)
                            }
                            fidl_fuchsia_memory_attribution::Resource::ProcessMapped(
                                fidl_fuchsia_memory_attribution::ProcessMapped {
                                    process,
                                    base,
                                    len,
                                    hint_skip_handle_table,
                                },
                            ) => ResourceReference::ProcessMapped {
                                process,
                                base,
                                len,
                                hint_skip_handle_table,
                            },
                            fidl_fuchsia_memory_attribution::Resource::__SourceBreaking {
                                unknown_ordinal: _,
                            } => unimplemented!("Unknown Resource type"),
                        })
                        .collect(),
                });
            }
        }

        Ok(AttributionData {
            principals_vec: principals,
            resources_vec: kernel_resources.resources.into_values().map(|r| r.into()).collect(),
            resource_names: kernel_resources.resource_names,
            attributions,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attribution_client::{AttributionProvider, AttributionState, PrincipalDefinition};
    use crate::common::LocalPrincipalIdentifier;
    use crate::resources::tests::{FakeJob, FakeProcess, simple_vmo_info};
    use assert_matches::assert_matches;
    use attribution_processing::summary::{MemorySummary, PrincipalSummary, VmoSummary};
    use attribution_processing::{
        GlobalPrincipalIdentifier, GlobalPrincipalIdentifierFactory, PrincipalDescription,
        PrincipalType, ZXName,
    };
    use itertools::Itertools;
    use maplit::hashmap;
    use pretty_assertions::assert_eq;
    use std::collections::HashSet;
    use {
        fidl_fuchsia_memory_attribution as fattribution,
        fidl_fuchsia_memory_attribution_plugin as fplugin,
    };

    /// Projection of object state into a standardized (canonical) form that can be
    /// compared with equality. This usually removes variability of the state
    /// representation, such as sequence orders or non-canonical field values.
    trait Normalize<T> {
        fn normalize(self) -> T;
    }

    impl Normalize<PrincipalSummary> for PrincipalSummary {
        fn normalize(self) -> PrincipalSummary {
            PrincipalSummary { processes: self.processes.into_iter().sorted().collect(), ..self }
        }
    }

    impl Normalize<MemorySummary> for MemorySummary {
        fn normalize(self) -> MemorySummary {
            MemorySummary {
                principals: self
                    .principals
                    .into_iter()
                    .map(|principal| principal.normalize())
                    .sorted_by_key(|principal| principal.id)
                    .collect(),
                ..self
            }
        }
    }

    #[test]
    fn test_get_capture() {
        let mut identifier_factory = GlobalPrincipalIdentifierFactory::default();

        #[derive(Default)]
        struct FakeAttributionClient {
            state: AttributionState,
        }

        impl FakeAttributionClient {
            fn add_provider_vmo(
                &mut self,
                attributor_id: GlobalPrincipalIdentifier,
                principal_id: GlobalPrincipalIdentifier,
                name: &str,
                resource_koid: u64,
            ) {
                self.state.0.insert(
                    attributor_id.clone(),
                    AttributionProvider {
                        definitions: [(
                            LocalPrincipalIdentifier(1),
                            PrincipalDefinition {
                                attributor: Some(attributor_id),
                                id: principal_id,
                                description: Some(PrincipalDescription::Component(name.to_owned())),
                                principal_type: PrincipalType::Runnable,
                            },
                        )]
                        .into(),
                        resources: [(
                            LocalPrincipalIdentifier(1),
                            [fattribution::Resource::KernelObject(resource_koid)].into(),
                        )]
                        .into(),
                    },
                );
            }

            fn add_provider_part_map(
                &mut self,
                attributor_id: GlobalPrincipalIdentifier,
                principal_id: GlobalPrincipalIdentifier,
                name: &str,
                process_koid: u64,
                base: u64,
                len: u64,
            ) {
                self.state.0.insert(
                    attributor_id.clone(),
                    AttributionProvider {
                        definitions: [(
                            LocalPrincipalIdentifier(1),
                            PrincipalDefinition {
                                attributor: Some(attributor_id),
                                id: principal_id,
                                description: Some(PrincipalDescription::Part(name.to_owned())),
                                principal_type: PrincipalType::Part,
                            },
                        )]
                        .into(),
                        resources: [(
                            LocalPrincipalIdentifier(1),
                            [fattribution::Resource::ProcessMapped(fattribution::ProcessMapped {
                                process: process_koid,
                                base,
                                len,
                                hint_skip_handle_table: true,
                            })]
                            .into(),
                        )]
                        .into(),
                    },
                );
            }
        }

        impl AttributionClient for FakeAttributionClient {
            fn get_attributions(&self) -> crate::attribution_client::AttributionState {
                self.state.clone()
            }
        }

        let mut fake_attribution_client = FakeAttributionClient::default();
        let parent_global_id = identifier_factory.next();
        let component_global_id = identifier_factory.next();

        fake_attribution_client.add_provider_vmo(
            parent_global_id,
            component_global_id.clone(),
            "component1",
            2,
        );
        fake_attribution_client.add_provider_part_map(
            component_global_id,
            identifier_factory.next(),
            "part2",
            5,
            0,
            1024,
        );

        let mut mapping_details = zx::MappingDetails::default();
        mapping_details.vmo_koid = zx::Koid::from_raw(6);
        mapping_details.committed_bytes = 1024;

        let capture_provider = AttributionDataProviderImpl::new(
            Arc::new(fake_attribution_client),
            Arc::new(Mutex::new(FakeJob::new(
                1,
                "job1",
                [FakeJob::new(
                    2,
                    "job2",
                    [].into(),
                    [
                        FakeProcess::new(
                            3,
                            "process3",
                            [
                                simple_vmo_info(4, "vmo4", 0, 1024, 2048),
                                simple_vmo_info(6, "vmo6", 0, 1024, 2048),
                            ]
                            .into(),
                            [].into(),
                        ),
                        FakeProcess::new(
                            5,
                            "process5",
                            [].into(),
                            [zx::MapInfo::new(
                                zx::Name::from_bytes_lossy("map1".as_bytes()),
                                0,
                                1024,
                                1,
                                zx::MapDetails::Mapping(&mapping_details),
                            )
                            .unwrap()]
                            .into(),
                        ),
                    ]
                    .into(),
                )]
                .into(),
                [].into(),
            ))),
        );

        // Exercise the code.
        let attribution_data = capture_provider.get_attribution_data().unwrap();
        assert_eq!(
            HashSet::from_iter(attribution_data.principals_vec.clone().into_iter()),
            HashSet::from([
                Principal {
                    identifier: GlobalPrincipalIdentifier::new_for_test(2),
                    parent: Some(GlobalPrincipalIdentifier::new_for_test(1)),
                    description: Some(PrincipalDescription::Component("component1".to_owned())),
                    principal_type: PrincipalType::Runnable,
                },
                Principal {
                    identifier: GlobalPrincipalIdentifier::new_for_test(3),
                    parent: Some(GlobalPrincipalIdentifier::new_for_test(2)),
                    description: Some(PrincipalDescription::Part("part2".to_owned())),
                    principal_type: PrincipalType::Part,
                }
            ])
        );

        assert_eq!(
            attribution_data.resource_names.clone().into_iter().collect::<HashSet<ZXName>>(),
            HashSet::from([
                ZXName::from_string_lossy("job1"),
                ZXName::from_string_lossy("job2"),
                ZXName::from_string_lossy("process3"),
                ZXName::from_string_lossy("vmo4"),
                ZXName::from_string_lossy("process5"),
                ZXName::from_string_lossy("vmo6")
            ])
        );

        // Verify that the process(koid:5) has some mapping collected.
        assert_matches!(
            attribution_data.resources_vec.iter().find(|r| r.koid == 5),
            Some(attribution_processing::Resource {
                resource_type: fplugin::ResourceType::Process(fplugin::Process {
                    mappings: Some(_),
                    vmos: None,
                    ..
                }),
                ..
            })
        );

        assert_eq!(
            attribution_processing::attribute_vmos(attribution_data).summary().normalize(),
            attribution_processing::summary::MemorySummary {
                principals: vec![
                    PrincipalSummary {
                        id: 2,
                        name: "component1".to_string(),
                        principal_type: "R".to_string(),
                        committed_private: 0,
                        committed_scaled: 1024.0,
                        committed_total: 1024,
                        populated_private: 0,
                        populated_scaled: 2048.0,
                        populated_total: 2048,
                        attributor: None,
                        processes: vec!["process3 (3)".to_string(), "process5 (5)".to_string(),],
                        vmos: hashmap! {
                            ZXName::from_string_lossy("vmo4") => VmoSummary {
                                count: 1,
                                committed_private: 0,
                                committed_scaled: 1024.0,
                                committed_total: 1024,
                                populated_private: 0,
                                populated_scaled: 2048.0,
                                populated_total: 2048,
                            },
                        },
                    },
                    PrincipalSummary {
                        id: 3,
                        name: "part2".to_string(),
                        principal_type: "P".to_string(),
                        committed_private: 0,
                        committed_scaled: 1024.0,
                        committed_total: 1024,
                        populated_private: 0,
                        populated_scaled: 2048.0,
                        populated_total: 2048,
                        attributor: Some("component1".to_string(),),
                        processes: vec!["process5 (5)".to_string(),],
                        vmos: hashmap! {  ZXName::from_string_lossy("vmo6") => VmoSummary {
                            count: 1,
                            committed_private: 0,
                            committed_scaled: 1024.0,
                            committed_total: 1024,
                            populated_private: 0,
                            populated_scaled: 2048.0,
                            populated_total: 2048,
                        },},
                    },
                ],
                unclaimed: 0
            }
            .normalize()
        );

        // Exercise provider muting.
        let attribution_data = capture_provider
            .with_muted_principal(Some(PrincipalDescription::Component("component1".to_owned())))
            .get_attribution_data()
            .unwrap();

        // Because process(koid:5) mapping attribution was declared by "component1", and
        // "component1" is muted. The mapping attribution should be ignored, and the
        // mapping collection should not be done.
        //
        // Verify that the process koid:5:
        // - has no mapping collected because the mapping attribution is muted, which is
        //   equivalent to no attribution.
        // - has not VMO collected because (i) hint_skip_handle_table is not affected by
        //   "muting", and (ii) the VMO are not explicitly attributed.
        assert_matches!(
            attribution_data.resources_vec.iter().find(|r| r.koid == 5),
            Some(attribution_processing::Resource {
                resource_type: fplugin::ResourceType::Process(fplugin::Process {
                    mappings: None,
                    vmos: None,
                    ..
                }),
                ..
            })
        );
        // Verify that the process koid:3 has VMO collected which is the default behavior.
        assert_matches!(
            attribution_data.resources_vec.iter().find(|r| r.koid == 3),
            Some(attribution_processing::Resource {
                resource_type: fplugin::ResourceType::Process(fplugin::Process {
                    mappings: None,
                    vmos: Some(_),
                    ..
                }),
                ..
            })
        );

        assert_eq!(
            attribution_processing::attribute_vmos(attribution_data).summary(),
            attribution_processing::summary::MemorySummary {
                principals: vec![
                    PrincipalSummary {
                        id: 2,
                        name: "component1".to_string(),
                        principal_type: "R".to_string(),
                        committed_private: 0,
                        committed_scaled: 2048.0,
                        committed_total: 2048,
                        populated_private: 0,
                        populated_scaled: 4096.0,
                        populated_total: 4096,
                        attributor: None,
                        processes: vec!["process3 (3)".to_string(), "process5 (5)".to_string(),],
                        vmos: hashmap! {
                            ZXName::from_string_lossy("vmo4") => VmoSummary {
                                count: 1,
                                committed_private: 0,
                                committed_scaled: 1024.0,
                                committed_total: 1024,
                                populated_private: 0,
                                populated_scaled: 2048.0,
                                populated_total: 2048,
                            },
                            // `vmo6` attributed to `part2` went back to `component1`
                            // as a result of muting `component1`.
                            ZXName::from_string_lossy("vmo6") => VmoSummary {
                                count: 1,
                                committed_private: 0,
                                committed_scaled: 1024.0,
                                committed_total: 1024,
                                populated_private: 0,
                                populated_scaled: 2048.0,
                                populated_total: 2048,
                            },
                        },
                    },
                    PrincipalSummary {
                        id: 3,
                        name: "part2".to_string(),
                        principal_type: "P".to_string(),
                        committed_private: 0,
                        committed_scaled: 0.0,
                        committed_total: 0,
                        populated_private: 0,
                        populated_scaled: 0.0,
                        populated_total: 0,
                        attributor: Some("component1".to_string(),),
                        processes: vec!["process5 (5)".to_string(),],
                        vmos: hashmap! {},
                    },
                ],
                unclaimed: 0
            }
        );
    }
}
