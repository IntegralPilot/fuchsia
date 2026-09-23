// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::errors::MetadataError;
use fuchsia_inspect as finspect;
use std::time::Duration;

/// Updates inspect state based on the result of the verifications. The inspect hierarchy is
/// constructed in a way that's compatible with Lapis.
pub(super) fn write_to_inspect(
    node: &finspect::Node,
    res: Result<(), &MetadataError>,
    total_duration: Duration,
) {
    // We need to convert duration to a u64 because that's the largest size integer that inspect
    // can accept. This is safe because duration will be max 1 hour, which fits into u64.
    // For context, 2^64 microseconds is over 5000 centuries.
    let total_duration_u64 = total_duration.as_micros().try_into().unwrap_or(u64::MAX);

    match res {
        Ok(()) => node.record_child("ota_verification_duration", |duration_node| {
            duration_node.record_uint("success", total_duration_u64)
        }),
        Err(MetadataError::Timeout) => {
            node.record_child("ota_verification_duration", |duration_node| {
                duration_node.record_uint("failure_timeout", total_duration_u64);
            });
            node.record_child("ota_verification_failure", |reason_node| {
                reason_node.record_uint("timeout", 1);
            });
        }
        Err(MetadataError::HealthVerification(_)) => {
            node.record_child("ota_verification_duration", |duration_node| {
                duration_node.record_uint("failure_health_check", total_duration_u64);
            });
            node.record_child("ota_verification_failure", |reason_node| {
                reason_node.record_uint("verify", 1);
            });
        }
        Err(_) => {
            node.record_child("ota_verification_duration", |duration_node| {
                duration_node.record_uint("failure_internal", total_duration_u64);
            });
            node.record_child("ota_verification_failure", |reason_node| {
                reason_node.record_uint("internal", 1);
            });
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use diagnostics_assertions::assert_data_tree;
    use fuchsia_inspect::Inspector;

    #[fuchsia::test]
    async fn success() {
        let inspector = Inspector::default();

        let () = write_to_inspect(inspector.root(), Ok(()), Duration::from_micros(2));

        assert_data_tree! {
            inspector,
            root: {
                "ota_verification_duration": {
                    "success" : 2u64,
                }
            }
        }
    }

    #[fuchsia::test]
    async fn failure_verify() {
        let inspector = Inspector::default();

        let () = write_to_inspect(
            inspector.root(),
            Err(&MetadataError::HealthVerification(
                crate::metadata::errors::HealthVerificationError::Unhealthy(zx::Status::INTERNAL),
            )),
            Duration::from_micros(2),
        );

        assert_data_tree! {
            inspector,
            root: {
                "ota_verification_duration": {
                    "failure_health_check" : 2u64,
                },
                "ota_verification_failure": {
                    "verify": 1u64,
                }
            }
        }
    }

    #[fuchsia::test]
    async fn failure_timeout() {
        let inspector = Inspector::default();

        let () = write_to_inspect(
            inspector.root(),
            Err(&MetadataError::Timeout),
            Duration::from_micros(2),
        );

        assert_data_tree! {
            inspector,
            root: {
                "ota_verification_duration": {
                    "failure_timeout" : 2u64,
                },
                "ota_verification_failure": {
                    "timeout": 1u64,
                }
            }
        }
    }

    #[fuchsia::test]
    async fn failure_internal() {
        let inspector = Inspector::default();

        let () = write_to_inspect(
            inspector.root(),
            Err(&MetadataError::Unblock),
            Duration::from_micros(2),
        );

        assert_data_tree! {
            inspector,
            root: {
                "ota_verification_duration": {
                    "failure_internal" : 2u64,
                },
                "ota_verification_failure": {
                    "internal": 1u64,
                }
            }
        }
    }

    /// Verify the reported duration is u64::MAX (millis), even if the actual duration is longer.
    #[fuchsia::test]
    async fn success_duration_max_u64() {
        let inspector = Inspector::default();

        let () = write_to_inspect(inspector.root(), Ok(()), Duration::new(u64::MAX, 0));

        assert_data_tree! {
            inspector,
            root: {
                "ota_verification_duration": {
                    "success" : u64::MAX,
                }
            }
        }
    }
}
