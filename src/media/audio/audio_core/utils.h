// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_AUDIO_CORE_UTILS_H_
#define SRC_MEDIA_AUDIO_AUDIO_CORE_UTILS_H_

#include <lib/fzl/vmo-mapper.h>
#include <stdint.h>

#include <atomic>

#include <fbl/ref_counted.h>

#include "src/media/audio/audio_core/mixer/constants.h"

namespace media::audio {

class GenerationId {
 public:
  uint32_t get() const { return id_; }
  uint32_t Next() {
    uint32_t ret;
    do {
      ret = ++id_;
    } while (ret == kInvalidGenerationId);
    return ret;
  }

 private:
  uint32_t id_ = kInvalidGenerationId + 1;
};

class AtomicGenerationId {
 public:
  AtomicGenerationId() : id_(kInvalidGenerationId + 1) {}

  uint32_t get() const { return id_.load(); }
  uint32_t Next() {
    uint32_t ret;
    do {
      ret = id_.fetch_add(1);
    } while (ret == kInvalidGenerationId);
    return ret;
  }

 private:
  std::atomic<uint32_t> id_;
};

// A simple extension to the libfzl VmoMapper which mixes in ref counting state
// to allow for shared VmoMapper semantics.
class RefCountedVmoMapper : public fzl::VmoMapper, public fbl::RefCounted<fzl::VmoMapper> {};

}  // namespace media::audio

#endif  // SRC_MEDIA_AUDIO_AUDIO_CORE_UTILS_H_
