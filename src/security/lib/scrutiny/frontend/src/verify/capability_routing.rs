// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::verify::{
    CapabilityRouteResults, ErrorResult, OkResult, ResultsBySeverity, ResultsForCapabilityType,
    WarningResult,
};
use anyhow::{Context, Result};
use argh::FromArgValue;
use cm_fidl_analyzer::component_instance::ComponentInstanceForAnalyzer;
use cm_fidl_analyzer::component_model::{AnalyzerModelError, ComponentModelForAnalyzer};
use cm_fidl_analyzer::route::VerifyRouteResult;
use cm_fidl_analyzer::{BreadthFirstModelWalker, ComponentInstanceVisitor, ComponentModelWalker};
use cm_rust::CapabilityTypeName;
use futures::FutureExt;
use routing::error::{ComponentInstanceError, PrettyPrintRef, RoutingError};
use scrutiny_collection::model::DataModel;
use scrutiny_collection::v2_component_model::V2ComponentModel;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Generic verification result type. Variants implement serialization.
enum ResultBySeverity {
    Error(ErrorResult),
    Warning(WarningResult),
    Ok(OkResult),
}

impl From<VerifyRouteResult> for ResultBySeverity {
    fn from(verify_route_result: VerifyRouteResult) -> Self {
        match verify_route_result.error {
            None => OkResult {
                using_node: verify_route_result.using_node,
                target_decl: verify_route_result.target_decl,
                capability: verify_route_result
                    .capability
                    .expect("successful route should have a capability"),
                source: verify_route_result.source.expect("successful route should have a source"),
            }
            .into(),
            Some(error) => {
                match error {
                    // It is expected that some components in a build may have
                    // children that are not included in the build.
                    AnalyzerModelError::ComponentInstanceError(
                        ComponentInstanceError::InstanceNotFound { .. },
                    )
                    | AnalyzerModelError::RoutingError(RoutingError::RouteSourceNotFound {
                        source: PrettyPrintRef::Child(_),
                        ..
                    }) => WarningResult {
                        using_node: verify_route_result.using_node,
                        target_decl: verify_route_result.target_decl,
                        capability: verify_route_result.capability,
                        warning: error.clone(),
                        message: error.to_string(),
                    }
                    .into(),
                    _ => ErrorResult {
                        using_node: verify_route_result.using_node,
                        target_decl: verify_route_result.target_decl,
                        capability: verify_route_result.capability,
                        error: error.clone(),
                        message: error.to_string(),
                    }
                    .into(),
                }
            }
        }
    }
}

impl From<OkResult> for ResultBySeverity {
    fn from(ok_result: OkResult) -> Self {
        Self::Ok(ok_result)
    }
}

impl From<WarningResult> for ResultBySeverity {
    fn from(warning_result: WarningResult) -> Self {
        Self::Warning(warning_result)
    }
}

impl From<ErrorResult> for ResultBySeverity {
    fn from(error_result: ErrorResult) -> Self {
        Self::Error(error_result)
    }
}

// CapabilityRouteController
//
// A DataController which verifies routes for all capabilities of the specified types.
#[derive(Default)]
pub struct CapabilityRouteController {}

// Configures the amount of information that `CapabilityRouteController` returns.
#[derive(Deserialize, Serialize, Debug, PartialEq)]
#[serde(rename = "lowercase")]
pub enum ResponseLevel {
    // Only return errors.
    Error,
    // Return errors and warnings.
    Warn,
    // Return errors, warnings, and a summary of each OK route.
    All,
    // Same as `All`; also include unstable `cm_rust` types.
    Verbose,
}

impl FromArgValue for ResponseLevel {
    fn from_arg_value(value: &str) -> Result<Self, String> {
        match value {
            "verbose" => Ok(Self::Verbose),
            "all" => Ok(Self::All),
            "warn" => Ok(Self::Warn),
            "error" => Ok(Self::Error),
            _ => Err(format!(
                "Unsupported response level \"{}\"; possible values are: \"verbose\", \"all\", \"warn\", \"error\".",
                value
            )),
        }
    }
}

