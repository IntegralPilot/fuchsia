// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::build::BuildScript;
use crate::graph::GnBuildGraph;
use crate::target::GnTarget;
use crate::types::*;
use anyhow::{Context, Result};
use argh::FromArgs;
use camino::Utf8PathBuf;
use cargo_metadata::{CargoOpt, DependencyKind, Package, PackageName};
use serde_derive::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::Command;

mod build;
mod cfg;
mod gn;
mod graph;
mod target;
mod types;

#[derive(FromArgs, Debug)]
/// Generate a GN manifest for your vendored Cargo dependencies.
pub struct Opt {
    /// cargo manifest path
    #[argh(option)]
    manifest_path: PathBuf,

    /// root of GN project
    #[argh(option)]
    project_root: PathBuf,

    /// cargo binary to use (for vendored toolchains)
    #[argh(option)]
    cargo: Option<PathBuf>,

    /// already generated configs from cargo build scripts
    #[argh(option, short = 'p')]
    // TODO(https://fxbug.dev/42165549)
    #[allow(unused)]
    cargo_configs: Option<PathBuf>,

    /// location of GN file
    #[argh(option, short = 'o')]
    output: PathBuf,

    /// location of JSON file with crate metadata
    #[argh(option)]
    emit_metadata: Option<PathBuf>,

    /// location of GN binary to use for formating.
    /// If no path is provided, no format will be run.
    #[argh(option)]
    gn_bin: Option<PathBuf>,

    /// don't generate a target for the root crate
    #[argh(switch)]
    skip_root: bool,

    /// run cargo with `--all-features`
    #[argh(switch)]
    all_features: bool,

    /// run cargo with `--no-default-features`
    #[argh(switch)]
    no_default_features: bool,

    /// run cargo with `--features <FEATURES>`
    #[argh(option)]
    features: Vec<String>,

    /// whether to emit Fuchsia SDK metadata atoms
    #[argh(switch)]
    output_fuchsia_sdk_metadata: bool,

    /// minimum age (in days) of published packages to resolve
    #[argh(option)]
    min_publish_age: Option<u32>,
}

type TargetName = String;
type Version = String;

/// Per-target metadata in the Cargo.toml file for Rust crates that
/// require extra information to go in the BUILD.gn file.
#[derive(Default, Clone, Serialize, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct TargetCfg {
    /// Config flags for rustc. Ex: --cfg=std
    rustflags: Option<Vec<String>>,
    /// Config flags to remove for rustc.
    remove_rustflags: Option<Vec<String>>,
    /// Feature flags for rustc. Ex: ["alloc", "std"]
    features: Option<Vec<String>>,
    /// Feature flags to remove for rustc. Ex: ["std"]
    remove_features: Option<Vec<String>>,
    /// Environment variables. These are usually from Cargo or the
    /// build.rs file in the crate.
    env_vars: Option<Vec<String>>,
    /// GN Targets that this crate should depend on. Generally for
    /// crates that build C libraries and link against.
    deps: Option<Vec<String>>,
    /// GN Configs that this crate should depend on.  Used to add
    /// crate-specific configs.
    configs: Option<Vec<String>>,
    /// GN Configs that this crate should exclude.  Used to remove
    /// crate-specific configs.
    remove_configs: Option<Vec<String>>,
    /// GN Visibility that controls which targets can depend on this target.
    visibility: Option<Vec<String>>,

    /// Controls whether rustdoc will be build for this crate.
    disable_rustdoc: Option<bool>,

    // Whether the package uses the Fuchsia license.
    uses_fuchsia_license: Option<bool>,

    // Path to explicitly defined license(s).
    license_files: Option<Vec<String>>,

    // Existing GN target to use in the output BUILD.gn instead of auto-generating one.
    existing_gn_target: Option<String>,
}

/// Configuration for a single GN executable target to generate from a Cargo binary target.
#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct BinaryCfg {
    /// Name to use as both the top-level GN group target and the executable's output name.
    output_name: String,
    /// Binary target configuration for all platforms.
    #[serde(default, flatten)]
    default_cfg: TargetCfg,
    /// Per-platform binary target configuration.
    #[serde(default)]
    #[serde(rename = "platform")]
    platform_cfg: HashMap<Platform, TargetCfg>,
}

/// Visibility list to use for a forwarding group
#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct GroupVisibility {
    /// .gni file to import which defines the variable
    import: String,
    /// Name of variable defined by the gni file containing the visibility list to use
    variable: String,
}

/// Configuration for a structured forwarding group target.
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct GroupCfg {
    /// The name of the GN group target to generate.
    pub name: String,
    /// Optional visibility configuration for that specific group target.
    #[serde(default)]
    pub visibility: Option<Vec<String>>,
}

