// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Result, anyhow, bail};
use assembly_container::{AssemblyContainer, WalkPaths, assembly_container};
use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::assembly_input_bundle::{CompiledPackageDefinition, ShellCommands};
use crate::{PackageDetails, PackagedDriverDetails};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
pub enum FeedbackBuildTypeConfig {
    #[serde(rename = "eng_with_upload")]
    EngWithUpload,

    #[serde(rename = "userdebug")]
    UserDebug,

    #[serde(rename = "user")]
    User,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct ForensicsOptions {
    /// The build type config Feedback should use.
    pub build_type_override: Option<FeedbackBuildTypeConfig>,
}

// LINT.IfChange

/// Developer Overrides struct that is similar to the ProductConfig struct,
/// but has extra fields added that allow it to convey extra fields.
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, WalkPaths)]
#[assembly_container(assembly_config.json)]
#[serde(default, deny_unknown_fields)]
pub struct DeveloperOverrides {
    /// The label of the target used to define the overrides.
    pub target_name: Option<String>,

    /// Special overrides-only flags to pass to assembly.  These features cannot
    /// be used by products, only by developers that need to override the standard
    /// behavior of assembly.
    ///
    /// Using these will generate warnings.
    pub developer_only_options: DeveloperOnlyOptions,

    /// Developer overrides for the kernel.
    ///
    /// Using these will generate warnings.
    pub kernel: KernelOptions,

    /// Developer overrides for the platform configuration.
    ///
    /// This is a 'Value' so that it can be be used to overlay the product's
    /// platform configuration before that's parsed into it's real type.
    pub platform: serde_json::Value,

    /// Developer overrides for the product configuration
    ///
    /// This is a 'Value' so that it can be be used to overlay the product's
    /// product configuration before that's parsed into it's real type.
    pub product: serde_json::Value,

    /// Developer overrides for the board configuration
    ///
    /// This is a 'Value' so that it can be be used to overlay the
    /// board configuration before that's parsed into it's real type.
    pub board: serde_json::Value,

    /// Packages to add to the build.
    #[walk_paths]
    pub packages: Vec<PackageDetails>,

    /// Drivers to add to the build.
    #[serde(default)]
    #[walk_paths]
    pub drivers: Vec<PackagedDriverDetails>,

    /// Compiled components to add to the build
    #[walk_paths]
    pub packages_to_compile: Vec<CompiledPackageDefinition>,

    /// Map of the names of packages that contain shell commands to the list of
    /// commands within each.
    pub shell_commands: ShellCommands,

    /// Map of files to apply to the platform and product configuration schemas.
    ///
    /// This are separately tracked because they need to be resolved from the
    /// path to the file that contains the developer overrides, and it's
    /// otherwise not known that they they are file paths, as these fields are
    /// all serde_json::Value type.
    #[walk_paths]
    pub developer_provided_files: Vec<DeveloperProvidedFilesNode>,

    /// Bootfs files to add via developer overrides
    #[walk_paths]
    pub bootfs_files_package: Option<Utf8PathBuf>,
}

/// Special flags for assembly that can only be used in the context of developer
/// overrides.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct DeveloperOnlyOptions {
    /// Force all non-bootfs packages known to assembly to be in the base package
    /// set (cache, universe, etc.).
    ///
    /// This feature exists to enable the use of a product image that has cache
    /// or universe packages in a context where networking is unavailable or
    /// a package server cannot be run.
    pub all_packages_in_base: bool,

    /// Whether to enable netboot mode for assembly.
    pub netboot_mode: bool,

    pub forensics_options: ForensicsOptions,
}

/// Kernel options and settings that are only to be used in the context of local
/// developer overrides.
#[derive(Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct KernelOptions {
    /// Additional kernel command line args to add to the assembled ZBI.
    pub command_line_args: Vec<String>,
}

