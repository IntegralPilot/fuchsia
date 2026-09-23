// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <zircon/syscalls.h>
#include <zircon/syscalls/clock.h>

#include <usb-inspect/usb-inspect.h>
#include <usb/descriptors.h>
// We include this legacy Banjo header strictly to retrieve the C-struct layouts of standard
// USB descriptors (usb_interface_descriptor_t, usb_endpoint_descriptor_t) to dynamically
// parse and serialize the descriptor tree in our lazy inspect callback.
#include <fuchsia/hardware/usb/c/banjo.h>

#include <format>

namespace usb_inspect {

const char* SpeedToString(usb_speed_t speed) {
  switch (speed) {
    case USB_SPEED_FULL:
      return "full";
    case USB_SPEED_LOW:
      return "low";
    case USB_SPEED_HIGH:
      return "high";
    case USB_SPEED_SUPER:
      return "super";
    default:
      return "undefined";
  }
}

// ============================================================================
// EndpointInspect Implementation
// ============================================================================

void EndpointInspect::Init(inspect::Node& parent, const std::string& name, size_t event_capacity,
                           size_t transfer_snapshot_capacity) {
  {
    std::lock_guard<std::mutex> _(lock_);
    event_history_capacity_ = event_capacity;
    event_history_.clear();
    event_history_index_ = 0;
    transfer_snapshot_capacity_ = transfer_snapshot_capacity;
    transfer_snapshots_.clear();
    transfer_snapshots_index_ = 0;
  }

  // Initialize atomic values
  tx_pending_requests_.store(0, std::memory_order_relaxed);
  rx_pending_requests_.store(0, std::memory_order_relaxed);
  rx_pending_processing_.store(0, std::memory_order_relaxed);
  total_bytes_tx_.store(0, std::memory_order_relaxed);
  total_bytes_rx_.store(0, std::memory_order_relaxed);
  failed_bytes_tx_.store(0, std::memory_order_relaxed);
  failed_bytes_rx_.store(0, std::memory_order_relaxed);
  max_bytes_per_second_.store(0, std::memory_order_relaxed);

  last_total_bytes_val_ = 0;

  // Create the pure lazy node!
  lazy_node_ = parent.CreateLazyNode(name, [this]() {
    inspect::Inspector inspector;
    auto& root = inspector.GetRoot();

    std::scoped_lock _(lock_);

    uint64_t cur_tx = total_bytes_tx_.load(std::memory_order_relaxed);
    uint64_t cur_rx = total_bytes_rx_.load(std::memory_order_relaxed);
    uint64_t cur_max_rate = max_bytes_per_second_.load(std::memory_order_relaxed);

    root.CreateUint("tx_pending_requests", tx_pending_requests_.load(std::memory_order_relaxed),
                    &inspector);
    root.CreateUint("rx_pending_requests", rx_pending_requests_.load(std::memory_order_relaxed),
                    &inspector);
    root.CreateUint("rx_pending_processing", rx_pending_processing_.load(std::memory_order_relaxed),
                    &inspector);
    root.CreateUint("total_bytes_tx", cur_tx, &inspector);
    root.CreateUint("total_bytes_rx", cur_rx, &inspector);
    if (auto failed_tx = failed_bytes_tx_.load(std::memory_order_relaxed); failed_tx > 0) {
      root.CreateUint("failed_bytes_tx", failed_tx, &inspector);
    }
    if (auto failed_rx = failed_bytes_rx_.load(std::memory_order_relaxed); failed_rx > 0) {
      root.CreateUint("failed_bytes_rx", failed_rx, &inspector);
    }
    root.CreateUint("max_bytes_per_second", cur_max_rate, &inspector);

    if (!event_history_.empty()) {
      auto event_node = root.CreateChild("event_history");
      size_t index = event_history_index_ - event_history_.size();
      for (const auto& entry : event_history_) {
        auto node = event_node.CreateChild(std::to_string(index));
        node.CreateUint("@time", entry.timestamp, &inspector);
        node.CreateString("event", entry.event, &inspector);
        inspector.emplace(std::move(node));
        index++;
      }
      inspector.emplace(std::move(event_node));
    }

    if (!transfer_snapshots_.empty()) {
      auto snapshot_parent = root.CreateChild("transfer_snapshots");
      size_t index = transfer_snapshots_index_ - transfer_snapshots_.size();
      for (const auto& entry : transfer_snapshots_) {
        auto node = snapshot_parent.CreateChild(std::to_string(index));
        node.CreateUint("@time", entry.timestamp, &inspector);
        node.CreateUint("total_bytes_tx", entry.stats.total_bytes_tx, &inspector);
        node.CreateUint("total_bytes_rx", entry.stats.total_bytes_rx, &inspector);
        node.CreateUint("max_bytes_per_second", entry.stats.max_bytes_per_second, &inspector);
        inspector.emplace(std::move(node));
        index++;
      }
      inspector.emplace(std::move(snapshot_parent));
    }

    return fpromise::make_ok_promise(std::move(inspector));
  });
}

void EndpointInspect::MeasureThroughput(zx::duration elapsed) {
  uint64_t total_bytes = total_bytes_tx_.load(std::memory_order_relaxed) +
                         total_bytes_rx_.load(std::memory_order_relaxed);
  uint64_t last_val = last_total_bytes_val_.load(std::memory_order_relaxed);
  uint64_t delta = total_bytes - last_val;
  last_total_bytes_val_.store(total_bytes, std::memory_order_relaxed);

  int64_t elapsed_ns = elapsed.to_nsecs();
  if (elapsed_ns > 0) {
    uint64_t rate =
        static_cast<uint64_t>((static_cast<unsigned __int128>(delta) * 1'000'000'000) / elapsed_ns);
    uint64_t max_rate = max_bytes_per_second_.load(std::memory_order_relaxed);
    if (rate > max_rate) {
      max_bytes_per_second_.store(rate, std::memory_order_relaxed);
    }
  }
}

void EndpointInspect::SnapshotTransferStatsLocked(zx_instant_boot_t raw_time) {
  if (transfer_snapshot_capacity_ == 0) {
    return;
  }
  if (transfer_snapshots_.size() >= transfer_snapshot_capacity_) {
    transfer_snapshots_.pop_front();
  }
  uint64_t tx = total_bytes_tx_.load(std::memory_order_relaxed);
  uint64_t rx = total_bytes_rx_.load(std::memory_order_relaxed);
  uint64_t max_rate = max_bytes_per_second_.load(std::memory_order_relaxed);
  transfer_snapshots_.push_back(TransferSnapshotEntry{
      .timestamp = raw_time,
      .stats =
          TransferStats{
              .total_bytes_tx = tx,
              .total_bytes_rx = rx,
              .max_bytes_per_second = max_rate,
          },
  });
  transfer_snapshots_index_++;
}

void EndpointInspect::SnapshotTransferStats() {
  if (transfer_snapshot_capacity_ == 0) {
    return;
  }
  std::scoped_lock _(lock_);
  SnapshotTransferStatsLocked(zx_clock_get_boot());
}

void EndpointInspect::RecordEvent(const std::string& event_name) {
  if (event_history_capacity_ == 0) {
    return;
  }
  std::scoped_lock _(lock_);
  if (event_history_.size() >= event_history_capacity_) {
    event_history_.pop_front();
  }

  zx_instant_boot_t raw_time = zx_clock_get_boot();
  event_history_.push_back(EventLogEntry{
      .timestamp = raw_time,
      .event = event_name,
  });
  event_history_index_++;
  SnapshotTransferStatsLocked(raw_time);
}

// ============================================================================
// DciInspect Implementation
// ============================================================================

void DciInspect::Init(inspect::Node& parent, const std::string& name, size_t ctrl_capacity,
                      size_t event_capacity) {
  {
    std::lock_guard<std::mutex> _(lock_);
    event_history_capacity_ = event_capacity;
    event_history_.clear();
    event_history_index_ = 0;

    control_history_capacity_ = ctrl_capacity;
    control_history_.clear();
    control_history_index_ = 0;

    state_ = "unknown";
    connected_ = false;
    speed_ = "undefined";
    usb_mode_ = "unknown";
  }

  // Create the pure lazy node!
  lazy_node_ = parent.CreateLazyNode(name, [this]() {
    inspect::Inspector inspector;
    auto& root = inspector.GetRoot();

    std::lock_guard<std::mutex> _(lock_);
    root.CreateString("state", state_, &inspector);
    root.CreateBool("connected", connected_, &inspector);
    root.CreateString("speed", speed_, &inspector);
    root.CreateString("usb_mode", usb_mode_, &inspector);

    if (!event_history_.empty()) {
      auto event_node = root.CreateChild("event_history");
      size_t index = event_history_index_ - event_history_.size();
      for (const auto& entry : event_history_) {
        auto node = event_node.CreateChild(std::to_string(index++));
        node.CreateUint("@time", entry.timestamp, &inspector);
        node.CreateString("event", entry.event, &inspector);
        inspector.emplace(std::move(node));
      }
      inspector.emplace(std::move(event_node));
    }

    if (!control_history_.empty()) {
      auto control_node = root.CreateChild("control_history");
      size_t index = control_history_index_ - control_history_.size();
      for (const auto& entry : control_history_) {
        auto node = control_node.CreateChild(std::to_string(index++));
        node.CreateUint("@time", entry.timestamp, &inspector);
        node.CreateUint("bm_request_type", entry.bm_request_type, &inspector);
        node.CreateUint("b_request", entry.b_request, &inspector);
        node.CreateUint("w_value", entry.w_value, &inspector);
        node.CreateUint("w_index", entry.w_index, &inspector);
        node.CreateUint("w_length", entry.w_length, &inspector);
        node.CreateInt("status", entry.status, &inspector);
        node.CreateUint("response_length", entry.response_length, &inspector);
        inspector.emplace(std::move(node));
      }
      inspector.emplace(std::move(control_node));
    }

    return fpromise::make_ok_promise(std::move(inspector));
  });
}

void DciInspect::UpdateState(const std::string& state) {
  std::lock_guard<std::mutex> _(lock_);
  state_ = state;
}

void DciInspect::UpdateConnectionStatus(bool connected, usb_speed_t speed) {
  std::lock_guard<std::mutex> _(lock_);
  connected_ = connected;
  speed_ = SpeedToString(speed);
}

void DciInspect::UpdateUsbMode(usb_mode_t usb_mode) {
  std::lock_guard<std::mutex> _(lock_);
  usb_mode_ = usb_mode_to_string(usb_mode);
}

void DciInspect::RecordEvent(const std::string& event_name) {
  if (event_history_capacity_ == 0) {
    return;
  }
  std::lock_guard<std::mutex> _(lock_);
  if (event_history_.size() >= event_history_capacity_) {
    event_history_.pop_front();
  }

  zx_instant_boot_t raw_time = zx_clock_get_boot();
  event_history_.push_back(EventLogEntry{raw_time, event_name});
  event_history_index_++;
}

void DciInspect::RecordControlTransfer(const ControlTransferInfo& info) {
  if (control_history_capacity_ == 0) {
    return;
  }
  std::lock_guard<std::mutex> _(lock_);
  if (control_history_.size() >= control_history_capacity_) {
    control_history_.pop_front();
  }

  zx_instant_boot_t raw_time = zx_clock_get_boot();
  control_history_.push_back(ControlTransferLogEntry{
      .timestamp = raw_time,
      .bm_request_type = info.request_type,
      .b_request = info.request,
      .w_value = info.value,
      .w_index = info.index,
      .w_length = info.length,
      .status = info.status,
      .response_length = info.actual_length,
  });
  control_history_index_++;
}

// ============================================================================
// FunctionInspect Implementation
// ============================================================================

void FunctionInspect::Init(inspect::Node& parent, const std::string& name, uint8_t index,
                           size_t event_capacity) {
  {
    std::lock_guard<std::mutex> _(lock_);
    event_history_capacity_ = event_capacity;
    event_history_.clear();
    event_history_index_ = 0;

    index_ = index;
    configuration_ = 0;
    configured_ = false;
    interface_class_ = 0;
    interface_subclass_ = 0;
    interface_protocol_ = 0;
  }

  lazy_node_ = parent.CreateLazyNode(name, [this]() {
    inspect::Inspector inspector;
    auto& root = inspector.GetRoot();

    std::lock_guard<std::mutex> _(lock_);
    root.CreateUint("index", index_, &inspector);
    root.CreateUint("configuration", configuration_, &inspector);
    root.CreateBool("configured", configured_, &inspector);
    root.CreateUint("interface_class", interface_class_, &inspector);
    root.CreateUint("interface_subclass", interface_subclass_, &inspector);
    root.CreateUint("interface_protocol", interface_protocol_, &inspector);

    if (!descriptors_.empty()) {
      const uint8_t* ptr = descriptors_.data();
      const uint8_t* end = descriptors_.data() + descriptors_.size();
      size_t interface_index = 0;
      std::optional<inspect::Node> current_intf_node;

      while (ptr < end) {
        const usb_descriptor_header_t* header =
            reinterpret_cast<const usb_descriptor_header_t*>(ptr);
        if (header->b_length == 0 || ptr + header->b_length > end) {
          ZX_DEBUG_ASSERT_MSG(
              false, "usb-inspect: Invalid descriptor header b_length=%d, ptr=%p, end=%p",
              header->b_length, static_cast<const void*>(ptr), static_cast<const void*>(end));
          break;
        }

        if (header->b_descriptor_type == USB_DT_INTERFACE) {
          if (current_intf_node.has_value()) {
            inspector.emplace(std::move(*current_intf_node));
          }

          const usb_interface_descriptor_t* desc =
              reinterpret_cast<const usb_interface_descriptor_t*>(header);
          std::string intf_name = std::format("interface-{:03d}", interface_index++);

          current_intf_node = root.CreateChild(intf_name);
          current_intf_node->CreateUint("interface_number", desc->b_interface_number, &inspector);
          current_intf_node->CreateUint("alternate_setting", desc->b_alternate_setting, &inspector);
          current_intf_node->CreateUint("num_endpoints", desc->b_num_endpoints, &inspector);
          current_intf_node->CreateUint("interface_class", desc->b_interface_class, &inspector);
          current_intf_node->CreateUint("interface_subclass", desc->b_interface_sub_class,
                                        &inspector);
          current_intf_node->CreateUint("interface_protocol", desc->b_interface_protocol,
                                        &inspector);

        } else if (header->b_descriptor_type == USB_DT_ENDPOINT) {
          const usb_endpoint_descriptor_t* desc =
              reinterpret_cast<const usb_endpoint_descriptor_t*>(header);
          std::string ep_name = std::format("endpoint-0x{:02x}", desc->b_endpoint_address);

          if (!current_intf_node.has_value()) {
            ZX_DEBUG_ASSERT_MSG(
                false,
                "usb-inspect: Endpoint descriptor (address=0x%02x) found before any interface descriptor!",
                desc->b_endpoint_address);
            break;
          }
          inspect::Node ep_node = current_intf_node->CreateChild(ep_name);

          ep_node.CreateUint("endpoint_address", desc->b_endpoint_address, &inspector);
          ep_node.CreateUint("attributes", desc->bm_attributes, &inspector);
          ep_node.CreateUint("max_packet_size", le16toh(desc->w_max_packet_size), &inspector);
          ep_node.CreateUint("interval", desc->b_interval, &inspector);

          inspector.emplace(std::move(ep_node));
        }
        ptr += header->b_length;
      }
      if (current_intf_node.has_value()) {
        inspector.emplace(std::move(*current_intf_node));
      }
    }

    if (!event_history_.empty()) {
      auto event_node = root.CreateChild("event_history");
      size_t index = event_history_index_ - event_history_.size();
      for (const auto& entry : event_history_) {
        auto node = event_node.CreateChild(std::to_string(index++));
        node.CreateUint("@time", entry.timestamp, &inspector);
        node.CreateString("event", entry.event, &inspector);
        inspector.emplace(std::move(node));
      }
      inspector.emplace(std::move(event_node));
    }

    return fpromise::make_ok_promise(std::move(inspector));
  });
}

void FunctionInspect::UpdateConfiguration(uint8_t config, bool configured) {
  std::scoped_lock _(lock_);
  configuration_ = config;
  configured_ = configured;
}

void FunctionInspect::UpdateDescriptorInfo(uint8_t intf_class, uint8_t subclass, uint8_t protocol) {
  std::scoped_lock _(lock_);
  interface_class_ = intf_class;
  interface_subclass_ = subclass;
  interface_protocol_ = protocol;
}

void FunctionInspect::SetDescriptors(std::vector<uint8_t> descriptors) {
  std::scoped_lock _(lock_);
  descriptors_ = std::move(descriptors);
}

void FunctionInspect::RecordEvent(const std::string& event_name) {
  if (event_history_capacity_ == 0) {
    return;
  }
  std::scoped_lock _(lock_);
  if (event_history_.size() >= event_history_capacity_) {
    event_history_.pop_front();
  }

  zx_instant_boot_t raw_time = zx_clock_get_boot();
  event_history_.push_back(EventLogEntry{raw_time, event_name});
  event_history_index_++;
}

}  // namespace usb_inspect
