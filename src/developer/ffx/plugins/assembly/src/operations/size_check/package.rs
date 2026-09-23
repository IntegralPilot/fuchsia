// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::operations::size_check::common::{PackageBlobSizeInfo, PackageSizeInfo};
use anyhow::{Context, Result, anyhow, format_err};
use assembly_blob_size::BlobSizeCalculator;
use assembly_sdk::SdkToolProvider;
use assembly_tool::ToolProvider;
use assembly_util::{read_config, write_json_file};
use camino::{Utf8Path, Utf8PathBuf};
use errors::ffx_bail;
use ffx_assembly_args::PackageSizeCheckArgs;
use ffx_config::EnvironmentContext;
use fuchsia_hash::Hash;
use fuchsia_pkg::PackageManifest;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{BTreeMap, HashSet};
use url::Url;

/// Blob information. Entry of the "blobs.json" file.
#[derive(Debug, Deserialize, PartialEq)]
pub struct BlobJsonEntry {
    /// Hash of the head for the blob tree.
    pub merkle: Hash,
    /// Size of the content in bytes, once compressed and aligned.
    pub size: u64,
}

/// Root of size checker JSON configuration.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct BudgetConfig {
    /// Apply a size budget to packages.
    #[serde(default)]
    pub package_set_budgets: Vec<PackageSetBudget>,
}

/// Size budget for a set of packages.
/// Part of JSON configuration.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct PackageSetBudget {
    /// Human readable name of the package set.
    pub name: String,
    /// Number of bytes allotted for the packages of this group.
    pub budget_bytes: u64,
    /// Allowed usage increase allowed for a given commit.
    #[serde(default)]
    pub creep_budget_bytes: u64,
    /// List of paths to `package_manifest.json` files for each package of the set of the group.
    pub packages: Vec<Utf8PathBuf>,
}

/// Intermediate data structure indexed by blob hash used to count how many times a blob is used.
struct BlobSizeAndCount {
    /// Size of the blob content in bytes, once compressed and aligned.
    size: u64,
    /// Number of packages using this blob.
    share_count: u64,
}

#[derive(Clone)]
struct BlobInstance {
    // Hash of the blob merkle root.
    hash: Hash,
    package_path: Utf8PathBuf,
    package_name: String,
    path: String,
}

#[derive(Clone)]
struct BudgetBlobs {
    // TODO(samans): BudgetResult is supposed to represent the result of size checker and should
    // not be used here.
    /// Budget to which one the blobs applies.
    budget: BudgetResult,
    /// List blobs that are charged to a given budget.
    blobs: Vec<BlobInstance>,
}

#[derive(Debug, Clone, Serialize, Eq, PartialEq)]
struct BudgetResult {
    /// Human readable name of this budget.
    pub name: String,
    /// Number of bytes allotted to the packages this budget applies to.
    pub budget_bytes: u64,
    /// Allowed usage increase allowed for a given commit.
    pub creep_budget_bytes: u64,
    /// Number of bytes used by the packages this budget applies to.
    pub used_bytes: u64,
    /// Breakdown of storage consumption by package.
    pub package_breakdown: BTreeMap<Utf8PathBuf, PackageSizeInfo>,
}

/// Verifies that no package budget is exceeded.
pub fn verify_package_budgets<W: std::io::Write>(
    writer: &mut W,
    ctx: &EnvironmentContext,
    args: PackageSizeCheckArgs,
) -> Result<bool> {
    let sdk_tools = SdkToolProvider::try_new(ctx).context("Getting SDK tools")?;
    verify_budgets_with_tools(writer, args, Box::new(sdk_tools))
}

fn verify_budgets_with_tools<W: std::io::Write>(
    writer: &mut W,
    args: PackageSizeCheckArgs,
    tools: Box<dyn ToolProvider>,
) -> Result<bool> {
    let blob_size_calculator = BlobSizeCalculator::new(tools, args.blobfs_layout);

    // Read the budget configuration file.
    let config: BudgetConfig = read_config(&args.budgets)?;

    // List blobs hashes for each package manifest of each package budget.
    let package_budget_blobs = load_manifests_blobs_match_budgets(&config.package_set_budgets)?;

    // Read blob json file if any, and collect sizes on target.
    let blobs = load_blob_info(&args.blob_sizes, &package_budget_blobs, &blob_size_calculator)?;

    // Calculate the budget results.
    let mut results = compute_budget_results(&package_budget_blobs, &blobs)?;

    // Write the output result if requested by the command line.
    if let Some(out_path) = &args.gerrit_output {
        write_json_file(out_path, &to_json_output(&results)?)?;
    }

    if let Some(verbose_json_output) = args.verbose_json_output {
        let output: BTreeMap<&str, &BudgetResult> =
            results.iter().map(|v| (v.name.as_str(), v)).collect();
        write_json_file(verbose_json_output, &output)?;
    }

    // Print a text report for each overrun budget.
    let over_budget = results.iter().filter(|e| e.used_bytes > e.budget_bytes).count();

    if over_budget > 0 {
        writeln!(writer, "FAILED: {over_budget} package set(s) over budget")
            .context("Failed to write package budget output")?;
    }
    if args.verbose || over_budget > 0 {
        // Order the results by package set name.
        results.sort_by(|lhs, rhs| lhs.name.cmp(&rhs.name));

        writeln!(
            writer,
            "{:<40} {:>10} {:>10} {:>10}",
            "Package Sets", "Size", "Budget", "Remaining"
        )
        .context("Failed to write package budget output")?;
        for result in &results {
            // Only print the component usage if it went over budget or verbose output is
            // requested.
            if !args.verbose && result.used_bytes <= result.budget_bytes {
                continue;
            }
            writeln!(
                writer,
                "{:<40} {:>10} {:>10} {:>10}",
                result.name,
                result.used_bytes,
                result.budget_bytes,
                result.budget_bytes as i64 - result.used_bytes as i64
            )
            .context("Failed to write package budget output")?;
            // Only print the package breakdown if verbose output is requested.
            if !args.verbose {
                continue;
            }

            // Order the package breakdown by file name.
            let package_breakdown = result
                .package_breakdown
                .iter()
                .map(|(key, value)| {
                    let name = key.file_name().ok_or_else(|| {
                        format_err!("Can't extract file name from path {:?}", key)
                    })?;
                    Ok((name, value))
                })
                .collect::<Result<BTreeMap<_, _>>>()?;

            for (key, value) in package_breakdown.iter() {
                writeln!(writer, "    {:<36} {:>10}", key, value.proportional_size)
                    .context("Failed to write package budget output")?;
            }
        }
        if let Some(out_path) = &args.gerrit_output {
            writeln!(writer, "Report written to {out_path}")
                .context("Failed to write package budget output")?;
        }
    }

    Ok(over_budget == 0)
}

