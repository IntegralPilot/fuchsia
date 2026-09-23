// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_INTEL_DISPLAY_REGISTERS_ARBITER_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_INTEL_DISPLAY_REGISTERS_ARBITER_H_

#include <zircon/assert.h>

#include <bit>
#include <cstdint>

#include <hwreg/bitfields.h>

#include "src/graphics/display/drivers/intel-display/hardware-common.h"

namespace registers {

// ARB_CTL (Display Arbitration Control 1)
//
// This register has bits that are reserved but not MBZ (must be zero). So, it
// can only be safely updated via read-modify-write operations.
//
// Tiger Lake: IHD-OS-TGL-Vol 2c-1.22-Rev2.0 Part 1 pages 51-53
// DG1: IHD-OS-DG1-Vol 2c-2.21 Part 1 pages 13-15
// Kaby Lake: IHD-OS-KBL-Vol 2c-1.17 Part 1 pages 82-83
// Skylake: IHD-OS-KBL-Vol 2c-1.17 Part 1 pages 81-82
class ArbitrationControl : public hwreg::RegisterBase<ArbitrationControl, uint32_t> {
 public:
  // If true, writes to FBC (framebuffer compression) surfaces wake up RAM.
  DEF_BIT(31, framebuffer_compression_writes_wake_memory);

  DEF_RSVDZ_BIT(29);

  // The HP (high-priority) queue is read after it has this many entries.
  //
  // This field has a non-trivial encoding. It should be used via the
  // `high_priority_queue_watermark()` and `set_high_priority_queue_watermark()`
  // helpers.
  DEF_FIELD(28, 26, high_priority_queue_watermark_bits);

  // Maximum number of LP (low-priority) write requests accepted from a client.
  //
  // This field has a non-trivial encoding. It should be used via the
  // `low_priority_back_to_back_request_limit()` and
  // `set_low_priority_back_to_back_request_limit()` helpers.
  DEF_FIELD(25, 24, low_priority_back_to_back_request_limit_bits);

  // Maximum number of TLB requests issued in one arbitration loop.
  //
  // This limit applies to TLB (translation lookaside buffer) and VT-D
  // (virtualized directed I/O) address translation requests.
  //
  // This value must not be zero.
  DEF_FIELD(23, 20, translation_lookaside_buffer_request_limit);

  // Maximum number of TLB requests in-flight.
  //
  // This limit applies to TLB (translation lookaside buffer) and VT-D
  // (virtualized directed I/O) address translation requests.
  //
  // This value must not be zero.
  DEF_FIELD(19, 16, translation_lookaside_buffer_in_flight_request_limit);

  // If true, the FBC (framebuffer compression) watermarks are disabled.
  DEF_BIT(15, framebuffer_compression_watermarks_disabled);

  // Zero on all supported display engines.
  //
  // This field was used to configure Display Engine-side swizzling of physical
  // memory address bits when accessing Y-tiled surfaces. On recent processors,
  // this feature has been deprecated in favor of DRAM controller-level address
  // swizzling, and the field should be set to 0 indicating no display requests
  // address swizzling on these processors.
  DEF_FIELD(14, 13, tiled_address_swizzling);

  // Maximum number of page breaks in a chain of HP (high-priority) requests.
  //
  // This value must not be zero.
  DEF_FIELD(12, 8, high_priority_request_chain_page_break_limit);

  DEF_RSVDZ_BIT(7);

  // Maximum number of cachelines in a chain of HP (high-priority) requests.
  //
  // This value must be greater than 8, which is the size of a DBUF (Display
  // Buffer) block.
  DEF_FIELD(6, 0, high_priority_request_chain_cache_line_limit);

  // Maximum number of LP (low-priority) write requests accepted from a client.
  int32_t high_priority_queue_watermark() const {
    // The cast and addition will not overflow (causing UB) because the
    // underlying field is a 3-bit integer.
    return static_cast<int32_t>(high_priority_queue_watermark_bits()) + 1;
  }

  // See `high_priority_queue_watermark()` for details.
  ArbitrationControl& set_high_priority_queue_watermark(int32_t watermark) {
    ZX_ASSERT(watermark >= 1);
    ZX_ASSERT(watermark <= 8);

    const uint32_t raw_watermark = static_cast<uint32_t>(watermark - 1);
    return set_high_priority_queue_watermark_bits(raw_watermark);
  }

  // Maximum number of LP (low-priority) write requests accepted from a client.
  //
  // After a client reaches this limit of back-to-back requests, re-arbitration
  // occurs.
  int32_t low_priority_back_to_back_request_limit() const {
    // The shift will not overflow (causing UB) because the underlying field is
    // a 2-bit integer.
    return int32_t{1} << low_priority_back_to_back_request_limit_bits();
  }

  // See `low_priority_back_to_back_request_limit()` for details.
  ArbitrationControl& set_low_priority_back_to_back_request_limit(int32_t request_limit) {
    const uint32_t raw_request_limit = TwoBitLog2(request_limit);
    return set_low_priority_back_to_back_request_limit_bits(raw_request_limit);
  }

