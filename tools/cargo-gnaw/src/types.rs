// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::gn::add_version_suffix;
use anyhow::{Error, anyhow};
use cargo_metadata::{Package, TargetKind};
use std::convert::TryFrom;

pub type Feature = cargo_metadata::FeatureName;
pub type Platform = String;

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub enum GnRustType {
    ProcMacro,
    Library,
    Rlib,
    Staticlib,
    Dylib,
    Cdylib,
    Binary,
    Example,
    Test,
    Bench,
    BuildScript,
}

impl<'a> TryFrom<&'a TargetKind> for GnRustType {
    type Error = Error;

    fn try_from(value: &TargetKind) -> Result<Self, Self::Error> {
        match value {
            TargetKind::Bin => Ok(GnRustType::Binary),
            TargetKind::Lib => Ok(GnRustType::Library),
            TargetKind::RLib => Ok(GnRustType::Rlib),
            TargetKind::StaticLib => Ok(GnRustType::Staticlib),
            TargetKind::DyLib => Ok(GnRustType::Dylib),
            TargetKind::CDyLib => Ok(GnRustType::Cdylib),
            TargetKind::ProcMacro => Ok(GnRustType::ProcMacro),
            TargetKind::Test => Ok(GnRustType::Test),
            TargetKind::Example => Ok(GnRustType::Example),
            TargetKind::Bench => Ok(GnRustType::Bench),
            TargetKind::CustomBuild => Ok(GnRustType::BuildScript),
            value => Err(anyhow!("unknown crate type: {value}")),
        }
    }
}

pub trait GnData {
    fn gn_name(&self) -> String;
    fn is_proc_macro(&self) -> bool;
}

impl GnData for Package {
    fn gn_name(&self) -> String {
        add_version_suffix(&self.name, &self.version)
    }

    fn is_proc_macro(&self) -> bool {
        for target in &self.targets {
            for kind in &target.kind {
                if kind == &TargetKind::ProcMacro {
                    return true;
                }
            }
        }
        false
    }
}