/// Reads each mentioned package manifest.
/// Returns pairs of budget and the list of blobs consuming this budget.
#[allow(clippy::ptr_arg)]
fn load_manifests_blobs_match_budgets(budgets: &Vec<PackageSetBudget>) -> Result<Vec<BudgetBlobs>> {
    let mut budget_blobs = Vec::new();
    if budgets.is_empty() {
        ffx_bail!(
            "Packages budget is empty, check the `package_set_budgets` field in your budget file."
        );
    }
    for budget in budgets.iter() {
        let mut budget_blob = BudgetBlobs {
            budget: BudgetResult {
                name: budget.name.clone(),
                budget_bytes: budget.budget_bytes,
                creep_budget_bytes: budget.creep_budget_bytes,
                used_bytes: 0,
                package_breakdown: BTreeMap::new(),
            },
            blobs: Vec::new(),
        };

        for package in budget.packages.iter() {
            let manifest = PackageManifest::try_load_from(package)?;
            let package_name = manifest.name().clone();
            for manifest_blob in manifest.into_blobs().drain(..) {
                budget_blob.blobs.push(BlobInstance {
                    hash: manifest_blob.merkle,
                    package_path: package.clone(),
                    package_name: package_name.to_string(),
                    path: manifest_blob.path,
                });
            }
        }

        budget_blobs.push(budget_blob);
    }
    Ok(budget_blobs)
}

/// Load the list of blobs and their sizes from a set of input files.
/// If any blobs are necessary for the budget calculation but are missing,
/// we calculate their size first.
/// TODO(https://fxbug.dev/42055004): Pass BlobsJson struct from blobfs.rs as input.
#[allow(clippy::ptr_arg)]
fn load_blob_info(
    blob_size_paths: &Vec<Utf8PathBuf>,
    blob_usages: &Vec<BudgetBlobs>,
    blob_size_calculator: &BlobSizeCalculator,
) -> Result<Vec<BlobJsonEntry>> {
    let mut result = vec![];
    for blobs_path in blob_size_paths.iter() {
        let mut blobs: Vec<BlobJsonEntry> = read_config(blobs_path)?;
        result.append(&mut blobs);
    }

    let found_blobs = result.iter().map(|blob| blob.merkle).collect::<HashSet<Hash>>();

    // Select packages for which one or more blob is missing.
    let incomplete_packages: Vec<&Utf8Path> = blob_usages
        .iter()
        .flat_map(|budget| &budget.blobs)
        .filter(|blob| !found_blobs.contains(&blob.hash))
        .map(|blob| blob.package_path.as_path())
        .collect::<HashSet<&Utf8Path>>()
        .drain()
        .collect();

    // Build blobfs and complete the blobs database if we found blobs absent from
    // `blob_size_paths`.
    if !incomplete_packages.is_empty() {
        let mut blobs = blob_size_calculator.calculate(&incomplete_packages).unwrap_or_else(|e| {
            log::warn!("Failed to build the blobfs: {:?}", e);
            Vec::default()
        });
        let mut blobs =
            blobs.iter_mut().map(|b| BlobJsonEntry { merkle: b.merkle, size: b.size }).collect();
        result.append(&mut blobs);
    }

    Ok(result)
}

/// Reads blob declaration file and count how many times blobs are used.
#[allow(clippy::ptr_arg)]
fn index_blobs_by_hash(
    blob_sizes: &Vec<BlobJsonEntry>,
    blob_count_by_hash: &mut BTreeMap<Hash, BlobSizeAndCount>,
) -> Result<()> {
    for blob_entry in blob_sizes.iter() {
        if let Some(previous) = blob_count_by_hash
            .insert(blob_entry.merkle, BlobSizeAndCount { size: blob_entry.size, share_count: 0 })
        {
            if previous.size != blob_entry.size {
                return Err(anyhow!(
                    "Two blobs with same hash {} but different sizes",
                    blob_entry.merkle
                ));
            }
        }
    }
    Ok(())
}

