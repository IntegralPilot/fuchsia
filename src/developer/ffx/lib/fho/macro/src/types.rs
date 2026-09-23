// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::errors::ParseError;
use syn::ExprCall;
use syn::spanned::Spanned;

pub(crate) fn attr_name(attr: &syn::Attribute) -> Result<String, ParseError> {
    if attr.path().segments.len() == 1 {
        Ok(attr.path().segments[0].ident.to_string())
    } else {
        Err(ParseError::InvalidAttr(attr.span()))
    }
}

#[derive(Clone, Debug)]
pub struct NamedField<'a> {
    pub field_ty: &'a syn::Type,
    pub field_name: &'a syn::Ident,
}

#[derive(Clone, Debug)]
pub enum NamedFieldTy<'a> {
    // No attr, so not interested.
    Blank(NamedField<'a>),
    // Is denoted as #[command].
    Command(NamedField<'a>),
    // Uses a decorator for adding context to the tryfrom
    With(ExprCall, NamedField<'a>),
}

impl<'a> NamedFieldTy<'a> {
    pub fn parse(field: &'a syn::Field) -> Result<Self, ParseError> {
        let mut res = Option::<Self>::None;
        let field_name = field.ident.as_ref().expect("field missing ident in struct");
        let field_ty = &field.ty;
        for attr in &field.attrs {
            match attr_name(attr)?.as_str() {
                "command" => {
                    if res.is_some() {
                        return Err(ParseError::DuplicateAttr(attr.span()));
                    }
                    res.replace(Self::Command(NamedField { field_ty, field_name }));
                }
                "with" => {
                    if res.is_some() {
                        return Err(ParseError::DuplicateAttr(attr.span()));
                    }
                    let expr =
                        attr.parse_args().map_err(|_| ParseError::InvalidWithAttr(attr.span()))?;
                    res.replace(Self::With(expr, NamedField { field_ty, field_name }));
                }
                _ => {} // ignore unknown attributes (like derive)
            }
        }
        Ok(res.unwrap_or(Self::Blank(NamedField { field_ty, field_name })))
    }
}

#[derive(Debug)]
pub struct FromEnvAttributes {
    pub checks: Vec<ExprCall>,
}

impl FromEnvAttributes {
    pub fn from_attrs(attrs: &Vec<syn::Attribute>) -> Result<Self, ParseError> {
        let mut checks = Vec::new();
        for attr in attrs.iter() {
            match attr_name(attr)?.as_str() {
                "check" => checks.push(
                    attr.parse_args().map_err(|_| ParseError::InvalidCheckAttr(attr.span()))?,
                ),
                name @ ("with" | "command") => {
                    return Err(ParseError::UnexpectedAttr(name.to_owned(), attr.span()));
                }
                _ => {} // ignore unknown attributes
            }
        }
        Ok(Self { checks })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::parse_macro_derive;

    #[test]
    fn test_parse_ffx_attr_ty_check() {
        let ast = parse_macro_derive(
            r#"
            #[derive(FfxTool)]
            #[check(ThingamaBobber("with-a-string"))]
            struct Foo {}
            "#,
        );
        let checks = FromEnvAttributes::from_attrs(&ast.attrs).unwrap().checks;
        assert_eq!(checks.len(), 1, "Expected a check attribute");
    }

    #[test]
    fn test_parse_ffx_attr_ty_check_invalid() {
        let ast = parse_macro_derive(
            r#"
            #[derive(FfxTool)]
            #[check = ThingamaBobber("with-a-string")]
            struct Foo {}
            "#,
        );
        assert!(
            matches!(
                FromEnvAttributes::from_attrs(&ast.attrs),
                Err(ParseError::InvalidCheckAttr(_))
            ),
            "Expected error parsing invalid check"
        );
    }

    #[test]
    fn test_parse_ffx_attr_ty_check_empty() {
        let ast = parse_macro_derive(
            r#"
            #[derive(FfxTool)]
            #[check]
            struct Foo {}
            "#,
        );
        assert!(
            matches!(
                FromEnvAttributes::from_attrs(&ast.attrs),
                Err(ParseError::InvalidCheckAttr(_))
            ),
            "Expected error parsing invalid check"
        );
    }

    #[test]
    fn test_parse_ffx_attr_with_invalid_place() {
        let ast = parse_macro_derive(
            r#"
            #[derive(FfxTool)]
            #[with(something("with-a-string"))]
            struct Foo {}
            "#,
        );
        assert!(
            matches!(
                FromEnvAttributes::from_attrs(&ast.attrs),
                Err(ParseError::UnexpectedAttr(attr, _)) if attr == "with",
            ),
            "Expected error parsing invalid check"
        );
    }
}
