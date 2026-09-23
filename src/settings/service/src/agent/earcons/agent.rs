// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::agent::earcons::bluetooth_handler::BluetoothHandler;
use crate::agent::earcons::volume_change_handler::VolumeChangeHandler;
use fidl_fuchsia_media_sounds::PlayerProxy;
use futures::channel::mpsc::UnboundedSender;
use futures::lock::Mutex;
use settings_audio::Request as AudioRequest;
use settings_common::inspect::event::ExternalEventPublisher;
use settings_common::service_context::{ExternalServiceProxy, ServiceContext};
use std::collections::HashSet;
use std::fmt::Debug;
use std::rc::Rc;

/// The Earcons Agent is responsible for watching updates to relevant sources that need to play
/// sounds.
pub(crate) struct Agent {
    external_publisher: ExternalEventPublisher,
    sound_player_connection:
        Rc<Mutex<Option<ExternalServiceProxy<PlayerProxy, ExternalEventPublisher>>>>,
    audio_request_tx: Option<UnboundedSender<AudioRequest>>,
}

/// Params that are common to handlers of the earcons agent.
#[derive(Clone)]
pub(super) struct CommonEarconsParams {
    pub(super) service_context: Rc<ServiceContext>,
    pub(super) sound_player_added_files: Rc<Mutex<HashSet<&'static str>>>,
    pub(super) sound_player_connection:
        Rc<Mutex<Option<ExternalServiceProxy<PlayerProxy, ExternalEventPublisher>>>>,
}

impl Debug for CommonEarconsParams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CommonEarconsParams")
            .field("sound_player_added_files", &self.sound_player_added_files)
            .field("sound_player_connection", &self.sound_player_connection)
            .finish_non_exhaustive()
    }
}

impl Agent {
    pub(crate) fn new(
        audio_request_tx: Option<UnboundedSender<AudioRequest>>,
        external_publisher: ExternalEventPublisher,
    ) -> Self {
        Self {
            external_publisher,
            sound_player_connection: Rc::new(Mutex::new(None)),
            audio_request_tx,
        }
    }

    pub async fn initialize(self, service_context: Rc<ServiceContext>) {
        let common_earcons_params = CommonEarconsParams {
            service_context,
            sound_player_added_files: Rc::new(Mutex::new(HashSet::new())),
            sound_player_connection: self.sound_player_connection.clone(),
        };

        if let Err(e) = VolumeChangeHandler::spawn(
            self.audio_request_tx.clone(),
            self.external_publisher.clone(),
            common_earcons_params.clone(),
        )
        .await
        {
            // For now, report back as an error to prevent issues on
            // platforms that don't support the handler's dependencies.
            // TODO(https://fxbug.dev/42139617): Handle with config
            log::error!("Could not set up VolumeChangeHandler: {:?}", e);
        }

        if BluetoothHandler::spawn(
            self.audio_request_tx,
            self.external_publisher,
            common_earcons_params,
        )
        .await
        .is_err()
        {
            // For now, report back as an error to prevent issues on
            // platforms that don't support the handler's dependencies.
            // TODO(https://fxbug.dev/42139617): Handle with config
            log::error!("Could not set up BluetoothHandler");
        }
    }
}
