// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use attribution_processing::ZXName;
use attribution_processing::fkernel_serde::{MemoryStatsCompressionDef, MemoryStatsDef};
use fdomain_fuchsia_kernel as fkernel;
use fdomain_fuchsia_memory_attribution_plugin as fplugin;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use zx_types::ZX_MAX_NAME_LEN;

/// An object with this trait can be converted to, and from, JSON.
// TODO: https://fxbug.dev/369609539 - This trait is necessary as FIDL bindings don't derive serde's
// traits for serialization and deserialization.
pub(crate) trait JsonConvertible {
    fn to_json(&self) -> Value;
    fn from_json(value: &Value) -> Option<Self>
    where
        Self: std::marker::Sized;
}

impl JsonConvertible for fplugin::Snapshot {
    fn to_json(&self) -> Value {
        json!({
            "attributions": self.attributions.to_json(),
            "principals": self.principals.to_json(),
            "resources": self.resources.to_json(),
            "resource_names": self.resource_names.to_json(),
            "kernel_statistics": self.kernel_statistics.to_json(),
            "bucket_definitions": self.bucket_definitions.to_json(),
            "performance_metrics": self.performance_metrics.to_json(),
        })
    }

    fn from_json(value: &Value) -> Option<Self> {
        let Some(obj) = value.as_object() else {
            return None;
        };
        Some(Self {
            attributions: obj
                .get("attributions")
                .map(|o| Vec::<fplugin::Attribution>::from_json(o))
                .flatten(),
            principals: obj
                .get("principals")
                .map(|o| Vec::<fplugin::Principal>::from_json(o))
                .flatten(),
            resources: obj
                .get("resources")
                .map(|o| Vec::<fplugin::Resource>::from_json(o))
                .flatten(),
            resource_names: obj
                .get("resource_names")
                .map(|o| Vec::<[u8; ZX_MAX_NAME_LEN]>::from_json(o))
                .flatten(),
            kernel_statistics: obj
                .get("kernel_statistics")
                .map(|o| fplugin::KernelStatistics::from_json(o))
                .flatten(),
            bucket_definitions: obj
                .get("bucket_definitions")
                .map(|o| Vec::<fplugin::BucketDefinition>::from_json(o))
                .flatten(),
            performance_metrics: obj
                .get("performance_metrics")
                .map(|o| fplugin::PerformanceImpactMetrics::from_json(o))
                .flatten(),
            ..Default::default()
        })
    }
}

impl<T: JsonConvertible> JsonConvertible for Vec<T> {
    fn to_json(&self) -> Value {
        Value::Array(self.iter().map(|element| element.to_json()).collect())
    }

    fn from_json(value: &Value) -> Option<Self> {
        match value.as_array() {
            None => None,
            Some(arr) => Some(arr.iter().map(|v| T::from_json(v)).flatten().collect()),
        }
    }
}

impl<T: JsonConvertible> JsonConvertible for Option<T> {
    fn to_json(&self) -> Value {
        match self {
            None => Value::Null,
            Some(element) => json!(element.to_json()),
        }
    }

    fn from_json(value: &Value) -> Option<Self> {
        Some(T::from_json(value))
    }
}

impl<T: JsonConvertible, const U: usize> JsonConvertible for [T; U] {
    fn to_json(&self) -> Value {
        Value::Array(self.iter().map(|element| element.to_json()).collect())
    }

    fn from_json(value: &Value) -> Option<Self> {
        match value.as_array() {
            None => None,
            Some(arr) => {
                arr.iter().map(|v| T::from_json(v)).flatten().collect::<Vec<T>>().try_into().ok()
            }
        }
    }
}

impl JsonConvertible for i64 {
    fn to_json(&self) -> Value {
        json!(*self)
    }

    fn from_json(value: &Value) -> Option<Self> {
        value.as_i64()
    }
}

impl JsonConvertible for u64 {
    fn to_json(&self) -> Value {
        json!(*self)
    }

    fn from_json(value: &Value) -> Option<Self> {
        value.as_u64()
    }
}

impl JsonConvertible for u32 {
    fn to_json(&self) -> Value {
        json!(*self)
    }

    fn from_json(value: &Value) -> Option<Self> {
        value.as_u64().and_then(|v| v.try_into().ok())
    }
}

impl JsonConvertible for String {
    fn to_json(&self) -> Value {
        json!(self)
    }

