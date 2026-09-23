// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::error::Error;
use crate::util;
use cml::types::document::Document;
use std::fs;
use std::path::PathBuf;

enum ComponentManifest {
    Cml(Box<Document>),
    Cm(Box<cm_rust::ComponentDecl>),
}

// These runners respect `program.binary`. We only validate the `program` section for components
// using them.
// TODO(https://fxbug.dev/42146038)
const RUNNER_VALIDATE_LIST: [&'static str; 6] = [
    "driver",
    "elf",
    "elf_test_runner",
    "go_test_runner",
    "gtest_runner",
    "rust_test_runner",
    // TODO(https://fxbug.dev/42147650): Add support for Dart components.
];

const CHECK_REFERENCES_URL: &str = "https://fuchsia.dev/go/components/build-errors";

/// Validates that a component manifest file is consistent with the content
/// of its package. Checks included in this function are:
///     1. If provided program binary in component manifest matches with
///        executable target declared in package manifest.
///     2. If provided program bind in component manifest matches with
///        executable target declared in package manifest.
///     3. If provided program compat in component manifest matches with
///        executable target declared in package manifest.
/// If all checks pass, this function returns Ok(()).
pub(crate) fn validate(
    component_manifest_path: &PathBuf,
    package_manifest_path: &PathBuf,
    context: Option<&String>,
) -> Result<(), Error> {
    let package_manifest = read_package_manifest(package_manifest_path)?;
    let package_targets = get_package_targets(&package_manifest);

    validate_binary(component_manifest_path, &package_targets, context)?;
    validate_driver_reference(component_manifest_path, &package_targets, context, "bind")?;
    validate_driver_reference(component_manifest_path, &package_targets, context, "compat")?;
    Ok(())
}

fn validate_binary(
    component_manifest_path: &PathBuf,
    package_targets: &Vec<String>,
    context: Option<&String>,
) -> Result<(), Error> {
    let component_manifest = read_component_manifest(component_manifest_path)?;
    match get_component_runner(&component_manifest).as_deref() {
        Some(runner) => {
            if !RUNNER_VALIDATE_LIST.iter().any(|r| &runner == r) {
                return Ok(());
            }
        }
        None => {}
    }

    let program_binary = get_program_reference(&component_manifest, "binary");

    if program_binary.is_none() {
        return Ok(());
    }

    let program_binary = program_binary.unwrap();

    if package_targets.contains(&program_binary) {
        return Ok(());
    }

    // Legacy package.gni supports the "disabled test" feature that
    // intentionally breaks component manifests.
    if program_binary.starts_with("test/")
        && package_targets.contains(&format!("test/disabled/{}", &program_binary[5..]))
    {
        return Ok(());
    }

    Err(missing_reference_error(
        "binary",
        component_manifest_path,
        program_binary,
        package_targets,
        context,
    ))
}

fn validate_driver_reference(
    component_manifest_path: &PathBuf,
    package_targets: &Vec<String>,
    context: Option<&String>,
    reference: &str,
) -> Result<(), Error> {
    let component_manifest = read_component_manifest(component_manifest_path)?;
    match get_component_runner(&component_manifest).as_deref() {
        Some(runner) => {
            if runner != "driver" {
                return Ok(());
            }
        }
        None => {}
    }

    let program_reference = get_program_reference(&component_manifest, reference);

    if program_reference.is_none() {
        return Ok(());
    }

    let program_reference = program_reference.unwrap();

    if package_targets.contains(&program_reference) {
        return Ok(());
    }

    Err(missing_reference_error(
        reference,
        component_manifest_path,
        program_reference,
        package_targets,
        context,
    ))
}

fn read_package_manifest(path: &PathBuf) -> Result<String, Error> {
    fs::read_to_string(path).map_err(|e| {
        Error::parse(format!("Couldn't parse file {:?}: {}", path, e), None, Some(path))
    })
}

