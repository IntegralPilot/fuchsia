// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

/// mtrace_control() can operate on a range of features.
/// It's an abstraction that doesn't mean much, and will likely be replaced
/// before it's useful; it's here in the interests of hackability in the
/// interim.
pub const MTRACE_KIND_PERFMON: u32 = 1;

// Actions for CPU Performance Counters/Statistics control

/// Get performonce monitoring system properties
/// The result is an mx_x86_ipm_properties_t struct filled in.
pub const MTRACE_PERFMON_GET_PROPERTIES: u32 = 0;

/// Prepare the kernel for performance data collection trace runs.
pub const MTRACE_PERFMON_INIT: u32 = 1;

/// Assign a buffer to the specified cpu.
pub const MTRACE_PERFMON_ASSIGN_BUFFER: u32 = 2;

/// Stage the perf config for a CPU.
/// Will allocate resources as necessary.
/// Must be called with data collection off.
pub const MTRACE_PERFMON_STAGE_CONFIG: u32 = 3;

/// Start data collection.
/// Must be called after STAGE_CONFIG with data collection off.
pub const MTRACE_PERFMON_START: u32 = 4;

/// Stop data collection.
/// May be called before START.
/// May be called multiple times.
pub const MTRACE_PERFMON_STOP: u32 = 5;

/// Finish data collection.
/// Must be called with data collection off.
/// Must be called when done: frees various resources allocated to perform
/// the data collection.
/// May be called multiple times.
pub const MTRACE_PERFMON_FINI: u32 = 6;

/// Encode/decode options values for mtrace_control().
/// At present we just encode the cpu number here.
/// We only support 32 cpus at the moment, the extra bit is for magic values.
pub const MTRACE_PERFMON_OPTIONS_CPU_MASK: u32 = 0x3f;

pub const fn mtrace_perfmon_options(cpu: u32) -> u32 {
    cpu & MTRACE_PERFMON_OPTIONS_CPU_MASK
}

pub const MTRACE_PERFMON_ALL_CPUS: u32 = 32;

pub fn mtrace_perfmon_options_cpu(options: u32) -> u32 {
    options & MTRACE_PERFMON_OPTIONS_CPU_MASK
}

/// The minimum version of the Intel Performance Monitoring Unit supported by the kernel.
pub const MTRACE_X86_INTEL_PMU_MIN_SUPPORTED_VERSION: u8 = 2;

/// The maximum version of the Intel Performance Monitoring Unit supported by the kernel.
pub const MTRACE_X86_INTEL_PMU_MAX_SUPPORTED_VERSION: u8 = 4;
