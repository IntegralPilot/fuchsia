// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use traceable_error::{TraceableBox, TraceableError};
use traceable_error_derive::TraceableError;

#[derive(Debug, thiserror::Error, TraceableError)]
enum SimpleError {
    #[error("CodeZero")]
    CodeZero,
    #[error("CodeOne")]
    CodeOne,
}

#[derive(Debug, thiserror::Error)]
#[error("RootError")]
struct RootError;

impl TraceableError for RootError {
    fn as_any(&self) -> &dyn ::std::any::Any {
        self
    }
    fn layer_code(&self) -> String {
        "RootError".to_string()
    }
    fn chain_codes(&self) -> Vec<String> {
        vec![self.layer_code()]
    }
}

#[derive(Debug, thiserror::Error, TraceableError)]
enum FromError {
    #[error("FromZero")]
    FromZero,
}

#[derive(Debug, thiserror::Error, TraceableError)]
enum ChainedError {
    #[error("Wrapped: {0}")]
    Wrapped(#[source] RootError),
    #[error("FromError: {0}")]
    FromError(#[from] FromError),
}

#[derive(Debug, thiserror::Error, TraceableError)]
enum OpaqueError {
    #[trace(opaque)]
    #[error("Wrapped: {0}")]
    Wrapped(#[source] RootError),
}

#[test]
fn test_derive_simple() {
    let err0 = SimpleError::CodeZero;
    let err1 = SimpleError::CodeOne;

    let codes0 = err0.chain_codes();
    let codes1 = err1.chain_codes();

    assert_eq!(codes0.len(), 1);
    assert_eq!(codes1.len(), 1);
    assert!(codes0[0].ends_with("::SimpleError::CodeZero"));
    assert!(codes1[0].ends_with("::SimpleError::CodeOne"));
}

#[test]
fn test_derive_chained() {
    let err = ChainedError::Wrapped(RootError);
    let codes = err.chain_codes();

    assert_eq!(codes.len(), 2);
    assert!(codes[0].ends_with("::ChainedError::Wrapped")); // ChainedError variant 0
    assert_eq!(codes[1], "RootError"); // RootError
}

#[test]
fn test_derive_chained_from() {
    let err = ChainedError::FromError(FromError::FromZero);
    let codes = err.chain_codes();

    assert_eq!(codes.len(), 2);
    assert!(codes[0].ends_with("::ChainedError::FromError")); // ChainedError variant 1
    assert!(codes[1].ends_with("::FromError::FromZero")); // FromError
}

#[test]
fn test_derive_opaque() {
    let err = OpaqueError::Wrapped(RootError);
    let codes = err.chain_codes();

    assert_eq!(codes.len(), 1);
    assert!(codes[0].ends_with("::OpaqueError::Wrapped")); // OpaqueError variant 0
}

#[derive(Debug, thiserror::Error, TraceableError)]
enum OuterError {
    #[error("FromInner {0}")]
    FromInner(#[source] InnerError),
}

#[derive(Debug, thiserror::Error, TraceableError)]
enum InnerError {
    #[error("InnerOne")]
    InnerOne,
}

#[test]
fn test_nested_error_chain() {
    let test_err = OuterError::FromInner(InnerError::InnerOne);
    let codes = test_err.chain_codes();
    assert_eq!(codes.len(), 2);
    assert!(codes[0].ends_with("::OuterError::FromInner"));
    assert!(codes[1].ends_with("::InnerError::InnerOne"));
}

#[derive(Debug, thiserror::Error, TraceableError)]
enum ChainedBoxError {
    #[error("WrappedBox")]
    WrappedBox(#[source] TraceableBox),
}

#[test]
fn test_derive_chained_box() {
    let root_err = SimpleError::CodeZero;
    let boxed_root: TraceableBox = root_err.into();
    let err = ChainedBoxError::WrappedBox(boxed_root);
    let codes = err.chain_codes();

    assert_eq!(codes.len(), 2);
    assert!(codes[0].ends_with("::ChainedBoxError::WrappedBox"));
    assert!(codes[1].ends_with("::SimpleError::CodeZero"));
}