/// Reads blob declaration file, and count how many times blobs are used.
#[allow(clippy::ptr_arg)]
fn count_blobs(
    blob_sizes: &Vec<BlobJsonEntry>,
    blob_usages: &Vec<BudgetBlobs>,
) -> Result<BTreeMap<Hash, BlobSizeAndCount>> {
    // Index blobs by hash.
    let mut blob_count_by_hash: BTreeMap<Hash, BlobSizeAndCount> = BTreeMap::new();
    index_blobs_by_hash(blob_sizes, &mut blob_count_by_hash)?;

    // Count how many times a blob is shared and report missing blobs.
    for budget_usage in blob_usages.iter() {
        for blob in budget_usage.blobs.iter() {
            match blob_count_by_hash.get_mut(&blob.hash) {
                Some(blob_entry_count) => {
                    blob_entry_count.share_count += 1;
                }
                None => {
                    return Err(anyhow!(
                        "ERROR: Blob not found for budget '{}' package '{}' path '{}' hash '{}'",
                        budget_usage.budget.name,
                        blob.package_path,
                        blob.path,
                        blob.hash
                    ));
                }
            }
        }
    }

    Ok(blob_count_by_hash)
}

// Computes the total size of each component taking into account blob sharing.
#[allow(clippy::ptr_arg)]
fn compute_budget_results(
    budget_usages: &Vec<BudgetBlobs>,
    blob_sizes: &Vec<BlobJsonEntry>,
) -> Result<Vec<BudgetResult>> {
    let mut result = vec![];
    for budget_usage in budget_usages.iter() {
        let single_budget_usage = vec![budget_usage.clone()];
        let blob_count_by_hash = count_blobs(blob_sizes, &single_budget_usage)?;

        let mut used_bytes = budget_usage.budget.used_bytes;
        let filtered_blobs = budget_usage.blobs.iter().collect::<Vec<&BlobInstance>>();

        used_bytes += filtered_blobs
            .iter()
            .map(|blob| match blob_count_by_hash.get(&blob.hash) {
                Some(blob_entry_count) => blob_entry_count.size / blob_entry_count.share_count,
                None => 0,
            })
            .sum::<u64>();

        let mut package_breakdown = BTreeMap::new();
        for blob in filtered_blobs {
            let count = blob_count_by_hash.get(&blob.hash).ok_or_else(|| {
                format_err!(
                    "Can't find blob {} from package {:?} in map",
                    blob.hash,
                    blob.package_path
                )
            })?;
            let package_result =
                package_breakdown.entry(blob.package_path.clone()).or_insert_with(|| {
                    PackageSizeInfo {
                        name: blob.package_name.clone(),
                        proportional_size: 0,
                        used_space_in_blobfs: 0,
                        blobs: vec![],
                    }
                });
            package_result.proportional_size += count.size / count.share_count;
            package_result.used_space_in_blobfs += count.size;
            package_result.blobs.push(PackageBlobSizeInfo {
                merkle: blob.hash,
                used_space_in_blobfs: count.size,
                share_count: count.share_count,
                absolute_share_count: count.share_count,
                path_in_package: blob.path.clone(),
            });
        }

        result.push(BudgetResult {
            name: budget_usage.budget.name.clone(),
            used_bytes,
            package_breakdown,
            ..budget_usage.budget
        });
    }
    Ok(result)
}

/// Builds a report with the gerrit size checker format from the computed component size and budget.
#[allow(clippy::ptr_arg)]
fn to_json_output(
    budget_usages: &Vec<BudgetResult>,
) -> Result<BTreeMap<String, serde_json::Value>> {
    // Use an ordered map to ensure the output is readable and stable.
    let mut budget_output = BTreeMap::new();
    for entry in budget_usages.iter() {
        budget_output.insert(entry.name.clone(), json!(entry.used_bytes));
        budget_output.insert(format!("{}.budget", entry.name), json!(entry.budget_bytes));
        budget_output
            .insert(format!("{}.creepBudget", entry.name), json!(entry.creep_budget_bytes));
        let url = Url::parse_with_params(
            "http://go/fuchsia-size-stats/single_component/",
            &[("f", format!("component:in:{}", entry.name))],
        )?;
        budget_output.insert(format!("{}.owner", entry.name), json!(url.as_str()));
    }
    Ok(budget_output)
}

#[cfg(test)]
#[allow(clippy::box_default)]
mod tests {
    use crate::operations::size_check::package::{
        BlobInstance, BlobJsonEntry, BudgetBlobs, BudgetConfig, BudgetResult, PackageBlobSizeInfo,
        PackageSizeInfo, compute_budget_results, verify_budgets_with_tools,
    };
    use anyhow::Result;
    use assembly_images_config::BlobfsLayout;
    use assembly_tool::testing::FakeToolProvider;
    use assembly_util::{read_config, write_json_file};
    use camino::Utf8PathBuf;
    use errors::IntoExitCode;
    use ffx_assembly_args::PackageSizeCheckArgs;
    use fuchsia_hash::Hash;
    use pretty_assertions::assert_eq;
    use serde_json::json;
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::Path;
    use std::str::FromStr;
    use tempfile::TempDir;

    struct TestFs {
        root: TempDir,
    }

    impl TestFs {
        fn new() -> TestFs {
            TestFs { root: TempDir::new().unwrap() }
        }

