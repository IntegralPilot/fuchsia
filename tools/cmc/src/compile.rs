// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::error::Error;
use crate::features::FeatureSet;
use crate::util;
use fidl_fuchsia_component_decl as fdecl;
use std::io::Write;
use std::path::PathBuf;
use tempfile_ext::NamedTempFileExt as _;

use cml::load::{CmlLoader, OsResolver};
use cml::types::common::ContextSpanned;
use cml::types::program::ContextProgram;

/// Read in a CML file and produce the equivalent CM.
pub(crate) fn compile(
    file: &PathBuf,
    output: &PathBuf,
    depfile: Option<PathBuf>,
    includepath: &Vec<PathBuf>,
    includeroot: &PathBuf,
    config_package_path: Option<&str>,
    features: &FeatureSet,
    experimental_force_runner: &Option<String>,
    required_capabilities: cml::CapabilityRequirements<'_>,
) -> Result<(), Error> {
    check_extensions(file, output)?;
    util::ensure_directory_exists(&output)?;

    let output_parent = output.parent().ok_or_else(|| {
        Error::invalid_args(format!("Output file {:?} does not have a parent directory.", output))
    })?;

    let resolver = OsResolver::new(includepath.clone(), includeroot.clone());
    let mut loader = CmlLoader::new(resolver);

    let mut document = loader.load_and_merge_all(file)?;

    if let Some(force_runner) = experimental_force_runner {
        let program_context = document.program.get_or_insert_with(|| ContextSpanned {
            value: ContextProgram::default(),
            origin: file.clone().into(),
        });

        let runner_name = cm_types::Name::new(force_runner)?;

        program_context.value.runner =
            Some(ContextSpanned { value: runner_name, origin: program_context.origin.clone() });
    }

    document.canonicalize();

    let options = cml::CompileOptions::new()
        .file(&file)
        .features(features)
        .protocol_requirements(required_capabilities);
    let options =
        if let Some(s) = config_package_path { options.config_package_path(s) } else { options };

    let mut out_data = cml::compile(&document, options)?;

    let mut includes: Vec<PathBuf> =
        loader.visited_files().into_iter().filter(|p| p != file).collect();

    includes.sort();

    let mut manifest_sources = vec![util::strip_leading_dots(&file.to_string_lossy())];
    manifest_sources
        .extend(includes.iter().map(|p| util::strip_leading_dots(&p.to_string_lossy())));

    out_data.debug_info =
        Some(fdecl::DebugInfo { manifest_sources: Some(manifest_sources), ..Default::default() });

    let bytes = fidl::persist(&out_data)
        .map_err(|e| Error::parse(format!("FIDL encoding failed: {}", e), None, None))?;

    // Write to the output file, but only if the bytes have changed.
    let mut tmp = tempfile::NamedTempFile::new_in(output_parent)?;
    tmp.write_all(&bytes)?;
    tmp.persist_if_changed(&output).map_err(|e| e.error)?;

    // Write includes to depfile
    if let Some(depfile_path) = depfile {
        let mut all_deps = vec![file.clone()];
        all_deps.extend(includes);

        util::write_depfile(&depfile_path, Some(&output.to_path_buf()), &all_deps)?;
    }

    Ok(())
}

