// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::base::SettingType;
use serde::Deserialize;

/// [Interface] defines the FIDL interfaces supported by the settings service.
#[derive(Copy, Clone, Debug, Hash, PartialEq, Eq)]
pub enum Interface {
    Accessibility,
    Audio,
    Display(display::InterfaceFlags),
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

/// [InterfaceSpec] is the serializable type that defines the configuration for FIDL interfaces
/// supported by the settings service. It's read in from configuration files to modify what
/// interfaces the settings service provides.
#[derive(Clone, Deserialize, PartialEq, Eq, Hash, Debug)]
pub enum InterfaceSpec {
    Accessibility,
    Audio,
    // Should ideally be a HashSet, but HashSet does not impl Hash.
    Display(Vec<display::InterfaceSpec>),
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

impl From<InterfaceSpec> for Interface {
    fn from(spec: InterfaceSpec) -> Self {
        match spec {
            InterfaceSpec::Audio => Interface::Audio,
            InterfaceSpec::Accessibility => Interface::Accessibility,
            InterfaceSpec::Display(variants) => Interface::Display(variants.into()),
            InterfaceSpec::DoNotDisturb => Interface::DoNotDisturb,
            InterfaceSpec::FactoryReset => Interface::FactoryReset,
            InterfaceSpec::Input => Interface::Input,
            InterfaceSpec::Intl => Interface::Intl,
            InterfaceSpec::Keyboard => Interface::Keyboard,
            InterfaceSpec::Light => Interface::Light,
            InterfaceSpec::NightMode => Interface::NightMode,
            InterfaceSpec::Privacy => Interface::Privacy,
            InterfaceSpec::Setup => Interface::Setup,
        }
    }
}

impl From<Interface> for SettingType {
    fn from(spec: Interface) -> Self {
        match spec {
            Interface::Audio => SettingType::Audio,
            Interface::Accessibility => SettingType::Accessibility,
            Interface::Display(..) => SettingType::Display,
            Interface::DoNotDisturb => SettingType::DoNotDisturb,
            Interface::FactoryReset => SettingType::FactoryReset,
            Interface::Input => SettingType::Input,
            Interface::Intl => SettingType::Intl,
            Interface::Keyboard => SettingType::Keyboard,
            Interface::Light => SettingType::Light,
            Interface::NightMode => SettingType::NightMode,
            Interface::Privacy => SettingType::Privacy,
            Interface::Setup => SettingType::Setup,
        }
    }
}

pub mod display {
    use bitflags::bitflags;
    use serde::Deserialize;

    bitflags! {
        /// The Display interface covers a number of feature spaces, each handled by a different
        /// entity dependency. The flags below allow the scope of these features to be specified by
        /// the interface.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct InterfaceFlags: u64 {
            const BASE = 1 << 0;
        }
    }

    #[derive(Copy, Clone, Deserialize, PartialEq, Eq, Hash, Debug)]
    pub enum InterfaceSpec {
        Base,
    }

    impl From<Vec<InterfaceSpec>> for InterfaceFlags {
        fn from(variants: Vec<InterfaceSpec>) -> Self {
            variants.into_iter().fold(InterfaceFlags::empty(), |flags, variant| {
                flags
                    | match variant {
                        InterfaceSpec::Base => InterfaceFlags::BASE,
                    }
            })
        }
    }
}
