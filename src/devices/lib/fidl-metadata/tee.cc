// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#include "src/devices/lib/fidl-metadata/tee.h"

#include <fidl/fuchsia.hardware.tee/cpp/fidl.h>

namespace fidl_metadata::tee {

zx::result<std::vector<uint8_t>> TeeMetadataToFidl(
    uint32_t default_thread_count, cpp20::span<const CustomThreadConfig> thread_config) {
  std::vector<fuchsia_hardware_tee::CustomThreadConfig> thr_config;
  thr_config.reserve(thread_config.size());

  for (const auto& src_thr : thread_config) {
    std::vector<fuchsia_tee::Uuid> apps;
    apps.reserve(src_thr.trusted_apps.size());
    for (const auto& src_app : src_thr.trusted_apps) {
      std::array<uint8_t, 8> clock_seq_and_node;
      for (size_t i = 0; i < clock_seq_and_node.size(); ++i) {
        clock_seq_and_node[i] = src_app.clock_seq_and_node[i];
      }
      apps.emplace_back(src_app.time_low, src_app.time_mid, src_app.time_hi_and_version,
                        clock_seq_and_node);
    }

    thr_config.emplace_back(fuchsia_hardware_tee::CustomThreadConfig{{
        .role{src_thr.role},
        .count = src_thr.count,
        .trusted_apps = std::move(apps),
    }});
  }

  fuchsia_hardware_tee::TeeMetadata metadata{{
      .default_thread_count = default_thread_count,
      .custom_threads = thr_config,
  }};

  return zx::result<std::vector<uint8_t>>{
      fidl::Persist(metadata).map_error(std::mem_fn(&fidl::Error::status))};
}

}  // namespace fidl_metadata::tee