        fn write(&self, rel_path: &str, value: serde_json::Value) {
            let path = self.root.path().join(rel_path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            println!("Write {}", path.display());
            write_json_file(&path, &value).unwrap()
        }

        fn assert_eq(&self, rel_path: &str, expected: serde_json::Value) {
            let path = self.root.path().join(rel_path);
            let actual: serde_json::Value = read_config(path).unwrap();
            assert_eq!(actual, expected);
        }

        fn path(&self, rel_path: &str) -> Utf8PathBuf {
            self.root.path().join(rel_path).try_into().unwrap()
        }
    }

    fn assert_failed<T, E>(err: Result<T, E>, prefix: &str)
    where
        E: std::fmt::Display,
    {
        match err {
            Ok(_) => panic!("Unexpected success, where a failure was expected."),
            Err(e) => assert!(
                e.to_string().starts_with(prefix),
                "Unexpected error message:\n\t{e:#}\ndoes not start with:\n\t{prefix}"
            ),
        }
    }

    #[test]
    fn default_creep() {
        let budgets: BudgetConfig = serde_json::from_value(json!({
        "package_set_budgets":[
            {
                "name": "budget_name",
                "budget_bytes": 10,
                "packages": [],
            },
        ]}))
        .unwrap();
        assert_eq!(budgets.package_set_budgets.len(), 1);
        let package_set_budget = &budgets.package_set_budgets[0];
        assert_eq!(package_set_budget.creep_budget_bytes, 0);
    }

    #[test]
    fn fails_because_of_missing_blobs_file() {
        let test_fs = TestFs::new();
        test_fs.write("size_budgets.json", json!({}));
        let err = verify_budgets_with_tools(
            &mut std::io::sink(),
            PackageSizeCheckArgs {
                blobfs_layout: BlobfsLayout::Compact,
                budgets: test_fs.path("size_budgets.json"),
                blob_sizes: [test_fs.path("blobs.json")].to_vec(),
                gerrit_output: None,
                verbose: false,
                verbose_json_output: None,
            },
            Box::new(FakeToolProvider::default()),
        );
        assert_failed(err, "Packages budget is empty");
    }

    #[test]
    fn fails_because_of_missing_budget_file() {
        let test_fs = TestFs::new();
        test_fs.write("blobs.json", json!([]));
        let err = verify_budgets_with_tools(
            &mut std::io::sink(),
            PackageSizeCheckArgs {
                blobfs_layout: BlobfsLayout::Compact,
                budgets: test_fs.path("size_budgets.json"),
                blob_sizes: [test_fs.path("blobs.json")].to_vec(),
                gerrit_output: None,
                verbose: false,
                verbose_json_output: None,
            },
            Box::new(FakeToolProvider::default()),
        );
        assert_eq!(err.as_ref().unwrap_err().exit_code(), 1);
        assert_failed(err, "Unable to open file:");
    }

    #[test]
    fn succeeds_because_equals_maximum_budget() {
        let test_fs = TestFs::new();
        test_fs.write(
            "size_budgets.json",
            json!({
            "package_set_budgets":[
                {
                    "name": "Software Delivery",
                    "budget_bytes": 27,
                    "creep_budget_bytes": 2i32,
                    "packages": [],
                },
                {
                    "name": "Component Framework",
                    "budget_bytes": 51i32,
                    "creep_budget_bytes": 2i32,
                    "packages": [],
                }
            ]}),
        );
        test_fs.write("blobs.json", json!([]));
        verify_budgets_with_tools(
            &mut std::io::sink(),
            PackageSizeCheckArgs {
                blobfs_layout: BlobfsLayout::Compact,
                budgets: test_fs.path("size_budgets.json"),
                blob_sizes: [test_fs.path("blobs.json")].to_vec(),
                gerrit_output: None,
                verbose: false,
                verbose_json_output: None,
            },
            Box::new(FakeToolProvider::default()),
        )
        .unwrap();
    }

    #[test]
    fn duplicate_merkle_in_blobs_file_with_different_sizes_causes_failure() {
        let test_fs = TestFs::new();
        test_fs.write(
            "size_budgets.json",
            json!({"package_set_budgets":[{
                "name": "Software Delivery",
                "budget_bytes": 1i32,
                "creep_budget_bytes": 2i32,
                "packages": [],
            }]}),
        );

        test_fs.write(
            "blobs.json",
            json!([{
                "merkle": "0e56473237b6b2ce39358c11a0fbd2f89902f246d966898d7d787c9025124d51",
                "size": 8i32
            },{
                "merkle": "0e56473237b6b2ce39358c11a0fbd2f89902f246d966898d7d787c9025124d51",
                "size": 16i32
            }]),
        );
        let res = verify_budgets_with_tools(
            &mut std::io::sink(),
            PackageSizeCheckArgs {
                blobfs_layout: BlobfsLayout::Compact,
                budgets: test_fs.path("size_budgets.json"),
                blob_sizes: [test_fs.path("blobs.json")].to_vec(),
                gerrit_output: None,
                verbose: false,
                verbose_json_output: None,
            },
            Box::new(FakeToolProvider::default()),
        );
        assert_failed(
            res,
            "Two blobs with same hash 0e56473237b6b2ce39358c11a0fbd2f89902f246d966898d7d787c9025124d51 but different sizes",
        );
    }