fn read_component_manifest(path: &PathBuf) -> Result<ComponentManifest, Error> {
    const BAD_EXTENSION: &str = "Input file does not have a component manifest extension \
                                 (.cm or .cml)";
    let ext = path.extension().and_then(|e| e.to_str());
    Ok(match ext {
        Some("cml") => ComponentManifest::Cml(Box::new(util::read_cml(path)?)),
        Some("cm") => ComponentManifest::Cm(Box::new(util::read_cm(path)?)),
        _ => {
            return Err(Error::invalid_args(BAD_EXTENSION));
        }
    })
}

fn get_package_targets(package_manifest: &str) -> Vec<String> {
    package_manifest
        .lines()
        .map(|line| line.split("="))
        .map(|mut parts| parts.next())
        .filter(Option::is_some)
        // Safe to unwrap because Option(None) values have been filtered out.
        .map(Option::unwrap)
        .map(str::to_owned)
        .collect()
}

fn get_program_reference(
    component_manifest: &ComponentManifest,
    reference: &str,
) -> Option<String> {
    match component_manifest {
        ComponentManifest::Cml(document) => match &document.program {
            Some(program) => match program.info.get(reference) {
                Some(binary) => match binary.as_str() {
                    Some(value) => Some(value.to_owned()),
                    None => None,
                },
                None => None,
            },
            None => None,
        },
        ComponentManifest::Cm(decl) => {
            let entries = match &decl.program {
                Some(program) => match &program.info.entries {
                    Some(entries) => entries,
                    None => return None,
                },
                None => return None,
            };

            for entry in entries {
                if entry.key == reference {
                    if let Some(value) = &entry.value {
                        if let fidl_fuchsia_data::DictionaryValue::Str(ref str) = **value {
                            return Some(str.clone());
                        } else {
                            return None;
                        }
                    } else {
                        return None;
                    }
                }
            }

            None
        }
    }
}

fn get_component_runner(component_manifest: &ComponentManifest) -> Option<String> {
    match component_manifest {
        ComponentManifest::Cml(document) => document
            .r#use
            .as_ref()
            .and_then(|u| {
                u.iter()
                    .find(|&r| r.runner.is_some())
                    .and_then(|ru| ru.runner.as_ref().map(|s| s.as_str().to_owned()))
            })
            .or_else(|| {
                document
                    .program
                    .as_ref()
                    .and_then(|p| p.runner.as_ref().map(|s| s.as_str().to_owned()))
            }),
        #[cfg(fuchsia_api_level_at_least = "HEAD")]
        ComponentManifest::Cm(decl) => decl
            .uses
            .iter()
            .find_map(|u| match u {
                cm_rust::UseDecl::Runner(r) => Some(r.source_name.to_string()),
                _ => None,
            })
            .or_else(|| {
                decl.program.as_ref().and_then(|p| p.runner.as_ref().map(|n| n.to_string()))
            }),
        #[cfg(fuchsia_api_level_less_than = "HEAD")]
        ComponentManifest::Cm(decl) => {
            decl.program.as_ref().and_then(|p| p.runner.as_ref().map(|n| n.to_string()))
        }
    }
}

fn missing_reference_error(
    reference: &str,
    component_manifest_path: &PathBuf,
    program_reference: String,
    package_targets: &Vec<String>,
    context: Option<&String>,
) -> Error {
    let header = gen_header(component_manifest_path, context);
    if package_targets.is_empty() {
        return Error::validate(format!("{}\n\tPackage deps is empty!", header));
    }

    // We couldn't find the reference, let's get the nearest match.
    let nearest_match = get_nearest_match(&program_reference, package_targets);

    Error::validate(format!(
        r"{}
program.{}={} but {} is not provided by deps!

Did you mean {}?

Try any of the following:
{}

For more details, see {}",
        header,
        reference,
        program_reference,
        program_reference,
        nearest_match,
        package_targets.join("\n"),
        CHECK_REFERENCES_URL,
    ))
}

