// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::attribution_client::AttributionState;
use attribution_processing::{PrincipalDescription, ZXName};
use fidl_fuchsia_memory_attribution as fattribution;
use fidl_fuchsia_memory_attribution_plugin as fplugin;
use fuchsia_trace::duration;
use index_table_builder::IndexTableBuilder;
use log::warn;
use std::collections::{HashMap, HashSet};
use std::mem::MaybeUninit;
use traces::CATEGORY_MEMORY_CAPTURE;
use zerocopy::{FromBytes, IntoBytes};
const ZX_INFO_CACHE_INITIAL_SIZE: usize = 64;
const ZX_INFO_CACHE_GROWTH_FACTOR: usize = 2;

/// Deduplicate resource names and reference them by index.
#[derive(Default)]
pub struct NameTableBuilder {
    builder: IndexTableBuilder<ZXName>,
}

fn into_zx_name(name: &zx::Name) -> Result<&ZXName, zx::Status> {
    ZXName::ref_from_bytes(name.as_bytes()).map_err(|_| zx::Status::INVALID_ARGS)
}

impl NameTableBuilder {
    fn intern(&mut self, resource_name: &ZXName) -> Result<u64, zx::Status> {
        Ok(self.builder.intern(resource_name).try_into().map_err(|_| fidl::Status::OUT_OF_RANGE)?)
    }

    pub fn build(self) -> Vec<ZXName> {
        self.builder.build()
    }
}

/// Set of jobs, processes, and VMOs, indexed by KOIDs.
#[derive(Default)]
pub struct KernelResources {
    /// Map of resource Koid to resource definition.
    pub resources: HashMap<zx::Koid, fplugin::Resource>,
    pub resource_names: Vec<ZXName>,
}

#[derive(Default)]
struct KernelResourcesBuilder {
    /// Map of resource Koid to resource definition.
    resources: HashMap<zx::Koid, fplugin::Resource>,
    /// Map of resource name to unique identifier.
    ///
    /// Many different resources often share the same name. In order to minimize the space taken by
    /// resource definitions, we give each unique name an identifier, and refer to these
    /// identifiers in the resource definitions
    resource_names: NameTableBuilder,
}

impl KernelResourcesBuilder {
    fn build(self) -> KernelResources {
        KernelResources { resources: self.resources, resource_names: self.resource_names.build() }
    }

    fn on_job(
        &mut self,
        job_koid: zx_types::zx_koid_t,
        job_name: &ZXName,
        job: fplugin::Job,
    ) -> Result<(), zx::Status> {
        let name_index = self.resource_names.intern(job_name)?;
        self.resources.insert(
            zx::Koid::from_raw(job_koid),
            fplugin::Resource {
                koid: Some(job_koid),
                name_index: Some(name_index),
                resource_type: Some(fplugin::ResourceType::Job(job)),
                ..Default::default()
            },
        );
        Ok(())
    }

    fn on_vmo(
        &mut self,
        vmo_koid: zx_types::zx_koid_t,
        vmo_name: &ZXName,
        vmo: fplugin::Vmo,
    ) -> Result<(), zx::Status> {
        let vmo_koid = zx::Koid::from_raw(vmo_koid);
        // No need to copy the VMO info if we have already seen it.
        if self.resources.contains_key(&vmo_koid) {
            return Ok(());
        }
        let name_index = self.resource_names.intern(vmo_name)?;
        self.resources.insert(
            vmo_koid,
            fplugin::Resource {
                koid: Some(vmo_koid.raw_koid()),
                name_index: Some(name_index),
                // TODO(https://fxbug.dev/393078902): also take into account the fractional
                // part.
                resource_type: Some(fplugin::ResourceType::Vmo(vmo)),
                ..Default::default()
            },
        );
        Ok(())
    }

    fn on_process(
        &mut self,
        process_koid: zx_types::zx_koid_t,
        process_name: &ZXName,
        process: fplugin::Process,
    ) -> Result<(), zx::Status> {
        let process_koid = zx::Koid::from_raw(process_koid);
        let process_name_index = self.resource_names.intern(process_name)?;
        self.resources.insert(
            process_koid,
            fplugin::Resource {
                koid: Some(process_koid.raw_koid()),
                name_index: Some(process_name_index),
                resource_type: Some(fplugin::ResourceType::Process(process)),
                ..Default::default()
            },
        );
        Ok(())
    }
}

/// Crawls the jobs, processes and vmos, calling back visitor method for each object.
#[derive(Default)]
pub struct KernelResourcesExplorer {
    cache: Cache,
}

