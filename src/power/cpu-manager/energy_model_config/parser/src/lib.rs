// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Error, ensure};
use serde_derive::Deserialize;
use std::fs::File;
use std::io::Read as _;
use std::path::Path;
use zx_types::{
    ZX_CPU_SET_BITS_PER_WORD, ZX_CPU_SET_MAX_CPUS, ZX_MAX_NAME_LEN,
    ZX_PROCESSOR_POWER_CONTROL_ARM_PSCI, ZX_PROCESSOR_POWER_CONTROL_ARM_WFI,
    ZX_PROCESSOR_POWER_CONTROL_CPU_DRIVER, ZX_PROCESSOR_POWER_CONTROL_RISCV_SBI,
    ZX_PROCESSOR_POWER_CONTROL_RISCV_WFI, ZX_PROCESSOR_POWER_LEVEL_OPTIONS_DOMAIN_INDEPENDENT,
    zx_cpu_set_t, zx_processor_power_domain_t, zx_processor_power_level_t,
    zx_processor_power_level_transition_t,
};

/// This library is used to parse a processor energy model JSON file into a data structure which
/// also implements some convenience methods for accessing and consuming the data.
///
/// The intended usage is that `EnergyModel::new()` is called with an energy model JSON file path.
/// If successful, the function returns an EnergyModel instance containing the parsed config data.
///
/// The parser expects a JSON5 file of the following format:
///     [
///         {
///             power_domain: {
///                 cpu_set: [
///                     1,
///                     2,
///                     3,
///                 ],
///                 domain_id: 0,
///             },
///             power_levels: [
///                 {
///                     processing_rate: 200,
///                     power_coefficient_nw: 100,
///                     control_interface: 'CpuDriver',
///                     control_argument: 0,
///                     diagnostic_name: 'Pstate 0',
///                 },
///                 {
///                     processing_rate: 100,
///                     power_coefficient_nw: 50,
///                     control_interface: 'CpuDriver',
///                     control_argument: 0,
///                     diagnostic_name: 'Pstate 1',
///                 },
///             ],
///             power_level_transitions: [
///                 {
///                     from: 0,
///                     to: 1,
///                     duration_ns: 100,
///                     energy_nj: 100,
///                 },
///             ],
///         },
///         {
///             power_domain: {
///                 cpu_set: [
///                     0,
///                     4,
///                 ],
///                 domain_id: 1,
///             },
///             power_levels: {
///                 option: 'DomainIndependent',
///                 processing_rate: 200,
///                 power_coefficient_nw: 200,
///                 control_interface: 'CpuDriver',
///                 control_argument: 0,
///                 diagnostic_name: 'Pstate 0',
///             },
///             power_level_transitions: [
///             ],
///         },
///     ]

/// Defines a Json compatible energy model struct of a specific set of CPUs.
#[derive(Deserialize, Debug, Clone, PartialEq)]
pub struct PowerLevelDomainJson {
    // Represents CPUs that are a subset of a domain.
    pub power_domain: PowerDomain,

    // Power levels are implicitly enumerated as the indices of `power_levels`.
    pub power_levels: Vec<PowerLevel>,

    // There are no ordering restrictions on `power_level_transitions` and the
    // power levels they encode correspond to those defined by `power_levels`.
    // An absent transition between levels is assumed to indicate that there
    // is no energy or latency cost borne by performing it.
    pub power_level_transitions: Vec<PowerLevelTransition>,
}

#[derive(Deserialize, Debug, Clone, PartialEq)]
pub struct PowerDomain {
    // A vector of CPU indexes.
    pub cpu_set: Vec<u16>,
    // A unique identifier for all CPUs in the cpu_set.
    pub domain_id: u32,
}

#[derive(Deserialize, Debug, Clone, PartialEq)]
pub struct PowerLevel {
    pub option: Option<PowerLevelOption>,
    pub processing_rate: u64,
    pub power_coefficient_nw: u64,
    pub control_interface: ControlInterface,
    pub control_argument: u64,
    pub diagnostic_name: String,
}

#[derive(Deserialize, Debug, Clone, PartialEq)]
pub struct PowerLevelTransition {
    pub from: u8,
    pub to: u8,
    pub duration_ns: i64,
    pub energy_nj: u64,
}

#[derive(Deserialize, Debug, Clone, PartialEq)]
pub enum ControlInterface {
    CpuDriver,
    ArmPsci,
    ArmWfi,
    RiscvSbi,
    RiscvWfi,
}

