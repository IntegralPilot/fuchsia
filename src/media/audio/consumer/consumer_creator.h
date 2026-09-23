// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_CONSUMER_CONSUMER_CREATOR_H_
#define SRC_MEDIA_AUDIO_CONSUMER_CONSUMER_CREATOR_H_

#include <fidl/fuchsia.media/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/sys/cpp/component_context.h>

namespace media::audio {

class ConsumerCreator : public fidl::Server<fuchsia_media::SessionAudioConsumerFactory> {
 public:
  explicit ConsumerCreator(async_dispatcher_t* dispatcher);

  ~ConsumerCreator() override = default;

  // Disallow copy, assign and move.
  ConsumerCreator(const ConsumerCreator&) = delete;
  ConsumerCreator& operator=(const ConsumerCreator&) = delete;
  ConsumerCreator(ConsumerCreator&&) = delete;
  ConsumerCreator& operator=(ConsumerCreator&&) = delete;

  // fuchsia_media::SessionAudioConsumerFactory implementation.
  void CreateAudioConsumer(CreateAudioConsumerRequest& request,
                           CreateAudioConsumerCompleter::Sync& completer) override;

 private:
  async_dispatcher_t* dispatcher_;
};

}  // namespace media::audio

#endif  // SRC_MEDIA_AUDIO_CONSUMER_CONSUMER_CREATOR_H_
