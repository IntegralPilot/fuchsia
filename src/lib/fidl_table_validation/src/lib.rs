// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fidl table validation tools.
//!
//! This crate's macro generates code to validate fidl tables.
//!
//! Import using `fidl_table_validation::*` to inherit the macro's imports.
//!
//! ## Basic Example
//!
//! ```
//! // Some fidl table defined somewhere...
//! struct FidlTable {
//!     required: Option<usize>,
//!     optional: Option<usize>,
//!     has_default: Option<usize>,
//! }
//!
//! #[derive(ValidFidlTable)]
//! #[fidl_table_src(FidlHello)]
//! struct ValidatedFidlTable {
//!     // The default is #[fidl_field_type(required)]
//!     required: usize,
//!     #[fidl_field_type(optional)]
//!     optional: Option<usize>,
//!     #[fidl_field_type(default = 22)]
//!     has_default: usize,
//! }
//! ```
//!
//! This code generates a [TryFrom][std::convert::TryFrom]<FidlTable>
//! implementation for `ValidatedFidlTable`:
//!
//! ```
//! pub enum FidlTableValidationError {
//!     MissingField(FidlTableMissingFieldError)
//! }
//!
//! impl TryFrom<FidlTable> for ValidatedFidlTable {
//!     type Error = FidlTableValidationError;
//!     fn try_from(src: FidlTable) -> Result<ValidatedFidlTable, Self::Error> { .. }
//! }
//! ```
//! and also a [From][std::convert::From]<ValidatedFidlTable> implementation for
//! `FidlTable`, so you can get a `FidlTable` using `validated.into()`.
//!
//! ## Custom Validations
//!
//! When tables have logical relationships between fields that must be checked,
//! you can use a custom validator:
//!
//! ```
//! struct FidlTableValidator;
//!
//! impl Validate<ValidatedFidlTable> for FidlTableValidator {
//!     type Error = String; // Can be any error type.
//!     fn validate(candidate: &ValidatedFidlTable) -> Result<(), Self::Error> {
//!         match candidate.required {
//!             10 => {
//!                 Err(String::from("10 is not a valid value!"))
//!             }
//!             _ => Ok(())
//!         }
//!     }
//! }
//!
//! #[fidl_table_src(FidlHello)]
//! #[fidl_table_validator(FidlTableValidator)]
//! struct ValidFidlTable {
//! ...
//! ```
//!
//! ## Non-literal defaults
//!
//! Attribute syntax for `name = value` only supports literals. Another
//! attribute for expressing defaults is used for consts. Or any type that has a
//! `Default` impl can simply omit the literal.
//!
//! ```
//! const MY_DEFAULT: MyEnum = MyEnum::MyVariant;
//!
//! #[derive(ValidFidlTable)]
//! #[fidl_table_src(FidlHello)]
//! struct ValidatedFidlTable {
//!     #[fidl_field_type(default)]
//!     has_default_impl: Vec<u32>,
//!     #[fidl_field_with_default(MY_DEFAULT)]
//!     has_default: MyEnum,
//! }
//! ```
//!
//! ## Custom converters
//!
//! Custom converters (implementing [`Converter`]) can be used to validate the
//! fields. A `converter` field type acts like `required` (i.e. the derive
//! handles the optionality) and passes the unwrapped option to the given
//! converter. An `optional_converter` field type acts like `optional` and the
//! converter is given (and expects to generate back) `Option` wrapped values.
//!
//! Example:
//!
//! ```
//! struct RequiredConverter;
//!
//! impl Converter for RequiredConverter {
//!     type Fidl = u32;
//!     type Validated = NonZeroU32;
//!     type Error = anyhow::Error;
//!     fn try_from_fidl(value: Self::Fidl) -> std::result::Result<Self::Validated, Self::Error> {
//!        NonZeroU32::new(value).ok_or_else(|| anyhow::anyhow!("bad zero value"))
//!     }
//!     fn from_validated(validated: Self::Validated) -> Self::Fidl {
//!        validated.get()
//!     }
//! }
//!
//! struct OptionalConverter;
//!
//! enum MaybeMissing { Present(u32), Missing }
//!
//! impl Converter for OptionalConverter {
//!    type Fidl = Option<u32>;
//!    type Validated = MaybeMissing;
//!    type Error = std::convert::Infallible;
//!     fn try_from_fidl(value: Self::Fidl) -> std::result::Result<Self::Validated, Self::Error> {
//!        Ok(match value {
//!           Some(v) => MaybeMissing::Present(v),
//!           None => MaybeMissing::Missing,
//!        })
//!     }
//!     fn from_validated(validated: Self::Validated) -> Self::Fidl {
//!        match validated {
//!          MaybeMissing::Present(v) => Some(v),
//!          MaybeMissing::Missing => None,
//!        }
//!     }
//! }
//!
//! #[derive(ValidFidlTable)]
//! #[fidl_table_src(FidlHello)]
//! #[fidl_table_strict]
//! struct ValidatedFidlTable {
//!     #[fidl_field_type(converter = RequiredConverter)]
//!     foo: NonZeroU32,
//!     #[fidl_field_type(optional_converter = OptionalConverter)]
//!     bar: MaybeMissing,
//! }
//!
//! ```
//!
//! ## Strict conversion
//!
//! By default, this derive does _not_ cause compilation errors if the source
//! FIDL table has more fields than the validated struct. This behavior can be
//! changed with the `fidl_table_strict` attribute. For example, the snippet
//! below fails to compile if `FidlHello` has more fields than the ones in
//! `ValidatedFidlTable`.
//!
//! ```
//! #[derive(ValidFidlTable)]
//! #[fidl_table_src(FidlHello)]
//! #[fidl_table_strict]
//! struct ValidatedFidlTable {
//!     hello: u32,
//! }
//! ```
//!
//! Fields from the FIDL table can be explicitly ignored by giving
//! `fidl_table_strict` arguments:
//!
//! ```
//! #[derive(ValidFidlTable)]
//! #[fidl_table_src(FidlHello)]
//! #[fidl_table_strict(world)]
//! struct ValidatedFidlTable {
//!     hello: u32,
//! }
//! ```
//!
//! This adds a `Logical(YourErrorType)` variant to the generated error enum.
// TODO(turnage): Infer optionality based on parsing for
//                "Option<" in field types.
pub use fidl_table_validation_derive::ValidFidlTable;

pub use anyhow;

/// Validations on `T` that can be run during construction of a validated
/// fidl table by adding the attribute `#[fidl_table_validator(YourImpl)]`
/// to the valid struct.
pub trait Validate<T> {
    type Error;
    fn validate(candidate: &T) -> std::result::Result<(), Self::Error>;
}

/// A converter trait that can convert from FIDL types `T` into a validated type
/// `Validated`.
///
/// Used in conjunction with `fidl_field_type` with custom converters.
pub trait Converter {
    type Fidl;
    type Validated;
    type Error;
    fn try_from_fidl(value: Self::Fidl) -> std::result::Result<Self::Validated, Self::Error>;
    fn from_validated(validated: Self::Validated) -> Self::Fidl;
}