struct Cache {
    /// Cache for `zx_info_vmo_t` objects, to speed up related syscalls.
    vmos_cache_internal: Vec<MaybeUninit<zx::VmoInfo>>,
    /// Cache for `zx_info_maps_t` objects, to speed up related syscalls.
    maps_cache_internal: Vec<MaybeUninit<zx::MapInfo>>,
}

impl Default for Cache {
    fn default() -> Self {
        let mut result = Self { vmos_cache_internal: vec![], maps_cache_internal: vec![] };
        result.vmos_cache(ZX_INFO_CACHE_INITIAL_SIZE);
        result.maps_cache(ZX_INFO_CACHE_INITIAL_SIZE);
        result
    }
}

impl Cache {
    /// Returns a buffer with enough space to hold at least `minimum_size` [zx::VmoInfo] objects.
    fn vmos_cache(&mut self, minimum_size: usize) -> &mut Vec<MaybeUninit<zx::VmoInfo>> {
        if self.vmos_cache_internal.len() > minimum_size {
            return &mut self.vmos_cache_internal;
        }

        // Having all entries inititialized to a non-zero value ensures their page are committed to
        // memory. This avoids the issue described in https://fxbug.dev/383401884, where faulting
        // pages during the syscall is much more expensive than faulting them in userspace.
        let mut base = zx::VmoInfo::default();
        base.size_bytes = 1;
        self.vmos_cache_internal =
            vec![MaybeUninit::new(base); minimum_size * ZX_INFO_CACHE_GROWTH_FACTOR];

        return &mut self.vmos_cache_internal;
    }

    /// Returns a buffer with enough space to hold at least `minimum_size` [zx::MapInfo] objects.
    fn maps_cache(&mut self, minimum_size: usize) -> &mut Vec<MaybeUninit<zx::MapInfo>> {
        if self.maps_cache_internal.len() > minimum_size {
            return &mut self.maps_cache_internal;
        }

        // Having all entries inititialized to a non-zero value ensures their page are committed to
        // memory. This avoids the issue described in https://fxbug.dev/383401884, where faulting
        // pages during the syscall is much more expensive than faulting them in userspace.
        let base = zx::MapInfo::new(Default::default(), 1, 0, 0, zx::MapDetails::None).unwrap();
        self.maps_cache_internal =
            vec![MaybeUninit::new(base); minimum_size * ZX_INFO_CACHE_GROWTH_FACTOR];

        return &mut self.maps_cache_internal;
    }
}

/// Interval of the virtual address space.
#[derive(Clone, Copy, PartialEq, Debug)]
struct Range {
    /// Inclusive lower bound.
    start: u64,
    /// Exclusive upper bound.
    end: u64,
}

impl Range {
    fn is_overlapping(&self, other: &Self) -> bool {
        self.start < other.end && other.start < self.end
    }

    fn merge(a: &Self, b: &Self) -> Self {
        if b.start > a.end + 1 || a.start > b.end + 1 {
            warn!("Not supported: Union of {:?} and {:?} is not continuous", a, b);
        }
        Self { start: a.start.min(b.start), end: a.end.max(b.end) }
    }
}

/// Decides whether we should collect information about VMOs or memory maps of a process.
#[derive(Clone, Debug, PartialEq)]
struct CollectionRequest {
    /// When one or more principals declared that the process handle table collection is optional.
    can_ignore_vmos: bool,
    /// True when one or more principals attributed explicitly the process to a principal.
    /// This has precedence on `can_ignore_vmos`.
    must_collect_vmos: bool,
    /// Set when one or more principals attributed explicitly a process mapping to a principal.
    collect_maps: Option<Range>,
}

impl Default for CollectionRequest {
    fn default() -> Self {
        // Unless specified all VMOs held by the process are collected,
        // and the process root VMAR is *not* enumerated.
        Self { can_ignore_vmos: false, must_collect_vmos: false, collect_maps: None }
    }
}

impl CollectionRequest {
    fn collect_vmos(&self) -> bool {
        self.must_collect_vmos || !self.can_ignore_vmos
    }

