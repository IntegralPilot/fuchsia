// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use assembly_constants::ZeroPageScanCount;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// What should happen if the device runs out-of-memory.
#[derive(Debug, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OOMBehavior {
    Reboot { timeout: OOMRebootTimeout },
    JobKill,
    Disable,
}

impl Default for OOMBehavior {
    fn default() -> Self {
        OOMBehavior::Reboot { timeout: OOMRebootTimeout::default() }
    }
}

/// The reboot timeout if the device runs out-of-memory.
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OOMRebootTimeout {
    #[default]
    Normal,
    Low,
}

/// Sets the memory reclamation strategy of the device's kernel.
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MemoryReclamationStrategy {
    /// Default strategy that balances memory with performance.
    #[default]
    Balanced,
    /// Try hard to reclaim memory, even recently-used pages.
    Eager,
}

/// Platform configuration options for the kernel area.
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct PlatformKernelConfig {
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub memory_compression: bool,

    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub lru_memory_compression: bool,

    /// Configures cprng related behaviors
    #[serde(default)]
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub cprng: CprngConfig,

    /// For address spaces that use ASLR this controls the number of bits of
    /// entropy in the randomization. Higher entropy results in a sparser
    /// address space and uses more memory for page tables. Valid values range
    /// from 0-36. Default value is 30.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aslr_entropy_bits: Option<u8>,

    /// Upper-bound in megabytes for the system memory.
    /// It simulates a system with less physical memory than it actually has.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_limit_mb: Option<u64>,

    /// Configuration for the kernel memory reclamation strategy.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub memory_reclamation_strategy: MemoryReclamationStrategy,

    /// Configurations related to page scanner behavior.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_scanner: Option<PageScannerConfig>,

    // Configurations related to out-of-memory and memory reclamation behavior.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub oom: OomConfig,

    // Configurations related to kernel tracing.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub ktrace: KtraceConfig,

    /// Sets the "memory block size" parameter for jitterentropy. When jitterentropy is
    /// performing memory operations (to increase variation in CPU timing), the memory
    /// will be accessed in blocks of this size.
    ///
    /// Please tune to specific CPU prior to setting and provide validation data like
    /// entropy rating to associated CL (similar process to go/fuchsia-per-device:jitter)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jitterentropy_bs: Option<u32>,

    /// Sets the "memory block count" parameter for jitterentropy. When jitterentropy
    /// is performing memory operations (to increase variation in CPU timing), this
    /// controls how many blocks (of size `kernel.jitterentropy.bs`) are accessed.
    ///
    /// Please tune to specific CPU prior to setting and provide validation data like
    /// entropy rating to associated CL (similar process to go/fuchsia-per-device:jitter)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jitterentropy_bc: Option<u32>,

    /// Sets the "memory loops" parameter for jitterentropy. When jitterentropy is
    /// performing memory operations (to increase variation in CPU timing), this
    /// controls how many times the memory access routine is repeated. This parameter
    /// is only used when `kernel.jitterentropy.raw` is true. If the value of this
    /// parameter is `0` or if `kernel.jitterentropy.raw` is `false`, then
    /// jitterentropy chooses the number of loops is a random-ish way.
    ///
    /// Please tune to specific CPU prior to setting and provide validation data like
    /// entropy rating to associated CL (similar process to go/fuchsia-per-device:jitter)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jitterentropy_ml: Option<u32>,

    /// Sets the "LFSR loops" parameter for jitterentropy. When
    /// jitterentropy is performing CPU-intensive LFSR operations (to increase variation
    /// in CPU timing), this controls how many times the LFSR routine is repeated.  This
    /// parameter is only used when `kernel.jitterentropy.raw` is true. If the value of
    /// this parameter is `0` or if `kernel.jitterentropy.raw` is `false`, then
    /// jitterentropy chooses the number of loops is a random-ish way.
    ///
    /// Please tune to specific CPU prior to setting and provide validation data like
    /// entropy rating to associated CL (similar process to go/fuchsia-per-device:jitter)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jitterentropy_ll: Option<u32>,

    /// Sets the "entropy per 1000 bytes" parameter for jitterentropy. This is an
    /// estimate of how many bits of entropy are collected for every 1000 bytes of
    /// output from the collector.
    ///
    /// The value is calculated as:
    /// 0.1 (safety factor) * <entropy bit per byte> * 1000
    ///
    /// Please tune to specific CPU prior to setting and provide validation data like
    /// entropy rating to associated CL (similar process to go/fuchsia-per-device:jitter)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jitterentropy_entropy_per_1000_bytes: Option<u32>,

    /// Disable emulation of the previous thread wakeup accounting behavior when
    /// unified bookkeeping is enabled. The previous wakeup behavior makes it
    /// less likely that a newly woken thread will preempt a currently running
    /// thread. This behavior is less fair to newly woken threads than the new
    /// behavior implemented by unified bookkeeping, but is desirable because it
    /// makes latent race conditions less likely to cause problems.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub scheduler_enable_new_wakeup_accounting: bool,

    /// Configurations related to the kernel heap.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub heap: HeapConfig,
}

/// Options for ktrace behaviors.
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct KtraceConfig {
    /// Total amount of memory in MiB used to store the ktrace buffers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bufsize: Option<u32>,
}