/// Defines a per-target rule for rule renaming.
///
/// Some external crates require additional post-processing. For those we define custom rules,
/// and force a rename of the rule and the group so that post-processing can be done by GN.
#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct RuleRenaming {
    /// The label of the `.gni` file used to make the group and target renaming
    /// available.
    ///
    /// Only a single file can be imported. This is on purpose so as
    /// not to pollute the generated file with many imports.  The same file will
    /// be imported only once.
    import: String,
    /// If set, this name will be used instead of `group` for the top level group.
    group_name: Option<String>,
    /// If set, this rule name will be used instead of the name suggested by
    /// the target type (e.g. `my_rule` instead of `rust_library`).
    rule_name: Option<String>,
}

/// Configuration for a Cargo package. Contains configuration for its (single) library target at the
/// top level and optionally zero or more binaries to generate.
#[derive(Default, Clone, Serialize, Deserialize, Debug)]
#[serde(default, deny_unknown_fields)]
pub struct PackageCfg {
    /// Library target configuration for all platforms.
    #[serde(flatten)]
    default_cfg: TargetCfg,
    /// Per-platform library target configuration.
    #[serde(rename = "platform")]
    platform_cfg: HashMap<Platform, TargetCfg>,
    /// Configuration for GN binary targets to generate from one of the package's binary targets.
    /// The map key identifies the cargo target name within this cargo package.
    binary: HashMap<TargetName, BinaryCfg>,
    /// List of cargo features which have been code reviewed for this cargo package
    ///
    /// Must be set if require_feature_reviews mentions this package.
    reviewed_features: Option<Vec<String>>,
    /// Overrides the name of the forwarding group, for use when multiple versions are enabled at
    /// the same time.
    group_name: Option<String>,
    /// Visibility list to use for the forwarding group, for use with fixits which seek to remove
    /// the use of a specific crate from the tree.
    group_visibility: Option<GroupVisibility>,
    /// Structured list of forwarding group targets to generate for this package version.
    groups: Option<Vec<GroupCfg>>,
    /// Whether or not this target my only be used in tests.
    testonly: Option<bool>,
    /// Build tests for this target.
    tests: bool,
    /// Used to rename the group and target names used to bring this particular package
    /// in.  Normally, `cargo-gnaw` will use `group` and `rust_*` target name, but this
    /// allows us to define custom replacement targets. An import is required, but the
    /// feature is deliberately limited to just a single definition and a limited number
    /// of renames, as it should be used in very special cases only.
    target_renaming: Option<RuleRenaming>,
    /// Explicitly allow a recently published crate version before cooldown expires
    allow_recent_publish: Option<bool>,
    /// Optional package-specific min-publish-age override in days (e.g. 0 or 3)
    min_publish_age: Option<u32>,
}

impl PackageCfg {
    pub fn validate(&self) -> Result<()> {
        if self.groups.is_some() && (self.group_name.is_some() || self.group_visibility.is_some()) {
            anyhow::bail!(
                "Cannot specify both `groups` and legacy `group_name` or `group_visibility`"
            );
        }
        Ok(())
    }
}

/// Configs added to all GN targets in the BUILD.gn
#[derive(Default, Serialize, Deserialize, Debug)]
#[serde(default, deny_unknown_fields)]
pub struct GlobalTargetCfgs {
    remove_cfgs: Vec<String>,
    add_cfgs: Vec<String>,
    /// When true, crates must have license files.
    require_licenses: Option<bool>,
    /// Minimum publication age (in days) for third-party crates (e.g. 14)
    min_publish_age: Option<u32>,
}

/// Extra metadata in the Cargo.toml file that feeds into the
/// BUILD.gn file.
#[derive(Serialize, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
struct GnBuildMetadata {
    /// global configs
    config: Option<GlobalTargetCfgs>,
    /// packages for which only some features will be code reviewed
    #[serde(default)]
    require_feature_reviews: HashSet<PackageName>,
    /// map of per-Cargo package configuration
    package: HashMap<PackageName, HashMap<Version, PackageCfg>>,
}

impl GnBuildMetadata {
    fn find_package<'a>(&'a self, pkg: &Package) -> Option<&'a PackageCfg> {
        self.package.get(&pkg.name).and_then(|p| p.get(&pkg.version.to_string()))
    }
}

#[derive(Serialize, Deserialize, Debug)]
struct BuildMetadata {
    gn: Option<GnBuildMetadata>,
}

/// Used for identifying 3p owners via reverse dependencies. Ties together several pieces of
/// metadata needed to associate a GN target with an OWNERS file and vice versa.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, PartialOrd, Ord, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CrateOutputMetadata {
    /// Name of the crate as specified in Cargo.toml.
    pub name: String,

    /// Version of the crate, used for disambiguating between duplicated transitive deps.
    pub version: String,

    /// Full GN target for depending on the crate.
    ///
    /// For example, Rust targets all have a canonical target like
    /// `//third_party/rust_crates:foo-v1_0_0`.
    pub canonical_target: String,

    /// Shorthand GN target for depending on the crate.
    ///
    /// For example, Rust targets listed in `third_party/rust_crates/Cargo.toml` have a
    /// shortcut target like `//third_party/rust_crates:foo`.
    pub shortcut_target: Option<String>,

    /// Filesystem path to the directory containing `Cargo.toml`.
    pub path: Utf8PathBuf,
}

