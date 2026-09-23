// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::types::{OperatingPoint, ThermalLoad};
use fidl_fuchsia_power_cpu as fcpu;
use zx::sys;

/// Defines the message types and arguments to be used for inter-node communication
#[derive(Debug, PartialEq)]
#[allow(dead_code)]
pub enum Message {
    /// Get the number of CPUs in the system
    GetNumCpus,

    /// Get the current load from each CPU in the system as a vector of values in the range [0.0 -
    /// 1.0]. Load is calculated by dividing the total time a CPU spent not idle during a duration
    /// by the total time elapsed during the same duration, where the duration is defined as the
    /// time since the previous GetCpuLoads call. The returned vector will have NUM_CPUS elements,
    /// where NUM_CPUS is the value returned by the GetNumCpus message.
    GetCpuLoads,

    /// Get all operating points for the handler's CPU domain
    GetCpuOperatingPoints,

    // Issues the zx_system_set_performance_info syscall.
    SetCpuPerformanceInfo(Vec<sys::zx_cpu_performance_info_t>),

    /// Get the current operating point
    GetOperatingPoint,

    /// Set the new operating point
    /// Args: a value in the range [0 - x] where x is an upper bound defined in the
    /// dev_control_handler crate. An increasing value indicates a lower operating point.
    SetOperatingPoint(u32),

    /// Set the maximum operating point limit.
    SetMaximumOperatingPointLimit(u32),

    /// Set the minimum operating point limit.
    SetMinimumOperatingPointLimit(u32),

    /// Communicate a thermal load value
    UpdateThermalLoad(ThermalLoad),

    /// Whether or not to enable cpu boost.
    /// Args:
    ///     - enable: true to enable boost, false to disable.
    ///     - id: unique identifier for the boost request.
    SetBoost(bool, u64),

    /// Get info about all power domains, i.e. CPU clusters.
    GetDomainInfos,

    /// Gets the max frequency of the CPU cluster.
    /// Arg: the index of the CPU cluster to set the max frequency of.
    GetMaxFrequency(u64),

    /// Sets the max frequency of the CPU cluster.
    /// Args:
    ///     - The index of the CPU cluster to set the max frequency of.
    ///     - The index of the frequency in available_frequencies_hz reported
    ///       by the node via GetCpuClusterInfos. If None, clears the max
    ///       frequency.
    SetMaxFrequency(u64, Option<u64>),
}

/// Defines the return values for each of the Message types from above
#[derive(Debug)]
#[allow(dead_code)]
pub enum MessageReturn {
    /// Arg: the number of CPUs in the system
    GetNumCpus(u32),

    /// Arg: the current load from each CPU in the system as a vector of values in the range [0.0 -
    /// 1.0]. The returned vector will have NUM_CPUS elements, where NUM_CPUS is the value returned
    /// by the GetNumCpus message.
    GetCpuLoads(Vec<f32>),

    /// Arg: all operating points for the CPU domain seviced by the message handler.
    GetCpuOperatingPoints(Vec<OperatingPoint>),

    /// There is no arg in this MessageReturn type. It only serves as an ACK.
    SetCpuPerformanceInfo,

    /// Arg: the operating point returned from the node
    GetOperatingPoint(u32),

    /// There is no arg in this MessageReturn type. It only serves as an ACK.
    SetOperatingPoint,

    /// There is no arg in this MessageReturn type. It only serves as an ACK.
    SetMaximumOperatingPointLimit,

    /// There is no arg in this MessageReturn type. It only serves as an ACK.
    SetMinimumOperatingPointLimit,

    /// There is no arg in this MessageReturn type. It only serves as an ACK.
    UpdateThermalLoad,

    /// There is no arg in this MessageReturn type. It only serves as an ACK.
    SetBoost,

    /// Arg: list of information about each CPU cluster.
    GetDomainInfos(Vec<fcpu::DomainInfo>),

    /// Arg: the frequency index of the CPU cluster.
    GetMaxFrequency(u64),

    /// There is no arg in this MessageReturn type. It only serves as an ACK.
    SetMaxFrequency,
}

pub type MessageResult = Result<MessageReturn, crate::error::CpuManagerError>;
