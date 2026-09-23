// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::errors::ParseError;
use crate::types::{FromEnvAttributes, NamedField, NamedFieldTy};
use proc_macro2::{Span, TokenStream};
use quote::{ToTokens, quote, quote_spanned};
use syn::spanned::Spanned;
use syn::{ExprCall, Ident};

/// Creates the top-level struct declaration before any brackets are used.
///
/// This would be used like so in a quote:
/// ```rust
/// let struct_decl = StructDecl(&ast);
/// quote! {
///     #struct_decl {
///         /* ... */
///     }
/// }
/// ```
struct StructDecl<'a>(&'a syn::DeriveInput, syn::Type);

impl ToTokens for StructDecl<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let ast = self.0;
        let err_ty = &self.1;
        let struct_name = &ast.ident;
        let (impl_generics, ty_generics, where_clause) = ast.generics.split_for_impl();
        tokens.extend(quote! {
            #[fho::macro_deps::async_trait(?Send)]
            impl #impl_generics fho::subtool::FfxTool<#err_ty> for #struct_name #ty_generics #where_clause
        })
    }
}

/// Creates a TryFromEnv invocation for the given type.
enum TryFromEnvInvocation<'a> {
    Normal(&'a syn::Type),
    Decorated(ExprCall),
}

impl ToTokens for TryFromEnvInvocation<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        use TryFromEnvInvocation::*;
        match self {
            Normal(ty) => {
                let span = Span::call_site().located_at(ty.span());
                tokens.extend(quote_spanned! {span=>
                    fho::macro_deps::futures::future::TryFutureExt::map_err(
                        <#ty as fho::TryFromEnv>::try_from_env(&_env),
                        // TryFromEnv associated error type may be Infallible
                        // (e.g. for Result<T>, Deferred<T>, EnvironmentContext,
                        // etc.). Evaluating an uninhabited type diverges,
                        // making any subsequent code in the closure
                        // unreachable.
                        #[allow(unreachable_code)]
                        |e| fho::macro_deps::fho::Error::from(fho::macro_deps::anyhow::Error::new(e)),
                    )
                });
            }
            Decorated(expr) => {
                let span = Span::call_site().located_at(expr.span());
                tokens.extend(quote_spanned! {span=>
                    fho::macro_deps::futures::future::TryFutureExt::map_err(
                        fho::TryFromEnvWith::try_from_env_with(#expr, &_env),
                        // TryFromEnvWith associated error type may be
                        // Infallible. Evaluating an uninhabited type diverges,
                        // making any subsequent code in the closure
                        // unreachable.
                        #[allow(unreachable_code)]
                        |e| fho::macro_deps::fho::Error::from(fho::macro_deps::anyhow::Error::new(e)),
                    )
                });
            }
        }
    }
}

/// Declares a command type which is used for the TryFromEnv trait.
///
/// Creates `type Command = FooType;`
struct CommandFieldTypeDecl<'a>(NamedField<'a>);

impl ToTokens for CommandFieldTypeDecl<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let command_field = &self.0;
        let ty = command_field.field_ty;
        let span = Span::call_site().located_at(ty.span());
        tokens.extend(quote_spanned! {span=>
            type Command = #ty;
        })
    }
}

/// Contains a collection of variables that will be created using TryFromEnv.
///
/// Each variable will have an assertion for the type (used to surface errors to
/// the appropriate span), the names of the values that are going to be joined,
/// and the try_from_env invocations of each type.
///
/// The `ToTokens` implementation of this struct only creates a `let (foo, bar) =
/// try_join!(Foo::try_from_env(_env), Bar::try_from_env(_env)` invocation.
///
/// This struct is used in a sort of sandwich fashion. Here's an example:
///
/// ```rust
/// let vcc = VariableCollection::new();
/// /* fill it with values using the impl API */
/// let asserts = vcc.try_from_env_type_assertions;
/// let results_names = join_results_names;
/// quote! {
/// #(#asserts)*
/// fn try_from_env(_env: FhoEnvironment) -> Result<Self> {
///
///     #vcc
///
///     Self {
///         #(#results_names,)*
///     }
/// }
/// }
/// ```
///
/// This will create the variables using an allocation statement, then allow for their
/// names to be put into the struct (as these are intended to be derived from the struct
/// field names).
#[derive(Default)]
struct VariableCreationCollection<'a> {
    join_results_names: Vec<&'a syn::Ident>,
    try_from_env_invocations: Vec<TryFromEnvInvocation<'a>>,
}