// Use BTreeMap so that iteration over platforms is stable.
type CombinedTargetCfg<'a> = BTreeMap<Option<&'a Platform>, &'a TargetCfg>;

/// Render options for binary.
pub struct BinaryRenderOptions<'a> {
    /// Name of the binary.
    binary_name: &'a str,
    /// If true, this binary target is a test target.
    tests_enabled: bool,
}

macro_rules! define_combined_cfg {
    ($t:ty) => {
        impl $t {
            fn combined_target_cfg(&self) -> CombinedTargetCfg<'_> {
                let mut combined: CombinedTargetCfg<'_> =
                    self.platform_cfg.iter().map(|(k, v)| (Some(k), v)).collect();
                assert!(
                    combined.insert(None, &self.default_cfg).is_none(),
                    "Default platform (None) already present in combined cfg"
                );
                combined
            }
        }
    };
}
define_combined_cfg!(PackageCfg);
define_combined_cfg!(BinaryCfg);

/// Writes out an import to the provided `output`.
///
/// The import is written out only the first time it appears. `imported_files`
/// is updated so that the next appearance of the same import does not result
/// in a repeated import.
fn write_import_once<W: io::Write>(
    mut output: &mut W,
    imported_files: &mut HashSet<String>,
    import: &String,
) -> Result<()> {
    if !imported_files.contains(import) {
        gn::write_import(&mut output, import).with_context(|| "writing import")?;
        let _ = imported_files.insert(import.clone());
    }
    Ok(())
}