    fn merge(&mut self, other: &Self) {
        self.can_ignore_vmos |= other.can_ignore_vmos;
        self.must_collect_vmos |= other.must_collect_vmos;
        self.collect_maps = match (self.collect_maps, other.collect_maps) {
            (None, None) => None,
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (Some(a), Some(b)) => Some(Range::merge(&a, &b)),
        };
    }
}
/// Interface for a Zircon job. This is useful to allow for dependency injection in tests.
pub trait Job: Send {
    /// Returns the Koid of the job.
    fn get_koid(&self) -> Result<zx::Koid, zx::Status>;
    /// Returns the name of the job.
    fn get_name(&self) -> Result<zx::Name, zx::Status>;
    /// Returns the koids of the job children of the job.
    fn children(&self) -> Result<Vec<zx::Koid>, zx::Status>;
    /// Returns the koids of the processes directly held by this job.
    fn processes(&self) -> Result<Vec<zx::Koid>, zx::Status>;
    /// Return a child Job from its Koid.
    fn get_child_job(
        &self,
        koid: &zx::Koid,
        rights: zx::Rights,
    ) -> Result<Box<dyn Job>, zx::Status>;
    /// Returns a child Process from its Koid.
    fn get_child_process(
        &self,
        koid: &zx::Koid,
        rights: zx::Rights,
    ) -> Result<Box<dyn Process>, zx::Status>;
}

impl Job for zx::Job {
    fn get_koid(&self) -> Result<zx::Koid, zx::Status> {
        zx::Job::koid(self)
    }

    fn get_name(&self) -> Result<zx::Name, zx::Status> {
        zx::Job::get_name(&self)
    }

    fn children(&self) -> Result<Vec<zx::Koid>, zx::Status> {
        zx::Job::children(&self)
    }

    fn processes(&self) -> Result<Vec<zx::Koid>, zx::Status> {
        zx::Job::processes(&self)
    }

    fn get_child_job(
        &self,
        koid: &zx::Koid,
        rights: zx::Rights,
    ) -> Result<Box<dyn Job>, zx::Status> {
        zx::Job::get_child(&self, koid, rights)
            .map(|handle| Box::<zx::Job>::new(handle.into()) as Box<dyn Job>)
    }

    fn get_child_process(
        &self,
        koid: &zx::Koid,
        rights: zx::Rights,
    ) -> Result<Box<dyn Process>, zx::Status> {
        zx::Job::get_child(&self, koid, rights)
            .map(|handle| Box::<zx::Process>::new(handle.into()) as Box<dyn Process>)
    }
}

/// Interface for a Zircon process. This is useful to allow for dependency injection in tests.
pub trait Process {
    /// Returns the name of the process.
    fn get_name(&self) -> Result<zx::Name, zx::Status>;

    fn info_vmos<'a>(
        &self,
        output_vector: &'a mut Vec<std::mem::MaybeUninit<zx::VmoInfo>>,
    ) -> Result<(&'a [zx::VmoInfo], usize), zx::Status>;

    /// Returns information about the memory mappings of this process.
    fn info_maps<'a>(
        &self,
        output_vector: &'a mut Vec<std::mem::MaybeUninit<zx::MapInfo>>,
    ) -> Result<(&'a [zx::MapInfo], usize), zx::Status>;
}

impl Process for zx::Process {
    fn get_name(&self) -> Result<zx::Name, zx::Status> {
        zx::Process::get_name(&self)
    }

    fn info_vmos<'a>(
        &self,
        output_vector: &'a mut Vec<std::mem::MaybeUninit<zx::VmoInfo>>,
    ) -> Result<(&'a [zx::VmoInfo], usize), zx::Status> {
        let (out, _, available) = zx::Process::info_vmos(self, output_vector)?;
        Ok((out, available))
    }

    fn info_maps<'a>(
        &self,
        output_vector: &'a mut Vec<std::mem::MaybeUninit<zx::MapInfo>>,
    ) -> Result<(&'a [zx::MapInfo], usize), zx::Status> {
        let (out, _, available) = zx::Process::info_maps(self, output_vector)?;
        Ok((out, available))
    }
}

impl KernelResources {
    // Get all jobs, processes and vmos for the specified root.
    pub fn get_resources(
        root: &dyn Job,
        attribution_state: &AttributionState,
        muted_principal: &Option<PrincipalDescription>,
    ) -> Result<KernelResources, zx::Status> {
        let mut kernel_resources_builder = KernelResourcesBuilder::default();
        KernelResourcesExplorer::default().explore_root_job(
            &mut kernel_resources_builder,
            root,
            attribution_state,
            muted_principal,
        )?;
        Ok(kernel_resources_builder.build())
    }
}