/// A path to a "node" in the json configuration
///
/// A path to a "node" in the json configuration, which has one or more fields
/// which are paths to a developer-provided file, and therefore need to be
/// resolved correctly, based on the developer overrides file location, before
/// they can be merged with the product assembly config.
///
/// Example:
///   node_path = `foo.bar`
///   fields = { "field": "some/path/to/a/file.txt" }
///
/// would be:
///   {
///     foo: {
///       bar: {
///         field: "some/path/to/a/file.txt"
///       }
///     }
///   }
///
#[derive(Debug, Deserialize, Serialize, PartialEq, WalkPaths)]
#[serde(deny_unknown_fields)]
pub struct DeveloperProvidedFilesNode {
    /// The path to a json object, in "foo.bar" notation.
    pub node_path: String,

    /// A map of field-names to file paths.
    #[walk_paths]
    pub fields: BTreeMap<String, Utf8PathBuf>,
}

// LINT.ThenChange(//build/assembly/scripts/developer_overrides.py)

impl DeveloperOverrides {
    /// Merge all of the developer-provided files into the appropriate
    /// `serde_json::Value` fields of the DeveloperOverrides struct
    pub fn merge_developer_provided_files(self) -> Result<Self> {
        let developer_provided_files = self.developer_provided_files;
        let mut platform = self.platform;
        let mut product = self.product;

        for node_entry in developer_provided_files {
            // Split the node into path segments.
            let node_path: Vec<_> = node_entry.node_path.split(".").collect();
            let node_path = node_path.as_slice();

            // The first is the main Value to apply to (product, platform, etc.)
            let Some((current_node_name, remaining_path)) = node_path.split_first() else {
                bail!("Empty node_path.");
            };
            // The checked split above makes this safe to assume.
            let starting_node = match *current_node_name {
                "platform" => &mut platform,
                "product" => &mut product,
                unknown => {
                    bail!("Unknown node_path: {}", unknown);
                }
            };
            merge_file_paths(
                starting_node,
                remaining_path,
                vec![current_node_name],
                node_entry.fields,
            )?;
        }

        Ok(Self { developer_provided_files: vec![], platform, product, ..self })
    }
}