pub fn generate_from_manifest<W: io::Write>(mut output: &mut W, opt: &Opt) -> Result<()> {
    let manifest_path = &opt.manifest_path;
    let path_from_root_to_generated = opt
        .output
        .parent()
        .unwrap()
        .strip_prefix(&opt.project_root)
        .expect("--project-root must be a parent of --output");
    let mut emitted_metadata: Vec<CrateOutputMetadata> = Vec::new();
    let mut top_level_metadata = HashSet::new();
    let mut imported_files: HashSet<String> = HashSet::new();

    // read out custom gn commands from the toml file
    let mut file = File::open(&manifest_path)
        .with_context(|| format!("opening {}", manifest_path.display()))?;
    let mut contents = String::new();
    let _ = file
        .read_to_string(&mut contents)
        .with_context(|| format!("while reading manifest: {}", manifest_path.display()))?;
    let metadata_configs: BuildMetadata =
        toml::from_str(&contents).context("parsing manifest toml")?;

    if let Some(gn) = &metadata_configs.gn {
        for (pkg_name, versions) in &gn.package {
            for (pkg_version, pkg_cfg) in versions {
                pkg_cfg.validate().with_context(|| {
                    format!("validating package config for {} version {}", pkg_name, pkg_version)
                })?;
            }
        }
    }

    // generate cargo metadata
    let mut cmd = cargo_metadata::MetadataCommand::new();
    let parent_dir = manifest_path
        .parent()
        .with_context(|| format!("while parsing parent path: {}", manifest_path.display()))?;
    let _ = cmd.current_dir(parent_dir);
    let _ = cmd.manifest_path(manifest_path);
    if let Some(ref cargo_path) = opt.cargo {
        let _ = cmd.cargo_path(cargo_path);
    }
    if opt.all_features {
        let _ = cmd.features(CargoOpt::AllFeatures);
    }
    if opt.no_default_features {
        let _ = cmd.features(CargoOpt::NoDefaultFeatures);
    }
    if !opt.features.is_empty() {
        let _ = cmd.features(CargoOpt::SomeFeatures(opt.features.clone()));
    }

    let global_min_publish_age = opt.min_publish_age.or_else(|| {
        metadata_configs
            .gn
            .as_ref()
            .and_then(|gn| gn.config.as_ref())
            .and_then(|cfg| cfg.min_publish_age)
    });

    if let Some(min_age_days) = global_min_publish_age
        && min_age_days > 0
    {
        let cargo_bin = opt.cargo.as_deref().unwrap_or_else(|| std::path::Path::new("cargo"));
        let mut check_cmd = std::process::Command::new(cargo_bin);
        check_cmd
            .arg("update")
            .arg("--dry-run")
            .arg("--manifest-path")
            .arg(manifest_path)
            .arg("-Zmin-publish-age")
            .arg(format!("--config=registry.global-min-publish-age=\"{} days\"", min_age_days));
        let output = check_cmd
            .output()
            .with_context(|| "running cargo update --dry-run for min-publish-age verification")?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Minimum publish age policy violation:\n{}", stderr.trim());
        }
    }

    let other_options = vec![String::from("--frozen")];
    let _ = cmd.other_options(other_options);

    let metadata = cmd.exec().with_context(|| {
        format!("while running cargo metadata: supplied cargo binary: {:?}", opt.cargo)
    })?;

    gn::write_header(&mut output, manifest_path).context("writing header")?;

    if opt.output_fuchsia_sdk_metadata {
        gn::write_fuchsia_sdk_metadata_header(&mut output)
            .context("writing Fuchsia SDK metadata header")?;
    }

    // Construct a build graph of all the targets for GN
    let mut build_graph = GnBuildGraph::new(&metadata);
    match metadata.resolve.as_ref() {
        Some(resolve) => {
            let top_level_id =
                resolve.root.as_ref().expect("the Cargo.toml file must define a package");
            if opt.skip_root {
                let top_level_node = resolve
                    .nodes
                    .iter()
                    .find(|node| node.id == *top_level_id)
                    .expect("top level node not in node graph");
                for dep in &top_level_node.deps {
                    build_graph
                        .add_cargo_package(dep.pkg.clone())
                        .context("adding cargo package")?;
                    let mut platforms = BTreeSet::new();
                    let mut has_unconditional = false;
                    for kinds in dep.dep_kinds.iter() {
                        if kinds.kind == DependencyKind::Normal {
                            if let Some(target) = &kinds.target {
                                platforms.insert(Some(format!("{}", target)));
                            } else {
                                has_unconditional = true;
                            }
                        }
                    }

                    let platforms_to_emit: Vec<Option<String>> = if has_unconditional {
                        vec![None]
                    } else {
                        platforms.into_iter().collect()
                    };

                    for platform in platforms_to_emit {
                        let package = &metadata[&dep.pkg];
                        let _ = top_level_metadata.insert(package.name.to_owned());
                        let cfg =
                            metadata_configs.gn.as_ref().and_then(|cfg| cfg.find_package(package));

                        let group_name = cfg.and_then(|cfg| cfg.group_name.as_deref());
                        let visibility = cfg.and_then(|cfg| cfg.group_visibility.as_ref());
                        let groups = cfg.and_then(|cfg| cfg.groups.as_deref());
                        if let Some(visibility) = visibility {
                            write_import_once(
                                &mut output,
                                &mut imported_files,
                                &visibility.import,
                            )?;
                        }

                        let target_renaming = cfg.and_then(|cfg| cfg.target_renaming.as_ref());
                        if let Some(target_renaming) = target_renaming {
                            write_import_once(
                                &mut output,
                                &mut imported_files,
                                &target_renaming.import,
                            )?;
                        }

                        gn::write_top_level_rule(
                            &mut output,
                            platform.as_deref(),
                            package,
                            group_name,
                            visibility,
                            groups,
                            target_renaming,
                            cfg.and_then(|cfg| cfg.testonly).unwrap_or(false),
                            cfg.map(|c| c.tests).unwrap_or(false),
                        )
                        .with_context(|| {
                            format!("while writing top level rule for package: {}", dep.pkg)
                        })
                        .context("writing top level rule")?;

                        // Write an alias for easier compatibility with Bazel naming.
                        gn::write_alias_rule(
                            &mut output,
                            platform.as_deref(),
                            package,
                            visibility,
                            groups,
                            cfg.and_then(|cfg| cfg.testonly).unwrap_or(false),
                        )
                        .with_context(|| {
                            format!("while writing alias rule for package: {}", dep.pkg)
                        })
                        .context("writing alias rule")?;

                        if opt.output_fuchsia_sdk_metadata {
                            gn::write_fuchsia_sdk_metadata(
                                &mut output,
                                platform.as_deref(),
                                package,
                            )
                            .with_context(|| {
                                format!("while writing top level rule for package: {}", dep.pkg)
                            })
                            .context("writing Fuchsia SDK metadata for top level rule")?;
                        }
                    }
                }
            } else {
                build_graph
                    .add_cargo_package(top_level_id.clone())
                    .with_context(|| "could not add cargo package")?;
                let package = &metadata[&top_level_id];
                let _ = top_level_metadata.insert(package.name.to_owned());
                let cfg = metadata_configs.gn.as_ref().and_then(|cfg| cfg.find_package(package));

                let group_name = cfg.and_then(|cfg| cfg.group_name.as_deref());
                let visibility = cfg.and_then(|cfg| cfg.group_visibility.as_ref());
                let groups = cfg.and_then(|cfg| cfg.groups.as_deref());
                if let Some(visibility) = visibility {
                    write_import_once(&mut output, &mut imported_files, &visibility.import)?;
                }

                let target_renaming = cfg.and_then(|cfg| cfg.target_renaming.as_ref());
                if let Some(target_renaming) = target_renaming {
                    write_import_once(&mut output, &mut imported_files, &target_renaming.import)?;
                }

                gn::write_top_level_rule(
                    &mut output,
                    None,
                    package,
                    group_name,
                    visibility,
                    groups,
                    target_renaming,
                    cfg.and_then(|cfg| cfg.testonly).unwrap_or(false),
                    cfg.map(|c| c.tests).unwrap_or(false),
                )
                .with_context(|| "writing top level rule")?;

                if opt.output_fuchsia_sdk_metadata {
                    gn::write_fuchsia_sdk_metadata(&mut output, None, package)
                        .with_context(|| "writing Fuchsia SDK metadata for top level rule")?;
                }
            }
        }
        None => anyhow::bail!("Failed to resolve a build graph for the package tree"),
    }

    // Sort targets for stable output to minimize diff churn
    let mut graph_targets: Vec<&GnTarget<'_>> = build_graph.targets().collect();
    graph_targets.sort();

    let global_config = match metadata_configs.gn {
        Some(ref gn_configs) => gn_configs.config.as_ref(),
        None => None,
    };

    let empty_hash_set = &HashSet::new();
    let require_feature_reviews = metadata_configs
        .gn
        .as_ref()
        .map(|gn| &gn.require_feature_reviews)
        .unwrap_or(empty_hash_set);

    // Grab the per-package configs.
    let gn_pkg_cfgs = metadata_configs.gn.as_ref().map(|i| &i.package);

    // Iterate through the target configs, verifying that the build graph contains the configured
    // targets, then save off a mapping of GnTarget to the target config.
    let mut target_cfgs = HashMap::<&GnTarget<'_>, CombinedTargetCfg<'_>>::new();
    let mut target_binaries = BTreeMap::<&GnTarget<'_>, BinaryRenderOptions<'_>>::new();
    let mut testonly_targets = HashSet::<&GnTarget<'_>>::new();
    let mut targets_with_tests = HashSet::<&GnTarget<'_>>::new();
    let mut reviewed_features_map = HashMap::<&GnTarget<'_>, Option<&[String]>>::new();
    // An entry for each target that needs a renamed rule.  There is no connection between
    // PackageCfg and GnTarget, so to use the setting from PackageCfg, we need to build this map as
    // we iterate through targets.
    let mut renamed_rules = HashMap::<&GnTarget<'_>, &'_ str>::new();
    let mut unused_configs = String::new();
    let mut existing_targets = HashMap::<&GnTarget<'_>, String>::new();

    if let Some(gn_pkg_cfgs) = gn_pkg_cfgs {
        for (pkg_name, versions) in gn_pkg_cfgs {
            for (pkg_version, pkg_cfg) in versions {
                // Search the build graph for the library target.
                if let Some(target) = build_graph.find_library_target(pkg_name, pkg_version) {
                    assert!(
                        target_cfgs.insert(target, pkg_cfg.combined_target_cfg()).is_none(),
                        "Duplicate library config somehow specified"
                    );
                    assert!(
                        reviewed_features_map
                            .insert(target, pkg_cfg.reviewed_features.as_deref())
                            .is_none(),
                        "Duplicate library config somehow specified"
                    );

                    if let Some(renamed_rule) = pkg_cfg
                        .target_renaming
                        .as_ref()
                        .and_then(|r| r.rule_name.as_ref())
                        .map(|x| x.as_str())
                    {
                        let _ = renamed_rules.insert(target, renamed_rule);
                    }

                    if pkg_cfg.testonly == Some(true) {
                        let _ = testonly_targets.insert(target);
                    }

                    if pkg_cfg.tests {
                        let _ = targets_with_tests.insert(target);
                    }

                    // If there is an existing GN target, there shouldn't be any other fields set.
                    if pkg_cfg.default_cfg.existing_gn_target.is_some() {
                        assert!(
                            pkg_cfg.default_cfg.rustflags.is_none()
                                && pkg_cfg.default_cfg.env_vars.is_none()
                                && pkg_cfg.default_cfg.deps.is_none()
                                && pkg_cfg.default_cfg.configs.is_none()
                                && pkg_cfg.default_cfg.visibility.is_none()
                                && pkg_cfg.default_cfg.uses_fuchsia_license.is_none()
                                && pkg_cfg.platform_cfg.is_empty()
                                && pkg_cfg.binary.is_empty()
                                && pkg_cfg.reviewed_features.is_none()
                                && pkg_cfg.group_visibility.is_none()
                                && pkg_cfg.groups.is_none()
                                && pkg_cfg.testonly.is_none()
                                && pkg_cfg.target_renaming.is_none()
                                && pkg_cfg.allow_recent_publish.is_none()
                                && pkg_cfg.min_publish_age.is_none(),
                            "No other field can be set, including platform sub-configs, if an existing GN target is specified"
                        );
                        assert!(
                            existing_targets
                                .insert(
                                    target,
                                    pkg_cfg.default_cfg.existing_gn_target.clone().unwrap()
                                )
                                .is_none(),
                            "Duplicate target"
                        );
                    }
                } else {
                    unused_configs.push_str(&format!(
                        "library crate, package {} version {}\n",
                        pkg_name, pkg_version
                    ));
                }

                // Handle binaries that should be built for this package, similarly searching the
                // build graph for the binary targets.
                for (bin_cargo_target, bin_cfg) in &pkg_cfg.binary {
                    if let Some(target) =
                        build_graph.find_binary_target(pkg_name, pkg_version, bin_cargo_target)
                    {
                        if let Some(old_options) = target_binaries.insert(
                            target,
                            BinaryRenderOptions {
                                binary_name: &bin_cfg.output_name,
                                tests_enabled: pkg_cfg.tests,
                            },
                        ) {
                            anyhow::bail!(
                                "A given binary target ({} in package {} version {}) can only be \
                                used for a single GN target, but multiple exist, including {} \
                                and {}",
                                bin_cargo_target,
                                pkg_name,
                                pkg_version,
                                bin_cfg.output_name,
                                old_options.binary_name,
                            );
                        }
                        assert!(
                            target_cfgs.insert(target, bin_cfg.combined_target_cfg()).is_none(),
                            "Should have bailed above"
                        );

                        if pkg_cfg.tests {
                            let _ = targets_with_tests.insert(target);
                        }
                    } else {
                        unused_configs.push_str(&format!(
                            "binary crate {}, package {} version {}\n",
                            bin_cargo_target, pkg_name, pkg_version
                        ));
                    }
                }
            }
        }
    }
    if !unused_configs.is_empty() {
        anyhow::bail!(
            "GNaw config exists for crates that were not found in the Cargo build graph:\n\n{}",
            unused_configs
        );
    }

    // Write the top-level GN rules for binaries. Verify that the names are unique, otherwise a
    // build failure will result.
    {
        let mut names = HashSet::new();
        for (target, options) in &target_binaries {
            let binary_group_name = format!("{}.{}", target.pkg_name, options.binary_name);
            if !names.insert(binary_group_name.clone()) {
                anyhow::bail!(
                    "Multiple targets are configured to generate executables named \"{}\"",
                    binary_group_name
                );
            }

            emitted_metadata.push(CrateOutputMetadata {
                name: options.binary_name.to_string(),
                version: target.version(),
                canonical_target: format!(
                    "//{}:{}",
                    path_from_root_to_generated.display(),
                    target.gn_target_name()
                ),
                shortcut_target: Some(format!(
                    "//{}:{}",
                    path_from_root_to_generated.display(),
                    binary_group_name
                )),
                path: target.package_root(),
            });
            gn::write_binary_top_level_rule(&mut output, None, target, options)
                .context(format!("writing binary top level rule: {}", target.name()))?;
        }
    }

    // Write out a GN rule for each target in the build graph
    for target in graph_targets {
        // Check whether we should generate a target if this is a binary.
        let binary_name = if let GnRustType::Binary = target.target_type {
            let name = target_binaries.get(target).map(|opt| opt.binary_name);
            if name.is_none() {
                continue;
            }
            name
        } else {
            None
        };

        let target_cfg = target_cfgs.get(target);
        if target.has_build_script && target_cfg.is_none() {
            let build_output = BuildScript::execute(target);
            match build_output {
                Ok(rules) => {
                    anyhow::bail!(
                        "Add this to your Cargo.toml located at {}:\n\
                        [gn.package.{}.\"{}\"]\n\
                        rustflags = [{}]\n\
                        rustenv = [{}]",
                        manifest_path.display(),
                        target.name(),
                        target.version(),
                        rules.rustflags.join(", "),
                        rules.rustenv.join(", ")
                    );
                }
                Err(err) => anyhow::bail!(
                    "{} {} uses a build script but no section defined in the GN section \
                    nor can we automatically generate the appropriate rules:\n{}",
                    target.name(),
                    target.version(),
                    err,
                ),
            }
        }

        // Check to see if we need to review individual features for this crate
        let reviewed_features = reviewed_features_map.get(target);
        if require_feature_reviews.contains(target.name().as_str()) {
            match reviewed_features {
                Some(Some(_)) => {}
                _ => {
                    let mut features_map = std::collections::BTreeMap::new();
                    let _ = features_map.insert("features", &target.features);

                    anyhow::bail!(
                        "{name} {version} requires feature review but reviewed features not found.\n\n\
                        Make sure to conduct code review assuming the following features are enabled, \
                        and then add this to your Cargo.toml located at {manifest_path}:\n\
                        [gn.package.{name}.\"{version}\"]\n\
                        reviewed_features = {features}",
                        manifest_path = manifest_path.display(),
                        name = target.name(),
                        version = target.version(),
                        features = toml::to_string(&features_map).unwrap()
                    );
                }
            }
        } else if let Some(Some(_)) = reviewed_features {
            anyhow::bail!(
                "{name} {version} sets reviewed_features but {name} was not found in \
                    require_feature_reviews.\n\n\
                    Make sure to add it there so that reviewed_features is not accidentally \
                    removed during future crate version bumps.",
                name = target.name(),
                version = target.version(),
            );
        }

        if let Some(&Some(reviewed_features)) = reviewed_features {
            let reviewed_features_set =
                reviewed_features.iter().map(|s| s.as_str()).collect::<HashSet<_>>();
            let unreviewed_features = target
                .features
                .iter()
                .filter(|f| !reviewed_features_set.contains(f.as_str()))
                .map(|feature| feature.to_string())
                .collect::<Vec<_>>();
            if !unreviewed_features.is_empty() {
                anyhow::bail!(
                    "{name} {version} is included with unreviewed features {unreviewed:?}\n\n\
                    Make sure to additionally review code gated by these features, then add them \
                    to reviewed_features under [gn.package.{name}.\"{version}\"] in {manifest_path}",
                    name = target.name(),
                    version = target.version(),
                    unreviewed = unreviewed_features,
                    manifest_path = manifest_path.display(),
                )
            }
        }

        let package_root = target.package_root();
        let _ = package_root.strip_prefix(&opt.project_root).unwrap_or_else(|e| {
            panic!(
                "{}: {} is not under the project_root ({})",
                e,
                target.package_root(),
                opt.project_root.display()
            )
        });

        let shortcut_target = if top_level_metadata.contains(target.pkg_name) {
            Some(format!("//{}:{}", path_from_root_to_generated.display(), target.pkg_name))
        } else {
            None
        };

        emitted_metadata.push(CrateOutputMetadata {
            name: target.name(),
            version: target.version(),
            canonical_target: format!(
                "//{}:{}",
                path_from_root_to_generated.display(),
                target.gn_target_name()
            ),
            shortcut_target,
            path: package_root,
        });

        // If there is an existing GN target, we want a group and not a new GN target.
        if let Some(existing_target) = existing_targets.get(target) {
            writeln!(
                output,
                include_str!("../templates/gn_group.template"),
                group_name = target.gn_target_name(),
                dep_name = existing_target,
            )?;
        } else {
            gn::write_rule(
                &mut output,
                target,
                &opt.project_root,
                global_config,
                target_cfg,
                binary_name,
                testonly_targets.contains(target),
                false,
                renamed_rules.get(target).copied(),
                true,
            )
            .with_context(|| format!("writing rule for: {} {}", target.name(), target.version()))?;
        }

        if targets_with_tests.contains(target) {
            gn::write_rule(
                &mut output,
                target,
                &opt.project_root,
                global_config,
                target_cfg,
                binary_name,
                true,
                true,
                renamed_rules.get(&target).copied(),
                true,
            )
            .with_context(|| format!("writing rule for: {} {}", target.name(), target.version()))?;
        }
    }

    if let Some(metadata_path) = &opt.emit_metadata {
        eprintln!("Emitting external crate metadata: {}", metadata_path.display());
        emitted_metadata.sort();
        let metadata_json = serde_json::to_string_pretty(&emitted_metadata)
            .context("serializing metadata to json")?;
        std::fs::create_dir_all(
            metadata_path
                .parent()
                .expect("The metadata path must include a valid parent directory"),
        )?;
        std::fs::write(metadata_path, &metadata_json).context("writing metadata file")?;
    }

    Ok(())
}

