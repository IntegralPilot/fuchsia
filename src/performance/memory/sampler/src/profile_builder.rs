// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use anyhow::Error;
use fidl_fuchsia_memory_sampler::ModuleMap;
use prost::Message;
use zx::Vmo;

use crate::crash_reporter::ProfileReport;
use crate::pprof;

/// The default sampling rate in bytes (128 KiB). This matches the default
/// client-side sampling rate configured in the instrumentation library.
const DEFAULT_SAMPLING_RATE_BYTES: f64 = 131072.0;

pub type StackTrace = Vec<u64>;

/// Represents an allocation for which no deallocation has been
/// reported.
#[derive(Clone, Debug, PartialEq)]
pub struct LiveAllocation {
    pub size: u64,
    pub scale_factor: f64,
    pub stack_trace: Rc<StackTrace>,
}

/// Aggregated counter of allocations for which a deallocation has
/// been recorded.
#[derive(Clone, Default, Debug)]
pub struct DeadAllocationCounter {
    pub total_size: f64,
    pub count: f64,
}

/// Aggregated counter of deallocations.
#[derive(Clone, Default, Debug)]
pub struct DeallocationCounter {
    pub total_size: f64,
    pub count: f64,
}

/// Accumulator for profiling information.
#[derive(Default, Debug)]
pub struct ProfileBuilder {
    process_name: String,
    module_map: Vec<ModuleMap>,
    /// Mapping from addresses to allocations; we assume there can
    /// only be one live allocation at a given address. Reallocations
    /// are recorded as a sequence of a deallocation and a new
    /// allocation.
    live_allocations: HashMap<u64, LiveAllocation>,
    /// Mapping from a stack trace to a counter of dead
    /// allocations. This representation aggregates allocations from
    /// the same call site to save memory.
    dead_allocations: HashMap<Rc<StackTrace>, DeadAllocationCounter>,
    /// Mapping from a stack trace to a counter of deallocations. This
    /// representation aggregates deallocations from the same call
    /// site to save memory.
    deallocations: HashMap<Rc<StackTrace>, DeallocationCounter>,
    /// Set used to hold references to recorded stack traces. This can
    /// be used to drastically reduce memory usage from allocations
    /// from an already known call site: additional allocations would
    /// store a reference, rather than the entire stack trace that
    /// could be fairly large.
    stack_traces: HashSet<Rc<StackTrace>>,
    is_lossy: bool,
}

/// Computes the unsampling scale factor for an allocation of a given `size` using
/// the Poisson sampling interval `rate`.
///
/// Under Poisson sampling, the probability of sampling an allocation of size $S$
/// with a mean sampling rate $R$ is $P(S) = 1 - e^{-S / R}$.
///
/// To reconstruct the original unsampled allocation volume, we scale each sampled
/// allocation's size and count by the inverse of its sampling probability:
/// $W = 1 / P(S) = 1 / (1 - e^{-S / R})$.
fn scale_factor(size: u64, rate: f64) -> f64 {
    if size == 0 { 1.0 } else { -1.0 / (-(size as f64) / rate).exp_m1() }
}

impl ProfileBuilder {
    pub fn process_name(&self) -> &str {
        &self.process_name
    }

    /// Remove all stack traces that are no longer referenced by any
    /// recorded allocation.
    ///
    /// Note: this function can be used to reclaim memory after
    /// consuming allocations/deallocations (e.g. by producing a
    /// partial profile).
    fn prune_unreferenced_stack_traces(&mut self) {
        self.stack_traces.retain(|st| Rc::strong_count(st) > 1);
    }
    /// Add the given stack_trace to the cache, if needed, then return
    /// a reference to the cached value.
    fn cache_stack_trace(&mut self, stack_trace: StackTrace) -> Rc<StackTrace> {
        // Note: `Entry` on `HashSet` would save us from cloning the
        // stack trace here.
        if !self.stack_traces.contains(&stack_trace) {
            self.stack_traces.insert(Rc::new(stack_trace.clone()));
        };
        self.stack_traces.get(&stack_trace).unwrap().clone()
    }
    /// Register an allocation. It is considered live until a
    /// deallocation for the same address has been reported. Note that
    /// this assumes that allocations and deallocations at a given
    /// address are ordered.
    pub fn allocate(&mut self, address: u64, stack_trace: StackTrace, size: u64) {
        let scale = scale_factor(size, DEFAULT_SAMPLING_RATE_BYTES);
        self.allocate_with_scale(address, stack_trace, size, scale);
    }