impl KernelResourcesExplorer {
    fn explore_root_job(
        &mut self,
        builder: &mut KernelResourcesBuilder,
        root: &dyn Job,
        attribution_state: &AttributionState,
        muted_principal: &Option<PrincipalDescription>,
    ) -> Result<(), zx::Status> {
        duration!(CATEGORY_MEMORY_CAPTURE, "get_resources");

        // Turn the description into a global identifier.
        let muted_principal = muted_principal
            .as_ref()
            .and_then(|description| attribution_state.find_principal_by_description(description));

        // Decide what information has to be collected for each process. There are two
        // sets of data:
        // 1. VMOS from the process handle tables. This includes VMO held by a handle, or
        //    via the root VMAR
        // 2. VMOS from the process root VMAR. It is a subset of #1. Collecting this data
        //    also makes it possible to attribute an address space range.
        //
        // #1 is collected by default.
        //
        // #2 is also collected when the VMAR is explicitly attributed. #1 *can* be
        //    ignored when a provider suggests it. This is just a performance hint.
        //    Enumerating the VMOs anyway does not change the final result.
        //
        // #1 is never ignored when the process is explicitly attributed to a principal.

        let mut process_collection_requests: HashMap<zx::Koid, CollectionRequest> = HashMap::new();

        for (global_principal_identifier, attribute_provier) in attribution_state.0.iter() {
            // Process each claim to know what we need to collect.
            for (_recipient, resources) in attribute_provier.resources.iter() {
                for resource in resources {
                    let (koid, resource_collection) = match resource {
                        fattribution::Resource::KernelObject(attributed_koid) => (
                            zx::Koid::from_raw(*attributed_koid),
                            CollectionRequest { must_collect_vmos: true, ..Default::default() },
                        ),
                        fattribution::Resource::ProcessMapped(pm) => (
                            zx::Koid::from_raw(pm.process),
                            CollectionRequest {
                                collect_maps: Some(Range { start: pm.base, end: pm.base + pm.len }),
                                can_ignore_vmos: pm.hint_skip_handle_table,
                                ..Default::default()
                            },
                        ),
                        fattribution::Resource::__SourceBreaking { unknown_ordinal: _ } => todo!(),
                    };
                    let (koid, resource_collection) =
                        if Some(global_principal_identifier) == muted_principal.as_ref() {
                            (
                                koid,
                                CollectionRequest {
                                    can_ignore_vmos: resource_collection.can_ignore_vmos,
                                    must_collect_vmos: false,
                                    collect_maps: None,
                                },
                            )
                        } else {
                            (koid, resource_collection)
                        };
                    process_collection_requests
                        .entry(koid)
                        .or_default()
                        .merge(&resource_collection);
                }
            }
        }
        self.explore_job(builder, &root.get_koid()?, root, &process_collection_requests)?;
        Ok(())
    }

    /// Recursively gather memory information from a job.
    fn explore_job(
        &mut self,
        builder: &mut KernelResourcesBuilder,
        job_koid: &zx::Koid,
        job: &dyn Job,
        process_collection_requests: &HashMap<zx::Koid, CollectionRequest>,
    ) -> Result<(), zx::Status> {
        let job_name = job.get_name()?;
        let child_job_koids = job.children()?;
        let child_process_koids = job.processes()?;
        for child_job_koid in &child_job_koids {
            // Here and below: jobs and processes can disappear while we explore the job
            // and process hierarchy. Therefore, we don't stop the exploration if we don't
            // find a previously mentioned job or process, but we just ignore it silently.
            let child_job = match job.get_child_job(child_job_koid, zx::Rights::SAME_RIGHTS) {
                Err(zx::Status::NOT_FOUND) => continue,
                Err(status) => return Err(status),
                Ok(child) => child,
            };
            match self.explore_job(
                builder,
                child_job_koid,
                child_job.as_ref(),
                process_collection_requests,
            ) {
                // If the job disappeared while being explored, we get a BAD_STATE error. In that
                // case, we want to go to the next job but not fail the entire collection.
                Err(zx::Status::BAD_STATE) => {}
                Err(status) => return Err(status),
                Ok(_) => {}
            }
        }

        for child_process_koid in &child_process_koids {
            let child_process =
                match job.get_child_process(child_process_koid, zx::Rights::SAME_RIGHTS) {
                    Err(zx::Status::NOT_FOUND) => continue,
                    Err(s) => return Err(s),
                    Ok(child) => child,
                };
            match self.explore_process(
                builder,
                child_process_koid,
                child_process.as_ref(),
                &process_collection_requests.get(child_process_koid).cloned().unwrap_or_default(),
            ) {
                // If the process disappeared while being explored, we get a BAD_STATE error. In
                // that case, we want to go to the next job but not fail the entire collection.
                Err(zx::Status::BAD_STATE) => continue,
                Err(s) => return Err(s),
                Ok(_) => continue,
            };
        }
        builder.on_job(
            job_koid.raw_koid(),
            into_zx_name(&job_name)?,
            fplugin::Job {
                child_jobs: Some(child_job_koids.iter().map(zx::Koid::raw_koid).collect()),
                processes: Some(child_process_koids.iter().map(zx::Koid::raw_koid).collect()),
                ..Default::default()
            },
        )?;
        Ok(())
    }

