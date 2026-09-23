// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "port.h"

#include <inttypes.h>
#include <lib/driver/component/cpp/driver_base.h>
#include <lib/io-buffer/phys-iter.h>
#include <lib/zx/clock.h>
#include <unistd.h>

#include <algorithm>
#include <mutex>

#include "controller.h"

#define AHCI_PAGE_MASK (AHCI_PAGE_SIZE - 1ull)

namespace ahci {

constexpr uint32_t hi32(uint64_t val) { return static_cast<uint32_t>(val >> 32); }
constexpr uint32_t lo32(uint64_t val) { return static_cast<uint32_t>(val); }

// Calculate the physical base of a virtual address.
static zx_paddr_t vtop(zx_paddr_t phys_base, void* virt_base, void* virt_addr) {
  uintptr_t addr = reinterpret_cast<uintptr_t>(virt_addr);
  uintptr_t base = reinterpret_cast<uintptr_t>(virt_base);
  return phys_base + (addr - base);
}

static bool IsNcqCommand(uint8_t cmd) {
  return (cmd == SATA_CMD_READ_FPDMA_QUEUED) || (cmd == SATA_CMD_WRITE_FPDMA_QUEUED);
}

Port::Port() { list_initialize(&txn_list_); }

Port::~Port() { ZX_DEBUG_ASSERT(list_is_empty(&txn_list_)); }

uint32_t Port::RegRead(size_t offset) {
  uint32_t val = 0;
  bus_->RegRead(reg_base_ + offset, &val);
  return val;
}

void Port::RegWrite(size_t offset, uint32_t val) { bus_->RegWrite(reg_base_ + offset, val); }

bool Port::SlotBusyLocked(uint32_t slot) {
  // a command slot is busy if a transaction is in flight or pending to be completed
  return ((RegRead(kPortSataActive) | RegRead(kPortCommandIssue)) & (1u << slot)) ||
         (commands_[slot] != nullptr) || (running_ & (1u << slot)) || (completed_ & (1u << slot));
}

zx_status_t Port::Configure(uint32_t num, Bus* bus, size_t reg_base, uint32_t max_command_tag) {
  std::lock_guard<std::mutex> lock(lock_);
  num_ = num;
  max_command_tag_ = max_command_tag;
  bus_ = bus;
  reg_base_ = reg_base + (num * sizeof(ahci_port_reg_t));
  port_implemented_ = true;
  device_present_ = false;
  paused_cmd_issuing_ = false;
  uint32_t cmd = RegRead(kPortCommand);
  if (cmd & (AHCI_PORT_CMD_ST | AHCI_PORT_CMD_FRE | AHCI_PORT_CMD_CR | AHCI_PORT_CMD_FR)) {
    fdf::error("port {}: port busy", num_);
    return ZX_ERR_UNAVAILABLE;
  }

  // Allocate memory for the command list, FIS receive area, command table and PRDT.
  zx_paddr_t phys_base;
  void* virt_base;
  zx_status_t status =
      bus_->DmaBufferInit(&buffer_, sizeof(ahci_port_mem_t), &phys_base, &virt_base);
  if (status != ZX_OK) {
    fdf::error("port {}: error allocating dma memory: {}", num_, zx_status_get_string(status));
    return status;
  }
  mem_ = static_cast<ahci_port_mem_t*>(virt_base);

  // clear memory area
  // order is command list (1024-byte aligned)
  //          FIS receive area (256-byte aligned)
  //          command table + PRDT (128-byte aligned)
  memset(mem_, 0, sizeof(*mem_));

  // command list.
  zx_paddr_t paddr = vtop(phys_base, mem_, &mem_->cl);
  RegWrite(kPortCommandListBase, lo32(paddr));
  RegWrite(kPortCommandListBaseUpper, hi32(paddr));

  // FIS receive area.
  paddr = vtop(phys_base, mem_, &mem_->fis);
  RegWrite(kPortFISBase, lo32(paddr));
  RegWrite(kPortFISBaseUpper, hi32(paddr));

  // command table, followed by PRDT.
  for (int i = 0; i < AHCI_MAX_COMMANDS; i++) {
    paddr = vtop(phys_base, mem_, &mem_->tab[i].ct);
    mem_->cl[i].ctba = lo32(paddr);
    mem_->cl[i].ctbau = hi32(paddr);
  }

  // clear port interrupts
  RegWrite(kPortInterruptStatus, RegRead(kPortInterruptStatus));

  // clear error
  RegWrite(kPortSataError, RegRead(kPortSataError));

  // spin up
  cmd |= AHCI_PORT_CMD_SUD;
  RegWrite(kPortCommand, cmd);

  // activate link
  cmd &= ~AHCI_PORT_CMD_ICC_MASK;
  cmd |= AHCI_PORT_CMD_ICC_ACTIVE;
  RegWrite(kPortCommand, cmd);

  // enable FIS receive
  cmd |= AHCI_PORT_CMD_FRE;
  RegWrite(kPortCommand, cmd);

  return ZX_OK;
}

zx_status_t Port::Enable() {
  uint32_t cmd = RegRead(kPortCommand);
  if (cmd & AHCI_PORT_CMD_ST)
    return ZX_OK;
  if (!(cmd & AHCI_PORT_CMD_FRE)) {
    fdf::error("port {}: cannot enable port without FRE enabled", num_);
    return ZX_ERR_BAD_STATE;
  }
  zx_status_t status =
      bus_->WaitForClear(reg_base_ + kPortCommand, AHCI_PORT_CMD_CR, zx::msec(500));
  if (status) {
    fdf::error("port {}: dma engine still running when enabling port", num_);
    return ZX_ERR_BAD_STATE;
  }
  cmd |= AHCI_PORT_CMD_ST;
  RegWrite(kPortCommand, cmd);
  return ZX_OK;
}

void Port::Disable() {
  bool unpin_pmts = true;
  uint32_t cmd = RegRead(kPortCommand);
  if (cmd & AHCI_PORT_CMD_ST) {
    cmd &= ~AHCI_PORT_CMD_ST;
    RegWrite(kPortCommand, cmd);
    zx_status_t status =
        bus_->WaitForClear(reg_base_ + kPortCommand, AHCI_PORT_CMD_CR, zx::msec(500));
    if (status) {
      fdf::error("port {}: port disable timed out", num_);
      unpin_pmts = false;
    }
  }
  CancelAll(ZX_ERR_CANCELED, unpin_pmts);
}

void Port::Reset() {
  // disable port
  Disable();

  // clear error
  RegWrite(kPortSataError, RegRead(kPortSataError));

  // wait for device idle
  zx_status_t status = bus_->WaitForClear(
      reg_base_ + kPortTaskFileData, AHCI_PORT_TFD_BUSY | AHCI_PORT_TFD_DATA_REQUEST, zx::sec(1));
  if (status != ZX_OK) {
    // if busy is not cleared, do a full comreset
    fdf::trace("port {}: timed out waiting for port idle, resetting", num_);
    // v1.3.1, 10.4.2 port reset
    uint32_t sctl =
        AHCI_PORT_SCTL_IPM_ACTIVE | AHCI_PORT_SCTL_IPM_PARTIAL | AHCI_PORT_SCTL_DET_INIT;
    RegWrite(kPortSataControl, sctl);
    usleep(1000);
    sctl = RegRead(kPortSataControl);
    sctl &= ~AHCI_PORT_SCTL_DET_MASK;
    RegWrite(kPortSataControl, sctl);
  }

  // enable port
  Enable();

  // wait for device detect
  status = bus_->WaitForSet(reg_base_ + kPortSataStatus, AHCI_PORT_SSTS_DET_PRESENT, zx::sec(1));
  if (status < 0) {
    fdf::trace("port {}: no device detected", num_);
  }

  // clear error
  RegWrite(kPortSataError, RegRead(kPortSataError));
}

void Port::SetDevInfo(const SataDeviceInfo* devinfo) {
  std::lock_guard<std::mutex> lock(lock_);
  memcpy(&devinfo_, devinfo, sizeof(devinfo_));
}

zx_status_t Port::Queue(SataTransaction* txn) {
  std::lock_guard<std::mutex> lock(lock_);
  if (!is_valid()) {
    return ZX_ERR_BAD_STATE;
  }

  // reset the physical address
  txn->pmt = ZX_HANDLE_INVALID;

  // put the cmd on the queue
  list_add_tail(&txn_list_, &txn->node);

  return ZX_OK;
}

bool Port::Complete() {
  SataTransaction* txn_complete[AHCI_MAX_COMMANDS];
  size_t complete_count = 0;
  bool active_txns = false;

  {
    std::lock_guard<std::mutex> lock(lock_);

    if (!is_valid()) {
      return false;
    }

    for (uint32_t slot = 0; slot < AHCI_MAX_COMMANDS; slot++) {
      SataTransaction* txn = commands_[slot];
      if (txn == nullptr) {
        continue;  // No transaction in this slot.
      }
      uint32_t slot_bit = (1u << slot);
      if ((completed_ & slot_bit) == 0) {
        // Not complete, check if timeout expired.
        zx::time now = zx::clock::get_monotonic();
        if (txn->timeout > now) {
          active_txns = true;
          continue;  // Still in progress.
        }
        // Timed out.
        zx::duration delta = now - txn->timeout;
        fdf::error("port {}: timed out txn {} ({} ms)", num_, static_cast<const void*>(txn),
                   delta.to_msecs());
        txn->timeout = zx::time::infinite_past();  // Signal that timeout occurred.
      }
      // Completed or timed out.
      commands_[slot] = nullptr;
      running_ &= ~slot_bit;
      completed_ &= ~slot_bit;
      txn_complete[complete_count] = txn;
      complete_count++;
    }
    if (!running_) {
      paused_cmd_issuing_ = false;
    }
  }

  for (size_t i = 0; i < complete_count; i++) {
    SataTransaction* txn = txn_complete[i];
    if (txn->pmt != ZX_HANDLE_INVALID) {
      zx_pmt_unpin(txn->pmt);
    }
    if (txn->timeout == zx::time::infinite_past()) {
      txn->Complete(ZX_ERR_TIMED_OUT);
    } else {
      fdf::trace("port {}: complete txn {}", num_, static_cast<const void*>(txn));
      txn->Complete(ZX_OK);
    }
  }

  return active_txns;
}

void Port::CancelAll(zx_status_t status, bool unpin_pmts) {
  SataTransaction* txn_complete[AHCI_MAX_COMMANDS * 2];
  size_t complete_count = 0;

  {
    std::lock_guard<std::mutex> lock(lock_);

    for (uint32_t slot = 0; slot < AHCI_MAX_COMMANDS; slot++) {
      SataTransaction* txn = commands_[slot];
      if (txn != nullptr) {
        commands_[slot] = nullptr;
        uint32_t slot_bit = (1u << slot);
        running_ &= ~slot_bit;
        completed_ &= ~slot_bit;
        txn_complete[complete_count++] = txn;
      }
    }

    SataTransaction* txn;
    while ((txn = list_peek_head_type(&txn_list_, SataTransaction, node)) != nullptr) {
      list_delete(&txn->node);
      txn_complete[complete_count++] = txn;
    }

    if (!running_) {
      paused_cmd_issuing_ = false;
    }
  }

  for (size_t i = 0; i < complete_count; i++) {
    SataTransaction* txn = txn_complete[i];
    if (txn->pmt != ZX_HANDLE_INVALID) {
      if (unpin_pmts) {
        zx_pmt_unpin(txn->pmt);
      } else {
        fdf::warn("port {}: leaking PMT due to port disable timeout", num_);
      }
      txn->pmt = ZX_HANDLE_INVALID;
    }
    txn->Complete(status);
  }
}

bool Port::ProcessQueued() {
  lock_.lock();
  if (!is_valid()) {
    lock_.unlock();
    return false;
  }

  bool added_txns = false;
  // If (running_ && paused_cmd_issuing_), commands that have been issued to the device are still
  // running, and we wait for them to complete.
  while (!(running_ && paused_cmd_issuing_)) {
    SataTransaction* txn = list_peek_head_type(&txn_list_, SataTransaction, node);
    if (!txn) {
      break;  // No queued commands to process.
    }

    // Native Command Queuing (NCQ) commands cannot run concurrently on the device with non-NCQ
    // commands. To issue a non-NCQ command, wait for active commands on the device to complete.
    // Furthermore, after issuing a non-NCQ command, pause further command issuing to prevent NCQ
    // commands from running simultaneously on the device.
    paused_cmd_issuing_ = !IsNcqCommand(txn->cmd);
    if (running_ && paused_cmd_issuing_) {
      // Unable to issue non-NCQ command now due to commands currently running on the device.
      break;
    }

    uint32_t slot;
    if (paused_cmd_issuing_) {
      slot = 0;
      ZX_DEBUG_ASSERT_MSG(!SlotBusyLocked(slot), "Command slot 0 is busy for non-NCQ command.");
    } else {
      // find a free command tag
      uint32_t max = std::min(devinfo_.max_cmd, max_command_tag_);
      for (slot = 0; slot <= max; slot++) {
        if (!SlotBusyLocked(slot))
          break;
      }
      if (slot > max) {
        break;  // No command slots available on the device.
      }
    }

    list_delete(&txn->node);

    // Issue the command to the device.
    zx_status_t st = TxnBeginLocked(slot, txn);
    // complete the transaction if it failed during processing
    if (st == ZX_OK) {
      running_ |= (1u << slot);
      commands_[slot] = txn;
    } else {
      lock_.unlock();
      txn->Complete(st);
      lock_.lock();
      continue;
    }
    added_txns = true;
  }
  lock_.unlock();
  return added_txns;
}

void Port::TxnComplete(zx_status_t status) {
  std::lock_guard<std::mutex> lock(lock_);
  uint32_t active = RegRead(kPortSataActive);  // Transactions active in hardware.
  uint32_t running = running_;                 // Transactions tagged as running.
  // Transactions active in hardware but not tagged as running.
  uint32_t unaccounted = active & ~running;
  // Remove transactions that have been completed by the watchdog.
  unaccounted &= ~completed_;
  // assert if a command slot without an outstanding transaction is active.
  ZX_DEBUG_ASSERT(unaccounted == 0);

  // Transactions tagged as running but completed by hardware.
  uint32_t done = running & ~active;
  completed_ |= done;
}

zx_status_t Port::TxnBeginLocked(uint32_t slot, SataTransaction* txn) {
  ZX_DEBUG_ASSERT(slot < AHCI_MAX_COMMANDS);
  ZX_DEBUG_ASSERT(!SlotBusyLocked(slot));

  uint64_t offset_vmo = 0;
  uint64_t bytes = 0;
  uint64_t lba = 0;
  uint64_t count = 0;
  bool is_write = false;
  bool is_flush = false;

  if (txn->operation.tag == block_server::Operation::Tag::Flush) {
    is_flush = true;
  } else if (txn->operation.tag == block_server::Operation::Tag::Read) {
    is_write = false;
    offset_vmo = txn->operation.read.vmo_offset;
    bytes = txn->operation.read.block_count * devinfo_.block_size;
    lba = txn->operation.read.device_block_offset;
    count = txn->operation.read.block_count;
  } else if (txn->operation.tag == block_server::Operation::Tag::Write) {
    is_write = true;
    offset_vmo = txn->operation.write.vmo_offset;
    bytes = txn->operation.write.block_count * devinfo_.block_size;
    lba = txn->operation.write.device_block_offset;
    count = txn->operation.write.block_count;
  } else {
    fdf::error("Unsupported operation tag: {}", static_cast<uint32_t>(txn->operation.tag));
    return ZX_ERR_NOT_SUPPORTED;
  }

  size_t pagecount =
      ((offset_vmo & AHCI_PAGE_MASK) + bytes + (AHCI_PAGE_SIZE - 1)) / AHCI_PAGE_SIZE;
  zx_paddr_t pages[AHCI_MAX_PAGES];
  if (pagecount > AHCI_MAX_PAGES) {
    fdf::trace("port {}: txn {} too many pages ({})", num_, static_cast<const void*>(txn),
               pagecount);
    return ZX_ERR_INVALID_ARGS;
  }

  if (!is_flush) {
    zx::unowned_vmo vmo(txn->vmo->get());
    uint32_t options = is_write ? ZX_BTI_PERM_READ : ZX_BTI_PERM_WRITE;
    zx::pmt pmt;
    zx_status_t st = bus_->BtiPin(options, vmo, offset_vmo & ~AHCI_PAGE_MASK,
                                  pagecount * AHCI_PAGE_SIZE, pages, pagecount, &pmt);
    if (st != ZX_OK) {
      fdf::trace("port {}: failed to pin pages, err = {}", num_, zx_status_get_string(st));
      return st;
    }
    txn->pmt = pmt.release();
  }

  phys_iter_buffer_t physbuf = {};
  physbuf.phys = pages;
  physbuf.phys_count = pagecount;
  physbuf.length = bytes;
  physbuf.vmo_offset = offset_vmo;

  phys_iter_t iter;
  phys_iter_init(&iter, &physbuf, AHCI_PRD_MAX_SIZE);

  uint8_t cmd = txn->cmd;
  uint8_t device = txn->device;

  // build the command
  ahci_cl_t* cl = &mem_->cl[slot];
  // don't clear the cl since we set up ctba/ctbau at init
  cl->prdtl_flags_cfl = 0;
  cl->cfl = 5;  // 20 bytes
  cl->w = is_write ? 1 : 0;
  cl->prdbc = 0;
  memset(&mem_->tab[slot].ct, 0, sizeof(ahci_ct_t));

  uint8_t* cfis = mem_->tab[slot].ct.cfis;
  cfis[0] = 0x27;  // host-to-device
  cfis[1] = 0x80;  // command
  cfis[2] = cmd;
  cfis[7] = device;

  // some commands have lba/count fields
  if (cmd == SATA_CMD_READ_DMA_EXT || cmd == SATA_CMD_WRITE_DMA_EXT) {
    cfis[4] = lba & 0xff;
    cfis[5] = (lba >> 8) & 0xff;
    cfis[6] = (lba >> 16) & 0xff;
    cfis[8] = (lba >> 24) & 0xff;
    cfis[9] = (lba >> 32) & 0xff;
    cfis[10] = (lba >> 40) & 0xff;
    cfis[12] = count & 0xff;
    cfis[13] = (count >> 8) & 0xff;
  } else if (IsNcqCommand(cmd)) {
    cfis[4] = lba & 0xff;
    cfis[5] = (lba >> 8) & 0xff;
    cfis[6] = (lba >> 16) & 0xff;
    cfis[8] = (lba >> 24) & 0xff;
    cfis[9] = (lba >> 32) & 0xff;
    cfis[10] = (lba >> 40) & 0xff;
    cfis[3] = count & 0xff;
    cfis[11] = (count >> 8) & 0xff;
    cfis[12] = (slot << 3) & 0xff;  // tag
    cfis[13] = 0;                   // normal priority
  }

  cl->prdtl = 0;
  size_t length;
  zx_paddr_t paddr;
  for (uint32_t i = 0; i < AHCI_MAX_PRDS; i++) {
    length = phys_iter_next(&iter, &paddr);
    if (length == 0) {
      break;
    } else if (length > AHCI_PRD_MAX_SIZE) {
      fdf::error("port {}: chunk size > {} is unsupported", num_, length);
      return ZX_ERR_NOT_SUPPORTED;
    } else if (cl->prdtl == AHCI_MAX_PRDS) {
      fdf::error("port {}: txn with more than {} chunks is unsupported", num_, cl->prdtl);
      return ZX_ERR_NOT_SUPPORTED;
    }

    ahci_prd_t* prd = &mem_->tab[slot].prd[i];
    prd->dba = lo32(paddr);
    prd->dbau = hi32(paddr);
    prd->dbc = ((length - 1) & (AHCI_PRD_MAX_SIZE - 1));  // 0-based byte count
    cl->prdtl++;
  }

  fdf::trace("port {}: do_txn txn {} ({}) offset 0x{:x} length 0x{:x} slot {} prdtl {}\n", num_,
             static_cast<const void*>(txn), cl->w ? 'w' : 'r', lba, count, slot, cl->prdtl);
  if (fdf::Logger::GlobalInstance()->GetSeverity() <= FUCHSIA_LOG_TRACE) {
    for (uint32_t i = 0; i < cl->prdtl; i++) {
      ahci_prd_t* prd = &mem_->tab[slot].prd[i];
      fdf::trace("{:04}: dbau=0x{:08x} dba=0x{:08x} dbc=0x{:x}", i, prd->dbau, prd->dba, prd->dbc);
    }
  }

  // start command
  if (IsNcqCommand(cmd)) {
    RegWrite(kPortSataActive, (1u << slot));
  }
  RegWrite(kPortCommandIssue, (1u << slot));

  txn->timeout = zx::clock::get_monotonic() + kTransactionTimeout;
  return ZX_OK;
}

// HandleIrq does not lock the port or check whether it is valid.
// It is assumed that an invalid port can not have scheduled operations that trigger an interrupt.
// The port may have been disabled or unplugged, but is still valid.
bool Port::HandleIrq() {
  // Clear interrupt status.
  uint32_t int_status = RegRead(kPortInterruptStatus);
  RegWrite(kPortInterruptStatus, int_status);

  if (int_status & AHCI_PORT_INT_PRC) {  // PhyRdy change
    uint32_t serr = RegRead(kPortSataError);
    RegWrite(kPortSataError, serr & ~0x1);
  }
  if (int_status & AHCI_PORT_INT_ERROR) {  // error
    fdf::error("port {}: error is=0x{:08x}", num_, int_status);
    TxnComplete(ZX_ERR_INTERNAL);
    return true;
  } else if (int_status) {
    TxnComplete(ZX_OK);
    return true;
  }
  return false;
}

// Set up the running state for testing Complete()
void Port::TestSetRunning(SataTransaction* txn, uint32_t slot) {
  ZX_DEBUG_ASSERT(slot < AHCI_MAX_COMMANDS);
  commands_[slot] = txn;
  running_ |= (1u << slot);
  completed_ &= ~(1u << slot);
}

}  // namespace ahci
