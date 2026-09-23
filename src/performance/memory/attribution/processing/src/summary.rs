// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::digest::Digest;
use crate::macros::vmo_digests;
use crate::{
    Claim, GlobalPrincipalIdentifier, InflatedPrincipal, InflatedResource, PrincipalType, ZXName,
    fplugin_serde,
};
use bstr::ByteSlice;
use core::default::Default;
use fidl_fuchsia_memory_attribution_plugin_common as fplugin;
use fplugin::Vmo;
#[cfg(target_os = "fuchsia")]
use fuchsia_trace::duration;
use rustc_hash::FxHashMap;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::fmt::Display;

/// Consider that two floats are equals if they differ less than [FLOAT_COMPARISON_EPSILON].
const FLOAT_COMPARISON_EPSILON: f64 = 1e-10;

#[derive(Debug, Default, PartialEq, Serialize)]
pub struct ComponentSummaryProfileResult {
    pub kernel: fplugin_serde::KernelStatistics,
    pub principals: Vec<PrincipalSummary>,
    /// Amount, in bytes, of memory that is known but remained unclaimed. Should be equal to zero.
    pub unclaimed: u64,
    #[serde(with = "fplugin_serde::PerformanceImpactMetricsDef")]
    pub performance: fplugin::PerformanceImpactMetrics,
    pub digest: Option<Digest>,
}

/// Summary view of the memory usage on a device.
///
/// This view aggregates the memory usage for each Principal, and, for each Principal, for VMOs
/// sharing the same name or belonging to the same logical group. This is a view appropriate to
/// display to developers who want to understand the memory usage of their Principal.
#[derive(Debug, PartialEq, Serialize)]
pub struct MemorySummary {
    pub principals: Vec<PrincipalSummary>,
    /// Amount, in bytes, of memory that is known but remained unclaimed. Should be equal to zero.
    pub unclaimed: u64,
}

fn compute_share_count(
    claims: &HashSet<Claim>,
    subjects_buf: &mut Vec<GlobalPrincipalIdentifier>,
) -> usize {
    match claims.len() {
        0 => 0,
        1 => 1,
        2 => {
            let mut iter = claims.iter();
            let s1 = iter.next().unwrap().subject;
            let s2 = iter.next().unwrap().subject;
            if s1 == s2 { 1 } else { 2 }
        }
        _ => {
            subjects_buf.clear();
            subjects_buf.extend(claims.iter().map(|c| c.subject));
            subjects_buf.sort_unstable();
            subjects_buf.dedup();
            subjects_buf.len()
        }
    }
}

impl MemorySummary {
    pub(crate) fn build(
        principals: &FxHashMap<GlobalPrincipalIdentifier, InflatedPrincipal>,
        resources: &FxHashMap<u64, InflatedResource>,
        resource_names: &Vec<ZXName>,
    ) -> MemorySummary {
        #[cfg(target_os = "fuchsia")]
        duration!(crate::CATEGORY_MEMORY_CAPTURE, c"MemorySummary::build");
        let digested_names: Vec<&ZXName> =
            resource_names.iter().map(vmo_name_to_digest_zxname).collect();
        let mut subjects_buf = Vec::new();
        let share_counts: FxHashMap<u64, usize> = resources
            .iter()
            .map(|(&koid, resource)| {
                (koid, compute_share_count(&resource.claims, &mut subjects_buf))
            })
            .collect();

        let mut output = MemorySummary { principals: Default::default(), unclaimed: 0 };
        for principal in principals.values() {
            output.principals.push(MemorySummary::build_one_principal(
                &principal,
                &principals,
                &resources,
                &resource_names,
                &digested_names,
                &share_counts,
            ));
        }

        output.principals.sort_unstable_by(|a, b| b.populated_total.cmp(&a.populated_total));

        let mut unclaimed = 0;
        for (_, resource) in resources {
            if resource.claims.is_empty() {
                match &resource.resource.resource_type {
                    fplugin::ResourceType::Job(_) | fplugin::ResourceType::Process(_) => {}
                    fplugin::ResourceType::Vmo(vmo) => {
                        unclaimed += vmo.scaled_populated_bytes.unwrap();
                    }
                    _ => todo!(),
                }
            }
        }
        output.unclaimed = unclaimed;
        output
    }

