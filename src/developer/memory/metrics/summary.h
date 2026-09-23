// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_MEMORY_METRICS_SUMMARY_H_
#define SRC_DEVELOPER_MEMORY_METRICS_SUMMARY_H_

#include <zircon/types.h>

#include <string>
#include <unordered_map>
#include <unordered_set>
#include <utility>
#include <vector>

#include <re2/re2.h>

#include "src/developer/memory/metrics/capture.h"

namespace memory {

struct Sizes {
  Sizes() = default;
  explicit Sizes(uint64_t b)
      : private_bytes(FractionalBytes{.integral = b}),
        scaled_bytes(FractionalBytes{.integral = b}),
        total_bytes(FractionalBytes{.integral = b}) {}

  FractionalBytes private_bytes{};
  FractionalBytes scaled_bytes{};
  FractionalBytes total_bytes{};
};

class ProcessSummary {
 public:
  static const zx_koid_t kKernelKoid;
  ProcessSummary(zx_koid_t koid, std::string name) : koid_(koid), name_(std::move(name)) {}
  ProcessSummary(const zx_info_kmem_stats_t& kmem, uint64_t vmo_bytes);

  zx_koid_t koid() const { return koid_; }
  const std::string& name() const { return name_; }
  const Sizes& sizes() const { return sizes_; }
  const std::unordered_map<std::string, Sizes>& name_to_sizes() const { return name_to_sizes_; }
  const Sizes& GetSizes(const std::string& name) const;

 private:
  zx_koid_t koid_;
  std::string name_;
  Sizes sizes_;
  std::unordered_set<zx_koid_t> vmos_;
  std::unordered_map<std::string, Sizes> name_to_sizes_;

  friend class Summary;
};

struct NameMatch {
  const char* regex = nullptr;
  const char* name = nullptr;
};

class Namer {
 public:
  explicit Namer(const std::vector<NameMatch>& name_matches);

  std::string NameForName(const std::string& name);

 private:
  struct RegexMatch {
    std::unique_ptr<re2::RE2> regex;  // unique_ptr because RE2 is not movable
    std::string name;
  };

  std::vector<RegexMatch> regex_matches_;
  std::unordered_map<std::string, std::string> name_to_name_;
};

class Summary {
 public:
  Summary(const Capture& capture, Namer* namer);
  explicit Summary(const Capture& capture,
                   const std::vector<NameMatch>& name_matches = kNameMatches);
  Summary(const Capture& capture, Namer* namer,
          const std::unordered_set<zx_koid_t>& undigested_vmos);
  inline static const std::vector<NameMatch> kNameMatches = {
      // To prevent the [bootfs-libraries] regex from catching ld.so.1-internal-heap,
      // this regex must be before the [bootfs-libraries] regex.
      {.regex = "ld\\.so\\.1-internal-heap|(^stack: msg of.*)", .name = "[process-bootstrap]"},
      {.regex = "blob-[0-9a-f]+", .name = "[blobs]"},
      {.regex = "inactive-blob-[0-9a-f]+", .name = "[inactive blobs]"},
      {.regex = "thrd_t:0x.*|initial-thread|pthread_(t|create):0x.*", .name = "[stacks]"},
      {.regex = "data[0-9]*:.*", .name = "[data]"},
      {.regex = "bss[0-9]*:.*", .name = "[bss]"},
      {.regex = "relro:.*", .name = "[relro]"},
      {.regex = "", .name = "[unnamed]"},
      {.regex = "scudo:.*", .name = "[scudo]"},
      {.regex = ".*\\.so.*", .name = "[bootfs-libraries]"},
      {.regex = "bootfs(:.*)?", .name = "[bootfs]"},
      {.regex = "restricted_state_vmo:[0-9]*", .name = "[restricted_state_vmo]"},
  };

  void SortProcessSummaries();
  zx_instant_boot_t time() const { return time_; }
  const zx_info_kmem_stats_t& kstats() const { return kstats_; }
  const std::vector<ProcessSummary>& process_summaries() const { return process_summaries_; }

 private:
  void Init(const Capture& capture, Namer* namer,
            const std::unordered_set<zx_koid_t>& undigested_vmos);
  zx_instant_boot_t time_;
  zx_info_kmem_stats_t kstats_;
  std::vector<ProcessSummary> process_summaries_;
};

}  // namespace memory

#endif  // SRC_DEVELOPER_MEMORY_METRICS_SUMMARY_H_
