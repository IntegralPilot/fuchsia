// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::model::component::WeakComponentInstance;
use crate::sandbox_util::take_handle_as_stream;
use ::routing::bedrock::sandbox_construction::ComponentSandbox;
use ::routing::component_instance::ComponentInstanceInterface;
use capability_source::CapabilitySource;
use cm_types::RelativePath;
use fidl_fuchsia_component as fcomponent;
use fidl_fuchsia_component_runtime::RouteRequest;
use fidl_fuchsia_sys2 as fsys;
use futures::future::BoxFuture;
use futures::{FutureExt, TryStreamExt};
use log::{error, warn};
use moniker::{ExtendedMoniker, Moniker};
use router_error::RouterError;
use runtime_capabilities::{Capability, Dictionary, WeakInstanceToken};
use std::sync::Arc;

pub fn serve(
    server_end: zx::Channel,
    _target: WeakComponentInstance,
    source: WeakComponentInstance,
) -> BoxFuture<'static, Result<(), anyhow::Error>> {
    async move {
        let stream = take_handle_as_stream::<fsys::RouteValidatorMarker>(server_end);
        serve_inner(stream, source).await;
        Ok(())
    }
    .boxed()
}

/// Serve the fuchsia.sys2.RouteValidator protocol for a given scope on a given stream
async fn serve_inner(mut stream: fsys::RouteValidatorRequestStream, source: WeakComponentInstance) {
    let res: Result<(), fidl::Error> = async move {
        while let Some(request) = stream.try_next().await? {
            match request {
                fsys::RouteValidatorRequest::Validate { moniker, responder } => {
                    let result = validate(&source, &moniker).await;
                    if let Err(e) = responder.send(result.as_deref().map_err(|e| *e)) {
                        warn!(error:% = e; "RouteValidator failed to send Validate response");
                    }
                }
                fsys::RouteValidatorRequest::Route { moniker, targets, responder } => {
                    let result = route(&source, &moniker, targets).await;
                    if let Err(e) = responder.send(result.as_deref().map_err(|e| *e)) {
                        warn!(error:% = e; "RouteValidator failed to send Route response");
                    }
                }
            }
        }
        Ok(())
    }
    .await;
    if let Err(e) = &res {
        warn!(error:% = e; "RouteValidator server failed");
    }
}

async fn validate(
    scope: &WeakComponentInstance,
    moniker_str: &str,
) -> Result<Vec<fsys::RouteReport>, fcomponent::Error> {
    // Construct the complete moniker using the scope moniker and the moniker string.
    let moniker =
        Moniker::try_from(moniker_str).map_err(|_| fcomponent::Error::InvalidArguments)?;
    let moniker = scope.moniker.concat(&moniker);

    let component_instance_token = scope.clone().into();
    let scope = scope.upgrade().map_err(|_| fcomponent::Error::InstanceNotFound)?;
    let instance =
        scope.find_absolute(&moniker).await.map_err(|_| fcomponent::Error::InstanceNotFound)?;

    let sandbox = instance
        .lock_resolved_state()
        .await
        .map_err(|_| fcomponent::Error::InstanceCannotResolve)?
        .sandbox
        .clone();
    let reports = validate_sandbox(&sandbox, component_instance_token, &scope.moniker).await;
    Ok(reports)
}

async fn route(
    scope: &WeakComponentInstance,
    moniker_str: &str,
    targets: Vec<fsys::RouteTarget>,
) -> Result<Vec<fsys::RouteReport>, fsys::RouteValidatorError> {
    // Construct the complete moniker using the scope moniker and the moniker string.

    let moniker =
        Moniker::try_from(moniker_str).map_err(|_| fsys::RouteValidatorError::InvalidArguments)?;
    let moniker = scope.moniker.concat(&moniker);

    let component_instance_token = scope.clone().into();
    let scope = scope.upgrade().map_err(|_| fsys::RouteValidatorError::InstanceNotFound)?;
    let instance = scope
        .find_absolute(&moniker)
        .await
        .map_err(|_| fsys::RouteValidatorError::InstanceNotFound)?;

    let sandbox = instance
        .lock_resolved_state()
        .await
        .map_err(|_| fsys::RouteValidatorError::InstanceNotResolved)?
        .sandbox
        .clone();
    let mut reports = validate_sandbox(&sandbox, component_instance_token, &scope.moniker).await;

    if targets.is_empty() {
        return Ok(reports);
    }

    reports.retain(|report| {
        targets.iter().any(|target| {
            let type_match = target.decl_type == fsys::DeclType::Any
                || report.decl_type == Some(target.decl_type);
            let name_match =
                report.capability.as_ref().map_or(false, |cap| cap.contains(&target.name));
            type_match && name_match
        })
    });
    Ok(reports)
}

async fn validate_sandbox(
    sandbox: &ComponentSandbox,
    component_instance_token: Arc<WeakInstanceToken>,
    scope: &Moniker,
) -> Vec<fsys::RouteReport> {
    let mut reports = Vec::new();

    reports = validate_dictionary(
        RelativePath::dot(),
        sandbox.program_input.namespace(),
        component_instance_token.clone(),
        fsys::DeclType::Use,
        reports,
    )
    .await;

    reports = validate_dictionary(
        RelativePath::dot(),
        sandbox.program_input.numbered_handles(),
        component_instance_token.clone(),
        fsys::DeclType::Use,
        reports,
    )
    .await;

    if let Some(runner_router) = sandbox.program_input.runner() {
        let result = runner_router
            .route_debug(RouteRequest::default(), component_instance_token.clone())
            .await;
        let mut report = fsys::RouteReport {
            capability: Some("<runner>".to_string()),
            decl_type: Some(fsys::DeclType::Use),
            ..Default::default()
        };
        fill_in_report_with_route_result(&mut report, result);
        reports.push(report);
    }

    reports = validate_dictionary(
        RelativePath::dot(),
        sandbox.component_output.capabilities(),
        component_instance_token,
        fsys::DeclType::Expose,
        reports,
    )
    .await;

    for report in reports.iter_mut() {
        if let Some(report_moniker) = report.source_moniker.as_mut() {
            // The monikers listed in the reports should be restricted to the scope of the
            // validator.
            match ExtendedMoniker::parse_str(report_moniker.as_str()) {
                Ok(ExtendedMoniker::ComponentInstance(moniker)) => {
                    *report_moniker = moniker
                        .strip_prefix(&scope)
                        .map(|m| m.to_string())
                        .unwrap_or_else(|_| "<above scope>".to_string());
                }
                Ok(ExtendedMoniker::ComponentManager) => {
                    if !scope.is_root() {
                        *report_moniker = "<above scope>".to_string();
                    }
                }
                Err(e) => {
                    error!(
                        "we generated a report with an invalid moniker: {report_moniker:?} {e:?}"
                    );
                    *report_moniker = "<invalid moniker>".to_string();
                }
            }
        }
    }
    reports
}