/// Given a Value, a path of nodes, and a set of fields that need to be set to file paths,
/// recursively walk the Value and its child Values (as objects), and then set the value of the
/// given fields to the file paths.
///
/// Given the following:
///   value:  { "foo": { "bar": { "existing": true } } }
///   remaining_node_path: [ "foo", "bar" ]
///   fields: [ "field1": "some/path/to/file1.txt", "field2": "some/path/to/file2.txt" ]
///
/// It recurses to the node 'bar', and then adds 'field1' and 'field2':
///   {
///     "foo": {
///       "bar": {
///         "existing": true,
///         "field1": "some/path/to/file1.txt",
///         "field2": "some/path/to/file2.txt",
///       }
///     }
///   }
///
/// If the node along the path is missing, it's added.  And if an existing value is found, an error
/// is returned.
fn merge_file_paths<'a>(
    value: &mut serde_json::Value,
    remaining_node_path: &[&'a str],
    mut previous_node_path: Vec<&'a str>,
    fields: BTreeMap<String, Utf8PathBuf>,
) -> Result<()> {
    let node_map = value
        .as_object_mut()
        .ok_or_else(|| anyhow!("Encountered a non-object node in the node path."))?;

    let (current_node_name, remaining_path_elements) = remaining_node_path
        .split_first()
        .ok_or_else(|| anyhow!("out of node path elements, should be unreachable."))?;
    // This is safe because of the above checked split.

    let current_node =
        node_map.entry(current_node_name.to_owned()).or_insert_with(|| serde_json::json!({}));

    if remaining_path_elements.is_empty() {
        // Assign the fields to this node.
        let current_node_map = current_node
            .as_object_mut()
            .ok_or_else(|| anyhow!("Encountered a non-object node in the node path."))?;

        for (field, path) in fields {
            if current_node_map.contains_key(&field) {
                bail!("There is already a value for the field: {}", field);
            }
            current_node_map.insert(field, serde_json::json!(path));
        }
    } else {
        // recurse into the current node.
        previous_node_path.push(current_node_name);
        merge_file_paths(current_node, remaining_path_elements, previous_node_path, fields)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn test_basic_merge() {
        let mut value = json!({
            "foo": {
                "bar": {
                    "existing": true
                }
            }
        });

        let remaining_node_path = ["foo", "bar"];
        let fields = BTreeMap::from([
            ("field1".to_owned(), "some/path/to/file1.txt".into()),
            ("field2".to_owned(), "some/path/to/file2.txt".into()),
        ]);

        let result = merge_file_paths(&mut value, &remaining_node_path, vec![], fields);
        assert!(result.is_ok());
        assert_eq!(
            value,
            json!({
                "foo": {
                    "bar": {
                        "existing": true,
                        "field1": "some/path/to/file1.txt",
                        "field2": "some/path/to/file2.txt"
                    }
                }
            })
        );
    }

    #[test]
    fn test_merge_creates_nodes() {
        let mut value = json!({});

        let remaining_node_path = ["foo", "bar"];
        let fields = BTreeMap::from([
            ("field1".to_owned(), "some/path/to/file1.txt".into()),
            ("field2".to_owned(), "some/path/to/file2.txt".into()),
        ]);

        let result = merge_file_paths(&mut value, &remaining_node_path, vec![], fields);
        assert!(result.is_ok());
        assert_eq!(
            value,
            json!({
                "foo": {
                    "bar": {
                        "field1": "some/path/to/file1.txt",
                        "field2": "some/path/to/file2.txt"
                    }
                }
            })
        );
    }

    #[test]
    fn test_merge_errors_on_node_path_to_field() {
        let mut value = json!({
            "foo": {
                "bar": true
            }
        });

        let remaining_node_path = ["foo", "bar"];
        let fields = BTreeMap::from([("field1".to_owned(), "some/path/to/file1.txt".into())]);

        let result = merge_file_paths(&mut value, &remaining_node_path, vec![], fields);
        assert!(result.is_err());
    }

    #[test]
    fn test_merge_errors_on_exiting_field() {
        let mut value = json!({
            "foo": {
                "bar": true
            }
        });

        let remaining_node_path = ["foo"];
        let fields = BTreeMap::from([("bar".to_owned(), "some/path/to/file1.txt".into())]);

        let result = merge_file_paths(&mut value, &remaining_node_path, vec![], fields);
        assert!(result.is_err());
    }

    #[test]
    fn test_merge_developer_provided_files() {
        let developer_overrides = DeveloperOverrides {
            target_name: Some("target_name".to_owned()),
            platform: json!({}),
            product: json!({}),
            developer_provided_files: vec![
                DeveloperProvidedFilesNode {
                    node_path: "platform.development_support".to_owned(),
                    fields: BTreeMap::from([(
                        "authorized_ssh_keys_path".to_owned(),
                        "resources/keys/test.pem".into(),
                    )]),
                },
                DeveloperProvidedFilesNode {
                    node_path: "product.build_info".to_owned(),
                    fields: BTreeMap::from([(
                        "version".to_owned(),
                        "resources/build_info/version.txt".into(),
                    )]),
                },
            ],
            ..Default::default()
        };

        let merge_result = developer_overrides.merge_developer_provided_files();
        let merged = merge_result.unwrap();

        assert_eq!(
            merged,
            DeveloperOverrides {
                target_name: Some("target_name".to_owned()),
                platform: json!({
                    "development_support": {
                        "authorized_ssh_keys_path": "resources/keys/test.pem"
                    }
                }),
                product: json!({
                    "build_info": {
                        "version": "resources/build_info/version.txt"
                    }
                }),
                ..Default::default()
            }
        );
    }
}