    #[test]
    fn duplicate_merkle_in_blobs_with_same_size_are_fine() {
        let test_fs = TestFs::new();
        test_fs.write(
            "size_budgets.json",
            json!({"package_set_budgets" :[{
                "name": "Software Deliver",
                "budget_bytes": 1i32,
                "creep_budget_bytes": 1i32,
                "packages": [],
            }]}),
        );

        test_fs.write(
            "blobs.json",
            json!([{
                "merkle": "0e56473237b6b2ce39358c11a0fbd2f89902f246d966898d7d787c9025124d51",
                "size": 16i32
            },{
                "merkle": "0e56473237b6b2ce39358c11a0fbd2f89902f246d966898d7d787c9025124d51",
                "size": 16i32
            }]),
        );
        verify_budgets_with_tools(
            &mut std::io::sink(),
            PackageSizeCheckArgs {
                blobfs_layout: BlobfsLayout::Compact,
                budgets: test_fs.path("size_budgets.json"),
                blob_sizes: [test_fs.path("blobs.json")].to_vec(),
                gerrit_output: None,
                verbose: false,
                verbose_json_output: None,
            },
            Box::new(FakeToolProvider::default()),
        )
        .unwrap();
    }

    #[test]
    fn blob_size_are_summed_test() {
        let test_fs = TestFs::new();
        test_fs.write(
            "size_budgets.json",
            json!({"package_set_budgets":[{
                "name": "Software Deliver",
                "creep_budget_bytes": 2i32,
                "budget_bytes": 7497932i32,
                "packages": [
                    test_fs.path("obj/src/sys/pkg/bin/pkg-cache/pkg-cache/package_manifest.json"),
                ]
            }]}),
        );
        test_fs.write(
            "obj/src/sys/pkg/bin/pkg-cache/pkg-cache/package_manifest.json",
            json!({
                "version": "1",
                "repository": "testrepository.com",
                "package": {
                    "name": "pkg-cache",
                    "version": "0"
                },
                "blobs" : [{
                    "source_path": "first_blob",
                    "path": "first_blob",
                    "merkle": "0e56473237b6b2ce39358c11a0fbd2f89902f246d966898d7d787c9025124d51",
                    "size": 1i32
                },{
                    "source_path": "second_blob",
                    "path": "second_blob",
                    "merkle": "b62ee413090825c2ae70fe143b34cbd851f055932cfd5e7ca4ef0efbb802da2f",
                    "size": 2i32
                }]
            }),
        );
        test_fs.write(
            "blobs.json",
            json!([{
                "merkle": "0e56473237b6b2ce39358c11a0fbd2f89902f246d966898d7d787c9025124d51",
                "size": 8i32
            },{
                "merkle": "b62ee413090825c2ae70fe143b34cbd851f055932cfd5e7ca4ef0efbb802da2f",
                "size": 32i32
            },{
                "merkle": "01ecd6256f89243e1f0f7d7022cc2e8eb059b06c941d334d9ffb108478749646",
                "size": 128i32
            }]),
        );
        verify_budgets_with_tools(
            &mut std::io::sink(),
            PackageSizeCheckArgs {
                blobfs_layout: BlobfsLayout::Compact,
                budgets: test_fs.path("size_budgets.json"),
                blob_sizes: [test_fs.path("blobs.json")].to_vec(),
                gerrit_output: Some(test_fs.path("output.json")),
                verbose: false,
                verbose_json_output: None,
            },
            Box::new(FakeToolProvider::default()),
        )
        .unwrap();

        test_fs.assert_eq(
            "output.json",
            json!({
                "Software Deliver": 40i32,
                "Software Deliver.budget": 7497932i32,
                "Software Deliver.creepBudget": 2i32,
                "Software Deliver.owner":
                "http://go/fuchsia-size-stats/single_component/?f=component%3Ain%3ASoftware+Deliver"}),
        );
    }

