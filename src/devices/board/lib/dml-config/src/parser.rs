// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{AggregateEntry, BoardConfig, Device, ResourceEntry};
use anyhow::{Context, anyhow, bail};
use fdf_fidl::DriverChannel;
use fidl_fuchsia_driver_metadata as fdr;
use fidl_next_fuchsia_driver_framework as fdf_framework;
use fidl_next_fuchsia_hardware_platform_bus as fpbus;
use phf;
use std::collections::{HashMap, HashSet};
use zx;

pub const BIND_PROTOCOL_DEVICE: u32 = 85; // 0x55
pub const BIND_PLATFORM_DEV_VID_GENERIC: u32 = 0;
pub const BIND_PLATFORM_DEV_DID_DEVICETREE: u32 = 50; // 0x32

pub fn property_int(val: u32) -> fdf_framework::NodePropertyValue {
    fdf_framework::NodePropertyValue::IntValue(val)
}

pub fn property_string(val: &str) -> fdf_framework::NodePropertyValue {
    fdf_framework::NodePropertyValue::StringValue(val.to_string())
}

pub fn property_bool(val: bool) -> fdf_framework::NodePropertyValue {
    fdf_framework::NodePropertyValue::BoolValue(val)
}

fn map_interrupt_mode(mode: Option<&str>) -> fpbus::ZirconInterruptMode {
    match mode {
        Some("EdgeLow") => fpbus::ZirconInterruptMode::EdgeLow,
        Some("EdgeHigh") => fpbus::ZirconInterruptMode::EdgeHigh,
        Some("LevelLow") => fpbus::ZirconInterruptMode::LevelLow,
        Some("LevelHigh") => fpbus::ZirconInterruptMode::LevelHigh,
        Some("EdgeBoth") => fpbus::ZirconInterruptMode::EdgeBoth,
        _ => fpbus::ZirconInterruptMode::Default,
    }
}

