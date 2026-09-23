// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! test_list_tool generates test-list.json.

use anyhow::{Context, Error, format_err};
use camino::{Utf8Path, Utf8PathBuf};
use clap::Parser;
use diagnostics_log_types::Severity;
use fidl::unpersist;
use fidl_fuchsia_component_decl::Component;
use fidl_fuchsia_data as fdata;
use maplit::btreeset;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::cmp::{Eq, PartialEq};
use std::collections::HashMap;
use std::fmt::Debug;
use std::fs;
use std::io::Read;
use test_list::{ExecutionEntry, FuchsiaComponentExecutionEntry, TestList, TestListEntry, TestTag};

const META_FAR_PREFIX: &'static str = "meta/";
const TEST_REALM_FACET_NAME: &'static str = "fuchsia.test.type";
const TEST_DEPRECATED_ALLOWED_PACKAGES_FACET_KEY: &'static str =
    "fuchsia.test.deprecated-allowed-packages";
const HERMETIC_TEST_REALM: &'static str = "hermetic";

const SUITE_PROTOCOL: &'static str = "fuchsia.test.Suite";

mod error;
mod opts;
mod test_config;

#[derive(Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TestsJsonEntry {
    test: TestEntry,
}

#[derive(Debug, Eq, PartialEq, Serialize, Deserialize, Default)]
struct TestEntry {
    name: String,
    label: String,
    cpu: String,
    os: String,
    package_url: Option<String>,
    component_label: Option<String>,
    package_label: Option<String>,
    package_manifests: Option<Vec<String>>,
    log_settings: Option<LogSettings>,
    build_rule: Option<String>,
    has_generated_manifest: Option<bool>,
    create_no_exception_channel: Option<bool>,
    test_filters: Option<Vec<String>>,
}

#[derive(Debug, Eq, PartialEq, Serialize, Deserialize)]
struct LogSettings {
    max_severity: Option<Severity>,
    min_severity: Option<Severity>,
}
#[derive(Debug, Eq, PartialEq, Deserialize)]
struct CtfDisableSuite {
    name: String,
    compact_name: Option<String>,
    cases: Vec<CtfDisableCase>,
}

#[derive(Debug, Eq, PartialEq, Deserialize)]
struct CtfDisableCase {
    name: String,
    disabled: bool,
    bug: String,
}

type DisableSuiteMap = HashMap<String, Vec<CtfDisableCase>>;

fn ctf_suites_into_map(suites: Vec<CtfDisableSuite>) -> DisableSuiteMap {
    suites.into_iter().map(|suite| (suite.name.clone(), suite.cases)).collect()
}

#[derive(Debug, Eq, PartialEq, Serialize, Deserialize, Clone)]
struct TestComponentsJsonEntry {
    test_component: TestComponentEntry,
}

#[derive(Debug, Eq, PartialEq, Serialize, Deserialize, Default, Clone)]
struct TestComponentEntry {
    label: String,
    moniker: Option<String>,
}

impl TestComponentsJsonEntry {
    fn convert_to_map(
        value: Vec<TestComponentsJsonEntry>,
    ) -> Result<HashMap<String, TestComponentsJsonEntry>, Error> {
        let mut map = HashMap::<String, TestComponentsJsonEntry>::default();
        for entry in value {
            if let Some(old_entry) = map.get(&entry.test_component.label) {
                if !old_entry.eq(&entry) {
                    return Err(format_err!(
                        "Conflicting test components: {:?}, {:?}",
                        old_entry,
                        entry
                    ));
                }
            } else {
                map.insert(entry.test_component.label.clone(), entry);
            }
        }
        Ok(map)
    }
}

/// This struct contains the set of tags that can be added to tests in the fuchsia.git build.
#[derive(Default, Debug, PartialEq, Eq)]
struct FuchsiaTestTags {
    os: Option<String>,
    cpu: Option<String>,
    scope: Option<String>,
    realm: Option<String>,
    hermetic: Option<bool>,
    legacy_test: Option<bool>,
    package_label: Option<String>,
}

impl FuchsiaTestTags {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn into_vec(self) -> Vec<TestTag> {
        let string_to_val = |v: Option<String>| v.unwrap_or_else(|| "".to_string());
        let bool_to_val = |v: Option<bool>| match v {
            None => "".to_string(),
            Some(true) => "true".to_string(),
            Some(false) => "false".to_string(),
        };
        btreeset![
            TestTag { key: "os".to_string(), value: string_to_val(self.os) },
            TestTag { key: "cpu".to_string(), value: string_to_val(self.cpu) },
            TestTag { key: "scope".to_string(), value: string_to_val(self.scope) },
            TestTag { key: "realm".to_string(), value: string_to_val(self.realm) },
            TestTag { key: "hermetic".to_string(), value: bool_to_val(self.hermetic) },
            TestTag { key: "legacy_test".to_string(), value: bool_to_val(self.legacy_test) },
            TestTag { key: "package_label".to_string(), value: string_to_val(self.package_label) }
        ]
        .into_iter()
        .collect()
    }
}

impl TestEntry {
    fn get_max_log_severity(&self) -> Option<Severity> {
        self.log_settings.as_ref().map(|settings| settings.max_severity).unwrap_or(None)
    }
    fn get_min_log_severity(&self) -> Option<Severity> {
        self.log_settings.as_ref().map(|settings| settings.min_severity).unwrap_or(None)
    }
}

fn find_meta_far(build_dir: &Utf8Path, manifest_path: String) -> Result<Utf8PathBuf, Error> {
    let package_manifest =
        fuchsia_pkg::PackageManifest::try_load_from(build_dir.join(&manifest_path))?;

    for blob in package_manifest.blobs() {
        if blob.path.eq(META_FAR_PREFIX) {
            // When blob sources are relative to the manifest file, the parsing
            // operation rebases the path and changes the blob_sources_relative value
            // back to "working_dir", which already prefixes it with the build directory.
            //
            // This code skips prefixing build directory if it is already present.
            let source_path = Utf8PathBuf::from(&blob.source_path);
            return if source_path.starts_with(&build_dir) {
                Ok(source_path)
            } else {
                Ok(build_dir.join(source_path))
            };
        }
    }
    Err(error::TestListToolError::MissingMetaBlob(manifest_path).into())
}

fn cm_decl_from_meta_far(meta_far_path: &Utf8PathBuf, cm_path: &str) -> Result<Component, Error> {
    let mut meta_far = fs::File::open(meta_far_path)
        .with_context(|| format!("attempted to open {meta_far_path}"))?;
    let mut far_reader = fuchsia_archive::Utf8Reader::new(&mut meta_far)?;
    let cm_contents = far_reader.read_file(cm_path)?;
    let decl: Component = unpersist(&cm_contents)?;
    Ok(decl)
}

