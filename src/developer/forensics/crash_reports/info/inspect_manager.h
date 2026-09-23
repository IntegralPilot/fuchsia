// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_CRASH_REPORTS_INFO_INSPECT_MANAGER_H_
#define SRC_DEVELOPER_FORENSICS_CRASH_REPORTS_INFO_INSPECT_MANAGER_H_

#include <lib/inspect/cpp/vmo/types.h>

#include <cstdint>
#include <map>
#include <string>
#include <vector>

#include "src/developer/forensics/crash_reports/product.h"
#include "src/developer/forensics/crash_reports/reporting_policy_watcher.h"
#include "src/developer/forensics/utils/inspect_node_manager.h"
#include "src/developer/forensics/utils/storage_size.h"
#include "src/lib/fxl/macros.h"

namespace forensics {
namespace crash_reports {

// Encapsulates the global state exposed through Inspect.
class InspectManager {
 public:
  explicit InspectManager(inspect::Node* root_node);

  // Exposes the reporting policy of the crash reporter.
  void ExposeReportingPolicy(ReportingPolicyWatcher* watcher);

  // Exposes the static properties of the report store.
  void ExposeStore(StorageSize max_size);

  // Upserts the mapping component URL to Product that a client registered.
  void UpsertComponentToProductMapping(const std::string& component_url, const Product& product);

  // Increase the total number of garbage collected reports by |num_reports|.
  void IncreaseReportsGarbageCollectedBy(uint64_t num_reports);

 private:
  // Inspect node containing the mutable settings.
  struct Settings {
    inspect::StringProperty upload_policy;
  };

  // Inspect node containing the store properties.
  struct Store {
    inspect::UintProperty max_size_in_kb;
    inspect::UintProperty num_garbage_collected;
  };

  // Inspect node for a single product.
  struct Product {
    inspect::StringProperty name;
    inspect::StringProperty version;
    inspect::StringProperty channel;
  };

  InspectNodeManager node_manager_;

  Settings settings_;
  Store store_;

  // Maps a component URL to a |Product|.
  std::map<std::string, Product> component_to_products_;

  FXL_DISALLOW_COPY_AND_ASSIGN(InspectManager);
};

}  // namespace crash_reports
}  // namespace forensics

#endif  // SRC_DEVELOPER_FORENSICS_CRASH_REPORTS_INFO_INSPECT_MANAGER_H_
