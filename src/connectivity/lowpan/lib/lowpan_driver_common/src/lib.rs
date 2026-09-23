// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![warn(missing_debug_implementations)]
#![warn(rust_2018_idioms)]
// TODO(https://fxbug.dev/42076923): remove after the lint is fixed
#![allow(unknown_lints, clippy::items_after_test_module)]

mod async_condition;
mod dummy_device;
mod lowpan_device;
mod register;
mod serve_to;

pub mod net;
pub mod spinel;

#[cfg(test)]
mod tests;

pub use async_condition::*;
pub use dummy_device::DummyDevice;
pub use lowpan_device::Driver;
pub use register::*;
pub use serve_to::*;

// NOTE: This line is a hack to work around some issues
//       with respect to external rust crates.
use spinel_pack::{self as spinel_pack};

#[macro_export]
macro_rules! traceln (($($args:tt)*) => { log::trace!($($args)*); }; );

#[macro_use]
pub(crate) mod prelude_internal {
    pub use traceln;

    pub use fidl::prelude::*;
    pub use futures::prelude::*;
    pub use spinel_pack::prelude::*;

    #[allow(unused_imports)]
    pub use log::{debug, error, info, trace, warn};

    pub use crate::{ServeTo as _, ZxResult, ZxStatus};
    pub use anyhow::{format_err, Context as _};
    pub use async_trait::async_trait;
}

pub mod lowpan_fidl {
    pub use fidl_fuchsia_factory_lowpan::*;
    pub use fidl_fuchsia_lowpan::*;
    pub use fidl_fuchsia_lowpan_device::*;
    pub use fidl_fuchsia_lowpan_experimental::{
        BeaconInfo, BeaconInfoStreamMarker, BeaconInfoStreamRequest, ChannelInfo,
        DeviceConnectorMarker as ExperimentalDeviceConnectorMarker,
        DeviceConnectorRequest as ExperimentalDeviceConnectorRequest,
        DeviceConnectorRequestStream as ExperimentalDeviceConnectorRequestStream,
        DeviceExtraConnectorMarker as ExperimentalDeviceExtraConnectorMarker,
        DeviceExtraConnectorRequest as ExperimentalDeviceExtraConnectorRequest,
        DeviceExtraConnectorRequestStream as ExperimentalDeviceExtraConnectorRequestStream,
        DeviceExtraMarker as ExperimentalDeviceExtraMarker,
        DeviceExtraRequest as ExperimentalDeviceExtraRequest,
        DeviceExtraRequestStream as ExperimentalDeviceExtraRequestStream,
        DeviceMarker as ExperimentalDeviceMarker, DeviceRequest as ExperimentalDeviceRequest,
        DeviceRequestStream as ExperimentalDeviceRequestStream, DeviceRouteConnectorMarker,
        DeviceRouteConnectorRequest, DeviceRouteConnectorRequestStream,
        DeviceRouteExtraConnectorMarker, DeviceRouteExtraConnectorRequest,
        DeviceRouteExtraConnectorRequestStream, DeviceRouteExtraMarker, DeviceRouteExtraRequest,
        DeviceRouteExtraRequestStream, DeviceRouteMarker, DeviceRouteRequest,
        DeviceRouteRequestStream, ExternalRoute, JoinParams, JoinerCommissioningParams,
        LegacyJoiningConnectorMarker, LegacyJoiningConnectorRequest,
        LegacyJoiningConnectorRequestStream, LegacyJoiningMarker, LegacyJoiningRequest,
        LegacyJoiningRequestStream, NetworkScanParameters, OnMeshPrefix, ProvisionError,
        ProvisioningMonitorRequest, ProvisioningProgress, RoutePreference, SrpServerAddressMode,
        SrpServerInfo, SrpServerRegistration, SrpServerState, Telemetry,
        TelemetryProviderConnectorMarker, TelemetryProviderConnectorRequest,
        TelemetryProviderConnectorRequestStream, TelemetryProviderMarker, TelemetryProviderRequest,
        TelemetryProviderRequestStream,
    };
    pub use fidl_fuchsia_lowpan_test::*;
    pub use fidl_fuchsia_lowpan_thread::*;
    pub use fidl_fuchsia_net::Ipv6AddressWithPrefix as Ipv6Subnet;
}

pub use zx_status::Status as ZxStatus;

/// A `Result` that uses `zx::Status` for the error condition.
pub type ZxResult<T = ()> = Result<T, ZxStatus>;

const MAX_CONCURRENT: usize = 100;

pub mod pii {
    use core::fmt::{Debug, Display, Formatter, Result};

    fn should_display_pii() -> bool {
        true
    }

    fn should_markup_pii() -> bool {
        true
    }

    fn should_highlight_pii() -> bool {
        true
    }

    pub struct MarkPii<'a, T>(pub &'a T);

    impl<'a, T: Debug> Debug for MarkPii<'a, T> {
        fn fmt(&self, f: &mut Formatter<'_>) -> Result {
            if should_display_pii() {
                if should_markup_pii() {
                    write!(f, "[PII](")?;
                }
                if should_highlight_pii() {
                    write!(f, "\x1b[7m")?;
                }
                let ret = self.0.fmt(f);
                if should_highlight_pii() {
                    write!(f, "\x1b[0m")?;
                }
                if should_markup_pii() {
                    write!(f, ")")?;
                }
                ret
            } else {
                write!(f, "[PII-REDACTED]")
            }
        }
    }

    impl<'a, T: Display> Display for MarkPii<'a, T> {
        fn fmt(&self, f: &mut Formatter<'_>) -> Result {
            if should_display_pii() {
                if should_markup_pii() {
                    write!(f, "[PII](")?;
                }
                if should_highlight_pii() {
                    write!(f, "\x1b[7m")?;
                }
                let ret = self.0.fmt(f);
                if should_highlight_pii() {
                    write!(f, "\x1b[0m")?;
                }
                if should_markup_pii() {
                    write!(f, ")")?;
                }
                ret
            } else {
                write!(f, "[PII-REDACTED]")
            }
        }
    }
}
