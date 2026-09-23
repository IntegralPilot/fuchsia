// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::error::Error;
use std::fmt;
use std::str::FromStr;

/// Represents the set of features a CML file is compiled with. This struct can be
/// used to check whether a feature used in CML is enabled.
#[derive(Debug)]
pub struct FeatureSet(Vec<Feature>);

impl FeatureSet {
    /// Create an empty FeatureSet.
    pub const fn empty() -> FeatureSet {
        FeatureSet(Vec::new())
    }

    /// Tests whether `feature` is enabled.
    pub fn has(&self, feature: &Feature) -> bool {
        self.0.iter().find(|f| *f == feature).is_some()
    }

    /// Returns an `Err` if `feature` is not enabled.
    pub fn check(&self, feature: Feature) -> Result<(), Error> {
        if self.has(&feature) { Ok(()) } else { Err(Error::RestrictedFeature(feature.to_string())) }
    }
}

impl From<Vec<Feature>> for FeatureSet {
    fn from(features: Vec<Feature>) -> FeatureSet {
        FeatureSet(features)
    }
}

/// A feature that can be enabled/opt-into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feature {
    /// Allows `dictionary` capabilities to be used.
    /// This is unused, and will be deleted in a followup
    Dictionaries,

    /// Allows `dictionary` capabilities with `extends: program/...` to be defined (a.k.a.
    /// dynamic dictionaries)
    DynamicDictionaries,

    /// Allows `dictionary` use declarations.
    UseDictionaries,

    // Allows dynamic child name lengths to exceed the default limit.
    AllowLongNames,

    // Allow tests to resolve non-hermetic packages. This requires EnableAllowNonHermeticPackagesFeature
    // to be enabled.
    AllowNonHermeticPackages,

    // Enable AllowNonHermeticPackages feature. This helps us to only enable
    // this in-tree.
    EnableAllowNonHermeticPackagesFeature,

    // Restrict test types in facet. This helps us to only restrict this in-tree.
    RestrictTestTypeInFacet,

    // Allows customizing when the framework opens a capability when a consumer
    // component requests to connect to the capability.
    DeliveryType,
}

impl FromStr for Feature {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "dictionaries" => Ok(Feature::Dictionaries),
            "dynamic_dictionaries" => Ok(Feature::DynamicDictionaries),
            "use_dictionaries" => Ok(Feature::UseDictionaries),
            "allow_long_names" => Ok(Feature::AllowLongNames),
            "allow_non_hermetic_packages" => Ok(Feature::AllowNonHermeticPackages),
            "enable_allow_non_hermetic_packages_feature" => {
                Ok(Feature::EnableAllowNonHermeticPackagesFeature)
            }
            "restrict_test_type_in_facets" => Ok(Feature::RestrictTestTypeInFacet),
            "delivery_type" => Ok(Feature::DeliveryType),
            _ => Err(format!("unrecognized feature \"{}\"", s)),
        }
    }
}

impl fmt::Display for Feature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Feature::Dictionaries => "dictionaries",
            Feature::DynamicDictionaries => "dynamic_dictionaries",
            Feature::UseDictionaries => "use_dictionaries",
            Feature::AllowLongNames => "allow_long_names",
            Feature::AllowNonHermeticPackages => "allow_non_hermetic_packages",
            Feature::EnableAllowNonHermeticPackagesFeature => {
                "enable_allow_non_hermetic_packages_feature"
            }
            Feature::RestrictTestTypeInFacet => "restrict_test_type_in_facets",
            Feature::DeliveryType => "delivery_type",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;

    #[test]
    fn feature_is_parsed() {
        assert_eq!(Feature::AllowLongNames, "allow_long_names".parse::<Feature>().unwrap());
        assert_eq!(
            Feature::AllowNonHermeticPackages,
            "allow_non_hermetic_packages".parse::<Feature>().unwrap()
        );
    }

    #[test]
    fn feature_is_printed() {
        assert_eq!("allow_long_names", Feature::AllowLongNames.to_string());
        assert_eq!("allow_non_hermetic_packages", Feature::AllowNonHermeticPackages.to_string());
        assert_eq!(
            "enable_allow_non_hermetic_packages_feature",
            Feature::EnableAllowNonHermeticPackagesFeature.to_string()
        );
        assert_eq!("restrict_test_type_in_facets", Feature::RestrictTestTypeInFacet.to_string());
    }

    #[test]
    fn feature_set_has() {
        let set = FeatureSet::empty();
        assert!(!set.has(&Feature::AllowLongNames));

        let set = FeatureSet::from(vec![Feature::AllowLongNames]);
        assert!(set.has(&Feature::AllowLongNames));
    }

    #[test]
    fn feature_set_check() {
        let set = FeatureSet::empty();
        assert_matches!(
            set.check(Feature::AllowLongNames),
            Err(Error::RestrictedFeature(f)) if f == "allow_long_names"
        );

        let set = FeatureSet::from(vec![Feature::AllowLongNames]);
        assert_matches!(set.check(Feature::AllowLongNames), Ok(()));
    }
}
