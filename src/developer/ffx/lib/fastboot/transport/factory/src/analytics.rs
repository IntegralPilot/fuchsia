// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use ffx_diagnostics_analytics::CustomEvent;
use ffx_fastboot_interface::interface_factory::InterfaceFactoryError;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

pub enum PointOfFailure<'a> {
    // These all might be a little too close to the underlying impl.
    /// Error encountered running `InterfaceFactoryBase::<_>::open`.
    FactoryOpenError(String, &'a InterfaceFactoryError),

    /// Error encountered running `InterfaceFactoryBase::<_>::rediscover`.
    FactoryRediscoveryError(String, &'a InterfaceFactoryError),
}

fn format_interface_factory_error_type(error: &InterfaceFactoryError) -> String {
    match error {
        InterfaceFactoryError::Usb(..) => "usb_discovery_error",
        InterfaceFactoryError::InterfaceOpenError(..) => "interface_open_error",
        InterfaceFactoryError::RediscoverTargetError(..) => "rediscover_target_error",
        InterfaceFactoryError::RediscoverTargetNotInFastboot(..) => {
            "rediscover_target_not_in_fastboot"
        }
        InterfaceFactoryError::RediscoverTargetNotInCorrectTransport(..) => {
            "rediscover_target_not_in_correct_transport"
        }
        InterfaceFactoryError::ConnectionError(..) => "connection_error",
    }
    .to_owned()
}

fn redacted_clone(err: &InterfaceFactoryError) -> Option<InterfaceFactoryError> {
    const REDACTED_DEVICE: &'static str = "<redacted>";
    const REDACTED_ADDRESS: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), 0);
    match err {
        InterfaceFactoryError::Usb(_) => None,
        InterfaceFactoryError::InterfaceOpenError(_) => None,
        InterfaceFactoryError::RediscoverTargetError(s) => {
            Some(InterfaceFactoryError::RediscoverTargetError(s.clone()))
        }
        InterfaceFactoryError::RediscoverTargetNotInFastboot(_, s2) => {
            Some(InterfaceFactoryError::RediscoverTargetNotInFastboot(
                REDACTED_DEVICE.to_owned(),
                s2.clone(),
            ))
        }
        InterfaceFactoryError::RediscoverTargetNotInCorrectTransport(s1, _, s3) => {
            Some(InterfaceFactoryError::RediscoverTargetNotInCorrectTransport(
                s1.clone(),
                REDACTED_DEVICE.to_owned(),
                s3.clone(),
            ))
        }
        InterfaceFactoryError::ConnectionError(p1, _, p3) => {
            Some(InterfaceFactoryError::ConnectionError(
                p1.clone(),
                REDACTED_ADDRESS.clone(),
                p3.clone(),
            ))
        }
    }
}

impl Into<CustomEvent> for PointOfFailure<'_> {
    fn into(self) -> CustomEvent {
        let (category, ty, err) = match self {
            Self::FactoryOpenError(ty, err) => ("open_fastboot_interface", ty, err),
            Self::FactoryRediscoveryError(ty, err) => ("fastboot_rediscovery", ty, err),
        };
        CustomEvent {
            category,
            custom_dimensions: [
                ("error_type", format_interface_factory_error_type(err).into()),
                ("connection_type", ty.into()),
                (
                    "error",
                    // Attempt to redact for errors including addresses for easier categorizing.
                    match redacted_clone(err) {
                        None => err.to_string().into(),
                        Some(i) => i.to_string().into(),
                    },
                ),
            ]
            .into_iter()
            .collect(),
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use analytics::GA4Value;
    use anyhow::anyhow;

    fn get_str_value(v: &GA4Value) -> Option<&str> {
        match v {
            GA4Value::Str(s) => Some(s.as_str()),
            _ => None,
        }
    }

    #[test]
    fn test_factory_open_error_interface_open_error() {
        let err = InterfaceFactoryError::InterfaceOpenError(anyhow!("inner error"));
        let pof = PointOfFailure::FactoryOpenError("udp".to_string(), &err);
        let event: CustomEvent = pof.into();

        assert_eq!(event.category, "open_fastboot_interface");
        let dims = event.custom_dimensions;
        assert_eq!(dims.get("error_type").and_then(get_str_value), Some("interface_open_error"));
        assert_eq!(dims.get("error").and_then(get_str_value), Some("inner error"));
        assert_eq!(dims.get("connection_type").and_then(get_str_value), Some("udp"));
    }

    #[test]
    fn test_factory_rediscovery_error_redacted() {
        let err = InterfaceFactoryError::RediscoverTargetNotInFastboot(
            "my-device".to_string(),
            "zedboot".to_string(),
        );
        let pof = PointOfFailure::FactoryRediscoveryError("usb".to_string(), &err);
        let event: CustomEvent = pof.into();

        assert_eq!(event.category, "fastboot_rediscovery");
        let dims = event.custom_dimensions;
        assert_eq!(
            dims.get("error_type").and_then(get_str_value),
            Some("rediscover_target_not_in_fastboot")
        );
        // Check redaction
        let error_msg = dims.get("error").and_then(get_str_value).unwrap();
        assert!(error_msg.contains("<redacted>"));
        assert!(error_msg.contains("zedboot"));
        assert!(!error_msg.contains("my-device"));
    }

    #[test]
    fn test_connection_error_redacted() {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)), 8080);
        let err = InterfaceFactoryError::ConnectionError("tcp".to_string(), addr, 5);
        let pof = PointOfFailure::FactoryOpenError("tcp".to_string(), &err);
        let event: CustomEvent = pof.into();

        let dims = event.custom_dimensions;
        assert_eq!(dims.get("error_type").and_then(get_str_value), Some("connection_error"));
        let error_msg = dims.get("error").and_then(get_str_value).unwrap();
        assert!(error_msg.contains("0.0.0.0:0")); // Redacted address
        assert!(!error_msg.contains("192.168.1.1"));
    }
}
