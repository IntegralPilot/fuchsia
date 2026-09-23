// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result};
use serde::Serialize;
use std::path::Path;
use std::{env, fs};
use syn::spanned::Spanned;
use syn::visit::{self, Visit};
use syn::{ExprUnsafe, ImplItemFn, ItemFn, ItemImpl, ItemTrait, Macro, TraitItemFn};

const UNSAFE_REVIEW_MESSAGE: &str = "\
Changes to unsafe Rust code.

You can reach out to the Fuchsia Rust Unsafe Reviews
<fuchsia-rust-unsafe-reviews@google.com> (+1 in Gerrit) for help with the reviews..

See https://fuchsia.dev/fuchsia-src/development/languages/rust/unsafe";

#[derive(Debug, Serialize, PartialEq, Eq, Clone, Copy)]
pub enum UnsafeKind {
    /// An unsafe block (`unsafe { ... }`).
    Block,
    /// An unsafe trait definition (`unsafe trait X { ... }`).
    Trait,
    /// An unsafe function or method implementation (`unsafe fn f() { ... }`).
    FuncImpl,
    /// An unsafe function declaration in a trait (`trait X { unsafe fn f(); }`).
    TraitFuncDecl,
    /// An implementation of an unsafe trait (`unsafe impl X for Y { ... }`).
    TraitImpl,
    /// A macro call containing `unsafe` in its token stream (`macro!(... unsafe ...)`).
    Macro,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct Finding {
    pub path: String,
    pub kind: UnsafeKind,
    pub line: usize,
    pub end_line: usize,
    pub col: usize,
    pub end_col: usize,
    pub unsafe_line: usize,
    pub message: String,
}

struct UnsafeVisitor<'a> {
    path: &'a str,
    findings: Vec<Finding>,
}

impl<'a> UnsafeVisitor<'a> {
    fn record_span(&mut self, kind: UnsafeKind, span: proc_macro2::Span, unsafe_line: usize) {
        let start = span.start();
        let end = span.end();
        let col = start.column + 1;
        let mut end_col = end.column + 1;
        if start.line == end.line && col >= end_col {
            end_col = col + 1;
        }
        self.findings.push(Finding {
            path: self.path.to_string(),
            kind,
            line: start.line,
            end_line: end.line,
            col,
            end_col,
            unsafe_line,
            message: UNSAFE_REVIEW_MESSAGE.to_string(),
        });
    }
}

fn find_unsafe_in_token_stream(stream: proc_macro2::TokenStream) -> Option<usize> {
    for tree in stream {
        match tree {
            proc_macro2::TokenTree::Group(g) => {
                if let Some(line) = find_unsafe_in_token_stream(g.stream()) {
                    return Some(line);
                }
            }
            proc_macro2::TokenTree::Ident(ident) => {
                if ident == "unsafe" {
                    return Some(ident.span().start().line);
                }
            }
            _ => {}
        }
    }
    None
}

impl<'ast> Visit<'ast> for UnsafeVisitor<'_> {
    fn visit_expr_unsafe(&mut self, node: &'ast ExprUnsafe) {
        self.record_span(UnsafeKind::Block, node.span(), node.unsafe_token.span.start().line);
        visit::visit_expr_unsafe(self, node);
    }

    fn visit_item_fn(&mut self, node: &'ast ItemFn) {
        if let Some(token) = node.sig.unsafety {
            self.record_span(UnsafeKind::FuncImpl, node.span(), token.span.start().line);
        }
        visit::visit_item_fn(self, node);
    }

    fn visit_impl_item_fn(&mut self, node: &'ast ImplItemFn) {
        if let Some(token) = node.sig.unsafety {
            self.record_span(UnsafeKind::FuncImpl, node.span(), token.span.start().line);
        }
        visit::visit_impl_item_fn(self, node);
    }

    fn visit_trait_item_fn(&mut self, node: &'ast TraitItemFn) {
        if let Some(token) = node.sig.unsafety {
            self.record_span(UnsafeKind::TraitFuncDecl, node.span(), token.span.start().line);
        }
        visit::visit_trait_item_fn(self, node);
    }

    fn visit_item_trait(&mut self, node: &'ast ItemTrait) {
        if let Some(token) = node.unsafety {
            self.record_span(UnsafeKind::Trait, node.span(), token.span.start().line);
        }
        visit::visit_item_trait(self, node);
    }

    fn visit_item_impl(&mut self, node: &'ast ItemImpl) {
        if let Some(token) = node.unsafety {
            self.record_span(UnsafeKind::TraitImpl, node.span(), token.span.start().line);
        }
        visit::visit_item_impl(self, node);
    }

    fn visit_macro(&mut self, node: &'ast Macro) {
        if let Some(unsafe_line) = find_unsafe_in_token_stream(node.tokens.clone()) {
            self.record_span(UnsafeKind::Macro, node.span(), unsafe_line);
        }
        visit::visit_macro(self, node);
    }
}

pub fn analyze_content(path: &str, content: &str) -> Result<Vec<Finding>> {
    let syntax = match syn::parse_file(content) {
        Ok(file) => file,
        Err(_) => return Ok(Vec::new()),
    };
    let mut visitor = UnsafeVisitor { path, findings: Vec::new() };
    visitor.visit_file(&syntax);
    Ok(visitor.findings)
}