fn fill_in_report_with_route_result(
    report: &mut fsys::RouteReport,
    result: Result<CapabilitySource, RouterError>,
) {
    match result {
        Ok(source) => {
            let outcome = match &source {
                CapabilitySource::Void(_) => fsys::RouteOutcome::Void,
                _ => fsys::RouteOutcome::Success,
            };
            report.outcome = Some(outcome);
            let service_instances = match &source {
                CapabilitySource::AnonymizedAggregate(anonymized_aggregate_source) => Some(
                    anonymized_aggregate_source
                        .instances
                        .clone()
                        .into_iter()
                        .map(fsys::ServiceInstance::from)
                        .collect(),
                ),
                _ => None,
            };
            report.service_instances = service_instances;

            report.source_moniker = Some(source.source_moniker().to_string());

            #[cfg(fuchsia_api_level_at_least = "HEAD")]
            {
                report.build_time_capability_type = source.type_name().map(|t| t.to_string());
            }
        }
        Err(routing_error) => {
            report.outcome = Some(fsys::RouteOutcome::Failed);
            report.error = Some(fsys::RouteError {
                summary: Some(format!("{:?}", routing_error)),
                ..fsys::RouteError::default()
            });
        }
    }
}

fn validate_dictionary(
    path: RelativePath,
    dictionary: Arc<Dictionary>,
    component_instance_token: Arc<WeakInstanceToken>,
    decl_type: fsys::DeclType,
    mut reports: Vec<fsys::RouteReport>,
) -> BoxFuture<'static, Vec<fsys::RouteReport>> {
    async move {
        for (name, capability) in dictionary.enumerate() {
            let mut capability_path = path.clone();
            assert!(capability_path.push(name));

            let mut report = fsys::RouteReport {
                capability: Some(capability_path.to_string()),
                decl_type: Some(decl_type.clone()),
                ..Default::default()
            };
            match capability {
                Capability::Connector(_) | Capability::DirConnector(_) | Capability::Data(_) => {
                    report.outcome = Some(fsys::RouteOutcome::Success);
                    reports.push(report);
                }
                Capability::Dictionary(child_dictionary) => {
                    reports = validate_dictionary(
                        capability_path,
                        child_dictionary,
                        component_instance_token.clone(),
                        decl_type.clone(),
                        reports,
                    )
                    .await;
                }
                Capability::ConnectorRouter(router) => {
                    let result = router
                        .route_debug(RouteRequest::default(), component_instance_token.clone())
                        .await;
                    fill_in_report_with_route_result(&mut report, result);
                    reports.push(report);
                }
                Capability::DirConnectorRouter(router) => {
                    let result = router
                        .route_debug(RouteRequest::default(), component_instance_token.clone())
                        .await;
                    fill_in_report_with_route_result(&mut report, result);
                    reports.push(report);
                }
                Capability::DataRouter(router) => {
                    let result = router
                        .route_debug(RouteRequest::default(), component_instance_token.clone())
                        .await;
                    fill_in_report_with_route_result(&mut report, result);
                    reports.push(report);
                }
                Capability::DictionaryRouter(router) => {
                    let result = router
                        .route_debug(RouteRequest::default(), component_instance_token.clone())
                        .await;
                    fill_in_report_with_route_result(&mut report, result);

                    if let Ok(Some(routed_dictionary)) = router
                        .route(RouteRequest::default(), component_instance_token.clone())
                        .await
                    {
                        let keys = routed_dictionary.snapshot_keys_as_strings();
                        let mut entries = Vec::with_capacity(keys.len());
                        for k in keys {
                            entries.push(fsys::DictionaryEntry {
                                name: Some(k),
                                ..Default::default()
                            });
                        }

                        report.dictionary_entries = Some(entries);

                        reports = validate_dictionary(
                            capability_path,
                            routed_dictionary,
                            component_instance_token.clone(),
                            decl_type.clone(),
                            reports,
                        )
                        .await;
                    }
                    reports.push(report);
                }
                other_value => warn!("unexpected capability type: {other_value:?}"),
            }
        }
        reports
    }
    .boxed()
}

#[cfg(all(test, not(feature = "src_model_tests")))]
mod tests {
    use super::*;
    use crate::model::component::StartReason;
    use crate::model::start::Start;
    use crate::model::testing::out_dir::OutDir;
    use crate::model::testing::test_helpers::{TestEnvironmentBuilder, TestModelResult};
    use ::routing::component_instance::ComponentInstanceInterface;
    use assert_matches::assert_matches;
    use cm_rust::offer::*;
    use cm_rust::*;
    use cm_rust_testing::*;
    use fidl::endpoints;
    use fidl_fuchsia_component_decl as fdecl;
    use fidl_fuchsia_io as fio;

