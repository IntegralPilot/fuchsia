// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
//
// Parses TRF Rust annotations into an Abstract Syntax Tree (AST).

use serde::{Deserialize, Serialize};
use syn::visit::{self, Visit};

/// Represents a parsed configuration value override from `#[trf::test(config(...))]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConfigValue {
    Boolean(bool),
    Integer(i64),
    String(String),
    Vector(Vec<ConfigValue>),
}

/// Key-value pair for a config override.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigOverride {
    pub key: String,
    pub value: ConfigValue,
}

/// Represents an argument for a mock control method.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlArg {
    pub name: String,
    pub rust_type: String,
}

/// Represents a control method that the client can use to manipulate the mock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlMethod {
    pub name: String,
    pub args: Vec<ControlArg>,
}

/// Represents a mock struct declaration (`#[trf::mock(protocol = "...")]`)
/// and its associated `#[trf::control]` implementation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MockDeclaration {
    pub struct_name: String,
    pub protocol: String,
    pub has_control_impl: bool,
    pub control_methods: Vec<ControlMethod>,
}

/// Represents a test function annotated with `#[trf::test(...)]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestFunction {
    pub function_name: String,
    pub config_overrides: Vec<ConfigOverride>,
    pub selected_mocks: Vec<String>,
    pub protocol_connections: Vec<String>,
}

/// Comprehensive result of parsing a Rust AST for TRF annotations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ParsedAstResult {
    pub mock_declarations: Vec<MockDeclaration>,
    pub test_functions: Vec<TestFunction>,
    pub protocol_connections: Vec<String>,
}

/// Helper to check if an attribute is of the form `#[trf::<name>(...)]` or `#[trf::<name>]`.
fn match_trf_attribute(attr: &syn::Attribute, name: &str) -> bool {
    let segments: Vec<String> =
        attr.path().segments.iter().map(|segment| segment.ident.to_string()).collect();
    (segments.len() == 2 && segments[0] == "trf" && segments[1] == name)
        || (segments.len() == 1 && segments[0] == format!("trf_{}", name))
}

/// Parse `#[trf::mock(protocol = "...")]` attribute.
fn parse_mock_attribute(attr: &syn::Attribute) -> syn::Result<String> {
    let mut protocol: Option<String> = None;

    attr.parse_nested_meta(|meta| {
        if meta.path.is_ident("protocol") {
            let lit_str: syn::LitStr = meta.value()?.parse()?;
            protocol = Some(lit_str.value());
            Ok(())
        } else {
            Err(meta.error("unsupported key in trf::mock attribute (expected 'protocol')"))
        }
    })?;

    protocol.ok_or_else(|| {
        syn::Error::new_spanned(attr, "missing 'protocol' parameter in trf::mock attribute")
    })
}

/// Parse `syn::Expr` into a `ConfigValue`.
fn parse_config_value_expr(expr: &syn::Expr) -> syn::Result<ConfigValue> {
    match expr {
        syn::Expr::Lit(syn::ExprLit { lit, .. }) => match lit {
            syn::Lit::Bool(lit_bool) => Ok(ConfigValue::Boolean(lit_bool.value)),
            syn::Lit::Int(lit_int) => {
                let parsed_int = lit_int.base10_parse::<i64>()?;
                Ok(ConfigValue::Integer(parsed_int))
            }
            syn::Lit::Str(lit_str) => Ok(ConfigValue::String(lit_str.value())),
            _ => Err(syn::Error::new_spanned(
                lit,
                "unsupported literal type for config override value",
            )),
        },
        syn::Expr::Unary(syn::ExprUnary { op: syn::UnOp::Neg(_), expr: inner_expr, .. }) => {
            if let syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Int(lit_int), .. }) =
                inner_expr.as_ref()
            {
                let parsed_int = lit_int.base10_parse::<i64>()?;
                Ok(ConfigValue::Integer(-parsed_int))
            } else {
                Err(syn::Error::new_spanned(
                    expr,
                    "unary negation in config override is only supported for integer literals",
                ))
            }
        }
        syn::Expr::Array(syn::ExprArray { elems, .. }) => {
            let mut vector_elements = Vec::with_capacity(elems.len());
            for elem in elems {
                vector_elements.push(parse_config_value_expr(elem)?);
            }
            Ok(ConfigValue::Vector(vector_elements))
        }
        _ => Err(syn::Error::new_spanned(expr, "invalid expression for config override value")),
    }
}