// A visitor that checks the route for each capability in `model` whose type appears in `capability_types`.
struct CapabilityRouteVisitor {
    model: Arc<ComponentModelForAnalyzer>,
    capability_types: HashSet<CapabilityTypeName>,
    results: HashMap<CapabilityTypeName, Vec<VerifyRouteResult>>,
}

impl CapabilityRouteVisitor {
    fn new(
        model: Arc<ComponentModelForAnalyzer>,
        capability_types: HashSet<CapabilityTypeName>,
    ) -> Self {
        let mut results = HashMap::new();
        for type_name in capability_types.iter() {
            results.insert(type_name.clone(), vec![]);
        }
        Self { model, capability_types, results }
    }

    fn split_ok_warn_error_results(
        self,
    ) -> HashMap<CapabilityTypeName, (Vec<OkResult>, Vec<WarningResult>, Vec<ErrorResult>)> {
        let mut split_results = HashMap::new();
        for (type_name, type_results) in self.results.into_iter() {
            let mut ok_results = vec![];
            let mut warning_results = vec![];
            let mut error_results = vec![];

            for result in type_results.into_iter() {
                match result.into() {
                    ResultBySeverity::Ok(ok_result) => ok_results.push(ok_result),
                    ResultBySeverity::Warning(warning_result) => {
                        warning_results.push(warning_result)
                    }
                    ResultBySeverity::Error(error_result) => error_results.push(error_result),
                }
            }
            split_results.insert(type_name.clone(), (ok_results, warning_results, error_results));
        }
        split_results
    }

    fn report_results(self, level: &ResponseLevel) -> Vec<ResultsForCapabilityType> {
        let mut filtered_results = Vec::new();
        let split_results = self.split_ok_warn_error_results();
        for (type_name, (ok, warnings, errors)) in split_results.into_iter() {
            let filtered_for_type = match level {
                ResponseLevel::Error => ResultsBySeverity { errors, ..Default::default() },
                ResponseLevel::Warn => ResultsBySeverity { errors, warnings, ..Default::default() },
                ResponseLevel::All => ResultsBySeverity { errors, warnings, ok },
                ResponseLevel::Verbose => ResultsBySeverity { errors, warnings, ok },
            };
            filtered_results.push(ResultsForCapabilityType {
                capability_type: type_name.clone(),
                results: filtered_for_type,
            })
        }
        filtered_results
            .sort_by(|r, s| r.capability_type.to_string().cmp(&s.capability_type.to_string()));
        filtered_results
    }
}

impl ComponentInstanceVisitor for CapabilityRouteVisitor {
    fn visit_instance(&mut self, instance: &Arc<ComponentInstanceForAnalyzer>) -> Result<()> {
        let check_results = self
            .model
            .check_routes_for_instance(instance, &self.capability_types)
            .now_or_never()
            .unwrap();
        for (type_name, results) in check_results.into_iter() {
            let type_results =
                self.results.get_mut(&type_name).expect("expected results for capability type");
            for result in results.into_iter() {
                type_results.push(result);
            }
        }
        Ok(())
    }
}

impl CapabilityRouteController {
    pub fn get_results(
        model: Arc<DataModel>,
        capability_types: HashSet<CapabilityTypeName>,
        response_level: &ResponseLevel,
    ) -> Result<CapabilityRouteResults> {
        let component_model_data = Arc::clone(
            &model
                .get::<V2ComponentModel>()
                .context("Failed to get V2ComponentModel from CapabilityRouteController model")?,
        );
        let deps = component_model_data.deps.clone();
        let component_model = &component_model_data.component_model;
        let mut walker = BreadthFirstModelWalker::new();
        let mut visitor =
            CapabilityRouteVisitor::new(Arc::clone(component_model), capability_types);
        walker.walk(&component_model, &mut visitor).context(
            "Failed to walk V2ComponentModel with BreadthFirstModelWalker and CapabilityRouteVisitor",
        )?;
        let results = visitor.report_results(response_level);
        Ok(CapabilityRouteResults { deps, results })
    }
}
