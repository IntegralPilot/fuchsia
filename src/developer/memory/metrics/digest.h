// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_MEMORY_METRICS_DIGEST_H_
#define SRC_DEVELOPER_MEMORY_METRICS_DIGEST_H_

#include <zircon/types.h>

#include <string>
#include <unordered_map>
#include <unordered_set>
#include <utility>
#include <vector>

#include "src/developer/memory/metrics/bucket_match.h"
#include "src/developer/memory/metrics/capture.h"

namespace memory {

class Bucket {
 public:
  Bucket(std::string name, uint64_t size)
      : name_(std::move(name)), size_(FractionalBytes{.integral = size}) {}
  const std::string& name() const { return name_; }
  uint64_t size() const { return size_.integral; }

 private:
  std::string name_;
  FractionalBytes size_;

  friend class Digester;
};

class Digester;
class Digest {
 public:
  Digest() = default;
  Digest(const Capture& capture, Digester* digester);
  zx_instant_boot_t time() const { return time_; }
  // Access aggregated memory usage, grouped by buckets. Those get populated by
  // calling |Digester::Digest| with a configured |Digester| instance. Buckets
  // are always populated in the same order when produced by the same |Digester|
  // configuration.
  const std::vector<Bucket>& buckets() const { return buckets_; }
  const std::unordered_set<zx_koid_t>& undigested_vmos() const { return undigested_vmos_; }

 private:
  zx_instant_boot_t time_;
  std::vector<Bucket> buckets_;
  std::unordered_set<zx_koid_t> undigested_vmos_;

  friend class Digester;
};

class Digester {
 public:
  explicit Digester(const std::vector<BucketMatch>& bucket_matches);
  void Digest(const Capture& capture, Digest* digest);

 private:
  std::vector<BucketMatch> bucket_matches_;
  std::unordered_map<zx_koid_t, std::string> process_names_;

  friend class Digest;
};

}  // namespace memory

#endif  // SRC_DEVELOPER_MEMORY_METRICS_DIGEST_H_
