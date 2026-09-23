// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_USB_LIB_USB_INSPECT_INCLUDE_USB_INSPECT_USB_INSPECT_TEST_HELPER_H_
#define SRC_DEVICES_USB_LIB_USB_INSPECT_INCLUDE_USB_INSPECT_USB_INSPECT_TEST_HELPER_H_

#include <lib/fit/result.h>
#include <lib/fpromise/single_threaded_executor.h>
#include <lib/inspect/cpp/hierarchy.h>
#include <lib/inspect/cpp/reader.h>

#include <optional>
#include <string>

namespace usb_inspect {

// Synchronously reads the entire Inspect hierarchy (including lazy nodes) for testing.
inline inspect::Hierarchy ReadHierarchyFromInspector(const inspect::Inspector& inspector) {
  fpromise::single_threaded_executor executor;
  fpromise::result<inspect::Hierarchy> hierarchy_res;
  executor.schedule_task(inspect::ReadFromInspector(inspector).then(
      [&](fpromise::result<inspect::Hierarchy>& res) { hierarchy_res = std::move(res); }));
  executor.run();
  return hierarchy_res.take_value();
}

// Helper to verify EndpointInspect node properties.
// Returns fit::ok() on success, or a fit::error with a descriptive error string on failure.
inline fit::result<std::string> VerifyEndpointInspect(
    const inspect::Hierarchy* node, std::optional<uint64_t> total_bytes_tx = std::nullopt,
    std::optional<uint64_t> total_bytes_rx = std::nullopt,
    std::optional<uint64_t> tx_pending_requests = std::nullopt,
    std::optional<uint64_t> rx_pending_requests = std::nullopt,
    std::optional<uint64_t> max_bytes_per_second = std::nullopt,
    std::optional<uint64_t> rx_pending_processing = std::nullopt,
    std::optional<uint64_t> failed_bytes_tx = std::nullopt,
    std::optional<uint64_t> failed_bytes_rx = std::nullopt) {
  if (!node) {
    return fit::error("Node is null");
  }

  auto verify_prop = [&](const std::string& name,
                         std::optional<uint64_t> expected) -> fit::result<std::string> {
    if (!expected)
      return fit::ok();
    const auto* prop = node->node().get_property<inspect::UintPropertyValue>(name);
    if (!prop) {
      return fit::error("Property '" + name + "' is missing");
    }
    if (prop->value() != *expected) {
      return fit::error("Property '" + name + "' mismatch: expected " + std::to_string(*expected) +
                        ", got " + std::to_string(prop->value()));
    }
    return fit::ok();
  };

  if (auto res = verify_prop("total_bytes_tx", total_bytes_tx); res.is_error())
    return res;
  if (auto res = verify_prop("total_bytes_rx", total_bytes_rx); res.is_error())
    return res;
  if (auto res = verify_prop("tx_pending_requests", tx_pending_requests); res.is_error())
    return res;
  if (auto res = verify_prop("rx_pending_requests", rx_pending_requests); res.is_error())
    return res;
  if (auto res = verify_prop("max_bytes_per_second", max_bytes_per_second); res.is_error())
    return res;
  if (auto res = verify_prop("rx_pending_processing", rx_pending_processing); res.is_error())
    return res;

  if (auto res = verify_prop("failed_bytes_tx", failed_bytes_tx); res.is_error())
    return res;
  if (auto res = verify_prop("failed_bytes_rx", failed_bytes_rx); res.is_error())
    return res;

  if (const auto* transfer_snapshots = node->GetByPath({"transfer_snapshots"});
      transfer_snapshots) {
    for (const auto& snapshot_entry : transfer_snapshots->children()) {
      if (!snapshot_entry.node().get_property<inspect::UintPropertyValue>("@time")) {
        return fit::error("transfer_snapshots '" + snapshot_entry.name() + "' missing '@time'");
      }
      if (!snapshot_entry.node().get_property<inspect::UintPropertyValue>("total_bytes_tx")) {
        return fit::error("transfer_snapshots '" + snapshot_entry.name() +
                          "' missing 'total_bytes_tx'");
      }
      if (!snapshot_entry.node().get_property<inspect::UintPropertyValue>("total_bytes_rx")) {
        return fit::error("transfer_snapshots '" + snapshot_entry.name() +
                          "' missing 'total_bytes_rx'");
      }
      if (!snapshot_entry.node().get_property<inspect::UintPropertyValue>("max_bytes_per_second")) {
        return fit::error("transfer_snapshots '" + snapshot_entry.name() +
                          "' missing 'max_bytes_per_second'");
      }
    }
  }

  return fit::ok();
}

}  // namespace usb_inspect

#endif  // SRC_DEVICES_USB_LIB_USB_INSPECT_INCLUDE_USB_INSPECT_USB_INSPECT_TEST_HELPER_H_
