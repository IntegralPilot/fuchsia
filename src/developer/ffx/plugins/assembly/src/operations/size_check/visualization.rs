// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::operations::size_check::breakdown::SizeBreakdown;
use anyhow::{Context, Result};
use camino::Utf8PathBuf;
use serde::Serialize;
use std::fs;

// The tree structure that needs to be generated for the HTML visualization.
// See "tree_data" in template/D3BlobTreeMap.js.
#[derive(Serialize)]
struct VisualizationRootNode {
    #[serde(rename = "n")]
    name: String,
    children: Vec<VisualizationPackageNode>,
    #[serde(rename = "k")]
    kind: String,
}

#[derive(Serialize)]
struct VisualizationPackageNode {
    #[serde(rename = "n")]
    name: String,
    children: Vec<VisualizationBlobNode>,
    #[serde(rename = "k")]
    kind: String,
}

#[derive(Serialize)]
struct VisualizationBlobNode {
    #[serde(rename = "n")]
    name: String,
    #[serde(rename = "k")]
    kind: String,
    #[serde(rename = "t")]
    uniqueness: String,
    #[serde(rename = "value")]
    proportional_size: u64,
    #[serde(rename = "originalSize")]
    original_size: u64,
    #[serde(rename = "c")]
    share_count: u64,
}

pub fn generate_visualization(
    visualization_dir: &Utf8PathBuf,
    breakdown: &SizeBreakdown,
) -> Result<()> {
    fs::create_dir_all(visualization_dir.join("d3_v3"))
        .context("creating d3_v3 directory for visualization")?;
    fs::write(
        visualization_dir.join("d3_v3").join("LICENSE"),
        include_bytes!("../../../../../../../../scripts/third_party/d3_v3/LICENSE"),
    )
    .context("creating LICENSE file for visualization")?;
    fs::write(
        visualization_dir.join("d3_v3").join("d3.js"),
        include_bytes!("../../../../../../../../scripts/third_party/d3_v3/d3.js"),
    )
    .context("creating d3.js file for visualization")?;
    fs::write(visualization_dir.join("D3BlobTreeMap.js"), include_bytes!("D3BlobTreeMap.js"))
        .context("creating D3BlobTreeMap.js file for visualization")?;
    fs::write(visualization_dir.join("index.html"), include_bytes!("index.html"))
        .context("creating index.html file for visualization")?;
    fs::write(
        visualization_dir.join("data.js"),
        format!(
            "var tree_data={}",
            serde_json::to_string(&generate_visualization_tree(breakdown))?
        ),
    )
    .context("creating data.js for visualization")
}

#[allow(clippy::ptr_arg)]
fn generate_visualization_tree(breakdown: &SizeBreakdown) -> VisualizationRootNode {
    VisualizationRootNode {
        name: "packages".to_string(),
        kind: "p".to_string(),
        children: breakdown
            .packages
            .iter()
            .map(|(name, package)| VisualizationPackageNode {
                name: name.clone(),
                kind: "p".to_string(),
                children: package
                    .blobs
                    .iter()
                    .map(|(path, blob)| VisualizationBlobNode {
                        name: path.clone(),
                        kind: "s".to_string(),
                        uniqueness: if blob.references.len() == 1 {
                            "unique".to_string()
                        } else {
                            "shared".to_string()
                        },
                        proportional_size: blob.psize,
                        original_size: blob.size,
                        share_count: blob.references.len().try_into().unwrap(),
                    })
                    .collect::<Vec<_>>(),
            })
            .collect::<Vec<_>>(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operations::size_check::breakdown::{
        BlobBreakdown, PackageBreakdown, PackageReference,
    };
    use pretty_assertions::assert_eq;
    use serde_json::json;
    use std::collections::BTreeMap;

    #[test]
    fn verify_visualization_tree_test() -> Result<()> {
        let breakdown = SizeBreakdown {
            packages: BTreeMap::from([
                ("package1".to_string(), PackageBreakdown {
                    name: "package1".to_string(),
                    blobs: BTreeMap::from([
                        ("bin/defg".to_string(), BlobBreakdown {
                            hash: "7ddff816740d5803358dd4478d8437585e8d5c984b4361817d891807a16ff581".to_string(),
                            size: 20,
                            psize: 20,
                            references: vec![
                                PackageReference {
                                    path: "bin/defg".to_string(),
                                    name: "package1".to_string(),
                                },
                            ],
                        }),
                        ("lib/ghij".to_string(), BlobBreakdown {
                            hash: "8cb3466c6e66592c8decaeaa3e399652fbe71dad5c3df1a5e919743a33815568".to_string(),
                            size: 60,
                            psize: 30,
                            references: vec![
                                PackageReference {
                                    path: "lib/ghij".to_string(),
                                    name: "package1".to_string(),
                                },
                                PackageReference {
                                    path: "lib/ghij".to_string(),
                                    name: "package2".to_string(),
                                },
                            ],
                        }),
                    ]),
                }),
                ("package2".to_string(), PackageBreakdown {
                    name: "package2".to_string(),
                    blobs: BTreeMap::from([
                        ("lib/ghij".to_string(), BlobBreakdown {
                            hash: "8cb3466c6e66592c8decaeaa3e399652fbe71dad5c3df1a5e919743a33815568".to_string(),
                            size: 60,
                            psize: 30,
                            references: vec![
                                PackageReference {
                                    path: "lib/ghij".to_string(),
                                    name: "package1".to_string(),
                                },
                                PackageReference {
                                    path: "lib/ghij".to_string(),
                                    name: "package2".to_string(),
                                },
                            ],
                        }),
                        ("abcd/".to_string(), BlobBreakdown {
                            hash: "eabdb84d26416c1821fd8972e0d835eedaf7468e5a9ebe01e5944462411aec71".to_string(),
                            size: 40,
                            psize: 40,
                            references: vec![
                                PackageReference {
                                    path: "abcd/".to_string(),
                                    name: "package2".to_string(),
                                },
                            ],
                        }),
                    ]),
                }),
            ]),
            blobs: BTreeMap::new(),
        };
        let visualization_tree = generate_visualization_tree(&breakdown);
        assert_eq!(
            serde_json::to_value(visualization_tree)?,
            json!(
                {
                    "n": "packages",
                    "children": [
                        {
                            "n": "package1",
                            "children": [
                                {
                                    "n": "bin/defg",
                                    "k": "s",
                                    "t": "unique",
                                    "value": 20,
                                    "originalSize": 20,
                                    "c": 1
                                },
                                {
                                    "n": "lib/ghij",
                                    "k": "s",
                                    "t": "shared",
                                    "value": 30,
                                    "originalSize": 60,
                                    "c": 2
                                }
                            ],
                            "k": "p"
                        },
                        {
                            "n": "package2",
                            "children": [
                                {
                                    "n": "abcd/",
                                    "k": "s",
                                    "t": "unique",
                                    "value": 40,
                                    "originalSize": 40,
                                    "c": 1
                                },
                                {
                                    "n": "lib/ghij",
                                    "k": "s",
                                    "t": "shared",
                                    "value": 30,
                                    "originalSize": 60,
                                    "c": 2
                                }
                            ],
                            "k": "p"
                        }
                    ],
                    "k": "p"
                }
            )
        );
        Ok(())
    }
}