pub fn analyze_file(path: &Path) -> Result<Vec<Finding>> {
    let content =
        fs::read_to_string(path).with_context(|| format!("Failed to read {}", path.display()))?;
    analyze_content(&path.to_string_lossy(), &content)
}

fn main() -> Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();
    let mut all_findings = Vec::new();
    for arg in args {
        let path = Path::new(&arg);
        if path.is_file() {
            let findings = analyze_file(path)?;
            all_findings.extend(findings);
        }
    }
    let json = serde_json::to_string_pretty(&all_findings)?;
    println!("{}", json);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_safe_function() {
        let content = "fn safe_func(x: i32) -> i32 {\n    x + 1\n}";
        let findings = analyze_content("test.rs", content).unwrap();
        assert_eq!(findings, vec![]);
    }

    #[test]
    fn test_unsafe_block() {
        let content = "fn wrapper() {\n    unsafe {\n        let _ = 1;\n    }\n}";
        let findings = analyze_content("test.rs", content).unwrap();
        assert_eq!(
            findings,
            vec![Finding {
                path: "test.rs".to_string(),
                kind: UnsafeKind::Block,
                line: 2,
                end_line: 4,
                col: 5,
                end_col: 6,
                unsafe_line: 2,
                message: UNSAFE_REVIEW_MESSAGE.to_string(),
            }]
        );
    }

    #[test]
    fn test_unsafe_fn() {
        let content = "/// Doc comment\nunsafe fn dangerous() {}";
        let findings = analyze_content("test.rs", content).unwrap();
        assert_eq!(
            findings,
            vec![Finding {
                path: "test.rs".to_string(),
                kind: UnsafeKind::FuncImpl,
                line: 1,
                end_line: 2,
                col: 1,
                end_col: 25,
                unsafe_line: 2,
                message: UNSAFE_REVIEW_MESSAGE.to_string(),
            }]
        );
    }

    #[test]
    fn test_unsafe_trait() {
        let content = "unsafe trait Foo {}";
        let findings = analyze_content("test.rs", content).unwrap();
        assert_eq!(
            findings,
            vec![Finding {
                path: "test.rs".to_string(),
                kind: UnsafeKind::Trait,
                line: 1,
                end_line: 1,
                col: 1,
                end_col: 20,
                unsafe_line: 1,
                message: UNSAFE_REVIEW_MESSAGE.to_string(),
            }]
        );
    }

    #[test]
    fn test_unsafe_impl() {
        let content = "struct Bar;\nunsafe impl Send for Bar {}";
        let findings = analyze_content("test.rs", content).unwrap();
        assert_eq!(
            findings,
            vec![Finding {
                path: "test.rs".to_string(),
                kind: UnsafeKind::TraitImpl,
                line: 2,
                end_line: 2,
                col: 1,
                end_col: 28,
                unsafe_line: 2,
                message: UNSAFE_REVIEW_MESSAGE.to_string(),
            }]
        );
    }

    #[test]
    fn test_unsafe_trait_method() {
        let content = "trait Foo {\n    unsafe fn dangerous();\n}";
        let findings = analyze_content("test.rs", content).unwrap();
        assert_eq!(
            findings,
            vec![Finding {
                path: "test.rs".to_string(),
                kind: UnsafeKind::TraitFuncDecl,
                line: 2,
                end_line: 2,
                col: 5,
                end_col: 27,
                unsafe_line: 2,
                message: UNSAFE_REVIEW_MESSAGE.to_string(),
            }]
        );
    }

    #[test]
    fn test_unsafe_impl_method() {
        let content = "struct Foo;\nimpl Foo {\n    unsafe fn dangerous() {}\n}";
        let findings = analyze_content("test.rs", content).unwrap();
        assert_eq!(
            findings,
            vec![Finding {
                path: "test.rs".to_string(),
                kind: UnsafeKind::FuncImpl,
                line: 3,
                end_line: 3,
                col: 5,
                end_col: 29,
                unsafe_line: 3,
                message: UNSAFE_REVIEW_MESSAGE.to_string(),
            }]
        );
    }

    #[test]
    fn test_unsafe_macro() {
        let content = "my_macro!(foo, unsafe { 123 });";
        let findings = analyze_content("test.rs", content).unwrap();
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].kind, UnsafeKind::Macro);
    }

    #[test]
    fn test_nested_unsafe() {
        let content =
            "unsafe impl Send for Bar {\n    unsafe fn test() {\n        unsafe {}\n    }\n}";
        let findings = analyze_content("test.rs", content).unwrap();
        assert_eq!(
            findings,
            vec![
                Finding {
                    path: "test.rs".to_string(),
                    kind: UnsafeKind::TraitImpl,
                    line: 1,
                    end_line: 5,
                    col: 1,
                    end_col: 2,
                    unsafe_line: 1,
                    message: UNSAFE_REVIEW_MESSAGE.to_string(),
                },
                Finding {
                    path: "test.rs".to_string(),
                    kind: UnsafeKind::FuncImpl,
                    line: 2,
                    end_line: 4,
                    col: 5,
                    end_col: 6,
                    unsafe_line: 2,
                    message: UNSAFE_REVIEW_MESSAGE.to_string(),
                },
                Finding {
                    path: "test.rs".to_string(),
                    kind: UnsafeKind::Block,
                    line: 3,
                    end_line: 3,
                    col: 9,
                    end_col: 18,
                    unsafe_line: 3,
                    message: UNSAFE_REVIEW_MESSAGE.to_string(),
                },
            ]
        );
    }
}
