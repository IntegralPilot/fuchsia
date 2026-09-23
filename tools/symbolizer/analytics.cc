// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "tools/symbolizer/analytics.h"

namespace symbolizer {

void SymbolizationAnalyticsBuilder::SetAtLeastOneInvalidInput() {
  valid_ = true;
  at_least_one_invalid_input_ = true;
}

void SymbolizationAnalyticsBuilder::SetRemoteSymbolLookupEnabledBit(bool bit) {
  valid_ = true;
  remote_symbol_lookup_enabled_ = bit;
}

void SymbolizationAnalyticsBuilder::SetNumberOfModules(uint64_t count) {
  valid_ = true;
  number_of_modules_ = count;
}

void SymbolizationAnalyticsBuilder::SetNumberOfModulesWithLocalSymbols(uint64_t count) {
  valid_ = true;
  number_of_modules_with_local_symbols_ = count;
}

void SymbolizationAnalyticsBuilder::SetNumberOfModulesWithCachedSymbols(uint64_t count) {
  valid_ = true;
  number_of_modules_with_cached_symbols_ = count;
}

void SymbolizationAnalyticsBuilder::SetNumberOfModulesWithDownloadedSymbols(uint64_t count) {
  valid_ = true;
  number_of_modules_with_downloaded_symbols_ = count;
}

void SymbolizationAnalyticsBuilder::SetNumberOfModulesWithDownloadingFailure(uint64_t count) {
  valid_ = true;
  number_of_modules_with_downloading_failure_ = count;
}

void SymbolizationAnalyticsBuilder::IncreaseNumberOfFrames() {
  valid_ = true;
  number_of_frames_++;
}

void SymbolizationAnalyticsBuilder::IncreaseNumberOfFramesSymbolized() {
  valid_ = true;
  number_of_frames_symbolized_++;
}

void SymbolizationAnalyticsBuilder::IncreaseNumberOfFramesInvalid() {
  valid_ = true;
  number_of_frames_invalid_++;
}

void SymbolizationAnalyticsBuilder::TotalTimerStart() {
  total_timer_start_ = std::chrono::steady_clock::now();
}

void SymbolizationAnalyticsBuilder::TotalTimerStop() {
  valid_ = true;
  total_time = std::chrono::steady_clock::now() - total_timer_start_;
}

void SymbolizationAnalyticsBuilder::DownloadTimerStart() {
  download_timer_start_ = std::chrono::steady_clock::now();
}

void SymbolizationAnalyticsBuilder::DownloadTimerStop() {
  valid_ = true;
  download_time = std::chrono::steady_clock::now() - download_timer_start_;
}

std::unique_ptr<SymbolizationEvent> SymbolizationAnalyticsBuilder::BuildGa4Event() const {
  auto total_time_ms = std::chrono::duration_cast<std::chrono::milliseconds>(total_time).count();
  auto download_time_ms =
      std::chrono::duration_cast<std::chrono::milliseconds>(download_time).count();
  auto event_ptr = std::make_unique<SymbolizationEvent>();
  event_ptr->SetParameter("has_invalid_input", at_least_one_invalid_input_);
  event_ptr->SetParameter("num_modules", static_cast<int64_t>(number_of_modules_));
  event_ptr->SetParameter("num_modules_local",
                          static_cast<int64_t>(number_of_modules_with_local_symbols_));
  event_ptr->SetParameter("num_modules_cached",
                          static_cast<int64_t>(number_of_modules_with_cached_symbols_));
  event_ptr->SetParameter("num_modules_downloaded",
                          static_cast<int64_t>(number_of_modules_with_downloaded_symbols_));
  event_ptr->SetParameter("num_modules_download_fail",
                          static_cast<int64_t>(number_of_modules_with_downloading_failure_));
  event_ptr->SetParameter("num_frames", static_cast<int64_t>(number_of_frames_));
  event_ptr->SetParameter("num_frames_symbolized",
                          static_cast<int64_t>(number_of_frames_symbolized_));
  event_ptr->SetParameter("num_frames_invalid", static_cast<int64_t>(number_of_frames_invalid_));
  event_ptr->SetParameter("remote_symbol_enabled", remote_symbol_lookup_enabled_);
  event_ptr->SetParameter("time_download_ms", static_cast<int64_t>(download_time_ms));
  event_ptr->SetParameter("time_total_ms", static_cast<int64_t>(total_time_ms));

  return event_ptr;
}

void SymbolizationAnalyticsBuilder::SendAnalytics() const {
  Analytics::IfEnabledSendGa4Event(BuildGa4Event());
}

}  // namespace symbolizer