    async fn route_validator(test: &TestModelResult) -> fsys::RouteValidatorProxy {
        let (proxy, server) = endpoints::create_proxy::<fsys::RouteValidatorMarker>();
        let weak_root = test.model.root().as_weak();
        test.model.root().execution_scope.spawn(async move {
            serve(server.into_channel(), weak_root.clone(), weak_root).await.unwrap();
        });
        proxy
    }

    #[derive(Ord, PartialOrd, Eq, PartialEq)]
    struct Key {
        capability: String,
        decl_type: fsys::DeclType,
    }

    /// Validate API reports that routing succeeded normally.
    #[fuchsia::test]
    async fn validate() {
        // Test several capability types.
        let components = vec![
            (
                "root",
                ComponentDeclBuilder::new_empty_component()
                    .use_(UseBuilder::runner().name("elf").source_static_child("my_child"))
                    .use_(
                        UseBuilder::protocol()
                            .source(UseSource::Framework)
                            .name("fuchsia.component.Realm"),
                    )
                    .use_(UseBuilder::protocol().name("foo.bar").source_static_child("my_child"))
                    .expose(
                        ExposeBuilder::protocol().name("foo.bar").source_static_child("my_child"),
                    )
                    .child_default("my_child")
                    .build(),
            ),
            (
                "my_child",
                ComponentDeclBuilder::new()
                    .dictionary_default("dict")
                    .protocol_default("foo.bar")
                    .runner_default("elf")
                    .expose(ExposeBuilder::runner().name("elf").source(ExposeSource::Self_))
                    .expose(ExposeBuilder::dictionary().name("dict").source(ExposeSource::Self_))
                    .expose(ExposeBuilder::protocol().name("foo.bar").source(ExposeSource::Self_))
                    .offer(
                        OfferBuilder::protocol()
                            .name("foo.bar")
                            .source(OfferSource::Self_)
                            .target(OfferTarget::Capability("dict".parse().unwrap())),
                    )
                    .build(),
            ),
        ];

        let test = TestEnvironmentBuilder::new().set_components(components).build().await;
        let validator = route_validator(&test).await;

        test.model.start().await;

        // Validate the root
        let mut results = validator.validate(".").await.unwrap().unwrap();

        results.sort_by_key(|r| Key {
            capability: r.capability.clone().unwrap(),
            decl_type: r.decl_type.clone().unwrap(),
        });

        let report = results.remove(0);
        assert_matches!(
            report,
            fsys::RouteReport {
                outcome: Some(fsys::RouteOutcome::Success),
                capability: Some(s),
                decl_type: Some(fsys::DeclType::Use),
                source_moniker: Some(m),
                error: None,
                ..
            } if s == "<runner>" && m == "my_child"
        );

        let report = results.remove(0);
        assert_matches!(
            report,
            fsys::RouteReport {
                outcome: Some(fsys::RouteOutcome::Success),
                capability: Some(s),
                decl_type: Some(fsys::DeclType::Expose),
                source_moniker: Some(m),
                error: None,
                ..
            } if s == "foo.bar" && m == "my_child"
        );

        let report = results.remove(0);
        assert_matches!(
            report,
            fsys::RouteReport {
                outcome: Some(fsys::RouteOutcome::Success),
                capability: Some(s),
                decl_type: Some(fsys::DeclType::Use),
                source_moniker: Some(m),
                error: None,
                ..
            } if s == "svc/foo.bar" && m == "my_child"
        );

        let report = results.remove(0);
        assert_matches!(
            report,
            fsys::RouteReport {
                outcome: Some(fsys::RouteOutcome::Success),
                capability: Some(s),
                decl_type: Some(fsys::DeclType::Use),
                source_moniker: Some(m),
                error: None,
                ..
            } if s == "svc/fuchsia.component.Realm" && m == "."
        );

        assert!(results.is_empty());

        // Validate `my_child`
        let mut results = validator.validate("my_child").await.unwrap().unwrap();

        results.sort_by_key(|r| Key {
            capability: r.capability.clone().unwrap(),
            decl_type: r.decl_type.clone().unwrap(),
        });

        let report = results.remove(0);
        assert_matches!(
            report,
            fsys::RouteReport {
                outcome: Some(fsys::RouteOutcome::Success),
                capability: Some(s),
                decl_type: Some(fsys::DeclType::Use),
                source_moniker: Some(m),
                error: None,
                ..
            } if s == "<runner>" && m == "<component_manager>"
        );

        let report = results.remove(0);
        assert_matches!(
            report,
            fsys::RouteReport {
                outcome: Some(fsys::RouteOutcome::Success),
                capability: Some(s),
                decl_type: Some(fsys::DeclType::Expose),
                source_moniker: Some(m),
                dictionary_entries: Some(d),
                error: None,
                ..
            } if s == "dict" && m == "my_child" &&
                d == [fsys::DictionaryEntry {
                    name: Some("foo.bar".into()),
                    ..Default::default()
                }]
        );

        let report = results.remove(0);
        assert_matches!(
            report,
            fsys::RouteReport {
                outcome: Some(fsys::RouteOutcome::Success),
                capability: Some(s),
                decl_type: Some(fsys::DeclType::Expose),
                source_moniker: Some(m),
                error: None,
                ..
            } if s == "dict/foo.bar" && m == "my_child"
        );

        let report = results.remove(0);
        assert_matches!(
            report,
            fsys::RouteReport {
                outcome: Some(fsys::RouteOutcome::Success),
                capability: Some(s),
                decl_type: Some(fsys::DeclType::Expose),
                source_moniker: Some(m),
                error: None,
                ..
            } if s == "elf" && m == "my_child"
        );

        let report = results.remove(0);
        assert_matches!(
            report,
            fsys::RouteReport {
                outcome: Some(fsys::RouteOutcome::Success),
                capability: Some(s),
                decl_type: Some(fsys::DeclType::Expose),
                source_moniker: Some(m),
                error: None,
                ..
            } if s == "foo.bar" && m == "my_child"
        );

        assert!(results.is_empty());
    }

