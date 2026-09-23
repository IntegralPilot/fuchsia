// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_SERVICES_COMMON_VECTOR_OF_WEAK_PTR_H_
#define SRC_MEDIA_AUDIO_SERVICES_COMMON_VECTOR_OF_WEAK_PTR_H_

#include <memory>

namespace media_audio {

// A collection of `std::weak_ptr<T>`. To prevent unbounded growth, pointers are automatically
// erased from this collection once they expire. Not safe for concurrent use.
template <class T>
class VectorOfWeakPtr {
 public:
  // Adds an object to this collection. The caller must maintain a strong reference to the object,
  // or else it will be destructed immediately.
  void push_back(std::shared_ptr<T> p) {
    GarbageCollect();
    objects_.push_back(p);
  }

  // Iterators for the begin/end of this collection. If a call to `begin()` is followed immediately
  // by `end()`, the range `[begin(), end())` is guaranteed to be live.
  typename std::vector<std::weak_ptr<T>>::const_iterator begin() {
    GarbageCollect();
    return objects_.begin();
  }
  typename std::vector<std::weak_ptr<T>>::const_iterator end() {
    GarbageCollect();
    return objects_.end();
  }

  // Returns the number of live objects.
  size_t size() {
    GarbageCollect();
    return static_cast<int64_t>(objects_.size());
  }

 private:
  void GarbageCollect() {
    for (auto it = objects_.begin(); it != objects_.end();) {
      if (it->expired()) {
        it = objects_.erase(it);
      } else {
        ++it;
      }
    }
  }

  std::vector<std::weak_ptr<T>> objects_;
};

}  // namespace media_audio

#endif  // SRC_MEDIA_AUDIO_SERVICES_COMMON_VECTOR_OF_WEAK_PTR_H_
