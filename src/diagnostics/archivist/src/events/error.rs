// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::events::types::Event;
use fidl_fuchsia_component as fcomponent;
use futures::channel::mpsc;
use moniker::MonikerError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum EventError {
    #[error(transparent)]
    Fidl(#[from] fidl::Error),

    #[error("incorrect capability name {received} (expected {expected})")]
    IncorrectName { received: String, expected: &'static str },

    #[error("received an invalid event type {0:?}")]
    InvalidEventType(fcomponent::EventType),

    #[error("missing `{0}`")]
    MissingField(&'static str),

    #[error("received an unknown event result {0:?}")]
    UnknownResult(fcomponent::EventPayload),

    #[error("expected a result in the fuchsia.sys2 event, but none was found")]
    ExpectedResult,

    #[error("Component error: {0:?}")]
    ComponentError(fcomponent::Error),

    #[error(transparent)]
    SendError(#[from] mpsc::TrySendError<Event>),

    #[error(transparent)]
    Moniker(#[from] MonikerError),
}