  // `arbiter_index` zero (0) represents the global arbiter. The following
  // indexes represent the numbered arbiters.
  //
  // Kaby Lake and Skylake only have a global arbiter.
  static auto GetForArbiter(int arbiter_index) {
    ZX_ASSERT(arbiter_index >= 0);
    ZX_ASSERT(arbiter_index <= 2);

    if (arbiter_index == 0) {
      return hwreg::RegisterAddr<ArbitrationControl>(0x45000);
    }
    return hwreg::RegisterAddr<ArbitrationControl>(0x45800 - 8 + 8 * arbiter_index);
  }

  // Helper for setting 2-bit fields that use log2 representation.
  static uint32_t TwoBitLog2(int32_t value) {
    ZX_ASSERT(value >= 1);
    ZX_ASSERT(value <= 8);
    ZX_ASSERT((value & (value - 1)) == 0);
    return std::countr_zero(static_cast<uint32_t>(value));
  }
};

// ARB_CTL (Display Arbitration Control 2)
//
// This register has bits that are reserved but not MBZ (must be zero). So, it
// can only be safely updated via read-modify-write operations.
//
// Tiger Lake: IHD-OS-TGL-Vol 2c-1.22-Rev2.0 Part 1 pages 54-56
// DG1: IHD-OS-DG1-Vol 2c-2.21 Part 1 pages 13-15
// Kaby Lake: IHD-OS-KBL-Vol 2c-1.17 Part 1 pages 82-83
// Skylake: IHD-OS-KBL-Vol 2c-1.17 Part 1 pages 81-82
class ArbitrationControl2 : public hwreg::RegisterBase<ArbitrationControl2, uint32_t> {
 public:
  DEF_RSVDZ_BIT(30);

  // Maximum number of write requests accepted from WD (display capture).
  //
  // This field has a non-trivial encoding. It should be used via the helpers
  // `display_capture_write_request_limit()` and
  // `set_display_capture_write_request_limit()`.
  DEF_FIELD(29, 28, display_capture_write_request_limit_bits);

  // Maximum number of write requests accepted from DSB (Display State Buffer).
  //
  // This field has a non-trivial encoding. It should be used via the helpers
  // `display_state_buffer_write_request_limit_tiger_lake()` and
  // `set_display_state_buffer_write_request_limit_tiger_lake()`.
  DEF_FIELD(27, 26, display_state_buffer_write_request_limit_tiger_lake_bits);

  DEF_RSVDZ_FIELD(25, 20);

  // Maximum number of requests accepted from "par5".
  //
  // "par5" does not appear to be documented.
  //
  // This field does not exist on Kaby Lake or Skylake.
  //
  // This field has a non-trivial encoding.
  DEF_FIELD(19, 18, par5_request_limit_tiger_lake_bits);

  // Maximum number of requests accepted from FBC (framebuffer compression).
  //
  // This field has a non-trivial encoding. It should be used via the helpers
  // `framebuffer_compression_request_limit_tiger_lake()` and
  // `set_framebuffer_compression_request_limit_tiger_lake()`.
  DEF_FIELD(17, 16, framebuffer_compression_request_limit_tiger_lake_bits);

  DEF_RSVDZ_FIELD(15, 14);

  // If true, a HP request causes the arbiter to accept trickle feed requests.
  //
  // Trickle feed requests are sent by DBUF (Display Buffer) clients that
  // react immediately when a DBUF block that they own frees up. By contrast,
  // DBUF clients may send batched requests after a few blocks free up. The
  // concept was most recently documented in IHD-OS-VLV-Vol10-04.14 (the
  // Valleyview Display Engine PRM).
  //
  // If this bit is enabled, the arbiter allows trickle feed requests from all
  // clients when it receives a HP (high-priority) request from one client.
  DEF_BIT(12, high_priority_requests_enable_trickle_feed_requests);

  // The maximum number of in-flight LP (low-priority) read transactions.
  //
  // This field has a non-trivial encoding. It should be used via the helpers
  // `max_inflight_low_priority_read_requests()` and
  // `set_max_inflight_low_priority_read_requests()`.
  DEF_FIELD(10, 9, max_inflight_low_priority_read_requests_bits);

  DEF_RSVDZ_FIELD(8, 6);

  // The maximum number of in-flight HP (high-priority) read transactions.
  //
  // This field has a non-trivial encoding. It should be used via the helpers
  // `max_inflight_high_priority_read_requests()` and
  // `set_max_inflight_high_priority_read_requests()`.
  DEF_FIELD(5, 4, max_inflight_high_priority_read_requests_bits);

  // If false, the Display Engine always sends requests at isochronous priority.
  //
  // If true, the Display Engine may issue request at lower priority, which may
  // improve performance for other memory-bound workloads. The Display Engine's
  // requests will be demoted once the relevant transition watermark is reached.
  // If the transition watermark is not enabled, the Display Engine's requests
  // are demoted once the DBUF (Display Buffer) is full.
  //
  // The hardware only reads this bit from the global arbiter's configuration.
  // Other arbiters, such as Arbiter 1 and 2 on Tiger Lake, use the global
  // arbiter's setting.
  DEF_BIT(3, isochronous_priority_control_enabled);