    /// Gather the memory information of a process.
    fn explore_process(
        &mut self,
        builder: &mut KernelResourcesBuilder,
        process_koid: &zx::Koid,
        process: &dyn Process,
        collection: &CollectionRequest,
    ) -> Result<(), zx::Status> {
        let process_name = process.get_name()?;
        let process_name_string = process_name.as_bstr().to_string();
        duration!(CATEGORY_MEMORY_CAPTURE, "explore_process", "name" => &*process_name_string);

        let vmo_koids = if collection.collect_vmos() {
            duration!(CATEGORY_MEMORY_CAPTURE, "vmos:explore_process");
            let (mut vmo_infos, available) = {
                duration!(CATEGORY_MEMORY_CAPTURE, "fetch:vmos:explore_process");
                process.info_vmos(self.cache.vmos_cache(0))?
            };

            if vmo_infos.len() < available {
                duration!(CATEGORY_MEMORY_CAPTURE, "refetch:vmos:explore_process",
                    "initial_length" => vmo_infos.len(), "target_length" => available);
                (vmo_infos, _) = process.info_vmos(self.cache.vmos_cache(available))?;
            }

            let mut vmo_koids = HashSet::with_capacity(vmo_infos.len());
            for vmo_info in vmo_infos {
                if !vmo_koids.insert(vmo_info.koid.clone()) {
                    // The VMO is already in the set, we can skip.
                    continue;
                }

                // TODO(https://fxbug.dev/393078902): also take into account the fractional
                // part.
                builder.on_vmo(
                    vmo_info.koid.raw_koid(),
                    into_zx_name(&vmo_info.name)?,
                    fplugin::Vmo {
                        parent: match vmo_info.parent_koid.raw_koid() {
                            0 => None,
                            k => Some(k),
                        },
                        private_committed_bytes: Some(vmo_info.committed_private_bytes),
                        private_populated_bytes: Some(vmo_info.populated_private_bytes),
                        scaled_committed_bytes: Some(vmo_info.committed_scaled_bytes),
                        scaled_populated_bytes: Some(vmo_info.populated_scaled_bytes),
                        total_committed_bytes: Some(vmo_info.committed_bytes),
                        total_populated_bytes: Some(vmo_info.populated_bytes),
                        flags: Some(vmo_info.flags.bits()),
                        ..Default::default()
                    },
                )?;
            }
            Some(vmo_koids.iter().map(zx::Koid::raw_koid).collect())
        } else {
            None
        };

        let process_maps = if let Some(selected_range) = collection.collect_maps {
            duration!(CATEGORY_MEMORY_CAPTURE, "maps:explore_process");

            let (mut info_maps, available) = {
                duration!(CATEGORY_MEMORY_CAPTURE, "fetch:maps:explore_process");
                process.info_maps(self.cache.maps_cache(0))?
            };
            if info_maps.len() < available {
                duration!(CATEGORY_MEMORY_CAPTURE, "refetch:maps:explore_process",
                    "initial_length" => info_maps.len(), "target_length" => available);
                (info_maps, _) = process.info_maps(self.cache.maps_cache(available))?;
            }

            // This overestimates the capacity needed, but it is still better than resizing several
            // times.
            let mut mappings = Vec::with_capacity(info_maps.len());
            for info_map in info_maps {
                if let zx::MapDetails::Mapping(details) = info_map.details() {
                    let mapping_range = Range {
                        start: info_map.base.try_into().unwrap(),
                        end: (info_map.base + info_map.size).try_into().unwrap(),
                    };
                    if selected_range.is_overlapping(&mapping_range) {
                        mappings.push(fplugin::Mapping {
                            vmo: Some(details.vmo_koid.raw_koid()),
                            address_base: Some(mapping_range.start),
                            size: Some(info_map.size.try_into().unwrap()),
                            ..Default::default()
                        });
                    }
                }
            }
            // As we overestimated the capacity, we now need to shrink it.
            mappings.shrink_to_fit();
            Some(mappings)
        } else {
            None
        };
        builder.on_process(
            process_koid.raw_koid(),
            into_zx_name(&process_name)?,
            fplugin::Process { vmos: vmo_koids, mappings: process_maps, ..Default::default() },
        )?;
        Ok(())
    }
}

#[cfg(test)]
pub mod tests {
    use std::mem::MaybeUninit;
    use std::vec;

    use super::*;
    use crate::attribution_client::{AttributionProvider, AttributionState};
    use crate::common::LocalPrincipalIdentifier;
    use attribution_processing::GlobalPrincipalIdentifier;
    use fidl_fuchsia_memory_attribution as fattribution;

