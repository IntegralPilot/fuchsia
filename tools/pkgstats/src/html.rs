// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::types::{Capability, FileInfo, OutputSummary, PackageContents, ProtocolToClientMap};
use anyhow::{Context, Result, bail};
use argh::FromArgs;
use camino::Utf8PathBuf;
use fuchsia_sync::Mutex;
use fuchsia_url::Hash;
use handlebars::{
    Handlebars, Helper, HelperResult, Output, RenderContext, RenderErrorReason, handlebars_helper,
};
use rayon::prelude::*;
use serde::Serialize;
use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::sync::LazyLock;
use std::time::Instant;

#[derive(FromArgs)]
#[argh(subcommand, name = "html")]
/// generate an HTML report from output data
pub struct HtmlCommand {
    /// input file generated using "process" command
    #[argh(option)]
    input: Utf8PathBuf,

    /// output directory for HTML, must not exist
    #[argh(option)]
    output: Utf8PathBuf,
}

impl HtmlCommand {
    pub fn execute(self) -> Result<()> {
        if !self.input.is_file() {
            bail!("{:?} is not a file", self.input);
        } else if self.output.exists() {
            bail!("{:?} must not exist, it will be created by this tool", self.output);
        }

        let start = Instant::now();

        let input: OutputSummary = serde_json::from_reader(&mut File::open(&self.input)?)?;

        std::fs::create_dir_all(&self.output)?;

        let mut hb = Handlebars::new();
        hb.set_strict_mode(true);

        handlebars_helper!(capability_str: |capability: Capability| {
            capability.to_string()
        });
        handlebars_helper!(capability_target_list: |capability: Capability, map: ProtocolToClientMap, packages: HashMap<Hash, PackageContents>| {
            let Capability::Protocol(protocol_name) = capability;
            let mut result = Vec::new();
            write!(&mut result, r#"<ul class="capability-targets">"#).unwrap();
            if let Some(hash_to_coverage)  = map.get(&protocol_name) {
                for (hash, component_to_coverage) in hash_to_coverage.iter() {
                    let package_url = packages.get(hash).unwrap().url.to_string();
                    for component in component_to_coverage.iter() {
                        let uri = package_page_url(&hash.to_string());
                        write!(&mut result, "<li><a href='{uri}'>{package_url}#meta/{component}</a></li>").unwrap();
                    }
                }
            }
            write!(&mut result, "</ul>").unwrap();
            String::from_utf8(result).unwrap()
        });

        hb.register_template_string("base", include_str!("../templates/base_template.html.hbs"))?;
        hb.register_template_string("index", include_str!("../templates/index.html.hbs"))?;
        hb.register_template_string("package", include_str!("../templates/package.html.hbs"))?;
        hb.register_template_string("content", include_str!("../templates/content.html.hbs"))?;

        hb.register_helper("package_link", Box::new(package_link_helper));
        hb.register_helper("content_link", Box::new(content_link_helper));
        hb.register_helper("capability_str", Box::new(capability_str));
        hb.register_helper("capability_target_list", Box::new(capability_target_list));

        render_page(
            &hb,
            BaseTemplateArgs {
                page_title: "Home",
                css_path: "style.css",
                root_link: "",
                body_content: &render_index_contents(&hb, &input)?,
            },
            self.output.join("index.html"),
        )?;

        *RENDER_PATH.lock() = "../".to_string();
        std::fs::create_dir_all(self.output.join("packages"))?;
        input
            .packages
            .par_iter()
            .map(|(package_hash, package)| -> Result<()> {
                let data =
                    (package.url.clone(), package, &input.protocol_to_client, &input.packages);
                let body_content = hb.render("package", &data).context("rendering package")?;

                render_page(
                    &hb,
                    BaseTemplateArgs {
                        page_title: &format!("Package: {}", package.url),
                        css_path: "../style.css",
                        root_link: "../",
                        body_content: &body_content,
                    },
                    self.output.join("packages").join(format!(
                        "{}.html",
                        simplify_name_for_linking(&package_hash.to_string())
                    )),
                )?;
                Ok(())
            })
            .collect::<Result<Vec<_>>>()?;
        std::fs::create_dir_all(self.output.join("contents"))?;
        input
            .contents
            .par_iter()
            .map(|item| -> Result<()> {
                let mut files = match item.1 {
                    FileInfo::Elf(elf) => elf
                        .source_file_references
                        .iter()
                        .map(|idx| input.files[idx].source_path.clone())
                        .collect::<Vec<_>>(),
                    _ => vec![],
                };
                files.sort();

                let with_files = (item.0, item.1, files);
                let body_content =
                    hb.render("content", &with_files).context("rendering content")?;
                render_page(
                    &hb,
                    BaseTemplateArgs {
                        page_title: &format!("File content: {}", item.0),
                        css_path: "../style.css",
                        root_link: "../",
                        body_content: &body_content,
                    },
                    self.output
                        .join("contents")
                        .join(format!("{}.html", simplify_name_for_linking(&item.0.to_string()))),
                )?;
                Ok(())
            })
            .collect::<Result<Vec<_>>>()?;

        *RENDER_PATH.lock() = "".to_string();

        File::create(self.output.join("style.css"))
            .context("open style.css")?
            .write_all(include_bytes!("../templates/style.css"))
            .context("write style.css")?;

        println!("Created site at {:?} in {:?}", self.output, Instant::now() - start);

        Ok(())
    }
}

#[derive(Serialize)]
struct BaseTemplateArgs<'a> {
    page_title: &'a str,
    css_path: &'a str,
    root_link: &'a str,
    body_content: &'a str,
}

