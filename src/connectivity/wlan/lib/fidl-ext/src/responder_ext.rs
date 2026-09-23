// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{SendResultExt, TryUnpack};
use anyhow::format_err;
use fidl_fuchsia_wlan_softmac as fidl_softmac;

/// Defines an abstract ResponderExt trait usually implemented using the impl_responder_ext!() macro.
pub trait ResponderExt {
    type Response<'a>;
    const REQUEST_NAME: &'static str;

    fn send(self, response: Self::Response<'_>) -> Result<(), fidl::Error>;

    /// Returns an success value containing all unpacked fields and the responder, or an error value
    /// if any of the values in `fields` is missing.
    ///
    /// The last argument is a closure which will compute a `Self::Response`.  If any field is
    /// missing a value, then this function will return an error and send the computed
    /// `Self::Response`.
    ///
    /// Example Usage:
    ///
    /// ```
    ///   enum Error {
    ///       UnableToStart,
    ///   }
    ///   let ((status, id), responder) = responder.unpack_fields_or_else_send(
    ///       (payload.status.with_name("status"), payload.id.with_name("id")),
    ///       |e| (e.context(format_err!("Unable to start.")), Error::UnableToStart),
    ///   )?;
    /// ```
    fn unpack_fields_or_else_send<'a, T, F>(
        self,
        fields: T,
        f: F,
    ) -> Result<(T::Unpacked, Self), anyhow::Error>
    where
        T: TryUnpack<Error = anyhow::Error>,
        F: FnOnce() -> Self::Response<'a>,
        Self: Sized,
    {
        match fields.try_unpack() {
            Ok(values) => Ok((values, self)),
            Err(error) => {
                let error = error
                    .context(format_err!("Missing required field(s) in {}.", Self::REQUEST_NAME));
                match self.send(f()).format_send_err() {
                    Ok(_) => Err(error),
                    Err(send_error) => Err(send_error.context(error)),
                }
            }
        }
    }

    fn unpack_fields_or_respond<T>(self, fields: T) -> Result<(T::Unpacked, Self), anyhow::Error>
    where
        T: TryUnpack<Error = anyhow::Error>,
        Self: for<'a> ResponderExt<Response<'a> = ()> + Sized,
    {
        self.unpack_fields_or_else_send(fields, || ())
    }
}

impl ResponderExt for fidl_fuchsia_wlan_device_service::DeviceMonitorCreateIfaceResponder {
    type Response<'a> = Result<
        &'a fidl_fuchsia_wlan_device_service::DeviceMonitorCreateIfaceResponse,
        fidl_fuchsia_wlan_device_service::DeviceMonitorError,
    >;
    const REQUEST_NAME: &'static str =
        stringify!(fidl_fuchsia_wlan_device_service::DeviceMonitorCreateIfaceRequest);

    fn send(self, response: Self::Response<'_>) -> Result<(), fidl::Error> {
        Self::send(self, response)
    }
}

impl ResponderExt for fidl_softmac::WlanSoftmacIfcBridgeNotifyScanCompleteResponder {
    type Response<'a> = ();
    const REQUEST_NAME: &'static str =
        stringify!(fidl_softmac::WlanSoftmacIfcBaseNotifyScanCompleteRequest);

    fn send(self, _: Self::Response<'_>) -> Result<(), fidl::Error> {
        Self::send(self)
    }
}

impl ResponderExt for fidl_softmac::WlanSoftmacIfcBridgeReportTxResultResponder {
    type Response<'a> = ();
    const REQUEST_NAME: &'static str =
        stringify!(fidl_softmac::WlanSoftmacIfcBaseReportTxResultRequest);

    fn send(self, _: Self::Response<'_>) -> Result<(), fidl::Error> {
        Self::send(self)
    }
}