    fn build_one_principal(
        principal: &InflatedPrincipal,
        principals: &FxHashMap<GlobalPrincipalIdentifier, InflatedPrincipal>,
        resources: &FxHashMap<u64, InflatedResource>,
        resource_names: &Vec<ZXName>,
        digested_names: &[&ZXName],
        share_counts: &FxHashMap<u64, usize>,
    ) -> PrincipalSummary {
        let mut output = PrincipalSummary {
            name: principal.name().to_owned(),
            id: principal.principal.identifier.0.into(),
            principal_type: match &principal.principal.principal_type {
                PrincipalType::Runnable => "R",
                PrincipalType::Part => "P",
            }
            .to_owned(),
            committed_private: 0,
            committed_scaled: 0.0,
            committed_total: 0,
            populated_private: 0,
            populated_scaled: 0.0,
            populated_total: 0,
            attributor: principal
                .principal
                .parent
                .as_ref()
                .and_then(|p| principals.get(p))
                .map(|p| p.name().to_owned()),
            processes: Vec::new(),
            vmos: HashMap::new(),
        };

        for resource_id in &principal.resources {
            let Some(resource) = resources.get(resource_id) else {
                continue;
            };
            let share_count = *share_counts.get(resource_id).unwrap();
            match &resource.resource.resource_type {
                fplugin::ResourceType::Job(_) => todo!(),
                fplugin::ResourceType::Process(_) => {
                    output.processes.push(format!(
                        "{} ({})",
                        resource_names.get(resource.resource.name_index).unwrap(),
                        resource.resource.koid
                    ));
                }
                fplugin::ResourceType::Vmo(vmo_info) => {
                    output.committed_total += vmo_info.total_committed_bytes.unwrap();
                    output.populated_total += vmo_info.total_populated_bytes.unwrap();
                    output.committed_scaled +=
                        vmo_info.scaled_committed_bytes.unwrap() as f64 / share_count as f64;
                    output.populated_scaled +=
                        vmo_info.scaled_populated_bytes.unwrap() as f64 / share_count as f64;
                    if share_count == 1 {
                        output.committed_private += vmo_info.private_committed_bytes.unwrap();
                        output.populated_private += vmo_info.private_populated_bytes.unwrap();
                    }
                    let digest_name = digested_names[resource.resource.name_index];
                    // This avoids using .entry(), which forces us to clone the key even when the
                    // entry already exists.
                    if let Some(summary) = output.vmos.get_mut(digest_name) {
                        summary.merge(vmo_info, share_count);
                    } else {
                        let mut summary = VmoSummary::default();
                        summary.merge(vmo_info, share_count);
                        output.vmos.insert(digest_name.clone(), summary);
                    }
                }
                _ => todo!(),
            }
        }

        for process_mapped in &principal.mapped_processes {
            if let Some(process) = resources.get(process_mapped) {
                output.processes.push(format!(
                    "{} ({})",
                    resource_names.get(process.resource.name_index).unwrap(),
                    process.resource.koid
                ));
            }
        }

        output.processes.sort();
        output
    }
}

impl Display for MemorySummary {
    fn fmt(&self, _f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Ok(())
    }
}

/// Summary of a Principal memory usage, and its breakdown per VMO group.
#[derive(Debug, Serialize)]
pub struct PrincipalSummary {
    /// Identifier for the Principal. This number is not meaningful outside of the memory
    /// attribution system.
    pub id: u64,
    /// Display name of the Principal.
    pub name: String,
    /// Type of the Principal.
    pub principal_type: String,
    /// Number of committed private bytes of the Principal.
    pub committed_private: u64,
    /// Number of committed bytes of all VMOs accessible to the Principal, scaled by the number of
    /// Principals that can access them.
    pub committed_scaled: f64,
    /// Total number of committed bytes of all the VMOs accessible to the Principal.
    pub committed_total: u64,
    /// Number of populated private bytes of the Principal.
    pub populated_private: u64,
    /// Number of populated bytes of all VMOs accessible to the Principal, scaled by the number of
    /// Principals that can access them.
    pub populated_scaled: f64,
    /// Total number of populated bytes of all the VMOs accessible to the Principal.
    pub populated_total: u64,
    /// Name of the Principal who gave attribution information for this Principal.
    pub attributor: Option<String>,
    /// List of Zircon processes attributed (even partially) to this Principal.
    pub processes: Vec<String>,
    /// Summary of memory usage for the VMOs accessible to this Principal, grouped by VMO name.
    pub vmos: HashMap<ZXName, VmoSummary>,
}

impl PartialEq for PrincipalSummary {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.name == other.name
            && self.principal_type == other.principal_type
            && self.committed_private == other.committed_private
            && (self.committed_scaled - other.committed_scaled).abs() < FLOAT_COMPARISON_EPSILON
            && self.committed_total == other.committed_total
            && self.populated_private == other.populated_private
            && (self.populated_scaled - other.populated_scaled).abs() < FLOAT_COMPARISON_EPSILON
            && self.populated_total == other.populated_total
            && self.attributor == other.attributor
            && self.processes == other.processes
            && self.vmos == other.vmos
    }
}