    /// Validate API reports that routing succeeded with a `void` source.
    #[fuchsia::test]
    async fn validate_from_void() {
        let use_from_child_decl = UseBuilder::protocol()
            .source_static_child("my_child")
            .name("foo.bar")
            .availability(cm_rust::Availability::Optional)
            .build();
        let expose_from_child_decl = ExposeBuilder::protocol()
            .name("foo.bar")
            .source_static_child("my_child")
            .availability(cm_rust::Availability::Optional)
            .build();
        let expose_from_void_decl = ExposeBuilder::protocol()
            .name("foo.bar")
            .source(ExposeSource::Void)
            .availability(cm_rust::Availability::Optional)
            .build();

        let components = vec![
            (
                "root",
                ComponentDeclBuilder::new_empty_component()
                    .use_(use_from_child_decl)
                    .expose(expose_from_child_decl)
                    .child_default("my_child")
                    .build(),
            ),
            (
                "my_child",
                ComponentDeclBuilder::new_empty_component()
                    .protocol_default("foo.bar")
                    .expose(expose_from_void_decl)
                    .build(),
            ),
        ];

        let test = TestEnvironmentBuilder::new().set_components(components).build().await;
        let validator = route_validator(&test).await;

        test.model.start().await;

        // `my_child` should not be resolved right now
        let instance = test.model.root().find_resolved(&["my_child"].try_into().unwrap()).await;
        assert!(instance.is_none());

        // Validate the root
        let mut results = validator.validate(".").await.unwrap().unwrap();

        results.sort_by_key(|r| Key {
            capability: r.capability.clone().unwrap(),
            decl_type: r.decl_type.clone().unwrap(),
        });

        assert_eq!(results.len(), 2);

        let report = results.remove(0);
        assert_matches!(
            report,
            fsys::RouteReport {
                outcome: Some(fsys::RouteOutcome::Void),
                capability: Some(s),
                decl_type: Some(fsys::DeclType::Expose),
                source_moniker: Some(m),
                availability: None,
                error: None,
                ..
            } if s == "foo.bar" && m == "my_child"
        );

        let report = results.remove(0);
        assert_matches!(
            report,
            fsys::RouteReport {
                outcome: Some(fsys::RouteOutcome::Void),
                capability: Some(s),
                decl_type: Some(fsys::DeclType::Use),
                source_moniker: Some(m),
                availability: None,
                error: None,
                ..
            } if s == "svc/foo.bar" && m == "my_child"
        );

        // This validation should have caused `my_child` to be resolved
        let instance = test.model.root().find_resolved(&["my_child"].try_into().unwrap()).await;
        assert!(instance.is_some());

        // Validate `my_child`
        let mut results = validator.validate("my_child").await.unwrap().unwrap();
        assert_eq!(results.len(), 1);

        let report = results.remove(0);
        assert_matches!(
            report,
            fsys::RouteReport {
                outcome: Some(fsys::RouteOutcome::Void),
                capability: Some(s),
                decl_type: Some(fsys::DeclType::Expose),
                source_moniker: Some(m),
                availability: None,
                error: None,
                ..
            } if s == "foo.bar" && m == "my_child"
        );
    }

    /// Validate API reports that routing failed and returns the error.
    #[fuchsia::test]
    async fn validate_error() {
        let invalid_source_name_use_from_child_decl =
            UseBuilder::protocol().source_static_child("my_child").name("a").build();
        let invalid_source_name_expose_from_child_decl =
            ExposeBuilder::protocol().name("c").source_static_child("my_child").build();

        let components = vec![
            (
                "root",
                ComponentDeclBuilder::new_empty_component()
                    .use_(invalid_source_name_use_from_child_decl)
                    .expose(invalid_source_name_expose_from_child_decl)
                    .child_default("my_child")
                    .build(),
            ),
            ("my_child", ComponentDeclBuilder::new_empty_component().build()),
        ];

        let test = TestEnvironmentBuilder::new().set_components(components).build().await;
        let validator = route_validator(&test).await;

        test.model.start().await;

        // `my_child` should not be resolved right now
        let instance = test.model.root().find_resolved(&["my_child"].try_into().unwrap()).await;
        assert!(instance.is_none());

        // Validate the root
        let mut results = validator.validate(".").await.unwrap().unwrap();
        assert_eq!(results.len(), 2);

        results.sort_by_key(|r| Key {
            capability: r.capability.clone().unwrap(),
            decl_type: r.decl_type.clone().unwrap(),
        });

        let report = results.remove(0);
        assert_matches!(
            report,
            fsys::RouteReport {
                capability: Some(s),
                outcome: Some(fsys::RouteOutcome::Failed),
                decl_type: Some(fsys::DeclType::Expose),
                source_moniker: None,
                error: Some(_),
                ..
            } if s == "c"
        );

        let report = results.remove(0);
        assert_matches!(
            report,
            fsys::RouteReport {
                capability: Some(s),
                outcome: Some(fsys::RouteOutcome::Failed),
                decl_type: Some(fsys::DeclType::Use),
                source_moniker: None,
                error: Some(_),
                ..
            } if s == "svc/a"
        );

        // This validation should have caused `my_child` to be resolved
        let instance = test.model.root().find_resolved(&["my_child"].try_into().unwrap()).await;
        assert!(instance.is_some());
    }