  DEF_RSVDZ_BIT(2);

  // Minimum RTID queue size required to start HP (high-priority) transactions.
  //
  // This field has a non-trivial encoding. It should be used via the helpers
  // `request_transaction_id_queue_watermark()` and
  // `set_request_transaction_id_queue_watermark()`.
  DEF_FIELD(1, 0, request_transaction_id_queue_watermark_bits);

  // Maximum number of write requests accepted from WD (display capture).
  //
  // WD (Wireless Display / display capture) transcoders issue LP (low-priority)
  // write requests. Re-arbitration occurs after this limit is hit.
  int32_t display_capture_write_request_limit() const {
    // The shift will not overflow (causing UB) because the underlying field is
    // a 2-bit integer.
    return int32_t{1} << display_capture_write_request_limit_bits();
  }

  // See `display_capture_write_request_limit()` for details.
  ArbitrationControl2& set_display_capture_write_request_limit(int32_t request_limit) {
    const uint32_t raw_request_limit = ArbitrationControl::TwoBitLog2(request_limit);
    return set_display_capture_write_request_limit_bits(raw_request_limit);
  }

  // Maximum number of write requests accepted from DSB (Display State Buffer).
  //
  // DSB (Display State Buffer) engines issues LP (low-priority) write requests.
  // Re-arbitration occurs after this limit is hit.
  //
  // This field does not exist on Kaby Lake or Skylake.
  int32_t display_state_buffer_write_request_limit_tiger_lake() const {
    // The shift will not overflow (causing UB) because the underlying field is
    // a 2-bit integer.
    return int32_t{1} << display_state_buffer_write_request_limit_tiger_lake_bits();
  }

  // See `display_state_buffer_write_request_limit_tiger_lake()` for details.
  ArbitrationControl2& set_display_state_buffer_write_request_limit_tiger_lake(
      int32_t request_limit) {
    const uint32_t raw_request_limit = ArbitrationControl::TwoBitLog2(request_limit);
    return set_display_state_buffer_write_request_limit_tiger_lake_bits(raw_request_limit);
  }

  // Maximum number of requests accepted from "par5".
  //
  // The "par5" client / feature does not appear to be documented. Nevertheless,
  // re-arbitration occurs after this limit is hit.
  //
  // This field does not exist on Kaby Lake or Skylake.
  int32_t par5_request_limit_tiger_lake() const {
    // The shift will not overflow (causing UB) because the underlying field is
    // a 2-bit integer.
    const int32_t exponential_value = int32_t{1} << par5_request_limit_tiger_lake_bits();
    return exponential_value == 8 ? 16 : exponential_value;
  }

  // See `par5_request_limit_tiger_lake()` for details.
  ArbitrationControl2& set_par5_request_limit_tiger_lake(int32_t request_limit) {
    ZX_ASSERT(request_limit <= 16);
    ZX_ASSERT(request_limit != 8);
    const uint32_t raw_request_limit =
        ArbitrationControl::TwoBitLog2(request_limit == 16 ? 8 : request_limit);
    return set_par5_request_limit_tiger_lake_bits(raw_request_limit);
  }

  // Maximum number of requests accepted from FBC (framebuffer compression).
  //
  // The FBC (framebuffer compression) feature issues LP (low-priority) write
  // requests to update the compressed framebuffer. Re-arbitration occurs after
  // this limit is hit.
  //
  // This field does not exist on Kaby Lake or Skylake.
  int32_t framebuffer_compression_request_limit_tiger_lake() const {
    // The shift will not overflow (causing UB) because the underlying field is
    // a 2-bit integer.
    return int32_t{1} << framebuffer_compression_request_limit_tiger_lake_bits();
  }

  // See `framebuffer_compression_request_limit_tiger_lake()` for details.
  ArbitrationControl2& set_framebuffer_compression_request_limit_tiger_lake(int32_t request_limit) {
    const uint32_t raw_request_limit = ArbitrationControl::TwoBitLog2(request_limit);
    return set_framebuffer_compression_request_limit_tiger_lake_bits(raw_request_limit);
  }

  // The maximum number of in-flight LP (low-priority) read transactions.
  int32_t max_inflight_low_priority_read_requests() const {
    // The cast and addition will not overflow (causing UB) because the
    // underlying field is a 2-bit integer.
    return static_cast<int32_t>(max_inflight_low_priority_read_requests_bits()) + 1;
  }

  // See `max_inflight_low_priority_read_requests()` for details.
  ArbitrationControl2& set_max_inflight_low_priority_read_requests(int32_t max_requests) {
    ZX_ASSERT(max_requests >= 1);
    ZX_ASSERT(max_requests <= 8);

    const uint32_t raw_max_requests = static_cast<uint32_t>(max_requests - 1);
    return set_max_inflight_low_priority_read_requests_bits(raw_max_requests);
  }

