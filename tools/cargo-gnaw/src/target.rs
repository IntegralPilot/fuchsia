// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::gn::add_version_suffix;
use crate::types::*;
use camino::{Utf8Path, Utf8PathBuf};
use cargo_metadata::{Edition, Package, PackageId};
use semver::Version;
use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

pub struct GnTarget<'a> {
    /// Package ID from the Cargo metadata
    cargo_pkg_id: &'a PackageId,
    /// Version of the Package from Cargo
    version: &'a Version,
    /// Name of the target given in Cargo.toml
    pub target_name: &'a str,
    /// Name of the package given in Cargo.toml
    pub pkg_name: &'a str,
    /// Path to the root of the crate
    pub crate_root: &'a Utf8Path,
    /// Rust Edition of the target
    /// rustc: --edition
    pub edition: Edition,
    /// Type of crate
    /// rustc: --crate-type
    pub target_type: GnRustType,
    /// Rust features enabled on this target
    /// rustc: --cfg=feature=<string>
    pub features: &'a [Feature],
    /// Target depends on Cargo running a custom build-script
    pub has_build_script: bool,
    /// Target depends on Cargo running a custom build-script
    pub dependencies: HashMap<Option<Platform>, Vec<(&'a Package, String)>>,
    /// Target dev-dependencies from Cargo.toml
    pub dev_dependencies: HashMap<Option<Platform>, Vec<(&'a Package, String)>>,
}

impl std::fmt::Debug for GnTarget<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let display_name = if self.target_name != self.pkg_name {
            Cow::Owned(format!("{}.{}", self.pkg_name, self.target_name))
        } else {
            Cow::Borrowed(self.pkg_name)
        };
        if self.has_build_script {
            write!(f, "{:?} with custom-build: {}", self.target_type, display_name)
        } else {
            write!(f, "{:?}: {}", self.target_type, display_name)
        }
    }
}

impl PartialEq for GnTarget<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.cargo_pkg_id == other.cargo_pkg_id
            && self.target_name == other.target_name
            && self.target_type == other.target_type
    }
}
impl Eq for GnTarget<'_> {}

impl PartialOrd for GnTarget<'_> {
    fn partial_cmp(&self, other: &GnTarget<'_>) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for GnTarget<'_> {
    fn cmp(&self, other: &GnTarget<'_>) -> Ordering {
        self.target_name
            .cmp(other.target_name)
            .then(self.target_type.cmp(&other.target_type))
            .then(self.cargo_pkg_id.cmp(other.cargo_pkg_id))
    }
}

impl Hash for GnTarget<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.cargo_pkg_id.hash(state);
        self.target_name.hash(state);
        self.target_type.hash(state);
    }
}

impl<'a> GnTarget<'a> {
    pub fn new(
        cargo_pkg_id: &'a PackageId,
        target_name: &'a str,
        pkg_name: &'a str,
        edition: Edition,
        crate_root: &'a Utf8Path,
        version: &'a Version,
        target_type: GnRustType,
        features: &'a [Feature],
        has_build_script: bool,
        dependencies: HashMap<Option<Platform>, Vec<(&'a Package, String)>>,
        dev_dependencies: HashMap<Option<Platform>, Vec<(&'a Package, String)>>,
    ) -> Self {
        GnTarget {
            cargo_pkg_id,
            target_name,
            pkg_name,
            edition,
            crate_root,
            version,
            target_type,
            features,
            has_build_script,
            dependencies,
            dev_dependencies,
        }
    }

    /// Name of the target given in Cargo.toml
    pub fn name(&self) -> String {
        self.target_name.to_owned()
    }

    /// Version of the Package from Cargo
    pub fn version(&self) -> String {
        self.version.to_string()
    }

    pub fn metadata_hash(&self) -> String {
        let mut hasher = DefaultHasher::new();
        self.gn_target_name().hash(&mut hasher);
        format!("{:x}", hasher.finish())
    }

    /// with version
    pub fn gn_target_name(&self) -> String {
        let prefix = match self.target_type {
            GnRustType::Library | GnRustType::Rlib | GnRustType::ProcMacro | GnRustType::Test => {
                Cow::Borrowed(self.pkg_name)
            }
            GnRustType::Binary => Cow::Owned(format!("{}.{}", self.pkg_name, self.target_name)),
            ty => panic!("Don't know how to represent this type \"{:?}\" in GN", ty),
        };
        add_version_suffix(&prefix, &self.version)
    }

    pub fn package_root(&self) -> Utf8PathBuf {
        let mut package_root = self.crate_root.canonicalize_utf8().unwrap();

        while !package_root.join("Cargo.toml").exists() {
            package_root = package_root
                .parent()
                .expect("searching up from the crate root we must find a cargo.toml")
                .to_path_buf();
        }

        assert!(
            !package_root.ends_with("third_party/rust_crates"),
            "must find a cargo.toml before the root one"
        );

        package_root
    }

    pub fn gn_target_type(&self) -> String {
        match self.target_type {
            GnRustType::Binary => String::from("rustc_binary"),
            GnRustType::Library | GnRustType::Rlib => String::from("rustc_library"),
            GnRustType::ProcMacro => String::from("rustc_macro"),
            GnRustType::Test => String::from("rustc_test"),
            ty => panic!("Don't know how to represent this type \"{:?}\" in GN", ty),
        }
    }
}