    /// Route API reports that routing succeeded normally, with exact capability names as inputs.
    #[fuchsia::test]
    async fn route() {
        let use_from_framework_decl = UseBuilder::protocol()
            .source(UseSource::Framework)
            .name("fuchsia.component.Realm")
            .build();
        let use_from_child_decl = UseBuilder::protocol()
            .source_static_child("my_child")
            .name("biz.buz")
            .path("/svc/foo.bar")
            .build();
        let expose_from_child_decl = ExposeBuilder::protocol()
            .name("biz.buz")
            .target_name("foo.bar")
            .source_static_child("my_child")
            .build();
        let expose_from_self_decl =
            ExposeBuilder::protocol().name("biz.buz").source(ExposeSource::Self_).build();

        let components = vec![
            (
                "root",
                ComponentDeclBuilder::new()
                    .use_(use_from_framework_decl)
                    .use_(use_from_child_decl)
                    .expose(expose_from_child_decl)
                    .child_default("my_child")
                    .build(),
            ),
            (
                "my_child",
                ComponentDeclBuilder::new()
                    .capability(
                        CapabilityBuilder::protocol().name("biz.buz").path("/svc/foo.bar").build(),
                    )
                    .expose(expose_from_self_decl)
                    .build(),
            ),
        ];

        let test = TestEnvironmentBuilder::new().set_components(components).build().await;
        let validator = route_validator(&test).await;

        test.model.start().await;

        // Validate the root
        let targets = &[
            fsys::RouteTarget { name: "foo.bar".parse().unwrap(), decl_type: fsys::DeclType::Use },
            fsys::RouteTarget {
                name: "foo.bar".parse().unwrap(),
                decl_type: fsys::DeclType::Expose,
            },
            fsys::RouteTarget {
                name: "fuchsia.component.Realm".parse().unwrap(),
                decl_type: fsys::DeclType::Use,
            },
        ];
        let mut results = validator.route(".", targets).await.unwrap().unwrap();
        results.sort_unstable_by_key(|result| result.capability.clone());

        assert_eq!(results.len(), 3);

        let report = results.remove(0);
        assert_matches!(
            report,
            fsys::RouteReport {
                outcome: Some(fsys::RouteOutcome::Success),
                capability: Some(s),
                decl_type: Some(fsys::DeclType::Expose),
                source_moniker: Some(m),
                error: None,
                ..
            } if s == "foo.bar" && m == "my_child"
        );

        let report = results.remove(0);
        assert_matches!(
            report,
            fsys::RouteReport {
                outcome: Some(fsys::RouteOutcome::Success),
                capability: Some(s),
                decl_type: Some(fsys::DeclType::Use),
                source_moniker: Some(m),
                error: None,
                ..
            } if s == "svc/foo.bar" && m == "my_child"
        );

        let report = results.remove(0);
        assert_matches!(
            report,
            fsys::RouteReport {
                outcome: Some(fsys::RouteOutcome::Success),
                capability: Some(s),
                decl_type: Some(fsys::DeclType::Use),
                source_moniker: Some(m),
                error: None,
                ..
            } if s == "svc/fuchsia.component.Realm" && m == "."
        );

        // Validate `my_child`
        let targets = &[fsys::RouteTarget {
            name: "biz.buz".parse().unwrap(),
            decl_type: fsys::DeclType::Expose,
        }];
        let mut results = validator.route("my_child", targets).await.unwrap().unwrap();

        assert_eq!(results.len(), 1);

        let report = results.remove(0);
        assert_matches!(
            report,
            fsys::RouteReport {
                outcome: Some(fsys::RouteOutcome::Success),
                capability: Some(s),
                decl_type: Some(fsys::DeclType::Expose),
                source_moniker: Some(m),
                error: None,
                ..
            } if s == "biz.buz" && m == "my_child"
        );
    }

    /// Route API reports that routing succeeded with a `void` source.
    #[fuchsia::test]
    async fn route_from_void() {
        let use_from_child_decl = UseBuilder::protocol()
            .source_static_child("my_child")
            .name("biz.buz")
            .path("/svc/foo.bar")
            .availability(cm_rust::Availability::Optional)
            .build();
        let expose_from_child_decl = ExposeBuilder::protocol()
            .name("biz.buz")
            .target_name("foo.bar")
            .source_static_child("my_child")
            .availability(cm_rust::Availability::Optional)
            .build();
        let expose_from_void_decl = ExposeBuilder::protocol()
            .name("biz.buz")
            .source(ExposeSource::Void)
            .availability(cm_rust::Availability::Optional)
            .build();

        let components = vec![
            (
                "root",
                ComponentDeclBuilder::new()
                    .use_(use_from_child_decl)
                    .expose(expose_from_child_decl)
                    .child_default("my_child")
                    .build(),
            ),
            (
                "my_child",
                ComponentDeclBuilder::new()
                    .capability(
                        CapabilityBuilder::protocol().name("biz.buz").path("/svc/foo.bar").build(),
                    )
                    .expose(expose_from_void_decl)
                    .build(),
            ),
        ];

        let test = TestEnvironmentBuilder::new().set_components(components).build().await;
        let validator = route_validator(&test).await;

        test.model.start().await;

        // Validate the root
        let targets = &[
            fsys::RouteTarget { name: "foo.bar".parse().unwrap(), decl_type: fsys::DeclType::Use },
            fsys::RouteTarget {
                name: "foo.bar".parse().unwrap(),
                decl_type: fsys::DeclType::Expose,
            },
        ];
        let mut results = validator.route(".", targets).await.unwrap().unwrap();
        results.sort_unstable_by_key(|result| result.capability.clone());

        assert_eq!(results.len(), 2);

        let report = results.remove(0);
        assert_matches!(
            report,
            fsys::RouteReport {
                outcome: Some(fsys::RouteOutcome::Void),
                capability: Some(s),
                decl_type: Some(fsys::DeclType::Expose),
                source_moniker: Some(m),
                availability: None,
                error: None,
                ..
            } if s == "foo.bar" && m == "my_child"
        );

        let report = results.remove(0);
        assert_matches!(
            report,
            fsys::RouteReport {
                outcome: Some(fsys::RouteOutcome::Void),
                capability: Some(s),
                decl_type: Some(fsys::DeclType::Use),
                source_moniker: Some(m),
                availability: None,
                error: None,
                ..
            } if s == "svc/foo.bar" && m == "my_child"
        );

        // Validate `my_child`
        let targets = &[fsys::RouteTarget {
            name: "biz.buz".parse().unwrap(),
            decl_type: fsys::DeclType::Expose,
        }];
        let mut results = validator.route("my_child", targets).await.unwrap().unwrap();

        assert_eq!(results.len(), 1);

        let report = results.remove(0);
        assert_matches!(
            report,
            fsys::RouteReport {
                outcome: Some(fsys::RouteOutcome::Void),
                capability: Some(s),
                decl_type: Some(fsys::DeclType::Expose),
                source_moniker: Some(m),
                availability: None,
                error: None,
                ..
            } if s == "biz.buz" && m == "my_child"
        );
    }

