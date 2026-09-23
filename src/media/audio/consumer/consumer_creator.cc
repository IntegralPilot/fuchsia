// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/consumer/consumer_creator.h"

#include <fidl/fuchsia.media/cpp/fidl.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/syslog/cpp/macros.h>

#include "src/media/audio/consumer/consumer.h"

namespace media::audio {

ConsumerCreator::ConsumerCreator(async_dispatcher_t* dispatcher) : dispatcher_(dispatcher) {}

void ConsumerCreator::CreateAudioConsumer(CreateAudioConsumerRequest& request,
                                          CreateAudioConsumerCompleter::Sync& completer) {
  zx::result audio_core_client_end = component::Connect<fuchsia_media::AudioCore>();
  if (!audio_core_client_end.is_ok()) {
    FX_LOGS(ERROR) << "Failed to connect to the |AudioCore| protocol: "
                   << audio_core_client_end.status_string();
    completer.Close(audio_core_client_end.status_value());
    return;
  }

  Consumer::CreateAndBind(dispatcher_, std::move(audio_core_client_end.value()),
                          std::move(request.audio_consumer_request()));
}

}  // namespace media::audio