    fn from_json(value: &Value) -> Option<Self> {
        value.as_str().map(|s| s.to_owned())
    }
}

impl JsonConvertible for fplugin::PrincipalIdentifier {
    fn to_json(&self) -> Value {
        json!(self.id)
    }

    fn from_json(value: &Value) -> Option<Self> {
        value.as_u64().map(|v| fplugin::PrincipalIdentifier { id: v })
    }
}

impl JsonConvertible for fplugin::Attribution {
    fn to_json(&self) -> Value {
        json!([self.source.to_json(), self.subject.to_json(), self.resources.to_json()])
    }

    fn from_json(value: &Value) -> Option<Self> {
        value
            .as_array()
            .map(|arr| {
                if arr.len() != 3 {
                    None
                } else {
                    Some(fplugin::Attribution {
                        source: arr.get(0).map(fplugin::PrincipalIdentifier::from_json).flatten(),
                        subject: arr.get(1).map(fplugin::PrincipalIdentifier::from_json).flatten(),
                        resources: Vec::<fplugin::ResourceReference>::from_json(
                            arr.get(2).unwrap(),
                        ),
                        ..Default::default()
                    })
                }
            })
            .flatten()
    }
}

impl JsonConvertible for fplugin::ResourceReference {
    fn to_json(&self) -> Value {
        match self {
            fplugin::ResourceReference::KernelObject(koid) => json!({
                "kernel_object": koid
            }),
            fplugin::ResourceReference::ProcessMapped(fplugin::ProcessMapped {
                process,
                base,
                len,
                hint_skip_handle_table,
            }) => json!({
                "process_mapped": {
                    "process": process,
                    "base": base,
                    "len": len,
                    "hint_skip_handle_table": hint_skip_handle_table
                }
            }),
            fplugin::ResourceReference::__SourceBreaking { unknown_ordinal: _ } => unimplemented!(),
        }
    }

    fn from_json(value: &Value) -> Option<Self> {
        let Some(obj) = value.as_object() else { return None };
        if let Some(kernel_object) = obj.get("kernel_object") {
            return Some(fplugin::ResourceReference::KernelObject(kernel_object.as_u64().unwrap()));
        }
        if let Some(process_mapped) = obj.get("process_mapped") {
            let Some(process_obj) = process_mapped.as_object() else { return None };
            return Some(fplugin::ResourceReference::ProcessMapped(fplugin::ProcessMapped {
                process: process_obj.get("process").unwrap().as_u64().unwrap(),
                base: process_obj.get("base").unwrap().as_u64().unwrap(),
                len: process_obj.get("len").unwrap().as_u64().unwrap(),
                hint_skip_handle_table: process_obj
                    .get("hint_skip_handle_table")
                    .unwrap()
                    .as_bool()
                    .unwrap(),
            }));
        }
        None
    }
}

impl JsonConvertible for fplugin::Principal {
    fn to_json(&self) -> Value {
        json!([
            self.identifier.to_json(),
            self.description.to_json(),
            self.principal_type.to_json(),
            self.parent.to_json()
        ])
    }

    fn from_json(value: &Value) -> Option<Self> {
        let Some(obj) = value.as_array() else { return None };
        Some(Self {
            identifier: obj.get(0).map(fplugin::PrincipalIdentifier::from_json).flatten(),
            description: obj.get(1).map(|v| fplugin::Description::from_json(v)).flatten(),
            principal_type: obj.get(2).map(|v| fplugin::PrincipalType::from_json(v)).flatten(),
            parent: obj.get(3).map(fplugin::PrincipalIdentifier::from_json).flatten(),
            ..Default::default()
        })
    }
}

impl JsonConvertible for fplugin::Description {
    fn to_json(&self) -> Value {
        match self {
            fplugin::Description::Component(name) => json!({"component": name}),
            fplugin::Description::Part(name) => json!({"part": name}),
            fplugin::Description::__SourceBreaking { unknown_ordinal: _ } => unimplemented!(),
        }
    }

    fn from_json(value: &Value) -> Option<Self> {
        let Some(obj) = value.as_object() else { return None };
        if let Some(component_name) = obj.get("component") {
            return Some(fplugin::Description::Component(
                component_name.as_str().unwrap().to_owned(),
            ));
        }
        if let Some(part_name) = obj.get("part") {
            return Some(fplugin::Description::Part(part_name.as_str().unwrap().to_owned()));
        }
        None
    }
}