    /// Route API reports that routing succeeded normally, with no capability names (that is, route
    /// all capabilities).
    #[fuchsia::test]
    async fn route_all() {
        // Test several capability types.
        let components = vec![
            (
                "root",
                ComponentDeclBuilder::new_empty_component()
                    .use_(UseBuilder::runner().name("elf").source_static_child("my_child"))
                    .use_(
                        UseBuilder::protocol()
                            .source(UseSource::Framework)
                            .name("fuchsia.component.Realm"),
                    )
                    .expose(
                        ExposeBuilder::runner()
                            .name("elf")
                            .target_name("exposed_elf")
                            .source_static_child("my_child"),
                    )
                    .expose(
                        ExposeBuilder::resolver()
                            .name("qax.qux")
                            .target_name("foo.buz")
                            .source_static_child("my_child"),
                    )
                    .expose(ExposeBuilder::dictionary().name("dict").source(ExposeSource::Self_))
                    .offer(
                        OfferBuilder::runner()
                            .name("elf")
                            .source_static_child("my_child")
                            .target(OfferTarget::Capability("dict".parse().unwrap())),
                    )
                    .dictionary_default("dict")
                    .child_default("my_child")
                    .build(),
            ),
            (
                "my_child",
                ComponentDeclBuilder::new()
                    .capability(CapabilityBuilder::resolver().name("qax.qux").path("/svc/qax.qux"))
                    .runner_default("elf")
                    .expose(ExposeBuilder::runner().name("elf").source(ExposeSource::Self_))
                    .expose(ExposeBuilder::resolver().name("qax.qux").source(ExposeSource::Self_))
                    .build(),
            ),
        ];

        let test = TestEnvironmentBuilder::new().set_components(components).build().await;
        let validator = route_validator(&test).await;

        test.model.start().await;

        // Validate the root, passing an empty vector. This should match all capabilities
        let mut results = validator.route(".", &[]).await.unwrap().unwrap();

        results.sort_by_key(|r| Key {
            capability: r.capability.clone().unwrap(),
            decl_type: r.decl_type.clone().unwrap(),
        });

        let report = results.remove(0);
        assert_matches!(
            report,
            fsys::RouteReport {
                outcome: Some(fsys::RouteOutcome::Success),
                capability: Some(s),
                decl_type: Some(fsys::DeclType::Use),
                source_moniker: Some(m),
                error: None,
                ..
            } if s == "<runner>" && m == "my_child"
        );

        let report = results.remove(0);
        assert_matches!(
            report,
            fsys::RouteReport {
                outcome: Some(fsys::RouteOutcome::Success),
                capability: Some(s),
                decl_type: Some(fsys::DeclType::Expose),
                source_moniker: Some(m),
                dictionary_entries: Some(d),
                error: None,
                ..
            } if s == "dict" && m == "." &&
                d == [fsys::DictionaryEntry {
                    name: Some("elf".into()),
                    ..Default::default()
                }]
        );

        let report = results.remove(0);
        assert_matches!(
            report,
            fsys::RouteReport {
                outcome: Some(fsys::RouteOutcome::Success),
                capability: Some(s),
                decl_type: Some(fsys::DeclType::Expose),
                source_moniker: Some(m),
                dictionary_entries: None,
                error: None,
                ..
            } if s == "dict/elf" && m == "my_child"
        );

        let report = results.remove(0);
        assert_matches!(
            report,
            fsys::RouteReport {
                outcome: Some(fsys::RouteOutcome::Success),
                capability: Some(s),
                decl_type: Some(fsys::DeclType::Expose),
                source_moniker: Some(m),
                error: None,
                ..
            } if s == "exposed_elf" && m == "my_child"
        );

        let report = results.remove(0);
        assert_matches!(
            report,
            fsys::RouteReport {
                outcome: Some(fsys::RouteOutcome::Success),
                capability: Some(s),
                decl_type: Some(fsys::DeclType::Expose),
                source_moniker: Some(m),
                error: None,
                ..
            } if s == "foo.buz" && m == "my_child"
        );

        let report = results.remove(0);
        assert_matches!(
            report,
            fsys::RouteReport {
                outcome: Some(fsys::RouteOutcome::Success),
                capability: Some(s),
                decl_type: Some(fsys::DeclType::Use),
                source_moniker: Some(m),
                error: None,
                ..
            } if s == "svc/fuchsia.component.Realm" && m == "."
        );

        assert!(results.is_empty());

        // Validate the child, passing an empty vector. Here we only care about checking that the
        // program's runner was routed.
        let mut results = validator.route("my_child", &[]).await.unwrap().unwrap();

        let report = results.remove(0);
        assert_matches!(
            report,
            fsys::RouteReport {
                outcome: Some(fsys::RouteOutcome::Success),
                capability: Some(s),
                decl_type: Some(fsys::DeclType::Use),
                source_moniker: Some(m),
                error: None,
                ..
            } if s == "<runner>" && m == "<component_manager>"
        );
    }

