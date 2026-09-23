// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::common::{ConfigurationBuilder, ConfigurationContext};
use anyhow::Context;
use assembly_config_schema::BuildType;
use assembly_constants::FileEntry;
use fuchsia_url::fuchsia_pkg::AbsoluteComponentUrl;
use fuchsia_url::fuchsia_pkg::AbsolutePackageUrl::{Pinned, Unpinned};
use handlebars::Handlebars;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::Write;

pub(crate) fn add_build_type_config_data(
    package: &str,
    context: &ConfigurationContext<'_>,
    builder: &mut dyn ConfigurationBuilder,
) -> anyhow::Result<()> {
    let build_type = match context.build_type {
        BuildType::Eng => "eng",
        BuildType::UserDebug => "userdebug",
        BuildType::User => "user",
    };
    let gendir = context.get_gendir().context(format!("Getting gendir for {package} subsystem"))?;
    let filepath = gendir.join(format!("{package}_build_type"));
    let mut file = File::create(&filepath).with_context(|| format!("Opening {}", filepath))?;
    file.write_all(build_type.as_bytes()).with_context(|| format!("Writing {}", filepath))?;
    builder
        .package(package)
        .config_data(FileEntry { source: filepath, destination: "build/type".into() })?;
    Ok(())
}

/// Adds a core shard to the product by taking the core shard template found in the assembly
/// resources named `core_shard_template_name` and filling in the COMPONENT_URL and PACKAGE_NAME
/// fields with data found in the `product_component_url`.
pub(crate) fn add_platform_declared_product_provided_component(
    product_component_url: &str,
    core_shard_template_name: &str,
    context: &ConfigurationContext<'_>,
    builder: &mut dyn ConfigurationBuilder,
) -> anyhow::Result<()> {
    // Render the cml template.
    let cml_template = context.get_resource(core_shard_template_name);
    let cml_template = std::fs::read_to_string(cml_template)
        .with_context(|| format!("Reading core shard template: {core_shard_template_name}"))?;
    let cml = render_cml_template(product_component_url, &cml_template)?;

    // Write the new cml.
    let gendir = context.get_gendir().context("Getting gendir")?;
    let cml_path = gendir.join(format!("{core_shard_template_name}.rendered.cml"));
    std::fs::write(&cml_path, cml).context("Writing core shard")?;

    // Add the core shard to the product.
    builder.core_shard(&cml_path);
    Ok(())
}

pub fn render_bootfs_cml_template(
    product_component_url: &str,
    eager_startup: bool,
    template_contents: &str,
) -> anyhow::Result<String> {
    let data = BTreeMap::from([
        ("COMPONENT_URL", product_component_url),
        ("STARTUP", if eager_startup { "eager" } else { "lazy" }),
    ]);

    let handlebars = Handlebars::new();
    handlebars.render_template(template_contents, &data).with_context(|| {
        format!("Rendering a bootfs package cml shard template for: {product_component_url}")
    })
}

fn render_cml_template(
    product_component_url: &str,
    template_contents: &str,
) -> anyhow::Result<String> {
    // Gather the data to render the cml template.
    let url = AbsoluteComponentUrl::parse(product_component_url)
        .with_context(|| format!("Parsing component_url: {product_component_url}"))?;
    let package_name = match url.package_url() {
        Unpinned(unpinned) => unpinned.name(),
        Pinned(pinned) => pinned.as_unpinned().name(),
    }
    .to_string();
    let data =
        BTreeMap::from([("COMPONENT_URL", product_component_url), ("PACKAGE_NAME", &package_name)]);

    let handlebars = Handlebars::new();
    handlebars
        .render_template(template_contents, &data)
        .with_context(|| format!("Rendering the core shard template for: {product_component_url}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_render_cml_template() {
        let cml_template = json!({
            "children": [
                {
                    "name": "something",
                    "url": "{{COMPONENT_URL}}",
                },
            ],
            "offer": [
                {
                    "directory": "config-data",
                    "from": "parent",
                    "to": "#something",
                    "subdir": "{{PACKAGE_NAME}}",
                },
            ],
        });

        let cml = render_cml_template(
            "fuchsia-pkg://fuchsia.com/my-package#meta/my-component.cm",
            &cml_template.to_string(),
        )
        .unwrap();
        let cml: serde_json::Value = serde_json::from_str(&cml).unwrap();

        assert_eq!(
            cml,
            json!({
                 "children": [
                    {
                        "name": "something",
                        "url": "fuchsia-pkg://fuchsia.com/my-package#meta/my-component.cm",
                    },
                ],
                "offer": [
                    {
                        "directory": "config-data",
                        "from": "parent",
                        "to": "#something",
                        "subdir": "my-package",
                    },
                ],
            })
        );
    }

    #[test]
    fn test_render_boot_cml_template() {
        let cml_template = json!({
            "children": [
                {
                    "name": "something",
                    "url": "{{COMPONENT_URL}}",
                    "startup": "{{STARTUP}}",
                },
            ],
            "offer": [
                {
                    "directory": "config-data",
                    "from": "parent",
                    "to": "#something",
                },
            ],
        });

        let cml = render_bootfs_cml_template(
            "fuchsia-boot:///my-package/path#meta/my-component.cm",
            true,
            &cml_template.to_string(),
        )
        .unwrap();
        let cml: serde_json::Value = serde_json::from_str(&cml).unwrap();

        assert_eq!(
            cml,
            json!({
                 "children": [
                    {
                        "name": "something",
                        "url": "fuchsia-boot:///my-package/path#meta/my-component.cm",
                        "startup": "eager",
                    },
                ],
                "offer": [
                    {
                        "directory": "config-data",
                        "from": "parent",
                        "to": "#something",
                    },
                ],
            })
        );

        let cml = render_bootfs_cml_template(
            "fuchsia-boot:///my-package/path#meta/my-component.cm",
            false,
            &cml_template.to_string(),
        )
        .unwrap();
        let cml: serde_json::Value = serde_json::from_str(&cml).unwrap();

        assert_eq!(
            cml,
            json!({
                 "children": [
                    {
                        "name": "something",
                        "url": "fuchsia-boot:///my-package/path#meta/my-component.cm",
                        "startup": "lazy",
                    },
                ],
                "offer": [
                    {
                        "directory": "config-data",
                        "from": "parent",
                        "to": "#something",
                    },
                ],
            })
        );
    }
}
