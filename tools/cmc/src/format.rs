// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::error::Error;
use crate::util;
use cml::format_cml;
use std::fs;
use std::io::{BufRead, Read, Write};
use std::path::PathBuf;
use std::str::from_utf8;

pub(crate) enum Input<'a, R: BufRead> {
    File(&'a PathBuf),
    Stdin(R),
}

/// Formats a CML file.
///
/// The file must be valid JSON5, but does not have to be valid CML.
///
/// If `output` is not given, prints the result to stdout.
///
/// See [format_cml] for current style conventions.
pub(crate) fn format<R: BufRead>(
    input: Input<'_, R>,
    output: Option<PathBuf>,
) -> Result<(), Error> {
    let mut buffer = String::new();
    let file = match input {
        Input::File(p) => {
            fs::File::open(&p)?.read_to_string(&mut buffer)?;
            Some(p.as_path())
        }
        Input::Stdin(mut handle) => {
            loop {
                let n = handle.read_line(&mut buffer)?;
                if n == 0 {
                    break;
                }
            }
            None
        }
    };

    let res = format_cml(&buffer, file)?;

    if let Some(output_path) = output {
        util::ensure_directory_exists(&output_path)?;
        fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(output_path)?
            .write_all(&res)?;
    } else {
        // Print without a newline because the formatters should have already added a final newline
        // (required by most software style guides).
        print!("{}", from_utf8(&res)?);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::BufReader;
    use tempfile::TempDir;
    use test_case::test_case;

    type Phantom = std::io::StdinLock<'static>;

    #[test_case(true; "stdin")]
    #[test_case(false; "file")]
    fn test_format_cml(stdin: bool) {
        let example_cml = r##"{
    offer: [
        {
            runner: "elf",
        },
        {
            from: "framework",
            to: "#elements",
            protocol: "/svc/fuchsia.component.Realm",
        },
        {
            to: "#elements",
            protocol: [
                "/svc/fuchsia.logger.LogSink",
                "/svc/fuchsia.cobalt.LoggerFactory",
            ],
            from: "realm",
        },
    ],
    collections: [
        {
            durability: "transient",
            name: "elements",
        }
    ],
    use: [
        {
            protocol: "/svc/fuchsia.component.Realm",
            from: "framework",
        },
        {
            from: "realm",
            to: "#elements",
            protocol: [
                "/svc/fuchsia.logger.LogSink",
                "/svc/fuchsia.cobalt.LoggerFactory",
            ],
        },
    ],
    children: [
    ],
    program: {
        args: [ "--zarg_first", "zoo_opt", "--arg3", "and_arg3_value" ],
        binary: "bin/session_manager",
        runner: "elf",
    },
}"##;
        let expected = r##"{
    program: {
        runner: "elf",
        binary: "bin/session_manager",
        args: [
            "--zarg_first",
            "zoo_opt",
            "--arg3",
            "and_arg3_value",
        ],
    },
    children: [],
    collections: [
        {
            name: "elements",
            durability: "transient",
        },
    ],
    use: [
        {
            protocol: "/svc/fuchsia.component.Realm",
            from: "framework",
        },
        {
            protocol: [
                "/svc/fuchsia.cobalt.LoggerFactory",
                "/svc/fuchsia.logger.LogSink",
            ],
            from: "realm",
            to: "#elements",
        },
    ],
    offer: [
        { runner: "elf" },
        {
            protocol: "/svc/fuchsia.component.Realm",
            from: "framework",
            to: "#elements",
        },
        {
            protocol: [
                "/svc/fuchsia.cobalt.LoggerFactory",
                "/svc/fuchsia.logger.LogSink",
            ],
            from: "realm",
            to: "#elements",
        },
    ],
}
"##;
        let tmp_dir = TempDir::new().unwrap();

        let tmp_file_path = tmp_dir.path().join("input.cml");
        File::create(&tmp_file_path).unwrap().write_all(example_cml.as_bytes()).unwrap();

        let output_file_path = tmp_dir.path().join("output.cml");

        let input = if stdin {
            let f = File::open(&tmp_file_path).unwrap();
            Input::Stdin(BufReader::new(f))
        } else {
            Input::File(&tmp_file_path)
        };
        let result = format(input, Some(output_file_path.clone()));
        assert!(result.is_ok());

        let mut buffer = String::new();
        File::open(&output_file_path).unwrap().read_to_string(&mut buffer).unwrap();
        assert_eq!(buffer, expected);
    }

    #[test]
    fn test_format_cml_with_environments() {
        let example_cml = r##"{
    include: [ "src/sys/test_manager/meta/common.shard.cml" ],
    environments: [
        {
            name: "test-env",
            extends: "realm",
            runners: [
                {
                    from: "#elf_test_runner",
                    runner: "elf_test_runner",
                },
                {
                    from: "#gtest_runner",
                    runner: "gtest_runner",
                },
                {
                    from: "#rust_test_runner",
                    runner: "rust_test_runner",
                },
                {
                    from: "#go_test_runner",
                    runner: "go_test_runner",
                },
                {
                    from: "#fuchsia_component_test_framework_intermediary",
                    runner: "fuchsia_component_test_mocks",
                },
            ],
            resolvers: [
                {
                    from: "#fuchsia_component_test_framework_intermediary",
                    resolver: "fuchsia_component_test_registry",
                    scheme: "fuchsia-component-test-registry",
                },
            ],
        },
    ],
}
"##;
        let expected = r##"{
    include: [ "src/sys/test_manager/meta/common.shard.cml" ],
    environments: [
        {
            name: "test-env",
            extends: "realm",
            runners: [
                {
                    runner: "elf_test_runner",
                    from: "#elf_test_runner",
                },
                {
                    runner: "gtest_runner",
                    from: "#gtest_runner",
                },
                {
                    runner: "rust_test_runner",
                    from: "#rust_test_runner",
                },
                {
                    runner: "go_test_runner",
                    from: "#go_test_runner",
                },
                {
                    runner: "fuchsia_component_test_mocks",
                    from: "#fuchsia_component_test_framework_intermediary",
                },
            ],
            resolvers: [
                {
                    resolver: "fuchsia_component_test_registry",
                    from: "#fuchsia_component_test_framework_intermediary",
                    scheme: "fuchsia-component-test-registry",
                },
            ],
        },
    ],
}
"##;
        let tmp_dir = TempDir::new().unwrap();

        let tmp_file_path = tmp_dir.path().join("input.cml");
        File::create(&tmp_file_path).unwrap().write_all(example_cml.as_bytes()).unwrap();

        let output_file_path = tmp_dir.path().join("output.cml");

        let result = format(Input::<Phantom>::File(&tmp_file_path), Some(output_file_path.clone()));
        assert!(result.is_ok());

        let mut buffer = String::new();
        File::open(&output_file_path).unwrap().read_to_string(&mut buffer).unwrap();
        assert_eq!(buffer, expected);
    }

    #[test]
    fn test_format_valid_json5() {
        let example_json5 = "{\"foo\": 1,}";
        let tmp_dir = TempDir::new().unwrap();

        let tmp_file_path = tmp_dir.path().join("input.json");
        File::create(&tmp_file_path).unwrap().write_all(example_json5.as_bytes()).unwrap();

        let result = format(Input::<Phantom>::File(&tmp_file_path), None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_format_invalid_json5() {
        let invalid_json5 = "{\"foo\" 1}";
        let tmp_dir = TempDir::new().unwrap();

        let tmp_file_path = tmp_dir.path().join("input.json");
        File::create(&tmp_file_path).unwrap().write_all(invalid_json5.as_bytes()).unwrap();

        let result = format(Input::<Phantom>::File(&tmp_file_path), None);
        assert!(result.is_err());
    }
}