    /// Route API reports that routing succeeded normally, with a partial capability name (fuzzy
    /// match).
    #[fuchsia::test]
    async fn route_fuzzy() {
        let use_decl = UseBuilder::protocol()
            .source(UseSource::Framework)
            .name("fuchsia.component.Realm")
            .build();
        let use_decl2 = UseBuilder::protocol().source(UseSource::Self_).name("fuchsia.foo").build();
        let use_decl3 =
            UseBuilder::protocol().source(UseSource::Framework).name("no.match").build();
        let expose_from_child_decl = ExposeBuilder::protocol()
            .name("qax.qux")
            .target_name("fuchsia.buz")
            .source_static_child("my_child")
            .build();
        let expose_from_child_decl2 = ExposeBuilder::protocol()
            .name("qax.qux")
            .target_name("fuchsia.biz")
            .source_static_child("my_child")
            .build();
        let expose_from_child_decl3 =
            ExposeBuilder::protocol().name("no.match").source(ExposeSource::Framework).build();
        let expose_from_self_decl =
            ExposeBuilder::protocol().name("qax.qux").source(ExposeSource::Self_).build();

        let components = vec![
            (
                "root",
                ComponentDeclBuilder::new()
                    .use_(use_decl)
                    .use_(use_decl2)
                    .use_(use_decl3)
                    .expose(expose_from_child_decl)
                    .expose(expose_from_child_decl2)
                    .expose(expose_from_child_decl3)
                    .protocol_default("fuchsia.foo")
                    .child_default("my_child")
                    .build(),
            ),
            (
                "my_child",
                ComponentDeclBuilder::new()
                    .protocol_default("qax.qux")
                    .expose(expose_from_self_decl)
                    .build(),
            ),
        ];

        let test = TestEnvironmentBuilder::new().set_components(components).build().await;
        let validator = route_validator(&test).await;

        test.model.start().await;

        // Validate the root
        let targets = &[fsys::RouteTarget {
            name: "fuchsia.".parse().unwrap(),
            decl_type: fsys::DeclType::Any,
        }];
        let mut results = validator.route(".", targets).await.unwrap().unwrap();
        assert_eq!(results.len(), 4);

        results.sort_by_key(|r| Key {
            capability: r.capability.clone().unwrap(),
            decl_type: r.decl_type.clone().unwrap(),
        });

        let report = results.remove(0);
        assert_matches!(
            report,
            fsys::RouteReport {
                outcome: Some(fsys::RouteOutcome::Success),
                capability: Some(s),
                decl_type: Some(fsys::DeclType::Expose),
                source_moniker: Some(m),
                error: None,
                ..
            } if s == "fuchsia.biz" && m == "my_child"
        );

        let report = results.remove(0);
        assert_matches!(
            report,
            fsys::RouteReport {
                outcome: Some(fsys::RouteOutcome::Success),
                capability: Some(s),
                decl_type: Some(fsys::DeclType::Expose),
                source_moniker: Some(m),
                error: None,
                ..
            } if s == "fuchsia.buz" && m == "my_child"
        );

        let report = results.remove(0);
        assert_matches!(
            report,
            fsys::RouteReport {
                outcome: Some(fsys::RouteOutcome::Success),
                capability: Some(s),
                decl_type: Some(fsys::DeclType::Use),
                source_moniker: Some(m),
                error: None,
                ..
            } if s == "svc/fuchsia.component.Realm" && m == "."
        );

        let report = results.remove(0);
        assert_matches!(
            report,
            fsys::RouteReport {
                outcome: Some(fsys::RouteOutcome::Success),
                capability: Some(s),
                decl_type: Some(fsys::DeclType::Use),
                source_moniker: Some(m),
                error: None,
                ..
            } if s == "svc/fuchsia.foo" && m == "."
        );

        // Validate the child (program runner)
        let targets = &[fsys::RouteTarget {
            name: "runner".parse().unwrap(),
            decl_type: fsys::DeclType::Any,
        }];
        let mut results = validator.route("my_child", targets).await.unwrap().unwrap();

        assert_eq!(results.len(), 1);
        let report = results.remove(0);
        assert_matches!(
            report,
            fsys::RouteReport {
                outcome: Some(fsys::RouteOutcome::Success),
                capability: Some(s),
                decl_type: Some(fsys::DeclType::Use),
                source_moniker: Some(m),
                error: None,
                ..
            } if s == "<runner>" && m == "<component_manager>"
        );
    }