impl<'a> VariableCreationCollection<'a> {
    fn new() -> Self {
        Self::default()
    }

    fn add_field(&mut self, field: NamedFieldTy<'a>) -> Result<(), ParseError> {
        use NamedFieldTy::*;
        match field {
            Blank(field) => {
                self.try_from_env_invocations.push(TryFromEnvInvocation::Normal(field.field_ty));
                self.join_results_names.push(field.field_name);
            }
            With(expr, field) => {
                self.try_from_env_invocations.push(TryFromEnvInvocation::Decorated(expr));
                self.join_results_names.push(field.field_name);
            }
            Command(field) => {
                return Err(ParseError::UnexpectedAttr(
                    "command".to_owned(),
                    field.field_ty.span(),
                ));
            }
        }
        Ok(())
    }
}

impl ToTokens for VariableCreationCollection<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let Self { join_results_names, try_from_env_invocations, .. } = self;
        let join_results_statement = if join_results_names.is_empty() {
            quote!()
        } else if join_results_names.len() == 1 {
            quote! {
                let #(#join_results_names),* = #(#try_from_env_invocations),*.await?;
            }
        } else {
            quote! {
                let (#(#join_results_names),*) = fho::macro_deps::futures::try_join!(#(#try_from_env_invocations),*)?;
            }
        };
        tokens.extend(join_results_statement);
    }
}

struct CheckCollection(Vec<ExprCall>);

impl ToTokens for CheckCollection {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        for check in &self.0 {
            let check_span = Span::call_site().located_at(check.span());
            tokens.extend(quote_spanned! { check_span =>
                fho::CheckEnv::check_env(#check, &_env).await?;
            })
        }
    }
}

enum TargetConnection {
    Direct,
    None,
    Default,
}
pub struct NamedFieldStruct<'a> {
    command_field_decl: CommandFieldTypeDecl<'a>,
    struct_decl: StructDecl<'a>,
    checks: CheckCollection,
    vcc: VariableCreationCollection<'a>,
    connection: TargetConnection,
}

fn extract_command_field<'a>(
    fields: &mut Vec<NamedFieldTy<'a>>,
) -> Result<NamedField<'a>, ParseError> {
    let command_field_idx = fields
        .iter()
        .position(|f| matches!(f, NamedFieldTy::Command(_)))
        .ok_or_else(|| ParseError::CommandRequired(Span::call_site()))?;
    let NamedFieldTy::Command(f) = fields.remove(command_field_idx) else { unreachable!() };
    Ok(f)
}

fn parse_target_connection(parent_ast: &syn::DeriveInput) -> Result<TargetConnection, ParseError> {
    let mut state = TargetConnection::Default;
    let mut found = false;
    for attr in &parent_ast.attrs {
        if attr.path().is_ident("target") {
            if found {
                return Err(ParseError::InvalidTargetAttr(
                    attr.span(),
                    "Only one #[target] attribute is allowed".into(),
                ));
            }
            found = true;
            let ident: Ident = match attr.parse_args() {
                Ok(ident) => ident,
                Err(e) => {
                    return Err(ParseError::InvalidTargetAttr(
                        attr.span(),
                        format!("Invalid #[target] argument: {e}"),
                    ));
                }
            };
            match ident.to_string().as_str() {
                "direct" => state = TargetConnection::Direct,
                "None" => state = TargetConnection::None,
                _ => {
                    return Err(ParseError::InvalidTargetAttr(
                        ident.span(),
                        "Expected `direct` or `None`".into(),
                    ));
                }
            }
        }
    }
    Ok(state)
}