    pub fn allocate_with_scale(
        &mut self,
        address: u64,
        stack_trace: StackTrace,
        size: u64,
        scale_factor: f64,
    ) {
        let stack_trace = self.cache_stack_trace(stack_trace);
        self.live_allocations.insert(address, LiveAllocation { size, scale_factor, stack_trace });
    }
    /// Register a deallocation, if the corresponding allocation has
    /// been registered before.
    pub fn deallocate(&mut self, address: u64, stack_trace: StackTrace) {
        self.live_allocations.remove(&address).map(|allocation| {
            {
                let dead_allocation =
                    self.dead_allocations.entry(allocation.stack_trace).or_default();
                dead_allocation.count += allocation.scale_factor;
                dead_allocation.total_size += (allocation.size as f64) * allocation.scale_factor;
            }
            {
                let stack_trace = self.cache_stack_trace(stack_trace);
                let deallocation = self.deallocations.entry(stack_trace).or_default();
                deallocation.count += allocation.scale_factor;
                deallocation.total_size += (allocation.size as f64) * allocation.scale_factor;
            }
        });
    }
    /// Set the process information necessary to produce a profile.
    pub fn set_process_info(
        &mut self,
        process_name: Option<String>,
        module_map: impl Iterator<Item = ModuleMap>,
    ) {
        if let Some(process_name) = process_name {
            self.process_name = process_name;
        }
        self.module_map.extend(module_map);
    }
    /// Returns the approximate amount of stack traces currently
    /// recorded in this instance.
    ///
    /// Note: Consuming past allocation events (e.g. by producing a
    /// partial profile) can be followed by a call to
    /// `self.prune_unereferenced_stack_traces` to reclaim space; this
    /// function can be used as an heuristic to estimate the amount of
    /// memory that can be reclaimed.
    pub fn get_approximate_reclaimable_stack_traces_count(&self) -> usize {
        self.dead_allocations.len() + self.deallocations.len()
    }
    /// Sets whether the profile session experienced buffer saturation and is lossy.
    pub fn set_lossy(&mut self, is_lossy: bool) {
        self.is_lossy = is_lossy;
    }

    /// Returns whether the profile session experienced buffer saturation and is lossy.
    pub fn is_lossy(&self) -> bool {
        self.is_lossy
    }

    /// Returns the process name to report, annotated when the profile is lossy.
    /// The annotation rides on the name because that is what `pprof` surfaces
    /// in the banner, tab title and profile picker, making the taint hard to
    /// overlook.
    fn annotated_process_name(&self) -> String {
        const TAINT_SUFFIX: &str = "[TAINTED: BUFFER OVERFLOW]";
        if !self.is_lossy {
            return self.process_name.clone();
        }
        if self.process_name.is_empty() {
            TAINT_SUFFIX.to_string()
        } else {
            format!("{} {}", self.process_name, TAINT_SUFFIX)
        }
    }

    /// Finalize the profile. Consumes this builder.
    pub fn build(self) -> Result<ProfileReport, Error> {
        let process_name = self.annotated_process_name();
        let profile = pprof::build_profile(
            self.module_map.iter(),
            self.live_allocations.values(),
            self.dead_allocations,
            self.deallocations,
            &self.stack_traces,
            self.is_lossy,
        );

        let (vmo, size) = profile_to_vmo(&profile)?;
        Ok(ProfileReport::Final { process_name, profile: vmo, size })
    }
    /// Produce a partial profile from a process that is still
    /// live. Drop `dead_allocations` from `self`, and prune the
    /// cache.
    ///
    /// Note: this lets one produce regular running profiles from a
    /// long-lived process, while clearing from memory the state that
    /// will no longer be useful.
    pub fn build_partial_profile(&mut self, iteration: usize) -> Result<ProfileReport, Error> {
        let process_name = self.annotated_process_name();
        let profile = {
            pprof::build_profile(
                self.module_map.iter(),
                self.live_allocations.values(),
                std::mem::replace(&mut self.dead_allocations, HashMap::new()),
                std::mem::replace(&mut self.deallocations, HashMap::new()),
                &self.stack_traces,
                self.is_lossy,
            )
        };
        self.prune_unreferenced_stack_traces();

        let (vmo, size) = profile_to_vmo(&profile)?;
        Ok(ProfileReport::Partial { process_name, profile: vmo, size, iteration })
    }
}

// Serialize a profile to a VMO. On success, returns a tuple of a
// `Vmo` and the size of its content.
fn profile_to_vmo(profile: &pprof::pproto::Profile) -> Result<(Vmo, u64), Error> {
    let proto_profile = profile.encode_to_vec();
    let size = proto_profile.len() as u64;
    let vmo = Vmo::create(size)?;
    vmo.write(&proto_profile[..], 0)?;
    Ok((vmo, size))
}