/// Group of VMOs sharing the same name.
#[derive(Default, Debug, Serialize)]
pub struct VmoSummary {
    /// Number of distinct VMOs under the same name.
    pub count: u64,
    /// Number of committed bytes of this VMO group only accessible by the Principal this group
    /// belongs.
    pub committed_private: u64,
    /// Number of committed bytes of this VMO group, scaled by the number of Principals that can
    /// access them.
    pub committed_scaled: f64,
    /// Total number of committed bytes of this VMO group.
    pub committed_total: u64,
    /// Number of populated bytes of this VMO group only accessible by the Principal this group
    /// belongs.
    pub populated_private: u64,
    /// Number of populated bytes of this VMO group, scaled by the number of Principals that can
    /// access them.
    pub populated_scaled: f64,
    /// Total number of populated bytes of this VMO group.
    pub populated_total: u64,
}

impl VmoSummary {
    fn merge(&mut self, vmo_info: &Vmo, share_count: usize) {
        self.count += 1;
        self.committed_total += vmo_info.total_committed_bytes.unwrap();
        self.populated_total += vmo_info.total_populated_bytes.unwrap();
        self.committed_scaled +=
            vmo_info.scaled_committed_bytes.unwrap() as f64 / share_count as f64;
        self.populated_scaled +=
            vmo_info.scaled_populated_bytes.unwrap() as f64 / share_count as f64;
        if share_count == 1 {
            self.committed_private += vmo_info.private_committed_bytes.unwrap();
            self.populated_private += vmo_info.private_populated_bytes.unwrap();
        }
    }
}

impl PartialEq for VmoSummary {
    fn eq(&self, other: &Self) -> bool {
        self.count == other.count
            && self.committed_private == other.committed_private
            && (self.committed_scaled - other.committed_scaled).abs() < FLOAT_COMPARISON_EPSILON
            && self.committed_total == other.committed_total
            && self.populated_private == other.populated_private
            && (self.populated_scaled - other.populated_scaled).abs() < FLOAT_COMPARISON_EPSILON
            && self.populated_total == other.populated_total
    }
}
vmo_digests! {
    (
        ProcessBootstrap,
        "[process-bootstrap]",
        or(contains("ld.so.1-internal-heap"), starts_with("stack: msg of"))
    ),
    (Blobs, "[blobs]", exact("blob-", hex())),
    (InactiveBlobs, "[inactive blobs]", exact("inactive-blob-", hex())),
    (
        Stacks,
        "[stacks]",
        or(
            starts_with("thrd_t:0x"),
            contains("initial-thread"),
            contains("pthread_t:0x"),
            contains("pthread_create:0x")
        )
    ),
    (Data, "[data]", starts_with("data", digits(), ":")),
    (Bss, "[bss]", starts_with("bss", digits(), ":")),
    (Relro, "[relro]", starts_with("relro:")),
    (Unnamed, "[unnamed]", exact("")),
    (Scudo, "[scudo]", starts_with("scudo:")),
    (BootfsLibraries, "[bootfs-libraries]", contains(".so")),
    (BionicStack, "[bionic-stack]", starts_with("stack_and_tls:")),
    (Ext4, "[ext4]", starts_with("ext4!")),
    (Dalvik, "[dalvik]", starts_with("dalvik-")),
    (Bootfs, "[bootfs]", or(exact("bootfs"), starts_with("bootfs:"))),
    (RestrictedStateVmo, "[restricted_state_vmo]", exact("restricted_state_vmo:", digits())),
}

#[cfg(test)]
const VMO_DIGEST_NAME_MAPPING: [(&str, &str); 15] = [
    ("ld\\.so\\.1-internal-heap|(^stack: msg of.*)", "[process-bootstrap]"),
    ("^blob-[0-9a-f]+$", "[blobs]"),
    ("^inactive-blob-[0-9a-f]+$", "[inactive blobs]"),
    ("^thrd_t:0x.*|initial-thread|pthread_(t|create):0x.*$", "[stacks]"),
    ("^data[0-9]*:.*$", "[data]"),
    ("^bss[0-9]*:.*$", "[bss]"),
    ("^relro:.*$", "[relro]"),
    ("^$", "[unnamed]"),
    ("^scudo:.*$", "[scudo]"),
    ("^.*\\.so.*$", "[bootfs-libraries]"),
    ("^stack_and_tls:.*$", "[bionic-stack]"),
    ("^ext4!.*$", "[ext4]"),
    ("^dalvik-.*$", "[dalvik]"),
    ("^bootfs(:.*)?$", "[bootfs]"),
    ("^restricted_state_vmo:[0-9]+$", "[restricted_state_vmo]"),
];

