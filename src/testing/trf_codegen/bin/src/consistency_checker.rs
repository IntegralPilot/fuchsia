// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
//
// Validates the parsed TRF AST against explicit component manifest routing rules.

use crate::parser::{ConfigValue, ParsedAstResult};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

const INJECTABLE_UNIVERSE_MONIKER: &str = "IU";
const COMPONENT_UNDER_TEST_MONIKER: &str = "CUT";
const TEST_DRIVER_MONIKER: &str = "TD";

/// Expected configuration types for Component Under Test (CUT) manifest fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConfigType {
    Boolean,
    Integer,
    String,
    Vector,
}

/// Representation of a compiled `.cm` manifest for the Component Under Test (CUT).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ComponentManifest {
    pub config_schema: HashMap<String, ConfigType>,
    pub used_protocols: Vec<String>,
    pub exposed_protocols: Vec<String>,
}

/// Capability route automatically inferred between components in the TRF topology.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityRoute {
    pub protocol: String,
    pub source: String,
    pub target: String,
}

/// Collection of automatically inferred capability routes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct AutoRoutes {
    pub routes: Vec<CapabilityRoute>,
}

/// Linting error or warning encountered during manifest validation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LintError {
    UnknownConfigKey {
        key: String,
        suggestion: Option<String>,
        function_name: String,
    },
    ConfigTypeMismatch {
        key: String,
        expected: ConfigType,
        actual: ConfigType,
        function_name: String,
    },
    UnmatchedMockProtocol {
        protocol: String,
        struct_name: String,
    },
}

impl std::fmt::Display for LintError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LintError::UnknownConfigKey { key, suggestion, function_name } => {
                if let Some(sug) = suggestion {
                    write!(
                        f,
                        "Unknown config key '{key}' in test function '{function_name}'. Did you mean '{sug}'?"
                    )
                } else {
                    write!(f, "Unknown config key '{key}' in test function '{function_name}'.")
                }
            }
            LintError::ConfigTypeMismatch { key, expected, actual, function_name } => {
                write!(
                    f,
                    "Config type mismatch for key '{key}' in test function '{function_name}': expected {expected:?}, got {actual:?}."
                )
            }
            LintError::UnmatchedMockProtocol { protocol, struct_name } => {
                write!(
                    f,
                    "Mock struct '{struct_name}' targets protocol '{protocol}', but this protocol is not in the CUT's used protocols."
                )
            }
        }
    }
}

impl std::error::Error for LintError {}

/// Calculates the Levenshtein edit distance between two string slices.
fn calculate_levenshtein_distance(string_a: &str, string_b: &str) -> usize {
    let chars_a: Vec<char> = string_a.chars().collect();
    let chars_b: Vec<char> = string_b.chars().collect();
    let length_a = chars_a.len();
    let length_b = chars_b.len();

    let mut distance_matrix = vec![vec![0usize; length_b + 1]; length_a + 1];

    for index_a in 0..=length_a {
        distance_matrix[index_a][0] = index_a;
    }
    for index_b in 0..=length_b {
        distance_matrix[0][index_b] = index_b;
    }

    for index_a in 1..=length_a {
        for index_b in 1..=length_b {
            let substitution_cost =
                if chars_a[index_a - 1] == chars_b[index_b - 1] { 0 } else { 1 };
            distance_matrix[index_a][index_b] = (distance_matrix[index_a - 1][index_b] + 1)
                .min(distance_matrix[index_a][index_b - 1] + 1)
                .min(distance_matrix[index_a - 1][index_b - 1] + substitution_cost);
        }
    }

    distance_matrix[length_a][length_b]
}