    #[test]
    fn blob_shared_by_two_budgets_test() {
        let test_fs = TestFs::new();
        test_fs.write(
            "size_budgets.json",
            json!({"package_set_budgets":[{
            "name": "Software Deliver",
                "creep_budget_bytes": 1i32,
                "budget_bytes": 7497932i32,
                "packages": [
                    test_fs.path("obj/src/sys/pkg/bin/pkg-cache/pkg-cache/package_manifest.json"),
                    test_fs.path("obj/src/sys/pkg/bin/pkgfs/pkgfs/package_manifest.json"),
                ]
            },{
                "name": "Connectivity",
                "creep_budget_bytes": 1i32,
                "budget_bytes": 10884219,
                "packages": [
                    test_fs.path( "obj/src/connectivity/bluetooth/core/bt-gap/bt-gap/package_manifest.json"),]
            }]}),
        );
        test_fs.write(
            "obj/src/sys/pkg/bin/pkg-cache/pkg-cache/package_manifest.json",
            json!({
                "version": "1",
                "repository": "testrepository.com",
                "package": {
                    "name": "pkg-cache",
                    "version": "0"
                },
                "blobs" : [{
                    "source_path": "first_blob",
                    "path": "path/in/pkg-cache",
                    "merkle": "0e56473237b6b2ce39358c11a0fbd2f89902f246d966898d7d787c9025124d51",
                    "size": 4i32
                }]
            }),
        );
        test_fs.write(
            "obj/src/sys/pkg/bin/pkgfs/pkgfs/package_manifest.json",
            json!({
                "version": "1",
                "repository": "testrepository.com",
                "package": {
                    "name": "pkgfs",
                    "version": "0"
                },
                "blobs" : [{
                    "source_path": "first_blob",
                    "path": "path/in/pkgfs",
                    "merkle": "0e56473237b6b2ce39358c11a0fbd2f89902f246d966898d7d787c9025124d51",
                    "size": 8i32
                }]
            }),
        );
        test_fs.write(
            "obj/src/connectivity/bluetooth/core/bt-gap/bt-gap/package_manifest.json",
            json!({
                "version": "1",
                "repository": "testrepository.com",
                "package": {
                    "name": "bt-gap",
                    "version": "0"
                },
                "blobs" : [{
                    "source_path": "first_blob",
                    "path": "path/in/bt-gap",
                    "merkle": "0e56473237b6b2ce39358c11a0fbd2f89902f246d966898d7d787c9025124d51",
                    "size": 16i32
                }]
            }),
        );
        test_fs.write(
            "blobs.json",
            json!([{
              "merkle": "0e56473237b6b2ce39358c11a0fbd2f89902f246d966898d7d787c9025124d51",
              "size": 159i32
            }]),
        );
        verify_budgets_with_tools(
            &mut std::io::sink(),
            PackageSizeCheckArgs {
                blobfs_layout: BlobfsLayout::Compact,
                budgets: test_fs.path("size_budgets.json"),
                blob_sizes: [test_fs.path("blobs.json")].to_vec(),
                gerrit_output: Some(test_fs.path("output.json")),
                verbose: false,
                verbose_json_output: Some(test_fs.path("verbose-output.json")),
            },
            Box::new(FakeToolProvider::default()),
        )
        .unwrap();

        test_fs.assert_eq(
            "output.json",
            json!({
                "Connectivity": 159i32,
                "Connectivity.creepBudget": 1i32,
                "Connectivity.budget": 10884219i32,
                "Connectivity.owner": "http://go/fuchsia-size-stats/single_component/?f=component%3Ain%3AConnectivity",
                "Software Deliver": 158i32,
                "Software Deliver.creepBudget": 1i32,
                "Software Deliver.budget": 7497932i32,
                "Software Deliver.owner": "http://go/fuchsia-size-stats/single_component/?f=component%3Ain%3ASoftware+Deliver"
            }),
        );
        test_fs.assert_eq(
            "verbose-output.json",
            json!({
                "Software Deliver": {
                  "name": "Software Deliver",
                  "budget_bytes": 7497932,
                  "creep_budget_bytes": 1,
                  "used_bytes": 158,
                  "package_breakdown": {
                    test_fs.path("obj/src/sys/pkg/bin/pkg-cache/pkg-cache/package_manifest.json").to_string(): {
                      "proportional_size": 79,
                      "used_space_in_blobfs": 159,
                      "name": "pkg-cache",
                      "blobs": [
                        {
                            "merkle": "0e56473237b6b2ce39358c11a0fbd2f89902f246d966898d7d787c9025124d51",
                            "path_in_package": "path/in/pkg-cache",
                            "used_space_in_blobfs": 159,
                            "share_count": 2,
                            "absolute_share_count": 2,
                        }
                      ]
                    },
                    test_fs.path("obj/src/sys/pkg/bin/pkgfs/pkgfs/package_manifest.json").to_string(): {
                      "proportional_size": 79,
                      "used_space_in_blobfs": 159,
                      "name": "pkgfs",
                      "blobs": [
                        {
                            "merkle": "0e56473237b6b2ce39358c11a0fbd2f89902f246d966898d7d787c9025124d51",
                            "path_in_package": "path/in/pkgfs",
                            "used_space_in_blobfs": 159,
                            "share_count": 2,
                            "absolute_share_count": 2,
                        }
                      ]
                    }
                  }
                },
                "Connectivity": {
                  "name": "Connectivity",
                  "budget_bytes": 10884219,
                  "creep_budget_bytes": 1,
                  "used_bytes": 159,
                  "package_breakdown": {
                    test_fs.path("obj/src/connectivity/bluetooth/core/bt-gap/bt-gap/package_manifest.json").to_string(): {
                      "proportional_size": 159,
                      "used_space_in_blobfs": 159,
                      "name": "bt-gap",
                      "blobs": [
                        {
                            "merkle": "0e56473237b6b2ce39358c11a0fbd2f89902f246d966898d7d787c9025124d51",
                            "path_in_package": "path/in/bt-gap",
                            "used_space_in_blobfs": 159,
                            "share_count": 1,
                            "absolute_share_count": 1,
                        }
                      ]
                    }
                  }
                }
              }
              )
        );
    }