/// Parse `#[trf::test(config(...))]` attribute arguments.
fn parse_test_attribute(attr: &syn::Attribute) -> syn::Result<(Vec<ConfigOverride>, Vec<String>)> {
    if matches!(attr.meta, syn::Meta::Path(_)) {
        return Ok((Vec::new(), Vec::new()));
    }

    let mut config_overrides = Vec::new();
    let mut selected_mocks = Vec::new();

    attr.parse_nested_meta(|meta| {
        if meta.path.is_ident("config") {
            meta.parse_nested_meta(|config_meta| {
                let key_ident = config_meta.path.get_ident().ok_or_else(|| {
                    syn::Error::new_spanned(&config_meta.path, "expected identifier for config key")
                })?;
                let key = key_ident.to_string();
                let value_expr: syn::Expr = config_meta.value()?.parse()?;
                let config_value = parse_config_value_expr(&value_expr)?;
                config_overrides.push(ConfigOverride { key, value: config_value });
                Ok(())
            })?;
            Ok(())
        } else if meta.path.is_ident("mocks") {
            meta.parse_nested_meta(|mock_meta| {
                let mock_ident = mock_meta.path.get_ident().ok_or_else(|| {
                    syn::Error::new_spanned(&mock_meta.path, "expected identifier for mock name")
                })?;
                selected_mocks.push(mock_ident.to_string());
                Ok(())
            })?;
            Ok(())
        } else {
            Err(meta.error("unsupported key in trf::test attribute (expected 'config' or 'mocks')"))
        }
    })?;

    Ok((config_overrides, selected_mocks))
}

/// Visitor for finding `realm.connect_to_protocol::<P>()` call expressions.
struct ProtocolConnectionVisitor {
    protocol_connections: Vec<String>,
}

impl ProtocolConnectionVisitor {
    fn extract_protocol_from_generic_args(
        &mut self,
        generic_args: &syn::AngleBracketedGenericArguments,
    ) {
        for arg in &generic_args.args {
            if let syn::GenericArgument::Type(target_type) = arg {
                let type_string = quote::quote!(#target_type).to_string().replace(" ", "");
                self.protocol_connections.push(type_string);
            }
        }
    }
}

impl<'ast> Visit<'ast> for ProtocolConnectionVisitor {
    fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
        if node.method == "connect_to_protocol" {
            if let Some(generic_args) = &node.turbofish {
                self.extract_protocol_from_generic_args(generic_args);
            }
        }
        visit::visit_expr_method_call(self, node);
    }

    fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
        if let syn::Expr::Path(expr_path) = node.func.as_ref() {
            if let Some(last_segment) = expr_path.path.segments.last() {
                if last_segment.ident == "connect_to_protocol" {
                    if let syn::PathArguments::AngleBracketed(generic_args) =
                        &last_segment.arguments
                    {
                        self.extract_protocol_from_generic_args(generic_args);
                    }
                }
            }
        }
        visit::visit_expr_call(self, node);
    }
}

/// Recurses through a `syn::UseTree` to build a map of item aliases to fully resolved paths.
fn traverse_use(
    tree: &syn::UseTree,
    prefix: &str,
    map: &mut std::collections::HashMap<String, String>,
) {
    match tree {
        syn::UseTree::Path(path) => {
            let next_prefix = if prefix.is_empty() {
                path.ident.to_string()
            } else {
                format!("{}::{}", prefix, path.ident)
            };
            traverse_use(&path.tree, &next_prefix, map);
        }
        syn::UseTree::Rename(rename) => {
            let ident = rename.ident.to_string();
            let rename_id = rename.rename.to_string();
            let full_path =
                if prefix.is_empty() { ident } else { format!("{}::{}", prefix, ident) };
            map.insert(rename_id, full_path);
        }
        syn::UseTree::Name(use_name) => {
            let ident = use_name.ident.to_string();
            let full_path =
                if prefix.is_empty() { ident.clone() } else { format!("{}::{}", prefix, ident) };
            map.insert(ident, full_path);
        }
        syn::UseTree::Group(group) => {
            for item in &group.items {
                traverse_use(item, prefix, map);
            }
        }
        syn::UseTree::Glob(_) => {}
    }
}