  // The maximum number of in-flight HP (high-priority) read transactions.
  int32_t max_inflight_high_priority_read_requests() const {
    // The subtraction will not produce a negative number (leading to UB in the
    // shift operator) because the underlying field is a 2-bit integer.
    return int32_t{1} << (7 - max_inflight_high_priority_read_requests_bits());
  }

  // See `max_inflight_high_priority_read_requests()` for details.
  ArbitrationControl2& set_max_inflight_high_priority_read_requests(int32_t max_requests) {
    ZX_ASSERT(max_requests >= 16);
    ZX_ASSERT(max_requests <= 128);
    ZX_ASSERT((max_requests & (max_requests - 1)) == 0);
    const uint32_t raw_max_requests = 7 - std::countr_zero(static_cast<uint32_t>(max_requests));
    return set_max_inflight_high_priority_read_requests_bits(raw_max_requests);
  }

  // Minimum RTID queue size required to start HP (high-priority) transactions.
  //
  // RTIDs (Request Transaction IDs) are credits issued by the per-processor
  // cache coherence coordinator, which is the CHA (combined Caching and Home
  // Agent) on recent CPU models. These credits track outstanding transactions
  // across the cache coherence bus, which is UPI (Ultra-Path Interconnect) on
  // recent CPU models, and QPI (Quick-Path Interconnect) on older models. The
  // "RTID" acronym is defined in some processor datasheets, such as document
  // 735086 (Ice Lake Xeon processor datasheet, volume 2). The CHA is described
  // at a very high level in uncore performance monitoring manuals, such as
  // document
  //
  //
  // This watermark is the minimum level of the RTID FIFO (queue) required for
  // the arbiter to start HP (high-priority) transactions.
  //
  // The pseudocode in IHD-OS-TGL-Vol 12-1.22-Rev2.0 "Bandwidth Restrictions" >
  // "Available Memory Bandwidth Calculation" on pages 163-166 suggests that
  // each memory transaction (on each DRAM channel) requires an RTID.
  int32_t request_transaction_id_queue_watermark() const {
    // The subtraction will not produce a negative number (leading to UB in the
    // shift operator) because the underlying field is a 2-bit integer.
    return int32_t{8} << request_transaction_id_queue_watermark_bits();
  }

  // See `request_transaction_id_queue_watermark()` for details.
  ArbitrationControl2& set_request_transaction_id_queue_watermark(int32_t watermark) {
    ZX_ASSERT(watermark >= 8);
    ZX_ASSERT(watermark <= 32);
    ZX_ASSERT((watermark & (watermark - 1)) == 0);
    const uint32_t raw_watermark = std::countr_zero(static_cast<uint32_t>(watermark)) - 3;
    return set_request_transaction_id_queue_watermark_bits(raw_watermark);
  }

  // `arbiter_index` zero (0) represents the global arbiter. The following
  // indexes represent the numbered arbiters.
  //
  // Kaby Lake and Skylake only have a global arbiter.
  static auto GetForArbiter(int arbiter_index) {
    ZX_ASSERT(arbiter_index >= 0);
    ZX_ASSERT(arbiter_index <= 2);

    if (arbiter_index == 0) {
      return hwreg::RegisterAddr<ArbitrationControl2>(0x45004);
    }
    return hwreg::RegisterAddr<ArbitrationControl2>(0x45804 - 8 + 8 * arbiter_index);
  }
};

// BOOTVECTOR

// BW_BUDDY_CTL (Bandwidth Buddy Control)
//
// This register has bits that are reserved but not MBZ (must be zero). So, it
// can only be safely updated via read-modify-write operations.
//
// This register is not documented on Kaby Lake or Skylake.
//
// Rocket Lake: IHD-OS-RKL-Vol 2-7.22 page 30
// Tiger Lake: IHD-OS-TGL-Vol 2c-1.22-Rev2.0 Part 1 page 171
// DG1: IHD-OS-DG1-Vol 2c-2.21 Part 2 page 136
class BandwidthBuddyControl : public hwreg::RegisterBase<BandwidthBuddyControl, uint32_t> {
 public:
  // If true, the address buddy logic is disabled.
  DEF_BIT(31, disabled);

  DEF_RSVDZ_BIT(29);

  // Time that a regular request (for plane data) waits for a buddy.
  //
  // Plane data is fetched using HP (high-priority) requests. Each incoming
  // request has an associated timer, whose initial value comes from this field.
  // The timer tracks the amount of time that a request is stalled waiting for a
  // suitable buddy request.
  //
  // The timer balances the DRAM bandwidth savings obtained by the buddy system
  // (reducing the number of DRAM management operations) with the delays
  // introduced by stalling requests until suitable buddies come along.
  DEF_FIELD(28, 23, plane_data_request_timer);

  DEF_RSVDZ_BIT(22);