/// Finds the closest matching key in valid keys for a given unknown key to provide a helpful suggestion.
fn find_closest_config_key_suggestion<'a>(
    unknown_key: &str,
    valid_keys: impl Iterator<Item = &'a String>,
) -> Option<String> {
    let mut closest_match: Option<(&'a String, usize)> = None;

    for valid_key in valid_keys {
        let distance = calculate_levenshtein_distance(unknown_key, valid_key);
        let is_prefix_or_suffix =
            valid_key.starts_with(unknown_key) || unknown_key.starts_with(valid_key);
        let maximum_allowed_distance = (unknown_key.len() / 2).max(3);

        if distance <= maximum_allowed_distance || is_prefix_or_suffix {
            if let Some((_, minimum_distance)) = closest_match {
                if distance < minimum_distance {
                    closest_match = Some((valid_key, distance));
                }
            } else {
                closest_match = Some((valid_key, distance));
            }
        }
    }

    closest_match.map(|(key, _)| key.clone())
}

/// Returns the corresponding `ConfigType` for a `ConfigValue`.
fn derive_config_type(value: &ConfigValue) -> ConfigType {
    match value {
        ConfigValue::Boolean(_) => ConfigType::Boolean,
        ConfigValue::Integer(_) => ConfigType::Integer,
        ConfigValue::String(_) => ConfigType::String,
        ConfigValue::Vector(_) => ConfigType::Vector,
    }
}

/// Compares a parsed test function against a Component Manifest to validate config overrides.
fn validate_config_overrides(
    ast: &ParsedAstResult,
    manifest: &ComponentManifest,
    lint_errors: &mut Vec<LintError>,
) {
    for test_function in &ast.test_functions {
        for config_override in &test_function.config_overrides {
            match manifest.config_schema.get(&config_override.key) {
                None => {
                    let suggestion = find_closest_config_key_suggestion(
                        &config_override.key,
                        manifest.config_schema.keys(),
                    );
                    lint_errors.push(LintError::UnknownConfigKey {
                        key: config_override.key.clone(),
                        suggestion,
                        function_name: test_function.function_name.clone(),
                    });
                }
                Some(expected_config_type) => {
                    let actual_config_type = derive_config_type(&config_override.value);
                    if &actual_config_type != expected_config_type {
                        lint_errors.push(LintError::ConfigTypeMismatch {
                            key: config_override.key.clone(),
                            expected: expected_config_type.clone(),
                            actual: actual_config_type,
                            function_name: test_function.function_name.clone(),
                        });
                    }
                }
            }
        }
    }
}

/// Matches the declared TRF mocks against the Component Manifest's used protocols.
fn validate_mocked_protocols(
    ast: &ParsedAstResult,
    manifest: &ComponentManifest,
    lint_errors: &mut Vec<LintError>,
) {
    for mock_declaration in &ast.mock_declarations {
        if !manifest
            .used_protocols
            .iter()
            .any(|used_protocol| used_protocol == &mock_declaration.protocol)
        {
            lint_errors.push(LintError::UnmatchedMockProtocol {
                protocol: mock_declaration.protocol.clone(),
                struct_name: mock_declaration.struct_name.clone(),
            });
        }
    }
}

/// Returns the fallback protocols when the manifest doesn't specify any exposed protocols.
fn get_fallback_protocols(ast: &ParsedAstResult) -> Vec<String> {
    let mut fallback_protocols = Vec::new();
    for conn in &ast.protocol_connections {
        // Simple heuristic to extract FIDL name from a generated Rust marker type.
        // e.g., `fidl_fuchsia_examples_reverser::ReverserMarker` ->
        //     `fuchsia.examples.reverser.Reverser`
        let parts: Vec<&str> = conn.split("::").collect();
        if parts.len() >= 2 {
            let pkg = parts.first().unwrap();
            let marker = parts.last().unwrap();
            if let Some(pkg_suffix) = pkg.strip_prefix("fidl_") {
                let mut fidl_name = pkg_suffix.replace("_", ".");
                fidl_name.push('.');
                if let Some(protocol) = marker.strip_suffix("Marker") {
                    fidl_name.push_str(protocol);
                    if !fallback_protocols.contains(&fidl_name) {
                        fallback_protocols.push(fidl_name);
                    }
                    continue;
                }
            }
        }
        if !fallback_protocols.contains(conn) {
            fallback_protocols.push(conn.clone());
        }
    }
    fallback_protocols
}

/// Evaluates verified configurations and infers the necessary dependency graph capacity paths.
fn infer_capability_routes(
    ast: &ParsedAstResult,
    manifest: Option<&ComponentManifest>,
) -> AutoRoutes {
    let mut inferred_routes = Vec::new();

    // Route mock protocols: Injectable Universe (IU) -> Component Under Test (CUT)
    for mock_declaration in &ast.mock_declarations {
        let route = CapabilityRoute {
            protocol: mock_declaration.protocol.clone(),
            source: INJECTABLE_UNIVERSE_MONIKER.to_string(),
            target: COMPONENT_UNDER_TEST_MONIKER.to_string(),
        };
        if !inferred_routes.contains(&route) {
            inferred_routes.push(route);
        }
    }

    // Route CUT exposed protocols: Component Under Test (CUT) -> Test Driver (TD)
    let exposed_protocols_to_route = if let Some(m) = manifest {
        if !m.exposed_protocols.is_empty() {
            m.exposed_protocols.clone()
        } else {
            get_fallback_protocols(ast)
        }
    } else {
        get_fallback_protocols(ast)
    };

    for exposed_protocol in &exposed_protocols_to_route {
        let route = CapabilityRoute {
            protocol: exposed_protocol.clone(),
            source: COMPONENT_UNDER_TEST_MONIKER.to_string(),
            target: TEST_DRIVER_MONIKER.to_string(),
        };
        if !inferred_routes.contains(&route) {
            inferred_routes.push(route);
        }
    }

    AutoRoutes { routes: inferred_routes }
}

/// Validates parsed AST test annotations against a compiled Component Under Test (`.cm`) manifest.
///
/// Returns auto-inferred capability routes on success, or a list of lint errors/warnings on
/// failure.
pub fn check_consistency(
    ast: &ParsedAstResult,
    manifest: Option<&ComponentManifest>,
) -> Result<AutoRoutes, Vec<LintError>> {
    let mut lint_errors = Vec::new();

    if let Some(m) = manifest {
        validate_config_overrides(ast, m, &mut lint_errors);
        validate_mocked_protocols(ast, m, &mut lint_errors);
    }

    if !lint_errors.is_empty() {
        return Err(lint_errors);
    }

    Ok(infer_capability_routes(ast, manifest))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::{ConfigOverride, MockDeclaration, TestFunction};

    #[test]
    fn test_lint_config_valid_keys_and_types_passes() {
        let mut config_schema = HashMap::new();
        config_schema.insert("enable_feature".to_string(), ConfigType::Boolean);
        config_schema.insert("max_threads".to_string(), ConfigType::Integer);
        config_schema.insert("log_prefix".to_string(), ConfigType::String);
        config_schema.insert("allowed_ports".to_string(), ConfigType::Vector);

        let manifest = ComponentManifest {
            config_schema,
            used_protocols: vec!["fuchsia.logger.LogSink".to_string()],
            exposed_protocols: vec!["fuchsia.example.Echo".to_string()],
        };

        let ast = ParsedAstResult {
            mock_declarations: vec![MockDeclaration {
                struct_name: "MockLogger".to_string(),
                protocol: "fuchsia.logger.LogSink".to_string(),
                has_control_impl: false,
                control_methods: vec![],
            }],
            test_functions: vec![TestFunction {
                function_name: "test_valid_config".to_string(),
                config_overrides: vec![
                    ConfigOverride {
                        key: "enable_feature".to_string(),
                        value: ConfigValue::Boolean(true),
                    },
                    ConfigOverride {
                        key: "max_threads".to_string(),
                        value: ConfigValue::Integer(4),
                    },
                    ConfigOverride {
                        key: "log_prefix".to_string(),
                        value: ConfigValue::String("TEST_PREFIX".to_string()),
                    },
                    ConfigOverride {
                        key: "allowed_ports".to_string(),
                        value: ConfigValue::Vector(vec![ConfigValue::Integer(8080)]),
                    },
                ],
                selected_mocks: vec![],
                protocol_connections: vec![],
            }],
            protocol_connections: vec![],
        };

        let result = check_consistency(&ast, Some(&manifest));

        assert!(result.is_ok(), "Expected linting to pass with valid config keys and types");
    }

    #[test]
    fn test_lint_config_unknown_key_fails_with_suggestion() {
        let mut config_schema = HashMap::new();
        config_schema.insert("fix_capitalization".to_string(), ConfigType::Boolean);

        let manifest =
            ComponentManifest { config_schema, used_protocols: vec![], exposed_protocols: vec![] };

        let ast = ParsedAstResult {
            mock_declarations: vec![],
            test_functions: vec![TestFunction {
                function_name: "test_unknown_key".to_string(),
                config_overrides: vec![ConfigOverride {
                    key: "fix_capital".to_string(),
                    value: ConfigValue::Boolean(true),
                }],
                selected_mocks: vec![],
                protocol_connections: vec![],
            }],
            protocol_connections: vec![],
        };

        let result = check_consistency(&ast, Some(&manifest));

        assert!(result.is_err(), "Expected linting to fail due to unknown config key");
        let errors = result.unwrap_err();
        assert_eq!(errors.len(), 1);
        match &errors[0] {
            LintError::UnknownConfigKey { key, suggestion, function_name } => {
                assert_eq!(key, "fix_capital");
                assert_eq!(suggestion.as_deref(), Some("fix_capitalization"));
                assert_eq!(function_name, "test_unknown_key");
            }
            _ => panic!("Expected LintError::UnknownConfigKey variant"),
        }
    }

    #[test]
    fn test_lint_config_type_mismatch_fails() {
        let mut config_schema = HashMap::new();
        config_schema.insert("max_threads".to_string(), ConfigType::Integer);

        let manifest =
            ComponentManifest { config_schema, used_protocols: vec![], exposed_protocols: vec![] };

        let ast = ParsedAstResult {
            mock_declarations: vec![],
            test_functions: vec![TestFunction {
                function_name: "test_type_mismatch".to_string(),
                config_overrides: vec![ConfigOverride {
                    key: "max_threads".to_string(),
                    value: ConfigValue::String("four".to_string()),
                }],
                selected_mocks: vec![],
                protocol_connections: vec![],
            }],
            protocol_connections: vec![],
        };

        let result = check_consistency(&ast, Some(&manifest));

        assert!(result.is_err(), "Expected linting to fail due to config type mismatch");
        let errors = result.unwrap_err();
        assert_eq!(errors.len(), 1);
        match &errors[0] {
            LintError::ConfigTypeMismatch { key, expected, actual, function_name } => {
                assert_eq!(key, "max_threads");
                assert_eq!(expected, &ConfigType::Integer);
                assert_eq!(actual, &ConfigType::String);
                assert_eq!(function_name, "test_type_mismatch");
            }
            _ => panic!("Expected LintError::ConfigTypeMismatch variant"),
        }
    }

    #[test]
    fn test_lint_unmatched_mock_protocol_warning() {
        let manifest = ComponentManifest {
            config_schema: HashMap::new(),
            used_protocols: vec!["fuchsia.logger.LogSink".to_string()],
            exposed_protocols: vec![],
        };

        let ast = ParsedAstResult {
            mock_declarations: vec![MockDeclaration {
                struct_name: "MockUnusedService".to_string(),
                protocol: "fuchsia.unused.UnusedProtocol".to_string(),
                has_control_impl: false,
                control_methods: vec![],
            }],
            test_functions: vec![],
            protocol_connections: vec![],
        };

        let result = check_consistency(&ast, Some(&manifest));

        assert!(
            result.is_err(),
            "Expected linting to fail/warn when mocked protocol is not used by CUT"
        );
        let errors = result.unwrap_err();
        assert_eq!(errors.len(), 1);
        match &errors[0] {
            LintError::UnmatchedMockProtocol { protocol, struct_name } => {
                assert_eq!(protocol, "fuchsia.unused.UnusedProtocol");
                assert_eq!(struct_name, "MockUnusedService");
            }
            _ => panic!("Expected LintError::UnmatchedMockProtocol variant"),
        }
    }

    #[test]
    fn test_auto_inferred_capability_routes() {
        let manifest = ComponentManifest {
            config_schema: HashMap::new(),
            used_protocols: vec!["fuchsia.emoji.EmojiReverser".to_string()],
            exposed_protocols: vec!["fuchsia.text.Text".to_string()],
        };

        let ast = ParsedAstResult {
            mock_declarations: vec![MockDeclaration {
                struct_name: "MockEmojiService".to_string(),
                protocol: "fuchsia.emoji.EmojiReverser".to_string(),
                has_control_impl: false,
                control_methods: vec![],
            }],
            test_functions: vec![],
            protocol_connections: vec![],
        };

        let result = check_consistency(&ast, Some(&manifest));

        assert!(result.is_ok(), "Expected linting to succeed and auto-infer routes");
        let auto_routes = result.unwrap();
        assert_eq!(auto_routes.routes.len(), 2);
        assert!(auto_routes.routes.contains(&CapabilityRoute {
            protocol: "fuchsia.emoji.EmojiReverser".to_string(),
            source: INJECTABLE_UNIVERSE_MONIKER.to_string(),
            target: COMPONENT_UNDER_TEST_MONIKER.to_string(),
        }));
        assert!(auto_routes.routes.contains(&CapabilityRoute {
            protocol: "fuchsia.text.Text".to_string(),
            source: COMPONENT_UNDER_TEST_MONIKER.to_string(),
            target: TEST_DRIVER_MONIKER.to_string(),
        }));
    }

    #[test]
    fn test_get_fallback_protocols_parsing() {
        let ast = ParsedAstResult {
            mock_declarations: vec![],
            test_functions: vec![],
            protocol_connections: vec![
                "fidl_fuchsia_driver_framework::TopologyMarker".to_string(),
                "fuchsia.unknown.Protocol".to_string(),
            ],
        };

        let fallbacks = get_fallback_protocols(&ast);

        assert_eq!(fallbacks.len(), 2);
        assert_eq!(fallbacks[0], "fuchsia.driver.framework.Topology");
        assert_eq!(fallbacks[1], "fuchsia.unknown.Protocol");
    }

    #[test]
    fn test_calculate_levenshtein_distance() {
        assert_eq!(calculate_levenshtein_distance("kitten", "sitting"), 3);
        assert_eq!(calculate_levenshtein_distance("flaw", "lawn"), 2);
    }

    #[test]
    fn test_find_closest_config_key_suggestion() {
        let valid = vec!["enable_foo".to_string(), "max_val".to_string()];

        // Minor typo
        assert_eq!(
            find_closest_config_key_suggestion("enable_fo", valid.iter()).as_deref(),
            Some("enable_foo")
        );

        // Entirely different string
        assert_eq!(find_closest_config_key_suggestion("totally_unrelated", valid.iter()), None);
    }
}