#[derive(Deserialize, Debug, Clone, PartialEq)]
pub enum PowerLevelOption {
    DomainIndependent,
}

impl PowerLevelDomainJson {
    pub fn validate(&self) -> Result<(), Error> {
        let power_level_num = self.power_levels.len();
        for power_level_transition in self.power_level_transitions.iter() {
            let from_index = power_level_transition.from as usize;
            let to_index = power_level_transition.to as usize;
            ensure!(
                from_index != to_index
                    && from_index < power_level_num
                    && to_index < power_level_num,
                "Power level transition need to be between two different valid indexes",
            );
            let from_processing_rate = self.power_levels[from_index].processing_rate;
            let to_processing_rate = self.power_levels[to_index].processing_rate;
            ensure!(
                from_processing_rate != 0 || to_processing_rate != 0,
                "Transitions directly from one idle power level to another idle power level are invalid",
            );
        }
        Ok(())
    }
}

/// Represents the top level of an energy model structure.
#[derive(Debug, Clone, PartialEq)]
pub struct EnergyModel(pub Vec<PowerLevelDomain>);

/// Zircon compatible version of PowerLevelDomainJson.
#[derive(Debug, Clone, PartialEq)]
pub struct PowerLevelDomain {
    pub power_domain: zx_processor_power_domain_t,
    pub power_levels: Vec<zx_processor_power_level_t>,
    pub power_level_transitions: Vec<zx_processor_power_level_transition_t>,
}

impl EnergyModel {
    pub fn new(json_file_path: &Path) -> Result<EnergyModel, Error> {
        let mut buffer = String::new();
        File::open(&json_file_path)?.read_to_string(&mut buffer)?;

        let power_level_domains = serde_json5::from_str::<Vec<PowerLevelDomainJson>>(&buffer)?;

        // Iterate and validate each underlying PowerLevelDomain instance.
        for power_level_domain in power_level_domains.iter() {
            power_level_domain.validate().context(format!(
                "Validation failed for power_domain {}",
                power_level_domain.power_domain.domain_id
            ))?;
        }

        Ok(Self(
            power_level_domains
                .into_iter()
                .map(|s| s.try_into())
                .collect::<Result<Vec<PowerLevelDomain>, _>>()?,
        ))
    }
}

impl TryFrom<PowerLevelDomainJson> for PowerLevelDomain {
    type Error = anyhow::Error;
    fn try_from(e: PowerLevelDomainJson) -> Result<Self, Error> {
        Ok(Self {
            power_domain: e.power_domain.try_into()?,
            power_levels: e
                .power_levels
                .into_iter()
                .map(|s| s.try_into())
                .collect::<Result<Vec<zx_processor_power_level_t>, _>>()?,
            power_level_transitions: e
                .power_level_transitions
                .into_iter()
                .map(|s| s.into())
                .collect(),
        })
    }
}

impl TryFrom<PowerDomain> for zx_processor_power_domain_t {
    type Error = anyhow::Error;
    fn try_from(p: PowerDomain) -> Result<Self, Error> {
        let mut ret = Self::default();
        ret.cpus = get_cpu_mask(p.cpu_set)?;
        ret.domain_id = p.domain_id;
        Ok(ret)
    }
}

impl From<PowerLevelTransition> for zx_processor_power_level_transition_t {
    fn from(p: PowerLevelTransition) -> Self {
        let mut ret = Self::default();
        ret.from = p.from;
        ret.to = p.to;
        ret.latency = p.duration_ns;
        ret.energy = p.energy_nj;
        ret
    }
}

impl TryFrom<PowerLevel> for zx_processor_power_level_t {
    type Error = anyhow::Error;
    fn try_from(p: PowerLevel) -> Result<Self, Error> {
        let control_interface = match p.control_interface {
            ControlInterface::CpuDriver => ZX_PROCESSOR_POWER_CONTROL_CPU_DRIVER,
            ControlInterface::ArmPsci => ZX_PROCESSOR_POWER_CONTROL_ARM_PSCI,
            ControlInterface::ArmWfi => ZX_PROCESSOR_POWER_CONTROL_ARM_WFI,
            ControlInterface::RiscvSbi => ZX_PROCESSOR_POWER_CONTROL_RISCV_SBI,
            ControlInterface::RiscvWfi => ZX_PROCESSOR_POWER_CONTROL_RISCV_WFI,
        };

        let options = match p.option {
            Some(PowerLevelOption::DomainIndependent) => {
                ZX_PROCESSOR_POWER_LEVEL_OPTIONS_DOMAIN_INDEPENDENT
            }
            _ => 0,
        };

        let bytes = p.diagnostic_name.as_bytes();
        ensure!(
            bytes.len() <= ZX_MAX_NAME_LEN,
            "diagnostic_name {:?} must be shorter than {:?} bytes",
            p.diagnostic_name,
            ZX_MAX_NAME_LEN,
        );
        let mut diagnostic_name = [0u8; ZX_MAX_NAME_LEN];
        diagnostic_name[..bytes.len()].copy_from_slice(bytes);

        let mut ret = Self::default();
        ret.options = options;
        ret.processing_rate = p.processing_rate;
        ret.power_coefficient_nw = p.power_coefficient_nw;
        ret.control_interface = control_interface;
        ret.control_argument = p.control_argument;
        ret.diagnostic_name = diagnostic_name;
        Ok(ret)
    }
}

