// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_MEMORY_METRICS_BUCKET_MATCH_H_
#define SRC_DEVELOPER_MEMORY_METRICS_BUCKET_MATCH_H_

#include <zircon/types.h>

#include <functional>
#include <optional>
#include <string>
#include <vector>

#include <re2/re2.h>

#include "src/developer/memory/metrics/capture.h"
#include "src/developer/memory/metrics/mas_cache.h"

namespace memory {

using VmoMatcher = std::function<bool(const Vmo&)>;

extern const std::string kBucketConfigPath;

class BucketMatch {
 public:
  BucketMatch(std::string name, const std::string& process, const std::string& vmo,
              std::optional<int64_t> event_code = std::nullopt);
  const std::string& name() const { return name_; }
  // Returns the Cobalt event code associated with this bucket. This is used for reporting bucket
  // usage through Cobalt.
  std::optional<int64_t> event_code() const { return event_code_; }
  bool ProcessMatch(const Process& process);
  bool VmoMatch(const std::string& vmo);
  // Invalidates the cached process match result for the given KOID.
  void InvalidateProcess(zx_koid_t koid);
  // Removes all processes and VMOs that haven't been matched since the last Sweep call from the
  // internal cache.
  void Sweep();

  // Parses a configuration string (e.g. stored in a file) to create bucket matches. The
  // configuration format is described in the README.md file in this directory. Returns true if the
  // parsing succeded, false otherwise.
  static std::optional<std::vector<BucketMatch>> ReadBucketMatchesFromConfig(
      const std::string& config_string);

 private:
  const std::string name_;
  bool match_all_processes_;
  const std::shared_ptr<re2::RE2> process_;  // shared_ptr because RE2 is not movable or copyable
  bool match_all_vmos_;
  const std::shared_ptr<re2::RE2> vmo_;
  const std::optional<int64_t> event_code_;

  // Cache of the matching results against the |process_| regexp.
  MarkAndSweepCache<zx_koid_t, bool> process_match_;
  // Cache of the matching results against the |vmo_| regexp.
  MarkAndSweepCache<std::string, bool> vmo_match_;
};

}  // namespace memory

#endif  // SRC_DEVELOPER_MEMORY_METRICS_BUCKET_MATCH_H_
