// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BIN_DRIVER_MANAGER_BIND_BIND_RESULT_TRACKER_H_
#define SRC_DEVICES_BIN_DRIVER_MANAGER_BIND_BIND_RESULT_TRACKER_H_

#include <fidl/fuchsia.driver.development/cpp/fidl.h>
#include <lib/zircon-internal/thread_annotations.h>

namespace driver_manager {

using NodeBindingInfoResultCallback =
    fit::callback<void(fidl::VectorView<fuchsia_driver_development::wire::NodeBindingInfo>)>;

// Used to track binding results. Once the tracker reaches the expected result count, it invokes the
// callback. The expected result count must be greater than 0.
class BindResultTracker {
 public:
  explicit BindResultTracker(size_t expected_result_count,
                             NodeBindingInfoResultCallback result_callback);

  void ReportSuccessfulBind(const std::string_view& node_name, const std::string_view& driver);
  void ReportSuccessfulBind(
      const std::string_view& node_name,
      const std::vector<fuchsia_driver_framework::CompositeParent>& composite_parents);
  void ReportNoBind();

 private:
  void Complete(size_t current);
  fidl::Arena<> arena_;
  size_t expected_result_count_;
  size_t currently_reported_ TA_GUARDED(lock_);
  std::mutex lock_;
  NodeBindingInfoResultCallback result_callback_;
  std::vector<fuchsia_driver_development::wire::NodeBindingInfo> results_;
};

}  // namespace driver_manager

#endif  // SRC_DEVICES_BIN_DRIVER_MANAGER_BIND_BIND_RESULT_TRACKER_H_
