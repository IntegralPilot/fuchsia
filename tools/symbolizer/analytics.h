// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef TOOLS_SYMBOLIZER_ANALYTICS_H_
#define TOOLS_SYMBOLIZER_ANALYTICS_H_

#include <chrono>
#include <memory>

#include "src/developer/debug/ipc/protocol.h"
#include "src/lib/analytics/cpp/core_dev_tools/analytics.h"
#include "src/lib/fxl/strings/substitute.h"

namespace symbolizer {

class SymbolizationEvent : public analytics::core_dev_tools::Ga4Event {
 public:
  SymbolizationEvent() : analytics::core_dev_tools::Ga4Event("symbolize") {}

 private:
  friend class SymbolizationAnalyticsBuilder;
};

class SymbolizationAnalyticsBuilder {
 public:
  SymbolizationAnalyticsBuilder() = default;
  bool valid() const { return valid_; }

  void SetAtLeastOneInvalidInput();
  void SetRemoteSymbolLookupEnabledBit(bool bit);
  void SetNumberOfModules(uint64_t count);
  void SetNumberOfModulesWithLocalSymbols(uint64_t count);
  void SetNumberOfModulesWithCachedSymbols(uint64_t count);
  void SetNumberOfModulesWithDownloadedSymbols(uint64_t count);
  void SetNumberOfModulesWithDownloadingFailure(uint64_t count);
  void IncreaseNumberOfFrames();
  void IncreaseNumberOfFramesSymbolized();
  void IncreaseNumberOfFramesInvalid();

  // Timing related.
  void TotalTimerStart();
  void TotalTimerStop();
  void DownloadTimerStart();
  void DownloadTimerStop();

  // Build the symbolize event
  std::unique_ptr<SymbolizationEvent> BuildGa4Event() const;

  // Send analytics
  void SendAnalytics() const;

 private:
  bool valid_ = false;
  bool remote_symbol_lookup_enabled_ = false;
  bool at_least_one_invalid_input_ = false;
  uint64_t number_of_modules_ = 0;
  uint64_t number_of_modules_with_local_symbols_ = 0;
  uint64_t number_of_modules_with_cached_symbols_ = 0;
  uint64_t number_of_modules_with_downloaded_symbols_ = 0;
  uint64_t number_of_modules_with_downloading_failure_ = 0;
  uint64_t number_of_frames_ = 0;
  uint64_t number_of_frames_symbolized_ = 0;
  uint64_t number_of_frames_invalid_ = 0;

  std::chrono::time_point<std::chrono::steady_clock> total_timer_start_;
  std::chrono::steady_clock::duration total_time{};
  std::chrono::time_point<std::chrono::steady_clock> download_timer_start_;
  std::chrono::steady_clock::duration download_time{};
};

class Analytics : public analytics::core_dev_tools::Analytics<Analytics> {
 private:
  friend class analytics::core_dev_tools::Analytics<Analytics>;

  static constexpr char kToolName[] = "symbolizer";
  static constexpr uint32_t kToolVersion = debug_ipc::kCurrentProtocolVersion;
  static constexpr int64_t kQuitTimeoutMs = 500;
  static constexpr char kMeasurementId[] = "G-B0SP6NVLC6";
  static constexpr char kMeasurementKey[] = "ABmnNSKcQX21P9EcYQDt2Q";
  static constexpr char kEnableArgs[] = "--analytics=enable";
  static constexpr char kDisableArgs[] = "--analytics=disable";
  static constexpr char kStatusArgs[] = "--analytics-show";
};

}  // namespace symbolizer

#endif  // TOOLS_SYMBOLIZER_ANALYTICS_H_