impl JsonConvertible for fplugin::PrincipalType {
    fn to_json(&self) -> Value {
        match self {
            fplugin::PrincipalType::Runnable => json!("runnable"),
            fplugin::PrincipalType::Part => json!("part"),
            fplugin::PrincipalType::__SourceBreaking { unknown_ordinal: _ } => unimplemented!(),
        }
    }

    fn from_json(value: &Value) -> Option<Self> {
        let Some(obj) = value.as_str() else { return None };
        match obj {
            "runnable" => Some(fplugin::PrincipalType::Runnable),
            "part" => Some(fplugin::PrincipalType::Part),
            _ => None,
        }
    }
}

impl JsonConvertible for fplugin::Resource {
    fn to_json(&self) -> Value {
        json!([self.koid, self.name_index, self.resource_type.to_json()])
    }

    fn from_json(value: &Value) -> Option<Self> {
        let Some(obj) = value.as_array() else { return None };
        Some(Self {
            koid: obj.get(0).map(|v| v.as_u64()).flatten(),
            name_index: obj.get(1).map(|v| v.as_u64()).flatten(),
            resource_type: obj.get(2).map(|v| fplugin::ResourceType::from_json(v)).flatten(),
            ..Default::default()
        })
    }
}

impl JsonConvertible for fplugin::ResourceType {
    fn to_json(&self) -> Value {
        match self {
            fplugin::ResourceType::Job(fplugin::Job {
                child_jobs,
                processes,
                __source_breaking: _,
            }) => json!({
                "job": [child_jobs.to_json(), processes.to_json()]
            }),
            fplugin::ResourceType::Process(fplugin::Process {
                vmos,
                mappings,
                __source_breaking: _,
            }) => json!({
                "process": [vmos.to_json(), mappings.to_json()],
            }),
            fplugin::ResourceType::Vmo(fplugin::Vmo {
                parent,
                private_committed_bytes,
                private_populated_bytes,
                scaled_committed_bytes,
                scaled_populated_bytes,
                total_committed_bytes,
                total_populated_bytes,
                flags,
                __source_breaking: _,
            }) => json!({
                "vmo": [parent.to_json(),
                        private_committed_bytes.to_json(),
                        private_populated_bytes.to_json(),
                        scaled_committed_bytes.to_json(),
                        scaled_populated_bytes.to_json(),
                        total_committed_bytes.to_json(),
                        total_populated_bytes.to_json(),
                        flags.to_json()],
            }),
            fplugin::ResourceType::__SourceBreaking { unknown_ordinal: _ } => unimplemented!(),
        }
    }

    fn from_json(value: &Value) -> Option<Self> {
        let Some(obj) = value.as_object() else { return None };
        if let Some(job) = obj.get("job") {
            let Some(arr) = job.as_array() else { return None };
            return Some(fplugin::ResourceType::Job(fplugin::Job {
                child_jobs: arr.get(0).map(|v| Vec::<u64>::from_json(v)).flatten(),
                processes: arr.get(1).map(|v| Vec::<u64>::from_json(v)).flatten(),
                ..Default::default()
            }));
        }
        if let Some(process) = obj.get("process") {
            let Some(arr) = process.as_array() else { return None };
            return Some(fplugin::ResourceType::Process(fplugin::Process {
                vmos: arr.get(0).map(|v| Vec::<u64>::from_json(v)).flatten(),
                mappings: arr.get(1).map(|v| Vec::<fplugin::Mapping>::from_json(v)).flatten(),
                ..Default::default()
            }));
        }
        if let Some(vmo) = obj.get("vmo") {
            let Some(arr) = vmo.as_array() else { return None };
            return Some(fplugin::ResourceType::Vmo(fplugin::Vmo {
                parent: arr.get(0).map(|v| u64::from_json(v)).flatten(),
                private_committed_bytes: arr.get(1).map(|v| u64::from_json(v)).flatten(),
                private_populated_bytes: arr.get(2).map(|v| u64::from_json(v)).flatten(),
                scaled_committed_bytes: arr.get(3).map(|v| u64::from_json(v)).flatten(),
                scaled_populated_bytes: arr.get(4).map(|v| u64::from_json(v)).flatten(),
                total_committed_bytes: arr.get(5).map(|v| u64::from_json(v)).flatten(),
                total_populated_bytes: arr.get(6).map(|v| u64::from_json(v)).flatten(),
                flags: arr.get(7).map(|v| u32::from_json(v)).flatten(),
                ..Default::default()
            }));
        }
        None
    }
}

