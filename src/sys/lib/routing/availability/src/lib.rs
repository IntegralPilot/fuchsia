// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use cm_types::Availability;
use moniker::ExtendedMoniker;
use router_error::Explain;
use thiserror::Error;
use zx_status as zx;

/// Ensure that availability cannot decrease from target to source.
pub fn advance(
    moniker: &ExtendedMoniker,
    current: Availability,
    next: Availability,
) -> Result<Availability, TargetHasStrongerAvailability> {
    match (current, next) {
        // `self` will be `SameAsTarget` when routing starts from an `Offer` or `Expose`. This
        // is to verify as much as possible the correctness of routes involving `Offer` and
        // `Expose` without full knowledge of the `use -> offer -> expose` chain.
        //
        // For the purpose of availability checking, we will skip any checks until we encounter
        // a route declaration that has a known availability.
        (Availability::SameAsTarget, _) => Ok(next),

        // If our availability doesn't change, there's nothing to do.
        (Availability::Required, Availability::Required)
        | (Availability::Optional, Availability::Optional)
        | (Availability::Transitional, Availability::Transitional)

        // If the next availability is explicitly a pass-through, there's nothing to do.
        | (Availability::Required, Availability::SameAsTarget)
        | (Availability::Optional, Availability::SameAsTarget)
        | (Availability::Transitional, Availability::SameAsTarget) => Ok(current),

        // Increasing the strength of availability as we travel toward the source is allowed.
        (Availability::Optional, Availability::Required)
        | (Availability::Transitional, Availability::Required)
        | (Availability::Transitional, Availability::Optional) =>
            Ok(next),

        // Decreasing the strength of availability is not allowed, as that could lead to
        // unsanctioned broken routes.
        (Availability::Optional, Availability::Transitional)
        | (Availability::Required, Availability::Transitional)
        | (Availability::Required, Availability::Optional) =>
            Err(TargetHasStrongerAvailability { moniker: moniker.clone() }),
    }
}

/// Availability requested by the target has stronger guarantees than what
/// is being provided at the source.
#[derive(Debug, Error, Clone, PartialEq)]
#[error(
    "Availability requested by the target has stronger guarantees than what \
    is being provided at the source."
)]
pub struct TargetHasStrongerAvailability {
    pub moniker: ExtendedMoniker,
}

impl Explain for TargetHasStrongerAvailability {
    fn as_zx_status(&self) -> zx::Status {
        zx::Status::NOT_FOUND
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_case::test_case;

    #[test_case(Availability::Optional, Availability::Optional, Ok(Availability::Optional))]
    #[test_case(Availability::Optional, Availability::Required, Ok(Availability::Required))]
    #[test_case(Availability::Optional, Availability::SameAsTarget, Ok(Availability::Optional))]
    #[test_case(
        Availability::Optional,
        Availability::Transitional,
        Err(TargetHasStrongerAvailability { moniker: ExtendedMoniker::ComponentManager })
    )]
    #[test_case(Availability::Required, Availability::Optional, Err(TargetHasStrongerAvailability { moniker: ExtendedMoniker::ComponentManager }))]
    #[test_case(Availability::Required, Availability::Required, Ok(Availability::Required))]
    #[test_case(Availability::Required, Availability::SameAsTarget, Ok(Availability::Required))]
    #[test_case(
        Availability::Required,
        Availability::Transitional,
        Err(TargetHasStrongerAvailability { moniker: ExtendedMoniker::ComponentManager })
    )]
    #[test_case(Availability::Transitional, Availability::Optional, Ok(Availability::Optional))]
    #[test_case(Availability::Transitional, Availability::Required, Ok(Availability::Required))]
    #[test_case(
        Availability::Transitional,
        Availability::SameAsTarget,
        Ok(Availability::Transitional)
    )]
    #[test_case(
        Availability::Transitional,
        Availability::Transitional,
        Ok(Availability::Transitional)
    )]
    fn advance_tests(
        current: Availability,
        next: Availability,
        expected: Result<Availability, TargetHasStrongerAvailability>,
    ) {
        let actual = advance(&ExtendedMoniker::ComponentManager, current, next);
        assert_eq!(actual, expected);
    }
}
