// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_media as media;
use fuchsia_bluetooth::types::PeerId;
use futures::StreamExt;
use futures::stream::BoxStream;
use std::collections::{HashMap, HashSet};

use crate::codec_id::CodecId;
use crate::sco;

use super::dai::DaiControl;
use super::inband::InbandControl;
use super::{Control, ControlEvent, Error};

/// A Control that either sends the audio directly to the controller (using an offload
/// Control) or encodes audio locally and sends it in the SCO channel, depending on
/// whether the codec is in the list of offload-supported codecs.
pub struct PartialOffloadControl {
    offload_codecids: HashSet<CodecId>,
    /// Used to control when the audio can be sent offloaded
    offload: Box<dyn Control>,
    /// Used to encode audio locally and send inband
    inband: InbandControl,
    /// The set of started peers. Value is true if the audio encoding is handled by the controller.
    started: HashMap<PeerId, bool>,
}

impl PartialOffloadControl {
    pub async fn setup_audio_core(
        audio_proxy: media::AudioDeviceEnumeratorProxy,
        offload_supported: HashSet<CodecId>,
    ) -> Result<Self, Error> {
        let dai = DaiControl::discover(audio_proxy.clone()).await?;
        let inband = InbandControl::create(audio_proxy)?;
        Ok(Self {
            offload_codecids: offload_supported,
            offload: Box::new(dai),
            inband,
            started: Default::default(),
        })
    }
}

impl Control for PartialOffloadControl {
    fn start(
        &mut self,
        id: PeerId,
        connection: sco::Connection,
        codec: CodecId,
    ) -> Result<(), Error> {
        if self.started.contains_key(&id) {
            return Err(Error::AlreadyStarted);
        }
        let result = if self.offload_codecids.contains(&codec) {
            self.offload.start(id, connection, codec)
        } else {
            self.inband.start(id, connection, codec)
        };
        if result.is_ok() {
            let _ = self.started.insert(id, self.offload_codecids.contains(&codec));
        }
        result
    }

    fn stop(&mut self, id: PeerId) -> Result<(), Error> {
        let stop_result = match self.started.get(&id) {
            None => return Err(Error::NotStarted),
            Some(true) => self.offload.stop(id),
            Some(false) => self.inband.stop(id),
        };
        if stop_result.is_ok() {
            let _ = self.started.remove(&id);
        }
        stop_result
    }

    fn connect(&mut self, id: PeerId, supported_codecs: &[CodecId]) {
        // TODO(b/341114499): Consider not connecting this here, since it could create a device we
        // don't want to use.
        self.inband.connect(id, supported_codecs);
        if supported_codecs.iter().any(|i| self.offload_codecids.contains(i)) {
            self.offload.connect(id, supported_codecs);
        }
    }

    fn disconnect(&mut self, id: PeerId) {
        self.inband.disconnect(id);
        self.offload.disconnect(id);
    }

    fn take_events(&self) -> BoxStream<'static, ControlEvent> {
        let inband_events = self.inband.take_events();
        let controller_events = self.offload.take_events();
        futures::stream::select_all([inband_events, controller_events]).boxed()
    }

    fn failed_request(&self, request: ControlEvent, error: Error) {
        // We only support requests from the controller Control (inband does not make
        // requests).
        self.offload.failed_request(request, error);
    }
}
