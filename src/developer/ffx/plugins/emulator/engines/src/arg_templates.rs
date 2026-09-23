// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Handlebars helper functions for working with an EmulatorConfiguration.

use std::collections::HashMap;

use anyhow::{Context as anyhow_context, Result};
use emulator_instance::{DataUnits, DiskImage, EmulatorConfiguration, FlagData};
use handlebars::{
    Context, Handlebars, Helper, HelperDef, HelperResult, JsonRender, Output, RenderContext,
    RenderErrorReason,
};

//  Actual path is //src/developer/ffx/plugins/emulator/templates/emulator_flags.json.template
const DEFAULT_FLAGS_TEMPLATE_STR: &str = include_str!("../templates/emulator_flags.json.template");
//  Actual path is //src/developer/ffx/plugins/emulator/templates/efi_flags.json.template
const EFI_FLAGS_TEMPLATE_STR: &str = include_str!("../templates/efi_flags.json.template");

#[derive(Clone, Copy)]
pub struct EqHelper {}

/// This Handlebars helper performs equality comparison of two parameters, and
/// returns a value which can be interpreted as "truthy" or not by the #if
/// built-in.
///
/// Example:
///
///   {{#if (eq var1 var2)}}
///       // Stuff you want if they match.
///   {{else}}
///       // Stuff you want if they don't.
///   {{/if}}
///
/// You can also embed literal values, rather than parameters, like this:
///
///   {{#if (eq "string" 42)}}
///       // String and Number don't match, but you can compare them.
///   {{/if}}
///   {{#if (eq "literal value" param.in.a.structure)}}
///       // Either parameter can be a literal or a variable.
///   {{/if}}
///
impl HelperDef for EqHelper {
    fn call<'reg: 'rc, 'rc>(
        &self,
        h: &Helper<'rc>,
        _: &'reg Handlebars<'reg>,
        _: &'rc Context,
        _: &mut RenderContext<'reg, 'rc>,
        out: &mut dyn Output,
    ) -> HelperResult {
        let first = h.param(0).ok_or_else(|| {
            RenderErrorReason::Other("First param not found for helper \"eq\"".to_owned())
        })?;
        let second = h.param(1).ok_or_else(|| {
            RenderErrorReason::Other("Second param not found for helper \"eq\"".to_owned())
        })?;

        // Compare the value of the two parameters, and writes "true" (which is truthy) to "out" if
        // they evaluate as equal. In the context of an "#if" helper, this causes the "true" branch
        // to be rendered if and only if the two are equal, and the "else" branch otherwise.
        if first.value() == second.value() {
            out.write("true")?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
pub struct UnitAbbreviationHelper {}

/// This Handlebars helper is used for substitution of a DataUnits variable,
/// only instead of printing the normal serialized form, it outputs the
/// abbreviated form.
///
/// Example:
///
///     let megs = DataUnits::Megabytes;
///
///   Template:
///
///     "{{megs}} abbreviates to {{#abbr megs}}."  {{! output: "megabytes abbreviates to M." }}
///
/// It also works if the template includes the serialized form of a DataUnits value, for example:
///
///     "Short form is {{ua "megabytes"}}."  {{! output: "Short form is M." }}
///
/// If the template wraps a variable that is not a DataUnits type with this helper, the helper
/// will render the full serialized value as normal.
///
///     let os = OperatingSystem::Linux;
///     let name = String::From("something");
///
///     "{{ua name}} and {{ua os}} aren't DataUnits."
///         {{! output: "something and linux aren't DataUnits."}}
///
impl HelperDef for UnitAbbreviationHelper {
    fn call<'reg: 'rc, 'rc>(
        &self,
        h: &Helper<'rc>,
        _: &'reg Handlebars<'reg>,
        _: &'rc Context,
        _: &mut RenderContext<'reg, 'rc>,
        out: &mut dyn Output,
    ) -> HelperResult {
        // Convert the serde_json::Value into a DataUnits.
        let param = h.param(0).ok_or_else(|| {
            RenderErrorReason::Other(format!("Parameter 0 is missing in {:?}", h))
        })?;

        match serde_json::from_value::<DataUnits>(param.value().clone()) {
            Ok(units) => out.write(&json_escape(units.abbreviate()))?,
            Err(_) => out.write(&json_escape(param.value().render().as_ref()))?,
        };
        Ok(())
    }
}

/// This Handlebars helper extracts a path from a DiskImage.
///
/// Example:
///
///     let path = DiskImage::Fxfs("foo.blk");
///
///   Template:
///
///     "{{path}} contains {{#diskimage path}}."  {{! output: "[object] contains foo.blk." }}
#[derive(Clone, Copy)]
pub struct DiskImageHelper {}

impl HelperDef for DiskImageHelper {
    fn call<'reg: 'rc, 'rc>(
        &self,
        h: &Helper<'rc>,
        _: &'reg Handlebars<'reg>,
        _: &'rc Context,
        _: &mut RenderContext<'reg, 'rc>,
        out: &mut dyn Output,
    ) -> HelperResult {
        // Convert the serde_json::Value into a DiskImage.
        let param = h.param(0).ok_or_else(|| {
            RenderErrorReason::Other(format!("Parameter 0 is missing in {:?}", h))
        })?;

        match serde_json::from_value::<DiskImage>(param.value().clone()) {
            Ok(path) => {
                let path_str = path
                    .to_str()
                    .ok_or_else(|| RenderErrorReason::Other(format!("Invalid path {:?}", path)))?;
                out.write(&json_escape(path_str))?;
            }
            Err(_) => out.write(&json_escape(param.value().render().as_ref()))?,
        };
        Ok(())
    }
}

