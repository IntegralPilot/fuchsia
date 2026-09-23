// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result, bail};
use assembly_tool::Tool;
use camino::{Utf8Path, Utf8PathBuf};

/// Builder for compiling a component out of cml shards.
pub struct ComponentBuilder {
    /// The name of the component.
    name: String,
    /// A list of component manifest shards to be merged into the final
    /// component manifest.  This keeps the order that shards are provided to
    /// it.
    manifest_shards: Vec<Utf8PathBuf>,
    /// A list of CMC features to enable during compilation.
    cmc_features: Vec<String>,
}

impl ComponentBuilder {
    /// Construct a new ComponentBuilder that uses the cmc |tool|.
    pub fn new(name: impl Into<String>) -> Self {
        ComponentBuilder {
            name: name.into(),
            manifest_shards: Vec::default(),
            cmc_features: Vec::default(),
        }
    }

    /// Add a CML shard or the primary CML file for this component.
    pub fn add_shard(&mut self, path: impl AsRef<Utf8Path>) -> Result<&mut Self> {
        let path = path.as_ref().to_path_buf();

        // Validate that the shard hasn't be previously added.
        if self.manifest_shards.contains(&path) {
            bail!("Component shard path {} already added", path);
        }

        // Add the shard.
        self.manifest_shards.push(path);
        Ok(self)
    }

    /// Adds a CMC feature to enable during compilation.
    pub fn add_cmc_feature(&mut self, feature: impl Into<String>) -> &mut Self {
        self.cmc_features.push(feature.into());
        self
    }

    /// Build the component.
    pub fn build(
        self,
        outdir: impl AsRef<Utf8Path>,
        component_includes_dir: &Option<Utf8PathBuf>,
        cmc_tool: &dyn Tool,
    ) -> Result<Utf8PathBuf> {
        let Self { name, manifest_shards, cmc_features } = self;
        // Write all generated files in a subdir with the name of the package.
        let outdir = outdir.as_ref().join(&name);
        let cmlfile = outdir.join(format!("{}.cml", name));
        let mut args = vec!["merge".to_owned(), "--output".to_owned(), cmlfile.to_string()];

        args.extend(manifest_shards.iter().map(Utf8PathBuf::to_string));

        cmc_tool
            .run(&args)
            .with_context(|| format!("Failed to run cmc merge with shards {args:?}"))?;

        let cmfile = outdir.join(format!("{}.cm", name));

        let mut args = vec![
            "compile".into(),
            "--features=allow_long_names".into(),
            "--config-package-path".into(),
            format!("meta/{}.cvf", &name),
            "-o".into(),
            cmfile.to_string(),
            cmlfile.to_string(),
        ];
        for feature in cmc_features {
            args.push(format!("--features={feature}"));
        }
        if let Some(component_includes_dir) = component_includes_dir {
            args.push("--includeroot".into());
            args.push(component_includes_dir.to_string());
            args.push("--includepath".into());
            args.push(component_includes_dir.to_string());
        }

        cmc_tool
            .run(&args)
            .with_context(|| format!("Failed to run cmc compile with args {args:?}"))?;

        Ok(cmfile)
    }
}

#[cfg(test)]
mod tests {
    use crate::ComponentBuilder;
    use assembly_tool::testing::FakeToolProvider;
    use assembly_tool::{ToolCommandLog, ToolProvider};
    use camino::Utf8Path;
    use serde_json::json;
    use tempfile::TempDir;

    #[test]
    fn add_shard_with_duplicates_returns_err() {
        let mut builder = ComponentBuilder::new("foo");
        builder.add_shard("foobar").unwrap();

        let result = builder.add_shard("foobar");

        assert!(result.is_err());
    }

    #[test]
    fn build_with_shards_compiles_component() {
        let tmpdir = TempDir::new().unwrap();
        let outdir = Utf8Path::from_path(tmpdir.path()).unwrap();
        let shard_path_1 = outdir.join("shard1.cml");
        let shard_path_2 = outdir.join("shard2.cml");
        let shard_path_3 = outdir.join("shard3.cml");
        let tools = FakeToolProvider::default();
        let mut builder = ComponentBuilder::new("test");
        builder.add_shard(&shard_path_1).unwrap();
        builder.add_shard(&shard_path_2).unwrap().add_shard(&shard_path_3).unwrap();
        let expected_commands: ToolCommandLog = serde_json::from_value(json!({
            "commands": [
                {
                    "tool": "./host_x64/cmc",
                    "args": [
                        "merge",
                        "--output",
                        outdir.join("test").join("test.cml").to_string(),
                        shard_path_1.to_string(),
                        shard_path_2.to_string(),
                        shard_path_3.to_string(),
                    ]
                },
                {
                    "tool": "./host_x64/cmc",
                    "args": [
                        "compile",
                        "--features=allow_long_names",
                        "--config-package-path",
                        "meta/test.cvf",
                        "-o",
                        outdir.join("test").join("test.cm").to_string(),
                        outdir.join("test").join("test.cml").to_string(),
                    ]
                }
            ]
        }))
        .unwrap();

        let result = builder.build(outdir, &None, tools.get_tool("cmc").unwrap().as_ref());

        assert!(result.is_ok());
        assert_eq!(&expected_commands, tools.log());
    }
}