/// Options for cprng behaviors
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CprngConfig {
    /// When enabled and if jitterentropy fails at initial seeding, CPRNG panics.
    #[serde(default)]
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub seed_require_jitterentropy: bool,

    /// When enabled and if you do not provide entropy input from the kernel
    /// command line, CPRNG panics.
    #[serde(default)]
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub seed_require_cmdline: bool,

    /// When enabled and if jitterentropy fails at reseeding, CPRNG panics.
    #[serde(default)]
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub reseed_require_jitterentropy: bool,
}

/// Options for user page tables the reclamation policy.
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PagetableEvictionPolicy {
    /// Unused page tables are evicted periodically. The period
    /// can be controlled by kernel.page-scanner.page-table-eviction-period.
    #[default]
    Always,

    /// Page tables are never evicted.
    Never,

    /// Only performs eviction on request, such as in
    /// response to a low memory scenario.
    OnRequest,
}

/// Configurations related to page scanner behavior.
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct PageScannerConfig {
    /// Sets the reclamation policy for user page tables that are not accessed.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub page_table_eviction_policy: PagetableEvictionPolicy,

    /// This option causes the kernels active memory scanner to be initially
    /// disabled on startup if the value is true. You can also enable and
    /// disable it using the kernel console. If you disable the scanner, you
    /// can have additional system predictability since it removes time based
    /// and background memory eviction.
    ///
    /// Every action the scanner performs can be individually configured and
    /// disabled. If all actions are disabled then enabling the scanner has no
    /// effect.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub disable_at_boot: bool,

    /// This option configures the maximal number of candidate pages the zero
    /// page scanner will consider every second.
    ///
    /// The page scanner must be running for this option to have any effect. It
    /// can be enabled at boot unless `disable_at_boot` is set to True.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub zero_page_scans_per_second: ZeroPageScanCount,

    /// When set, disable the page scanner to evict user pager backed pages.
    /// Eviction can reduce memory usage and prevent out of memory scenarios,
    /// but removes some timing predictability from system behavior.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub disable_eviction: bool,
}

// Configurations related to out-of-memory and memory reclamation behavior.
#[derive(Debug, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct OomConfig {
    /// What should happen if the device runs out-of-memory.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub behavior: OOMBehavior,

    /// Triggers kernel eviction when free memory falls below warning_mb,
    /// as opposed to the default of triggering eviction when free memory falls
    /// below critical_mb.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub evict_at_warning: bool,

    /// This option configures kernel eviction to run continually in the
    /// background to try and keep the system out of memory pressure, as opposed
    /// to triggering one-shot eviction only at memory pressure level
    /// transitions.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub evict_continuous: bool,

    /// Delay (in ms) before kernel eviction is triggered under memory pressure.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub eviction_delay_ms: Option<u32>,

    /// Whether kernel eviction also tries to free a minimum amount in addition
    /// to meeting a free memory target.
    #[serde(skip_serializing_if = "is_evict_with_min_target_default")]
    pub evict_with_min_target: bool,

    /// The granularity (in MiB) of synchronous kernel eviction to avoid OOM.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub eviction_delta_at_oom_mb: Option<u32>,

    /// This option specifies the free-memory threshold at which the
    /// out-of-memory (OOM) thread will trigger an out-of-memory event and begin
    /// killing processes, or rebooting the system.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub out_of_memory_mb: Option<u32>,

    /// This option specifies the free-memory threshold at which the
    /// out-of-memory (OOM) thread will trigger a critical memory pressure
    /// event, signaling that processes should free up memory.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub critical_mb: Option<u32>,

    /// This option specifies the free-memory threshold at which the
    /// out-of-memory (OOM) thread will trigger a warning memory pressure event,
    /// signaling that processes should slow down memory allocations.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning_mb: Option<u32>,

    /// Delta (in MiB) above OOM level at which the Imminent-OOM event is signaled.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub imminent_oom_delta_mb: Option<u32>,

    /// Memory pressure state is changed only if free memory is debounce_mb outside of that state.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub debounce_mb: Option<u32>,

    /// Hysteresis interval (in seconds) between memory pressure state transitions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hysteresis_seconds: Option<u32>,

    /// (Experimental) If true, enable expanded memory stall metrics.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub experimental_expand_memory_stall: bool,
}

impl Default for OomConfig {
    fn default() -> Self {
        Self {
            behavior: Default::default(),
            evict_at_warning: false,
            evict_continuous: false,
            eviction_delay_ms: None,
            evict_with_min_target: true,
            eviction_delta_at_oom_mb: None,
            out_of_memory_mb: None,
            critical_mb: None,
            warning_mb: None,
            imminent_oom_delta_mb: None,
            debounce_mb: None,
            hysteresis_seconds: None,
            experimental_expand_memory_stall: false,
        }
    }
}

fn is_evict_with_min_target_default(val: &bool) -> bool {
    *val
}

/// Configurations related to the kernel heap.
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct HeapConfig {
    /// Enable the use of a virtually managed kernel heap instead of one managed directly out of the
    /// physmap.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub enable_virtually_managed: bool,

    /// Maximum size of the virtual kernel heap (if enabled).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_size_mb: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_serialization() {
        crate::common::tests::default_serialization_helper::<PlatformKernelConfig>();
    }
}