impl JsonConvertible for fplugin::Mapping {
    fn to_json(&self) -> Value {
        json!([self.vmo.to_json(), self.address_base.to_json(), self.size.to_json(),])
    }

    fn from_json(value: &Value) -> Option<Self> {
        let Some(arr) = value.as_array() else { return None };
        Some(fplugin::Mapping {
            vmo: arr.get(0).map(|v| u64::from_json(v)).flatten(),
            address_base: arr.get(1).map(|v| u64::from_json(v)).flatten(),
            size: arr.get(2).map(|v| u64::from_json(v)).flatten(),
            ..Default::default()
        })
    }
}

// TODO(https://github.com/serde-rs/serde/issues/723): Use remote serialization.
impl JsonConvertible for fplugin::KernelStatistics {
    fn to_json(&self) -> Value {
        json!({
            "memory_stats": self.memory_stats.to_json(),
            "compression_stats": self.compression_stats.to_json(),
        })
    }

    fn from_json(value: &Value) -> Option<Self> {
        let Some(obj) = value.as_object() else {
            return None;
        };
        Some(Self {
            memory_stats: obj
                .get("memory_stats")
                .map(|v| fkernel::MemoryStats::from_json(v))
                .flatten(),
            compression_stats: obj
                .get("compression_stats")
                .map(|v| fkernel::MemoryStatsCompression::from_json(v))
                .flatten(),
            ..Default::default()
        })
    }
}

impl JsonConvertible for [u8; ZX_MAX_NAME_LEN] {
    fn to_json(&self) -> Value {
        serde_json::to_value(ZXName::from_bytes_lossy(self)).expect("invalid data")
    }
    fn from_json(value: &Value) -> Option<[u8; ZX_MAX_NAME_LEN]> {
        Some(*ZXName::deserialize(value).unwrap().buffer())
    }
}

impl JsonConvertible for fkernel::MemoryStats {
    fn to_json(&self) -> Value {
        #[derive(Serialize)]
        struct Helper<'a>(#[serde(with = "MemoryStatsDef")] &'a fkernel::MemoryStats);
        serde_json::to_value(Helper(self)).expect("invalid data")
    }
    fn from_json(value: &Value) -> Option<fkernel::MemoryStats> {
        Some(MemoryStatsDef::deserialize(value).unwrap())
    }
}

impl JsonConvertible for fkernel::MemoryStatsCompression {
    fn to_json(&self) -> Value {
        #[derive(Serialize)]
        struct Helper<'a>(
            #[serde(with = "MemoryStatsCompressionDef")] &'a fkernel::MemoryStatsCompression,
        );
        serde_json::to_value(Helper(self)).expect("invalid data")
    }
    fn from_json(value: &Value) -> Option<fkernel::MemoryStatsCompression> {
        Some(MemoryStatsCompressionDef::deserialize(value).unwrap())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(remote = "fplugin::BucketDefinition")]
struct BucketDefinitionDef {
    pub name: Option<String>,
    pub process: Option<String>,
    pub vmo: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub principal: Option<String>,
    #[doc(hidden)]
    #[serde(skip)]
    pub __source_breaking: fidl::marker::SourceBreaking,
}

impl JsonConvertible for fplugin::BucketDefinition {
    fn to_json(&self) -> Value {
        #[derive(Serialize)]
        struct Helper<'a>(#[serde(with = "BucketDefinitionDef")] &'a fplugin::BucketDefinition);
        serde_json::to_value(Helper(self)).expect("invalid data")
    }
    fn from_json(value: &Value) -> Option<fplugin::BucketDefinition> {
        Some(BucketDefinitionDef::deserialize(value).unwrap())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(remote = "fplugin::PerformanceImpactMetrics")]
struct PerformanceImpactMetricsDef {
    pub some_memory_stalls_ns: Option<i64>,
    pub full_memory_stalls_ns: Option<i64>,
    pub page_refaults: Option<u64>,
    #[doc(hidden)]
    #[serde(skip)]
    pub __source_breaking: fidl::marker::SourceBreaking,
}

