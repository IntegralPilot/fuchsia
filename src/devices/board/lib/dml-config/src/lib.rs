// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_board_dml_config as fbdc;
use fidl_fuchsia_driver_metadata as fdr;

// Re-export FIDL types for convenience
pub use fbdc::{AggregateEntry, BoardConfig, Device, ResourceEntry, StaticMetadata};

#[derive(Debug, Clone, Default)]
pub struct Mmio {
    pub name: Option<String>,
    pub base: u64,
    pub length: u64,
}

#[derive(Debug, Clone, Default)]
pub struct Irq {
    pub name: Option<String>,
    pub number: u32,
    pub mode: Option<String>,
    pub wake_vector: Option<bool>,
}

#[derive(Debug, Clone, Default)]
pub struct Bti {
    pub id: u32,
}

#[derive(Debug, Clone, Default)]
pub struct Smc {
    pub service_call_num_base: u32,
    pub count: u32,
    pub exclusive: bool,
    pub name: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct BootMetadata {
    pub zbi_type: u32,
    pub zbi_extra: Option<u32>,
}

// Dictionary Lookup Helpers
pub fn find_value<'a>(dict: &'a fdr::Dictionary, key: &str) -> Option<&'a fdr::DictionaryValue> {
    dict.entries.as_ref()?.iter().find(|e| e.key == key).map(|e| &e.value)
}

pub fn get_int64(dict: &fdr::Dictionary, key: &str) -> Option<i64> {
    match find_value(dict, key)? {
        fdr::DictionaryValue::Int64(i) => Some(*i),
        _ => None,
    }
}

pub fn get_uint32(dict: &fdr::Dictionary, key: &str) -> Option<u32> {
    get_int64(dict, key).and_then(|i| u32::try_from(i).ok())
}

pub fn get_uint64(dict: &fdr::Dictionary, key: &str) -> Option<u64> {
    get_int64(dict, key).map(|i| i as u64)
}

pub fn get_string(dict: &fdr::Dictionary, key: &str) -> Option<String> {
    match find_value(dict, key)? {
        fdr::DictionaryValue::Str(s) => Some(s.clone()),
        _ => None,
    }
}

pub fn get_bool(dict: &fdr::Dictionary, key: &str) -> Option<bool> {
    match find_value(dict, key)? {
        fdr::DictionaryValue::Boolean(b) => Some(*b),
        _ => None,
    }
}

// Resource Parsing Helpers
pub fn mmio_list(dict: &fdr::Dictionary) -> Vec<Mmio> {
    let mut list = Vec::new();
    for i in 0.. {
        let prefix = format!("mmio.{}", i);
        let base = get_uint64(dict, &format!("{}.base", prefix))
            .or_else(|| get_uint64(dict, &format!("{}.address", prefix)));
        if let Some(base) = base {
            let length = get_uint64(dict, &format!("{}.length", prefix))
                .or_else(|| get_uint64(dict, &format!("{}.size", prefix)))
                .unwrap_or(0);
            let name = get_string(dict, &format!("{}.name", prefix));
            list.push(Mmio { name, base, length });
        } else {
            break;
        }
    }
    list
}

pub fn irq_list(dict: &fdr::Dictionary) -> Vec<Irq> {
    let mut list = Vec::new();
    for i in 0.. {
        let prefix = format!("interrupts.{}", i);
        if let Some(number) = get_uint32(dict, &format!("{}.number", prefix)) {
            let name = get_string(dict, &format!("{}.name", prefix));
            let mode = get_string(dict, &format!("{}.mode", prefix));
            let wake_vector = get_bool(dict, &format!("{}.wake_vector", prefix));
            list.push(Irq { name, number, mode, wake_vector });
        } else {
            break;
        }
    }
    list
}

pub fn bti_list(dict: &fdr::Dictionary) -> Vec<Bti> {
    let mut list = Vec::new();
    for i in 0.. {
        let prefix = format!("btis.{}", i);
        if let Some(id) = get_uint32(dict, &format!("{}.id", prefix)) {
            list.push(Bti { id });
        } else {
            break;
        }
    }
    list
}

pub fn smc_list(dict: &fdr::Dictionary) -> Vec<Smc> {
    let mut list = Vec::new();
    for i in 0.. {
        let prefix = format!("smcs.{}", i);
        if let Some(service_call_num_base) =
            get_uint32(dict, &format!("{}.service_call_num_base", prefix))
        {
            let count = get_uint32(dict, &format!("{}.count", prefix)).unwrap_or(0);
            let exclusive = get_bool(dict, &format!("{}.exclusive", prefix)).unwrap_or(false);
            let name = get_string(dict, &format!("{}.name", prefix));
            list.push(Smc { service_call_num_base, count, exclusive, name });
        } else {
            break;
        }
    }
    list
}

pub fn boot_metadata_list(dict: &fdr::Dictionary) -> Vec<BootMetadata> {
    let mut list = Vec::new();
    for i in 0.. {
        let prefix = format!("boot_metadata.{}", i);
        if let Some(zbi_type) = get_uint32(dict, &format!("{}.zbi_type", prefix)) {
            let zbi_extra = get_uint32(dict, &format!("{}.zbi_extra", prefix));
            list.push(BootMetadata { zbi_type, zbi_extra });
        } else {
            break;
        }
    }
    list
}

pub fn pdev_constraints<'a>(
    config: &'a BoardConfig,
    node_name: &str,
) -> Option<&'a fdr::Dictionary> {
    config
        .aggregates
        .as_ref()?
        .iter()
        .find(|agg| {
            agg.provider.as_deref() == Some("pdev")
                && agg.service.as_deref() == Some("fuchsia.hardware.platform.device.Service")
        })
        .and_then(|agg| {
            agg.resources
                .as_ref()?
                .iter()
                .find(|res| res.node.as_deref() == Some(node_name))
                .and_then(|res| res.constraint.as_ref())
        })
}

#[cfg(target_os = "fuchsia")]
pub mod parser;