fn update_tags_from_facets(
    test_tags: &mut FuchsiaTestTags,
    facets: &fdata::Dictionary,
) -> Result<(), Error> {
    let mut runs_in_hermetic_realm = test_tags.realm.is_none();
    let mut depends_on_system_packages = false;
    for facet in facets.entries.as_ref().unwrap_or(&vec![]) {
        // TODO(rudymathu): CFv1 tests should not have a hermetic tag.
        if facet.key.eq(TEST_REALM_FACET_NAME) && test_tags.realm.is_none() {
            let val = facet
                .value
                .as_ref()
                .ok_or_else(|| error::TestListToolError::NullFacet(facet.key.clone()))?;
            match &**val {
                fdata::DictionaryValue::Str(s) => {
                    test_tags.realm = Some(s.to_string());
                    runs_in_hermetic_realm = s.eq(HERMETIC_TEST_REALM);
                }
                _ => {
                    return Err(error::TestListToolError::InvalidFacetValue(
                        facet.key.clone(),
                        format!("{:?}", val),
                    )
                    .into());
                }
            }
        } else if facet.key.eq(TEST_DEPRECATED_ALLOWED_PACKAGES_FACET_KEY) {
            let val = facet
                .value
                .as_ref()
                .ok_or_else(|| error::TestListToolError::NullFacet(facet.key.clone()))?;
            match &**val {
                fdata::DictionaryValue::StrVec(s) => {
                    depends_on_system_packages = !s.is_empty();
                }
                _ => {
                    return Err(error::TestListToolError::InvalidFacetValue(
                        facet.key.clone(),
                        format!("{:?}", val),
                    )
                    .into());
                }
            }
        }
    }
    if test_tags.realm.is_none() {
        test_tags.realm = Some(HERMETIC_TEST_REALM.to_string());
    }
    // Only hermetic if the test is executed in `hermetic` realm and does not depend on any system
    // packages.
    test_tags.hermetic = Some(!depends_on_system_packages && runs_in_hermetic_realm);

    Ok(())
}

struct ToTestListEntryArgs<'a, 'b> {
    test_entry: &'a TestEntry,
    realm: Option<String>,
    disable_suites: &'b DisableSuiteMap,
}

fn to_test_list_entry(args: ToTestListEntryArgs<'_, '_>) -> TestListEntry {
    let ToTestListEntryArgs { test_entry, realm, disable_suites } = args;
    let execution = match &test_entry.package_url {
        Some(url) => {
            let mut filters = test_entry.test_filters.clone().unwrap_or_default();
            if let Some(cases) = disable_suites.get(url) {
                for case in cases {
                    if case.disabled {
                        filters.push(format!("-{}", case.name));
                    }
                }
            }
            let (filters, no_cases_equals_success) = match filters.len() {
                0 => (None, None),
                _ => (Some(filters), Some(true)),
            };
            Some(ExecutionEntry::FuchsiaComponent(FuchsiaComponentExecutionEntry {
                component_url: url.to_string(),
                test_args: vec![],
                timeout_seconds: None,
                test_filters: filters,
                no_cases_equals_success,
                also_run_disabled_tests: false,
                parallel: None,
                max_severity_logs: test_entry.get_max_log_severity(),
                min_severity_logs: test_entry.get_min_log_severity(),
                realm: realm,
                create_no_exception_channel: test_entry
                    .create_no_exception_channel
                    .unwrap_or(false),
            }))
        }
        None => None,
    };

    TestListEntry {
        name: test_entry.name.clone(),
        labels: vec![test_entry.label.clone()],
        tags: vec![],
        execution,
    }
}

fn update_tags_with_test_entry(tags: &mut FuchsiaTestTags, test_entry: &TestEntry) {
    let realm = &tags.realm;
    let (build_rule, has_generated_manifest) = (
        test_entry.build_rule.as_ref().map(|s| s.as_str()),
        test_entry.has_generated_manifest.unwrap_or_default(),
    );

    // Determine the type of a test as follows:
    // - A "host" test is run from a host binary.
    // - A "unit" test is in a hermetic realm with a generated manifest.
    // - An "integration" test is in a hermetic realm with a custom manifest.
    // - A "system" test is any test in a non-hermetic realm.
    // - "unknown" means we do not have knowledge of the build rule being used.
    // - "uncategorized" means we otherwise could not match the test.

    let test_type =
        if test_entry.os.to_lowercase() == "linux" || test_entry.os.to_lowercase() == "mac" {
            "host"
        } else {
            match (build_rule, has_generated_manifest, realm) {
                (_, _, Some(realm)) if realm != HERMETIC_TEST_REALM => "system",
                (Some("fuchsia_unittest_package"), true, _) => "unit",
                (Some("fuchsia_unittest_package"), false, _) => "integration",
                (Some("fuchsia_test_package"), true, _) => "unit",
                (Some("fuchsia_test_package"), false, _) => "integration",
                (Some("fuchsia_test"), true, _) => "unit",
                (Some("fuchsia_test"), false, _) => "integration",
                (Some("prebuilt_test_package"), _, _) => "prebuilt",
                (Some("fuchsia_fuzzer_package"), _, _) => "fuzzer",
                (Some("fuzzer_package"), _, _) => "fuzzer",
                (Some("bootfs_test"), _, _) => "bootfs",
                (None, _, _) => "unknown",
                _ => "uncategorized",
            }
        };

    tags.os = Some(test_entry.os.clone());
    tags.cpu = Some(test_entry.cpu.clone());
    tags.scope = Some(test_type.to_string());
    tags.package_label = test_entry.package_label.clone();
}

fn main() -> Result<(), Error> {
    run_tool()
}

fn read_tests_json(file: &Utf8PathBuf) -> Result<Vec<TestsJsonEntry>, Error> {
    let mut buffer = String::new();
    fs::File::open(&file)?.read_to_string(&mut buffer)?;
    let t: Vec<TestsJsonEntry> = serde_json::from_str(&buffer)?;
    Ok(t)
}

fn read_ctf_disable(file: &Option<Utf8PathBuf>) -> Result<Option<Vec<CtfDisableSuite>>, Error> {
    match file {
        None => Ok(None),
        Some(file) => {
            let mut buffer = String::new();
            fs::File::open(&file)?.read_to_string(&mut buffer)?;
            let t: Vec<CtfDisableSuite> = serde_json::from_str(&buffer)?;
            Ok(Some(t))
        }
    }
}

fn read_test_components_json(file: &Utf8PathBuf) -> Result<Vec<TestComponentsJsonEntry>, Error> {
    let mut buffer = String::new();
    fs::File::open(&file)?.read_to_string(&mut buffer)?;
    let t: Vec<TestComponentsJsonEntry> = serde_json::from_str(&buffer)?;
    Ok(t)
}

fn write_depfile(
    depfile: &Utf8PathBuf,
    output: &Utf8PathBuf,
    inputs: &Vec<Utf8PathBuf>,
) -> Result<(), Error> {
    if inputs.len() == 0 {
        return Ok(());
    }
    #[allow(clippy::format_collect, reason = "mass allow for https://fxbug.dev/381896734")]
    let contents =
        format!("{}: {}\n", output, inputs.iter().map(|i| format!(" {}", i)).collect::<String>(),);
    if let Some(depfile_dir) = depfile.parent() {
        std::fs::create_dir_all(depfile_dir)
            .with_context(|| format!("Creating directory for depfile: {}", depfile))?;
    }
    fs::write(depfile, contents).with_context(|| format!("Writing depfile to: {}", depfile))?;
    Ok(())
}