#[derive(Clone, Copy)]
pub struct EnvironmentHelper {}

/// This Handlebars helper is used for substitution of an environment variable.
/// Rather than pulling the value from a data structure, it will call
/// std::env::var(value), where value is the parameter to the helper. If the
/// specified environment variable is unset, the resulting output will be empty.
///
/// Example:
///
///     $ export KEY=value
///
///   Template:
///
///     "key = {{env "KEY"}}."  {{! output: "key = value" }}
///
/// It also works if you provide the name of a variable that contains the key:
///
///     struct Data {
///         variable: String,
///     }
///
///     let data = Data { variable: "KEY" };
///
///   Template:
///
///     "key = {{env variable}}"  {{! output: "key = value" }}
///
impl HelperDef for EnvironmentHelper {
    fn call<'reg: 'rc, 'rc>(
        &self,
        h: &Helper<'rc>,
        _: &'reg Handlebars<'reg>,
        _: &'rc Context,
        _: &mut RenderContext<'reg, 'rc>,
        out: &mut dyn Output,
    ) -> HelperResult {
        // Get the key specified in param(0) and retrieve the value from std::env.
        let param = h.param(0).ok_or_else(|| {
            RenderErrorReason::Other(format!("Parameter 0 is missing in {:?}", h))
        })?;
        if let Some(key) = param.value().as_str() {
            match std::env::var(key) {
                Ok(val) => out.write(&json_escape(&val))?,
                Err(_) => (), // An Err means the variable isn't set or the key is invalid.
            }
        }
        Ok(())
    }
}

pub fn process_flag_template(emu_config: &EmulatorConfiguration) -> Result<FlagData> {
    let template_text = if let Some(template_path) = &emu_config.runtime.template {
        std::fs::read_to_string(template_path).context(format!(
            "couldn't locate template file from path {:?}",
            emu_config.runtime.template
        ))?
    } else {
        if emu_config.guest.is_efi() || emu_config.guest.is_gpt {
            EFI_FLAGS_TEMPLATE_STR.to_string()
        } else {
            DEFAULT_FLAGS_TEMPLATE_STR.to_string()
        }
    };
    let flag_data = process_flag_template_inner(&template_text, emu_config)?;
    dedupe_kernel_args(flag_data, &emu_config.runtime.addl_kernel_args)
}

/// It is possible for kernel args to be passed in on the command line when starting the emulator.
/// Some of these args may override/duplicate the kernel args included in there template file.
/// This function prioritizes the command line kernel args, and then adds any from the template
/// that are not present.
pub(crate) fn dedupe_kernel_args(
    data: FlagData,
    addl_kernel_args: &Vec<String>,
) -> Result<FlagData> {
    let mut map: HashMap<&str, &String> = HashMap::new();

    let items: Vec<_> = addl_kernel_args
        .iter()
        .filter_map(|a| {
            if let Some(key) = a.split("=").next() {
                Some((key, a))
            } else {
                log::info!("kernel arg {a} does not contain an =, so skipping.");
                None
            }
        })
        .collect();
    for (k, v) in items {
        map.entry(k).or_insert(v);
    }

    // Now add any args from the Flag Data that are not in the map already.
    let kernel_args = data.kernel_args.iter().filter_map(|a| {
        if let Some(key) = a.split("=").next() {
            Some((key, a))
        } else {
            log::info!("Invalid kernel arg entry: {a}. Kernel args are of the form name=value.");
            None
        }
    });
    for (k, v) in kernel_args {
        map.entry(k).or_insert(v);
    }

    let mut updated = FlagData {
        args: data.args,
        envs: data.envs,
        features: data.features,
        kernel_args: map.values().map(|v| v.to_string()).collect(),
        options: data.options,
    };

    // Sort the kernel args to make the order stable.
    updated.kernel_args.sort();
    Ok(updated)
}

pub(crate) fn process_flags_from_str(
    text: &str,
    emu_config: &EmulatorConfiguration,
) -> Result<FlagData> {
    process_flag_template_inner(text, emu_config)
}

/// Escapes string content for safe interpolation into JSON flag templates.
///
/// Emulator flag templates are rendered as raw JSON text using Handlebars before
/// being parsed by `serde_json`. Because variables are interpolated directly inside
/// JSON string quotes (e.g., `"{{guest.kernel_image}}"`), string values must have
/// quotes, backslashes, and control characters escaped to prevent JSON injection
/// into the generated argument lists.
///
/// This escaping is specific to the emulator flag template rendering workflow,
/// where Handlebars outputs raw JSON text rather than structured data structures.
pub(crate) fn json_escape(s: &str) -> String {
    let json_str = serde_json::to_string(s).unwrap_or_else(|_| s.to_string());
    json_str
        .strip_prefix('"')
        .and_then(|str_slice| str_slice.strip_suffix('"'))
        .unwrap_or(&json_str)
        .to_string()
}