/// Resolves an aliased or partially qualified type/protocol path against the collected `alias_map`.
///
/// If the first segment of `protocol` matches an entry in the map (e.g. from `use some::path as alias;`),
/// it is expanded to the full path. If no matching alias is found, the original string is returned unaltered.
fn resolve_alias(protocol: &str, alias_map: &std::collections::HashMap<String, String>) -> String {
    let parts: Vec<&str> = protocol.split("::").collect();
    if let Some(&first_part) = parts.first() {
        if let Some(resolved) = alias_map.get(first_part) {
            let mut resolved_str = resolved.clone();
            for part in parts.iter().skip(1) {
                resolved_str.push_str("::");
                resolved_str.push_str(part);
            }
            return resolved_str;
        }
    }
    protocol.to_string()
}

/// Extracts `#[trf::mock]` declarations from a struct item.
///
/// Parses the mock attributes to identify the implemented FIDL protocol, resolves any aliases internally,
/// and registers the baseline MockDeclaration into the map. Control methods will be appended later.
fn extract_mock_declaration(
    item_struct: &syn::ItemStruct,
    alias_map: &std::collections::HashMap<String, String>,
    mock_declarations_map: &mut std::collections::BTreeMap<String, MockDeclaration>,
) -> syn::Result<()> {
    let struct_name = item_struct.ident.to_string();
    for attr in &item_struct.attrs {
        if match_trf_attribute(attr, "mock") {
            let protocol = parse_mock_attribute(attr)?;
            let protocol = resolve_alias(&protocol, alias_map);
            mock_declarations_map.insert(
                struct_name.clone(),
                MockDeclaration {
                    struct_name: struct_name.clone(),
                    protocol,
                    has_control_impl: false,
                    control_methods: Vec::new(),
                },
            );
        }
    }
    Ok(())
}

/// Extracts test functions annotated with TRF attributes (`#[trf::test]`),
/// parses their configuration and selected mocks, and resolves any protocol
/// alias references detected in the function body.
fn extract_test_function(
    item_fn: &syn::ItemFn,
    alias_map: &std::collections::HashMap<String, String>,
    test_functions: &mut Vec<TestFunction>,
) -> syn::Result<()> {
    let function_name = item_fn.sig.ident.to_string();
    for attr in &item_fn.attrs {
        if match_trf_attribute(attr, "test") {
            let (config_overrides, selected_mocks) = parse_test_attribute(attr)?;
            let mut visitor = ProtocolConnectionVisitor { protocol_connections: Vec::new() };
            visitor.visit_item_fn(item_fn);
            let mut resolved_connections = Vec::new();
            for conn in visitor.protocol_connections {
                resolved_connections.push(resolve_alias(&conn, alias_map));
            }

            test_functions.push(TestFunction {
                function_name: function_name.clone(),
                config_overrides,
                selected_mocks,
                protocol_connections: resolved_connections,
            });
        }
    }
    Ok(())
}

