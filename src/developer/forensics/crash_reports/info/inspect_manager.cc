// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/crash_reports/info/inspect_manager.h"

#include <lib/syslog/cpp/macros.h>
#include <lib/zx/time.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <map>
#include <utility>

#include "src/developer/forensics/crash_reports/errors.h"
#include "src/lib/files/path.h"

namespace forensics {
namespace crash_reports {
namespace {

using files::JoinPath;

}  // namespace

InspectManager::InspectManager(inspect::Node* root_node) : node_manager_(root_node) {
  node_manager_.Get("/crash_reporter/settings");
}

void InspectManager::ExposeReportingPolicy(ReportingPolicyWatcher* watcher) {
  settings_.upload_policy = node_manager_.Get("/crash_reporter/settings")
                                .CreateString("upload_policy", ToString(watcher->CurrentPolicy()));

  watcher->OnPolicyChange(
      [=, this](const ReportingPolicy policy) { settings_.upload_policy.Set(ToString(policy)); });
}

void InspectManager::ExposeStore(const StorageSize max_size) {
  // TODO(https://fxbug.dev/42060400): This field will be ambiguous once snapshots are persisted.
  store_.max_size_in_kb = node_manager_.Get("/crash_reporter/store")
                              .CreateUint("max_size_in_kb", max_size.ToKilobytes());
}

void InspectManager::IncreaseReportsGarbageCollectedBy(uint64_t num_reports) {
  if (!store_.num_garbage_collected) {
    store_.num_garbage_collected = node_manager_.Get("/crash_reporter/store")
                                       .CreateUint("num_reports_garbage_collected", num_reports);
  } else {
    store_.num_garbage_collected.Add(num_reports);
  }
}

void InspectManager::UpsertComponentToProductMapping(const std::string& component_url,
                                                     const crash_reports::Product& product) {
  const std::string path =
      JoinPath("/crash_register/mappings", InspectNodeManager::SanitizeString(component_url));
  inspect::Node& node = node_manager_.Get(path);

  component_to_products_[component_url] = Product{
      .name = node.CreateString("name", product.name),
      .version = node.CreateString("version", product.version.HasValue()
                                                  ? product.version.Value()
                                                  : ToReason(product.version.Error())),
      .channel = node.CreateString("channel", product.channel.HasValue()
                                                  ? product.channel.Value()
                                                  : ToReason(product.channel.Error())),
  };
}

}  // namespace crash_reports
}  // namespace forensics
