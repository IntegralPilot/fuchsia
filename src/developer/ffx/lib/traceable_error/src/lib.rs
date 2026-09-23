// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
//! Deterministic Error Tracing Architecture (`traceable_error`)
//!
//! This crate provides the foundational traits and compile-time hashing mechanisms
//! required to establish a deterministic, traceable error hierarchy across distributed
//! systems and multi-layered software architectures (such as Fuchsia and `ffx`).
//!
//! ## Overview
//!
//! When dealing with deeply nested software stacks or distributed IPC boundaries
//! (e.g. FIDL, Overnet), errors often undergo type erasure or stringification. This crate
//! establishes a mechanism where each distinct error variant across independent crates
//! is assigned a stable string-based layer code in the format:
//! `{crate_name}::{enum_name}::{variant_name}`.
//!
//! By chaining these layer codes chronologically, diagnostic systems can reconstruct the exact
//! trajectory of a failure without relying on brittle string parsing or runtime type metadata.

/// Defines an error that can be deterministically traced through a distributed architecture.
///
/// Implementations of this trait (typically derived automatically via `#[derive(TraceableError)]`
/// on enums) are capable of recursively interrogating their underlying causal chain
/// and reporting a unified chronological history of layer codes.
///
/// Each layer code is represented by a structured `String` identifier:
/// `format!("{crate_name}::{enum_name}::{variant_name}")`.
///
/// This structured layout facilitates highly readable failure trajectory reconstruction
/// across distributed IPC boundaries and dynamic crate boundaries.
pub trait TraceableError: std::fmt::Debug + std::fmt::Display + 'static {
    /// Downcasts this trait object to a concrete type.
    fn as_any(&self) -> &dyn std::any::Any;
    /// Returns this specific layer's string identifier (format: CrateName::EnumName::EnumValue).
    fn layer_code(&self) -> String;

    /// Recursively interrogates underlying error sources to build the chronological array of layer codes.
    ///
    /// The resulting vector is ordered from outermost (most recent) layer to innermost (root cause).
    fn chain_codes(&self) -> Vec<String>;

    /// Formats the layer code vector into a standardized diagnostic string (e.g., `"Crate1::Enum1::Val1-Crate2::Enum2::Val2"`).
    fn diagnostic_code(&self) -> String {
        self.chain_codes().join("-")
    }

    /// Returns the underlying causal error, if any, as a dynamic `std::error::Error`.
    ///
    /// If your type also implements `std::error::Error`, you should override this
    /// to return `self.source()`. The `#[derive(TraceableError)]` macro does this automatically.
    fn source_error(&self) -> Option<&(dyn std::error::Error + 'static)> {
        None
    }
}

impl TraceableError for anyhow::Error {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn layer_code(&self) -> String {
        if let Some(boxed) = self.downcast_ref::<TraceableBox>() {
            boxed.layer_code()
        } else {
            "anyhow".to_string()
        }
    }

    fn chain_codes(&self) -> Vec<String> {
        if let Some(boxed) = self.downcast_ref::<TraceableBox>() {
            // If the anyhow::Error contains a TraceableBox, we traverse into it.
            // This intentionally bypasses the "anyhow" type-erasing transport layer
            // to focus on the semantic concrete error chain.
            boxed.chain_codes()
        } else {
            vec!["anyhow".to_string()]
        }
    }

    fn source_error(&self) -> Option<&(dyn std::error::Error + 'static)> {
        if let Some(boxed) = self.downcast_ref::<TraceableBox>() {
            std::error::Error::source(boxed)
        } else {
            self.source()
        }
    }
}