fn validate_test_decl(decl: &Component) -> Result<(), Error> {
    if let Some(exposes) = &decl.exposes {
        if exposes.iter().any(|e| match e {
            fidl_fuchsia_component_decl::Expose::Protocol(ep) => {
                ep.target_name.eq(&Some(SUITE_PROTOCOL.to_string()))
            }
            _ => false,
        }) {
            return Ok(());
        }
    }
    Err(format_err!("Does not expose {} protocol.
Refer to https://fuchsia.dev/fuchsia-src/development/testing/components/test_runner_framework?hl=en#troubleshoot-test-root", SUITE_PROTOCOL))
}

fn validate_and_get_test_cml(
    component_url: &str,
    meta_far_path: &Utf8PathBuf,
) -> Result<Component, Error> {
    let cm_path = fuchsia_url::AbsoluteComponentUrl::parse(&component_url)?.resource().to_string();
    let decl = match cm_decl_from_meta_far(&meta_far_path, &cm_path) {
        Ok(decl) => decl,
        Err(e) => {
            return Err(format_err!("Error retrieving manifest for {}: {}", component_url, e));
        }
    };
    if let Err(e) = validate_test_decl(&decl) {
        return Err(format_err!("Error validating manifest for {}: {}", component_url, e));
    }
    Ok(decl)
}

fn run_tool() -> Result<(), Error> {
    let opt = opts::Opt::parse();
    opt.validate()?;
    if opt.single_threaded {
        rayon::ThreadPoolBuilder::new().num_threads(1).build_global()?;
    }

    let tests_json = read_tests_json(&opt.input)?;
    let ctf_disable_map = match read_ctf_disable(&opt.disabled_ctf_tests)? {
        None => HashMap::new(),
        Some(suites) => ctf_suites_into_map(suites),
    };
    let test_components_json = read_test_components_json(&opt.test_components_list)?;
    let test_components_map = TestComponentsJsonEntry::convert_to_map(test_components_json)?;

    let (successes, errors): (Vec<_>, Vec<_>) = tests_json
        .par_iter()
        .map(|entry| {
            let realm = match &entry.test.component_label {
                Some(component_label) => match test_components_map.get(component_label) {
                    Some(test_component) => test_component.test_component.moniker.clone(),
                    None => None,
                },
                None => None,
            };
            // Construct the base TestListEntry.
            let mut test_list_entry = to_test_list_entry(ToTestListEntryArgs {
                test_entry: &entry.test,
                realm: realm.clone(),
                disable_suites: &ctf_disable_map,
            });
            let mut test_tags = FuchsiaTestTags::new();
            let pkg_manifests = entry.test.package_manifests.clone().unwrap_or(vec![]);
            test_tags.realm = realm;
            // Aggregate any tags from the component manifest of the test.
            let mut inputs: Vec<Utf8PathBuf> = vec![];
            if entry.test.package_url.is_some() && pkg_manifests.len() > 0 {
                let pkg_url = entry.test.package_url.clone().unwrap();
                let pkg_manifest = pkg_manifests[0].clone();
                inputs.push(pkg_manifest.clone().into());

                let res = find_meta_far(&opt.build_dir, pkg_manifest.clone());
                if res.is_err() {
                    return Err(format_err!(
                        "error finding meta.far file in package manifest {}: {:?}",
                        pkg_manifest,
                        res.unwrap_err()
                    ));
                }
                let meta_far_path = res.unwrap();
                inputs.push(meta_far_path.clone());

                let decl = validate_and_get_test_cml(&pkg_url, &meta_far_path)?;

                // Find additional tags. Note that this can override existing tags (e.g. to set the test type based on realm)
                if let Err(e) = update_tags_from_facets(
                    &mut test_tags,
                    &decl.facets.unwrap_or_else(fdata::Dictionary::default),
                ) {
                    return Err(format_err!(
                        "error processing manifest for package URL {}: {:?}",
                        pkg_url,
                        e
                    ));
                }
            }

            update_tags_with_test_entry(&mut test_tags, &entry.test);
            test_list_entry.tags = test_tags.into_vec();
            let mut config = None;
            if opt.test_config_output.is_some() {
                if let Some(ExecutionEntry::FuchsiaComponent(execution_entry)) =
                    &test_list_entry.execution
                {
                    config = Some(test_config::create_test_config_entry(
                        test_list_entry.tags.clone(),
                        &entry,
                        execution_entry,
                    ));
                }
            }

            Ok(((Some(test_list_entry), config), inputs))
        })
        .partition(Result::is_ok);

    if !errors.is_empty() && !opt.ignore_device_test_errors {
        let error_message: String = errors
            .into_iter()
            .map(Result::unwrap_err)
            .map(|err| err.to_string())
            .collect::<Vec<String>>()
            .join("\n");
        return Err(format_err!("{}", error_message));
    }

    let (data, inputs): (
        Vec<(Option<TestListEntry>, Option<test_config::TestConfig>)>,
        Vec<Vec<Utf8PathBuf>>,
    ) = successes.into_iter().map(Result::unwrap).unzip();

    let (test_list_entries, test_configs): (
        Vec<Option<TestListEntry>>,
        Vec<Option<test_config::TestConfig>>,
    ) = data.into_par_iter().unzip();

    let test_list_entries = test_list_entries.into_par_iter().flatten().collect();
    let inputs = inputs.into_par_iter().flatten().collect();

    let test_list = TestList::Experimental { data: test_list_entries };
    let test_list_json = serde_json::to_string_pretty(&test_list)?;
    fs::write(&opt.output, test_list_json)?;
    if let Some(depfile) = opt.depfile {
        write_depfile(&depfile, &opt.output, &inputs)?;
    }
    if let Some(outfile) = opt.test_config_output {
        let test_configs: Vec<test_config::TestConfig> =
            test_configs.into_par_iter().flatten().collect();
        let test_config_json = serde_json::to_string_pretty(&test_configs)?;
        fs::write(outfile, test_config_json)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use fidl_fuchsia_component_decl as fdecl;
    use tempfile::tempdir;

    #[test]
    fn test_find_meta_far_default() {
        // By default, meta.far paths are relative to working directories.
        let tmp = tempdir().expect("failed to get tempdir");
        let build_dir = Utf8Path::from_path(tmp.path()).unwrap();
        let package_manifest_path = "package_manifest.json";

        // Test the working case.
        let contents = r#"
            {
                "version": "1",
                "repository": "fuchsia.com",
                "package": {
                    "name": "echo-integration-test",
                    "version": "0"
                },
                "blobs": [
                    {
                        "source_path": "obj/build/components/tests/echo-integration-test/meta.far",
                        "path": "meta/",
                        "merkle": "0ec72cdf55fec3e0cc3dd47e86b95ee62c974ebaebea1d05769fea3fc4edca0b",
                        "size": 36864
                    }
                ]
            }"#;
        fs::write(build_dir.join(package_manifest_path), contents)
            .expect("failed to write fake package manifest");
        assert_eq!(
            find_meta_far(&build_dir.to_path_buf(), package_manifest_path.into()).unwrap(),
            build_dir.join("obj/build/components/tests/echo-integration-test/meta.far"),
        );
    }

    #[test]
    fn test_find_meta_far_relative_to_file() {
        // Try to load a meta.far that is relative to the manifest file rather than relative to working directory.
        let tmp = tempdir().expect("failed to get tempdir");
        let build_dir = Utf8Path::from_path(tmp.path()).unwrap();
        let package_manifest_dir = Utf8PathBuf::from("out/package/foo");
        let package_manifest_path = package_manifest_dir.join("package_manifest.json");
        std::fs::create_dir_all(build_dir.join(&package_manifest_dir))
            .expect("create test directory");

        // Test the working case.
        let contents = r#"
            {
                "version": "1",
                "repository": "fuchsia.com",
                "package": {
                    "name": "echo-integration-test",
                    "version": "0"
                },
                "blobs": [
                    {
                        "source_path": "blobs/meta.far",
                        "path": "meta/",
                        "merkle": "0ec72cdf55fec3e0cc3dd47e86b95ee62c974ebaebea1d05769fea3fc4edca0b",
                        "size": 36864
                    }
                ],
                "blob_sources_relative": "file"
            }"#;
        fs::write(build_dir.join(&package_manifest_path), contents)
            .expect("failed to write fake package manifest");
        assert_eq!(
            find_meta_far(&build_dir.to_path_buf(), package_manifest_path.into()).unwrap(),
            build_dir.join("out/package/foo/blobs/meta.far"),
        );
    }

    #[test]
    fn test_find_meta_far_error() {
        let tmp = tempdir().expect("failed to get tempdir");
        let build_dir = Utf8Path::from_path(tmp.path()).unwrap();
        let package_manifest_path = "package_manifest.json";

        // Test the error case.
        let contents = r#"
            {
                "version": "1",
                "repository": "fuchsia.com",
                "package": {
                    "name": "echo-integration-test",
                    "version": "0"
                },
                "blobs": []
            }"#;
        fs::write(build_dir.join(package_manifest_path), contents)
            .expect("failed to write fake package manifest");
        let err = find_meta_far(&build_dir.to_path_buf(), package_manifest_path.into())
            .expect_err("find_meta_far failed unexpectedly");
        match err.downcast_ref::<error::TestListToolError>() {
            Some(error::TestListToolError::MissingMetaBlob(path)) => {
                assert_eq!(package_manifest_path.to_string(), *path)
            }
            Some(e) => panic!("find_meta_far returned incorrect TestListToolError: {:?}", e),
            None => panic!("find_meta_far returned non TestListToolError: {:?}", err),
        }
    }

    #[test]
    fn test_update_tags_from_facets() {
        // Test that empty facets return the hermetic tags.
        let mut facets = fdata::Dictionary::default();
        let mut tags = FuchsiaTestTags::default();
        update_tags_from_facets(&mut tags, &facets).expect("failed get tags in tags_from_facets");
        let hermetic_tags = FuchsiaTestTags {
            realm: Some(HERMETIC_TEST_REALM.to_string()),
            hermetic: Some(true),
            ..FuchsiaTestTags::default()
        };
        assert_eq!(tags, hermetic_tags);

        // Test that a facet of fuchsia.test: tests returns hermetic tags.
        let mut tags = FuchsiaTestTags::default();
        facets.entries = Some(vec![fdata::DictionaryEntry {
            key: TEST_REALM_FACET_NAME.to_string(),
            value: Some(Box::new(fdata::DictionaryValue::Str(HERMETIC_TEST_REALM.to_string()))),
        }]);
        update_tags_from_facets(&mut tags, &facets).expect("failed get tags in tags_from_facets");
        assert_eq!(tags, hermetic_tags);

        // Test that a null fuchsia.test facet returns a NullFacet error.
        let mut tags = FuchsiaTestTags::default();
        facets.entries = Some(vec![fdata::DictionaryEntry {
            key: TEST_REALM_FACET_NAME.to_string(),
            value: None,
        }]);
        let err = update_tags_from_facets(&mut tags, &facets)
            .expect_err("tags_from_facets succeeded on null facet value");
        match err.downcast_ref::<error::TestListToolError>() {
            Some(error::TestListToolError::NullFacet(key)) => {
                assert_eq!(*key, TEST_REALM_FACET_NAME.to_string());
            }
            Some(e) => panic!("tags_from_facets returned incorrect TestListToolError: {:?}", e),
            None => panic!("tags_from_facets returned non-TestListToolError: {:?}", err),
        }

        // Test that an invalid fuchsia.test facet returns an InvalidFacetValue error.
        let mut tags = FuchsiaTestTags::default();
        facets.entries = Some(vec![fdata::DictionaryEntry {
            key: TEST_REALM_FACET_NAME.to_string(),
            value: Some(Box::new(fdata::DictionaryValue::StrVec(vec![
                HERMETIC_TEST_REALM.to_string(),
            ]))),
        }]);
        let err = update_tags_from_facets(&mut tags, &facets)
            .expect_err("tags_from_facets succeeded on null facet value");
        match err.downcast_ref::<error::TestListToolError>() {
            Some(error::TestListToolError::InvalidFacetValue(k, _)) => {
                assert_eq!(*k, TEST_REALM_FACET_NAME.to_string());
            }
            Some(e) => panic!("tags_from_facets returned incorrect TestListToolError: {:?}", e),
            None => panic!("tags_from_facets returned non-TestListToolError: {:?}", err),
        }

        // test that hermetic tag depends on allowed_package_list
        let mut tags = FuchsiaTestTags::default();
        facets.entries = Some(vec![fdata::DictionaryEntry {
            key: TEST_DEPRECATED_ALLOWED_PACKAGES_FACET_KEY.to_string(),
            value: Some(Box::new(fdata::DictionaryValue::StrVec(vec![]))),
        }]);
        update_tags_from_facets(&mut tags, &facets).expect("failed get tags in tags_from_facets");
        assert_eq!(tags, hermetic_tags);

        let mut tags = FuchsiaTestTags::default();
        facets.entries = Some(vec![fdata::DictionaryEntry {
            key: TEST_DEPRECATED_ALLOWED_PACKAGES_FACET_KEY.to_string(),
            value: Some(Box::new(fdata::DictionaryValue::StrVec(vec!["some-pkg".to_string()]))),
        }]);
        update_tags_from_facets(&mut tags, &facets).expect("failed get tags in tags_from_facets");
        let non_hermetic_tags = FuchsiaTestTags {
            realm: Some(HERMETIC_TEST_REALM.to_string()),
            hermetic: Some(false),
            ..FuchsiaTestTags::default()
        };
        assert_eq!(tags, non_hermetic_tags);

        let mut tags = FuchsiaTestTags::default();
        facets.entries = Some(vec![
            fdata::DictionaryEntry {
                key: TEST_DEPRECATED_ALLOWED_PACKAGES_FACET_KEY.to_string(),
                value: Some(Box::new(fdata::DictionaryValue::StrVec(vec!["some-pkg".to_string()]))),
            },
            fdata::DictionaryEntry {
                key: TEST_REALM_FACET_NAME.to_string(),
                value: Some(Box::new(fdata::DictionaryValue::Str(HERMETIC_TEST_REALM.to_string()))),
            },
        ]);
        update_tags_from_facets(&mut tags, &facets).expect("failed get tags in tags_from_facets");
        assert_eq!(tags, non_hermetic_tags);

        let mut tags = FuchsiaTestTags::default();
        facets.entries = Some(vec![
            fdata::DictionaryEntry {
                key: TEST_DEPRECATED_ALLOWED_PACKAGES_FACET_KEY.to_string(),
                value: Some(Box::new(fdata::DictionaryValue::StrVec(vec![]))),
            },
            fdata::DictionaryEntry {
                key: TEST_REALM_FACET_NAME.to_string(),
                value: Some(Box::new(fdata::DictionaryValue::Str(HERMETIC_TEST_REALM.to_string()))),
            },
        ]);
        update_tags_from_facets(&mut tags, &facets).expect("failed get tags in tags_from_facets");
        assert_eq!(tags, hermetic_tags);

        // test with other collections
        let mut tags = FuchsiaTestTags::default();
        facets.entries = Some(vec![fdata::DictionaryEntry {
            key: TEST_REALM_FACET_NAME.to_string(),
            value: Some(Box::new(fdata::DictionaryValue::Str("other_collection".to_string()))),
        }]);
        update_tags_from_facets(&mut tags, &facets).expect("failed get tags in tags_from_facets");
        let non_hermetic_tags = FuchsiaTestTags {
            realm: Some("other_collection".to_string()),
            hermetic: Some(false),
            ..FuchsiaTestTags::default()
        };
        assert_eq!(tags, non_hermetic_tags);

        let mut tags = FuchsiaTestTags::default();
        facets.entries = Some(vec![
            fdata::DictionaryEntry {
                key: TEST_REALM_FACET_NAME.to_string(),
                value: Some(Box::new(fdata::DictionaryValue::Str("other_collection".to_string()))),
            },
            fdata::DictionaryEntry {
                key: TEST_DEPRECATED_ALLOWED_PACKAGES_FACET_KEY.to_string(),
                value: Some(Box::new(fdata::DictionaryValue::StrVec(vec![]))),
            },
        ]);
        update_tags_from_facets(&mut tags, &facets).expect("failed get tags in tags_from_facets");
        assert_eq!(tags, non_hermetic_tags);

        let mut tags = FuchsiaTestTags::default();
        tags.realm = Some("/some/moniker".into());
        facets.entries = Some(vec![
            fdata::DictionaryEntry {
                key: TEST_REALM_FACET_NAME.to_string(),
                value: Some(Box::new(fdata::DictionaryValue::Str("other_collection".to_string()))),
            },
            fdata::DictionaryEntry {
                key: TEST_DEPRECATED_ALLOWED_PACKAGES_FACET_KEY.to_string(),
                value: Some(Box::new(fdata::DictionaryValue::StrVec(vec![]))),
            },
        ]);
        update_tags_from_facets(&mut tags, &facets).expect("failed get tags in tags_from_facets");
        assert_eq!(tags.hermetic, Some(false));
        assert_eq!(tags.realm, Some("/some/moniker".into()));
    }

    #[test]
    fn test_to_test_list_entry() {
        let make_test_entry = |log_settings, create_no_exception_channel| TestEntry {
            name: "test-name".to_string(),
            label: "test-label".to_string(),
            cpu: "x64".to_string(),
            os: "fuchsia".to_string(),
            package_url: Some(
                "fuchsia-pkg://fuchsia.com/echo-integration-test#meta/echo-client-test.cm"
                    .to_string(),
            ),
            package_manifests: Some(vec![
                "obj/build/components/tests/echo-integration-test/package_manifest.json"
                    .to_string(),
            ]),
            log_settings,
            create_no_exception_channel,
            ..TestEntry::default()
        };

        let host_test_entry = TestEntry {
            name: "test-name".to_string(),
            label: "test-label".to_string(),
            cpu: "x64".to_string(),
            os: "linux".to_string(),
            log_settings: None,
            package_url: None,
            package_manifests: None,
            ..TestEntry::default()
        };

        let make_expected_test_list_entry =
            |max_severity_logs, min_severity_logs, create_no_exception_channel| TestListEntry {
                name: "test-name".to_string(),
                labels: vec!["test-label".to_string()],
                tags: vec![],
                execution: Some(ExecutionEntry::FuchsiaComponent(FuchsiaComponentExecutionEntry {
                    component_url:
                        "fuchsia-pkg://fuchsia.com/echo-integration-test#meta/echo-client-test.cm"
                            .to_string(),
                    test_args: vec![],
                    timeout_seconds: None,
                    test_filters: None,
                    no_cases_equals_success: None,
                    also_run_disabled_tests: false,
                    parallel: None,
                    max_severity_logs,
                    min_severity_logs,
                    realm: None,
                    create_no_exception_channel,
                })),
            };

        // Default severity.
        let test_list_entry = to_test_list_entry(ToTestListEntryArgs {
            test_entry: &make_test_entry(None, None),
            realm: None,
            disable_suites: &HashMap::new(),
        });
        assert_eq!(test_list_entry, make_expected_test_list_entry(None, None, false),);

        // Inner default severity.
        let test_list_entry = to_test_list_entry(ToTestListEntryArgs {
            test_entry: &make_test_entry(
                Some(LogSettings { max_severity: None, min_severity: None }),
                None,
            ),

            realm: None,
            disable_suites: &HashMap::new(),
        });
        assert_eq!(test_list_entry, make_expected_test_list_entry(None, None, false),);

        // Explicit severity
        let test_list_entry = to_test_list_entry(ToTestListEntryArgs {
            test_entry: &make_test_entry(
                Some(LogSettings { max_severity: Some(Severity::Error), min_severity: None }),
                None,
            ),

            realm: None,
            disable_suites: &HashMap::new(),
        });
        assert_eq!(
            test_list_entry,
            make_expected_test_list_entry(Some(Severity::Error), None, false)
        );

        // Explicit create_no_exception_channel.
        let test_list_entry = to_test_list_entry(ToTestListEntryArgs {
            test_entry: &make_test_entry(None, Some(true)),
            realm: None,
            disable_suites: &HashMap::new(),
        });
        assert_eq!(test_list_entry, make_expected_test_list_entry(None, None, true),);
        let test_list_entry = to_test_list_entry(ToTestListEntryArgs {
            test_entry: &make_test_entry(None, Some(false)),
            realm: None,
            disable_suites: &HashMap::new(),
        });
        assert_eq!(test_list_entry, make_expected_test_list_entry(None, None, false),);

        // pass in realm
        let test_list_entry = to_test_list_entry(ToTestListEntryArgs {
            test_entry: &make_test_entry(None, None),
            realm: Some("/some/moniker".into()),
            disable_suites: &HashMap::new(),
        });
        let expected_list = TestListEntry {
            name: "test-name".to_string(),
            labels: vec!["test-label".to_string()],
            tags: vec![],
            execution: Some(ExecutionEntry::FuchsiaComponent(FuchsiaComponentExecutionEntry {
                component_url:
                    "fuchsia-pkg://fuchsia.com/echo-integration-test#meta/echo-client-test.cm"
                        .to_string(),
                test_args: vec![],
                timeout_seconds: None,
                test_filters: None,
                no_cases_equals_success: None,
                also_run_disabled_tests: false,
                parallel: None,
                max_severity_logs: None,
                min_severity_logs: None,
                realm: Some("/some/moniker".into()),
                create_no_exception_channel: false,
            })),
        };
        assert_eq!(test_list_entry, expected_list);

        // Host test
        let test_list_entry = to_test_list_entry(ToTestListEntryArgs {
            test_entry: &host_test_entry,
            realm: None,
            disable_suites: &HashMap::new(),
        });
        assert_eq!(
            test_list_entry,
            TestListEntry {
                name: "test-name".to_string(),
                labels: vec!["test-label".to_string()],
                tags: vec![],
                execution: None,
            }
        )
    }

    #[test]
    fn test_update_tags_from_test_entry() {
        let cases = vec![
            (
                TestEntry {
                    cpu: "x64".to_string(),
                    os: "fuchsia".to_string(),
                    ..TestEntry::default()
                },
                FuchsiaTestTags { ..FuchsiaTestTags::default() },
                vec![
                    TestTag { key: "cpu".to_string(), value: "x64".to_string() },
                    TestTag { key: "hermetic".to_string(), value: "".to_string() },
                    TestTag { key: "legacy_test".to_string(), value: "".to_string() },
                    TestTag { key: "os".to_string(), value: "fuchsia".to_string() },
                    TestTag { key: "package_label".to_string(), value: "".to_string() },
                    TestTag { key: "realm".to_string(), value: "".to_string() },
                    TestTag { key: "scope".to_string(), value: "unknown".to_string() },
                ],
            ),
            (
                TestEntry {
                    cpu: "x64".to_string(),
                    os: "fuchsia".to_string(),
                    build_rule: Some("fuchsia_unittest_package".to_string()),
                    has_generated_manifest: Some(true),
                    package_label: Some("//examples/test/echo-integration-test".to_string()),
                    ..TestEntry::default()
                },
                FuchsiaTestTags { ..FuchsiaTestTags::default() },
                vec![
                    TestTag { key: "cpu".to_string(), value: "x64".to_string() },
                    TestTag { key: "hermetic".to_string(), value: "".to_string() },
                    TestTag { key: "legacy_test".to_string(), value: "".to_string() },
                    TestTag { key: "os".to_string(), value: "fuchsia".to_string() },
                    TestTag {
                        key: "package_label".to_string(),
                        value: "//examples/test/echo-integration-test".to_string(),
                    },
                    TestTag { key: "realm".to_string(), value: "".to_string() },
                    TestTag { key: "scope".to_string(), value: "unit".to_string() },
                ],
            ),
            (
                TestEntry {
                    cpu: "x64".to_string(),
                    os: "fuchsia".to_string(),
                    build_rule: Some("fuchsia_unittest_package".to_string()),
                    has_generated_manifest: Some(false),
                    ..TestEntry::default()
                },
                FuchsiaTestTags { ..FuchsiaTestTags::default() },
                vec![
                    TestTag { key: "cpu".to_string(), value: "x64".to_string() },
                    TestTag { key: "hermetic".to_string(), value: "".to_string() },
                    TestTag { key: "legacy_test".to_string(), value: "".to_string() },
                    TestTag { key: "os".to_string(), value: "fuchsia".to_string() },
                    TestTag { key: "package_label".to_string(), value: "".to_string() },
                    TestTag { key: "realm".to_string(), value: "".to_string() },
                    TestTag { key: "scope".to_string(), value: "integration".to_string() },
                ],
            ),
            (
                TestEntry {
                    cpu: "x64".to_string(),
                    os: "fuchsia".to_string(),
                    build_rule: Some("fuchsia_test_package".to_string()),
                    has_generated_manifest: Some(true),
                    ..TestEntry::default()
                },
                FuchsiaTestTags { ..FuchsiaTestTags::default() },
                vec![
                    TestTag { key: "cpu".to_string(), value: "x64".to_string() },
                    TestTag { key: "hermetic".to_string(), value: "".to_string() },
                    TestTag { key: "legacy_test".to_string(), value: "".to_string() },
                    TestTag { key: "os".to_string(), value: "fuchsia".to_string() },
                    TestTag { key: "package_label".to_string(), value: "".to_string() },
                    TestTag { key: "realm".to_string(), value: "".to_string() },
                    TestTag { key: "scope".to_string(), value: "unit".to_string() },
                ],
            ),
            (
                TestEntry {
                    cpu: "x64".to_string(),
                    os: "fuchsia".to_string(),
                    build_rule: Some("fuchsia_test_package".to_string()),
                    has_generated_manifest: Some(false),
                    ..TestEntry::default()
                },
                FuchsiaTestTags { ..FuchsiaTestTags::default() },
                vec![
                    TestTag { key: "cpu".to_string(), value: "x64".to_string() },
                    TestTag { key: "hermetic".to_string(), value: "".to_string() },
                    TestTag { key: "legacy_test".to_string(), value: "".to_string() },
                    TestTag { key: "os".to_string(), value: "fuchsia".to_string() },
                    TestTag { key: "package_label".to_string(), value: "".to_string() },
                    TestTag { key: "realm".to_string(), value: "".to_string() },
                    TestTag { key: "scope".to_string(), value: "integration".to_string() },
                ],
            ),
            (
                TestEntry {
                    cpu: "x64".to_string(),
                    os: "fuchsia".to_string(),
                    build_rule: Some("fuchsia_test_package".to_string()),
                    has_generated_manifest: Some(false),
                    ..TestEntry::default()
                },
                FuchsiaTestTags { realm: Some("vulkan".to_string()), ..FuchsiaTestTags::default() },
                vec![
                    TestTag { key: "cpu".to_string(), value: "x64".to_string() },
                    TestTag { key: "hermetic".to_string(), value: "".to_string() },
                    TestTag { key: "legacy_test".to_string(), value: "".to_string() },
                    TestTag { key: "os".to_string(), value: "fuchsia".to_string() },
                    TestTag { key: "package_label".to_string(), value: "".to_string() },
                    TestTag { key: "realm".to_string(), value: "vulkan".to_string() },
                    TestTag { key: "scope".to_string(), value: "system".to_string() },
                ],
            ),
            (
                TestEntry {
                    cpu: "x64".to_string(),
                    os: "fuchsia".to_string(),
                    build_rule: Some("fuchsia_test".to_string()),
                    has_generated_manifest: Some(true),
                    ..TestEntry::default()
                },
                FuchsiaTestTags {
                    realm: Some("hermetic".to_string()),
                    ..FuchsiaTestTags::default()
                },
                vec![
                    TestTag { key: "cpu".to_string(), value: "x64".to_string() },
                    TestTag { key: "hermetic".to_string(), value: "".to_string() },
                    TestTag { key: "legacy_test".to_string(), value: "".to_string() },
                    TestTag { key: "os".to_string(), value: "fuchsia".to_string() },
                    TestTag { key: "package_label".to_string(), value: "".to_string() },
                    TestTag { key: "realm".to_string(), value: "hermetic".to_string() },
                    TestTag { key: "scope".to_string(), value: "unit".to_string() },
                ],
            ),
            (
                TestEntry {
                    cpu: "x64".to_string(),
                    os: "fuchsia".to_string(),
                    build_rule: Some("fuchsia_fuzzer_package".to_string()),
                    has_generated_manifest: Some(false),
                    ..TestEntry::default()
                },
                FuchsiaTestTags {
                    realm: Some("hermetic".to_string()),
                    ..FuchsiaTestTags::default()
                },
                vec![
                    TestTag { key: "cpu".to_string(), value: "x64".to_string() },
                    TestTag { key: "hermetic".to_string(), value: "".to_string() },
                    TestTag { key: "legacy_test".to_string(), value: "".to_string() },
                    TestTag { key: "os".to_string(), value: "fuchsia".to_string() },
                    TestTag { key: "package_label".to_string(), value: "".to_string() },
                    TestTag { key: "realm".to_string(), value: "hermetic".to_string() },
                    TestTag { key: "scope".to_string(), value: "fuzzer".to_string() },
                ],
            ),
            (
                TestEntry {
                    cpu: "x64".to_string(),
                    os: "fuchsia".to_string(),
                    build_rule: Some("fuzzer_package".to_string()),
                    has_generated_manifest: Some(false),
                    ..TestEntry::default()
                },
                FuchsiaTestTags {
                    realm: Some("hermetic".to_string()),
                    ..FuchsiaTestTags::default()
                },
                vec![
                    TestTag { key: "cpu".to_string(), value: "x64".to_string() },
                    TestTag { key: "hermetic".to_string(), value: "".to_string() },
                    TestTag { key: "legacy_test".to_string(), value: "".to_string() },
                    TestTag { key: "os".to_string(), value: "fuchsia".to_string() },
                    TestTag { key: "package_label".to_string(), value: "".to_string() },
                    TestTag { key: "realm".to_string(), value: "hermetic".to_string() },
                    TestTag { key: "scope".to_string(), value: "fuzzer".to_string() },
                ],
            ),
            (
                TestEntry {
                    cpu: "x64".to_string(),
                    os: "fuchsia".to_string(),
                    build_rule: Some("prebuilt_test_package".to_string()),
                    has_generated_manifest: Some(false),
                    ..TestEntry::default()
                },
                FuchsiaTestTags {
                    realm: Some("hermetic".to_string()),
                    ..FuchsiaTestTags::default()
                },
                vec![
                    TestTag { key: "cpu".to_string(), value: "x64".to_string() },
                    TestTag { key: "hermetic".to_string(), value: "".to_string() },
                    TestTag { key: "legacy_test".to_string(), value: "".to_string() },
                    TestTag { key: "os".to_string(), value: "fuchsia".to_string() },
                    TestTag { key: "package_label".to_string(), value: "".to_string() },
                    TestTag { key: "realm".to_string(), value: "hermetic".to_string() },
                    TestTag { key: "scope".to_string(), value: "prebuilt".to_string() },
                ],
            ),
            (
                TestEntry {
                    cpu: "x64".to_string(),
                    os: "fuchsia".to_string(),
                    build_rule: Some("bootfs_test".to_string()),
                    has_generated_manifest: None,
                    ..TestEntry::default()
                },
                FuchsiaTestTags { ..FuchsiaTestTags::default() },
                vec![
                    TestTag { key: "cpu".to_string(), value: "x64".to_string() },
                    TestTag { key: "hermetic".to_string(), value: "".to_string() },
                    TestTag { key: "legacy_test".to_string(), value: "".to_string() },
                    TestTag { key: "os".to_string(), value: "fuchsia".to_string() },
                    TestTag { key: "package_label".to_string(), value: "".to_string() },
                    TestTag { key: "realm".to_string(), value: "".to_string() },
                    TestTag { key: "scope".to_string(), value: "bootfs".to_string() },
                ],
            ),
            (
                TestEntry {
                    cpu: "x64".to_string(),
                    os: "linux".to_string(),
                    name: "some_host_test.sh".to_string(),
                    ..TestEntry::default()
                },
                FuchsiaTestTags { ..FuchsiaTestTags::default() },
                vec![
                    TestTag { key: "cpu".to_string(), value: "x64".to_string() },
                    TestTag { key: "hermetic".to_string(), value: "".to_string() },
                    TestTag { key: "legacy_test".to_string(), value: "".to_string() },
                    TestTag { key: "os".to_string(), value: "linux".to_string() },
                    TestTag { key: "package_label".to_string(), value: "".to_string() },
                    TestTag { key: "realm".to_string(), value: "".to_string() },
                    TestTag { key: "scope".to_string(), value: "host".to_string() },
                ],
            ),
            (
                TestEntry {
                    cpu: "x64".to_string(),
                    os: "linux".to_string(),
                    name: "host_x64/test".to_string(),
                    ..TestEntry::default()
                },
                FuchsiaTestTags { ..FuchsiaTestTags::default() },
                vec![
                    TestTag { key: "cpu".to_string(), value: "x64".to_string() },
                    TestTag { key: "hermetic".to_string(), value: "".to_string() },
                    TestTag { key: "legacy_test".to_string(), value: "".to_string() },
                    TestTag { key: "os".to_string(), value: "linux".to_string() },
                    TestTag { key: "package_label".to_string(), value: "".to_string() },
                    TestTag { key: "realm".to_string(), value: "".to_string() },
                    TestTag { key: "scope".to_string(), value: "host".to_string() },
                ],
            ),
            (
                TestEntry {
                    cpu: "x64".to_string(),
                    os: "linux".to_string(),
                    name: "linux_x64/test".to_string(),
                    ..TestEntry::default()
                },
                FuchsiaTestTags { ..FuchsiaTestTags::default() },
                vec![
                    TestTag { key: "cpu".to_string(), value: "x64".to_string() },
                    TestTag { key: "hermetic".to_string(), value: "".to_string() },
                    TestTag { key: "legacy_test".to_string(), value: "".to_string() },
                    TestTag { key: "os".to_string(), value: "linux".to_string() },
                    TestTag { key: "package_label".to_string(), value: "".to_string() },
                    TestTag { key: "realm".to_string(), value: "".to_string() },
                    TestTag { key: "scope".to_string(), value: "host".to_string() },
                ],
            ),
        ];

        for (entry, mut tags, expected) in cases.into_iter() {
            update_tags_with_test_entry(&mut tags, &entry);
            assert_eq!(tags.into_vec(), expected, "entry was {:?}", entry);
        }
    }

    #[test]
    fn test_read_tests_json() {
        let data = r#"
            [
                {
                    "test": {
                        "cpu": "x64",
                        "label": "//build/components/tests:echo-integration-test_test_echo-client-test(//build/toolchain/fuchsia:x64)",
                        "log_settings": {
                            "max_severity": "WARN",
                            "min_severity": "DEBUG"
                        },
                        "name": "fuchsia-pkg://fuchsia.com/echo-integration-test#meta/echo-client-test.cm",
                        "os": "fuchsia",
                        "package_label": "//build/components/tests:echo-integration-test(//build/toolchain/fuchsia:x64)",
                        "package_manifests": [
                            "obj/build/components/tests/echo-integration-test/package_manifest.json"
                        ],
                        "package_url": "fuchsia-pkg://fuchsia.com/echo-integration-test#meta/echo-client-test.cm"
                    }
                }
            ]"#;
        let tmp = tempdir().expect("failed to get tempdir");
        let dir = Utf8Path::from_path(tmp.path()).unwrap();
        let tests_json_path = dir.join("tests.json");
        fs::write(&tests_json_path, data).expect("failed to write tests.json to tempfile");
        let tests_json = read_tests_json(&tests_json_path).expect("read_tests_json() failed");
        assert_eq!(
            tests_json,
            vec![
                TestsJsonEntry{
                    test: TestEntry{
                        name: "fuchsia-pkg://fuchsia.com/echo-integration-test#meta/echo-client-test.cm".to_string(),
                        label: "//build/components/tests:echo-integration-test_test_echo-client-test(//build/toolchain/fuchsia:x64)".to_string(),
                        cpu: "x64".to_string(),
                        os: "fuchsia".to_string(),
                        package_url: Some("fuchsia-pkg://fuchsia.com/echo-integration-test#meta/echo-client-test.cm".to_string()),
                        package_manifests: Some(vec![
                            "obj/build/components/tests/echo-integration-test/package_manifest.json".to_string(),
                        ]),
                        package_label: Some("//build/components/tests:echo-integration-test(//build/toolchain/fuchsia:x64)".into()),
                        log_settings: Some(LogSettings {
                            max_severity: Some(Severity::Warn),
                            min_severity: Some(Severity::Debug),
                        }),
                        ..TestEntry::default()
                    },
                }
            ],
        );
    }

    #[test]
    fn test_read_ctf_disable() {
        let data = r#"
                [
                {
                    "name": "fuchsia-pkg://fuchsia.com/fdio-spawn-tests-package#meta/test-root.cm",
                    "cases": [
                        {
                            "name": "main",
                            "disabled": true,
                            "bug": ""
                        }
                    ],
                    "compact_name": "fx/fdio-spawn-tests-package#meta/test-root.cm"
                },
                {
                    "name": "fuchsia-pkg://fuchsia.com/fdio-spawn-tests-package#meta/resolve-test-root.cm",
                    "cases": [
                        {
                            "name": "main2",
                            "disabled": false,
                            "bug": "b/12345"
                        }
                    ],
                    "compact_name": "fx/fdio-spawn-tests-package#meta/resolve-test-root.cm"
                }
            ]"#;
        let tmp = tempdir().expect("failed to get tempdir");
        let dir = Utf8Path::from_path(tmp.path()).unwrap();
        let disable_json_path = dir.join("tests.json");
        fs::write(&disable_json_path, data)
            .expect("failed to write disabled_tests.json to tempfile");
        let disable_json =
            read_ctf_disable(&Some(disable_json_path)).expect("read_ctf_disable() failed");
        assert_eq!(
            disable_json,
            Some(vec![
                CtfDisableSuite {
                    name: "fuchsia-pkg://fuchsia.com/fdio-spawn-tests-package#meta/test-root.cm".to_string(),
                    compact_name: Some("fx/fdio-spawn-tests-package#meta/test-root.cm".to_string()),
                    cases: vec![
                        CtfDisableCase {
                            name: "main".to_string(),
                            disabled: true,
                            bug: "".to_string(),
                        }
                    ]
                },
                CtfDisableSuite {
                    name: "fuchsia-pkg://fuchsia.com/fdio-spawn-tests-package#meta/resolve-test-root.cm".to_string(),
                    compact_name: Some("fx/fdio-spawn-tests-package#meta/resolve-test-root.cm".to_string()),
                    cases: vec![
                        CtfDisableCase {
                            name: "main2".to_string(),
                            disabled: false,
                            bug: "b/12345".to_string(),
                        }
                    ]
                },
            ]),
        );
    }

    #[test]
    fn test_read_test_components_json() {
        let data = r#"
            [
                {
                    "test_component": {
                        "label": "//build/components/tests:echo-integration-test(//build/toolchain/fuchsia:x64)"
                    }
                },
                {
                    "test_component": {
                        "label": "//build/components/tests:echo-system-integration-test(//build/toolchain/fuchsia:x64)",
                        "moniker": "/some/moniker"
                    }
                }
            ]"#;
        let tmp = tempdir().expect("failed to get tempdir");
        let dir = Utf8Path::from_path(tmp.path()).unwrap();
        let json_path = dir.join("test_components.json");
        fs::write(&json_path, data).expect("failed to write tests.json to tempfile");
        let tests_json =
            read_test_components_json(&json_path).expect("read_test_components_json() failed");
        assert_eq!(
            tests_json,
            vec![
                TestComponentsJsonEntry{
                    test_component: TestComponentEntry {
                        label: "//build/components/tests:echo-integration-test(//build/toolchain/fuchsia:x64)".into(),
                        moniker: None
                    }
                },
                TestComponentsJsonEntry{
                    test_component: TestComponentEntry {
                        label: "//build/components/tests:echo-system-integration-test(//build/toolchain/fuchsia:x64)".into(),
                        moniker: Some("/some/moniker".into())
                    }
                }
            ],
        );
    }

    #[test]
    fn convert_test_components_to_map() {
        let entry1 = TestComponentsJsonEntry {
            test_component: TestComponentEntry {
                label:
                    "//build/components/tests:echo-integration-test(//build/toolchain/fuchsia:x64)"
                        .into(),
                moniker: None,
            },
        };
        let entry2 = TestComponentsJsonEntry{
            test_component: TestComponentEntry {
                label: "//build/components/tests:echo-system-integration-test(//build/toolchain/fuchsia:x64)".into(),
                moniker: Some("/some/moniker".into())
            }
        };
        let entries = vec![entry1.clone(), entry2.clone()];
        let map = TestComponentsJsonEntry::convert_to_map(entries).unwrap();
        let mut expected_map = HashMap::new();
        expected_map.insert(entry1.test_component.label.clone(), entry1.clone());
        expected_map.insert(entry2.test_component.label.clone(), entry2.clone());

        assert_eq!(map, expected_map);

        // test non-conflicting entries

        // duplicate entries.
        let entries = vec![entry1.clone(), entry2.clone(), entry1.clone(), entry2.clone()];
        let map = TestComponentsJsonEntry::convert_to_map(entries).unwrap();
        let mut expected_map = HashMap::new();
        expected_map.insert(entry1.test_component.label.clone(), entry1.clone());
        expected_map.insert(entry2.test_component.label.clone(), entry2.clone());
        assert_eq!(map, expected_map);

        // test non-conflicting entries
        let mut entry3 = entry1.clone();
        entry3.test_component.moniker = Some("moniker".into());
        let entries = vec![entry1, entry2, entry3];
        let _ = TestComponentsJsonEntry::convert_to_map(entries)
            .expect_err("This should error out due to conflicting entries");
    }

    #[test]
    fn test_validate_test_decl_exposes_suite_protocol() {
        let mut component = Component::default();
        let mut expose_protocol = fdecl::ExposeProtocol::default();
        expose_protocol.target_name = Some(SUITE_PROTOCOL.to_string());
        let mut random_expose_protocol = fdecl::ExposeProtocol::default();
        random_expose_protocol.target_name = Some("random".to_string());
        component.exposes = Some(vec![
            fdecl::Expose::Protocol(expose_protocol),
            fdecl::Expose::Protocol(random_expose_protocol),
        ]);

        let result = validate_test_decl(&component);

        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_test_decl_does_not_expose_suite_protocol() {
        let mut component = Component::default();

        let err = validate_test_decl(&component).unwrap_err();

        assert!(
            err.to_string().contains(&format!("Does not expose {} protocol", SUITE_PROTOCOL)),
            "{:?}",
            err
        );

        let mut expose_protocol = fdecl::ExposeProtocol::default();
        expose_protocol.target_name = Some("random".to_string());
        component.exposes = Some(vec![fdecl::Expose::Protocol(expose_protocol)]);
        let err = validate_test_decl(&component).unwrap_err();

        assert!(
            err.to_string().contains(&format!("Does not expose {} protocol", SUITE_PROTOCOL)),
            "{:?}",
            err
        );
    }
}