  // Time that a TLB request (for address translation data) waits for a buddy.
  //
  // Plane data is conceptually stored in graphics virtual memory, using the
  // GGTT (Global Graphics Translation Table). Similarly to IA (Intel
  // Architecture) CPUs, virtual-to-physical address translations are cached in
  // a TLB (translation lookaside buffer). TLB requests occur when fetching
  // plane data result in TLB misses, and the TLB controller needs to fetch
  // virtual-to-physical address translations from the GGTT.
  //
  // TLB requests are stalled waiting for suitable buddies, using the same
  // mechanism as `plane_data_request_timer`.
  DEF_FIELD(21, 16, address_translation_request_timer);

  DEF_RSVDZ_FIELD(14, 0);

  // Tiger Lake has two bandwidth buddy instances, at indexes 1 and 2.
  // Rocket Lake, has a single bandwidth buddy instance, at index 0.
  static auto GetForArbiter(int arbiter_index) {
    ZX_ASSERT(arbiter_index >= 0);
    ZX_ASSERT(arbiter_index <= 2);
    return hwreg::RegisterAddr<BandwidthBuddyControl>(0x45130 + 0x10 * arbiter_index);
  }
};

// BW_BUDDY_PAGE_MASK (Bandwidth Buddy Page Mask)
//
// This register has bits that are reserved but not MBZ (must be zero). So, it
// can only be safely updated via read-modify-write operations.
//
// This register is not documented on Kaby Lake or Skylake.
//
// Rocket Lake: IHD-OS-RKL-Vol 2-7.22 page 32
// Tiger Lake: IHD-OS-TGL-Vol 2c-1.22-Rev2.0 Part 1 page 173
// DG1: IHD-OS-DG1-Vol 2c-2.21 Part 2 page 138
class BandwidthBuddyPageMask : public hwreg::RegisterBase<BandwidthBuddyPageMask, uint32_t> {
 public:
  DEF_RSVDZ_FIELD(31, 28);

  // The address bits masked for buddy address comparison.
  //
  // The buddy system determines if two memory requests can be paired by
  // comparing bits 36-9 of the requests' physical addresses. However, the
  // comparison ignores the bits whose corresponding mask bits in this field are
  // set to 1. Conceptually, bits 36-9 in the physical address are ANDed with
  // the inverse (NOT) of bits 27-0 in this field before going into the
  // comparator.
  //
  // This field accounts for the system's DRAM configuration. The intent is that
  // the buddy system improves DRAM bandwidth efficiency by pairing up memory
  // requests in a way that cuts down on DRAM management operations, such as
  // opening rows.
  DEF_FIELD(27, 0, address_page_mask);

  // Tiger Lake has two bandwidth buddy instances, at indexes 1 and 2.
  // Rocket Lake, has a single bandwidth buddy instance, at index 0.
  static auto GetForArbiter(int arbiter_index) {
    ZX_ASSERT(arbiter_index >= 0);
    ZX_ASSERT(arbiter_index <= 2);
    return hwreg::RegisterAddr<BandwidthBuddyPageMask>(0x45134 + 0x10 * arbiter_index);
  }
};

// DEDICATED_PATH_ARB_CREDITS (Dedicated Path Arbiter Credits)
//
// This register has bits that are reserved but not MBZ (must be zero). So, it
// can only be safely updated via read-modify-write operations.
//
// Starting with Tiger Lake, the Display Engine has a dedicated path to memory
// through the processor's fabric. This dedicated path is intended to ensure
// that the Display Engine's latency requirements are met even under heavy
// memory contention.
//
// This register configures the credits are used for arbitrating between ABOXes
// (Arbiter Boxes) for streaming HP (high-priority) traffic to the dedicated
// memory path.
//
// Tiger Lake: IHD-OS-TGL-Vol 2c-1.22-Rev2.0 Part 1 page 415
// DG1: IHD-OS-DG1-Vol 2c-2.21 Part 2 page 396
class DedicatedPathArbiterCredits
    : public hwreg::RegisterBase<DedicatedPathArbiterCredits, uint32_t> {
 public:
  // Dedicated memory path credits for ABOX0.
  DEF_FIELD(14, 8, arbiter_box1_credits);

  // Dedicated memory path credits for ABOX1.
  DEF_FIELD(6, 0, arbiter_box0_credits);

  static auto Get() { return hwreg::RegisterAddr<DedicatedPathArbiterCredits>(0x100180); }
};

// MBUS_ABOX_CTL (MBus ABox Control)
//
// Does not exist on Kaby Lake or Skylake, which don't have MBus.
//
// ABoxes (Arbiter Boxes) are the attachment points for the arbiters, which are
// configured by the ArbitrationControl (ARB_CTL) registers.
//
// All reserved bits in this register are MBZ (must be zero). So, the register
// can be safely updated without reading it first.
//
// Tiger Lake: IHD-OS-TGL-Vol 2c-1.22-Rev2.0 Part 2 pages 8-9
// DG1: IHD-OS-DG1-Vol 2c-2.21 Part 2 pages 8-9
class MbusArbiterBoxControl : public hwreg::RegisterBase<MbusArbiterBoxControl, uint32_t> {
 public:
  // Read-only status bit.
  DEF_BIT(31, enabled);

  // Read-only address of the box in the ring.
  DEF_FIELD(30, 27, ring_stop_address);