    #[test]
    fn range_is_overlapping() {
        assert!(!Range { start: 10, end: 20 }.is_overlapping(&Range { start: 9, end: 10 }));
        assert!(Range { start: 10, end: 20 }.is_overlapping(&Range { start: 10, end: 11 }));
        assert!(Range { start: 10, end: 20 }.is_overlapping(&Range { start: 5, end: 15 }));
        assert!(Range { start: 10, end: 20 }.is_overlapping(&Range { start: 19, end: 20 }));
        assert!(!Range { start: 10, end: 20 }.is_overlapping(&Range { start: 20, end: 21 }));
    }

    #[test]
    fn range_merge() {
        assert_eq!(
            Range { start: 1, end: 6 },
            Range::merge(&Range { start: 1, end: 3 }, &Range { start: 5, end: 6 })
        );
        assert_eq!(
            Range { start: 2, end: 6 },
            Range::merge(&Range { start: 2, end: 4 }, &Range { start: 5, end: 6 })
        );
        assert_eq!(
            Range { start: 3, end: 6 },
            Range::merge(&Range { start: 3, end: 5 }, &Range { start: 5, end: 6 })
        );
        assert_eq!(
            Range { start: 4, end: 6 },
            Range::merge(&Range { start: 4, end: 6 }, &Range { start: 5, end: 6 })
        );
        assert_eq!(
            Range { start: 5, end: 7 },
            Range::merge(&Range { start: 5, end: 7 }, &Range { start: 5, end: 6 })
        );
        assert_eq!(
            Range { start: 5, end: 8 },
            Range::merge(&Range { start: 6, end: 8 }, &Range { start: 5, end: 6 })
        );
        assert_eq!(
            Range { start: 5, end: 9 },
            Range::merge(&Range { start: 7, end: 9 }, &Range { start: 5, end: 6 })
        );
        assert_eq!(
            Range { start: 5, end: 10 },
            Range::merge(&Range { start: 8, end: 10 }, &Range { start: 5, end: 6 })
        );
    }

    #[derive(Clone)]
    pub struct FakeJob {
        koid: zx::Koid,
        name: zx::Name,
        children: HashMap<zx::Koid, FakeJob>,
        processes: HashMap<zx::Koid, FakeProcess>,
        status: Option<zx::Status>,
    }

    impl FakeJob {
        pub fn new(
            koid: u64,
            name: &str,
            children: Vec<FakeJob>,
            processes: Vec<FakeProcess>,
        ) -> FakeJob {
            FakeJob {
                koid: zx::Koid::from_raw(koid),
                name: zx::Name::from_bytes_lossy(name.as_bytes()),
                children: children.into_iter().map(|c| (c.koid, c)).collect(),
                processes: processes.into_iter().map(|p| (p.koid, p)).collect(),
                status: None,
            }
        }

        pub fn new_disappearing(koid: u64, status: zx::Status) -> FakeJob {
            FakeJob {
                koid: zx::Koid::from_raw(koid),
                name: zx::Name::default(),
                children: HashMap::new(),
                processes: HashMap::new(),
                status: Some(status),
            }
        }
    }

    impl Job for FakeJob {
        fn get_koid(&self) -> Result<zx::Koid, zx::Status> {
            match self.status {
                Some(status) => Err(status),
                None => Ok(self.koid.clone()),
            }
        }

        fn get_name(&self) -> Result<zx::Name, zx::Status> {
            match self.status {
                Some(status) => Err(status),
                None => Ok(self.name.clone()),
            }
        }

        fn children(&self) -> Result<Vec<zx::Koid>, zx::Status> {
            match self.status {
                Some(status) => Err(status),
                None => Ok(self.children.keys().copied().collect()),
            }
        }

        fn processes(&self) -> Result<Vec<zx::Koid>, zx::Status> {
            match self.status {
                Some(status) => Err(status),
                None => Ok(self.processes.keys().copied().collect()),
            }
        }

        fn get_child_job(
            &self,
            koid: &zx::Koid,
            _rights: zx::Rights,
        ) -> Result<Box<dyn Job>, zx::Status> {
            match self.status {
                Some(status) => Err(status),
                None => Ok(Box::new(self.children.get(koid).ok_or(zx::Status::NOT_FOUND)?.clone())),
            }
        }

        fn get_child_process(
            &self,
            koid: &zx::Koid,
            _rights: zx::Rights,
        ) -> Result<Box<dyn Process>, zx::Status> {
            match self.status {
                Some(status) => Err(status),
                None => {
                    Ok(Box::new(self.processes.get(koid).ok_or(zx::Status::NOT_FOUND)?.clone()))
                }
            }
        }
    }