pub fn run(args: &[impl AsRef<str>]) -> Result<()> {
    // Check if running through cargo or stand-alone before arg parsing
    let mut strs: Vec<&str> = args.iter().map(|s| s.as_ref()).collect();
    if strs.get(1) == Some(&"gnaw") {
        // If the second command is "gnaw" this likely invoked by `cargo gnaw`
        // shift all args by one.
        strs = strs[1..].to_vec()
    }
    let opt = match Opt::from_args(&[strs[0]], &strs[1..]) {
        Ok(opt) => opt,
        Err(e) if e.status.is_ok() => {
            println!("{}", e.output);
            return Ok(());
        }
        Err(e) => return Err(anyhow::Error::msg(e.output)),
    };

    eprintln!("Generating GN file from {}", opt.manifest_path.to_string_lossy());

    // redirect to stdout if no GN output file specified
    // Stores data in a buffer in-case to prevent creating bad BUILD.gn
    let mut gn_output_buffer = vec![];
    generate_from_manifest(&mut gn_output_buffer, &opt).context("generating manifest")?;

    // Write the file buffer to an actual file
    File::create(&opt.output)
        .context("creating output file")?
        .write_all(&gn_output_buffer)
        .context("writing output contents")?;

    if let Some(gn_bin) = opt.gn_bin {
        eprintln!("Formatting output file: {}", opt.output.display());
        let status = Command::new(&gn_bin)
            .arg("format")
            .arg(opt.output)
            .status()
            .with_context(|| format!("could not spawn GN: {}", gn_bin.display()))?;
        if !status.success() {
            anyhow::bail!("GN format command failed:{:?}", status);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_package_cfg_with_groups() {
        let toml_str = r#"
            groups = [
                { name = "foo-v1_2", visibility = [ "//vendor/google/*" ] },
                { name = "foo-legacy" }
            ]
        "#;

        let cfg: PackageCfg = toml::from_str(toml_str).expect("deserialize PackageCfg");
        let groups = cfg.groups.expect("groups should be Some");
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].name, "foo-v1_2");
        assert_eq!(groups[0].visibility, Some(vec!["//vendor/google/*".to_string()]));
        assert_eq!(groups[1].name, "foo-legacy");
        assert_eq!(groups[1].visibility, None);
    }

    #[test]
    fn deserialize_package_cfg_without_groups() {
        let toml_str = r#"
            group_name = "foo-legacy"
        "#;

        let cfg: PackageCfg = toml::from_str(toml_str).expect("deserialize PackageCfg");
        assert_eq!(cfg.group_name.as_deref(), Some("foo-legacy"));
        assert!(cfg.groups.is_none());
    }

    #[test]
    fn package_cfg_with_group_name_and_groups_errors() {
        let toml_str = r#"
            group_name = "foo-legacy"
            groups = [
                { name = "foo-v1_2" }
            ]
        "#;

        let cfg: PackageCfg = toml::from_str(toml_str).expect("deserialize PackageCfg");
        let err = cfg.validate().expect_err("expected validation error");
        assert!(err.to_string().contains(
            "Cannot specify both `groups` and legacy `group_name` or `group_visibility`"
        ));
    }

    #[test]
    fn package_cfg_with_group_visibility_and_groups_errors() {
        let toml_str = r#"
            groups = [
                { name = "foo-v1_2" }
            ]
            [group_visibility]
            import = "//build/foo.gni"
            variable = "foo_visibility"
        "#;

        let cfg: PackageCfg = toml::from_str(toml_str).expect("deserialize PackageCfg");
        let err = cfg.validate().expect_err("expected validation error");
        assert!(err.to_string().contains(
            "Cannot specify both `groups` and legacy `group_name` or `group_visibility`"
        ));
    }

    #[test]
    fn package_cfg_validate_ok() {
        let toml_groups = r#"
            groups = [
                { name = "foo-v1_2" }
            ]
        "#;
        let cfg: PackageCfg = toml::from_str(toml_groups).expect("deserialize PackageCfg");
        assert!(cfg.validate().is_ok());

        let toml_legacy = r#"
            group_name = "foo-legacy"
            [group_visibility]
            import = "//build/foo.gni"
            variable = "foo_visibility"
        "#;
        let cfg_legacy: PackageCfg = toml::from_str(toml_legacy).expect("deserialize PackageCfg");
        assert!(cfg_legacy.validate().is_ok());
    }

    #[test]
    fn deserialize_target_cfg_with_remove_features() {
        let toml_str = r#"
            features = [ "alloc" ]
            remove_features = [ "std" ]
            rustflags = [ "--cfg=foo" ]
            remove_rustflags = [ "--cfg=bar" ]
        "#;

        let cfg: TargetCfg = toml::from_str(toml_str).expect("deserialize TargetCfg");
        assert_eq!(cfg.features, Some(vec!["alloc".to_string()]));
        assert_eq!(cfg.remove_features, Some(vec!["std".to_string()]));
        assert_eq!(cfg.rustflags, Some(vec!["--cfg=foo".to_string()]));
        assert_eq!(cfg.remove_rustflags, Some(vec!["--cfg=bar".to_string()]));
    }

    #[test]
    fn deserialize_global_target_cfgs_with_min_publish_age() {
        let toml_str = r#"
            min_publish_age = 14
            require_licenses = true
        "#;

        let cfg: GlobalTargetCfgs = toml::from_str(toml_str).expect("deserialize GlobalTargetCfgs");
        assert_eq!(cfg.min_publish_age, Some(14));
        assert_eq!(cfg.require_licenses, Some(true));
        assert!(cfg.remove_cfgs.is_empty());
        assert!(cfg.add_cfgs.is_empty());
    }

    #[test]
    fn deserialize_package_cfg_with_min_publish_age_override() {
        let toml_str = r#"
            allow_recent_publish = true
            min_publish_age = 0
        "#;

        let cfg: PackageCfg = toml::from_str(toml_str).expect("deserialize PackageCfg");
        assert_eq!(cfg.allow_recent_publish, Some(true));
        assert_eq!(cfg.min_publish_age, Some(0));
        assert!(cfg.validate().is_ok());

        let toml_override = r#"
            min_publish_age = 3
        "#;
        let cfg_override: PackageCfg =
            toml::from_str(toml_override).expect("deserialize PackageCfg");
        assert_eq!(cfg_override.min_publish_age, Some(3));
        assert!(cfg_override.validate().is_ok());
    }
}
