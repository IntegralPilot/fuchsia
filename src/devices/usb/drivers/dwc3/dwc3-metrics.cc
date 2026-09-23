// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/usb/drivers/dwc3/dwc3-metrics.h"

#include <lib/driver/mmio/cpp/mmio.h>
#include <zircon/syscalls.h>
#include <zircon/syscalls/clock.h>

#include <format>

#include "src/devices/usb/drivers/dwc3/dwc3-regs.h"
#include "src/devices/usb/drivers/dwc3/dwc3.h"

namespace dwc3 {

void Dwc3Metrics::RecordEvent(std::string message) {
  if (event_history_.size() >= kEventHistoryCapacity) {
    event_history_.pop_front();
  }
  event_history_.push_back(EventLogEntry{zx_clock_get_boot(), std::move(message)});
}

void Dwc3Metrics::Init() {
  // Initialize the local stats data
  time_start_ = zx_clock_get_boot();
  for (unsigned long& event_count : event_counts_) {
    event_count = 0;
  }
  max_event_batch_size_ = 0;
}

// Lazily produce the Inspect data.
inspect::Inspector Dwc3Metrics::RecordMetrics(fdf::MmioBuffer* mmio, Dwc3* dwc3) {
  inspect::Inspector inspector;
  auto& root = inspector.GetRoot();

  root.RecordUint("time_start", time_start_);
  root.RecordUint("time_stats", zx_clock_get_boot());
  root.RecordUint("max_event_batch_size", max_event_batch_size_);

  auto events_node = root.CreateChild("event_counts");
  for (uint32_t i = 0; i < static_cast<uint32_t>(MetricEventType::kDevtNumEventTypes); i++) {
    MetricEventType type = static_cast<MetricEventType>(i);
    events_node.RecordUint(std::format("{}", type), event_counts_[i]);
  }
  root.Record(std::move(events_node));

  if (!event_history_.empty()) {
    auto history_node = root.CreateChild("event_history");
    size_t idx = 0;
    for (const auto& entry : event_history_) {
      auto node = history_node.CreateChild(std::to_string(idx++));
      node.RecordUint("@time", entry.timestamp);
      node.RecordString("event", entry.message);
      history_node.Record(std::move(node));
    }
    root.Record(std::move(history_node));
  }

  // Hardware registers should only be sampled when the controller is actively running
  // in peripheral mode (both PHY power is on and the controller is started). Reading
  // MMIO registers while the controller is idle, disconnected, or unpowered can cause
  // hardware bus faults. Note that mmio and dwc3 can be null in tests.
  if (!dwc3 || !dwc3->power_on()) {
    root.RecordString("hardware_state", "powered_off");
  } else if (!dwc3->is_active()) {
    root.RecordString("hardware_state", "inactive");
  } else if (mmio) {
    // Read and decode core hardware registers
    auto gctl = GCTL::Get().ReadFrom(mmio);
    auto gctl_node = root.CreateChild("GCTL");
    gctl_node.RecordUint("raw", gctl.reg_value());
    gctl_node.RecordUint("PRTCAPDIR", gctl.PRTCAPDIR());
    gctl_node.RecordBool("CORESOFTRESET", gctl.CORESOFTRESET());
    gctl_node.RecordBool("DEBUGATTACH", gctl.DEBUGATTACH());
    root.Record(std::move(gctl_node));

    auto gsts = GSTS::Get().ReadFrom(mmio);
    auto gsts_node = root.CreateChild("GSTS");
    gsts_node.RecordUint("raw", gsts.reg_value());
    gsts_node.RecordUint("CURMOD", gsts.CURMOD());
    gsts_node.RecordBool("Device_IP", gsts.Device_IP());
    gsts_node.RecordBool("Host_IP", gsts.Host_IP());
    root.Record(std::move(gsts_node));

    auto dcfg = DCFG::Get().ReadFrom(mmio);
    auto dcfg_node = root.CreateChild("DCFG");
    dcfg_node.RecordUint("raw", dcfg.reg_value());
    dcfg_node.RecordUint("DEVADDR", dcfg.DEVADDR());
    dcfg_node.RecordUint("DEVSPD", dcfg.DEVSPD());
    dcfg_node.RecordUint("NUMP", dcfg.NUMP());
    root.Record(std::move(dcfg_node));

    auto dctl = DCTL::Get().ReadFrom(mmio);
    auto dctl_node = root.CreateChild("DCTL");
    dctl_node.RecordUint("raw", dctl.reg_value());
    dctl_node.RecordBool("RUN_STOP", dctl.RUN_STOP());
    dctl_node.RecordBool("CSFTRST", dctl.CSFTRST());
    dctl_node.RecordBool("KeepConnect", dctl.KeepConnect());
    root.Record(std::move(dctl_node));

    auto dsts = DSTS::Get().ReadFrom(mmio);
    auto dsts_node = root.CreateChild("DSTS");
    dsts_node.RecordUint("raw", dsts.reg_value());
    dsts_node.RecordBool("COREIDLE", dsts.COREIDLE());
    dsts_node.RecordBool("DEVCTRLHLT", dsts.DEVCTRLHLT());
    dsts_node.RecordUint("USBLNKST", dsts.USBLNKST());
    dsts_node.RecordUint("CONNECTSPD", dsts.CONNECTSPD());
    dsts_node.RecordUint("SOFFN", dsts.SOFFN());
    root.Record(std::move(dsts_node));
  }

  if (dwc3) {
    auto endpoints_node = root.CreateChild("endpoints");
    auto record_endpoint = [](const Dwc3::Endpoint& ep, inspect::Node& ep_node) {
      ep_node.RecordUint("type", static_cast<uint64_t>(ep.type));
      ep_node.RecordUint("interval", ep.interval);
      ep_node.RecordUint("max_packet_size", ep.max_packet_size);
      ep_node.RecordBool("enabled", ep.enabled);
      ep_node.RecordUint("rsrc_id", ep.rsrc_id);
      ep_node.RecordBool("stalled", ep.stalled);
      ep_node.RecordString("transfer_state", std::format("{}", ep.transfer_state));
      ep_node.RecordBool("got_not_ready", ep.got_not_ready);
      ep_node.RecordUint("total_transfers", ep.total_transfers);
      ep_node.RecordUint("total_bytes", ep.total_bytes);
      ep_node.RecordUint("command_failures", ep.command_failures);
      ep_node.RecordUint("usb_endpoint_address", ep.usb_endpoint_address);
    };

    auto ep0_out_node =
        endpoints_node.CreateChild(std::format("endpoint-0x{:02x}", dwc3->ep0_.out.ep_num));
    record_endpoint(dwc3->ep0_.out, ep0_out_node);
    endpoints_node.Record(std::move(ep0_out_node));

    auto ep0_in_node =
        endpoints_node.CreateChild(std::format("endpoint-0x{:02x}", dwc3->ep0_.in.ep_num));
    record_endpoint(dwc3->ep0_.in, ep0_in_node);
    endpoints_node.Record(std::move(ep0_in_node));

    for (auto& uep : dwc3->user_endpoints_) {
      auto& ep = uep.ep;
      // Skip physical endpoints that have never been configured or used.
      if (!ep.enabled && ep.usb_endpoint_address == 0 && ep.total_transfers == 0) {
        continue;
      }
      auto ep_node = endpoints_node.CreateChild(std::format("endpoint-0x{:02x}", ep.ep_num));
      record_endpoint(ep, ep_node);

      if (uep.server.has_value()) {
        auto vmos_info = uep.server->GetRegisteredVmosInfo();
        if (!vmos_info.empty()) {
          auto vmos_node = ep_node.CreateChild("registered_vmos");
          bool all_same = true;
          uint64_t first_size = vmos_info[0].size;
          for (const auto& [id, size] : vmos_info) {
            if (size != first_size) {
              all_same = false;
              break;
            }
          }
          if (all_same) {
            vmos_node.RecordUint("count", vmos_info.size());
            vmos_node.RecordUint("size", first_size);
          } else {
            for (const auto& [id, size] : vmos_info) {
              vmos_node.RecordUint(std::to_string(id), size);
            }
          }
          ep_node.Record(std::move(vmos_node));
        }
      }

      auto& fifo = uep.fifo;
      if (fifo.TotalSlots() > 0) {
        auto fifo_node = ep_node.CreateChild("trb_fifo");
        fifo_node.RecordUint("total_slots", fifo.TotalSlots());
        fifo_node.RecordUint("write_offset", fifo.WriteOffset());
        fifo_node.RecordUint("read_offset", fifo.ReadOffset());
        fifo_node.RecordUint("available_slots", fifo.AvailableSlots());

        // Record active TRBs currently pending in the FIFO ring buffer.
        if (!fifo.IsEmpty()) {
          size_t active_count = fifo.GetActiveCount();
          std::vector<dwc3_trb_t> active_trbs_data = fifo.Fifo::Read(active_count);

          auto active_trbs = fifo_node.CreateChild("active_trbs");
          for (size_t idx = 0; idx < active_trbs_data.size(); ++idx) {
            const auto& trb = active_trbs_data[idx];
            auto trb_node = active_trbs.CreateChild(std::to_string(idx));
            trb_node.RecordUint("ptr_low", trb.ptr_low);
            trb_node.RecordUint("ptr_high", trb.ptr_high);
            trb_node.RecordUint("status", trb.status);
            trb_node.RecordUint("control", trb.control);
            trb_node.RecordBool("hardware_owned", (trb.control & TRB_HWO) != 0);

            active_trbs.Record(std::move(trb_node));
          }
          fifo_node.Record(std::move(active_trbs));
        }
        ep_node.Record(std::move(fifo_node));
      }
      endpoints_node.Record(std::move(ep_node));
    }
    root.Record(std::move(endpoints_node));

    auto ep0_state_node = root.CreateChild("ep0_buffer");
    ep0_state_node.RecordString("state", std::format("{}", dwc3->ep0_.state));
    ep0_state_node.RecordUint("cur_transfer_len", dwc3->ep0_.cur_transfer_len);
    ep0_state_node.RecordUint("bm_request_type", dwc3->ep0_.cur_setup.bm_request_type);
    ep0_state_node.RecordUint("b_request", dwc3->ep0_.cur_setup.b_request);
    ep0_state_node.RecordUint("w_value", dwc3->ep0_.cur_setup.w_value);
    ep0_state_node.RecordUint("w_index", dwc3->ep0_.cur_setup.w_index);
    ep0_state_node.RecordUint("w_length", dwc3->ep0_.cur_setup.w_length);
    if (dwc3->ep0_.buffer) {
      ep0_state_node.RecordUint("buffer_size", dwc3->ep0_.buffer->size());
      ep0_state_node.RecordUint("buffer_phys", dwc3->ep0_.buffer->phys());
      if (dwc3->ep0_.buffer->virt()) {
        auto* ptr = reinterpret_cast<const uint8_t*>(dwc3->ep0_.buffer->virt());
        ep0_state_node.RecordString(
            "buffer_virt_first_16_bytes",
            std::format(
                "{:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x}",
                ptr[0], ptr[1], ptr[2], ptr[3], ptr[4], ptr[5], ptr[6], ptr[7], ptr[8], ptr[9],
                ptr[10], ptr[11], ptr[12], ptr[13], ptr[14], ptr[15]));
      }
    }
    root.Record(std::move(ep0_state_node));
  }

  return inspector;
}

}  // namespace dwc3