/// Returns the name of a VMO category when the name matches one of the rules.
/// This is used for presentation and aggregation.
pub fn vmo_name_to_digest_name(name: &str) -> &str {
    if let Some(category) = match_vmo_digest(name.trim()) { category.as_str() } else { name }
}

pub fn vmo_name_to_digest_zxname(name: &ZXName) -> &ZXName {
    if let Ok(name_str) = name.as_bstr().to_str() {
        if let Some(category) = match_vmo_digest(name_str) {
            return category.as_zxname();
        }
    }
    name
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Claim, ClaimType, GlobalPrincipalIdentifier, InflatedPrincipal, InflatedResource};

    #[test]
    fn rename_zx_test() {
        pretty_assertions::assert_eq!(
            vmo_name_to_digest_zxname(&ZXName::from_string_lossy("ld.so.1-internal-heap")),
            &ZXName::from_string_lossy("[process-bootstrap]"),
        );
    }

    #[test]
    fn rename_zx_test_small_name() {
        // Verify that we can match regular expressions anchored at both ends even when the name is
        // not taking the full size of a [ZXName].
        pretty_assertions::assert_eq!(
            vmo_name_to_digest_zxname(&ZXName::from_string_lossy("blob-1234")),
            &ZXName::from_string_lossy("[blobs]"),
        );
    }

    #[test]
    fn rename_test() {
        pretty_assertions::assert_eq!(
            vmo_name_to_digest_name("ld.so.1-internal-heap"),
            "[process-bootstrap]"
        );
        pretty_assertions::assert_eq!(
            vmo_name_to_digest_name("stack: msg of 123"),
            "[process-bootstrap]"
        );
        pretty_assertions::assert_eq!(vmo_name_to_digest_name("blob-123"), "[blobs]");
        pretty_assertions::assert_eq!(vmo_name_to_digest_name("blob-15e0da8e"), "[blobs]");
        pretty_assertions::assert_eq!(
            vmo_name_to_digest_name("inactive-blob-123"),
            "[inactive blobs]"
        );
        pretty_assertions::assert_eq!(vmo_name_to_digest_name("thrd_t:0x123"), "[stacks]");
        pretty_assertions::assert_eq!(vmo_name_to_digest_name("initial-thread"), "[stacks]");
        pretty_assertions::assert_eq!(vmo_name_to_digest_name("pthread_t:0x123"), "[stacks]");
        pretty_assertions::assert_eq!(
            vmo_name_to_digest_name("pthread_create:0xfa124714"),
            "[stacks]"
        );
        pretty_assertions::assert_eq!(vmo_name_to_digest_name("data456:"), "[data]");
        pretty_assertions::assert_eq!(vmo_name_to_digest_name("bss456:"), "[bss]");
        pretty_assertions::assert_eq!(vmo_name_to_digest_name("relro:foobar"), "[relro]");
        pretty_assertions::assert_eq!(vmo_name_to_digest_name(""), "[unnamed]");
        pretty_assertions::assert_eq!(vmo_name_to_digest_name("scudo:primary"), "[scudo]");
        pretty_assertions::assert_eq!(vmo_name_to_digest_name("libfoo.so.1"), "[bootfs-libraries]");
        pretty_assertions::assert_eq!(vmo_name_to_digest_name("foobar"), "foobar");
        pretty_assertions::assert_eq!(
            vmo_name_to_digest_name("stack_and_tls:2331"),
            "[bionic-stack]"
        );
        pretty_assertions::assert_eq!(vmo_name_to_digest_name("ext4!foobar"), "[ext4]");
        pretty_assertions::assert_eq!(vmo_name_to_digest_name("dalvik-data1234"), "[dalvik]");
        pretty_assertions::assert_eq!(
            vmo_name_to_digest_name("restricted_state_vmo:119723"),
            "[restricted_state_vmo]"
        );
    }

    // Verifies that the fast string matching rules match the regex rules.
    #[test]
    fn test_vmo_digest_rules_match_regex() {
        let test_strings = [
            "ld.so.1-internal-heap",
            "prefix-ld.so.1-internal-heap",
            "stack: msg of something",
            "stack: msg of",
            "stack: msg",
            "blob-1234",
            "blob-abcdef",
            "blob-0123456789abcdef",
            "blob-",
            "blob-123g",
            "blob-ABC",
            "inactive-blob-1234",
            "inactive-blob-abcdef",
            "inactive-blob-",
            "inactive-blob-xyz",
            "thrd_t:0x123",
            "thrd_t:0x",
            "prefix-thrd_t:0x123",
            "initial-thread",
            "prefix-initial-thread-suffix",
            "pthread_t:0x123",
            "pthread_create:0xfa124714",
            "data:",
            "data0:",
            "data123:foo",
            "data:bar",
            "data_foo:",
            "data",
            "bss:",
            "bss99:",
            "bss456:bar",
            "bss_foo:",
            "bss",
            "relro:",
            "relro:foo",
            "relro_other",
            "",
            "scudo:",
            "scudo:primary",
            "scudo_other",
            "libfoo.so.1",
            "test.so",
            ".so",
            "stack_and_tls:123",
            "stack_and_tls:",
            "ext4!foobar",
            "ext4!",
            "dalvik-data",
            "dalvik-",
            "bootfs",
            "bootfs:",
            "bootfs:bin",
            "bootfs_other",
            "restricted_state_vmo:",
            "restricted_state_vmo:0",
            "restricted_state_vmo:12345",
            "restricted_state_vmo:abc",
            "restricted_state_vmo:12a",
            "foobar",
            "random_string_123",
            "other-blob-1234",
        ];

        static RULES: std::sync::LazyLock<Vec<(regex_lite::Regex, &'static str)>> =
            std::sync::LazyLock::new(|| {
                VMO_DIGEST_NAME_MAPPING
                    .iter()
                    .map(|&(pattern, replacement)| {
                        (regex_lite::Regex::new(pattern).unwrap(), replacement)
                    })
                    .collect()
            });

        for s in test_strings {
            let expected =
                RULES.iter().find(|(regex, _)| regex.is_match(s)).map_or(s, |rule| rule.1);
            let actual = vmo_name_to_digest_name(s);
            assert_eq!(actual, expected, "Mismatch for string: {:?}", s);

            let zx_in = ZXName::from_string_lossy(s);
            let zx_expected = ZXName::from_string_lossy(expected);
            let zx_actual = vmo_name_to_digest_zxname(&zx_in);
            assert_eq!(zx_actual, &zx_expected, "ZXName mismatch for string: {:?}", s);
        }
    }

    fn make_test_principal(id: u64, name: &str) -> InflatedPrincipal {
        InflatedPrincipal::new(
            fplugin::Principal {
                identifier: Some(fplugin::PrincipalIdentifier { id }),
                description: Some(fplugin::Description::Component(name.to_owned())),
                principal_type: Some(fplugin::PrincipalType::Runnable),
                parent: None,
                ..Default::default()
            }
            .into(),
        )
    }

    fn make_test_vmo_resource(
        koid: u64,
        name_index: usize,
        committed: u64,
        populated: u64,
        claims: Vec<(u64, u64)>,
    ) -> InflatedResource {
        let mut res = InflatedResource::new(
            fplugin::Resource {
                koid: Some(koid),
                name_index: Some(name_index as u64),
                resource_type: Some(fplugin::ResourceType::Vmo(fplugin::Vmo {
                    private_committed_bytes: Some(committed),
                    private_populated_bytes: Some(populated),
                    scaled_committed_bytes: Some(committed),
                    scaled_populated_bytes: Some(populated),
                    total_committed_bytes: Some(committed),
                    total_populated_bytes: Some(populated),
                    ..Default::default()
                })),
                ..Default::default()
            }
            .into(),
        );
        for (source, subject) in claims {
            res.claims.insert(Claim {
                source: GlobalPrincipalIdentifier::new_for_test(source),
                subject: GlobalPrincipalIdentifier::new_for_test(subject),
                claim_type: ClaimType::Direct,
            });
        }
        res
    }

    /// What is tested: `MemorySummary::build` sorting of `PrincipalSummary` entries by
    /// `populated_total` in descending order.
    ///
    /// Expectations verified:
    /// - Principals in `summary.principals` are ordered descending by their total populated bytes
    ///   (`1_000_000_000` -> `500_000_000` -> `100_000_000`).
    /// - Verifies that large unsigned byte totals are handled correctly without sign-overflow when
    ///   sorting comparator logic is refactored.
    #[test]
    fn test_memory_summary_build_sorting_and_overflow() {
        let mut principals = FxHashMap::default();
        let mut p1 = make_test_principal(1, "small_principal");
        p1.resources.push(101);
        let mut p2 = make_test_principal(2, "large_principal");
        p2.resources.push(102);
        let mut p3 = make_test_principal(3, "medium_principal");
        p3.resources.push(103);
        principals.insert(GlobalPrincipalIdentifier::new_for_test(1), p1);
        principals.insert(GlobalPrincipalIdentifier::new_for_test(2), p2);
        principals.insert(GlobalPrincipalIdentifier::new_for_test(3), p3);

        let mut resources = FxHashMap::default();
        resources
            .insert(101, make_test_vmo_resource(101, 0, 100_000_000, 100_000_000, vec![(1, 1)]));
        resources.insert(
            102,
            make_test_vmo_resource(102, 1, 1_000_000_000, 1_000_000_000, vec![(2, 2)]),
        );
        resources
            .insert(103, make_test_vmo_resource(103, 2, 500_000_000, 500_000_000, vec![(3, 3)]));

        let resource_names = vec![
            ZXName::from_string_lossy("vmo_1"),
            ZXName::from_string_lossy("vmo_2"),
            ZXName::from_string_lossy("vmo_3"),
        ];

        let summary = MemorySummary::build(&principals, &resources, &resource_names);
        assert_eq!(summary.principals.len(), 3);
        assert_eq!(summary.principals[0].name, "large_principal");
        assert_eq!(summary.principals[0].populated_total, 1_000_000_000);
        assert_eq!(summary.principals[1].name, "medium_principal");
        assert_eq!(summary.principals[1].populated_total, 500_000_000);
        assert_eq!(summary.principals[2].name, "small_principal");
        assert_eq!(summary.principals[2].populated_total, 100_000_000);
    }

    /// What is tested: VMO digest aggregation and merging when they have the same name.
    ///
    /// Expectations verified:
    /// - When multiple VMOs owned by a principal have distinct names ("blob-1111", "blob-2222")
    ///   that digest to the same bucket ("[blobs]"), they are merged into a single `VmoSummary`
    ///   entry.
    /// - Verifies `vmo_summary.count == 2` and that all committed/populated byte metrics (total and
    ///   private) are accurately summed across the aggregated VMOs.
    #[test]
    fn test_memory_summary_vmo_digest_aggregation() {
        let mut principals = FxHashMap::default();
        let mut p1 = make_test_principal(1, "blob_owner");
        p1.resources.push(1001);
        p1.resources.push(1002);
        principals.insert(GlobalPrincipalIdentifier::new_for_test(1), p1);

        let mut resources = FxHashMap::default();
        resources.insert(1001, make_test_vmo_resource(1001, 0, 100, 200, vec![(1, 1)]));
        resources.insert(1002, make_test_vmo_resource(1002, 1, 300, 400, vec![(1, 1)]));

        let resource_names =
            vec![ZXName::from_string_lossy("blob-1111"), ZXName::from_string_lossy("blob-2222")];

        let summary = MemorySummary::build(&principals, &resources, &resource_names);
        assert_eq!(summary.principals.len(), 1);
        let p_summary = &summary.principals[0];
        assert_eq!(p_summary.vmos.len(), 1);

        let blob_digest = ZXName::from_string_lossy("[blobs]");
        let vmo_summary = p_summary.vmos.get(&blob_digest).expect("Should aggregate under [blobs]");
        assert_eq!(vmo_summary.count, 2);
        assert_eq!(vmo_summary.committed_total, 400);
        assert_eq!(vmo_summary.populated_total, 600);
        assert_eq!(vmo_summary.committed_private, 400);
        assert_eq!(vmo_summary.populated_private, 600);
    }

    /// What is tested: Process formatting and alphabetical sorting of process strings in
    /// `PrincipalSummary.processes`.
    ///
    /// Expectations verified:
    /// - Multiple distinct process resources attributed to a principal are formatted as `"name
    ///   (koid)"` and sorted alphabetically (`"alpha_process (2002)"` before `"zeta_process (2001)
    ///   "`).
    #[test]
    fn test_memory_summary_process_formatting_and_sorting() {
        let mut principals = FxHashMap::default();
        let mut p1 = make_test_principal(1, "proc_owner");
        p1.resources.push(2001);
        p1.resources.push(2002);
        principals.insert(GlobalPrincipalIdentifier::new_for_test(1), p1);

        let mut resources = FxHashMap::default();
        let r1 = InflatedResource::new(
            fplugin::Resource {
                koid: Some(2001),
                name_index: Some(0),
                resource_type: Some(fplugin::ResourceType::Process(fplugin::Process {
                    vmos: Some(vec![]),
                    mappings: None,
                    ..Default::default()
                })),
                ..Default::default()
            }
            .into(),
        );
        let r2 = InflatedResource::new(
            fplugin::Resource {
                koid: Some(2002),
                name_index: Some(1),
                resource_type: Some(fplugin::ResourceType::Process(fplugin::Process {
                    vmos: Some(vec![]),
                    mappings: None,
                    ..Default::default()
                })),
                ..Default::default()
            }
            .into(),
        );
        resources.insert(2001, r1);
        resources.insert(2002, r2);

        let resource_names = vec![
            ZXName::from_string_lossy("zeta_process"),
            ZXName::from_string_lossy("alpha_process"),
        ];

        let summary = MemorySummary::build(&principals, &resources, &resource_names);
        assert_eq!(summary.principals.len(), 1);
        assert_eq!(
            summary.principals[0].processes,
            vec!["alpha_process (2002)".to_owned(), "zeta_process (2001)".to_owned()]
        );
    }

    /// What is tested: `share_count` division and private vs. scaled memory calculations when a VMO
    /// is shared across multiple principals.
    ///
    /// Expectations verified:
    /// - When a VMO is shared among 2 distinct principals (`share_count == 2`), scaled bytes equal
    ///   `total / 2.0`.
    /// - Because `share_count > 1`, `committed_private` and `populated_private` are exactly 0 for
    ///   both sharing principals.
    #[test]
    fn test_memory_summary_share_count_calculation() {
        let mut principals = FxHashMap::default();
        let mut p1 = make_test_principal(1, "owner1");
        let mut p2 = make_test_principal(2, "owner2");
        p1.resources.push(3001);
        p2.resources.push(3001);
        principals.insert(GlobalPrincipalIdentifier::new_for_test(1), p1);
        principals.insert(GlobalPrincipalIdentifier::new_for_test(2), p2);

        let mut resources = FxHashMap::default();
        resources.insert(3001, make_test_vmo_resource(3001, 0, 1000, 2000, vec![(1, 1), (2, 2)]));

        let resource_names = vec![ZXName::from_string_lossy("shared_mem")];
        let summary = MemorySummary::build(&principals, &resources, &resource_names);

        assert_eq!(summary.principals.len(), 2);
        for p_sum in &summary.principals {
            assert_eq!(p_sum.committed_total, 1000);
            assert_eq!(p_sum.populated_total, 2000);
            assert_eq!(p_sum.committed_scaled, 500.0);
            assert_eq!(p_sum.populated_scaled, 1000.0);
            assert_eq!(p_sum.committed_private, 0);
            assert_eq!(p_sum.populated_private, 0);
        }
    }

    /// What is tested: Aggregation of unclaimed VMOs (VMO resources with an empty claims list) into
    /// `MemorySummary.unclaimed`.
    ///
    /// Expectations verified:
    /// - A VMO with no attribution claims has its `scaled_populated_bytes` added to `summary.
    ///   unclaimed`.
    #[test]
    fn test_memory_summary_unclaimed_vmos() {
        let principals = FxHashMap::default();
        let mut resources = FxHashMap::default();
        resources.insert(4001, make_test_vmo_resource(4001, 0, 500, 1234, vec![]));

        let resource_names = vec![ZXName::from_string_lossy("unclaimed_vmo")];
        let summary = MemorySummary::build(&principals, &resources, &resource_names);
        assert_eq!(summary.unclaimed, 1234);
    }

    /// What is tested: `compute_share_count` properly deduplicates subjects.
    #[test]
    fn test_compute_share_count() {
        let mut subjects_buf = Vec::new();

        let empty_claims = HashSet::new();
        assert_eq!(compute_share_count(&empty_claims, &mut subjects_buf), 0);

        let mut single_claim = HashSet::new();
        single_claim.insert(Claim {
            source: GlobalPrincipalIdentifier::new_for_test(1),
            subject: GlobalPrincipalIdentifier::new_for_test(1),
            claim_type: ClaimType::Direct,
        });
        assert_eq!(compute_share_count(&single_claim, &mut subjects_buf), 1);

        let mut two_same_subject = HashSet::new();
        two_same_subject.insert(Claim {
            source: GlobalPrincipalIdentifier::new_for_test(1),
            subject: GlobalPrincipalIdentifier::new_for_test(10),
            claim_type: ClaimType::Direct,
        });
        two_same_subject.insert(Claim {
            source: GlobalPrincipalIdentifier::new_for_test(2),
            subject: GlobalPrincipalIdentifier::new_for_test(10),
            claim_type: ClaimType::Indirect,
        });
        assert_eq!(compute_share_count(&two_same_subject, &mut subjects_buf), 1);

        let mut two_diff_subject = HashSet::new();
        two_diff_subject.insert(Claim {
            source: GlobalPrincipalIdentifier::new_for_test(1),
            subject: GlobalPrincipalIdentifier::new_for_test(10),
            claim_type: ClaimType::Direct,
        });
        two_diff_subject.insert(Claim {
            source: GlobalPrincipalIdentifier::new_for_test(2),
            subject: GlobalPrincipalIdentifier::new_for_test(20),
            claim_type: ClaimType::Direct,
        });
        assert_eq!(compute_share_count(&two_diff_subject, &mut subjects_buf), 2);

        let mut multi_claims = HashSet::new();
        multi_claims.insert(Claim {
            source: GlobalPrincipalIdentifier::new_for_test(1),
            subject: GlobalPrincipalIdentifier::new_for_test(10),
            claim_type: ClaimType::Direct,
        });
        multi_claims.insert(Claim {
            source: GlobalPrincipalIdentifier::new_for_test(2),
            subject: GlobalPrincipalIdentifier::new_for_test(10),
            claim_type: ClaimType::Indirect,
        });
        multi_claims.insert(Claim {
            source: GlobalPrincipalIdentifier::new_for_test(3),
            subject: GlobalPrincipalIdentifier::new_for_test(20),
            claim_type: ClaimType::Direct,
        });
        multi_claims.insert(Claim {
            source: GlobalPrincipalIdentifier::new_for_test(4),
            subject: GlobalPrincipalIdentifier::new_for_test(30),
            claim_type: ClaimType::Direct,
        });
        assert_eq!(compute_share_count(&multi_claims, &mut subjects_buf), 3);
    }

    /// What is tested: `MemorySummary::build` scaling when a VMO has multiple claims from the same
    /// principal as well as distinct principals.
    ///
    /// Expectations verified:
    /// - 3 claims across 2 distinct principals -> `share_count == 2`.
    /// - Scaled bytes are divided by 2.0.
    #[test]
    fn test_memory_summary_share_count_multi_and_duplicate_claims() {
        let mut principals = FxHashMap::default();
        let mut p1 = make_test_principal(1, "principal1");
        let mut p2 = make_test_principal(2, "principal2");
        p1.resources.push(5001);
        p2.resources.push(5001);
        principals.insert(GlobalPrincipalIdentifier::new_for_test(1), p1);
        principals.insert(GlobalPrincipalIdentifier::new_for_test(2), p2);

        let mut resources = FxHashMap::default();
        // 3 claims: (1, 1), (2, 1), (2, 2) -> subjects: 1, 1, 2 -> unique subjects: 1, 2 -> share_count = 2
        resources
            .insert(5001, make_test_vmo_resource(5001, 0, 600, 1200, vec![(1, 1), (2, 1), (2, 2)]));

        let resource_names = vec![ZXName::from_string_lossy("multi_claim_vmo")];
        let summary = MemorySummary::build(&principals, &resources, &resource_names);

        assert_eq!(summary.principals.len(), 2);
        for p_sum in &summary.principals {
            assert_eq!(p_sum.committed_total, 600);
            assert_eq!(p_sum.populated_total, 1200);
            assert_eq!(p_sum.committed_scaled, 300.0);
            assert_eq!(p_sum.populated_scaled, 600.0);
            assert_eq!(p_sum.committed_private, 0);
            assert_eq!(p_sum.populated_private, 0);
        }
    }

    /// What is tested: `MemorySummary::build` gracefully skips resource IDs in a principal's
    /// resource list that do not exist in the `resources` map.
    ///
    /// Expectations verified:
    /// - A principal referencing valid resource 5001 and non-existent resource 99999
    ///   does not panic and attributes only 5001.
    #[test]
    fn test_memory_summary_skips_missing_resource_id() {
        let mut principals = FxHashMap::default();
        let mut p1 = make_test_principal(1, "principal_with_missing_res");
        p1.resources.push(5001);
        p1.resources.push(99999);
        principals.insert(GlobalPrincipalIdentifier::new_for_test(1), p1);

        let mut resources = FxHashMap::default();
        resources.insert(5001, make_test_vmo_resource(5001, 0, 400, 800, vec![(1, 1)]));

        let resource_names = vec![ZXName::from_string_lossy("valid_vmo")];
        let summary = MemorySummary::build(&principals, &resources, &resource_names);

        assert_eq!(summary.principals.len(), 1);
        assert_eq!(summary.principals[0].committed_total, 400);
        assert_eq!(summary.principals[0].populated_total, 800);
    }
}