/// Extracts control methods from an `impl` block by looking for the `#[trf::control]`
/// attribute either at the `impl` block level or on individual methods.
/// Appends the extracted methods to the provided `mock_declarations_map`.
fn extract_control_methods_from_impl(
    item_impl: &syn::ItemImpl,
    mock_declarations_map: &mut std::collections::BTreeMap<String, MockDeclaration>,
) {
    let is_impl_level_control =
        item_impl.attrs.iter().any(|attr| match_trf_attribute(attr, "control"));

    if let syn::Type::Path(type_path) = item_impl.self_ty.as_ref() {
        if let Some(target_segment) = type_path.path.segments.last() {
            let target_struct_name = target_segment.ident.to_string();

            let mut control_methods = Vec::new();
            for impl_item in &item_impl.items {
                if let syn::ImplItem::Fn(method) = impl_item {
                    let is_method_level_control =
                        method.attrs.iter().any(|attr| match_trf_attribute(attr, "control"));
                    if is_impl_level_control || is_method_level_control {
                        let mut args = Vec::new();
                        for input in &method.sig.inputs {
                            if let syn::FnArg::Typed(pat_type) = input {
                                if let syn::Pat::Ident(pat_ident) = &*pat_type.pat {
                                    let arg_name = pat_ident.ident.to_string();
                                    let ty = &pat_type.ty;
                                    let arg_type = quote::quote!(#ty).to_string().replace(" ", "");
                                    args.push(ControlArg { name: arg_name, rust_type: arg_type });
                                }
                            }
                        }
                        control_methods
                            .push(ControlMethod { name: method.sig.ident.to_string(), args });
                    }
                }
            }

            if is_impl_level_control || !control_methods.is_empty() {
                if let Some(mock_declaration) = mock_declarations_map.get_mut(&target_struct_name) {
                    mock_declaration.has_control_impl = true;
                    mock_declaration.control_methods.extend(control_methods);
                }
            }
        }
    }
}

/// Parses a string of Rust source code and extracts TRF mock declarations,
/// test function definitions with config overrides, and protocol connection calls.
pub fn parse_source(source_code: &str, mock_sources: &[String]) -> syn::Result<ParsedAstResult> {
    let mut mock_declarations_map: std::collections::BTreeMap<String, MockDeclaration> =
        std::collections::BTreeMap::new();
    let mut impl_blocks: Vec<syn::ItemImpl> = Vec::new();
    let mut test_functions: Vec<TestFunction> = Vec::new();
    let mut file_visitor = ProtocolConnectionVisitor { protocol_connections: Vec::new() };
    let mut all_sources = vec![source_code.to_string()];
    all_sources.extend_from_slice(mock_sources);
    let mut alias_map: std::collections::HashMap<String, String> = std::collections::HashMap::new();

    for source in &all_sources {
        let syntax_file = syn::parse_str::<syn::File>(&source)?;
        for item in &syntax_file.items {
            if let syn::Item::Use(item_use) = item {
                traverse_use(&item_use.tree, "", &mut alias_map);
            }
        }
    }

    // Traverse source files to extract structures and components
    for source in all_sources {
        let syntax_file = syn::parse_str::<syn::File>(&source)?;
        file_visitor.visit_file(&syntax_file);

        for item in syntax_file.items {
            match item {
                syn::Item::Struct(item_struct) => {
                    extract_mock_declaration(&item_struct, &alias_map, &mut mock_declarations_map)?;
                }
                syn::Item::Impl(item_impl) => {
                    impl_blocks.push(item_impl);
                }
                syn::Item::Fn(item_fn) => {
                    extract_test_function(&item_fn, &alias_map, &mut test_functions)?;
                }
                _ => {}
            }
        }
    }

    // Process impl blocks to correlate control implementations with mock declarations
    for item_impl in &impl_blocks {
        extract_control_methods_from_impl(item_impl, &mut mock_declarations_map);
    }

    let mut resolved_connections_global = Vec::new();
    for conn in &file_visitor.protocol_connections {
        let resolved = resolve_alias(conn, &alias_map);
        if !resolved_connections_global.contains(&resolved) {
            resolved_connections_global.push(resolved);
        }
    }

    Ok(ParsedAstResult {
        mock_declarations: mock_declarations_map.into_values().collect(),
        test_functions,
        protocol_connections: resolved_connections_global,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_single_mock_with_control_impl() {
        let code = r#"
            #[trf::mock(protocol = "fuchsia.examples.Echo")]
            pub struct MyEchoMock;

            #[trf::control]
            impl MyEchoMock {
                pub fn send_signal(&self) {}
            }
        "#;

        let result = parse_source(code, &[]).expect("should parse successfully");

        assert_eq!(result.mock_declarations.len(), 1);
        let mock = &result.mock_declarations[0];
        assert_eq!(mock.struct_name, "MyEchoMock");
        assert_eq!(mock.protocol, "fuchsia.examples.Echo");
        assert!(mock.has_control_impl);
        assert_eq!(mock.control_methods.len(), 1);
        assert_eq!(mock.control_methods[0].name, "send_signal");
        assert_eq!(mock.control_methods[0].args.len(), 0);
    }

    #[test]
    fn test_parse_test_function_config_overrides() {
        let code = r#"
            #[trf::test(config(
                enabled = true,
                max_retries = 3,
                environment = "staging",
                ports = [8080, 8081]
            ))]
            pub async fn my_test_function(realm: TestRealm) {
                let _ = realm.connect_to_protocol::<EchoMarker>();
            }
        "#;

        let result = parse_source(code, &[]).expect("should parse successfully");

        assert_eq!(result.test_functions.len(), 1);
        let test_fn = &result.test_functions[0];
        assert_eq!(test_fn.function_name, "my_test_function");
        assert_eq!(test_fn.config_overrides.len(), 4);
        assert_eq!(
            test_fn.config_overrides[0],
            ConfigOverride { key: "enabled".to_string(), value: ConfigValue::Boolean(true) }
        );
        assert_eq!(
            test_fn.config_overrides[1],
            ConfigOverride { key: "max_retries".to_string(), value: ConfigValue::Integer(3) }
        );
        assert_eq!(
            test_fn.config_overrides[2],
            ConfigOverride {
                key: "environment".to_string(),
                value: ConfigValue::String("staging".to_string()),
            }
        );
        assert_eq!(
            test_fn.config_overrides[3],
            ConfigOverride {
                key: "ports".to_string(),
                value: ConfigValue::Vector(vec![
                    ConfigValue::Integer(8080),
                    ConfigValue::Integer(8081),
                ]),
            }
        );
        assert_eq!(test_fn.protocol_connections, vec!["EchoMarker"]);
        assert!(test_fn.selected_mocks.is_empty());
        assert_eq!(result.protocol_connections, vec!["EchoMarker"]);
    }

    #[test]
    fn test_parse_multiple_mock_declarations() {
        let code = r#"
            #[trf::mock(protocol = "fuchsia.examples.Echo")]
            pub struct EchoMock;

            #[trf::mock(protocol = "fuchsia.examples.Other")]
            pub struct OtherMock;
        "#;

        let result = parse_source(code, &[]).expect("should parse successfully");

        assert_eq!(result.mock_declarations.len(), 2);
        assert_eq!(result.mock_declarations[1].struct_name, "OtherMock");
    }

    #[test]
    fn test_parse_config_value_types() {
        let code = r#"
            #[trf::test(config(my_bool = false, my_int = 42, my_str = "hello", my_vec = [1, 2]))]
            pub async fn test_config() {}
        "#;

        let result = parse_source(code, &[]).expect("should parse successfully");

        assert_eq!(result.test_functions.len(), 1);
        let config = &result.test_functions[0].config_overrides;
        assert_eq!(config.len(), 4);
        assert_eq!(config[0].value, ConfigValue::Boolean(false));
        assert_eq!(config[1].value, ConfigValue::Integer(42));
        assert_eq!(config[2].value, ConfigValue::String("hello".to_string()));
        assert_eq!(
            config[3].value,
            ConfigValue::Vector(vec![ConfigValue::Integer(1), ConfigValue::Integer(2)])
        );
    }

    #[test]
    fn test_parse_invalid_attribute_syntax_returns_spanned_error() {
        let code = r#"
            #[trf::mock(invalid_key = 123)]
            pub struct BadMock;
        "#;

        let result = parse_source(code, &[]);

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("unsupported key"));
    }
}
