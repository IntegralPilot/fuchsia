// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::error::Error;
use crate::util;
use crate::util::write_depfile;
use cml::features::FeatureSet;
use cml::load::{CmlLoader, OsResolver};
use cml::translate::compile;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;

/// Read in the provided JSON file and add includes.
/// If the JSON file is an object with a key "include" that references an array of strings then
/// the strings are treated as paths to JSON files to be merged with the input file.
///
/// If a depfile is provided, also writes includes encountered to the depfile.
pub(crate) fn merge_includes(
    file: &PathBuf,
    output: Option<&PathBuf>,
    depfile: Option<&PathBuf>,
    includepath: &Vec<PathBuf>,
    includeroot: &PathBuf,
    validate: bool,
    features: &FeatureSet,
) -> Result<(), Error> {
    let resolver = OsResolver::new(includepath.clone(), includeroot.clone());
    let mut loader = CmlLoader::new(resolver);

    let mut document = loader.load_and_merge_all(file)?;

    document.include = None;
    document.canonicalize();

    if validate {
        compile(&document, cml::CompileOptions::new().features(features).file(file.as_path()))?;
    }

    let json_str = serde_json::to_string_pretty(&document)?;

    if let Some(output_path) = output.as_ref() {
        util::ensure_directory_exists(&output_path)?;
        std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(output_path)?
            .write_all(json_str.as_bytes())?;
    } else {
        println!("{:#}", json_str);
    }

    if let Some(depfile_path) = depfile {
        let mut includes: Vec<PathBuf> =
            loader.visited_files().into_iter().filter(|p| p != file).collect();

        includes.sort();
        util::write_depfile(depfile_path, output, &includes)?;
    }

    Ok(())
}

const CHECK_INCLUDES_URL: &str = "https://fuchsia.dev/go/components/build-errors";

/// Read in the provided JSON file and ensure that it contains all expected includes.
pub(crate) fn check_includes(
    file: &PathBuf,
    mut expected_includes: Vec<String>,
    // If specified, this is a path to newline-delimited `expected_includes`
    fromfile: Option<&PathBuf>,
    depfile: Option<&PathBuf>,
    stamp: Option<&PathBuf>,
    includepath: &Vec<PathBuf>,
    includeroot: &PathBuf,
) -> Result<(), Error> {
    if let Some(path) = fromfile {
        let reader = BufReader::new(fs::File::open(path)?);
        for line in reader.lines() {
            match line {
                Ok(value) => expected_includes.push(String::from(value)),
                Err(e) => return Err(Error::invalid_args(format!("Invalid --fromfile: {}", e))),
            }
        }
    }

    if expected_includes.is_empty() {
        if let Some(depfile_path) = depfile {
            if depfile_path.exists() {
                // Delete stale depfile
                fs::remove_file(depfile_path)?;
            }
        }
        return Ok(());
    }

    let resolver = OsResolver::new(includepath.clone(), includeroot.clone());
    let mut loader = CmlLoader::new(resolver);

    loader.load_and_merge_all(file)?;

    let actual_includes = loader.visited_files();

    for expected in expected_includes.iter() {
        let expected_path = canonicalize_include(&expected, includepath, &includeroot);

        if !actual_includes.contains(&expected_path) {
            return Err(Error::Validate {
                err: format!(
                    "{:?} must include {:?}.\nFor more details, see {}",
                    file, expected, CHECK_INCLUDES_URL
                ),
                filename: file.to_str().map(String::from),
            });
        }
    }

    // Write includes to depfile
    if let Some(depfile_path) = depfile {
        let mut inputs: Vec<PathBuf> = actual_includes.into_iter().collect();
        inputs.sort();
        write_depfile(depfile_path, stamp, &inputs)?;
    }

    Ok(())
}