  // Ignored if `back_to_back_transactions_regulation_enabled` is false.
  //
  // Used in conjunction with `back_to_back_transaction_delay` to limit the
  // number of back-to-back transactions sent from this box.
  //
  // Zero is not a valid value.
  DEF_FIELD(26, 22, max_back_to_back_transactions);

  // BW credits used to write to the DBUF (Display Buffer).
  //
  // These credits are used by the VGA host controller.
  DEF_FIELD(21, 20, display_buffer_write_credits);

  // B credits used by the Arbiter to read from DBUF (Display Buffer).
  //
  // These credits are used by the Arbiter, which reads data from DBUF and
  // writes it to the main memory as part of FBC (FrameBuffer Compression) and
  // WiDi (Display Capture).
  DEF_FIELD(19, 16, display_buffer_read_credits);

  DEF_RSVDZ_FIELD(15, 14);

  // If true, B2B (back-to-back) transaction regulation fields are in effect.
  DEF_BIT(13, back_to_back_transactions_regulation_enabled);

  // BT credits used by the Arbiter to request DBUF (Display Buffer) trackers.
  DEF_FIELD(12, 8, display_buffer_tracker_credits_pool2);

  // Number of wait cycles after `max_back_to_back_transactions` is hit.
  DEF_FIELD(7, 5, back_to_back_transaction_delay);

  // BT credits used by the Arbiter to request DBUF (Display Buffer) trackers.
  DEF_FIELD(4, 0, display_buffer_tracker_credits_pool1);

  // `arbiter_index` zero (0) represents the global arbiter. The following
  // indexes represent the numbered arbiters.
  static auto GetForArbiter(int arbiter_index) {
    ZX_ASSERT(arbiter_index >= 0);
    ZX_ASSERT(arbiter_index <= 2);

    if (arbiter_index == 0) {
      return hwreg::RegisterAddr<MbusArbiterBoxControl>(0x45038);
    }
    return hwreg::RegisterAddr<MbusArbiterBoxControl>(0x45044 + 4 * arbiter_index);
  }
};

// MBUS_BBOX_CTL (MBus BBox Control)
//
// Does not exist on Kaby Lake or Skylake, which don't have MBus.
//
// BBoxes (Buffer Boxes) are the attachment points for the DBUF (Display Buffer)
// slices.
//
// All reserved bits in this register are MBZ (must be zero). So, the register
// can be safely updated without reading it first.
//
// Tiger Lake: IHD-OS-TGL-Vol 2c-1.22-Rev2.0 Part 2 pages 10-11
// DG1: IHD-OS-DG1-Vol 2c-2.21 Part 2 pages 10-11
class MbusDisplayBufferBoxControl
    : public hwreg::RegisterBase<MbusDisplayBufferBoxControl, uint32_t> {
 public:
  // Read-only status bit.
  DEF_BIT(31, enabled);

  // Read-only address of the box in the ring.
  DEF_FIELD(30, 27, ring_stop_address);

  DEF_RSVDZ_FIELD(26, 25);

  // Ignored if `back_to_back_transactions_regulation_enabled` is false.
  //
  // Used in conjunction with `back_to_back_transaction_delay` to limit the
  // number of back-to-back transactions sent from this box.
  //
  // Zero is not a valid value.
  DEF_FIELD(24, 20, max_back_to_back_transactions);

  // Number of wait cycles after `max_back_to_back_transactions` is hit.
  DEF_FIELD(19, 17, back_to_back_transaction_delay);

  // If true, B2B (back-to-back) transaction regulation fields are in effect.
  DEF_BIT(16, back_to_back_transactions_regulation_enabled);

  DEF_RSVDZ_FIELD(15, 0);

  // `slice_index` is 0-based. Index 0 points to DBUF (Display Buffer) Slice 1.
  static auto GetForSlice(int slice_index) {
    ZX_ASSERT(slice_index >= 0);
    ZX_ASSERT(slice_index < 2);
    return hwreg::RegisterAddr<MbusDisplayBufferBoxControl>(0x45040 + 4 * slice_index);
  }
};

// MBUS_DBOX_CTL (Pipe MBus DBox Control)
//
// Does not exist on Kaby Lake or Skylake, which don't have MBus.
//
// All reserved bits in this register are MBZ (must be zero). So, the register
// can be safely updated without reading it first.
//
// Tiger Lake: IHD-OS-TGL-Vol 2c-1.22-Rev2.0 Part 2 pages 12-13
// DG1: IHD-OS-DG1-Vol 2c-2.21 Part 2 pages 12-13
class MbusPipeDataBoxControl : public hwreg::RegisterBase<MbusPipeDataBoxControl, uint32_t> {
 public:
  // Read-only status bit.
  DEF_BIT(31, enabled);

  // Read-only address of the box in the ring.
  DEF_FIELD(30, 27, ring_stop_address);

  DEF_RSVDZ_FIELD(26, 25);

