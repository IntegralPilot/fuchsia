// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/memory/metrics/bucket_match.h"

#include <lib/syslog/cpp/macros.h>
#include <lib/trace/event.h>

#include <optional>

#include <re2/re2.h>

#include "third_party/rapidjson/include/rapidjson/document.h"

namespace memory {

BucketMatch::BucketMatch(std::string name, const std::string& process, const std::string& vmo,
                         std::optional<int64_t> event_code)
    : name_(std::move(name)),
      match_all_processes_(process.empty() || process == ".*"),
      process_(std::make_shared<re2::RE2>(process)),
      match_all_vmos_(vmo.empty() || vmo == ".*"),
      vmo_(std::make_shared<re2::RE2>(vmo)),
      event_code_(event_code) {}

bool BucketMatch::ProcessMatch(const Process& process) {
  if (match_all_processes_) {
    return true;
  }
  if (auto match = process_match_.Find(process.koid)) {
    return *match;
  }
  bool match = re2::RE2::FullMatch(process.name, *process_);
  process_match_.Emplace(process.koid, match);
  return match;
}

bool BucketMatch::VmoMatch(const std::string& vmo) {
  if (match_all_vmos_) {
    return true;
  }
  if (auto match = vmo_match_.Find(vmo)) {
    return *match;
  }
  bool match = re2::RE2::FullMatch(vmo, *vmo_);
  vmo_match_.Emplace(vmo, match);
  return match;
}

void BucketMatch::InvalidateProcess(zx_koid_t koid) { process_match_.Erase(koid); }

void BucketMatch::Sweep() {
  process_match_.Sweep();
  vmo_match_.Sweep();
}

std::optional<std::vector<BucketMatch>> BucketMatch::ReadBucketMatchesFromConfig(
    const std::string& config_string) {
  std::vector<BucketMatch> result;
  rapidjson::Document doc;
  doc.Parse(config_string);
  if (!doc.IsArray()) {
    FX_LOGS(WARNING) << "Configuration is not valid JSON";
    return std::nullopt;
  }

  result.reserve(doc.GetArray().Size());

  for (const auto& v : doc.GetArray()) {
    if (!(v.HasMember("name") && v["name"].IsString() && v.HasMember("process") &&
          v["process"].IsString() && v.HasMember("vmo") && v["vmo"].IsString())) {
      FX_LOGS(WARNING) << "Missing member";
      return std::nullopt;
    }

    if (v.HasMember("principal")) {
      // Principal filtering is only supported in memory_monitor2; skip those.
      continue;
    }
    std::string name(v["name"].GetString(), v["name"].GetStringLength());
    std::string process(v["process"].GetString(), v["process"].GetStringLength());
    std::string vmo(v["vmo"].GetString(), v["vmo"].GetStringLength());
    std::optional<int64_t> event_code;
    if (v.HasMember("event_code") && v["event_code"].IsInt64()) {
      event_code = v["event_code"].GetInt64();
    }
    result.emplace_back(name, process, vmo, event_code);
  }
  return result;
}

}  // namespace memory
