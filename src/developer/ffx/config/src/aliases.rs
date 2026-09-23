// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::ConfigValue;
use crate::api::value::TryConvert;
use crate::environment::EnvironmentContext;
use crate::nested::nested_get;

use ffx_config_domain::ConfigMap;
use serde_json::Value;

// Mechanisms for implementing config "aliases", in which one config option can be used
// to stand in for a group of other options.  In this (simplistic) implementation, users of
// the aliases option (e.g. "discovery.mdns.enabled") must not query it directly, but must
// instead go through the accessor function.

pub trait ConfigAliases {
    fn get_with_alias(&self, key: &str, alias: &str) -> Option<(ConfigValue, ConfigValue)>;
}

impl ConfigAliases for EnvironmentContext {
    // Return values at first config level for which either the key or the alias has a value
    fn get_with_alias(&self, key: &str, alias: &str) -> Option<(ConfigValue, ConfigValue)> {
        let key_vec: Vec<&str> = key.split('.').collect();
        let alias_vec: Vec<&str> = alias.split('.').collect();
        // These are called only by functions that hard-code the keys, so we won't panic
        let key_head = key_vec[0];
        let alias_head = alias_vec[0];

        for config in self.config.iter() {
            let kval = nested_get(config, key_head, &key_vec[1..]);
            let aval = nested_get(config, alias_head, &alias_vec[1..]);
            if kval.is_some() || aval.is_some() {
                return Some((ConfigValue::from(kval.cloned()), ConfigValue::from(aval.cloned())));
            }
        }
        Some((ConfigValue::from(None), ConfigValue::from(None)))
    }
}

// Specific aliases
//------------------------

// "ffx.isolated"
const FFX_ISOLATED: &str = "ffx.isolated";

const FASTBOOT_USB_DISCOVERY_DISABLED: &str = "fastboot.usb.disabled";
const FFX_ANALYTICS_DISABLED: &str = "ffx.analytics.disabled";
const MDNS_DISCOVERY_ENABLED: &str = "discovery.mdns.enabled";
const MDNS_AUTOCONNECT_ENABLED: &str = "discovery.mdns.autoconnect";

// Get the aliased value, along with the isolation alias -- both bools.
fn get_with_isolated_alias(
    ctx: &EnvironmentContext,
    key: &str,
) -> Option<(Option<bool>, Option<bool>)> {
    let (v, isov) = ctx.get_with_alias(key, FFX_ISOLATED)?;
    Some((bool::try_convert(v).ok(), bool::try_convert(isov).ok()))
}

pub fn is_usb_discovery_disabled(ctx: &EnvironmentContext) -> bool {
    let default = false;
    match get_with_isolated_alias(ctx, FASTBOOT_USB_DISCOVERY_DISABLED) {
        None => return default,
        Some((usb, iso)) => usb.unwrap_or_else(|| iso.unwrap_or(default)),
    }
}

pub fn is_analytics_disabled(ctx: &EnvironmentContext) -> bool {
    let default = false;
    if ctx.has_no_environment() {
        return true;
    }
    match get_with_isolated_alias(ctx, FFX_ANALYTICS_DISABLED) {
        None => return default,
        Some((ad, iso)) => ad.unwrap_or_else(|| iso.unwrap_or(default)),
    }
}

pub fn is_mdns_discovery_disabled(ctx: &EnvironmentContext) -> bool {
    let default = false;
    match get_with_isolated_alias(ctx, MDNS_DISCOVERY_ENABLED) {
        None => return default,
        // The option is _enabled_, so we have to invert it
        Some((mdns_disc, iso)) => mdns_disc.map(|b| !b).unwrap_or_else(|| iso.unwrap_or(default)),
    }
}

pub fn is_mdns_autoconnect_disabled(ctx: &EnvironmentContext) -> bool {
    let default = false;
    match get_with_isolated_alias(ctx, MDNS_AUTOCONNECT_ENABLED) {
        None => return default,
        // The option is _enabled_, so we have to invert it
        Some((mdns_conn, iso)) => mdns_conn.map(|b| !b).unwrap_or_else(|| iso.unwrap_or(default)),
    }
}

/// When run in an isolated dir, also set `ffx.isolated`. This will only work
/// "usefully" if it is invoked with the global EnvironmentContext, i.e. the
/// installed by ffx_config::init()
pub(crate) fn add_isolation_default(cm: &mut ConfigMap) {
    cm.insert(FFX_ISOLATED.into(), Value::Bool(true));
}
//------------------------

////////////////////////////////////////////////////////////////////////////////
// tests
#[cfg(test)]
mod test {
    use super::*;
    use crate::{self as ffx_config};

    #[fuchsia::test]
    fn test_ffx_isolated() {
        let env = ffx_config::test_env()
            .user_config("ffx.isolated", true)
            .build()
            .expect("create test config");

        assert!(is_usb_discovery_disabled(&env.context));
        assert!(is_analytics_disabled(&env.context));
        assert!(is_mdns_discovery_disabled(&env.context));
        assert!(is_mdns_autoconnect_disabled(&env.context));
    }

    #[fuchsia::test]
    fn test_ffx_isolated_can_override_global() {
        let env = ffx_config::test_env()
            .user_config("ffx.isolated", true)
            .global_config("fastboot.usb.disabled", false)
            .build()
            .expect("create test config");

        // Isolation is respected, since it is set at a higher level
        assert!(is_usb_discovery_disabled(&env.context));
    }

    #[fuchsia::test]
    fn test_ffx_isolated_can_be_overridden() {
        let env = ffx_config::test_env()
            .global_config("ffx.isolated", true)
            .user_config("fastboot.usb.disabled", false)
            .build()
            .expect("create test config");

        // Isolation is overridden, since it is set at a lower level
        // (It's not clear we _want_ this behavior, but this is the current plan)
        assert!(!is_usb_discovery_disabled(&env.context));
        // Nothing else is affected
        assert!(is_analytics_disabled(&env.context));
    }
}