/// Resolves an include to a canonical path.
fn canonicalize_include(
    include: &String,
    includepath: &Vec<PathBuf>,
    includeroot: &PathBuf,
) -> PathBuf {
    if include.starts_with("//") {
        // Resolve against sources root
        return includeroot.join(&include[2..]);
    }
    for prefix in includepath {
        // Resolve against first matching includepath
        let resolved = prefix.join(&include);
        if resolved.exists() {
            return resolved;
        }
    }
    // Resolve against CWD
    return PathBuf::from(include);
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use serde_json::{Value, json};
    use std::fmt::Display;
    use std::fs::File;
    use std::io::{LineWriter, Read};
    use tempfile::TempDir;

    struct TestContext {
        // Root of source tree
        root_path: PathBuf,
        // Resolve includes in these paths
        include_path: Vec<PathBuf>,
        // Various inputs and outputs
        fromfile: PathBuf,
        depfile: PathBuf,
        stamp: PathBuf,
        output: PathBuf,
        _tmpdir: TempDir,
    }

    impl TestContext {
        fn new() -> Self {
            let tmpdir = TempDir::new().unwrap();
            let tmpdir_path = tmpdir.path();
            let includedir_path = tmpdir_path.join("includes");
            fs::create_dir(&includedir_path).unwrap();
            // Includes may also be resolved from a secondary directory
            let includedir_path_secondary = tmpdir_path.join("more_includes");
            fs::create_dir(&includedir_path_secondary).unwrap();
            let include_path = vec![includedir_path, includedir_path_secondary];
            Self {
                root_path: tmpdir_path.to_path_buf(),
                include_path: include_path,
                depfile: tmpdir_path.join("depfile"),
                stamp: tmpdir_path.join("stamp"),
                fromfile: tmpdir_path.join("fromfile"),
                output: tmpdir_path.join("out"),
                _tmpdir: tmpdir,
            }
        }

        fn new_include(&self, name: &str, contents: impl Display) -> PathBuf {
            let path = self.include_path[0].join(name);
            File::create(&path).unwrap().write_all(format!("{:#}", contents).as_bytes()).unwrap();
            path
        }

        fn new_include_secondary(&self, name: &str, contents: impl Display) -> PathBuf {
            let path = self.include_path[1].join(name);
            File::create(&path).unwrap().write_all(format!("{:#}", contents).as_bytes()).unwrap();
            path
        }

        fn new_file(&self, name: &str, contents: impl Display) -> PathBuf {
            let path = self.root_path.join(name);
            File::create(&path).unwrap().write_all(format!("{:#}", contents).as_bytes()).unwrap();
            return path;
        }

        fn new_dir(&self, name: &str) {
            fs::create_dir_all(self.root_path.join(name)).unwrap();
        }

        fn merge_includes(&self, file: &PathBuf) -> Result<(), Error> {
            super::merge_includes(
                file,
                Some(&self.output),
                Some(&self.depfile),
                &self.include_path,
                &self.root_path,
                true,
                &FeatureSet::empty(),
            )
        }

        fn merge_includes_no_validation(&self, file: &PathBuf) -> Result<(), Error> {
            super::merge_includes(
                file,
                Some(&self.output),
                Some(&self.depfile),
                &self.include_path,
                &self.root_path,
                false,
                &FeatureSet::empty(),
            )
        }

        fn check_includes(
            &self,
            file: &PathBuf,
            expected_includes: Vec<String>,
        ) -> Result<(), Error> {
            let fromfile = if self.fromfile.exists() { Some(&self.fromfile) } else { None };
            super::check_includes(
                file,
                expected_includes,
                fromfile,
                Some(&self.depfile),
                Some(&self.stamp),
                &self.include_path,
                &self.root_path,
            )
        }

        #[track_caller]
        fn assert_output_eq(&self, expected: Value) {
            let mut actual = String::new();
            File::open(&self.output).unwrap().read_to_string(&mut actual).unwrap();
            let actual_json: Value = serde_json::from_str(actual.as_str()).unwrap();
            assert_eq!(actual_json, expected, "Unexpected contents of {:?}", &self.output);
        }

        fn assert_depfile_eq(&self, out: &PathBuf, ins: &[&PathBuf]) {
            let mut actual = String::new();
            File::open(&self.depfile).unwrap().read_to_string(&mut actual).unwrap();
            let expected = format!(
                "{}:{}\n",
                out.display(),
                &ins.iter().map(|i| format!(" {}", i.display())).collect::<String>()
            );
            assert_eq!(actual, expected, "Unexpected contents of {:?}", &self.depfile);
        }

        fn assert_no_depfile(&self) {
            assert!(!self.depfile.exists());
        }
    }

    #[test]
    fn test_include_cml() {
        let ctx = TestContext::new();
        let cml_path = ctx.new_file(
            "some.cml",
            "{include: [\"shard.cml\"], program: {binary: \"bin/hello_world\", runner: \"foo\"}}",
        );
        let shard_path =
            ctx.new_include("shard.cml", "{use: [{ protocol: [\"fuchsia.foo.Bar\"]}]}");
        ctx.merge_includes(&cml_path).unwrap();

        ctx.assert_output_eq(json!({
            "program": {
                "binary": "bin/hello_world",
                "runner": "foo"
            },
            "use": [{
                "protocol": "fuchsia.foo.Bar"
            }]
        }));
        ctx.assert_depfile_eq(&ctx.output, &[&shard_path]);
    }

    #[test]
    fn test_include_cml_with_dictionary() {
        let ctx = TestContext::new();
        let cml_path = ctx.new_file(
            "some.cml",
            "{include: [\"shard.cml\"], program: {binary: \"bin/hello_world\", runner: \"foo\"}}",
        );
        let shard_path = ctx.new_include(
            "shard.cml",
            json!({
                "expose": [
                    {
                        "dictionary": "diagnostics",
                        "from": "self",
                    }
                ],
                "capabilities": [
                    {
                        "dictionary": "diagnostics",
                    }
                ]
            }),
        );
        ctx.merge_includes(&cml_path).unwrap();

        ctx.assert_output_eq(json!({
            "program": {
                "binary": "bin/hello_world",
                "runner": "foo"
            },
            "expose": [
                {
                    "dictionary": "diagnostics",
                    "from": "self",
                }
            ],
            "capabilities": [
                {
                    "dictionary": "diagnostics",
                }
            ]
        }));
        ctx.assert_depfile_eq(&ctx.output, &[&shard_path]);
    }

    #[test]
    fn test_include_cml_no_validation() {
        let ctx = TestContext::new();
        let cml_path = ctx.new_file(
            "some.cml",
            // A missing `program.runner` would normally fail validation.
            "{include: [\"shard.cml\"], program: {binary: \"bin/hello_world\"}}",
        );
        let shard_path =
            ctx.new_include("shard.cml", "{use: [{ protocol: [\"fuchsia.foo.Bar\"]}]}");
        ctx.merge_includes_no_validation(&cml_path).unwrap();

        ctx.assert_output_eq(json!({
            "program": {
                "binary": "bin/hello_world"
            },
            "use": [{
                "protocol": "fuchsia.foo.Bar"
            }]
        }));
        ctx.assert_depfile_eq(&ctx.output, &[&shard_path]);
    }

    #[test]
    fn test_include_cml_dup_protocol() {
        let ctx = TestContext::new();
        let cml_path =
            ctx.new_file("some.cml", "{include: [\"shard.cml\"], use: [{ protocol: [\"bar\"]}]}");
        let shard_path = ctx.new_include("shard.cml", "{use: [{ protocol: [\"foo\", \"bar\"]}]}");
        ctx.merge_includes(&cml_path).unwrap();

        ctx.assert_output_eq(json!({
            "use": [
                {
                    "protocol": [ "bar", "foo" ]
                }
            ]
        }));
        ctx.assert_depfile_eq(&ctx.output, &[&shard_path]);
    }

    #[test]
    fn test_include_cml_dup_protocol_string_variant() {
        let ctx = TestContext::new();
        let cml_path = ctx.new_file(
            "some.cml",
            json!({
                "include": ["shard.cml"],
                "use": [
                    {"protocol": "bar"}
                ]
            }),
        );
        let shard_path = ctx.new_include(
            "shard.cml",
            json!({
                "use": [{
                    "protocol": ["bar"]
                }]
            }),
        );
        ctx.merge_includes(&cml_path).unwrap();

        ctx.assert_output_eq(json!({
            "use": [{
                "protocol": "bar"
            }]
        }));
        ctx.assert_depfile_eq(&ctx.output, &[&shard_path]);
    }

    #[test]
    fn test_include_cml_dup_protocol_complex_string_variant() {
        let ctx = TestContext::new();
        let cml_path = ctx.new_file(
            "some.cml",
            json!({
                "include": ["shard.cml"],
                "use": [
                    {"protocol": "bar", "path": "/svc/foo"}
                ]
            }),
        );
        let shard_path = ctx.new_include(
            "shard.cml",
            json!({
                "use": [{
                    "protocol": ["bar"]
                }]
            }),
        );
        ctx.merge_includes(&cml_path).unwrap();

        ctx.assert_output_eq(json!({
            "use": [
                {
                    "path": "/svc/foo",
                    "protocol": "bar",
                },
                {
                    "protocol": "bar"
                }
            ]
        }));
        ctx.assert_depfile_eq(&ctx.output, &[&shard_path]);
    }

    #[test]
    fn test_include_paths_take_priority_by_order() {
        let ctx = TestContext::new();
        let cml_path = ctx.new_file(
            "some.cml",
            json!({
                "include": ["shard.cml"],
                "program": {
                    "binary": "bin/hello_world",
                    "runner": "foo"
                }
            }),
        );
        let shard_path = ctx.new_include(
            "shard.cml",
            json!({
                "use": [{
                    "protocol": "bar"
                }]
            }),
        );
        // This shard won't be included because the one above will match first
        ctx.new_include_secondary(
            "shard.cml",
            json!({
                "use": [{
                    "protocol": "qux"
                }]
            }),
        );
        ctx.merge_includes(&cml_path).unwrap();

        ctx.assert_output_eq(json!({
            "program": {
                "binary": "bin/hello_world",
                "runner": "foo"
            },
            "use": [{ "protocol": "bar" }]
        }));
        ctx.assert_depfile_eq(&ctx.output, &[&shard_path]);
    }

    #[test]
    fn test_include_absolute() {
        let ctx = TestContext::new();
        ctx.new_dir("path/to");
        let cml_path = ctx.new_include(
            "some.cml",
            json!({
                "include": ["//path/to/shard.cml"],
                "program": {
                    "binary": "bin/hello_world",
                    "runner": "foo"
                }
            }),
        );
        let shard_path = ctx.new_file(
            "path/to/shard.cml",
            json!({
                "use": [{
                    "protocol": "bar"
                }]
            }),
        );
        ctx.merge_includes(&cml_path).unwrap();

        ctx.assert_output_eq(json!({
            "program": {
                "binary": "bin/hello_world",
                "runner": "foo"
            },
            "use": [{ "protocol": "bar" }]
        }));
        ctx.assert_depfile_eq(&ctx.output, &[&shard_path]);
    }

    #[test]
    fn test_include_multiple_shards() {
        let ctx = TestContext::new();
        let cml_path = ctx.new_include(
            "some.cml",
            json!({
                "include": ["shard1.cml", "shard2.cml"],
                "program": {
                    "binary": "bin/hello_world",
                    "runner": "foo"
                }
            }),
        );
        let shard1_path = ctx.new_include(
            "shard1.cml",
            json!({
                "use": [{ "protocol": "bar" }]
            }),
        );
        let shard2_path = ctx.new_include(
            "shard2.cml",
            json!({
                "use": [{ "protocol": "qux" }]
            }),
        );
        ctx.merge_includes(&cml_path).unwrap();

        ctx.assert_output_eq(json!({
            "program": {
                "binary": "bin/hello_world",
                "runner": "foo"
            },
            "use": [{ "protocol": ["bar", "qux"] }]
        }));
        ctx.assert_depfile_eq(&ctx.output, &[&shard1_path, &shard2_path]);
    }

    #[test]
    fn test_include_recursively() {
        let ctx = TestContext::new();
        let cml_path = ctx.new_include(
            "some.cml",
            json!({
                "include": ["shard1.cml"],
                "program": {
                    "binary": "bin/hello_world",
                    "runner": "foo"
                }
            }),
        );
        let shard1_path = ctx.new_include(
            "shard1.cml",
            json!({
                "include": ["shard2.cml"],
                "use": [{
                    "protocol": "bar"
                }]
            }),
        );
        let shard2_path = ctx.new_include(
            "shard2.cml",
            json!({
                "use": [{
                    "protocol": "qux"
                }]
            }),
        );
        ctx.merge_includes(&cml_path).unwrap();

        ctx.assert_output_eq(json!({
            "program": {
                "binary": "bin/hello_world",
                "runner": "foo"
            },
            "use": [{ "protocol": ["bar", "qux"] }]
        }));
        ctx.assert_depfile_eq(&ctx.output, &[&shard1_path, &shard2_path]);
    }

    #[test]
    fn test_include_nothing() {
        let ctx = TestContext::new();
        let cml_path = ctx.new_include(
            "some.cml",
            json!({
                "include": [],
                "program": {
                    "binary": "bin/hello_world",
                    "runner": "foo"
                }
            }),
        );
        ctx.merge_includes(&cml_path).unwrap();

        ctx.assert_output_eq(json!({
            "program": {
                "binary": "bin/hello_world",
                "runner": "foo"
            }
        }));
        ctx.assert_depfile_eq(&ctx.output, &[]);
    }

    #[test]
    fn test_no_includes() {
        let ctx = TestContext::new();
        let cml_path = ctx.new_include(
            "some.cml",
            json!({
                "program": {
                    "binary": "bin/hello_world",
                    "runner": "foo"
                }
            }),
        );
        ctx.merge_includes(&cml_path).unwrap();

        ctx.assert_output_eq(json!({
            "program": {
                "binary": "bin/hello_world",
                "runner": "foo"
            }
        }));
        ctx.assert_depfile_eq(&ctx.output, &[]);
    }

    #[test]
    fn test_invalid_include() {
        let ctx = TestContext::new();
        let cml_path = ctx.new_include(
            "some.cml",
            json!({
                "include": ["doesnt_exist.cml"],
                "program": {
                    "binary": "bin/hello_world"
                }
            }),
        );
        let result = ctx.merge_includes(&cml_path);

        assert_matches!(result, Err(Error::ValidateContext { err, .. })
                        if err.starts_with("Internal error") && err.contains("doesnt_exist.cml"));
    }

    #[test]
    fn test_include_detect_cycle() {
        let ctx = TestContext::new();
        let cml_path = ctx.new_include(
            "some1.cml",
            json!({
                "include": ["some2.cml"],
            }),
        );
        ctx.new_include(
            "some2.cml",
            json!({
                "include": ["some1.cml"],
            }),
        );
        let result = ctx.merge_includes(&cml_path);
        assert_matches!(result, Err(Error::ValidateContext { err, .. }) if err.contains("Circular include detected"));
    }

    #[test]
    fn test_expect_nothing() {
        let ctx = TestContext::new();
        let cml1_path = ctx.new_include(
            "some1.cml",
            json!({
                "program": {
                    "binary": "bin/hello_world"
                }
            }),
        );
        assert_matches!(ctx.check_includes(&cml1_path, vec![]), Ok(()));
        // Don't generate depfile (or delete existing) if no includes found
        ctx.assert_no_depfile();

        let cml2_path = ctx.new_include(
            "some2.cml",
            json!({
                "include": [],
                "program": {
                    "binary": "bin/hello_world"
                }
            }),
        );
        assert_matches!(ctx.check_includes(&cml2_path, vec![]), Ok(()));

        let cml3_path = ctx.new_include(
            "some3.cml",
            json!({
                "include": [ "foo.cml" ],
                "program": {
                    "binary": "bin/hello_world"
                }
            }),
        );
        assert_matches!(ctx.check_includes(&cml3_path, vec![]), Ok(()));
    }

    #[test]
    fn test_expect_something_present() {
        let ctx = TestContext::new();
        let cml_path = ctx.new_include(
            "some.cml",
            json!({
                "include": [ "foo.cml", "bar.cml" ],
                "program": {
                    "binary": "bin/hello_world"
                }
            }),
        );
        let foo_path = ctx.new_include("foo.cml", json!({}));
        let bar_path = ctx.new_include("bar.cml", json!({}));
        assert_matches!(ctx.check_includes(&cml_path, vec!["bar.cml".into()]), Ok(()));
        // Note that inputs are sorted to keep depfile contents stable,
        // so bar.cml comes before foo.cml.
        ctx.assert_depfile_eq(&ctx.stamp, &[&bar_path, &foo_path, &cml_path]);
    }

    #[test]
    fn test_expect_something_missing() {
        let ctx = TestContext::new();
        let cml1_path = ctx.new_include(
            "some1.cml",
            json!({
                "include": [ "foo.cml", "bar.cml" ],
                "program": {
                    "binary": "bin/hello_world"
                }
            }),
        );
        ctx.new_include("foo.cml", json!({}));
        ctx.new_include("bar.cml", json!({}));
        assert_matches!(ctx.check_includes(&cml1_path, vec!["qux.cml".into()]),
                        Err(Error::Validate { filename, .. }) if filename == cml1_path.to_str().map(String::from));

        let cml2_path = ctx.new_include(
            "some2.cml",
            json!({
                // No includes
                "program": {
                    "binary": "bin/hello_world"
                }
            }),
        );
        assert_matches!(ctx.check_includes(&cml2_path, vec!["qux.cml".into()]),
                        Err(Error::Validate { filename, .. }) if filename == cml2_path.to_str().map(String::from));
    }

    #[test]
    fn test_expect_something_transitive() {
        let ctx = TestContext::new();
        let cml_path = ctx.new_include(
            "some.cml",
            json!({
                "include": [ "foo.cml" ],
                "program": {
                    "binary": "bin/hello_world"
                }
            }),
        );
        ctx.new_include("foo.cml", json!({"include": [ "bar.cml" ]}));
        ctx.new_include("bar.cml", json!({}));
        assert_matches!(ctx.check_includes(&cml_path, vec!["bar.cml".into()]), Ok(()));
    }

    #[test]
    fn test_expect_fromfile() {
        let ctx = TestContext::new();
        let cml_path = ctx.new_include(
            "some.cml",
            json!({
                "include": [ "foo.cml", "bar.cml" ],
                "program": {
                    "binary": "bin/hello_world"
                }
            }),
        );
        ctx.new_include("foo.cml", json!({}));
        ctx.new_include("bar.cml", json!({}));

        let mut fromfile = LineWriter::new(File::create(ctx.fromfile.clone()).unwrap());
        writeln!(fromfile, "foo.cml").unwrap();
        writeln!(fromfile, "bar.cml").unwrap();
        assert_matches!(ctx.check_includes(&cml_path, vec![]), Ok(()));

        // Add another include that's missing
        writeln!(fromfile, "qux.cml").unwrap();
        assert_matches!(ctx.check_includes(&cml_path, vec![]),
                        Err(Error::Validate { filename, .. }) if filename == cml_path.to_str().map(String::from));
    }
}