    #[derive(Clone)]
    pub struct FakeProcess {
        koid: zx::Koid,
        name: zx::Name,
        vmos: Vec<zx::VmoInfo>,
        maps: Vec<zx::MapInfo>,
        status: Option<zx::Status>,
    }

    impl FakeProcess {
        pub fn new(
            koid: u64,
            name: &str,
            vmos: Vec<zx::VmoInfo>,
            maps: Vec<zx::MapInfo>,
        ) -> FakeProcess {
            FakeProcess {
                koid: zx::Koid::from_raw(koid),
                name: zx::Name::from_bytes_lossy(name.as_bytes()),
                vmos,
                maps,
                status: None,
            }
        }

        pub fn new_disappearing(koid: u64, status: zx::Status) -> FakeProcess {
            FakeProcess {
                koid: zx::Koid::from_raw(koid),
                name: zx::Name::default(),
                vmos: Vec::new(),
                maps: Vec::new(),
                status: Some(status),
            }
        }
    }

    impl Process for FakeProcess {
        fn get_name(&self) -> Result<zx::Name, zx::Status> {
            match self.status {
                Some(status) => Err(status),
                None => Ok(self.name.clone()),
            }
        }

        fn info_vmos<'a>(
            &self,
            output_vector: &'a mut Vec<std::mem::MaybeUninit<zx::VmoInfo>>,
        ) -> Result<(&'a [zx::VmoInfo], usize), zx::Status> {
            if let Some(status) = self.status {
                return Err(status);
            }
            self.vmos.iter().take(output_vector.len()).copied().enumerate().for_each(
                |(index, vmo)| {
                    output_vector[index] = MaybeUninit::new(vmo);
                },
            );

            let (initialized, _) = output_vector.split_at_mut(self.vmos.len());
            // TODO(https://fxbug.dev/352398385) switch to MaybeUninit::slice_assume_init_mut
            // SAFETY: these values have been initialized just above.
            let initialized = unsafe {
                std::slice::from_raw_parts_mut(
                    initialized.as_mut_ptr().cast::<zx::VmoInfo>(),
                    initialized.len(),
                )
            };
            return Ok((initialized, self.vmos.len()));
        }