fn parse_main_error(parent_ast: &syn::DeriveInput) -> Result<syn::Type, ParseError> {
    let mut main_error = None;
    let mut found = false;
    for attr in &parent_ast.attrs {
        if attr.path().is_ident("main_error") {
            if found {
                return Err(ParseError::DuplicateAttr(attr.span()));
            }
            found = true;
            let parsed_type = attr.parse_args::<syn::Type>().map_err(|e| {
                ParseError::InvalidTargetAttr(
                    attr.span(),
                    format!("Invalid #[main_error] argument: {e}"),
                )
            })?;
            main_error = Some(parsed_type);
        }
    }
    Ok(main_error.unwrap_or_else(|| syn::parse_quote! { fho::macro_deps::fho::Error }))
}

impl<'a> NamedFieldStruct<'a> {
    pub fn new(
        parent_ast: &'a syn::DeriveInput,
        fields: &'a syn::FieldsNamed,
    ) -> Result<Self, ParseError> {
        let mut fields =
            fields.named.iter().map(NamedFieldTy::parse).collect::<Result<Vec<_>, ParseError>>()?;
        let command_field_decl = CommandFieldTypeDecl(extract_command_field(&mut fields)?);
        let err_ty = parse_main_error(parent_ast)?;
        let struct_decl = StructDecl(&parent_ast, err_ty);
        let attrs = FromEnvAttributes::from_attrs(&parent_ast.attrs)?;
        let connection = parse_target_connection(parent_ast)?;
        let checks = CheckCollection(attrs.checks);
        let mut vcc = VariableCreationCollection::new();
        for field in fields.into_iter() {
            vcc.add_field(field)?;
        }
        Ok(Self { command_field_decl, struct_decl, checks, vcc, connection })
    }
}