// Convert from a list of CPU indexes to CPU mask.
//
// Zircon definition:
// The |N|'th CPU is considered in the CPU set if the bit:
//
//   cpu_mask[N / ZX_CPU_SET_BITS_PER_WORD]
//       & (1 << (N % ZX_CPU_SET_BITS_PER_WORD))
//
// is set.
fn get_cpu_mask(cpu_set: Vec<u16>) -> Result<zx_cpu_set_t, anyhow::Error> {
    let mut cpu_mask = [0 as u64; ZX_CPU_SET_MAX_CPUS / ZX_CPU_SET_BITS_PER_WORD];
    for cpu in cpu_set {
        let cpu_index = cpu as usize;
        ensure!(
            cpu_index < ZX_CPU_SET_MAX_CPUS,
            "CPU index must be smaller than {:?}",
            ZX_CPU_SET_MAX_CPUS,
        );

        let i = cpu_index / ZX_CPU_SET_BITS_PER_WORD;
        cpu_mask[i] = cpu_mask[i] | (1 << (cpu_index % ZX_CPU_SET_BITS_PER_WORD));
    }
    Ok(zx_cpu_set_t { mask: cpu_mask })
}

#[cfg(test)]
mod tests {
    use crate::*;
    use assert_matches::assert_matches;

    #[fuchsia::test]
    fn test_convert_cpu_mask() {
        assert_eq!(
            get_cpu_mask(vec![0u16, 1u16, 2u16, 3u16]).unwrap().mask,
            [15u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64]
        );

        fn is_nth_set(
            cpu_mask: [u64; ZX_CPU_SET_MAX_CPUS / ZX_CPU_SET_BITS_PER_WORD],
            n: usize,
        ) -> bool {
            cpu_mask[n / ZX_CPU_SET_BITS_PER_WORD] & (1 << (n % ZX_CPU_SET_BITS_PER_WORD)) != 0
        }

        let mut cpu_set = vec![15u16];
        let mut cpu_mask = get_cpu_mask(cpu_set).unwrap().mask;
        assert!(is_nth_set(cpu_mask, 15));

        cpu_set = vec![32u16];
        cpu_mask = get_cpu_mask(cpu_set).unwrap().mask;
        assert!(is_nth_set(cpu_mask, 32));

        // CPU index must be smaller than ZX_CPU_SET_MAX_CPUS.
        cpu_set = vec![ZX_CPU_SET_MAX_CPUS as u16];
        assert_matches!(get_cpu_mask(cpu_set), Err(_));
    }

    #[fuchsia::test]
    fn test_processor_power_level() {
        let power_level = PowerLevel {
            option: Some(PowerLevelOption::DomainIndependent),
            processing_rate: 0,
            power_coefficient_nw: 0,
            control_interface: ControlInterface::ArmPsci,
            control_argument: 0,
            diagnostic_name:
                "This is a very very very long diagnositc name which exceeds 32 bytes.".to_string(),
        };
        assert_matches!(zx_processor_power_level_t::try_from(power_level), Err(_));

        let mut zx_power_level = zx_processor_power_level_t::default();
        zx_power_level.control_interface = ZX_PROCESSOR_POWER_CONTROL_CPU_DRIVER;
        let power_level = PowerLevel {
            option: None,
            processing_rate: 0,
            power_coefficient_nw: 0,
            control_interface: ControlInterface::CpuDriver,
            control_argument: 0,
            diagnostic_name: "".to_string(),
        };

        assert!(zx_processor_power_level_t::try_from(power_level).unwrap() == zx_power_level);
    }
}