        fn info_maps<'a>(
            &self,
            output_vector: &'a mut Vec<std::mem::MaybeUninit<zx::MapInfo>>,
        ) -> Result<(&'a [zx::MapInfo], usize), zx::Status> {
            if let Some(status) = self.status {
                return Err(status);
            }
            self.maps.iter().take(output_vector.len()).copied().enumerate().for_each(
                |(index, maps)| {
                    output_vector[index] = MaybeUninit::new(maps);
                },
            );

            let (initialized, _) = output_vector.split_at_mut(self.maps.len());
            // TODO(https://fxbug.dev/352398385) switch to MaybeUninit::slice_assume_init_mut
            // SAFETY: these values have been initialized just above.
            let initialized = unsafe {
                std::slice::from_raw_parts_mut(
                    initialized.as_mut_ptr().cast::<zx::MapInfo>(),
                    initialized.len(),
                )
            };
            return Ok((initialized, self.maps.len()));
        }
    }

    pub fn simple_vmo_info(
        koid: u64,
        name: &str,
        parent: u64,
        committed_bytes: u64,
        populated_bytes: u64,
    ) -> zx::VmoInfo {
        let mut vmo_info: zx::VmoInfo = Default::default();
        vmo_info.koid = zx::Koid::from_raw(koid);
        vmo_info.name = zx::Name::from_bytes_lossy(name.as_bytes());
        vmo_info.size_bytes = populated_bytes;
        vmo_info.parent_koid = zx::Koid::from_raw(parent);
        vmo_info.committed_bytes = committed_bytes;
        vmo_info.populated_bytes = populated_bytes;
        vmo_info.committed_fractional_scaled_bytes = 0;
        vmo_info.populated_fractional_scaled_bytes = 0;
        vmo_info.committed_scaled_bytes = committed_bytes;
        vmo_info.populated_scaled_bytes = populated_bytes;
        vmo_info
    }

    #[test]
    fn test_gather_resources() {
        let mut mapping31_details = zx::MappingDetails::default();
        mapping31_details.mmu_flags = zx::VmarFlagsExtended::PERM_READ;
        mapping31_details.vmo_koid = zx::Koid::from_raw(211);
        mapping31_details.committed_bytes = 100;
        mapping31_details.populated_bytes = 100;
        mapping31_details.committed_private_bytes = 100;
        mapping31_details.populated_private_bytes = 100;
        mapping31_details.committed_scaled_bytes = 100;
        mapping31_details.populated_scaled_bytes = 100;
        let root_job = Box::new(FakeJob::new(
            0,
            "component_manager",
            vec![
                FakeJob::new(
                    1,
                    "job1",
                    vec![],
                    vec![
                        FakeProcess::new(
                            11,
                            "proc11",
                            vec![
                                simple_vmo_info(111, "vmo111", 0, 100, 100),
                                simple_vmo_info(112, "vmo112", 0, 200, 200),
                            ],
                            vec![],
                        ),
                        // A disappearing process won't affect the data collection.
                        FakeProcess::new_disappearing(12, zx::Status::BAD_STATE),
                    ],
                ),
                FakeJob::new_disappearing(4, zx::Status::BAD_STATE),
                FakeJob::new(
                    2,
                    "job2",
                    vec![FakeJob::new(
                        3,
                        "job3",
                        vec![],
                        vec![FakeProcess::new(
                            31,
                            "proc31",
                            vec![],
                            vec![
                                zx::MapInfo::new(
                                    zx::Name::from_bytes_lossy("mapping31".as_bytes()),
                                    0x1200,
                                    1024,
                                    2,
                                    zx::MapDetails::Mapping(&mapping31_details),
                                )
                                .unwrap(),
                            ],
                        )],
                    )],
                    vec![FakeProcess::new(
                        21,
                        "proc21",
                        vec![simple_vmo_info(211, "vmo211", 0, 200, 200)],
                        vec![],
                    )],
                ),
            ],
            vec![],
        ));

        let mut attribution_state = AttributionState::default();
        let root_id = GlobalPrincipalIdentifier::new_for_test(1);
        attribution_state.0.insert(
            root_id,
            AttributionProvider {
                definitions: Default::default(),
                resources: vec![(
                    LocalPrincipalIdentifier::new_for_tests(1),
                    vec![fattribution::Resource::ProcessMapped(fattribution::ProcessMapped {
                        process: 31,
                        base: 0x1000,
                        len: 2048,
                        hint_skip_handle_table: false,
                    })],
                )]
                .into_iter()
                .collect(),
            },
        );
        let kernel_resoures =
            KernelResources::get_resources(root_job.as_ref(), &attribution_state, &None)
                .expect("Failed to gather resources");

        if let fplugin::ResourceType::Process(proc11) = kernel_resoures
            .resources
            .get(&zx::Koid::from_raw(11))
            .unwrap_or_else(|| panic!("Unable to find proc11 in {:?}", kernel_resoures.resources))
            .resource_type
            .as_ref()
            .expect("No resource type")
        {
            assert_eq!(proc11.vmos.as_ref().expect("No VMOs").len(), 2);
        } else {
            unreachable!("Not a process");
        }

        if let fplugin::ResourceType::Process(proc31) = kernel_resoures
            .resources
            .get(&zx::Koid::from_raw(31))
            .expect("Unable to find proc31")
            .resource_type
            .as_ref()
            .expect("No resource type")
        {
            assert_eq!(proc31.mappings.as_ref().expect("No mappings").len(), 1);
        } else {
            unreachable!("Not a process");
        }
    }

    #[test]
    fn test_collection_request_merges() {
        let mut a1 = CollectionRequest {
            collect_maps: Some(Range { start: 100, end: 200 }),
            ..Default::default()
        };
        a1.merge(&CollectionRequest {
            collect_maps: Some(Range { start: 300, end: 400 }),
            ..Default::default()
        });
        assert_eq!(
            a1,
            CollectionRequest {
                collect_maps: Some(Range { start: 100, end: 400 }),
                ..Default::default()
            }
        );

        let mut a2 = CollectionRequest {
            collect_maps: Some(Range { start: 100, end: 200 }),
            ..Default::default()
        };
        a2.merge(&CollectionRequest {
            collect_maps: Some(Range { start: 50, end: 400 }),
            ..Default::default()
        });
        assert_eq!(
            a2,
            CollectionRequest {
                collect_maps: Some(Range { start: 50, end: 400 }),
                ..Default::default()
            }
        );

        let mut a3 = CollectionRequest {
            collect_maps: Some(Range { start: 100, end: 200 }),
            ..Default::default()
        };
        a3.merge(&CollectionRequest { must_collect_vmos: true, ..Default::default() });
        assert_eq!(
            a3,
            CollectionRequest {
                can_ignore_vmos: false,
                must_collect_vmos: true,
                collect_maps: Some(Range { start: 100, end: 200 })
            }
        );
    }
}
