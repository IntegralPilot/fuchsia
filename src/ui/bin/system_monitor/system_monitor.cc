// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/bin/system_monitor/system_monitor.h"

#include <fuchsia/diagnostics/cpp/fidl.h>
#include <lib/async/cpp/task.h>
#include <lib/async/default.h>
#include <lib/stdcompat/string_view.h>
#include <lib/sys/cpp/service_directory.h>
#include <lib/syslog/cpp/macros.h>

#include <algorithm>
#include <iostream>
#include <iterator>
#include <string>

#include "rapidjson/schema.h"
#include "src/lib/fsl/vmo/strings.h"
#include "src/lib/json_parser/json_parser.h"

namespace system_monitor {

constexpr char kTarget[] = "platform_metrics";
constexpr zx::duration kPrintFrequency = zx::sec(10);
constexpr char kCpuData[] = "CPU MEAN ";

SystemMonitor::SystemMonitor() {
  params_ = fuchsia::diagnostics::StreamParameters();
  params_.set_stream_mode(fuchsia::diagnostics::StreamMode::SNAPSHOT);
  params_.set_data_type(fuchsia::diagnostics::DataType::INSPECT);
  params_.set_format(fuchsia::diagnostics::Format::JSON);
  params_.set_client_selector_configuration(
      fuchsia::diagnostics::ClientSelectorConfiguration::WithSelectAll(true));
}

void SystemMonitor::ConnectToArchiveAccessor() {
  auto services = sys::ServiceDirectory::CreateFromNamespace();
  services->Connect(accessor_.NewRequest());
}

void SystemMonitor::InitializeRenderer() { renderer_.Initialize(); }

void SystemMonitor::UpdateRecentDiagnostic() {
  accessor_->StreamDiagnostics(std::move(params_), iterator_.NewRequest());
  iterator_->GetNext(&iterator_result_);
}

std::vector<std::string> SystemMonitor::ParseBatch(
    const std::vector<fuchsia::diagnostics::FormattedContent>& batch) {
  std::vector<std::string> recent_diagnostics;
  for (auto& content : batch) {
    if (!content.is_json()) {
      FX_LOGS(WARNING) << "Invalid JSON Inspect content, skipping";
      continue;
    }
    if (std::string json; fsl::StringFromVmo(content.json(), &json)) {
      recent_diagnostics.push_back(std::move(json));
    } else {
      FX_LOGS(WARNING) << "Failed to convert Inspect content to string, skipping";
    }
  }
  return recent_diagnostics;
}

std::string SystemMonitor::GetTargetFromDiagnostics(std::vector<std::string> recent_diagnostics) {
  for (auto& content : recent_diagnostics) {
    if (cpp23::contains(content, kTarget)) {
      return content;
    }
  }
  return "";
}

std::string SystemMonitor::GetCPUData() {
  auto& batch = iterator_result_.response().batch;
  if (batch.empty()) {
    return "";
  }
  return GetTargetFromDiagnostics(ParseBatch(std::move(batch)));
}

void SystemMonitor::PrintRecentDiagnostic() {
  UpdateRecentDiagnostic();
  const std::string& recent_diag = GetCPUData();
  FX_LOGS(INFO) << "Recent Diagnostics: " << recent_diag;
  json_parser::JSONParser stringParser;
  auto jsonDoc = stringParser.ParseFromString(recent_diag, "");
  // Extract the mean values
  const rapidjson::Value& meanValues =
      jsonDoc["payload"]["root"]["platform_metrics"]["cpu"]["mean"];

  // Print the mean values
  double sum = 0.0;
  for (rapidjson::SizeType i = 0; i < meanValues.Size(); ++i) {
    sum += meanValues[i].GetDouble();
  }
  FX_LOGS(INFO) << "CPU mean: " << sum / meanValues.Size();
  std::string cpu_data = kCpuData + std::to_string(sum / meanValues.Size());
  renderer_.RenderFrame(cpu_data);
  async::PostDelayedTask(
      async_get_default_dispatcher(), [&] { PrintRecentDiagnostic(); }, kPrintFrequency);
}
}  // namespace system_monitor
