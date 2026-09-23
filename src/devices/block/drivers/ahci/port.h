// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BLOCK_DRIVERS_AHCI_PORT_H_
#define SRC_DEVICES_BLOCK_DRIVERS_AHCI_PORT_H_

#include <lib/dma-buffer/buffer.h>
#include <lib/driver/mmio/cpp/mmio-buffer.h>
#include <threads.h>
#include <zircon/types.h>

#include <mutex>

#include "ahci.h"
#include "bus.h"
#include "sata.h"

namespace ahci {

// Command table for a port.
struct ahci_command_tab_t {
  ahci_ct_t ct;
  ahci_prd_t prd[AHCI_MAX_PRDS];
} __attribute__((aligned(128)));

// Memory for port command lists is laid out in the order described by this struct.
struct ahci_port_mem_t {
  ahci_cl_t cl[AHCI_MAX_COMMANDS];            // 1024-byte aligned.
  ahci_fis_t fis;                             // 256-byte aligned.
  ahci_command_tab_t tab[AHCI_MAX_COMMANDS];  // 128-byte aligned.
};

static_assert(sizeof(ahci_port_mem_t) == 271616, "port memory layout size invalid");

class Controller;

class Port {
 public:
  static constexpr zx::duration kTransactionTimeout = zx::sec(5);

  Port();
  ~Port();

  DISALLOW_COPY_ASSIGN_AND_MOVE(Port);

  // Configure a port for use.
  zx_status_t Configure(uint32_t num, Bus* bus, size_t reg_base, uint32_t max_command_tag);

  uint32_t RegRead(size_t offset);
  void RegWrite(size_t offset, uint32_t val);

  zx_status_t Enable();
  void Disable();
  void Reset();

  void SetDevInfo(const SataDeviceInfo* devinfo);

  zx_status_t Queue(SataTransaction* txn);

  // Complete in-progress transactions.
  // Returns true if there remain transactions in progress.
  bool Complete();

  // Cancel all in-progress and queued transactions.
  void CancelAll(zx_status_t status, bool unpin_pmts);

  // Process incoming transaction queue and run them.
  // Returns true if transactions were added (are now in progress)
  bool ProcessQueued();

  // Returns true if a transaction was handled.
  bool HandleIrq();

  uint32_t num() const { return num_; }

  // These flag-access functions should require holding the port lock.
  // In their current use, they frequently access them unlocked. This
  // will be fixed and thread annotations will be added in future CLs.

  bool port_implemented() const { return port_implemented_; }

  bool device_present() const { return device_present_; }
  void set_device_present(bool device_present) { device_present_ = device_present; }

  bool is_valid() const { return port_implemented_ && device_present_; }

  bool paused_cmd_issuing() const { return paused_cmd_issuing_; }

  // Test functions

  // Mark transaction as running without going through the Queue path.
  // Does not modify bus registers.
  void TestSetRunning(SataTransaction* txn, uint32_t slot);

  // Peek at running transactions.
  const SataTransaction* TestGetRunning(uint32_t slot) const { return commands_[slot]; }
  SataTransaction* TestGetRunning(uint32_t slot) { return commands_[slot]; }

 private:
  bool SlotBusyLocked(uint32_t slot);
  zx_status_t TxnBeginLocked(uint32_t slot, SataTransaction* txn);
  void TxnComplete(zx_status_t status);

  uint32_t num_ = 0;  // 0-based
  // Pointer to controller's bus provider. Pointer is not owned.
  Bus* bus_ = nullptr;
  // Largest command tag (= maximum number of simultaneous commands minus 1).
  uint32_t max_command_tag_ = 0;

  std::mutex lock_;
  // Whether this port is implemented by the controller.
  bool port_implemented_ = false;
  // Whether a device is present on this port.
  bool device_present_ = false;
  // Whether this port is paused (not processing queued transactions) until commands that have been
  // issued to the device are complete.
  bool paused_cmd_issuing_ = false;
  list_node_t txn_list_{};
  uint32_t running_ = 0;    // bitmask of running commands
  uint32_t completed_ = 0;  // bitmask of completed commands

  std::unique_ptr<dma_buffer::ContiguousBuffer> buffer_;
  size_t reg_base_ = 0;
  ahci_port_mem_t* mem_ = nullptr;

  SataDeviceInfo devinfo_{};
  SataTransaction* commands_[AHCI_MAX_COMMANDS] = {};  // commands in flight
};

}  // namespace ahci

#endif  // SRC_DEVICES_BLOCK_DRIVERS_AHCI_PORT_H_