fn process_flag_template_inner(
    template_text: &str,
    emu_config: &EmulatorConfiguration,
) -> Result<FlagData> {
    // This performs all the variable substitution and condition resolution.
    let mut handlebars = Handlebars::new();
    handlebars.set_strict_mode(true);
    handlebars.register_escape_fn(json_escape);
    handlebars.register_helper("env", Box::new(EnvironmentHelper {}));
    handlebars.register_helper("eq", Box::new(EqHelper {}));
    handlebars.register_helper("ua", Box::new(UnitAbbreviationHelper {}));
    handlebars.register_helper("di", Box::new(DiskImageHelper {}));
    let json = handlebars.render_template(&template_text, &emu_config)?;

    log::trace!("Processed template ====\n {json}\n ======");
    // Deserialize and return the flags from the template.
    let flags = serde_json::from_str(&json)?;
    Ok(flags)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use emulator_instance::{DeviceConfig, GuestConfig, HostConfig, NetworkingMode, VirtualCpu};
    use ffx_emulator_config::AudioModel;
    use serde::Serialize;

    #[derive(Serialize)]
    struct StringStruct {
        pub units: String,
    }

    #[derive(Serialize)]
    struct UnitsStruct {
        pub units: DataUnits,
    }

    #[derive(Serialize)]
    struct DiskImageStruct {
        pub disk_image: DiskImage,
    }

    #[fuchsia::test]
    async fn test_env_helper() {
        unsafe { std::env::set_var("MY_VARIABLE", "my_value") };

        let var_template = "{{env units}}";
        let literal_template = r#"{{env "MY_VARIABLE"}}"#;
        let string_struct = StringStruct { units: "MY_VARIABLE".to_string() };

        let mut handlebars = Handlebars::new();
        handlebars.set_strict_mode(true);
        handlebars.register_helper("env", Box::new(EnvironmentHelper {}));

        let json = handlebars.render_template(&var_template, &string_struct);
        assert!(json.is_ok());
        assert_eq!(json.unwrap(), "my_value");

        let json = handlebars.render_template(&literal_template, &string_struct);
        assert!(json.is_ok());
        assert_eq!(json.unwrap(), "my_value");
    }

    #[fuchsia::test]
    async fn test_ua_helper() {
        let template = "{{ua units}}";
        let units_struct = UnitsStruct { units: DataUnits::Gigabytes };
        let string_struct = StringStruct { units: "Gigabytes".to_string() };

        let mut handlebars = Handlebars::new();
        handlebars.set_strict_mode(true);
        handlebars.register_helper("ua", Box::new(UnitAbbreviationHelper {}));

        let json = handlebars.render_template(&template, &string_struct);
        assert!(json.is_ok());
        assert_eq!(json.unwrap(), "Gigabytes");

        let json = handlebars.render_template(&template, &units_struct);
        assert!(json.is_ok());
        assert_eq!(json.unwrap(), "G");
    }

    #[fuchsia::test]
    async fn test_eq_helper() {
        let template = r#"{{eq units "Gigabytes"}}"#;
        let if_template = r#"{{#if (eq units "Gigabytes")}}yes{{else}}no{{/if}}"#;
        let mut string_struct = StringStruct { units: "Gigabytes".to_string() };

        let mut handlebars = Handlebars::new();
        handlebars.set_strict_mode(true);
        handlebars.register_helper("eq", Box::new(EqHelper {}));

        let json = handlebars.render_template(&template, &string_struct);
        assert!(json.is_ok());
        assert_eq!(json.unwrap(), "true");

        let json = handlebars.render_template(&if_template, &string_struct);
        assert!(json.is_ok());
        assert_eq!(json.unwrap(), "yes");

        string_struct.units = "Something Else".to_string();

        let json = handlebars.render_template(&template, &string_struct);
        assert!(json.is_ok());
        assert_eq!(json.unwrap(), "");

        let json = handlebars.render_template(&if_template, &string_struct);
        assert!(json.is_ok());
        assert_eq!(json.unwrap(), "no");
    }

    #[fuchsia::test]
    async fn test_disk_image_helper() {
        let template = r#"{{di disk_image}}"#;

        let mut handlebars = Handlebars::new();
        handlebars.set_strict_mode(true);
        handlebars.register_helper("di", Box::new(DiskImageHelper {}));

        let disk_image = DiskImageStruct { disk_image: DiskImage::Fxfs("fxfs.sparse.blk".into()) };
        let json = handlebars.render_template(&template, &disk_image);
        assert!(json.is_ok());
        assert_eq!(json.unwrap(), "fxfs.sparse.blk");

        let disk_image = DiskImageStruct { disk_image: DiskImage::Fvm("fvm.blk".into()) };
        let json = handlebars.render_template(&template, &disk_image);
        assert!(json.is_ok());
        assert_eq!(json.unwrap(), "fvm.blk");
    }

    #[fuchsia::test]
    async fn test_empty_template() -> Result<()> {
        // Fails because empty templates can't be rendered.
        let empty_template = "";
        let emu_config = EmulatorConfiguration::default();

        let flags = process_flag_template_inner(empty_template, &emu_config);
        assert!(flags.is_err());

        Ok(())
    }

    #[fuchsia::test]
    async fn test_escaping() -> Result<()> {
        // Make sure equals characters don't get escaped.
        let addl_kernel_args = r#"
        {
            "args": [],
            "envs": {},
            "features": [],
            "kernel_args": [
                "a=b"
                {{#each runtime.addl_kernel_args}}
                    ,"{{this}}"
                {{/each}}
            ],
            "options": []
        }"#;
        let mut emu_config = EmulatorConfiguration::default();
        emu_config.runtime.addl_kernel_args = vec!["b=c".to_string()];

        let flags = process_flag_template_inner(addl_kernel_args, &emu_config);
        assert!(flags.is_ok(), "{:?}", flags);
        // Kernel args from the command line should be after any others, so they take precedence.
        // Neither should have the text modified due to character escaping.
        assert_eq!(flags.as_ref().unwrap().kernel_args.len(), 2);
        assert_eq!(flags.as_ref().unwrap().kernel_args[0], "a=b".to_string());
        assert_eq!(flags.as_ref().unwrap().kernel_args[1], "b=c".to_string());

        Ok(())
    }

    #[fuchsia::test]
    async fn test_empty_vector_template() -> Result<()> {
        // Succeeds without any content in the vectors.
        let empty_vectors_template = r#"
        {
            "args": [],
            "envs": {},
            "features": [],
            "kernel_args": [],
            "options": []
        }"#;
        let emu_config = EmulatorConfiguration::default();

        let flags = process_flag_template_inner(empty_vectors_template, &emu_config);
        assert!(flags.is_ok(), "{:?}", flags);
        assert_eq!(flags.as_ref().unwrap().args.len(), 0);
        assert_eq!(flags.as_ref().unwrap().envs.len(), 0);
        assert_eq!(flags.as_ref().unwrap().features.len(), 0);
        assert_eq!(flags.as_ref().unwrap().kernel_args.len(), 0);
        assert_eq!(flags.as_ref().unwrap().options.len(), 0);

        Ok(())
    }

    #[fuchsia::test]
    async fn test_invalid_template() -> Result<()> {
        // Fails because it doesn't have all the required fields.
        let invalid_template = r#"
        {
            "args": [],
            "envs": {},
            "features": [],
            "kernel_args": []
            {{! It's missing the options field }}
        }"#;
        let emu_config = EmulatorConfiguration::default();

        let flags = process_flag_template_inner(invalid_template, &emu_config);
        assert!(flags.is_err());

        Ok(())
    }

    #[fuchsia::test]
    async fn test_ok_template() -> Result<()> {
        // Succeeds with a single string "value" in the options field, and one in the envs map.
        let ok_template = r#"
        {
            "args": [],
            "envs": {"key": "value"},
            "features": [],
            "kernel_args": [],
            "options": ["value"]
        }"#;
        let emu_config = EmulatorConfiguration::default();

        let flags = process_flag_template_inner(ok_template, &emu_config);
        assert!(flags.is_ok(), "{:?}", flags);
        assert_eq!(flags.as_ref().unwrap().args.len(), 0);
        assert_eq!(flags.as_ref().unwrap().envs.len(), 1);
        assert_eq!(flags.as_ref().unwrap().features.len(), 0);
        assert_eq!(flags.as_ref().unwrap().kernel_args.len(), 0);
        assert_eq!(flags.as_ref().unwrap().options.len(), 1);
        assert!(flags.as_ref().unwrap().envs.contains_key(&"key".to_string()));
        assert_eq!(flags.as_ref().unwrap().envs.get("key").unwrap(), &"value".to_string());
        assert!(flags.as_ref().unwrap().options.contains(&"value".to_string()));

        Ok(())
    }

    #[fuchsia::test]
    async fn test_substitution_template() -> Result<()> {
        // Succeeds with the default value of AudioModel in the args field.
        let substitution_template = r#"
        {
            "args": ["{{device.audio.model}}"],
            "envs": {},
            "features": [],
            "kernel_args": [],
            "options": []
        }"#;
        let mut emu_config = EmulatorConfiguration::default();

        let flags = process_flag_template_inner(substitution_template, &emu_config);
        assert!(flags.is_ok(), "{:?}", flags);
        assert_eq!(flags.as_ref().unwrap().args.len(), 1);
        assert_eq!(flags.as_ref().unwrap().envs.len(), 0);
        assert_eq!(flags.as_ref().unwrap().features.len(), 0);
        assert_eq!(flags.as_ref().unwrap().kernel_args.len(), 0);
        assert_eq!(flags.as_ref().unwrap().options.len(), 0);
        assert!(flags.as_ref().unwrap().args.contains(&format!("{}", AudioModel::default())));

        emu_config.device.audio.model = AudioModel::Hda;

        let flags = process_flag_template_inner(substitution_template, &emu_config);
        assert!(flags.is_ok(), "{:?}", flags);
        assert_eq!(flags.as_ref().unwrap().args.len(), 1);
        assert_eq!(flags.as_ref().unwrap().envs.len(), 0);
        assert_eq!(flags.as_ref().unwrap().features.len(), 0);
        assert_eq!(flags.as_ref().unwrap().kernel_args.len(), 0);
        assert_eq!(flags.as_ref().unwrap().options.len(), 0);
        assert!(flags.as_ref().unwrap().args.contains(&format!("{}", AudioModel::Hda)));

        Ok(())
    }

    #[fuchsia::test]
    async fn test_conditional_template() -> Result<()> {
        // Succeeds. If headless is set, features contains the "ok" value.
        // If headless is not set, features contains the "none" value.
        let conditional_template = r#"
        {
            "args": [],
            "envs": {},
            "features": [{{#if runtime.headless}}"ok"{{else}}"none"{{/if}}],
            "kernel_args": [],
            "options": []
        }"#;
        let mut emu_config = EmulatorConfiguration::default();

        emu_config.runtime.headless = false;

        let flags = process_flag_template_inner(conditional_template, &emu_config);
        assert!(flags.is_ok(), "{:?}", flags);
        assert_eq!(flags.as_ref().unwrap().args.len(), 0);
        assert_eq!(flags.as_ref().unwrap().envs.len(), 0);
        assert_eq!(flags.as_ref().unwrap().features.len(), 1);
        assert_eq!(flags.as_ref().unwrap().kernel_args.len(), 0);
        assert_eq!(flags.as_ref().unwrap().options.len(), 0);
        assert!(flags.as_ref().unwrap().features.contains(&"none".to_string()));

        emu_config.runtime.headless = true;

        let flags = process_flag_template_inner(conditional_template, &emu_config);
        assert!(flags.is_ok(), "{:?}", flags);
        assert_eq!(flags.as_ref().unwrap().args.len(), 0);
        assert_eq!(flags.as_ref().unwrap().envs.len(), 0);
        assert_eq!(flags.as_ref().unwrap().features.len(), 1);
        assert_eq!(flags.as_ref().unwrap().kernel_args.len(), 0);
        assert_eq!(flags.as_ref().unwrap().options.len(), 0);
        assert!(flags.as_ref().unwrap().features.contains(&"ok".to_string()));

        Ok(())
    }

    #[fuchsia::test]
    async fn test_ua_template() -> Result<()> {
        // Succeeds, with the abbreviated form of the units field in the kernel_args field.
        // The default value of units is Bytes, which has an empty abbreviation.
        // The DataUnits::Megabytes value has the abbreviated form "M".
        let template = r#"
        {
            "args": [],
            "envs": {},
            "features": [],
            "kernel_args": ["{{ua device.storage.units}}"],
            "options": []
        }"#;

        let mut emu_config = EmulatorConfiguration::default();

        let flags = process_flag_template_inner(template, &emu_config);
        assert!(flags.is_ok(), "{:?}", flags);
        assert_eq!(flags.as_ref().unwrap().args.len(), 0);
        assert_eq!(flags.as_ref().unwrap().envs.len(), 0);
        assert_eq!(flags.as_ref().unwrap().features.len(), 0);
        assert_eq!(flags.as_ref().unwrap().kernel_args.len(), 1);
        assert_eq!(flags.as_ref().unwrap().options.len(), 0);
        assert!(flags.as_ref().unwrap().kernel_args.contains(&"".to_string()));

        emu_config.device.storage.units = DataUnits::Megabytes;

        let flags = process_flag_template_inner(template, &emu_config);
        assert!(flags.is_ok(), "{:?}", flags);
        assert_eq!(flags.as_ref().unwrap().args.len(), 0);
        assert_eq!(flags.as_ref().unwrap().envs.len(), 0);
        assert_eq!(flags.as_ref().unwrap().features.len(), 0);
        assert_eq!(flags.as_ref().unwrap().kernel_args.len(), 1);
        assert_eq!(flags.as_ref().unwrap().options.len(), 0);
        assert!(flags.as_ref().unwrap().kernel_args.contains(&"M".to_string()));

        Ok(())
    }

    /// Tests invalid substitutions return errors.
    #[fuchsia::test]
    async fn test_invalid_substitutions() {
        let templates: Vec<(bool, String, String)> = vec![
            (
                true,
                "empty template".to_string(),
                r#" {
                "args": [],
                "envs": {},
                "features": [],
                "kernel_args": [],
                "options": []
            }"#
                .to_string(),
            ),
            (
                false,
                "unknown top level variable".to_string(),
                r#"
        {
            "args": [],
            "envs": {},
            "features": [],
            "kernel_args": ["{{unknown_top_level}}"],
            "options": []
        }"#
                .to_string(),
            ),
            (
                false,
                "unknown sub variable".to_string(),
                r#"
    {
        "args": [],
        "envs": {},
        "features": [],
        "kernel_args": ["{{guest.something}}"],
        "options": []
    }"#
                .to_string(),
            ),
            (
                true,
                "optional value = None should not err.".to_string(),
                r#"
    {
        "args": [],
        "envs": {},
        "features": [],
        "kernel_args": ["{{runtime.upscript}}"],
        "options": []
    }"#
                .to_string(),
            ),
            (
                false,
                "missing quote - invalid json should  err.".to_string(),
                r#"
    {
        "args": [],
        "envs": {},
        "features": [],
        "kernel_args": ["{{host.port_map}}],
        "options": []
    }"#
                .to_string(),
            ),
        ];
        let emu_config = EmulatorConfiguration::default();
        for (expected_ok, name, template) in templates.iter() {
            let flags = process_flag_template_inner(template, &emu_config);
            if *expected_ok {
                assert!(flags.is_ok(), "Processing {}: {:?}", name, flags);
            } else {
                assert!(flags.is_err(), "Processing {}: {:?}", name, flags);
            }
        }
    }

    #[test]
    fn test_dedupe_kernel_args() {
        let data = FlagData {
            args: vec!["arg1".into(), "arg2".into()],
            envs: HashMap::new(),
            features: vec!["feature1".into()],
            kernel_args: vec![
                "kernel.lockup-detector.critical-section-threshold-ms=5000".into(),
                "kernel.lockup-detector.heartbeat-age-fatal-threshold-ms=0".into(),
                "TERM=dumb".into(),
                "kernel.entropy-mixin=aaaaaaaaaaaaaaa".into(),
                "kernel.halt-on-panic=true".into(),
                "zircon.nodename=node1".into(),
            ],
            options: vec!["option1".into()],
        };

        let addl_kernel_args = vec![
            "kernel.lockup-detector.critical-section-threshold-ms=0".into(),
            "TERM=xterm-256color".into(),
            "kernel.entropy-mixin=42ac2452e99c1c979ebfca03bce0cbb14126e4021a6199ccfeca217999c0aaa0"
                .into(),
            "kernel.halt-on-panic=true".into(),
            "zircon.nodename=node1".into(),
        ];

        let mut expected: Vec<String> = vec![
            "TERM=xterm-256color".into(),
            "kernel.lockup-detector.critical-section-threshold-ms=0".into(),
            "kernel.lockup-detector.heartbeat-age-fatal-threshold-ms=0".into(),
            "kernel.entropy-mixin=42ac2452e99c1c979ebfca03bce0cbb14126e4021a6199ccfeca217999c0aaa0"
                .into(),
            "kernel.halt-on-panic=true".into(),
            "zircon.nodename=node1".into(),
        ];
        expected.sort();

        let updated = dedupe_kernel_args(data.clone(), &addl_kernel_args).expect("dedupe ok");

        // non-kernel args should be unchanged
        assert_eq!(updated.args, data.args);
        assert_eq!(updated.envs, data.envs);
        assert_eq!(updated.features, data.features);
        assert_eq!(updated.options, data.options);

        // sort the kernel args for stable comparison
        let mut sorted_actual = updated.kernel_args.clone();
        sorted_actual.sort();
        assert_eq!(sorted_actual, expected);
    }
    #[test]
    fn test_empty_dedupe_kernel_args() {
        let data = FlagData {
            args: vec!["arg1".into(), "arg2".into()],
            envs: HashMap::new(),
            features: vec!["feature1".into()],
            kernel_args: vec![
                "kernel.lockup-detector.critical-section-threshold-ms=5000".into(),
                "kernel.lockup-detector.heartbeat-age-fatal-threshold-ms=0".into(),
                "TERM=dumb".into(),
                "kernel.entropy-mixin=aaaaaaaaaaaaaaa".into(),
                "kernel.halt-on-panic=true".into(),
                "zircon.nodename=node1".into(),
            ],
            options: vec!["option1".into()],
        };

        let addl_kernel_args = vec![];

        let mut expected: Vec<String> = data.kernel_args.clone();
        expected.sort();

        let updated = dedupe_kernel_args(data.clone(), &addl_kernel_args).expect("dedupe ok");

        // non-kernel args should be unchanged
        assert_eq!(updated.args, data.args);
        assert_eq!(updated.envs, data.envs);
        assert_eq!(updated.features, data.features);
        assert_eq!(updated.options, data.options);

        // sort the kernel args for stable comparison
        let mut sorted_actual = updated.kernel_args.clone();
        sorted_actual.sort();
        assert_eq!(sorted_actual, expected);
    }

    #[fuchsia::test]
    fn test_efi_template_efi_kernel() {
        let config = EmulatorConfiguration {
            guest: GuestConfig {
                disk_image: None,
                kernel_image: Some(PathBuf::from("/path/to/some.efi")),
                ovmf_code: "/some/ovmf_code.fd".into(),
                ovmf_vars: "/some/ovmf_vars.fd".into(),
                ..Default::default()
            },
            host: HostConfig { networking: NetworkingMode::None, ..Default::default() },
            device: DeviceConfig {
                cpu: VirtualCpu { architecture: sdk_metadata::CpuArchitecture::X64, count: 2 },
                ..Default::default()
            },
            ..Default::default()
        };

        let actual = process_flag_template(&config).expect("ok processing");
        let expected_args: Vec<String> = [
            "-kernel",
            "/path/to/some.efi",
            "-drive",
            "if=pflash,format=raw,readonly=on,file=/some/ovmf_code.fd",
            "-drive",
            "if=pflash,format=raw,snapshot=on,file=/some/ovmf_vars.fd",
            "-m",
            "0",
            "-smp",
            "2,threads=2",
            "-qmp-pretty",
            "unix:/qmp,server,nowait",
            "-monitor",
            "unix:/monitor,server,nowait",
            "-serial",
            "unix:/serial,server,nowait,logfile=.serial",
            "-machine",
            "q35",
            "-fw_cfg",
            "name=etc/sercon-port,string=0",
            "-accel",
            "tcg,thread=single",
            "-cpu",
            "Haswell,+smap,-check,-fsgsbase",
            "-no-audio",
            "-nic",
            "none",
            "-nodefaults",
            "-parallel",
            "none",
            "-vga",
            "none",
            "-device",
            "virtio-keyboard-pci",
        ]
        .iter()
        .map(|a| a.to_string())
        .collect();

        assert_eq!(actual.args, expected_args)
    }

    #[fuchsia::test]
    fn test_efi_template_avx2_disabled() {
        let config = EmulatorConfiguration {
            guest: GuestConfig {
                disk_image: None,
                kernel_image: Some(PathBuf::from("/path/to/some.efi")),
                ovmf_code: "/some/ovmf_code.fd".into(),
                ovmf_vars: "/some/ovmf_vars.fd".into(),
                ..Default::default()
            },
            host: HostConfig { networking: NetworkingMode::None, ..Default::default() },
            device: DeviceConfig {
                cpu: VirtualCpu { architecture: sdk_metadata::CpuArchitecture::X64, count: 2 },
                avx2_enabled: false,
                ..Default::default()
            },
            ..Default::default()
        };

        let actual = process_flag_template(&config).expect("ok processing");
        assert!(actual.args.contains(&"-cpu".to_string()));
        let cpu_index = actual.args.iter().position(|r| r == "-cpu").unwrap();
        let cpu_flags = &actual.args[cpu_index + 1];
        assert!(cpu_flags.contains("-avx2"));
        assert!(cpu_flags.contains("-avx512f"));
    }

    #[fuchsia::test]
    fn test_efi_template_bootloader() {
        let config = EmulatorConfiguration {
            guest: GuestConfig {
                disk_image: Some(DiskImage::Fat("/path/to/file.fat".into())),
                kernel_image: None,
                ovmf_code: "/some/ovmf_code.fd".into(),
                ovmf_vars: "/some/ovmf_vars.fd".into(),
                ..Default::default()
            },
            device: DeviceConfig {
                cpu: VirtualCpu { architecture: sdk_metadata::CpuArchitecture::X64, count: 2 },
                ..Default::default()
            },
            host: HostConfig { networking: NetworkingMode::None, ..Default::default() },
            ..Default::default()
        };

        let actual = process_flag_template(&config).expect("ok processing");
        let expected_args: Vec<String> = [
            "-drive",
            "if=pflash,format=raw,readonly=on,file=/some/ovmf_code.fd",
            "-drive",
            "if=pflash,format=raw,snapshot=on,file=/some/ovmf_vars.fd",
            "-drive",
            "if=none,format=raw,file=/path/to/file.fat,id=uefi",
            "-device",
            "nec-usb-xhci,id=xhci0",
            "-device",
            "usb-storage,bus=xhci0.0,drive=uefi,removable=on,bootindex=0",
            "-m",
            "0",
            "-smp",
            "2,threads=2",
            "-qmp-pretty",
            "unix:/qmp,server,nowait",
            "-monitor",
            "unix:/monitor,server,nowait",
            "-serial",
            "unix:/serial,server,nowait,logfile=.serial",
            "-machine",
            "q35",
            "-fw_cfg",
            "name=etc/sercon-port,string=0",
            "-accel",
            "tcg,thread=single",
            "-cpu",
            "Haswell,+smap,-check,-fsgsbase",
            "-no-audio",
            "-nic",
            "none",
            "-nodefaults",
            "-parallel",
            "none",
            "-vga",
            "none",
            "-device",
            "virtio-keyboard-pci",
        ]
        .iter()
        .map(|a| a.to_string())
        .collect();

        assert_eq!(actual.args, expected_args)
    }

    #[fuchsia::test]
    fn test_efi_template_gpt_full_disk() {
        let config = EmulatorConfiguration {
            guest: GuestConfig {
                disk_image: Some(DiskImage::Gpt("/path/to/some.img".into())),
                is_gpt: true,
                ovmf_code: "/some/ovmf_code.fd".into(),
                ovmf_vars: "/some/ovmf_vars.fd".into(),
                ..Default::default()
            },
            host: HostConfig { networking: NetworkingMode::None, ..Default::default() },
            device: DeviceConfig {
                cpu: VirtualCpu { architecture: sdk_metadata::CpuArchitecture::X64, count: 2 },
                ..Default::default()
            },
            ..Default::default()
        };

        let actual = process_flag_template(&config).expect("ok processing");
        let expected_args: Vec<String> = [
            "-drive",
            "if=pflash,format=raw,readonly=on,file=/some/ovmf_code.fd",
            "-drive",
            "if=pflash,format=raw,snapshot=on,file=/some/ovmf_vars.fd",
            "-drive",
            "if=none,format=raw,file=/path/to/some.img,id=uefi",
            "-object",
            "iothread,id=iothread0",
            "-device",
            "virtio-blk-pci,drive=uefi,iothread=iothread0",
            "-m",
            "0",
            "-smp",
            "2,threads=2",
            "-qmp-pretty",
            "unix:/qmp,server,nowait",
            "-monitor",
            "unix:/monitor,server,nowait",
            "-serial",
            "unix:/serial,server,nowait,logfile=.serial",
            "-machine",
            "q35",
            "-device",
            "isa-debug-exit,iobase=0xf4,iosize=0x04",
            "-fw_cfg",
            "name=etc/sercon-port,string=0",
            "-accel",
            "tcg,thread=single",
            "-cpu",
            "Haswell,+smap,-check,-fsgsbase",
            "-no-audio",
            "-nic",
            "none",
            "-nodefaults",
            "-parallel",
            "none",
            "-vga",
            "none",
            "-device",
            "virtio-keyboard-pci",
        ]
        .iter()
        .map(|a| a.to_string())
        .collect();

        assert_eq!(actual.args, expected_args)
    }

    #[test]
    fn test_json_escape() {
        assert_eq!(json_escape(""), "");
        assert_eq!(json_escape("hello"), "hello");
        assert_eq!(json_escape("a=b"), "a=b");
        assert_eq!(json_escape(r#"evil", "-flag"#), r#"evil\", \"-flag"#);
        assert_eq!(json_escape("hello\nworld\r\t\\\""), "hello\\nworld\\r\\t\\\\\\\"");
    }

    #[fuchsia::test]
    fn test_json_injection_kernel_image() {
        let malicious_path = r#"kernel", "-append", "init=/bin/sh", "-kernel", "kernel"#;
        let config = EmulatorConfiguration {
            guest: GuestConfig {
                kernel_image: Some(PathBuf::from(malicious_path)),
                ..Default::default()
            },
            ..Default::default()
        };

        let actual = process_flag_template(&config).expect("ok processing");
        // Ensure the injected arguments did not become separate elements in `args`.
        assert!(!actual.args.contains(&"-append".to_string()));
        assert!(!actual.args.contains(&"init=/bin/sh".to_string()));
        let kernel_idx = actual.args.iter().position(|r| r == "-kernel").unwrap();
        assert_eq!(actual.args[kernel_idx + 1], malicious_path);
    }

    #[fuchsia::test]
    fn test_json_injection_disk_image() {
        let malicious_disk = r#"fxfs.blk", "-fsdev", "local,path=/,id=hostfs"#;
        let config = EmulatorConfiguration {
            guest: GuestConfig {
                disk_image: Some(DiskImage::Fxfs(malicious_disk.into())),
                kernel_image: Some(PathBuf::from("/path/to/kernel")),
                ..Default::default()
            },
            ..Default::default()
        };

        let actual = process_flag_template(&config).expect("ok processing");
        assert!(!actual.args.contains(&"-fsdev".to_string()));
        assert!(!actual.args.contains(&"local,path=/,id=hostfs".to_string()));
        assert!(actual.args.iter().any(|arg| arg.contains(malicious_disk)));
    }

    #[fuchsia::test]
    fn test_json_injection_ramdisk() {
        let malicious_ramdisk = r#"ramdisk.zbi", "-fsdev", "local,path=/,id=hostfs"#;
        let config = EmulatorConfiguration {
            guest: GuestConfig {
                kernel_image: Some(PathBuf::from("/path/to/kernel")),
                ramdisk: Some(emulator_instance::Ramdisk {
                    path: malicious_ramdisk.into(),
                    kind: emulator_instance::RamdiskKind::Zbi,
                }),
                ..Default::default()
            },
            ..Default::default()
        };

        let actual = process_flag_template(&config).expect("ok processing");
        assert!(!actual.args.contains(&"-fsdev".to_string()));
        assert!(!actual.args.contains(&"local,path=/,id=hostfs".to_string()));
        let initrd_idx = actual.args.iter().position(|r| r == "-initrd").unwrap();
        assert_eq!(actual.args[initrd_idx + 1], malicious_ramdisk);
    }

    #[fuchsia::test]
    async fn test_json_injection_env_helper() {
        unsafe { std::env::set_var("EVIL_INJECT_VAR", r#"val", "injected_arg"#) };
        let template = r#"
        {
            "args": ["{{env "EVIL_INJECT_VAR"}}"],
            "envs": {},
            "features": [],
            "kernel_args": [],
            "options": []
        }"#;
        let emu_config = EmulatorConfiguration::default();
        let flags = process_flag_template_inner(template, &emu_config).expect("processed");
        assert_eq!(flags.args.len(), 1);
        assert_eq!(flags.args[0], r#"val", "injected_arg"#);
    }
}
