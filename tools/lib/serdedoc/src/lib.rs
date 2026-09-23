// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod data;
mod toc;

use anyhow::{Context, Result};
use camino::Utf8PathBuf;
use data::{AllData, DataType, DataTypeInner};
use handlebars::{
    Handlebars, Helper, JsonRender, Output, RenderContext, RenderError, RenderErrorReason,
};
use schemars::JsonSchema;
use schemars::r#gen::SchemaSettings;
use std::fs::File;
use std::io::Write;
use toc::TableOfContents;

/// Documentation generator for serde classes.
/// Basic usage:
///   ```
///   let writer = DocWriter::new("/reference/something/".to_string());
///   writer.write::<MyInterface>(output_dir)?;
///   ```
pub struct DocWriter<'a> {
    /// The base url path for all generated links.
    url_path: String,
    handlebars: Handlebars<'a>,
}

impl<'a> DocWriter<'a> {
    pub fn new(url_path: String) -> Self {
        let mut s = DocWriter { url_path, handlebars: Handlebars::new() };

        s.handlebars.register_template_string("header", "").expect("Registering header template");
        s.handlebars
            .register_template_string("toc", include_str!("toc.hbs"))
            .expect("Registering enum template");
        s.handlebars
            .register_template_string("main", include_str!("main.hbs"))
            .expect("Registering main template");
        s.handlebars
            .register_template_string("enum", include_str!("enum.hbs"))
            .expect("Registering enum template");
        s.handlebars
            .register_template_string("primitive", include_str!("primitive.hbs"))
            .expect("Registering primitive template");
        s.handlebars
            .register_template_string("struct", include_str!("struct.hbs"))
            .expect("Registering struct template");

        s.handlebars.register_helper("md_escaped", Box::new(md_escaped));

        s
    }

    pub fn write<T: JsonSchema>(&self, output_dir: Utf8PathBuf) -> Result<()> {
        std::fs::create_dir_all(&output_dir)
            .with_context(|| format!("Creating output dir: {}", output_dir))?;

        let settings = SchemaSettings::default().with(|s| {
            // Remove the definitions path so that we don't have to strip it out later to determine
            // the child type.
            s.definitions_path = "".to_string();
        });
        let generator = settings.into_generator();
        let root_schema = generator.into_root_schema_for::<T>();
        let all_data = AllData::from_root_schema(&self.url_path, &root_schema)?;

        // Generate the table of contents (TOC).
        self.write_table_of_contents(&output_dir, &all_data)?;

        // Generate the main README.
        self.write_main(&output_dir, &all_data)?;

        // Generate all the data type READMEs.
        for data_type in all_data.data_types.values() {
            self.write_data_type(&output_dir, data_type)?;
        }

        Ok(())
    }

    fn write_table_of_contents(&self, output_dir: &Utf8PathBuf, all_data: &AllData) -> Result<()> {
        let output_path = output_dir.join("_toc.yaml");
        let mut output_file =
            File::create(&output_path).with_context(|| format!("Creating {}", output_path))?;

        let content = self.handlebars.render("toc", &all_data).context("Rendering TOC")?;
        output_file
            .write_all(content.as_bytes())
            .with_context(|| format!("Writing {}", output_path))?;

        let toc = TableOfContents::new(all_data);
        toc.write(&mut output_file)?;
        Ok(())
    }

    fn write_main(&self, output_dir: &Utf8PathBuf, all_data: &AllData) -> Result<()> {
        let content = self.handlebars.render("main", &all_data).context("Rendering main")?;
        let output_path = output_dir.join("README.md");
        let mut output_file =
            File::create(&output_path).with_context(|| format!("Creating {}", output_path))?;
        output_file
            .write_all(content.as_bytes())
            .with_context(|| format!("Writing {}", output_path))?;
        Ok(())
    }

    fn write_data_type(&self, output_dir: &Utf8PathBuf, data_type: &DataType) -> Result<()> {
        let template_string = match &data_type.inner {
            DataTypeInner::Primitive(_) => "primitive",
            DataTypeInner::Enum(_) => "enum",
            DataTypeInner::Struct(_) => "struct",
        };
        let content = self
            .handlebars
            .render(template_string, &data_type)
            .with_context(|| format!("Rendering {}", data_type.rust_type))?;

        let output_dir = output_dir.join(&data_type.rust_type);
        std::fs::create_dir_all(&output_dir)
            .with_context(|| format!("Creating output dir: {}", output_dir))?;

        let output_path = output_dir.join("README.md");
        let mut output_file =
            File::create(&output_path).with_context(|| format!("Creating {}", output_path))?;
        output_file
            .write_all(content.as_bytes())
            .with_context(|| format!("Writing {}", output_path))?;
        Ok(())
    }
}

/// Escape special characters that confuse the markdown parser.
pub fn md_escaped(
    h: &Helper<'_>,
    _: &Handlebars<'_>,
    _: &handlebars::Context,
    _: &mut RenderContext<'_, '_>,
    out: &mut dyn Output,
) -> Result<(), RenderError> {
    // get parameter from helper or throw an error
    let param = h.param(0).ok_or_else(|| {
        RenderErrorReason::Other("Param 0 is required for md_escaped helper".to_owned())
    })?;
    let output: String = if let Some(s) = param.value().as_str() {
        md_escaped_impl(s)
    } else {
        md_escaped_impl(&param.value().render())
    };
    out.write(&output)?;
    Ok(())
}

fn md_escaped_impl(input: &str) -> String {
    let mut output = Vec::<String>::new();
    // look line by line
    let mut in_code = false;
    for l in input.lines() {
        if l.contains("```") {
            if in_code {
                let escaped = l
                    .replace("{", "&#123;")
                    .replace("}", "&#125;")
                    .replace("[", "\\[")
                    .replace("```", "</code>");
                output.push(escaped);
                in_code = false;
            } else {
                let escaped = l
                    .replace("{", "&#123;")
                    .replace("}", "&#125;")
                    .replace("[", "\\[")
                    .replace("```", "<code>");
                output.push(escaped);
                in_code = true;
            }
        } else {
            let escaped = l.replace("{", "&#123;").replace("}", "&#125;").replace("[", "\\[");
            output.push(escaped);
        }
    }
    output.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_md_escaped() {
        let testdata = [
            ("", ""),
            ("No special chars", "No special chars"),
            ("{special} chars", "&#123;special&#125; chars"),
            ("sum = x[i] + y[i]", "sum = x\\[i] + y\\[i]"),
            ("`codeword`", "`codeword`"),
            ("```c++\ncode\nblock\n```", "<code>c++\ncode\nblock\n</code>"),
        ];

        for (input, want) in testdata {
            let got = md_escaped_impl(input);
            assert_eq!(got, want);
        }
    }
}