pub type DriverSpecificMetadata =
    phf::Map<&'static str, &'static [(&'static str, fn() -> anyhow::Result<Vec<u8>>)]>;

pub fn make_accept_bind_rule(
    key: &str,
    value: fdf_framework::NodePropertyValue,
) -> fdf_framework::BindRule2 {
    fdf_framework::BindRule2 {
        key: key.to_string(),
        condition: fdf_framework::Condition::Accept,
        values: vec![value],
    }
}

pub fn make_property2(
    key: &str,
    value: fdf_framework::NodePropertyValue,
) -> fdf_framework::NodeProperty2 {
    fdf_framework::NodeProperty2 { key: key.to_string(), value }
}

pub fn add_rule_and_property(
    bind_rules: &mut Vec<fdf_framework::BindRule2>,
    properties: &mut Vec<fdf_framework::NodeProperty2>,
    key: &str,
    value: fdf_framework::NodePropertyValue,
) {
    bind_rules.push(make_accept_bind_rule(key, value.clone()));
    properties.push(make_property2(key, value));
}

#[derive(Clone, Copy, Debug)]
pub enum ValueSource {
    ConstraintKey(&'static str),
    ResourceName,
    ResourceNode,
    Integer(u32),
    ProviderId,
}

#[derive(Clone, Copy, Debug)]
pub enum RuleValueType {
    Integer,
    String,
    Bool,
}

#[derive(Clone, Copy, Debug)]
pub enum Destination {
    BindRules,
    Properties,
    Both,
}

#[derive(Clone, Copy, Debug)]
pub struct PropertyRule {
    pub bind_key: &'static str,
    pub sources: &'static [ValueSource],
    pub value_type: RuleValueType,
    pub destination: Destination,
}

#[derive(Clone, Copy, Debug)]
pub enum TransportType {
    None,
    Zircon,
    Driver,
}

#[derive(Clone, Copy, Debug)]
pub struct ServiceBindConfig {
    pub transport: TransportType,
    pub rules: &'static [PropertyRule],
    pub parent_key_sources: &'static [ValueSource],
}

pub const DEFAULT_SERVICE_BIND_CONFIG: ServiceBindConfig = ServiceBindConfig {
    transport: TransportType::Zircon,
    rules: &[],
    parent_key_sources: &[ValueSource::ResourceName],
};

pub struct DmlParserConfig {
    pub service_configs: phf::Map<&'static str, ServiceBindConfig>,
}

pub static STANDARD_SERVICE_CONFIGS: phf::Map<&'static str, ServiceBindConfig> = phf::phf_map! {
    "fuchsia.clock.Init" => ServiceBindConfig {
        transport: TransportType::None,
        rules: &[PropertyRule {
            bind_key: "fuchsia.BIND_INIT_STEP",
            sources: &[ValueSource::Integer(0x494B4C43)],
            value_type: RuleValueType::Integer,
            destination: Destination::Both,
        }],
        ..DEFAULT_SERVICE_BIND_CONFIG
    },
    "fuchsia.pwm.Init" => ServiceBindConfig {
        transport: TransportType::None,
        rules: &[PropertyRule {
            bind_key: "fuchsia.BIND_INIT_STEP",
            sources: &[ValueSource::Integer(0x004D5750)],
            value_type: RuleValueType::Integer,
            destination: Destination::Both,
        }],
        ..DEFAULT_SERVICE_BIND_CONFIG
    },
    "fuchsia.gpio.Init" => ServiceBindConfig {
        transport: TransportType::None,
        rules: &[PropertyRule {
            bind_key: "fuchsia.BIND_INIT_STEP",
            sources: &[ValueSource::Integer(0x4F495047)],
            value_type: RuleValueType::Integer,
            destination: Destination::Both,
        }],
        ..DEFAULT_SERVICE_BIND_CONFIG
    },
    "fuchsia.hardware.gpu.mali.Service" => ServiceBindConfig {
        transport: TransportType::Driver,
        ..DEFAULT_SERVICE_BIND_CONFIG
    },
    "fuchsia.hardware.platform.device.Service" => ServiceBindConfig {
        rules: &[
            PropertyRule {
                bind_key: "fuchsia.BIND_PROTOCOL",
                sources: &[ValueSource::Integer(BIND_PROTOCOL_DEVICE)],
                value_type: RuleValueType::Integer,
                destination: Destination::Both,
            },
            PropertyRule {
                bind_key: "fuchsia.BIND_PLATFORM_DEV_VID",
                sources: &[ValueSource::ConstraintKey("vid")],
                value_type: RuleValueType::Integer,
                destination: Destination::Both,
            },
            PropertyRule {
                bind_key: "fuchsia.BIND_PLATFORM_DEV_DID",
                sources: &[ValueSource::ConstraintKey("did")],
                value_type: RuleValueType::Integer,
                destination: Destination::Both,
            },
        ],
        ..DEFAULT_SERVICE_BIND_CONFIG
    },
    "fuchsia.hardware.platform.bus.Service" => ServiceBindConfig {
        transport: TransportType::Driver,
        rules: &[
            PropertyRule {
                bind_key: "fuchsia.BIND_PLATFORM_DEV_VID",
                sources: &[ValueSource::ConstraintKey("vid")],
                value_type: RuleValueType::Integer,
                destination: Destination::Both,
            },
            PropertyRule {
                bind_key: "fuchsia.BIND_PLATFORM_DEV_PID",
                sources: &[ValueSource::ConstraintKey("pid")],
                value_type: RuleValueType::Integer,
                destination: Destination::Both,
            },
            PropertyRule {
                bind_key: "fuchsia.BIND_PLATFORM_DEV_DID",
                sources: &[ValueSource::ConstraintKey("did")],
                value_type: RuleValueType::Integer,
                destination: Destination::Both,
            },
        ],
        ..DEFAULT_SERVICE_BIND_CONFIG
    },
};

pub const DEFAULT_DML_PARSER_CONFIG: DmlParserConfig =
    DmlParserConfig { service_configs: phf::phf_map! {} };

impl Default for DmlParserConfig {
    fn default() -> Self {
        DEFAULT_DML_PARSER_CONFIG
    }
}

fn resolve_value(
    _provider: &str,
    provider_id: u32,
    source: &ValueSource,
    res: &ResourceEntry,
    constraint: &fdr::Dictionary,
) -> Option<ResolvedValue> {
    match source {
        ValueSource::ConstraintKey(key) => {
            if let Some(i) = crate::get_int64(constraint, key) {
                Some(ResolvedValue::Integer(i as u32))
            } else if let Some(s) = crate::get_string(constraint, key) {
                Some(ResolvedValue::String(s))
            } else if let Some(b) = crate::get_bool(constraint, key) {
                Some(ResolvedValue::Bool(b))
            } else {
                None
            }
        }
        ValueSource::ResourceName => res.name.clone().map(ResolvedValue::String),
        ValueSource::ResourceNode => res.node.clone().map(ResolvedValue::String),
        ValueSource::Integer(i) => Some(ResolvedValue::Integer(*i)),
        ValueSource::ProviderId => Some(ResolvedValue::Integer(provider_id)),
    }
}

enum ResolvedValue {
    Integer(u32),
    String(String),
    Bool(bool),
}

fn apply_rule(
    provider: &str,
    provider_id: u32,
    rule: &PropertyRule,
    res: &ResourceEntry,
    constraint: &fdr::Dictionary,
    bind_rules: &mut Vec<fdf_framework::BindRule2>,
    properties: &mut Vec<fdf_framework::NodeProperty2>,
) -> anyhow::Result<()> {
    let resolved = rule
        .sources
        .iter()
        .find_map(|source| resolve_value(provider, provider_id, source, res, constraint));

    let Some(val) = resolved else {
        return Ok(());
    };

    let property_value = match (val, rule.value_type) {
        (ResolvedValue::Integer(i), RuleValueType::Integer) => property_int(i),
        (ResolvedValue::String(s), RuleValueType::String) => property_string(&s),
        (ResolvedValue::Bool(b), RuleValueType::Bool) => property_bool(b),
        (ResolvedValue::String(s), RuleValueType::Integer) => {
            let parsed = if s.starts_with("0x") || s.starts_with("0X") {
                u32::from_str_radix(&s[2..], 16)
            } else {
                s.parse::<u32>()
            };
            match parsed {
                Ok(i) => property_int(i),
                Err(e) => {
                    bail!("Failed to parse string {} as integer: {}", s, e);
                }
            }
        }
        (ResolvedValue::String(s), RuleValueType::Bool) => match s.parse::<bool>() {
            Ok(b) => property_bool(b),
            Err(e) => {
                bail!("Failed to parse string {} as bool: {}", s, e);
            }
        },
        _ => {
            bail!("Type mismatch in rule resolution");
        }
    };

    match rule.destination {
        Destination::BindRules => {
            bind_rules.push(make_accept_bind_rule(rule.bind_key, property_value));
        }
        Destination::Properties => {
            properties.push(make_property2(rule.bind_key, property_value));
        }
        Destination::Both => {
            add_rule_and_property(bind_rules, properties, rule.bind_key, property_value);
        }
    }

    Ok(())
}

pub fn generate_parent_spec_generic(
    provider: &str,
    provider_id: u32,
    service_name: &str,
    res: &ResourceEntry,
    config: &DmlParserConfig,
) -> anyhow::Result<Option<(fdf_framework::ParentSpec2, String)>> {
    let mut bind_rules = Vec::new();
    let mut properties = Vec::new();

    let service_config = config
        .service_configs
        .get(service_name)
        .or_else(|| STANDARD_SERVICE_CONFIGS.get(service_name))
        .unwrap_or(&DEFAULT_SERVICE_BIND_CONFIG);

    match service_config.transport {
        TransportType::Zircon => {
            add_rule_and_property(
                &mut bind_rules,
                &mut properties,
                "fuchsia.Service",
                property_string(service_name),
            );
            properties.push(make_property2(
                service_name,
                property_string(&format!("{service_name}.ZirconTransport")),
            ));
        }
        TransportType::Driver => {
            add_rule_and_property(
                &mut bind_rules,
                &mut properties,
                "fuchsia.Service",
                property_string(service_name),
            );
            properties.push(make_property2(
                service_name,
                property_string(&format!("{service_name}.DriverTransport")),
            ));
        }
        TransportType::None => {}
    }

    let constraint =
        res.constraint.as_ref().ok_or_else(|| anyhow!("constraint is missing in ResourceEntry"))?;

    for rule in service_config.rules {
        apply_rule(provider, provider_id, rule, res, constraint, &mut bind_rules, &mut properties)?;
    }

    if !properties.iter().any(|p| p.key == "fuchsia.NAME") {
        let name_opt = res.name.clone().or_else(|| crate::get_string(constraint, "name"));
        if let Some(name) = name_opt {
            properties.push(make_property2("fuchsia.NAME", property_string(&name)));
        }
    }

    if service_config.rules.is_empty() && !bind_rules.iter().any(|r| r.key == "fuchsia.ID") {
        // TODO(https://fxbug.dev/555962083): Remove this hack
        let id_opt =
            crate::get_int64(constraint, "node_id").or_else(|| crate::get_int64(constraint, "id"));
        if let Some(id) = id_opt {
            bind_rules.push(make_accept_bind_rule("fuchsia.ID", property_int(id as u32)));
        }
    }

    let resolved_key = service_config.parent_key_sources.iter().find_map(|source| {
        match resolve_value(provider, provider_id, source, res, constraint) {
            Some(ResolvedValue::String(k)) => Some(k),
            _ => None,
        }
    });

    let key = match resolved_key {
        Some(k) => k,
        None => {
            bail!(
                "Failed to resolve parent key for service {}. If this service should be ignored, add it to the config.",
                service_name
            );
        }
    };

    let parent = fdf_framework::ParentSpec2 { bind_rules, properties };
    Ok(Some((parent, key)))
}

fn get_aggregate_id(agg: &AggregateEntry, devices: &[Device], fallback_id: u32) -> u32 {
    let device = devices.iter().find(|d| d.name.as_deref() == agg.provider.as_deref());
    let id = device.and_then(|d| d.id).unwrap_or(fallback_id);
    log::debug!(
        "get_aggregate_id: provider={:?}, device_found={}, device_id={:?}, final_id={}, fallback={}",
        agg.provider,
        device.is_some(),
        device.and_then(|d| d.id),
        id,
        fallback_id
    );
    id
}

pub async fn publish_dml_devices(
    pbus: &fidl_next::Client<fpbus::PlatformBus, DriverChannel>,
    composite_manager: &fidl_next::Client<fdf_framework::CompositeNodeManager, zx::Channel>,
    config: &BoardConfig,
    parser_config: &DmlParserConfig,
    driver_metadata: Option<&DriverSpecificMetadata>,
) -> anyhow::Result<()> {
    let mut provider_metadata = HashMap::<String, Vec<fpbus::Metadata>>::new();

    // 1. Generate driver specific metadata for devices in config
    if let (Some(drv_meta), Some(devices)) = (driver_metadata, &config.devices) {
        for dev in devices {
            let name = dev.name.as_deref().unwrap_or("");
            if let Some(generators) = drv_meta.get(name) {
                for (metadata_id, gen_fn) in *generators {
                    let data = gen_fn()
                        .with_context(|| format!("Failed to generate metadata for {}", name))?;
                    provider_metadata.entry(name.to_string()).or_default().push(fpbus::Metadata {
                        id: Some(metadata_id.to_string()),
                        data: Some(data),
                        ..Default::default()
                    });
                }
            }
        }
    }

    let devices = config
        .devices
        .as_ref()
        .ok_or_else(|| anyhow!("devices field is missing in BoardConfig"))?;
    for (idx, dev) in devices.iter().enumerate() {
        let instance_id = idx as u32 + 1;
        let mut node = fpbus::Node {
            name: dev.name.clone(),
            vid: Some(0),
            pid: Some(0),
            did: Some(0),
            instance_id: Some(instance_id),
            driver_host: dev.url.clone(),
            ..Default::default()
        };

        if let Some(compatible) = &dev.compatible {
            node.properties = Some(vec![fdf_framework::NodeProperty2 {
                key: "fuchsia.COMPATIBLE".to_string(),
                value: fdf_framework::NodePropertyValue::StringValue(compatible.clone()),
            }]);
            node.did = Some(BIND_PLATFORM_DEV_DID_DEVICETREE);
            node.vid = Some(BIND_PLATFORM_DEV_VID_GENERIC);
        }

        let mut mmio_list = Vec::new();
        let mut irq_list = Vec::new();
        let mut bti_list = Vec::new();
        let mut smc_list = Vec::new();
        let mut boot_metadata_list = Vec::new();

        if let Some(pdev_dict) = crate::pdev_constraints(config, dev.name.as_deref().unwrap_or(""))
        {
            for mmio in crate::mmio_list(pdev_dict) {
                mmio_list.push(fpbus::Mmio {
                    base: Some(mmio.base),
                    length: Some(mmio.length),
                    name: mmio.name,
                    ..Default::default()
                });
            }
            for irq in crate::irq_list(pdev_dict) {
                irq_list.push(fpbus::Irq {
                    irq: Some(fpbus::IrqSpec::Irq(irq.number)),
                    mode: Some(map_interrupt_mode(irq.mode.as_deref())),
                    name: irq.name.clone(),
                    wake_vector: irq.wake_vector,
                    ..Default::default()
                });
            }
            for bti in crate::bti_list(pdev_dict) {
                bti_list.push(fpbus::Bti {
                    iommu_id: Some(0),
                    bti_id: Some(bti.id),
                    ..Default::default()
                });
            }
            for smc in crate::smc_list(pdev_dict) {
                smc_list.push(fpbus::Smc {
                    service_call_num_base: Some(smc.service_call_num_base),
                    count: Some(smc.count),
                    exclusive: Some(smc.exclusive),
                    name: smc.name,
                    ..Default::default()
                });
            }
            for bm in crate::boot_metadata_list(pdev_dict) {
                boot_metadata_list.push(fpbus::BootMetadata {
                    zbi_type: Some(bm.zbi_type),
                    zbi_extra: bm.zbi_extra.or(Some(0)),
                    ..Default::default()
                });
            }
        }

        if !mmio_list.is_empty() {
            node.mmio = Some(mmio_list);
        }
        if !irq_list.is_empty() {
            node.irq = Some(irq_list);
        }
        if !bti_list.is_empty() {
            node.bti = Some(bti_list);
        }
        if !smc_list.is_empty() {
            node.smc = Some(smc_list);
        }
        if !boot_metadata_list.is_empty() {
            node.boot_metadata = Some(boot_metadata_list);
        }

        let mut metadata_list = Vec::new();

        // Handle static metadata from config
        if let Some(metadata) = &dev.metadata {
            for meta in metadata {
                let data =
                    meta.data.clone().ok_or_else(|| anyhow!("Static metadata missing data"))?;
                metadata_list.push(fpbus::Metadata {
                    id: meta.id.clone(),
                    data: Some(data),
                    ..Default::default()
                });
            }
        }

        let dev_name = dev.name.as_deref().unwrap_or("");
        if let Some(meta) = provider_metadata.get(dev_name) {
            metadata_list.extend(meta.clone());
        }

        if !metadata_list.is_empty() {
            node.metadata = Some(metadata_list);
        }

        let mut resource_parents = Vec::new();
        let mut generated_keys = HashSet::new();
        if let Some(aggregates) = &config.aggregates {
            for (agg_idx, agg) in aggregates.iter().enumerate() {
                if let Some(resources) = &agg.resources {
                    for res in resources {
                        if res.node.as_deref() == dev.name.as_deref() {
                            let parent_and_key = if agg.service.as_deref()
                                == Some("fuchsia.hardware.platform.device.Service")
                                && dev.compatible.is_some()
                            {
                                None
                            } else {
                                generate_parent_spec_generic(
                                    agg.provider.as_deref().unwrap_or(""),
                                    get_aggregate_id(
                                        agg,
                                        config.devices.as_deref().unwrap_or(&[]),
                                        agg_idx as u32,
                                    ),
                                    agg.service.as_deref().unwrap_or(""),
                                    res,
                                    parser_config,
                                )?
                            };
                            if let Some((parent, key)) = parent_and_key {
                                if generated_keys.insert(key) {
                                    if matches!(
                                        agg.provider.as_deref(),
                                        Some("parent") | Some("pdev")
                                    ) {
                                        resource_parents.insert(0, parent);
                                    } else {
                                        resource_parents.push(parent);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        let mut spec =
            fdf_framework::CompositeNodeSpec { name: dev.name.clone(), ..Default::default() };

        let mut parents2 = Vec::new();

        // 1. Generate pdev parent if compatible is present
        if let Some(compatible) = &dev.compatible {
            let pdev_parent = fdf_framework::ParentSpec2 {
                bind_rules: vec![
                    make_accept_bind_rule(
                        "fuchsia.BIND_PROTOCOL",
                        property_int(BIND_PROTOCOL_DEVICE),
                    ),
                    make_accept_bind_rule(
                        "fuchsia.BIND_PLATFORM_DEV_VID",
                        property_int(BIND_PLATFORM_DEV_VID_GENERIC),
                    ),
                    make_accept_bind_rule(
                        "fuchsia.BIND_PLATFORM_DEV_DID",
                        property_int(BIND_PLATFORM_DEV_DID_DEVICETREE),
                    ),
                    make_accept_bind_rule(
                        "fuchsia.BIND_PLATFORM_DEV_INSTANCE_ID",
                        property_int(instance_id),
                    ),
                    make_accept_bind_rule("fuchsia.COMPATIBLE", property_string(compatible)),
                ],
                properties: vec![
                    make_property2("fuchsia.NAME", property_string("pdev")),
                    make_property2("fuchsia.BIND_PROTOCOL", property_int(BIND_PROTOCOL_DEVICE)),
                    make_property2(
                        "fuchsia.BIND_PLATFORM_DEV_VID",
                        property_int(BIND_PLATFORM_DEV_VID_GENERIC),
                    ),
                    make_property2(
                        "fuchsia.BIND_PLATFORM_DEV_DID",
                        property_int(BIND_PLATFORM_DEV_DID_DEVICETREE),
                    ),
                    make_property2(
                        "fuchsia.BIND_PLATFORM_DEV_INSTANCE_ID",
                        property_int(instance_id),
                    ),
                    make_property2("fuchsia.COMPATIBLE", property_string(compatible)),
                    make_property2(
                        "fuchsia.devicetree.FIRST_COMPATIBLE",
                        property_string(compatible),
                    ),
                    make_property2(
                        "fuchsia.Service",
                        property_string("fuchsia.hardware.platform.device.Service"),
                    ),
                ],
            };
            parents2.push(pdev_parent);
        }

        parents2.extend(resource_parents);

        if !parents2.is_empty() {
            spec.parents2 = Some(parents2);

            log::debug!(
                "DML-CONFIG: Adding composite spec: name={:?}, parents2={:?}",
                spec.name,
                spec.parents2
            );
            composite_manager
                .add_spec_with(spec)
                .await
                .context("AddSpec request failed")?
                .map_err(|e| anyhow!("AddSpec failed: {e:?}"))?;
        }

        if dev.compatible.is_some() {
            log::debug!("DML-CONFIG: Adding node: {}", dev_name);
            pbus.node_add(node)
                .await
                .context("NodeAdd request failed")?
                .map_err(|e| e.err().unwrap_or(zx::Status::INTERNAL))
                .context("NodeAdd failed")?;
        }
    }

    Ok(())
}