#[cfg(test)]
mod test {
    use crate::profile_builder::{
        DEFAULT_SAMPLING_RATE_BYTES, DeadAllocationCounter, DeallocationCounter, ModuleMap,
        ProfileBuilder,
    };
    use fidl_fuchsia_memory_sampler::ExecutableSegment;

    #[fuchsia::test]
    fn test_allocate() {
        let mut builder = ProfileBuilder::default();
        let address = 0x1000;
        let stack_trace = vec![];
        let size = 10;

        builder.allocate(address, stack_trace.clone(), size);

        let allocation =
            builder.live_allocations.get(&address).expect("Could not retrieve live allocation.");

        let expected_scale_factor = super::scale_factor(size, DEFAULT_SAMPLING_RATE_BYTES);
        assert_eq!(size, allocation.size);
        assert!((allocation.scale_factor - expected_scale_factor).abs() < 1e-5);
        assert_eq!(stack_trace, *(allocation.stack_trace));
    }

    #[fuchsia::test]
    fn test_deallocate_mismatch() {
        let mut builder = ProfileBuilder::default();
        let address = 0x1000;
        let stack_trace = vec![];

        builder.deallocate(address, stack_trace);

        let deallocations = builder.deallocations;

        assert!(deallocations.is_empty());
    }

    #[fuchsia::test]
    fn test_deallocate_match() {
        let mut builder = ProfileBuilder::default();
        let address = 0x1000;
        let allocation_stack_trace = vec![1, 2];
        let deallocation_stack_trace = vec![3, 4];
        let size = 10;

        builder.allocate(address, allocation_stack_trace.clone(), size);
        builder.deallocate(address, deallocation_stack_trace.clone());

        assert!(builder.live_allocations.is_empty());
        let expected_count = super::scale_factor(size, DEFAULT_SAMPLING_RATE_BYTES);
        let expected_size = expected_count * (size as f64);
        {
            let allocations = builder.dead_allocations;
            assert_eq!(1, allocations.values().len());
            {
                let (stack_trace, DeadAllocationCounter { count, total_size }) =
                    allocations.into_iter().next().unwrap();
                assert!((total_size - expected_size).abs() < 1e-5);
                assert!((count - expected_count).abs() < 1e-5);
                assert_eq!(allocation_stack_trace, *(stack_trace));
            }
        }

        {
            let deallocations = builder.deallocations;
            assert_eq!(1, deallocations.values().len());
            {
                let (stack_trace, DeallocationCounter { count, total_size }) =
                    deallocations.into_iter().next().unwrap();
                assert!((total_size - expected_size).abs() < 1e-5);
                assert!((count - expected_count).abs() < 1e-5);
                assert_eq!(deallocation_stack_trace, *(stack_trace));
            }
        }
    }

    #[fuchsia::test]
    fn test_build_partial_profile_prunes_stack_traces() {
        let mut builder = ProfileBuilder::default();
        let address = 0x1000;
        let allocation_stack_trace = vec![1, 2];
        let deallocation_stack_trace = vec![3, 4];
        let size = 10;
        let test_index = 42;

        builder.allocate(address, allocation_stack_trace.clone(), size);
        builder.deallocate(address, deallocation_stack_trace.clone());

        assert_ne!(0, builder.get_approximate_reclaimable_stack_traces_count());
        let _ = builder.build_partial_profile(test_index).unwrap();
        assert_eq!(0, builder.get_approximate_reclaimable_stack_traces_count());
    }

    #[fuchsia::test]
    fn test_set_process_info() {
        let mut builder = ProfileBuilder::default();
        let process_name = "test_process".to_string();
        let module_map = vec![ModuleMap {
            build_id: Some(vec![1, 2, 3, 4]),
            executable_segments: Some(vec![ExecutableSegment {
                start_address: Some(0),
                size: Some(10),
                relative_address: Some(100),
                ..Default::default()
            }]),
            ..Default::default()
        }];

        builder.set_process_info(Some(process_name.clone()), module_map.clone().into_iter());

        assert_eq!(process_name, builder.process_name);
        assert_eq!(module_map, builder.module_map);
    }

    #[fuchsia::test]
    fn test_lossy_profile_name() {
        let mut builder = ProfileBuilder::default();
        builder.set_process_info(Some("my_proc".to_string()), [].into_iter());
        builder.set_lossy(true);
        let report = builder.build().unwrap();
        assert_eq!(report.get_process_name(), "my_proc [TAINTED: BUFFER OVERFLOW]");
    }
}