/// A concrete, sized encapsulation of a dynamic `TraceableError` trait object.
///
/// This wrapper acts as a type-erased boundary. It enables seamless bidirectional `?` operator
/// compatibility across dynamic crate boundaries, allowing concrete error enums (via `thiserror`)
/// and untyped conduits (`anyhow`) to nest inside each other without losing causal tracing history.
///
/// ## Example
///
/// ```rust
/// use traceable_error::{TraceableError, TraceableBox};
///
/// fn produce_anyhow() -> anyhow::Result<()> {
///     Err(anyhow::anyhow!("root failure"))
/// }
///
/// // Seamlessly converts the anyhow::Error into a TraceableBox trait object via ?
/// fn consume_box() -> Result<(), TraceableBox> {
///     produce_anyhow()?;
///     Ok(())
/// }
/// ```
// Note: TraceableBox intentionally does NOT implement TraceableError.
// This prevents double-boxing (e.g., wrapping a TraceableBox inside another TraceableBox)
// at compile time, as it will fail the `E: TraceableError` bound in the `From` implementation.
#[derive(Debug)]
pub struct TraceableBox(pub Box<dyn TraceableError + Send + Sync + 'static>);

impl<E: TraceableError + Send + Sync + 'static> From<E> for TraceableBox {
    fn from(err: E) -> Self {
        TraceableBox(Box::new(err))
    }
}

impl TraceableBox {
    pub fn as_any(&self) -> &dyn std::any::Any {
        self.0.as_any()
    }

    pub fn layer_code(&self) -> String {
        self.0.layer_code()
    }

    pub fn chain_codes(&self) -> Vec<String> {
        self.0.chain_codes()
    }

    pub fn diagnostic_code(&self) -> String {
        self.0.diagnostic_code()
    }
}

impl std::fmt::Display for TraceableBox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for TraceableBox {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.0.source_error()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct DummyError;
    impl std::fmt::Display for DummyError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "DummyError")
        }
    }
    impl TraceableError for DummyError {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
        fn layer_code(&self) -> String {
            "DummyError".to_string()
        }
        fn chain_codes(&self) -> Vec<String> {
            vec![self.layer_code()]
        }
    }

    #[test]
    fn test_traceable_error() {
        let _err = DummyError;
    }

    #[test]
    fn test_traceable_box_display_delegates() {
        #[derive(Debug)]
        struct InnerError;
        impl std::fmt::Display for InnerError {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "root failure")
            }
        }
        impl TraceableError for InnerError {
            fn as_any(&self) -> &dyn std::any::Any {
                self
            }
            fn layer_code(&self) -> String {
                "Inner".to_string()
            }
            fn chain_codes(&self) -> Vec<String> {
                vec![self.layer_code()]
            }
        }

        #[derive(Debug)]
        struct OuterError(TraceableBox);
        impl std::fmt::Display for OuterError {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "outer wrapper: {}", self.0)
            }
        }
        impl TraceableError for OuterError {
            fn as_any(&self) -> &dyn std::any::Any {
                self
            }
            fn layer_code(&self) -> String {
                "Outer".to_string()
            }
            fn chain_codes(&self) -> Vec<String> {
                let mut c = self.0.chain_codes();
                c.insert(0, self.layer_code());
                c
            }
        }

        let inner_box = TraceableBox::from(InnerError);
        assert_eq!(inner_box.to_string(), "root failure");
        assert_eq!(inner_box.diagnostic_code(), "Inner");

        let outer_box = TraceableBox::from(OuterError(inner_box));
        assert_eq!(outer_box.to_string(), "outer wrapper: root failure");
        assert_eq!(outer_box.diagnostic_code(), "Outer-Inner");
    }

    #[test]
    fn test_anyhow_traceable() {
        let err = anyhow::anyhow!("boom");
        assert_eq!(err.chain_codes().len(), 1);
        assert_eq!(err.chain_codes()[0], "anyhow");
    }
    #[test]
    fn test_traceable_box_conversion() {
        fn produce_anyhow() -> anyhow::Result<()> {
            Err(anyhow::anyhow!("root failure"))
        }

        fn consume_box() -> Result<(), TraceableBox> {
            produce_anyhow()?;
            Ok(())
        }

        let boxed_err = consume_box().unwrap_err();
        assert_eq!(boxed_err.chain_codes().len(), 1);
        assert_eq!(boxed_err.chain_codes()[0], "anyhow");
        assert_eq!(boxed_err.to_string(), "root failure");
    }

    #[test]
    fn test_nested_traceable_box_display() {
        let root_err = DummyError;
        let boxed_root: TraceableBox = root_err.into();
        let anyhow_err = anyhow::Error::new(boxed_root);
        let boxed_anyhow: TraceableBox = anyhow_err.into();

        let display_str = boxed_anyhow.to_string();
        assert_eq!(display_str, "DummyError");
    }
}