fn check_extensions(file: &PathBuf, output: &PathBuf) -> Result<(), Error> {
    if file.extension().map_or(true, |e| e != "cml") {
        return Err(Error::invalid_args("Input must be .cml"));
    }
    if output.extension().map_or(true, |e| e != "cm") {
        return Err(Error::invalid_args("Output must be .cm"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::Feature;
    use assert_matches::assert_matches;
    use cml::OfferToAllCapability;
    use difference::Changeset;
    use fidl::unpersist;
    use fidl_fuchsia_component_decl as fdecl;
    use fidl_fuchsia_data as fdata;
    use serde_json::json;
    use std::fs::File;
    use std::io::Read;
    use tempfile::TempDir;

    macro_rules! test_compile_with_features {
        (
            $features:expr,
            {
                $(
                    $(#[$m:meta])*
                    $test_name:ident => {
                        input = $input:expr,
                        output = $result:expr,
                    },
                )+
            }
        ) => {
            $(
                $(#[$m])*
                #[test]
                fn $test_name() {
                    let tmp_dir = TempDir::new().unwrap();
                    let tmp_in_path = tmp_dir.path().join("test.cml");
                    let tmp_out_path = tmp_dir.path().join("test.cm");
                    let features = $features;
                    compile_test(tmp_in_path, tmp_out_path, None, $input, $result, &features).expect("compilation failed");
                }
            )+
        }
    }

    #[track_caller]
    fn compile_test_all_options(
        in_path: PathBuf,
        out_path: PathBuf,
        includepath: Option<PathBuf>,
        input: serde_json::value::Value,
        expected_output: fdecl::Component,
        features: &FeatureSet,
        experimental_force_runner: &Option<String>,
        must_offer_protocol: &[String],
        must_use_protocol: &[String],
        must_offer_dictionary: &[String],
    ) -> Result<(), Error> {
        File::create(&in_path).unwrap().write_all(format!("{}", input).as_bytes()).unwrap();
        let includepath = includepath.unwrap_or(PathBuf::new());

        compile(
            &in_path.clone(),
            &out_path.clone(),
            None,
            &vec![includepath.clone()],
            &includepath.clone(),
            Some("test.cvf"),
            features,
            experimental_force_runner,
            cml::CapabilityRequirements {
                must_offer: &must_offer_protocol
                    .iter()
                    .map(|value| cml::OfferToAllCapability::Protocol(value))
                    .chain(
                        must_offer_dictionary
                            .iter()
                            .map(|value| OfferToAllCapability::Dictionary(value)),
                    )
                    .collect::<Vec<_>>(),
                must_use: &must_use_protocol
                    .iter()
                    .map(|value| cml::MustUseRequirement::Protocol(value))
                    .collect::<Vec<_>>(),
            },
        )?;
        let mut buffer = Vec::new();
        File::open(&out_path).unwrap().read_to_end(&mut buffer).unwrap();

        let mut output: fdecl::Component = unpersist(&buffer).unwrap();
        output.debug_info = None;
        if output != expected_output {
            let e = format!("{:#?}", expected_output);
            let a = format!("{:#?}", output);
            panic!("compiled output did not match expected\n{}", Changeset::new(&a, &e, "\n"));
        }

        Ok(())
    }

    #[track_caller]
    fn compile_test_with_forced_runner(
        in_path: PathBuf,
        out_path: PathBuf,
        includepath: Option<PathBuf>,
        input: serde_json::value::Value,
        expected_output: fdecl::Component,
        features: &FeatureSet,
        experimental_force_runner: &Option<String>,
    ) -> Result<(), Error> {
        compile_test_all_options(
            in_path,
            out_path,
            includepath,
            input,
            expected_output,
            features,
            experimental_force_runner,
            &[],
            &[],
            &[],
        )
    }

    #[track_caller]
    fn compile_test_with_required_protocols(
        in_path: PathBuf,
        out_path: PathBuf,
        includepath: Option<PathBuf>,
        input: serde_json::value::Value,
        expected_output: fdecl::Component,
        features: &FeatureSet,
        must_offer: &[String],
        must_use: &[String],
    ) -> Result<(), Error> {
        compile_test_all_options(
            in_path,
            out_path,
            includepath,
            input,
            expected_output,
            features,
            &None,
            must_offer,
            must_use,
            &[],
        )
    }

    #[track_caller]
    fn compile_test(
        in_path: PathBuf,
        out_path: PathBuf,
        includepath: Option<PathBuf>,
        input: serde_json::value::Value,
        expected_output: fdecl::Component,
        features: &FeatureSet,
    ) -> Result<(), Error> {
        compile_test_with_forced_runner(
            in_path,
            out_path,
            includepath,
            input,
            expected_output,
            features,
            &None,
        )
    }

    fn default_component_decl() -> fdecl::Component {
        fdecl::Component::default()
    }

    test_compile_with_features! { FeatureSet::from(vec![]), {
        test_compile_service_capabilities => {
            input = json!({
                "capabilities": [
                    {
                        "service": "myservice",
                        "path": "/service",
                    },
                    {
                        "service": [ "myservice2", "myservice3" ],
                    },
                ]
            }),
            output = fdecl::Component {
                capabilities: Some(vec![
                    fdecl::Capability::Service (
                        fdecl::Service {
                            name: Some("myservice".to_string()),
                            source_path: Some("/service".to_string()),
                            ..Default::default()
                        }
                    ),
                    fdecl::Capability::Service (
                        fdecl::Service {
                            name: Some("myservice2".to_string()),
                            source_path: Some("/svc/myservice2".to_string()),
                            ..Default::default()
                        }
                    ),
                    fdecl::Capability::Service (
                        fdecl::Service {
                            name: Some("myservice3".to_string()),
                            source_path: Some("/svc/myservice3".to_string()),
                            ..Default::default()
                        }
                    ),
                ]),
                ..Default::default()
            },
        },
        test_compile_use_service => {
            input = json!({
                "use": [
                    { "service": "CoolFonts", "path": "/svc/fuchsia.fonts.Provider" },
                    { "service": "fuchsia.component.Realm", "from": "framework" },
                    { "service": [ "myservice", "myservice2" ] },
                    { "service": "myservice3", "availability": "optional" },
                ]
            }),
            output = fdecl::Component {
                uses: Some(vec![
                    fdecl::Use::Service (
                        fdecl::UseService {
                            dependency_type: Some(fdecl::DependencyType::Strong),
                            source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            source_name: Some("CoolFonts".to_string()),
                            target_path: Some("/svc/fuchsia.fonts.Provider".to_string()),
                            availability: Some(fdecl::Availability::Required),
                            ..Default::default()
                        }
                    ),
                    fdecl::Use::Service (
                        fdecl::UseService {
                            dependency_type: Some(fdecl::DependencyType::Strong),
                            source: Some(fdecl::Ref::Framework(fdecl::FrameworkRef {})),
                            source_name: Some("fuchsia.component.Realm".to_string()),
                            target_path: Some("/svc/fuchsia.component.Realm".to_string()),
                            availability: Some(fdecl::Availability::Required),
                            ..Default::default()
                        }
                    ),
                    fdecl::Use::Service (
                        fdecl::UseService {
                            dependency_type: Some(fdecl::DependencyType::Strong),
                            source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            source_name: Some("myservice".to_string()),
                            target_path: Some("/svc/myservice".to_string()),
                            availability: Some(fdecl::Availability::Required),
                            ..Default::default()
                        }
                    ),
                    fdecl::Use::Service (
                        fdecl::UseService {
                            dependency_type: Some(fdecl::DependencyType::Strong),
                            source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            source_name: Some("myservice2".to_string()),
                            target_path: Some("/svc/myservice2".to_string()),
                            availability: Some(fdecl::Availability::Required),
                            ..Default::default()
                        }
                    ),
                    fdecl::Use::Service (
                        fdecl::UseService {
                            dependency_type: Some(fdecl::DependencyType::Strong),
                            source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            source_name: Some("myservice3".to_string()),
                            target_path: Some("/svc/myservice3".to_string()),
                            availability: Some(fdecl::Availability::Optional),
                            ..Default::default()
                        }
                    ),
                ]),
                ..Default::default()
            },
        },
        test_compile_offer_service => {
            input = json!({
                "offer": [
                    {
                        "service": "fuchsia.logger.Log",
                        "from": "#logger",
                        "to": [ "#netstack" ]
                    },
                    {
                        "service": "fuchsia.logger.Log",
                        "from": "#logger",
                        "to": [ "#coll" ],
                        "as": "fuchsia.logger.Log2",
                    },
                    {
                        "service": [
                            "my.service.Service",
                            "my.service.Service2",
                        ],
                        "from": "self",
                        "to": [ "#netstack" ]
                    },
                    {
                        "service": "my.service.CollectionService",
                        "from": "#coll",
                        "to": "#netstack",
                    },
                    {
                        "service": "agg.service.CollectionService",
                        "from": [ "#coll", "#other_coll" ],
                        "to": [ "#netstack" ],
                        "availability": "same_as_target",
                    },
                ],
                "capabilities": [
                    {
                        "service": [
                            "my.service.Service",
                            "my.service.Service2",
                        ],
                    },
                ],
                "children": [
                    {
                        "name": "logger",
                        "url": "fuchsia-pkg://logger.cm"
                    },
                    {
                        "name": "netstack",
                        "url": "fuchsia-pkg://netstack.cm"
                    },
                ],
                "collections": [
                    {
                        "name": "coll",
                        "durability": "transient",
                    },
                    {
                        "name": "other_coll",
                        "durability": "transient",
                    },
                ],
            }),
            output = fdecl::Component {
                offers: Some(vec![
                    fdecl::Offer::Service (
                        fdecl::OfferService {
                            source: Some(fdecl::Ref::Collection(fdecl::CollectionRef { name: "coll".to_string() })),
                            source_name: Some("agg.service.CollectionService".to_string()),
                            target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                                name: "netstack".to_string(),
                                collection: None,
                            })),
                            target_name: Some("agg.service.CollectionService".to_string()),
                            availability: Some(fdecl::Availability::SameAsTarget),
                            dependency_type: Some(fdecl::DependencyType::Strong),
                            ..Default::default()
                        }
                    ),
                    fdecl::Offer::Service (
                        fdecl::OfferService {
                            source: Some(fdecl::Ref::Collection(fdecl::CollectionRef { name: "other_coll".to_string() })),
                            source_name: Some("agg.service.CollectionService".to_string()),
                            target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                                name: "netstack".to_string(),
                                collection: None,
                            })),
                            target_name: Some("agg.service.CollectionService".to_string()),
                            availability: Some(fdecl::Availability::SameAsTarget),
                            dependency_type: Some(fdecl::DependencyType::Strong),
                            ..Default::default()
                        }
                    ),
                    fdecl::Offer::Service (
                        fdecl::OfferService {
                            source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                                name: "logger".to_string(),
                                collection: None,
                            })),
                            source_name: Some("fuchsia.logger.Log".to_string()),
                            target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                                name: "netstack".to_string(),
                                collection: None,
                            })),
                            target_name: Some("fuchsia.logger.Log".to_string()),
                            dependency_type: Some(fdecl::DependencyType::Strong),
                            availability: Some(fdecl::Availability::Required),
                            ..Default::default()
                        }
                    ),
                    fdecl::Offer::Service (
                        fdecl::OfferService {
                            source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                                name: "logger".to_string(),
                                collection: None,
                            })),
                            source_name: Some("fuchsia.logger.Log".to_string()),
                            target: Some(fdecl::Ref::Collection(fdecl::CollectionRef {
                                name: "coll".to_string(),
                            })),
                            target_name: Some("fuchsia.logger.Log2".to_string()),
                            availability: Some(fdecl::Availability::Required),
                            dependency_type: Some(fdecl::DependencyType::Strong),
                            ..Default::default()
                        }
                    ),
                    fdecl::Offer::Service (
                        fdecl::OfferService {
                            source: Some(fdecl::Ref::Collection(fdecl::CollectionRef { name: "coll".to_string() })),
                            source_name: Some("my.service.CollectionService".to_string()),
                            target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                                name: "netstack".to_string(),
                                collection: None,
                            })),
                            target_name: Some("my.service.CollectionService".to_string()),
                            availability: Some(fdecl::Availability::Required),
                            dependency_type: Some(fdecl::DependencyType::Strong),
                            ..Default::default()
                        }
                    ),
                    fdecl::Offer::Service (
                        fdecl::OfferService {
                            source: Some(fdecl::Ref::Self_(fdecl::SelfRef {})),
                            source_name: Some("my.service.Service".to_string()),
                            target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                                name: "netstack".to_string(),
                                collection: None,
                            })),
                            target_name: Some("my.service.Service".to_string()),
                            availability: Some(fdecl::Availability::Required),
                            dependency_type: Some(fdecl::DependencyType::Strong),
                            ..Default::default()
                        }
                    ),
                    fdecl::Offer::Service (
                        fdecl::OfferService {
                            source: Some(fdecl::Ref::Self_(fdecl::SelfRef {})),
                            source_name: Some("my.service.Service2".to_string()),
                            target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                                name: "netstack".to_string(),
                                collection: None,
                            })),
                            target_name: Some("my.service.Service2".to_string()),
                            availability: Some(fdecl::Availability::Required),
                            dependency_type: Some(fdecl::DependencyType::Strong),
                            ..Default::default()
                        }
                    ),
                ]),
                capabilities: Some(vec![
                    fdecl::Capability::Service (
                        fdecl::Service {
                            name: Some("my.service.Service".to_string()),
                            source_path: Some("/svc/my.service.Service".to_string()),
                            ..Default::default()
                        }
                    ),
                    fdecl::Capability::Service (
                        fdecl::Service {
                            name: Some("my.service.Service2".to_string()),
                            source_path: Some("/svc/my.service.Service2".to_string()),
                            ..Default::default()
                        }
                    ),
                ]),
                children: Some(vec![
                    fdecl::Child {
                        name: Some("logger".to_string()),
                        url: Some("fuchsia-pkg://logger.cm".to_string()),
                        startup: Some(fdecl::StartupMode::Lazy),
                        environment: None,
                        on_terminate: None,
                        ..Default::default()
                    },
                    fdecl::Child {
                        name: Some("netstack".to_string()),
                        url: Some("fuchsia-pkg://netstack.cm".to_string()),
                        startup: Some(fdecl::StartupMode::Lazy),
                        environment: None,
                        on_terminate: None,
                        ..Default::default()
                    }
                ]),
                collections: Some(vec![
                    fdecl::Collection {
                        name: Some("coll".to_string()),
                        durability: Some(fdecl::Durability::Transient),
                        environment: None,
                        allowed_offers: None,
                        allow_long_names: None,
                        ..Default::default()
                    },
                    fdecl::Collection {
                        name: Some("other_coll".to_string()),
                        durability: Some(fdecl::Durability::Transient),
                        environment: None,
                        allowed_offers: None,
                        allow_long_names: None,
                        ..Default::default()
                    },
                ]),
                ..Default::default()
            },
        },
        test_compile_expose_service => {
            input = json!({
                "expose": [
                    {
                        "service": "fuchsia.logger.Log",
                        "from": "#logger",
                        "as": "fuchsia.logger.Log2",
                    },
                    {
                        "service": [
                            "my.service.Service",
                            "my.service.Service2",
                        ],
                        "from": "self",
                    },
                    {
                        "service": "my.service.CollectionService",
                        "from": "#coll",
                    },
                    {
                        "service": "agg.service.CollectionService",
                        "from": [ "#coll", "#other_coll" ],
                    },
                ],
                "capabilities": [
                    {
                        "service": [
                            "my.service.Service",
                            "my.service.Service2",
                        ],
                    },
                ],
                "children": [
                    {
                        "name": "logger",
                        "url": "fuchsia-pkg://logger.cm"
                    },
                ],
                "collections": [
                    {
                        "name": "coll",
                        "durability": "transient",
                    },
                    {
                        "name": "other_coll",
                        "durability": "transient",
                    },
                ],
            }),
            output = fdecl::Component {
                exposes: Some(vec![
                    fdecl::Expose::Service (
                        fdecl::ExposeService {
                            source: Some(fdecl::Ref::Collection(fdecl::CollectionRef { name: "coll".to_string() })),
                            source_name: Some("agg.service.CollectionService".to_string()),
                            target_name: Some("agg.service.CollectionService".to_string()),
                            target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            availability: Some(fdecl::Availability::Required),
                            ..Default::default()
                        }
                    ),
                    fdecl::Expose::Service (
                        fdecl::ExposeService {
                            source: Some(fdecl::Ref::Collection(fdecl::CollectionRef { name: "other_coll".to_string() })),
                            source_name: Some("agg.service.CollectionService".to_string()),
                            target_name: Some("agg.service.CollectionService".to_string()),
                            target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            availability: Some(fdecl::Availability::Required),
                            ..Default::default()
                        }
                    ),
                    fdecl::Expose::Service (
                        fdecl::ExposeService {
                            source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                                name: "logger".to_string(),
                                collection: None,
                            })),
                            source_name: Some("fuchsia.logger.Log".to_string()),
                            target_name: Some("fuchsia.logger.Log2".to_string()),
                            target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            availability: Some(fdecl::Availability::Required),
                            ..Default::default()
                        }
                    ),
                    fdecl::Expose::Service (
                        fdecl::ExposeService {
                            source: Some(fdecl::Ref::Collection(fdecl::CollectionRef { name: "coll".to_string() })),
                            source_name: Some("my.service.CollectionService".to_string()),
                            target_name: Some("my.service.CollectionService".to_string()),
                            target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            availability: Some(fdecl::Availability::Required),
                            ..Default::default()
                        }
                    ),
                    fdecl::Expose::Service (
                        fdecl::ExposeService {
                            source: Some(fdecl::Ref::Self_(fdecl::SelfRef {})),
                            source_name: Some("my.service.Service".to_string()),
                            target_name: Some("my.service.Service".to_string()),
                            target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            availability: Some(fdecl::Availability::Required),
                            ..Default::default()
                        }
                    ),
                    fdecl::Expose::Service (
                        fdecl::ExposeService {
                            source: Some(fdecl::Ref::Self_(fdecl::SelfRef {})),
                            source_name: Some("my.service.Service2".to_string()),
                            target_name: Some("my.service.Service2".to_string()),
                            target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            availability: Some(fdecl::Availability::Required),
                            ..Default::default()
                        }
                    ),
                ]),
                capabilities: Some(vec![
                    fdecl::Capability::Service (
                        fdecl::Service {
                            name: Some("my.service.Service".to_string()),
                            source_path: Some("/svc/my.service.Service".to_string()),
                            ..Default::default()
                        }
                    ),
                    fdecl::Capability::Service (
                        fdecl::Service {
                            name: Some("my.service.Service2".to_string()),
                            source_path: Some("/svc/my.service.Service2".to_string()),
                            ..Default::default()
                        }
                    ),
                ]),
                children: Some(vec![
                    fdecl::Child {
                        name: Some("logger".to_string()),
                        url: Some("fuchsia-pkg://logger.cm".to_string()),
                        startup: Some(fdecl::StartupMode::Lazy),
                        environment: None,
                        on_terminate: None,
                        ..Default::default()
                    }
                ]),
                collections: Some(vec![
                    fdecl::Collection {
                        name: Some("coll".to_string()),
                        durability: Some(fdecl::Durability::Transient),
                        environment: None,
                        allowed_offers: None,
                        allow_long_names: None,
                        ..Default::default()
                    },
                    fdecl::Collection {
                        name: Some("other_coll".to_string()),
                        durability: Some(fdecl::Durability::Transient),
                        environment: None,
                        allowed_offers: None,
                        allow_long_names: None,
                        ..Default::default()
                    }
                ]),
                ..Default::default()
            },
        },

        test_compile_expose_service_other_availability => {
            input = json!({
                "expose": [
                    {
                        "service": "fuchsia.logger.Log",
                        "from": "#logger",
                        "as": "fuchsia.logger.Log_default",
                    },
                    {
                        "service": "fuchsia.logger.Log",
                        "from": "#logger",
                        "as": "fuchsia.logger.Log_required",
                        "availability": "required",
                    },
                    {
                        "service": [
                            "my.service.Service",
                            "my.service.Service2",
                        ],
                        "from": "self",
                        "availability": "optional",
                    },
                    {
                        "service": "my.service.CollectionService",
                        "from": ["#coll"],
                        "availability": "same_as_target",
                    },
                    {
                        "service": "fuchsia.logger.Log",
                        "from": "#logger",
                        "as": "fuchsia.logger.Log_transitional",
                        "availability": "transitional",
                    },
                ],
                "capabilities": [
                    {
                        "service": [
                            "my.service.Service",
                            "my.service.Service2",
                        ],
                    },
                ],
                "children": [
                    {
                        "name": "logger",
                        "url": "fuchsia-pkg://logger.cm"
                    },
                ],
                "collections": [
                    {
                        "name": "coll",
                        "durability": "transient",
                    },
                ],
            }),
            output = fdecl::Component {
                exposes: Some(vec![
                    fdecl::Expose::Service (
                        fdecl::ExposeService {
                            source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                                name: "logger".to_string(),
                                collection: None,
                            })),
                            source_name: Some("fuchsia.logger.Log".to_string()),
                            target_name: Some("fuchsia.logger.Log_default".to_string()),
                            target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            availability: Some(fdecl::Availability::Required),
                            ..Default::default()
                        }
                    ),
                    fdecl::Expose::Service (
                        fdecl::ExposeService {
                            source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                                name: "logger".to_string(),
                                collection: None,
                            })),
                            source_name: Some("fuchsia.logger.Log".to_string()),
                            target_name: Some("fuchsia.logger.Log_required".to_string()),
                            target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            availability: Some(fdecl::Availability::Required),
                            ..Default::default()
                        }
                    ),
                    fdecl::Expose::Service (
                        fdecl::ExposeService {
                            source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                                name: "logger".to_string(),
                                collection: None,
                            })),
                            source_name: Some("fuchsia.logger.Log".to_string()),
                            target_name: Some("fuchsia.logger.Log_transitional".to_string()),
                            target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            availability: Some(fdecl::Availability::Transitional),
                            ..Default::default()
                        }
                    ),
                    fdecl::Expose::Service (
                        fdecl::ExposeService {
                            source: Some(fdecl::Ref::Collection(fdecl::CollectionRef { name: "coll".to_string() })),
                            source_name: Some("my.service.CollectionService".to_string()),
                            target_name: Some("my.service.CollectionService".to_string()),
                            target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            availability: Some(fdecl::Availability::SameAsTarget),
                            ..Default::default()
                        }
                    ),
                    fdecl::Expose::Service (
                        fdecl::ExposeService {
                            source: Some(fdecl::Ref::Self_(fdecl::SelfRef {})),
                            source_name: Some("my.service.Service".to_string()),
                            target_name: Some("my.service.Service".to_string()),
                            target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            availability: Some(fdecl::Availability::Optional),
                            ..Default::default()
                        }
                    ),
                    fdecl::Expose::Service (
                        fdecl::ExposeService {
                            source: Some(fdecl::Ref::Self_(fdecl::SelfRef {})),
                            source_name: Some("my.service.Service2".to_string()),
                            target_name: Some("my.service.Service2".to_string()),
                            target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            availability: Some(fdecl::Availability::Optional),
                            ..Default::default()
                        }
                    ),
                ]),
                capabilities: Some(vec![
                    fdecl::Capability::Service (
                        fdecl::Service {
                            name: Some("my.service.Service".to_string()),
                            source_path: Some("/svc/my.service.Service".to_string()),
                            ..Default::default()
                        }
                    ),
                    fdecl::Capability::Service (
                        fdecl::Service {
                            name: Some("my.service.Service2".to_string()),
                            source_path: Some("/svc/my.service.Service2".to_string()),
                            ..Default::default()
                        }
                    ),
                ]),
                children: Some(vec![
                    fdecl::Child {
                        name: Some("logger".to_string()),
                        url: Some("fuchsia-pkg://logger.cm".to_string()),
                        startup: Some(fdecl::StartupMode::Lazy),
                        environment: None,
                        on_terminate: None,
                        ..Default::default()
                    }
                ]),
                collections: Some(vec![
                    fdecl::Collection {
                        name: Some("coll".to_string()),
                        durability: Some(fdecl::Durability::Transient),
                        environment: None,
                        allowed_offers: None,
                        allow_long_names: None,
                        ..Default::default()
                    }
                ]),
                ..Default::default()
            },
        },

        // TODO(https://fxbug.dev/42077867): Add a test where different availabilities aggregated by
        // several service expose declarations is an error.
    }}

    test_compile_with_features! { FeatureSet::from(vec![Feature::AllowLongNames]), {
        test_compile_allow_long_names => {
            input = json!({
                "collections": [
                    {
                        "name": "long_child_names",
                        "durability": "transient",
                        "allow_long_names": true,
                    },
                ],
            }),
            output = fdecl::Component {
                collections: Some(vec![
                   fdecl::Collection {
                        name: Some("long_child_names".to_string()),
                        durability: Some(fdecl::Durability::Transient),
                        allowed_offers: None,
                        allow_long_names: Some(true),
                        ..Default::default()
                    }
                ]),
                ..Default::default()
            },
        },
    }}

    test_compile_with_features! { FeatureSet::from(vec![]), {
        test_compile_config => {
            input = json!({
                "config": {
                    "test8": {
                        "type": "vector",
                        "max_count": 100,
                        "element": {
                            "type": "uint16"
                        }
                    },
                    "test7": { "type": "int64" },
                    "test6": { "type": "uint64" },
                    "test5": { "type": "int8" },
                    "test4": { "type": "uint8" },
                    "test3": { "type": "bool" },
                    "test2": {
                        "type": "vector",
                        "max_count": 100,
                        "element": {
                            "type": "string",
                            "max_size": 50
                        }
                    },
                    "test1": {
                        "type": "string",
                        "max_size": 50
                    },
                    "test_mutable_by_parent": {
                        "type": "bool",
                        "mutability": ["parent"]
                    },
                }
            }),
            output = fdecl::Component {
                config: Some(fdecl::ConfigSchema{
                    fields: Some(vec![
                        fdecl::ConfigField {
                            key: Some("test1".to_string()),
                            type_: Some(fdecl::ConfigType {
                                layout: fdecl::ConfigTypeLayout::String,
                                parameters: Some(vec![]),
                                constraints: vec![fdecl::LayoutConstraint::MaxSize(50)]
                            }),
                            mutability: Some(Default::default()),
                            ..Default::default()
                        },
                        fdecl::ConfigField {
                            key: Some("test2".to_string()),
                            type_: Some(fdecl::ConfigType {
                                layout: fdecl::ConfigTypeLayout::Vector,
                                parameters: Some(vec![fdecl::LayoutParameter::NestedType(
                                    fdecl::ConfigType {
                                        layout: fdecl::ConfigTypeLayout::String,
                                        parameters: Some(vec![]),
                                        constraints: vec![fdecl::LayoutConstraint::MaxSize(50)]
                                    }
                                )]),
                                constraints: vec![fdecl::LayoutConstraint::MaxSize(100)]
                            }),
                            mutability: Some(Default::default()),
                            ..Default::default()
                        },
                        fdecl::ConfigField {
                            key: Some("test3".to_string()),
                            type_: Some(fdecl::ConfigType {
                                layout: fdecl::ConfigTypeLayout::Bool,
                                parameters: Some(vec![]),
                                constraints: vec![]
                            }),
                            mutability: Some(Default::default()),
                            ..Default::default()
                        },
                        fdecl::ConfigField {
                            key: Some("test4".to_string()),
                            type_: Some(fdecl::ConfigType {
                                layout: fdecl::ConfigTypeLayout::Uint8,
                                parameters: Some(vec![]),
                                constraints: vec![]
                            }),
                            mutability: Some(Default::default()),
                            ..Default::default()
                        },
                        fdecl::ConfigField {
                            key: Some("test5".to_string()),
                            type_: Some(fdecl::ConfigType {
                                layout: fdecl::ConfigTypeLayout::Int8,
                                parameters: Some(vec![]),
                                constraints: vec![]
                            }),
                            mutability: Some(Default::default()),
                            ..Default::default()
                        },
                        fdecl::ConfigField {
                            key: Some("test6".to_string()),
                            type_: Some(fdecl::ConfigType {
                                layout: fdecl::ConfigTypeLayout::Uint64,
                                parameters: Some(vec![]),
                                constraints: vec![]
                            }),
                            mutability: Some(Default::default()),
                            ..Default::default()
                        },
                        fdecl::ConfigField {
                            key: Some("test7".to_string()),
                            type_: Some(fdecl::ConfigType {
                                layout: fdecl::ConfigTypeLayout::Int64,
                                parameters: Some(vec![]),
                                constraints: vec![]
                            }),
                            mutability: Some(Default::default()),
                            ..Default::default()
                        },

                        fdecl::ConfigField {
                            key: Some("test8".to_string()),
                            type_: Some(fdecl::ConfigType {
                                layout: fdecl::ConfigTypeLayout::Vector,
                                parameters: Some(vec![fdecl::LayoutParameter::NestedType(
                                    fdecl::ConfigType {
                                        layout: fdecl::ConfigTypeLayout::Uint16,
                                        parameters: Some(vec![]),
                                        constraints: vec![]
                                    }
                                )]),
                                constraints: vec![fdecl::LayoutConstraint::MaxSize(100)]
                            }),
                            mutability: Some(Default::default()),
                            ..Default::default()
                        },
                        fdecl::ConfigField {
                            key: Some("test_mutable_by_parent".to_string()),
                            type_: Some(fdecl::ConfigType {
                                layout: fdecl::ConfigTypeLayout::Bool,
                                parameters: Some(vec![]),
                                constraints: vec![]
                            }),
                            mutability: Some(fdecl::ConfigMutability::PARENT),
                            ..Default::default()
                        },
                    ]),
                    checksum: Some(fdecl::ConfigChecksum::Sha256([
                        252, 234, 115, 44, 165, 222, 210, 109, 67, 231, 20, 151, 150, 45, 191, 36,
                        245, 50, 72, 140, 14, 188, 24, 192, 80, 161, 38, 18, 85, 151, 99, 115,
                    ])),
                    value_source: Some(fdecl::ConfigValueSource::PackagePath("test.cvf".to_string())),
                    ..Default::default()
                }),
                ..Default::default()
            },
        },
    }}

    #[test]
    fn test_missing_include() {
        let tmp_dir = TempDir::new().unwrap();
        let in_path = tmp_dir.path().join("test.cml");
        let out_path = tmp_dir.path().join("test.cm");
        let result = compile_test(
            in_path,
            out_path,
            Some(tmp_dir.keep()),
            json!({ "include": [ "doesnt_exist.cml" ] }),
            default_component_decl(),
            &FeatureSet::empty(),
        );
        assert_matches!(
            result,
            Err(Error::ValidateContext { err, .. }) if err.starts_with("Internal error: File not found") && err.contains("doesnt_exist.cml")
        );
    }

    #[test]
    fn test_good_include() {
        let tmp_dir = TempDir::new().unwrap();
        let foo_path = tmp_dir.path().join("foo.cml");
        File::create(&foo_path)
            .unwrap()
            .write_all(format!("{}", json!({ "program": { "runner": "elf" } })).as_bytes())
            .unwrap();

        let in_path = tmp_dir.path().join("test.cml");
        let out_path = tmp_dir.path().join("test.cm");
        compile_test(
            in_path,
            out_path,
            Some(tmp_dir.keep()),
            json!({
                "include": [ "foo.cml" ],
                "program": { "binary": "bin/test" },
            }),
            fdecl::Component {
                program: Some(fdecl::Program {
                    runner: Some("elf".to_string()),
                    info: Some(fdata::Dictionary {
                        entries: Some(vec![fdata::DictionaryEntry {
                            key: "binary".to_string(),
                            value: Some(Box::new(fdata::DictionaryValue::Str(
                                "bin/test".to_string(),
                            ))),
                        }]),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                ..default_component_decl()
            },
            &FeatureSet::empty(),
        )
        .unwrap();
    }

    #[test]
    fn test_offer_to_all_with_shards() {
        let tmp_dir = TempDir::new().unwrap();
        let offer_path = tmp_dir.path().join("offer.shard.cml");
        let shard_input = json!({
            "offer": [
                {
                    "protocol": "fuchsia.logger.LogSink",
                    "from": "parent",
                    "to": "all",
                },
                {
                    "protocol": "fuchsia.inspect.InspectSink",
                    "from": "parent",
                    "to": "all",
                },
            ],
        });
        File::create(&offer_path)
            .unwrap()
            .write_all(format!("{}", shard_input).as_bytes())
            .unwrap();

        let foo_path = tmp_dir.path().join("foo.shard.cml");
        let foo_input = json!({
            "include": ["offer.shard.cml"],
            "children": [
                {
                    "name": "foo",
                    "url": "fuchsia-pkg://fuchsia.com/foo/stable#meta/foo.cm",
                },
            ],
        });
        File::create(&foo_path).unwrap().write_all(format!("{}", foo_input).as_bytes()).unwrap();

        let main_input = json!({
            "include": [
                "offer.shard.cml",
                "foo.shard.cml",
            ],
        });

        let expected_output = fdecl::Component {
            offers: Some(vec![
                fdecl::Offer::Protocol(fdecl::OfferProtocol {
                    source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                    source_name: Some("fuchsia.inspect.InspectSink".into()),
                    target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                        name: "foo".into(),
                        collection: None,
                    })),
                    target_name: Some("fuchsia.inspect.InspectSink".into()),
                    dependency_type: Some(fdecl::DependencyType::Strong),
                    availability: Some(fdecl::Availability::Required),
                    ..Default::default()
                }),
                fdecl::Offer::Protocol(fdecl::OfferProtocol {
                    source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                    source_name: Some("fuchsia.logger.LogSink".into()),
                    target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                        name: "foo".into(),
                        collection: None,
                    })),
                    target_name: Some("fuchsia.logger.LogSink".into()),
                    dependency_type: Some(fdecl::DependencyType::Strong),
                    availability: Some(fdecl::Availability::Required),
                    ..Default::default()
                }),
            ]),
            children: Some(vec![fdecl::Child {
                name: Some("foo".into()),
                url: Some("fuchsia-pkg://fuchsia.com/foo/stable#meta/foo.cm".into()),
                startup: Some(fdecl::StartupMode::Lazy),
                ..Default::default()
            }]),
            ..default_component_decl()
        };

        let in_path = tmp_dir.path().join("test.cml");
        let out_path = tmp_dir.path().join("test.cm");
        compile_test_with_required_protocols(
            in_path,
            out_path,
            Some(tmp_dir.keep()),
            main_input,
            expected_output,
            &FeatureSet::empty(),
            &["fuchsia.logger.LogSink".into()],
            &[],
        )
        .unwrap();
    }

    #[test]
    fn test_good_include_with_force_runner() {
        let tmp_dir = TempDir::new().unwrap();
        let foo_path = tmp_dir.path().join("foo.cml");
        File::create(&foo_path)
            .unwrap()
            .write_all(format!("{}", json!({ "program": { "runner": "elf" } })).as_bytes())
            .unwrap();

        let in_path = tmp_dir.path().join("test.cml");
        let out_path = tmp_dir.path().join("test.cm");
        compile_test_with_forced_runner(
            in_path,
            out_path,
            Some(tmp_dir.keep()),
            json!({
                "include": [ "foo.cml" ],
                "program": { "binary": "bin/test" },
            }),
            fdecl::Component {
                program: Some(fdecl::Program {
                    runner: Some("elf_test_runner".to_string()),
                    info: Some(fdata::Dictionary {
                        entries: Some(vec![fdata::DictionaryEntry {
                            key: "binary".to_string(),
                            value: Some(Box::new(fdata::DictionaryValue::Str(
                                "bin/test".to_string(),
                            ))),
                        }]),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                ..default_component_decl()
            },
            &FeatureSet::empty(),
            &Some("elf_test_runner".to_string()),
        )
        .unwrap();
    }

    #[test]
    fn test_recursive_include() {
        let tmp_dir = TempDir::new().unwrap();
        let foo_path = tmp_dir.path().join("foo.cml");
        File::create(&foo_path)
            .unwrap()
            .write_all(format!("{}", json!({ "include": [ "bar.cml" ] })).as_bytes())
            .unwrap();

        let bar_path = tmp_dir.path().join("bar.cml");
        File::create(&bar_path)
            .unwrap()
            .write_all(format!("{}", json!({ "program": { "runner": "elf" } })).as_bytes())
            .unwrap();

        let in_path = tmp_dir.path().join("test.cml");
        let out_path = tmp_dir.path().join("test.cm");
        compile_test(
            in_path,
            out_path,
            Some(tmp_dir.keep()),
            json!({
                "include": [ "foo.cml" ],
                "program": { "binary": "bin/test" },
            }),
            fdecl::Component {
                program: Some(fdecl::Program {
                    runner: Some("elf".to_string()),
                    info: Some(fdata::Dictionary {
                        entries: Some(vec![fdata::DictionaryEntry {
                            key: "binary".to_string(),
                            value: Some(Box::new(fdata::DictionaryValue::Str(
                                "bin/test".to_string(),
                            ))),
                        }]),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                ..default_component_decl()
            },
            &FeatureSet::empty(),
        )
        .unwrap();
    }

    #[test]
    fn test_cyclic_include() {
        let tmp_dir = TempDir::new().unwrap();
        let foo_path = tmp_dir.path().join("foo.cml");
        File::create(&foo_path)
            .unwrap()
            .write_all(format!("{}", json!({ "include": [ "bar.cml" ] })).as_bytes())
            .unwrap();

        let bar_path = tmp_dir.path().join("bar.cml");
        File::create(&bar_path)
            .unwrap()
            .write_all(format!("{}", json!({ "include": [ "foo.cml" ] })).as_bytes())
            .unwrap();

        let in_path = tmp_dir.path().join("test.cml");
        let out_path = tmp_dir.path().join("test.cm");
        let result = compile_test(
            in_path,
            out_path,
            Some(tmp_dir.keep()),
            json!({
                "include": [ "foo.cml" ],
                "program": {
                    "runner": "elf",
                    "binary": "bin/test",
                },
            }),
            default_component_decl(),
            &FeatureSet::empty(),
        );
        assert_matches!(result, Err(Error::ValidateContext { err, .. }) if err.contains("Circular include"));
    }

    #[test]
    fn test_conflicting_includes() {
        let tmp_dir = TempDir::new().unwrap();
        let foo_path = tmp_dir.path().join("foo.cml");
        File::create(&foo_path)
            .unwrap()
            .write_all(
                format!("{}", json!({ "use": [ { "protocol": "foo", "path": "/svc/foo" } ] }))
                    .as_bytes(),
            )
            .unwrap();
        let bar_path = tmp_dir.path().join("bar.cml");

        // Try to mount protocol "bar" under the same path "/svc/foo".
        File::create(&bar_path)
            .unwrap()
            .write_all(
                format!("{}", json!({ "use": [ { "protocol": "bar", "path": "/svc/foo" } ] }))
                    .as_bytes(),
            )
            .unwrap();

        let in_path = tmp_dir.path().join("test.cml");
        let out_path = tmp_dir.path().join("test.cm");
        let result = compile_test(
            in_path,
            out_path,
            Some(tmp_dir.keep()),
            json!({
                "include": [ "foo.cml", "bar.cml" ],
                "program": {
                    "runner": "elf",
                    "binary": "bin/test",
                },
            }),
            default_component_decl(),
            &FeatureSet::empty(),
        );
        // Including both foo.cml and bar.cml should fail to validate because of an incoming
        // namespace collision.
        assert_matches!(result, Err(Error::ValidateContexts { err, .. }) if err.contains("is a duplicate \"use\""));
    }

    #[test]
    fn test_overlapping_includes() {
        let tmp_dir = TempDir::new().unwrap();
        let foo1_path = tmp_dir.path().join("foo1.cml");
        File::create(&foo1_path)
            .unwrap()
            .write_all(format!("{}", json!({ "use": [ { "protocol": "foo" } ] })).as_bytes())
            .unwrap();

        let foo2_path = tmp_dir.path().join("foo2.cml");
        // Include protocol "foo" again
        File::create(&foo2_path)
            .unwrap()
            // Use different but equivalent syntax to further stress any overlap affordances
            .write_all(format!("{}", json!({ "use": [ { "protocol": [ "foo" ] } ] })).as_bytes())
            .unwrap();

        let in_path = tmp_dir.path().join("test.cml");
        let out_path = tmp_dir.path().join("test.cm");
        let result = compile_test(
            in_path,
            out_path,
            Some(tmp_dir.keep()),
            json!({
                "include": [ "foo1.cml", "foo2.cml" ],
                "program": {
                    "runner": "elf",
                    "binary": "bin/test",
                },
            }),
            fdecl::Component {
                program: Some(fdecl::Program {
                    runner: Some("elf".to_string()),
                    info: Some(fdata::Dictionary {
                        entries: Some(vec![fdata::DictionaryEntry {
                            key: "binary".to_string(),
                            value: Some(Box::new(fdata::DictionaryValue::Str(
                                "bin/test".to_string(),
                            ))),
                        }]),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                uses: Some(vec![fdecl::Use::Protocol(fdecl::UseProtocol {
                    dependency_type: Some(fdecl::DependencyType::Strong),
                    source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                    source_name: Some("foo".to_string()),
                    target_path: Some("/svc/foo".to_string()),
                    availability: Some(fdecl::Availability::Required),
                    ..Default::default()
                })]),
                ..default_component_decl()
            },
            &FeatureSet::empty(),
        );
        // Including both foo1.cml and foo2.cml is fine because they overlap,
        // so merging foo2.cml after having merged foo1.cml is a no-op.
        assert_matches!(result, Ok(()));
    }
}