fn gen_header(component_manifest_path: &PathBuf, context: Option<&String>) -> String {
    match context {
        Some(label) => format!(
            "Error found in: {}\n\tFailed to validate manifest: {:#?}",
            label, component_manifest_path
        ),
        None => format!("Failed to validate manifest: {:#?}", component_manifest_path),
    }
}

fn get_nearest_match<'a>(reference: &'a str, candidates: &'a Vec<String>) -> &'a str {
    let mut nearest = &candidates[0];
    let mut min_distance = strsim::levenshtein(reference, &candidates[0]);
    for candidate in candidates.iter().skip(1) {
        let distance = strsim::levenshtein(reference, candidate);
        if distance < min_distance {
            min_distance = distance;
            nearest = candidate;
        }
    }
    &nearest
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::compile;
    use crate::features::FeatureSet;
    use assert_matches::assert_matches;
    use serde_json::json;
    use std::fmt::Display;
    use std::fs::File;
    use std::io::Write;
    use tempfile::TempDir;

    fn tmp_file(tmp_dir: &TempDir, name: &str, contents: impl Display) -> PathBuf {
        let path = tmp_dir.path().join(name);
        File::create(tmp_dir.path().join(name))
            .unwrap()
            .write_all(format!("{:#}", contents).as_bytes())
            .unwrap();
        return path;
    }

    fn compiled_tmp_file(tmp_dir: &TempDir, name: &str, contents: impl Display) -> PathBuf {
        let tmp_dir_path = tmp_dir.path().to_path_buf();
        let path = tmp_dir_path.join(name);
        let cml_path = tmp_dir_path.join("temp.cml");
        File::create(&cml_path).unwrap().write_all(format!("{:#}", contents).as_bytes()).unwrap();
        compile(
            &cml_path,
            &path,
            None,
            &vec![],
            &tmp_dir_path,
            None,
            &FeatureSet::empty(),
            &None,
            cml::CapabilityRequirements { must_offer: &[], must_use: &[] },
        )
        .unwrap();
        return path;
    }

    macro_rules! fini_file {
        ( $( $line:literal ),* ) => {
            {
                let mut buf: Vec<&str> = Vec::new();
                $(
                    buf.push($line);
                )*
                buf.join("\n")
            }
        };
    }

    #[test]
    fn validate_returns_ok_if_program_binary_empty() {
        let tmp_dir = TempDir::new().unwrap();
        let component_manifest = tmp_file(&tmp_dir, "empty.cml", json!({}));
        let package_manifest = tmp_file(
            &tmp_dir,
            "test.fini",
            fini_file!("bin/hello_world=hello_world", "lib/foo=foo"),
        );

        assert_matches!(validate(&component_manifest, &package_manifest, None), Ok(()));
    }

    #[test]
    fn validate_returns_ok_for_proper_cml() {
        let tmp_dir = TempDir::new().unwrap();
        let component_manifest = tmp_file(
            &tmp_dir,
            "test.cml",
            r#"{
                // JSON5, which .cml uses, allows for comments.
                program: {
                    runner: "elf",
                    binary: "bin/hello_world",
                },
            }"#,
        );
        let package_manifest = tmp_file(
            &tmp_dir,
            "test.fini",
            fini_file!("bin/hello_world=hello_world", "lib/foo=foo"),
        );

        assert_matches!(validate(&component_manifest, &package_manifest, None), Ok(()));
    }

    #[test]
    fn validate_returns_ok_for_proper_cml_with_use_runner() {
        let tmp_dir = TempDir::new().unwrap();
        let component_manifest = tmp_file(
            &tmp_dir,
            "test.cml",
            r#"{
                // JSON5, which .cml uses, allows for comments.
                program: {
                    binary: "bin/hello_world",
                },
                use: [{  runner: "elf", }],
            }"#,
        );
        let package_manifest = tmp_file(
            &tmp_dir,
            "test.fini",
            fini_file!("bin/hello_world=hello_world", "lib/foo=foo"),
        );

        assert_matches!(validate(&component_manifest, &package_manifest, None), Ok(()));
    }

    #[test]
    fn validate_returns_ok_for_proper_cm() {
        let tmp_dir = TempDir::new().unwrap();
        let component_manifest = compiled_tmp_file(
            &tmp_dir,
            "test.cm",
            r#"{
                program: {
                    runner: "elf",
                    binary: "bin/hello_world",
                },
            }"#,
        );
        let package_manifest = tmp_file(
            &tmp_dir,
            "test.fini",
            fini_file!("bin/hello_world=hello_world", "lib/foo=foo"),
        );

        assert_matches!(validate(&component_manifest, &package_manifest, None), Ok(()));
    }

    #[test]
    fn validate_returns_ok_for_test_binaries() {
        let tmp_dir = TempDir::new().unwrap();
        let component_manifest = compiled_tmp_file(
            &tmp_dir,
            "test.cm",
            r#"{
                program: {
                    runner: "elf",
                    binary: "test/hello_world",
                },
            }"#,
        );
        let package_manifest = tmp_file(
            &tmp_dir,
            "test.fini",
            fini_file!("test/hello_world=hello_world", "lib/foo=foo"),
        );

        assert_matches!(validate(&component_manifest, &package_manifest, None), Ok(()));
    }

    #[test]
    fn validate_returns_ok_for_disabled_test_binaries() {
        let tmp_dir = TempDir::new().unwrap();
        let component_manifest = compiled_tmp_file(
            &tmp_dir,
            "test.cm",
            r#"{
                program: {
                    runner: "elf",
                    binary: "test/disabled/hello_world",
                },
            }"#,
        );
        let package_manifest = tmp_file(
            &tmp_dir,
            "test.fini",
            fini_file!("test/disabled/hello_world=hello_world", "lib/foo=foo"),
        );

        assert_matches!(validate(&component_manifest, &package_manifest, None), Ok(()));
    }

    #[test]
    fn validate_returns_err_if_package_manifest_is_empty() {
        let tmp_dir = TempDir::new().unwrap();
        let package_manifest = tmp_file(&tmp_dir, "test.fini", "");
        let component_manifest = compiled_tmp_file(
            &tmp_dir,
            "test.cm",
            r#"{
                program: {
                    runner: "elf",
                    binary: "bin/hello_world",
                },
            }"#,
        );

        assert_matches!(validate(&component_manifest, &package_manifest, None), Err(_));
    }

    #[test]
    fn validate_returns_err_if_binary_not_found() {
        let tmp_dir = TempDir::new().unwrap();
        let component_manifest = compiled_tmp_file(
            &tmp_dir,
            "test.cm",
            r#"{
                program: {
                    runner: "elf",
                    "binary": "bin/not_a_listed_binary"
                },
            }"#,
        );
        let package_manifest = tmp_file(
            &tmp_dir,
            "test.fini",
            fini_file!("bin/hello_world=hello_world", "lib/foo=foo"),
        );

        assert_matches!(validate(&component_manifest, &package_manifest, None), Err(_));
    }

    #[test]
    fn validate_returns_err_if_component_manifest_has_unknown_extension() {
        let tmp_dir = TempDir::new().unwrap();
        let component_manifest = tmp_file(
            &tmp_dir,
            "test.txt",
            r#"{
                // JSON5, which .cml uses, allows for comments.
                program: {
                    binary: "bin/hello_world",
                },
            }"#,
        );
        let package_manifest = tmp_file(
            &tmp_dir,
            "test.fini",
            fini_file!("bin/hello_world=hello_world", "lib/foo=foo"),
        );

        assert_matches!(validate(&component_manifest, &package_manifest, None), Err(_));
    }

    #[test]
    fn validate_returns_err_if_cml_is_malformed() {
        let tmp_dir = TempDir::new().unwrap();
        let component_manifest = tmp_file(
            &tmp_dir,
            "test.cml",
            r#"
            // With no opening '{', the json5 parser should yield an error.
                program: {
                    binary: "bin/hello_world",
                }
            "#,
        );
        let package_manifest = tmp_file(
            &tmp_dir,
            "test.fini",
            fini_file!("bin/hello_world=hello_world", "lib/foo=foo"),
        );

        assert_matches!(validate(&component_manifest, &package_manifest, None), Err(_));
    }

    #[test]
    fn validate_returns_err_if_package_manifest_is_bad_file() {
        let tmp_dir = TempDir::new().unwrap();
        let component_manifest = tmp_file(
            &tmp_dir,
            "test.cml",
            r#"{
                // JSON5, which .cml uses, allows for comments.
                program: {
                    binary: "bin/hello_world",
                },
                use: [{  runner: "elf", }],
            }"#,
        );

        assert_matches!(
            validate(&component_manifest, &PathBuf::from("file/doesnt/exist"), None),
            Err(_)
        );
    }

    #[test]
    fn get_nearest_match_returns_correct_value() {
        assert_eq!(
            get_nearest_match("foo", &vec!["lib/bar".to_string(), "bin/foo".to_string()]),
            "bin/foo"
        );
        assert_eq!(
            get_nearest_match("foo", &vec!["bin/foo".to_string(), "lib/bar".to_string()]),
            "bin/foo"
        );
    }

    #[test]
    fn validate_returns_ok_for_proper_driver_cml() {
        let tmp_dir = TempDir::new().unwrap();
        let component_manifest = tmp_file(
            &tmp_dir,
            "test.cml",
            r#"{
                // JSON5, which .cml uses, allows for comments.
                program: {
                    runner: "driver",
                    binary: "bin/compat.so",
                    bind: "meta/hello_world.bindbc",
                    compat: "bin/hello_world.so",
                },
            }"#,
        );
        let package_manifest = tmp_file(
            &tmp_dir,
            "test.fini",
            fini_file!(
                "bin/compat.so=compat",
                "bin/hello_world.so=hello_world",
                "lib/foo=foo",
                "meta/hello_world.bindbc=bind"
            ),
        );

        assert_matches!(validate(&component_manifest, &package_manifest, None), Ok(()));
    }

    #[test]
    fn validate_returns_err_if_bind_not_found() {
        let tmp_dir = TempDir::new().unwrap();
        let component_manifest = tmp_file(
            &tmp_dir,
            "test.cml",
            r#"{
                // JSON5, which .cml uses, allows for comments.
                program: {
                    runner: "driver",
                    binary: "bin/compat.so",
                    bind: "meta/does_not_exist.bindbc",
                    compat: "bin/hello_world.so",
                },
            }"#,
        );
        let package_manifest = tmp_file(
            &tmp_dir,
            "test.fini",
            fini_file!(
                "bin/compat.so=compat",
                "bin/hello_world.so=hello_world",
                "lib/foo=foo",
                "meta/hello_world.bindbc=bind"
            ),
        );

        assert_matches!(validate(&component_manifest, &package_manifest, None), Err(_));
    }

    #[test]
    fn validate_returns_err_if_compat_not_found() {
        let tmp_dir = TempDir::new().unwrap();
        let component_manifest = tmp_file(
            &tmp_dir,
            "test.cml",
            r#"{
                // JSON5, which .cml uses, allows for comments.
                program: {
                    runner: "driver",
                    binary: "bin/compat.so",
                    bind: "meta/hello_world.bindbc",
                    compat: "bin/does_not_exist.so",
                },
            }"#,
        );
        let package_manifest = tmp_file(
            &tmp_dir,
            "test.fini",
            fini_file!(
                "bin/compat.so=compat",
                "bin/hello_world.so=hello_world",
                "lib/foo=foo",
                "meta/hello_world.bindbc=bind"
            ),
        );

        assert_matches!(validate(&component_manifest, &package_manifest, None), Err(_));
    }
}
