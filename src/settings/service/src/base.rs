// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Service-wide definitions.
//!
//! # Summary
//!
//! The base mod houses the core definitions for communicating information
//! across the service. Note that there are currently references to types in
//! other nested base mods. It is the long-term intention that the common
//! general (non-domain specific or overarching) definitions are migrated here,
//! while particular types, such as setting-specific definitions, are moved to
//! a common base mod underneath the parent setting mod.

use crate::ingress::fidl;
use serde::Serialize;
use std::collections::HashSet;

/// The setting types supported by the service.
#[derive(PartialEq, Debug, Eq, Hash, Clone, Copy, Serialize)]
pub enum SettingType {
    Accessibility,
    Audio,
    Display,
    DoNotDisturb,
    FactoryReset,
    Input,
    Intl,
    Keyboard,
    Light,
    NightMode,
    Privacy,
    Setup,
}

/// Returns the default interfaces supported by any product if none are supplied.
pub fn get_default_interfaces() -> HashSet<fidl::InterfaceSpec> {
    [
        fidl::InterfaceSpec::Accessibility,
        fidl::InterfaceSpec::Intl,
        fidl::InterfaceSpec::Privacy,
        fidl::InterfaceSpec::Setup,
    ]
    .into()
}
