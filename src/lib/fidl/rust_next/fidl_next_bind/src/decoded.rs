// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_next_codec::{Decoded, FromWire, IntoNatural, Wire};
use fidl_next_protocol::{Message, Transport};

use crate::HasTransport;

use super::Method;

/// A received FIDL message that will be handled by a client or server handler.
pub struct Request<M: Method, T: Transport = <<M as Method>::Protocol as HasTransport>::Transport> {
    decoded: Decoded<M::Request, Message<T>>,
}

impl<M: Method, T: Transport> Request<M, T> {
    /// Creates a new `Request` from a decoded buffer.
    pub fn from_decoded(decoded: Decoded<M::Request, Message<T>>) -> Self {
        Self { decoded }
    }

    /// Returns the payload of the request.
    pub fn payload(self) -> <M::Request as IntoNatural>::Natural
    where
        M::Request:
            Wire + for<'de> IntoNatural<Natural: FromWire<<M::Request as Wire>::Narrowed<'de>>>,
    {
        self.decoded.take()
    }

    /// Returns the payload of the request as a wire type.
    pub fn wire_payload(self) -> Decoded<M::Request, Message<T>> {
        self.decoded
    }
}