    /// Route API reports that routing succeeded normally with a service capability, including
    /// returing service info.
    #[fuchsia::test]
    async fn route_service() {
        let offer_from_collection_decl = OfferBuilder::service()
            .name("my_service")
            .source(OfferSource::Collection("coll".parse().unwrap()))
            .target_static_child("target")
            .build();
        let expose_from_self_decl =
            ExposeBuilder::service().name("my_service").source(ExposeSource::Self_).build();
        let use_decl = UseBuilder::service().name("my_service").path("/svc/foo.bar").build();
        let capability_decl =
            CapabilityBuilder::service().name("my_service").path("/svc/foo.bar").build();

        let target_decl = ComponentDeclBuilder::new().use_(use_decl).build();
        let components = vec![
            (
                "root",
                ComponentDeclBuilder::new()
                    .offer(offer_from_collection_decl)
                    .collection_default("coll")
                    .child_default("target")
                    .build(),
            ),
            ("target", target_decl),
            (
                "child_a",
                ComponentDeclBuilder::new()
                    .capability(capability_decl.clone())
                    .expose(expose_from_self_decl.clone())
                    .build(),
            ),
            (
                "child_b",
                ComponentDeclBuilder::new()
                    .capability(capability_decl.clone())
                    .expose(expose_from_self_decl.clone())
                    .build(),
            ),
        ];

        let test = TestEnvironmentBuilder::new()
            .set_components(components)
            .set_realm_moniker(Moniker::root())
            .build()
            .await;
        let realm_proxy = test.realm_proxy.as_ref().unwrap();
        let validator = route_validator(&test).await;

        test.model.start().await;

        // Create two children in the collection, each exposing `my_service` with two instances.
        let collection_ref = fdecl::CollectionRef { name: "coll".parse().unwrap() };
        for name in &["child_a", "child_b"] {
            realm_proxy
                .create_child(
                    &collection_ref,
                    &child_decl(name),
                    fcomponent::CreateChildArgs::default(),
                )
                .await
                .unwrap()
                .unwrap();

            let mut out_dir = OutDir::new();
            out_dir.add_echo_protocol("/svc/foo.bar/instance_a/echo".parse().unwrap());
            out_dir.add_echo_protocol("/svc/foo.bar/instance_b/echo".parse().unwrap());
            test.mock_runner.add_host_fn(&format!("test:///{}", name), out_dir.host_fn());

            let child = test
                .model
                .root()
                .find_and_maybe_resolve(&format!("coll:{}", name).as_str().try_into().unwrap())
                .await
                .unwrap();
            child.ensure_started(&StartReason::Debug).await.unwrap();
        }

        // Open the service directory from `target` so that it gets instantiated.
        {
            let target = test
                .model
                .root()
                .find_and_maybe_resolve(&"target".try_into().unwrap())
                .await
                .unwrap();
            target.ensure_started(&StartReason::Debug).await.unwrap();
            test.mock_runner.wait_for_url("test:///target").await;
            let ns = test.mock_runner.get_namespace("test:///target").unwrap();
            let ns = ns.lock().await;
            // /pkg and /svc
            let mut ns = ns.clone().flatten();
            ns.sort();
            assert_eq!(ns.len(), 2);
            let ns = ns.remove(1);
            assert_eq!(ns.path.to_string(), "/svc");
            let svc_dir = ns.directory.into_proxy();
            fuchsia_fs::directory::open_directory(&svc_dir, "foo.bar", fio::PERM_READABLE)
                .await
                .unwrap();
        }

        let targets = &[fsys::RouteTarget {
            name: "foo.bar".parse().unwrap(),
            decl_type: fsys::DeclType::Use,
        }];
        let mut results = validator.route("target", targets).await.unwrap().unwrap();

        assert_eq!(results.len(), 1);

        let report = results.remove(0);
        assert_matches!(
            report,
            fsys::RouteReport {
                outcome: Some(fsys::RouteOutcome::Success),
                capability: Some(s),
                decl_type: Some(fsys::DeclType::Use),
                source_moniker: Some(m),
                service_instances: Some(_),
                error: None,
                ..
            } if s == "svc/foo.bar" && m == "."
        );
        let service_instances = report.service_instances.unwrap();
        assert_eq!(service_instances.len(), 4);
        // (child_id, instance_id)
        let pairs = vec![("a", "a"), ("a", "b"), ("b", "a"), ("b", "b")];
        for (service_instance, pair) in service_instances.into_iter().zip(pairs) {
            let (child_id, instance_id) = pair;
            assert_matches!(
                service_instance,
                fsys::ServiceInstance {
                    instance_name: Some(instance_name),
                    child_name: Some(child_name),
                    child_instance_name: Some(child_instance_name),
                    ..
                } if instance_name.len() == 32 &&
                    instance_name.chars().all(|c| c.is_ascii_hexdigit()) &&
                    child_name == format!("child `coll:child_{}`", child_id) &&
                    child_instance_name == format!("instance_{}", instance_id)
            );
        }
    }

    fn child_decl(name: &str) -> fdecl::Child {
        fdecl::Child {
            name: Some(name.to_owned()),
            url: Some(format!("test:///{}", name)),
            startup: Some(fdecl::StartupMode::Lazy),
            ..Default::default()
        }
    }

    /// Route API reports that routing failed and returns the error.
    #[fuchsia::test]
    async fn route_error() {
        let invalid_source_name_use_from_child_decl =
            UseBuilder::protocol().source_static_child("my_child").name("a").build();
        let invalid_source_name_expose_from_child_decl =
            ExposeBuilder::protocol().name("c").source_static_child("my_child").build();

        let components = vec![
            (
                "root",
                ComponentDeclBuilder::new()
                    .use_(invalid_source_name_use_from_child_decl)
                    .expose(invalid_source_name_expose_from_child_decl)
                    .child_default("my_child")
                    .build(),
            ),
            ("my_child", ComponentDeclBuilder::new().build()),
        ];

        let test = TestEnvironmentBuilder::new().set_components(components).build().await;
        let validator = route_validator(&test).await;

        test.model.start().await;

        // `my_child` should not be resolved right now
        let instance = test.model.root().find_resolved(&["my_child"].try_into().unwrap()).await;
        assert!(instance.is_none());

        let targets = &[
            fsys::RouteTarget { name: "a".parse().unwrap(), decl_type: fsys::DeclType::Use },
            fsys::RouteTarget { name: "c".parse().unwrap(), decl_type: fsys::DeclType::Expose },
        ];
        let mut results = validator.route(".", targets).await.unwrap().unwrap();
        assert_eq!(results.len(), 2);

        results.sort_by_key(|r| Key {
            capability: r.capability.clone().unwrap(),
            decl_type: r.decl_type.clone().unwrap(),
        });

        let report = results.remove(0);
        assert_matches!(
            report,
            fsys::RouteReport {
                outcome: Some(fsys::RouteOutcome::Failed),
                capability: Some(s),
                decl_type: Some(fsys::DeclType::Expose),
                source_moniker: None,
                error: Some(_),
                ..
            } if s == "c"
        );

        let report = results.remove(0);
        assert_matches!(
            report,
            fsys::RouteReport {
                outcome: Some(fsys::RouteOutcome::Failed),
                capability: Some(s),
                decl_type: Some(fsys::DeclType::Use),
                source_moniker: None,
                error: Some(_),
                ..
            } if s == "svc/a"
        );
    }
}
