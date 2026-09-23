// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Common formatting, path, and service discovery helpers for batteryutil.

use anyhow::{Result, anyhow};
use std::fmt;
use std::path::{Path, PathBuf};

pub const MEMBER_BATTERY: &str = "battery";
pub const MEMBER_DEVICE: &str = "device";
pub const MEMBER_CHARGER: &str = "charger";
pub const MEMBER_CONTROLLER: &str = "controller";

const KNOWN_MEMBER_SUFFIXES: &[&str] =
    &[MEMBER_BATTERY, MEMBER_CHARGER, MEMBER_CONTROLLER, MEMBER_DEVICE];

/// Formats micro-units (uA, uAh, uV) into human-readable quantities with appropriate prefixes.
#[derive(Debug, PartialEq)]
pub struct MicroUnit<T>(pub T, pub &'static str);

impl<T: Into<i64> + Copy> fmt::Display for MicroUnit<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let raw: i64 = self.0.into();
        let sign = if raw < 0 { "-" } else { "" };
        let val: f64 = (raw as f64).abs();
        let (scaled, prefix) = if val >= 1_000_000.0 {
            (val / 1_000_000.0, "")
        } else if val >= 1_000.0 {
            (val / 1_000.0, "m")
        } else {
            (val, "u")
        };
        write!(f, "{}{:.3} {}{}", sign, scaled, prefix, self.1)
    }
}

/// Lists all service instance directory paths found under `/svc/<service_name>`.
pub fn discover_instances(service_name: impl AsRef<str>) -> Vec<String> {
    let service_name = service_name.as_ref();
    let service_dir = Path::new("/svc").join(service_name);
    let mut instances: Vec<String> = std::fs::read_dir(&service_dir)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.ok()?.path().into_os_string().into_string().ok())
        .collect();
    instances.sort();
    instances
}

/// Discovers available service instances in `/svc/<service_name>` and selects the first instance if not specified.
pub fn select_instance(service_name: impl AsRef<str>) -> Result<String> {
    let service_name = service_name.as_ref();
    let instances = discover_instances(service_name);

    if instances.is_empty() {
        return Err(anyhow!("No instances found for {}", service_name));
    }

    if instances.len() == 1 {
        return Ok(instances[0].clone());
    }

    eprintln!("Multiple service instances found for {}:", service_name);
    for (i, inst) in instances.iter().enumerate() {
        eprintln!("  {}. {}", i + 1, inst);
    }
    eprintln!(
        "Defaulting to first instance: {} (use -p to specify a different instance)",
        instances[0]
    );
    Ok(instances[0].clone())
}

/// Appends a member protocol suffix (e.g. `battery` or `charger`) if not already present, stripping
/// any preexisting sibling member suffixes (`battery`, `charger`, `controller`, `device`).
pub fn append_member_suffix(path: impl AsRef<Path>, member_suffix: &str) -> PathBuf {
    let mut path = path.as_ref();
    let member = member_suffix.trim_start_matches('/');

    if let Some(file_name) = path.file_name().and_then(|f| f.to_str()) {
        if KNOWN_MEMBER_SUFFIXES.contains(&file_name) {
            if let Some(parent) = path.parent() {
                path = parent;
            }
        }
    }

    path.join(member)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_unit() {
        assert_eq!(MicroUnit(500i64, "A").to_string(), "500.000 uA");
        assert_eq!(MicroUnit(1_500i64, "A").to_string(), "1.500 mA");
        assert_eq!(MicroUnit(2_500_000i64, "A").to_string(), "2.500 A");
        assert_eq!(MicroUnit(3_774_000i64, "V").to_string(), "3.774 V");
        assert_eq!(MicroUnit(5_040_000i64, "Ah").to_string(), "5.040 Ah");

        // Negative values
        assert_eq!(MicroUnit(-520_312i64, "A").to_string(), "-520.312 mA");
        assert_eq!(MicroUnit(-1_500_000i64, "A").to_string(), "-1.500 A");
    }

    #[test]
    fn test_append_member_suffix() {
        assert_eq!(
            append_member_suffix("/svc/fuchsia.hardware.power.battery.Service/default", "battery"),
            PathBuf::from("/svc/fuchsia.hardware.power.battery.Service/default/battery")
        );
        assert_eq!(
            append_member_suffix(
                "/svc/fuchsia.hardware.power.battery.Service/default/battery",
                "battery"
            ),
            PathBuf::from("/svc/fuchsia.hardware.power.battery.Service/default/battery")
        );
        assert_eq!(
            append_member_suffix("/svc/fuchsia.hardware.power.charger.Service/default", "/charger"),
            PathBuf::from("/svc/fuchsia.hardware.power.charger.Service/default/charger")
        );
        assert_eq!(
            append_member_suffix("/svc/my-battery/default", "battery"),
            PathBuf::from("/svc/my-battery/default/battery")
        );
    }
}