impl ToTokens for NamedFieldStruct<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let Self { command_field_decl, struct_decl, checks, vcc, connection } = self;
        let requires_target = !matches!(connection, TargetConnection::None);
        let command_field_name = command_field_decl.0.field_name;
        let join_results_names = &vcc.join_results_names;
        let span = Span::call_site();
        let connection_initialization = match connection {
            TargetConnection::None | TargetConnection::Default => quote! {},
            TargetConnection::Direct => quote! {
                {
                    let mut target_env = target_behavior::target_interface(&_env);
                    let _behavior = target_env.set_direct();
                }
            },
        };
        let res = quote_spanned! {span=>
            #struct_decl {
                #command_field_decl
                async fn from_env(
                    _env: fho::FhoEnvironment,
                    cmd: Self::Command,
                ) -> Result<Self, fho::Error> {
                    #checks

                    #connection_initialization

                    #vcc

                    Ok(Self {
                        #(#join_results_names,)*
                        #command_field_name: cmd
                    })
                }

                fn supports_structured_output(&self) -> bool {
                    <<Self as fho::FfxMain>::Writer as fho::macro_deps::writer::ToolIO>::supports_structured_output()
                }
                fn has_schema(&self) -> bool {
                    <<Self as fho::FfxMain>::Writer as fho::macro_deps::writer::ToolIO>::has_schema()
                }
                fn requires_target() -> bool {
                    #requires_target
                }
            }
        };
        tokens.extend(res);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::parse_macro_derive;

    #[test]
    fn test_vcc_empty() {
        let vcc = VariableCreationCollection::new();
        assert_eq!("", vcc.into_token_stream().to_string());
    }

    #[test]
    fn test_vcc_single_element() {
        let ast = parse_macro_derive(
            r#"
            #[derive(FfxTool)]
            struct Foo {
                bar: u32,
            }
            "#,
        );
        let ds = crate::extract_struct_info(&ast).unwrap();
        let syn::Fields::Named(fields) = &ds.fields else { unreachable!() };
        let mut fields = fields
            .named
            .iter()
            .map(NamedFieldTy::parse)
            .collect::<Result<Vec<_>, ParseError>>()
            .unwrap();
        let mut vcc = VariableCreationCollection::new();
        vcc.add_field(fields.remove(0)).expect("correct kind of field");
        assert_eq!(
            quote! {
                let bar = fho::macro_deps::futures::future::TryFutureExt::map_err(
                    <u32 as fho::TryFromEnv>::try_from_env(&_env),
                    // TryFromEnvWith associated error type may be Infallible.
                    // Evaluating an uninhabited type diverges, making any
                    // subsequent code in the closure unreachable.
                    #[allow(unreachable_code)]
                    |e| fho::macro_deps::fho::Error::from(fho::macro_deps::anyhow::Error::new(e)),
                ).await?;
            }
            .to_string(),
            vcc.into_token_stream().to_string(),
        );
    }

    #[test]
    fn test_vcc_multiple_elements() {
        let ast = parse_macro_derive(
            r#"
            #[derive(FfxTool)]
            struct Foo {
                bar: u32,
                baz: u8,
            }
            "#,
        );
        let ds = crate::extract_struct_info(&ast).unwrap();
        let syn::Fields::Named(fields) = &ds.fields else { unreachable!() };
        let mut fields = fields
            .named
            .iter()
            .map(NamedFieldTy::parse)
            .collect::<Result<Vec<_>, ParseError>>()
            .unwrap();
        let mut vcc = VariableCreationCollection::new();
        vcc.add_field(fields.remove(0)).expect("correct kind of field");
        vcc.add_field(fields.remove(0)).expect("correct kind of field");
        assert_eq!(
            quote! {
                let (bar, baz) = fho::macro_deps::futures::try_join!(
                    fho::macro_deps::futures::future::TryFutureExt::map_err(
                        <u32 as fho::TryFromEnv>::try_from_env(&_env),
                        // TryFromEnvWith associated error type may be
                        // Infallible. Evaluating an uninhabited type diverges,
                        // making any subsequent code in the closure
                        // unreachable.
                        #[allow(unreachable_code)]
                        |e| fho::macro_deps::fho::Error::from(fho::macro_deps::anyhow::Error::new(e)),
                    ),
                    fho::macro_deps::futures::future::TryFutureExt::map_err(
                        <u8 as fho::TryFromEnv>::try_from_env(&_env),
                        // TryFromEnvWith associated error type may be
                        // Infallible. Evaluating an uninhabited type diverges,
                        // making any subsequent code in the closure
                        // unreachable.
                        #[allow(unreachable_code)]
                        |e| fho::macro_deps::fho::Error::from(fho::macro_deps::anyhow::Error::new(e)),
                    )
                )?;
            }.to_string(),
            vcc.into_token_stream().to_string(),
        );
    }

    #[test]
    fn test_vcc_with() {
        let ast = parse_macro_derive(
            r#"
            #[derive(FfxTool)]
            struct Foo {
                #[with(something("stuff"))]
                bar: u32,
            }
            "#,
        );
        let ds = crate::extract_struct_info(&ast).unwrap();
        let syn::Fields::Named(fields) = &ds.fields else { unreachable!() };
        let mut fields = fields
            .named
            .iter()
            .map(NamedFieldTy::parse)
            .collect::<Result<Vec<_>, ParseError>>()
            .unwrap();
        let mut vcc = VariableCreationCollection::new();
        vcc.add_field(fields.remove(0)).expect("correct kind of field");
        assert_eq!(
            quote! {
                let bar = fho::macro_deps::futures::future::TryFutureExt::map_err(
                    fho::TryFromEnvWith::try_from_env_with(something("stuff"), &_env),
                    // TryFromEnvWith associated error type may be Infallible.
                    // Evaluating an uninhabited type diverges, making any
                    // subsequent code in the closure unreachable.
                    #[allow(unreachable_code)]
                    |e| fho::macro_deps::fho::Error::from(fho::macro_deps::anyhow::Error::new(e)),
                ).await?;
            }
            .to_string(),
            vcc.into_token_stream().to_string(),
        );
    }

    #[test]
    fn test_no_target() {
        let ast = parse_macro_derive(
            r#"
            #[derive(FfxTool)]
            #[target(None)]
            struct Foo {
                #[command]
                bar: bool,
            }
            "#,
        );
        let ds = crate::extract_struct_info(&ast).unwrap();
        let syn::Fields::Named(fields) = &ds.fields else {
            unreachable!();
        };
        let nfs = NamedFieldStruct::new(&ast, &fields).unwrap();
        assert!(matches!(nfs.connection, TargetConnection::None));
    }

    #[test]
    fn test_direct_target() {
        let ast = parse_macro_derive(
            r#"
            #[derive(FfxTool)]
            #[target(direct)]
            struct Foo {
                #[command]
                bar: bool,
            }
            "#,
        );
        let ds = crate::extract_struct_info(&ast).unwrap();
        let syn::Fields::Named(fields) = &ds.fields else {
            unreachable!();
        };
        let nfs = NamedFieldStruct::new(&ast, &fields).unwrap();
        assert!(matches!(nfs.connection, TargetConnection::Direct));
    }

    #[test]
    fn test_no_no_target() {
        let ast = parse_macro_derive(
            r#"
            #[derive(FfxTool)]
            struct Foo {
                #[command]
                bar: bool,
            }
            "#,
        );
        let ds = crate::extract_struct_info(&ast).unwrap();
        let syn::Fields::Named(fields) = &ds.fields else {
            unreachable!();
        };
        let nfs = NamedFieldStruct::new(&ast, &fields).unwrap();
        assert!(matches!(nfs.connection, TargetConnection::Default));
    }

    #[test]
    fn test_main_error_specified() {
        let ast = parse_macro_derive(
            r#"
            #[derive(FfxTool)]
            #[main_error(MyCoolError)]
            struct Foo {
                #[command]
                bar: bool,
            }
            "#,
        );
        let ds = crate::extract_struct_info(&ast).unwrap();
        let syn::Fields::Named(_) = &ds.fields else {
            unreachable!();
        };
        let err_ty = parse_main_error(&ast).unwrap();
        let expected_ty: syn::Type = syn::parse_quote! { MyCoolError };
        assert_eq!(err_ty.to_token_stream().to_string(), expected_ty.to_token_stream().to_string());
    }

    #[test]
    fn test_main_error_default() {
        let ast = parse_macro_derive(
            r#"
            #[derive(FfxTool)]
            struct Foo {
                #[command]
                bar: bool,
            }
            "#,
        );
        let ds = crate::extract_struct_info(&ast).unwrap();
        let syn::Fields::Named(_) = &ds.fields else {
            unreachable!();
        };
        let err_ty = parse_main_error(&ast).unwrap();
        let expected_ty: syn::Type = syn::parse_quote! { fho::macro_deps::fho::Error };
        assert_eq!(err_ty.to_token_stream().to_string(), expected_ty.to_token_stream().to_string());
    }

    #[test]
    fn test_main_error_duplicate() {
        let ast = parse_macro_derive(
            r#"
            #[derive(FfxTool)]
            #[main_error(MyCoolError)]
            #[main_error(AnotherError)]
            struct Foo {
                #[command]
                bar: bool,
            }
            "#,
        );
        let ds = crate::extract_struct_info(&ast).unwrap();
        let syn::Fields::Named(fields) = &ds.fields else {
            unreachable!();
        };
        let res = NamedFieldStruct::new(&ast, &fields);
        assert!(matches!(res, Err(ParseError::DuplicateAttr(_))));
    }
}