impl JsonConvertible for fplugin::PerformanceImpactMetrics {
    fn to_json(&self) -> Value {
        #[derive(Serialize)]
        struct Helper<'a>(
            #[serde(with = "PerformanceImpactMetricsDef")] &'a fplugin::PerformanceImpactMetrics,
        );
        serde_json::to_value(Helper(self)).expect("invalid data")
    }
    fn from_json(value: &Value) -> Option<fplugin::PerformanceImpactMetrics> {
        Some(PerformanceImpactMetricsDef::deserialize(value).unwrap())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Context;
    use fdomain_fuchsia_memory_attribution_plugin as fplugin;

    #[test]
    fn test_serialize_deserialize() {
        let example_snapshot = fplugin::Snapshot {
            attributions: Some(vec![
                fplugin::Attribution {
                    source: Some(fplugin::PrincipalIdentifier { id: 0 }),
                    subject: Some(fplugin::PrincipalIdentifier { id: 0 }),
                    resources: Some(vec![fplugin::ResourceReference::KernelObject(1000)]),
                    ..Default::default()
                },
                fplugin::Attribution {
                    source: Some(fplugin::PrincipalIdentifier { id: 0 }),
                    subject: Some(fplugin::PrincipalIdentifier { id: 1 }),
                    resources: Some(vec![fplugin::ResourceReference::KernelObject(1004)]),
                    ..Default::default()
                },
                fplugin::Attribution {
                    source: Some(fplugin::PrincipalIdentifier { id: 0 }),
                    subject: Some(fplugin::PrincipalIdentifier { id: 2 }),
                    resources: Some(vec![fplugin::ResourceReference::KernelObject(1008)]),
                    ..Default::default()
                },
                fplugin::Attribution {
                    source: Some(fplugin::PrincipalIdentifier { id: 1 }),
                    subject: Some(fplugin::PrincipalIdentifier { id: 3 }),
                    resources: Some(vec![fplugin::ResourceReference::KernelObject(1007)]),
                    ..Default::default()
                },
            ]),
            principals: Some(vec![
                fplugin::Principal {
                    identifier: Some(fplugin::PrincipalIdentifier { id: 0 }),
                    description: Some(fplugin::Description::Component("root".to_owned())),
                    principal_type: Some(fplugin::PrincipalType::Runnable),
                    parent: None,
                    ..Default::default()
                },
                fplugin::Principal {
                    identifier: Some(fplugin::PrincipalIdentifier { id: 1 }),
                    description: Some(fplugin::Description::Component("runner".to_owned())),
                    principal_type: Some(fplugin::PrincipalType::Part),
                    parent: Some(fplugin::PrincipalIdentifier { id: 0 }),
                    ..Default::default()
                },
                fplugin::Principal {
                    identifier: Some(fplugin::PrincipalIdentifier { id: 2 }),
                    description: Some(fplugin::Description::Component("component 2".to_owned())),
                    principal_type: Some(fplugin::PrincipalType::Runnable),
                    parent: Some(fplugin::PrincipalIdentifier { id: 0 }),
                    ..Default::default()
                },
                fplugin::Principal {
                    identifier: Some(fplugin::PrincipalIdentifier { id: 3 }),
                    description: Some(fplugin::Description::Component("component 3".to_owned())),
                    principal_type: Some(fplugin::PrincipalType::Runnable),
                    parent: Some(fplugin::PrincipalIdentifier { id: 1 }),
                    ..Default::default()
                },
            ]),
            resources: Some(vec![
                fplugin::Resource {
                    koid: Some(1000),
                    name_index: Some(0),
                    resource_type: Some(fplugin::ResourceType::Job(fplugin::Job {
                        child_jobs: Some(vec![1004, 1008]),
                        processes: Some(vec![1001]),
                        ..Default::default()
                    })),
                    ..Default::default()
                },
                fplugin::Resource {
                    koid: Some(1001),
                    name_index: Some(1),
                    resource_type: Some(fplugin::ResourceType::Process(fplugin::Process {
                        vmos: Some(vec![1002, 1003]),
                        mappings: None,
                        ..Default::default()
                    })),
                    ..Default::default()
                },
                fplugin::Resource {
                    koid: Some(1002),
                    name_index: Some(2),
                    resource_type: Some(fplugin::ResourceType::Vmo(fplugin::Vmo {
                        private_committed_bytes: Some(1024),
                        private_populated_bytes: Some(2048),
                        scaled_committed_bytes: Some(1024),
                        scaled_populated_bytes: Some(2048),
                        total_committed_bytes: Some(1024),
                        total_populated_bytes: Some(2048),
                        flags: Some(1),
                        ..Default::default()
                    })),
                    ..Default::default()
                },
                fplugin::Resource {
                    koid: Some(1003),
                    name_index: Some(3),
                    resource_type: Some(fplugin::ResourceType::Vmo(fplugin::Vmo {
                        private_committed_bytes: Some(1024),
                        private_populated_bytes: Some(2048),
                        scaled_committed_bytes: Some(1024),
                        scaled_populated_bytes: Some(2048),
                        total_committed_bytes: Some(1024),
                        total_populated_bytes: Some(2048),
                        flags: Some(0),
                        ..Default::default()
                    })),
                    ..Default::default()
                },
                fplugin::Resource {
                    koid: Some(1004),
                    name_index: Some(4),
                    resource_type: Some(fplugin::ResourceType::Job(fplugin::Job {
                        child_jobs: Some(vec![]),
                        processes: Some(vec![1005]),
                        ..Default::default()
                    })),
                    ..Default::default()
                },
                fplugin::Resource {
                    koid: Some(1005),
                    name_index: Some(5),
                    resource_type: Some(fplugin::ResourceType::Process(fplugin::Process {
                        vmos: Some(vec![1006, 1007]),
                        mappings: None,
                        ..Default::default()
                    })),
                    ..Default::default()
                },
                fplugin::Resource {
                    koid: Some(1006),
                    name_index: Some(6),
                    resource_type: Some(fplugin::ResourceType::Vmo(fplugin::Vmo {
                        private_committed_bytes: Some(1024),
                        private_populated_bytes: Some(2048),
                        scaled_committed_bytes: Some(1024),
                        scaled_populated_bytes: Some(2048),
                        total_committed_bytes: Some(1024),
                        total_populated_bytes: Some(2048),
                        flags: Some(0),
                        ..Default::default()
                    })),
                    ..Default::default()
                },
                fplugin::Resource {
                    koid: Some(1007),
                    name_index: Some(7),
                    resource_type: Some(fplugin::ResourceType::Vmo(fplugin::Vmo {
                        private_committed_bytes: Some(1024),
                        private_populated_bytes: Some(2048),
                        scaled_committed_bytes: Some(1024),
                        scaled_populated_bytes: Some(2048),
                        total_committed_bytes: Some(1024),
                        total_populated_bytes: Some(2048),
                        flags: Some(0),
                        ..Default::default()
                    })),
                    ..Default::default()
                },
                fplugin::Resource {
                    koid: Some(1008),
                    name_index: Some(8),
                    resource_type: Some(fplugin::ResourceType::Job(fplugin::Job {
                        child_jobs: Some(vec![]),
                        processes: Some(vec![1009]),
                        ..Default::default()
                    })),
                    ..Default::default()
                },
                fplugin::Resource {
                    koid: Some(1009),
                    name_index: Some(9),
                    resource_type: Some(fplugin::ResourceType::Process(fplugin::Process {
                        vmos: Some(vec![1010, 1003]),
                        mappings: None,
                        ..Default::default()
                    })),
                    ..Default::default()
                },
                fplugin::Resource {
                    koid: Some(1010),
                    name_index: Some(10),
                    resource_type: Some(fplugin::ResourceType::Vmo(fplugin::Vmo {
                        private_committed_bytes: Some(1024),
                        private_populated_bytes: Some(2048),
                        scaled_committed_bytes: Some(1024),
                        scaled_populated_bytes: Some(2048),
                        total_committed_bytes: Some(1024),
                        total_populated_bytes: Some(2048),
                        flags: Some(0),
                        ..Default::default()
                    })),
                    ..Default::default()
                },
            ]),
            resource_names: Some(vec![
                *ZXName::from_string_lossy("root_job").buffer(),
                *ZXName::from_string_lossy("root_process").buffer(),
                *ZXName::from_string_lossy("root_vmo").buffer(),
                *ZXName::from_string_lossy("shared_vmo").buffer(),
                *ZXName::from_string_lossy("runner_job").buffer(),
                *ZXName::from_string_lossy("runner_process").buffer(),
                *ZXName::from_string_lossy("runner_vmo").buffer(),
                *ZXName::from_string_lossy("component_vmo").buffer(),
                *ZXName::from_string_lossy("component_2_job").buffer(),
                *ZXName::from_string_lossy("2_process").buffer(),
                *ZXName::from_string_lossy("2_vmo").buffer(),
            ]),
            kernel_statistics: Some(fplugin::KernelStatistics {
                memory_stats: Some(fdomain_fuchsia_kernel::MemoryStats {
                    total_bytes: Some(1),
                    free_bytes: Some(2),
                    free_loaned_bytes: Some(3),
                    wired_bytes: Some(4),
                    total_heap_bytes: Some(5),
                    free_heap_bytes: Some(6),
                    vmo_bytes: Some(7),
                    mmu_overhead_bytes: Some(8),
                    ipc_bytes: Some(9),
                    cache_bytes: Some(10),
                    slab_bytes: Some(11),
                    zram_bytes: Some(12),
                    other_bytes: Some(13),
                    vmo_reclaim_total_bytes: Some(14),
                    vmo_reclaim_newest_bytes: Some(15),
                    vmo_reclaim_oldest_bytes: Some(16),
                    vmo_reclaim_disabled_bytes: Some(17),
                    vmo_discardable_locked_bytes: Some(18),
                    vmo_discardable_unlocked_bytes: Some(19),
                    ..Default::default()
                }),
                compression_stats: Some(fdomain_fuchsia_kernel::MemoryStatsCompression {
                    uncompressed_storage_bytes: Some(101),
                    compressed_storage_bytes: Some(102),
                    compressed_fragmentation_bytes: Some(103),
                    compression_time: Some(104),
                    decompression_time: Some(105),
                    total_page_compression_attempts: Some(106),
                    failed_page_compression_attempts: Some(107),
                    total_page_decompressions: Some(108),
                    compressed_page_evictions: Some(109),
                    eager_page_compressions: Some(110),
                    memory_pressure_page_compressions: Some(111),
                    critical_memory_page_compressions: Some(112),
                    pages_decompressed_unit_ns: Some(113),
                    pages_decompressed_within_log_time: Some([0, 1, 2, 3, 4, 5, 6, 7]),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            bucket_definitions: Some(vec![fplugin::BucketDefinition {
                name: Some("abc".to_string()),
                process: None,
                vmo: Some("def".to_string()),
                ..Default::default()
            }]),
            performance_metrics: Some(fplugin::PerformanceImpactMetrics {
                some_memory_stalls_ns: Some(33),
                full_memory_stalls_ns: Some(44),
                page_refaults: Some(55),
                ..Default::default()
            }),
            ..Default::default()
        };
        let expected_snapshot_json = r#"
            {
                "attributions": [
                    [
                        0,
                        0,
                        [
                            {
                                "kernel_object": 1000
                            }
                        ]
                    ],
                    [
                        0,
                        1,
                        [
                            {
                                "kernel_object": 1004
                            }
                        ]
                    ],
                    [
                        0,
                        2,
                        [
                            {
                                "kernel_object": 1008
                            }
                        ]
                    ],
                    [
                        1,
                        3,
                        [
                            {
                                "kernel_object": 1007
                            }
                        ]
                    ]
                ],
                "bucket_definitions": [
                    {
                        "name": "abc",
                        "process": null,
                        "vmo": "def"
                    }
                ],
                "kernel_statistics": {
                    "compression_stats": {
                        "compressed_fragmentation_bytes": 103,
                        "compressed_page_evictions": 109,
                        "compressed_storage_bytes": 102,
                        "compression_time": 104,
                        "critical_memory_page_compressions": 112,
                        "decompression_time": 105,
                        "eager_page_compressions": 110,
                        "failed_page_compression_attempts": 107,
                        "memory_pressure_page_compressions": 111,
                        "pages_decompressed_unit_ns": 113,
                        "pages_decompressed_within_log_time": [
                            0,
                            1,
                            2,
                            3,
                            4,
                            5,
                            6,
                            7
                        ],
                        "total_page_compression_attempts": 106,
                        "total_page_decompressions": 108,
                        "uncompressed_storage_bytes": 101
                    },
                    "memory_stats": {
                        "cache_bytes": 10,
                        "free_bytes": 2,
                        "free_heap_bytes": 6,
                        "free_loaned_bytes": 3,
                        "ipc_bytes": 9,
                        "mmu_overhead_bytes": 8,
                        "other_bytes": 13,
                        "slab_bytes": 11,
                        "total_bytes": 1,
                        "total_heap_bytes": 5,
                        "vmo_bytes": 7,
                        "vmo_discardable_locked_bytes": 18,
                        "vmo_discardable_unlocked_bytes": 19,
                        "vmo_reclaim_disabled_bytes": 17,
                        "vmo_reclaim_newest_bytes": 15,
                        "vmo_reclaim_oldest_bytes": 16,
                        "vmo_reclaim_total_bytes": 14,
                        "wired_bytes": 4,
                        "zram_bytes": 12
                    }
                },
                "performance_metrics": {
                    "full_memory_stalls_ns": 44,
                    "some_memory_stalls_ns": 33,
                    "page_refaults": 55
                },
                "principals": [
                    [
                        0,
                        {
                            "component": "root"
                        },
                        "runnable",
                        null
                    ],
                    [
                        1,
                        {
                            "component": "runner"
                        },
                        "part",
                        0
                    ],
                    [
                        2,
                        {
                            "component": "component 2"
                        },
                        "runnable",
                        0
                    ],
                    [
                        3,
                        {
                            "component": "component 3"
                        },
                        "runnable",
                        1
                    ]
                ],
                "resource_names": [
                    "root_job",
                    "root_process",
                    "root_vmo",
                    "shared_vmo",
                    "runner_job",
                    "runner_process",
                    "runner_vmo",
                    "component_vmo",
                    "component_2_job",
                    "2_process",
                    "2_vmo"
                ],
                "resources": [
                    [
                        1000,
                        0,
                        {
                            "job": [
                                [
                                    1004,
                                    1008
                                ],
                                [
                                    1001
                                ]
                            ]
                        }
                    ],
                    [
                        1001,
                        1,
                        {
                            "process": [
                                [
                                    1002,
                                    1003
                                ],
                                null
                            ]
                        }
                    ],
                    [
                        1002,
                        2,
                        {
                            "vmo": [
                                null,
                                1024,
                                2048,
                                1024,
                                2048,
                                1024,
                                2048,
                                1
                            ]
                        }
                    ],
                    [
                        1003,
                        3,
                        {
                            "vmo": [
                                null,
                                1024,
                                2048,
                                1024,
                                2048,
                                1024,
                                2048,
                                0
                            ]
                        }
                    ],
                    [
                        1004,
                        4,
                        {
                            "job": [
                                [],
                                [
                                    1005
                                ]
                            ]
                        }
                    ],
                    [
                        1005,
                        5,
                        {
                            "process": [
                                [
                                    1006,
                                    1007
                                ],
                                null
                            ]
                        }
                    ],
                    [
                        1006,
                        6,
                        {
                            "vmo": [
                                null,
                                1024,
                                2048,
                                1024,
                                2048,
                                1024,
                                2048,
                                0
                            ]
                        }
                    ],
                    [
                        1007,
                        7,
                        {
                            "vmo": [
                                null,
                                1024,
                                2048,
                                1024,
                                2048,
                                1024,
                                2048,
                                0
                            ]
                        }
                    ],
                    [
                        1008,
                        8,
                        {
                            "job": [
                                [],
                                [
                                    1009
                                ]
                            ]
                        }
                    ],
                    [
                        1009,
                        9,
                        {
                            "process": [
                                [
                                    1010,
                                    1003
                                ],
                                null
                            ]
                        }
                    ],
                    [
                        1010,
                        10,
                        {
                            "vmo": [
                                null,
                                1024,
                                2048,
                                1024,
                                2048,
                                1024,
                                2048,
                                0
                            ]
                        }
                    ]
                ]
            }
        "#;
        let actual_json_value = example_snapshot.to_json();
        assert_eq!(
            actual_json_value,
            serde_json::from_str::<Value>(expected_snapshot_json).unwrap(),
            "`actual_json_value` json was:\n======\n{}\n\n======",
            actual_json_value
        );

        let actual_snapshot = fplugin::Snapshot::from_json(&actual_json_value)
            .context("Unable to deserialize snapshot")
            .unwrap();

        assert_eq!(
            example_snapshot,
            actual_snapshot,
            "`actual_snapshot` json was:\n======\n{}\n\n======",
            actual_snapshot.to_json()
        );
    }
}