fn render_page(
    hb: &Handlebars<'_>,
    args: BaseTemplateArgs<'_>,
    out_path: impl AsRef<Path> + std::fmt::Debug,
) -> Result<()> {
    println!("..rendering {:?}", out_path);
    hb.render_to_write("base", &args, File::create(out_path)?)?;

    Ok(())
}

fn render_index_contents(hb: &Handlebars<'_>, output: &OutputSummary) -> Result<String> {
    Ok(hb.render("index", output)?)
}

static RENDER_PATH: LazyLock<Mutex<String>> = LazyLock::new(|| Mutex::new("".to_string()));

fn simplify_name_for_linking(name: &str) -> String {
    let mut ret = String::with_capacity(name.len());

    let replace_chars = ".-/\\:";

    let mut replaced = false;
    for ch in name.chars() {
        if replace_chars.contains(ch) {
            if replaced {
                continue;
            }
            ret.push('_');
            replaced = true;
        } else {
            replaced = false;
            ret.push(ch);
        }
    }

    ret
}

fn package_link_helper(
    h: &Helper<'_>,
    _: &Handlebars<'_>,
    _: &handlebars::Context,
    _: &mut RenderContext<'_, '_>,
    out: &mut dyn Output,
) -> HelperResult {
    let input_name = if let Some(name) = h.param(0) {
        name.value()
            .as_str()
            .ok_or_else(|| RenderErrorReason::Other("Value is not a non-empty string".to_owned()))?
    } else {
        return Err(RenderErrorReason::Other("Helper requires one param".to_owned()).into());
    };
    out.write(&package_page_url(input_name))?;
    Ok(())
}

fn package_page_url(package_hash: impl AsRef<str>) -> String {
    format!(
        "{}packages/{}.html",
        *RENDER_PATH.lock(),
        simplify_name_for_linking(package_hash.as_ref())
    )
}

fn content_link_helper(
    h: &Helper<'_>,
    _: &Handlebars<'_>,
    _: &handlebars::Context,
    _: &mut RenderContext<'_, '_>,
    out: &mut dyn Output,
) -> HelperResult {
    let input_name = if let Some(name) = h.param(0) {
        name.value()
            .as_str()
            .ok_or_else(|| RenderErrorReason::Other("Value is not a non-empty string".to_owned()))?
    } else {
        return Err(RenderErrorReason::Other("Helper requires one param".to_owned()).into());
    };

    out.write(&format!(
        "{}contents/{}.html",
        *RENDER_PATH.lock(),
        simplify_name_for_linking(input_name)
    ))?;
    Ok(())
}
