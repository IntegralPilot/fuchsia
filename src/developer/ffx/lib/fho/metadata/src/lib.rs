// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use serde::{Deserialize, Serialize, de};

/// Metadata about an FHO-compliant ffx subtool
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Hash)]
pub struct FhoToolMetadata {
    /// The name of the subtool. Should be the same as the executable binary
    pub name: String,
    /// A brief description of the subtool. Should be one line long and suitable
    /// for including in help output.
    pub description: String,
    /// The minimum fho version this tool can support (details will be the maximum)
    pub requires_fho: u16,
    /// Further details about the tool's expected FHO interface version.
    pub fho_details: FhoDetails,
}

/// Metadata for versions of fho
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Hash)]
#[serde(untagged)]
#[non_exhaustive]
pub enum FhoDetails {
    /// Run the command as if it were a normal ffx invocation, with
    /// no real protocol to speak of. This is a transitionary option,
    /// and will be removed before we're ready to land external tools
    /// in the sdk (though tools will be free to implement it if
    /// it's useful to be able to run independently of ffx).
    FhoVersion0 {
        /// Only match a version 0 field
        version: Only<0>,
    },
    /// The currently active version, which should take any value for
    /// version other than legacy versions specified above this one.
    /// Note that currently FhoVersion1 is not fully specified, so
    /// this will have more details in it later.
    FhoVersion1 {
        /// Match any version higher than the current
        version: u16,
    },
}

/// Serializer/deserializer that's restricted to one possible value.
///
/// This lets us get around serde's inability to do tagged enums on non-string
/// discriminator values (ie. version 0, version 1, etc.) by using an untagged
/// union with a type that is uniquely mapped to a number.
///
/// It also allows us to get a 'real' other discriminant, since the built-in
/// serde(other) attribute only works on a unit variant.
#[derive(Clone, Copy, Default, Debug, Eq, PartialEq, PartialOrd, Ord, Hash)]
pub struct Only<const N: u16>;

impl FhoToolMetadata {
    /// Creates new metadata aligned to the current version and expectations of fho
    pub fn new(name: &str, description: &str) -> Self {
        let name = name.to_owned();
        let description = description.to_owned();
        let requires_fho = 0;
        let fho_details = Default::default();
        Self { name, description, requires_fho, fho_details }
    }

    /// Returns the json schema of this structure as a string
    pub const fn schema() -> &'static str {
        include_str!("../schema/fho_metadata.json")
    }
    /// Returns the json schema url of this structure
    pub const fn schema_id() -> &'static str {
        "http://fuchsia.com/schemas/ffx/fho_metadata.json"
    }
}

impl Default for FhoDetails {
    fn default() -> Self {
        FhoDetails::FhoVersion0 { version: Only }
    }
}

impl<const N: u16> Serialize for Only<N> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        Serialize::serialize(&N, serializer)
    }
}

impl<'de, const N: u16> Deserialize<'de> for Only<N> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let ver: u16 = Deserialize::deserialize(deserializer)?;
        if ver == N {
            Ok(Self)
        } else {
            Err(de::Error::invalid_type(
                de::Unexpected::Unsigned(ver as u64),
                &format!("{N}").as_str(),
            ))
        }
    }
}

impl FhoToolMetadata {
    /// Whether or not this library is capable of running the subtool based on its
    /// metadata (ie. the minimum fho version is met). Returns the version enum value
    /// we can run it at.
    pub fn is_supported(&self) -> Option<FhoDetails> {
        // Currently we only support fho version 0.
        if self.requires_fho == 0 { Some(FhoDetails::FhoVersion0 { version: Only }) } else { None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, from_value, json, to_value};
    use valico::json_schema;

    fn validate_fho_metadata(metadata: &FhoToolMetadata) -> json_schema::ValidationState {
        let schema =
            serde_json::from_str(FhoToolMetadata::schema()).expect("Schema file to parse as json");
        let metadata = serde_json::to_value(metadata).expect("Metadata to be translatable to json");

        let mut scope = json_schema::Scope::new();
        let validator =
            scope.compile_and_return(schema, /*ban_unknown=*/ true).expect("Schema to compile");
        let result = validator.validate(&metadata);
        println!("Report: {result:?}");
        result
    }

    #[test]
    fn fho_v0_schema_validates() {
        let valid_metadata = FhoToolMetadata {
            name: "mytool".to_string(),
            description: "My tool is the best tool".to_string(),
            requires_fho: 0,
            fho_details: FhoDetails::FhoVersion0 { version: Only },
        };

        let result = validate_fho_metadata(&valid_metadata);
        assert!(result.is_strictly_valid(), "Schema to be strictly valid");
    }

    #[test]
    fn fho_v0_schema_invalidates() {
        let valid_metadata = FhoToolMetadata {
            name: "".to_string(),
            description: "".to_string(),
            requires_fho: 0,
            fho_details: FhoDetails::FhoVersion0 { version: Only },
        };

        let result = validate_fho_metadata(&valid_metadata);
        assert!(!result.is_valid(), "Schema to be invalid");
    }

    #[test]
    fn only_enum_variants() -> Result<(), serde_json::Error> {
        #[derive(Serialize, Deserialize, Debug, Eq, PartialEq)]
        #[serde(untagged)]
        enum Testing {
            VariantOne {
                version: Only<1>,
            },
            VariantTwo {
                version: Only<2>,
            },
            VariantThree {
                version: Only<3>,
            },
            VariantOther {
                version: u16,
                #[serde(flatten)]
                details: Value,
            },
        }
        use Testing::*;

        assert_eq!(to_value(VariantOne { version: Only })?, json!({ "version": 1 }));
        assert_eq!(to_value(VariantTwo { version: Only })?, json!({ "version": 2 }));
        assert_eq!(to_value(VariantThree { version: Only })?, json!({ "version": 3 }));

        assert_eq!(from_value::<Testing>(json!({ "version": 1 }))?, VariantOne { version: Only });
        assert_eq!(from_value::<Testing>(json!({ "version": 2 }))?, VariantTwo { version: Only });
        assert_eq!(from_value::<Testing>(json!({ "version": 3 }))?, VariantThree { version: Only });
        assert_eq!(
            from_value::<Testing>(json!({ "version": 999, "some_other": "stuff" }))?,
            VariantOther { version: 999, details: json!({ "some_other": "stuff" }) }
        );

        Ok(())
    }
}