  // Ignored if `back_to_back_transactions_regulation_enabled` is false.
  //
  // Used in conjunction with `back_to_back_transaction_delay` to limit the
  // number of back-to-back transactions sent from this box.
  //
  // Zero is not a valid value.
  DEF_FIELD(24, 20, max_back_to_back_transactions);

  // Number of wait cycles after `max_back_to_back_transactions` is hit.
  DEF_FIELD(19, 17, back_to_back_transaction_delay);

  // If true, B2B (back-to-back) transaction regulation fields are in effect.
  DEF_BIT(16, back_to_back_transactions_regulation_enabled);

  // BW credits used by the display pipe to write to the DBUF (Display Buffer).
  //
  // These credits are used to write data regarding color-clear, WiDi (display
  // capture), and FBC (framebuffer compression).
  DEF_FIELD(15, 14, display_buffer_write_credits);

  DEF_RSVDZ_BIT(13);

  // B credits used by the display pipe to read from DBUF (Display Buffer).
  //
  // The default value (0) is unsuitable for Display Engine operation. The PRM
  // recommends setting this field to 12.
  DEF_FIELD(12, 8, display_buffer_read_credits);

  DEF_RSVDZ_FIELD(7, 4);

  // A credits used by the display pipe to make requests to the Arbiter.
  //
  // Arbiter requests cover pipe data, TLB (address translation lookaside
  // buffer), VTd (virtualization for directed IO), and MCS (Multi-sample
  // Control Surface, described in Vol 5).
  DEF_FIELD(3, 0, arbiter_read_credits);

  static auto GetForPipe(intel_display::PipeId pipe_id) {
    // TODO(https://fxbug.dev/42060657): Accept pipe D, once we support it.
    ZX_ASSERT(pipe_id >= intel_display::PipeId::PIPE_A);
    ZX_ASSERT(pipe_id <= intel_display::PipeId::PIPE_C);
    const int pipe_index = pipe_id - intel_display::PipeId::PIPE_A;
    return hwreg::RegisterAddr<MbusPipeDataBoxControl>(0x7003c + 0x1000 * pipe_index);
  }
};

// MBUS_UBOX_CTL (MBus UBox Control)
//
// Does not exist on Kaby Lake or Skylake, which don't have MBus.
//
// All reserved bits in this register are MBZ (must be zero). So, the register
// can be safely updated without reading it first.
//
// Tiger Lake: IHD-OS-TGL-Vol 2c-1.22-Rev2.0 Part 2 pages 14-15
// DG1: IHD-OS-DG1-Vol 2c-2.21 Part 2 pages 14-15
class MbusUtilityBoxControl : public hwreg::RegisterBase<MbusUtilityBoxControl, uint32_t> {
 public:
  // Read-only status bit.
  DEF_BIT(31, enabled);

  // Read-only address of the box in the ring.
  DEF_FIELD(30, 27, ring_stop_address);

  DEF_RSVDZ_FIELD(26, 25);

  // Ignored if `back_to_back_transactions_regulation_enabled` is false.
  //
  // Used in conjunction with `back_to_back_transaction_delay` to limit the
  // number of back-to-back transactions sent from this box.
  //
  // Zero is not a valid value.
  DEF_FIELD(24, 20, max_back_to_back_transactions);

  // Number of wait cycles after `max_back_to_back_transactions` is hit.
  DEF_FIELD(19, 17, back_to_back_transaction_delay);

  // If true, B2B (back-to-back) transaction regulation fields are in effect.
  DEF_BIT(16, back_to_back_transactions_regulation_enabled);

  DEF_RSVDZ_FIELD(15, 7);

  // A credits used by KVM to make requests to the Arbiter.
  //
  // These credits are used by the KVM functionality in the CSME (Converged
  // Security and Management Engine).
  DEF_FIELD(6, 4, arbiter_read_credits);

  DEF_RSVDZ_BIT(3);

  // B credits used by VGA to read from DBUF (Display Buffer).
  DEF_FIELD(2, 0, display_buffer_read_credits);

  static auto Get() { return hwreg::RegisterAddr<MbusUtilityBoxControl>(0x4503c); }
};

// PIPE_ARB_CTL (Pipe Arbiter Control)
//
// This register does not exist on Kaby Lake or Skylake.
//
// This register has bits that are reserved but not MBZ (must be zero). So, it
// can only be safely updated via read-modify-write operations.
//
// Tiger Lake: IHD-OS-TGL-Vol 2c-1.22-Rev2.0 Part 2 pages 669-670
// DG1: IHD-OS-DG1-Vol 2c-2.21 Part 2 pages 669-670
class PipeArbiterControl : public hwreg::RegisterBase<PipeArbiterControl, uint32_t> {
 public:
  DEF_RSVDZ_FIELD(31, 21);

  // If true, weighted pipe slice arbitration is disabled.
  DEF_BIT(20, weighted_arbitration_disabled);

  // Number of additional slots added to an arbitration cycle.
  //
  // During these additional slots, clients are served using round-robin.
  DEF_FIELD(18, 16, arbitration_cycle_additional_slots);

  DEF_RSVDZ_FIELD(15, 14);