    #[test]
    fn blob_hash_not_found_test() {
        let test_fs = TestFs::new();
        test_fs.write(
            "size_budgets.json",
            json!({"package_set_budgets":[{
                "name": "Connectivity",
                "creep_budget_bytes": 1i32,
                "budget_bytes": 7497932i32,
                "packages": [
                    test_fs.path("obj/src/sys/pkg/bin/pkg-cache/pkg-cache/package_manifest.json"),
                ]
            }]}),
        );
        test_fs.write(
            "obj/src/sys/pkg/bin/pkg-cache/pkg-cache/package_manifest.json",
            json!({
                "version": "1",
                "repository": "testrepository.com",
                "package": {
                    "name": "pkg-cache",
                    "version": "0"
                },
                "blobs" : [{
                    "source_path": "first_blob",
                    "path": "not found",
                    "merkle": "0e56473237b6b2ce39358c11a0fbd2f89902f246d966898d7d787c9025124d51",
                    "size": 4i32
                }]
            }),
        );

        test_fs.write("blobs.json", json!([]));
        let err = verify_budgets_with_tools(
            &mut std::io::sink(),
            PackageSizeCheckArgs {
                blobfs_layout: BlobfsLayout::Compact,
                budgets: test_fs.path("size_budgets.json"),
                blob_sizes: [test_fs.path("blobs.json")].to_vec(),
                gerrit_output: Some(test_fs.path("output.json")),
                verbose: false,
                verbose_json_output: None,
            },
            Box::new(FakeToolProvider::default()),
        );

        assert_failed(err, "ERROR: Blob not found for budget 'Connectivity' package")
    }

    #[test]
    fn generating_blobfs_for_a_missing_file() {
        let test_fs = TestFs::new();
        test_fs.write(
            "size_budgets.json",
            json!({
                "package_set_budgets":[{
                    "name": "Software Deliver",
                    "creep_budget_bytes": 2i32,
                    "budget_bytes": 256i32,
                    "packages": [
                        test_fs.path("obj/src/my_program/package_manifest.json"),
                    ]
                }]
            }),
        );
        test_fs.write(
            "obj/src/my_program/package_manifest.json",
            json!({
                "version": "1",
                "package": {
                    "name": "pkg-cache",
                    "version": "0"
                },
                "blobs" : [{
                    "source_path": test_fs.path("first.txt"),
                    "path": "first",
                    "merkle": "0e56473237b6b2ce39358c11a0fbd2f89902f246d966898d7d787c9025124d51",
                    "size": 8i32
                }, {
                    "source_path": test_fs.path("second.txt"),
                    "path": "second",
                    "merkle": "b62ee413090825c2ae70fe143b34cbd851f055932cfd5e7ca4ef0efbb802da2a",
                    "size": 16i32
                }]
            }),
        );
        test_fs.write("first.txt", json!("some text content"));
        test_fs.write("second.txt", json!("some other text content"));
        test_fs.write(
            "blobs1.json",
            json!([{
                "merkle": "0e56473237b6b2ce39358c11a0fbd2f89902f246d966898d7d787c9025124d51",
                "size": 37i32
            }]),
        );
        let tool_provider =
            Box::new(FakeToolProvider::new_with_side_effect(|_name: &str, args: &[String]| {
                assert_eq!(args[0], "--json-output");
                write_json_file(
                    Path::new(&args[1]),
                    &json!([{
                      "merkle": "b62ee413090825c2ae70fe143b34cbd851f055932cfd5e7ca4ef0efbb802da2a",
                      "size": 73
                    }]),
                )
                .unwrap();
            }));
        verify_budgets_with_tools(
            &mut std::io::sink(),
            PackageSizeCheckArgs {
                blobfs_layout: BlobfsLayout::DeprecatedPadded,
                budgets: test_fs.path("size_budgets.json"),
                blob_sizes: [test_fs.path("blobs1.json")].to_vec(),
                gerrit_output: Some(test_fs.path("output.json")),
                verbose: false,
                verbose_json_output: None,
            },
            tool_provider,
        )
        .unwrap();

        test_fs.assert_eq(
            "output.json",
            json!({
                "Software Deliver": 110i32,
                "Software Deliver.creepBudget": 2i32,
                "Software Deliver.budget": 256i32,
                "Software Deliver.owner": "http://go/fuchsia-size-stats/single_component/?f=component%3Ain%3ASoftware+Deliver"
            }),
        );
    }

