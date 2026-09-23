// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/memory/metrics/summary.h"

#include <lib/trace/event.h>

#include <unordered_set>

namespace memory {

Namer::Namer(const std::vector<NameMatch>& name_matches) {
  regex_matches_.reserve(name_matches.size());
  for (auto& name_match : name_matches) {
    regex_matches_.push_back(
        RegexMatch{.regex = std::make_unique<re2::RE2>(name_match.regex), .name = name_match.name});
  }
}

std::string Namer::NameForName(const std::string& name) {
  const auto& found_name = name_to_name_.find(name);
  if (found_name != name_to_name_.end()) {
    return found_name->second;
  }
  for (const auto& regex_match : regex_matches_) {
    if (re2::RE2::FullMatch(name, *regex_match.regex)) {
      name_to_name_.emplace(name, regex_match.name);
      return regex_match.name;
    }
  }
  name_to_name_.emplace(name, name);
  return name;
}

Summary::Summary(const Capture& capture, const std::vector<NameMatch>& name_matches)
    : time_(capture.time()), kstats_(capture.kmem()) {
  Namer namer(name_matches);
  std::unordered_set<zx_koid_t> empty_vmos;
  Init(capture, &namer, empty_vmos);
}

Summary::Summary(const Capture& capture, Namer* namer)
    : time_(capture.time()), kstats_(capture.kmem()) {
  std::unordered_set<zx_koid_t> empty_vmos;
  Init(capture, namer, empty_vmos);
}

Summary::Summary(const Capture& capture, Namer* namer,
                 const std::unordered_set<zx_koid_t>& undigested_vmos)
    : time_(capture.time()), kstats_(capture.kmem()) {
  Init(capture, namer, undigested_vmos);
}

void Summary::Init(const Capture& capture, Namer* namer,
                   const std::unordered_set<zx_koid_t>& undigested_vmos) {
  TRACE_DURATION("memory_metrics", "Summary::Summary");
  bool check_undigested = undigested_vmos.size() > 0;
  std::unordered_map<zx_koid_t, std::unordered_set<zx_koid_t>> vmo_to_processes(
      capture.koid_to_process().size() + 1);

  for (const auto& [process_koid, process] : capture.koid_to_process()) {
    auto& s = process_summaries_.emplace_back(process_koid, process.name);
    for (auto vmo_koid : process.vmos) {
      if (!check_undigested || undigested_vmos.contains(vmo_koid)) {
        vmo_to_processes[vmo_koid].insert(process_koid);
        s.vmos_.insert(vmo_koid);
      }
    }
    if (s.vmos_.empty()) {
      process_summaries_.pop_back();
    }
  }
  for (auto& s : process_summaries_) {
    for (const auto& v : s.vmos_) {
      const auto& vmo = capture.vmo_for_koid(v);
      const auto committed_bytes = vmo.committed_scaled_bytes;
      const auto share_count = vmo_to_processes.at(v).size();
      auto& name_sizes = s.name_to_sizes_[namer->NameForName(vmo.name)];
      name_sizes.total_bytes += committed_bytes;
      s.sizes_.total_bytes += committed_bytes;
      if (share_count == 1) {
        name_sizes.private_bytes += committed_bytes;
        s.sizes_.private_bytes += committed_bytes;
        name_sizes.scaled_bytes += committed_bytes;
        s.sizes_.scaled_bytes += committed_bytes;
      } else {
        auto scaled_bytes = committed_bytes / share_count;
        name_sizes.scaled_bytes += scaled_bytes;
        s.sizes_.scaled_bytes += scaled_bytes;
      }
    }
  }

  FractionalBytes vmo_bytes{};
  for (const auto& [koid, vmo] : capture.koid_to_vmo()) {
    vmo_bytes += vmo.committed_scaled_bytes;
  }
  process_summaries_.emplace_back(kstats_, vmo_bytes.integral);
}  // namespace memory

const zx_koid_t ProcessSummary::kKernelKoid = 1;

ProcessSummary::ProcessSummary(const zx_info_kmem_stats_t& kmem, uint64_t vmo_bytes)
    : koid_(kKernelKoid), name_("kernel") {
  auto kmem_vmo_bytes = kmem.vmo_bytes < vmo_bytes ? 0 : kmem.vmo_bytes - vmo_bytes;
  name_to_sizes_.emplace("heap", kmem.total_heap_bytes);
  name_to_sizes_.emplace("wired", kmem.wired_bytes);
  name_to_sizes_.emplace("mmu", kmem.mmu_overhead_bytes);
  name_to_sizes_.emplace("ipc", kmem.ipc_bytes);
  name_to_sizes_.emplace("other", kmem.other_bytes);
  name_to_sizes_.emplace("vmo", kmem_vmo_bytes);

  const uint64_t total_bytes = kmem.wired_bytes + kmem.total_heap_bytes + kmem.mmu_overhead_bytes +
                               kmem.ipc_bytes + kmem.other_bytes + kmem_vmo_bytes;
  sizes_.private_bytes = sizes_.scaled_bytes = sizes_.total_bytes =
      FractionalBytes{.integral = total_bytes};
}

const Sizes& ProcessSummary::GetSizes(const std::string& name) const {
  return name_to_sizes_.at(name);
}

}  // namespace memory