  // If true, arbitration configuration in `PlaneControl` (PLANE_CTL) is used.
  //
  // This field enables the use of the
  // `pipe_slice_request_arbitration_slot_count_tiger_lake` field.
  // If this field is false, hardware defaults are used instead.
  DEF_BIT(13, plane_control_slot_config_enabled);

  // If true, the pipe arbiter does not check for block validity.
  DEF_BIT(12, block_validity_check_disabled);

  // The DSB (Display Stream Buffer) engine service interval, in clock cycles.
  //
  // This field has a non-trivial encoding.
  DEF_FIELD(11, 10, display_stream_buffer_arbitration_interval_bits);

  // Number of DBUF (Display Buffer) requests serviced per streamer request.
  //
  // This field has a non-trivial encoding. It should be used via the
  // `display_buffer_requests_per_streamer_request()` and
  // `set_display_buffer_requests_per_streamer_request()` helpers.
  DEF_FIELD(9, 8, display_buffer_requests_per_streamer_request_bits);

  DEF_RSVDZ_FIELD(7, 6);

  // Number of microseconds the pipe waits after frame start to drain DBUF data.
  DEF_FIELD(5, 0, display_buffer_drain_delay_after_frame_start_us);

  // The DSB (Display Stream Buffer) engine service interval, in clock cycles.
  //
  // This field has a non-trivial encoding.
  int32_t display_stream_buffer_arbitration_interval() const {
    // The shift will not overflow (causing UB) because the underlying field is
    // a 2-bit integer.
    return int32_t{16} << display_stream_buffer_arbitration_interval_bits();
  }

  // See `display_stream_buffer_arbitration_interval()` for details.
  PipeArbiterControl& set_display_stream_buffer_arbitration_interval(int32_t clocks) {
    ZX_ASSERT(clocks >= 16);
    ZX_ASSERT(clocks <= 128);
    ZX_ASSERT((clocks & (clocks - 1)) == 0);

    const uint32_t raw_clocks = std::countr_zero(static_cast<uint32_t>(clocks >> 4));
    return set_display_stream_buffer_arbitration_interval_bits(raw_clocks);
  }

  // Number of DBUF (Display Buffer) requests serviced per streamer request.
  //
  // The register's documentation uses the DDB acronym (Display Data Buffer)
  // instead of DBUF. IHD-OS-TGL-Vol 12-1.22-Rev2.0 section "Maximum Data Buffer
  // Bandwidth" on page 167 contains "display data buffer (DBUF)", which hints
  // that DBUF and DDB are synonyms. The hint is confirmed by the intel-display
  // driver code, which uses both "ddb" and "dbuf" in DBUF-related identifiers.
  //
  // The "streamer" referenced here is most likely the plane's display streamer,
  // which issues memory requests when the plane's free DBUF space exceeds its
  // watermark. This streamer is mentioned as the "display streamer" or "overlay
  // streamer" in older documentation, such as the "FW1 - Display FIFO Watermark
  // Control 1" section in IHD-OS-VLV-Vol10-04.14 (the Display PRM for
  // ValleyView graphics). Older documentation had specific names ("display",
  // "overlay" / "sprite", "cursor") for planes, used the term "display RAM"
  // instead of DBUF, and the term "display FIFO" to refer to a plane's DBUF
  // allocation. The "display FIFO" concept is briefly covered in the "DSPARB -
  // Display Arbitration Control" section in IHD-OS-VLV-Vol10-04.14. These terms
  // still appear in current documentation, and in Intel's driver code.
  int32_t display_buffer_requests_per_streamer_request() const {
    // The shift will not overflow (causing UB) because the underlying field is
    // a 2-bit integer.
    return int32_t{2} << display_buffer_requests_per_streamer_request_bits();
  }

  // See `display_buffer_requests_per_streamer_request()` for details.
  PipeArbiterControl& set_display_buffer_requests_per_streamer_request(
      int32_t display_buffer_requests) {
    ZX_ASSERT(display_buffer_requests >= 2);
    ZX_ASSERT(display_buffer_requests <= 16);
    ZX_ASSERT((display_buffer_requests & (display_buffer_requests - 1)) == 0);

    const uint32_t raw_display_buffer_requests =
        std::countr_zero(static_cast<uint32_t>(display_buffer_requests >> 1));
    return set_display_buffer_requests_per_streamer_request_bits(raw_display_buffer_requests);
  }

  static auto GetForPipe(intel_display::PipeId pipe_id) {
    // TODO(https://fxbug.dev/42060657): Accept pipe D, once we support it.
    ZX_ASSERT(pipe_id >= intel_display::PipeId::PIPE_A);
    ZX_ASSERT(pipe_id <= intel_display::PipeId::PIPE_C);
    const int pipe_index = pipe_id - intel_display::PipeId::PIPE_A;
    return hwreg::RegisterAddr<PipeArbiterControl>(0x70028 + 0x1000 * pipe_index);
  }
};

}  // namespace registers

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_INTEL_DISPLAY_REGISTERS_ARBITER_H_
