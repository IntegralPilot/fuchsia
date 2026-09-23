// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::events::ExitStatus;
use fidl_fuchsia_component as fcomponent;

#[derive(Clone, Eq, PartialEq, PartialOrd, Ord, Debug)]
pub struct EventDescriptor {
    pub event_type: Option<fcomponent::EventType>,
    pub capability_name: Option<String>,
    pub target_moniker: Option<String>,
    pub exit_status: Option<ExitStatus>,
    pub event_is_ok: Option<bool>,
}

impl TryFrom<&fcomponent::Event> for EventDescriptor {
    type Error = anyhow::Error;

    fn try_from(event: &fcomponent::Event) -> Result<Self, Self::Error> {
        // Construct the EventDescriptor from the Event
        let event_type = event.header.as_ref().and_then(|header| header.event_type.clone());
        let target_moniker = event.header.as_ref().and_then(|header| header.moniker.clone());
        let capability_name = match &event.payload {
            Some(fcomponent::EventPayload::CapabilityRequested(
                fcomponent::CapabilityRequestedPayload { name, .. },
            )) => name.clone(),
            _ => None,
        };
        let exit_status = match &event.payload {
            Some(fcomponent::EventPayload::Stopped(fcomponent::StoppedPayload {
                status, ..
            })) => status.map(|val| val.into()),
            _ => None,
        };
        let event_is_ok = match &event.payload {
            Some(_) => Some(true),
            _ => None,
        };

        Ok(EventDescriptor {
            event_type,
            target_moniker,
            capability_name,
            exit_status,
            event_is_ok,
        })
    }
}