    #[test]
    fn test_package_breakdown() -> Result<()> {
        let blob1_hash: Hash =
            Hash::from_str("0e56473237b6b2ce39358c11a0fbd2f89902f246d966898d7d787c9025124d51")
                .unwrap();
        let blob2_hash: Hash =
            Hash::from_str("b62ee413090825c2ae70fe143b34cbd851f055932cfd5e7ca4ef0efbb802da2a")
                .unwrap();
        let blob1_path: &str = "/a/x/blob1";
        let blob2_path: &str = "/a/x/blob2";
        let package1_path = Utf8PathBuf::from("x/a/s/package1");
        let package2_path = Utf8PathBuf::from("x/a/s/package2");
        let package3_path = Utf8PathBuf::from("x/a/s/package3");

        let budget_blobs: Vec<BudgetBlobs> = vec![
            BudgetBlobs {
                budget: BudgetResult {
                    name: "Component1".to_string(),
                    budget_bytes: 123,
                    creep_budget_bytes: 3245,
                    used_bytes: 0,
                    package_breakdown: BTreeMap::new(),
                },
                blobs: vec![
                    BlobInstance {
                        hash: blob1_hash,
                        package_path: package1_path.clone(),
                        package_name: "package1".into(),
                        path: blob1_path.to_string(),
                    },
                    BlobInstance {
                        hash: blob2_hash,
                        package_path: package2_path.clone(),
                        package_name: "package2".into(),
                        path: blob2_path.to_string(),
                    },
                    BlobInstance {
                        hash: blob1_hash,
                        package_path: package2_path.clone(),
                        package_name: "package2".into(),
                        path: blob1_path.to_string(),
                    },
                ],
            },
            BudgetBlobs {
                budget: BudgetResult {
                    name: "Component2".to_string(),
                    budget_bytes: 456,
                    creep_budget_bytes: 111,
                    used_bytes: 6,
                    package_breakdown: BTreeMap::new(),
                },
                blobs: vec![BlobInstance {
                    hash: blob2_hash,
                    package_path: package3_path.clone(),
                    package_name: "package3".into(),
                    path: blob2_path.to_string(),
                }],
            },
        ];
        let blobs = vec![
            BlobJsonEntry { merkle: blob1_hash, size: 90 },
            BlobJsonEntry { merkle: blob2_hash, size: 50 },
        ];
        let results = compute_budget_results(&budget_blobs, &blobs)?;
        let expected_result = vec![
            BudgetResult {
                name: "Component1".to_string(),
                budget_bytes: 123,
                creep_budget_bytes: 3245,
                used_bytes: 140,
                package_breakdown: BTreeMap::from([
                    (
                        package2_path,
                        PackageSizeInfo {
                            name: "package2".into(),
                            proportional_size: 95, /* 90/2 + 50 */
                            used_space_in_blobfs: 140,
                            blobs: vec![
                                PackageBlobSizeInfo {
                                    merkle: blob2_hash,
                                    used_space_in_blobfs: 50,
                                    share_count: 1,
                                    absolute_share_count: 1,
                                    path_in_package: blob2_path.to_string(),
                                },
                                PackageBlobSizeInfo {
                                    merkle: blob1_hash,
                                    path_in_package: blob1_path.to_string(),
                                    used_space_in_blobfs: 90,
                                    share_count: 2,
                                    absolute_share_count: 2,
                                },
                            ],
                        },
                    ),
                    (
                        package1_path,
                        PackageSizeInfo {
                            name: "package1".into(),
                            proportional_size: 45, /* 90/2 */
                            used_space_in_blobfs: 90,
                            blobs: vec![PackageBlobSizeInfo {
                                merkle: blob1_hash,
                                path_in_package: blob1_path.to_string(),
                                used_space_in_blobfs: 90,
                                share_count: 2,
                                absolute_share_count: 2,
                            }],
                        },
                    ),
                ]),
            },
            BudgetResult {
                name: "Component2".to_string(),
                budget_bytes: 456,
                creep_budget_bytes: 111,
                used_bytes: 56, /* 50 + 6 */
                package_breakdown: BTreeMap::from([(
                    package3_path,
                    PackageSizeInfo {
                        name: "package3".into(),
                        proportional_size: 50,
                        used_space_in_blobfs: 50,
                        blobs: vec![PackageBlobSizeInfo {
                            merkle: blob2_hash,
                            path_in_package: blob2_path.to_string(),
                            used_space_in_blobfs: 50,
                            share_count: 1,
                            absolute_share_count: 1,
                        }],
                    },
                )]),
            },
        ];
        assert_eq!(results, expected_result);
        Ok(())
    }

    #[test]
    fn test_verify_budgets_with_tools_return_fits() {
        let test_fs = TestFs::new();
        test_fs.write(
            "package_manifest.json",
            json!({
                "version": "1",
                "repository": "testrepository.com",
                "package": {
                    "name": "my-pkg",
                    "version": "0"
                },
                "blobs": [{
                    "source_path": "first_blob",
                    "path": "path/in/pkg",
                    "merkle": "0e56473237b6b2ce39358c11a0fbd2f89902f246d966898d7d787c9025124d51",
                    "size": 100i32
                }]
            }),
        );
        test_fs.write(
            "size_budgets.json",
            json!({"package_set_budgets": [{
                "name": "Software Delivery",
                "budget_bytes": 10i32,
                "creep_budget_bytes": 2i32,
                "packages": [test_fs.path("package_manifest.json")],
            }]}),
        );
        test_fs.write(
            "blobs.json",
            json!([{
                "merkle": "0e56473237b6b2ce39358c11a0fbd2f89902f246d966898d7d787c9025124d51",
                "size": 100i32
            }]),
        );
        // Over budget: used_bytes (100) > budget_bytes (10)
        let fits = verify_budgets_with_tools(
            &mut std::io::sink(),
            PackageSizeCheckArgs {
                blobfs_layout: BlobfsLayout::Compact,
                budgets: test_fs.path("size_budgets.json"),
                blob_sizes: [test_fs.path("blobs.json")].to_vec(),
                gerrit_output: None,
                verbose: false,
                verbose_json_output: None,
            },
            Box::new(FakeToolProvider::default()),
        )
        .unwrap();
        assert!(!fits);

        // Under budget: budget_bytes (1000) > used_bytes (100)
        test_fs.write(
            "size_budgets.json",
            json!({"package_set_budgets": [{
                "name": "Software Delivery",
                "budget_bytes": 1000i32,
                "creep_budget_bytes": 2i32,
                "packages": [test_fs.path("package_manifest.json")],
            }]}),
        );
        let fits = verify_budgets_with_tools(
            &mut std::io::sink(),
            PackageSizeCheckArgs {
                blobfs_layout: BlobfsLayout::Compact,
                budgets: test_fs.path("size_budgets.json"),
                blob_sizes: [test_fs.path("blobs.json")].to_vec(),
                gerrit_output: None,
                verbose: false,
                verbose_json_output: None,
            },
            Box::new(FakeToolProvider::default()),
        )
        .unwrap();
        assert!(fits);
    }
}
