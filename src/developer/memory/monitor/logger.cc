// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/memory/monitor/logger.h"

#include <lib/syslog/cpp/macros.h>

#include "lib/zx/time.h"
#include "src/developer/memory/metrics/printer.h"

namespace monitor {

namespace {

constexpr size_t kMeasurementsCapacity = 100;  // Max number of digest entries to store in inspect
constexpr std::string_view kMeasurementsEntry{"bucket_sizes"};
constexpr std::string_view kTimestamp{"@timestamp"};
constexpr std::string_view kMeasurementsKey{"measurements"};
constexpr std::string_view kBucketsKey{"buckets"};

void PushDigestToInspect(inspect::BoundedListNode& node, const memory::Digest& digest) {
  node.CreateEntry([&digest](inspect::Node& n) {
    n.RecordUint(kTimestamp, digest.time());
    auto& buckets = digest.buckets();
    auto ia = n.CreateUintArray(kMeasurementsEntry, buckets.size());
    for (std::size_t i = 0; i < buckets.size(); i++) {
      ia.Set(i, buckets[i].size());
    }
    n.Record(std::move(ia));
  });
}

}  // namespace

Logger::Logger(async_dispatcher_t* dispatcher, std::optional<monitor::HighWater*> high_water,
               CaptureCb capture_cb, DigestCb digest_cb, memory_monitor_config::Config* config,
               inspect::Node node)
    : dispatcher_(dispatcher),
      high_water_(high_water),
      capture_cb_(std::move(capture_cb)),
      digest_cb_(std::move(digest_cb)),
      config_(config),
      root_node_(std::move(node)),
      inspect_bucket_digest_node_(root_node_.CreateChild(kMeasurementsKey), kMeasurementsCapacity) {
  // Ensure that all delays are above 0, otherwise the logger risks busy
  // looping, which is all the more problematic because it typically runs on
  // memory_monitor's main thread, blocking it entirely.
  FX_CHECK(config->imminent_oom_capture_delay_s() > 0);
  FX_CHECK(config->critical_capture_delay_s() > 0);
  FX_CHECK(config->warning_capture_delay_s() > 0);
  FX_CHECK(config->normal_capture_delay_s() > 0);
}

void Logger::SetPressureLevel(pressure_signaler::Level l) {
  switch (l) {
    case pressure_signaler::kImminentOOM:
      duration_ = zx::sec(config_->imminent_oom_capture_delay_s());
      capture_high_water_ = true;
      break;
    case pressure_signaler::kCritical:
      duration_ = zx::sec(config_->critical_capture_delay_s());
      capture_high_water_ = false;
      break;
    case pressure_signaler::kWarning:
      duration_ = zx::sec(config_->warning_capture_delay_s());
      capture_high_water_ = false;
      break;
    case pressure_signaler::kNormal:
      duration_ = zx::sec(config_->normal_capture_delay_s());
      capture_high_water_ = false;
      break;
    case pressure_signaler::kNumLevels:
      break;
  }
  if (config_->capture_on_pressure_change()) {
    task_.Cancel();
    task_.Post(dispatcher_);
  } else if (task_.last_deadline() > zx::deadline_after(duration_)) {
    task_.Cancel();
    task_.PostDelayed(dispatcher_, duration_);
  }
}

void Logger::Log() {
  memory::Capture c;
  auto s = capture_cb_(&c);
  if (s != ZX_OK) {
    FX_LOGS_FIRST_N(INFO, 1) << "Error getting Capture: " << s;
    return;
  }
  memory::Digest d;
  digest_cb_(c, &d);
  // TODO(https://fxbug.dev/391360542): Remove the text output.
  {
    std::ostringstream oss;
    memory::TextPrinter p(oss);
    p.PrintDigest(d);
    auto str = std::move(oss).str();
    std::ranges::replace(str, '\n', ' ');
    FX_LOGS(INFO) << str;
  }

  // Enshrine the order of buckets, once they have been populated.
  std::call_once(bucket_names_flag_, [this, &buckets = d.buckets()] {
    bucket_names_ = root_node_.CreateStringArray(kBucketsKey, buckets.size());
    for (size_t i = 0; i < buckets.size(); i++) {
      bucket_names_->Set(i, buckets[i].name());
    }
  });
  PushDigestToInspect(inspect_bucket_digest_node_, d);

  if (capture_high_water_ && high_water_) {
    (*high_water_)->RecordHighWater(c);
    (*high_water_)->RecordHighWaterDigest(d);
  }

  task_.PostDelayed(dispatcher_, duration_);
}

}  // namespace monitor
