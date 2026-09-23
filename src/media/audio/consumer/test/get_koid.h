// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_CONSUMER_TEST_GET_KOID_H_
#define SRC_MEDIA_AUDIO_CONSUMER_TEST_GET_KOID_H_

#include <lib/zx/vmo.h>

namespace media::audio::tests {

zx_koid_t GetKoid(const zx::vmo& vmo);

}  // namespace media::audio::tests

#endif  // SRC_MEDIA_AUDIO_CONSUMER_TEST_GET_KOID_H_
