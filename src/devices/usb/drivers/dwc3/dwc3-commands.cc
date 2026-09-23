// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/trace/event.h>

#include "src/devices/usb/drivers/dwc3/dwc3-regs.h"
#include "src/devices/usb/drivers/dwc3/dwc3.h"

namespace dwc3 {

static constexpr uint32_t kEarlyLoopExitCount = 100000;

// Spin wait for a command to complete with an early exit if stuck.
void Dwc3::WaitForCmdAct(const char* caller_name, const uint8_t ep_num) {
  TRACE_DURATION("dwc3", "Dwc3::WaitForCmdAct", "caller", caller_name);
  auto* mmio = get_mmio();
  uint32_t loop_count = 0;

  while (true) {
    loop_count++;
    if (loop_count >= kEarlyLoopExitCount) {
      fdf::warn("Dwc3::WaitForCmdAct() Forced exit from spin loop for {:s} for ep{}", caller_name,
                ep_num);
      break;
    }
    if (!DEPCMD::Get(ep_num).ReadFrom(mmio).CMDACT()) {
      break;
    }
  }
}

void Dwc3::CmdStartNewConfig(const Endpoint& ep, uint32_t rsrc_id_base) {
  TRACE_DURATION("dwc3", "Dwc3::CmdStartNewConfig", "ep_num", ep.ep_num, "rsrc_id_base",
                 rsrc_id_base);

  // The Start New Configuration specification expects this function to
  // only be called with '0', when setting up EP0 and EP1. After these endpoints
  // are configured and Start Configuration is called, we expect this function
  // to be called with '2', for setting up EP2 and above.
  ZX_DEBUG_ASSERT_MSG(rsrc_id_base == 0 || rsrc_id_base == 2, "%s: rsrc_id_base = %u != {0, 2}",
                      __func__, rsrc_id_base);

  auto* mmio = get_mmio();
  const uint8_t ep_num = ep.ep_num;

  DEPCMDPAR0::Get(ep_num).FromValue(0).WriteTo(mmio);
  DEPCMDPAR1::Get(ep_num).FromValue(0).WriteTo(mmio);
  DEPCMDPAR2::Get(ep_num).FromValue(0).WriteTo(mmio);
  DEPCMD::Get(ep_num)
      .FromValue(0)
      .set_CMDTYP(DEPCMD::DEPSTARTCFG)
      .set_COMMANDPARAM(rsrc_id_base)
      .set_CMDACT(1)
      .WriteTo(mmio);

  WaitForCmdAct(__func__, ep_num);
}

void Dwc3::CmdEpSetConfig(const Endpoint& ep, bool modify) {
  TRACE_DURATION("dwc3", "Dwc3::CmdEpSetConfig", "ep_num", ep.ep_num, "modify", modify);
  auto* mmio = get_mmio();
  const uint8_t ep_num = ep.ep_num;

  // fifo number is zero for OUT endpoints and EP0_IN
  const uint32_t fifo_num = (ep.IsOutput() || (ep_num == kEp0In)) ? 0 : ep_num >> 1;
  const uint32_t action =
      modify ? DEPCFG_DEPCMDPAR0::ACTION_MODIFY : DEPCFG_DEPCMDPAR0::ACTION_INITIALIZE;

  DEPCFG_DEPCMDPAR0::Get(ep_num)
      .FromValue(0)
      .set_FIFO_NUM(fifo_num)
      .set_MAX_PACKET_SIZE(ep.max_packet_size)
      .set_EP_TYPE(static_cast<uint32_t>(ep.type))
      .set_ACTION(action)
      .WriteTo(mmio);
  DEPCFG_DEPCMDPAR1::Get(ep_num)
      .FromValue(0)
      .set_EP_NUMBER(ep_num)
      .set_INTERVAL(ep.interval)
      .set_XFER_NOT_READY_EN(1)
      .set_XFER_COMPLETE_EN(1)
      // Only pertinent if we're allowing multiple TRBs per transfer.
      .set_XFER_IN_PROGRESS_EN(AllowEnqueueManyTRBs(ep.type))
      .set_INTR_NUM(0)
      .WriteTo(mmio);
  DEPCMDPAR2::Get(ep_num).FromValue(0).WriteTo(mmio);
  DEPCMD::Get(ep_num).FromValue(0).set_CMDTYP(DEPCMD::DEPCFG).set_CMDACT(1).WriteTo(mmio);

  WaitForCmdAct(__func__, ep_num);
}

void Dwc3::CmdEpTransferConfig(const Endpoint& ep) {
  TRACE_DURATION("dwc3", "Dwc3::CmdEpTransferConfig", "ep_num", ep.ep_num);
  auto* mmio = get_mmio();
  const uint8_t ep_num = ep.ep_num;

  DEPCMDPAR0::Get(ep_num).FromValue(0).set_PARAMETER(1).WriteTo(mmio);
  DEPCMDPAR1::Get(ep_num).FromValue(0).WriteTo(mmio);
  DEPCMDPAR2::Get(ep_num).FromValue(0).WriteTo(mmio);
  DEPCMD::Get(ep_num).FromValue(0).set_CMDTYP(DEPCMD::DEPXFERCFG).set_CMDACT(1).WriteTo(mmio);

  WaitForCmdAct(__func__, ep_num);
}

void Dwc3::CmdEpStartTransfer(const Endpoint& ep, zx_paddr_t trb_phys) {
  TRACE_DURATION("dwc3", "Dwc3::CmdEpStartTransfer", "ep_num", ep.ep_num, "trb_phys", trb_phys);
  auto* mmio = get_mmio();
  const uint8_t ep_num = ep.ep_num;

  // Per the Synopsys DWC3 Databook specification for the StartTransfer command,
  // the 64-bit TRB address must be mapped as follows:
  // - DEPCMDPAR0 (offset 0x08) receives the High Address (bits 63:32)
  // - DEPCMDPAR1 (offset 0x04) receives the Low Address (bits 31:0)
  // This matches little-endian layout where the lower 32-bits sit at the lower offset.
  DEPCMDPAR0::Get(ep_num)
      .FromValue(0)
      .set_PARAMETER(static_cast<uint32_t>(trb_phys >> 32))
      .WriteTo(mmio);
  DEPCMDPAR1::Get(ep_num).FromValue(0).set_PARAMETER(static_cast<uint32_t>(trb_phys)).WriteTo(mmio);
  DEPCMDPAR2::Get(ep_num).FromValue(0).WriteTo(mmio);
  DEPCMD::Get(ep_num)
      .FromValue(0)
      .set_CMDTYP(DEPCMD::DEPSTRTXFER)
      .set_CMDACT(1)
      .set_CMDIOC(1)
      .WriteTo(mmio);

  WaitForCmdAct(__func__, ep_num);
}

void Dwc3::CmdEpUpdateTransfer(const Endpoint& ep) {
  TRACE_DURATION("dwc3", "Dwc3::CmdEpUpdateTransfer", "ep_num", ep.ep_num);
  auto* mmio = get_mmio();
  const uint8_t ep_num = ep.ep_num;

  // From the DWC3 programming manual, section 3.2.26
  //
  //    Software may issue a special "No Response Update Transfer" command by
  //    setting CmdAct=0 and CmdIOC=0. In this case, the hardware does not
  //    generate a Command Complete event, does not set the CmdAct bit to '0'
  //    (because it will be '0'), and software may immediately issue another
  //    command to the same endpoint following this one.
  //
  // This command is used to "kick" the controller to fetch more TRBs but we
  // don't need to wait on it, allowing for much faster TRB enqueuing.
  DEPCMD::Get(ep_num)
      .FromValue(0)
      .set_CMDTYP(DEPCMD::DEPUPDXFER)
      .set_COMMANDPARAM(ep.rsrc_id)
      .set_CMDACT(0)
      .set_CMDIOC(0)
      .WriteTo(mmio);
}

void Dwc3::CmdEpEndTransfer(const Endpoint& ep) {
  TRACE_DURATION("dwc3", "Dwc3::CmdEpEndTransfer", "ep_num", ep.ep_num);
  if (!power_on_) {
    return;
  }

  auto* mmio = get_mmio();

  const uint8_t ep_num = ep.ep_num;
  const uint32_t rsrc_id = ep.rsrc_id;

  ZX_DEBUG_ASSERT_MSG(rsrc_id != Endpoint::kInvalidResourceId,
                      "%s: Called before rsrc_id was initialized with a valid value "
                      "ep.ep_num=%d ep.enabled=%d ep.type=%d ep.transfer_state=%d "
                      "ep.stalled=%d ep.rsrc_id=0x%08x",
                      __func__, ep_num, ep.enabled, static_cast<uint8_t>(ep.type),
                      ep.transfer_state, ep.stalled, rsrc_id);

  DEPCMDPAR0::Get(ep_num).FromValue(0).WriteTo(mmio);
  DEPCMDPAR1::Get(ep_num).FromValue(0).WriteTo(mmio);
  DEPCMDPAR2::Get(ep_num).FromValue(0).WriteTo(mmio);
  DEPCMD::Get(ep_num)
      .FromValue(0)
      .set_CMDTYP(DEPCMD::DEPENDXFER)
      .set_COMMANDPARAM(rsrc_id)
      .set_CMDACT(1)
      .set_CMDIOC(1)
      .set_HIPRI_FORCERM(1)
      .WriteTo(mmio);

  if (poll_end_xfer_) {
    WaitForCmdAct(__func__, ep_num);
  } else {
    // Rather than synchronize against a CommandComplete endpoint event, just give the core some
    // time to complete halting any DMA.
    zx::nanosleep(zx::deadline_after(zx::msec(1)));
  }
}

void Dwc3::CmdEpSetStall(const Endpoint& ep) {
  TRACE_DURATION("dwc3", "Dwc3::CmdEpSetStall", "ep_num", ep.ep_num);
  auto* mmio = get_mmio();

  const uint8_t ep_num = ep.ep_num;

  DEPCMDPAR0::Get(ep_num).FromValue(0).WriteTo(mmio);
  DEPCMDPAR1::Get(ep_num).FromValue(0).WriteTo(mmio);
  DEPCMDPAR2::Get(ep_num).FromValue(0).WriteTo(mmio);
  DEPCMD::Get(ep_num)
      .FromValue(0)
      .set_CMDTYP(DEPCMD::DEPSSTALL)
      .set_CMDACT(1)
      .set_CMDIOC(1)
      .WriteTo(mmio);

  WaitForCmdAct(__func__, ep_num);
}

void Dwc3::CmdEpClearStall(const Endpoint& ep) {
  TRACE_DURATION("dwc3", "Dwc3::CmdEpClearStall", "ep_num", ep.ep_num);
  auto* mmio = get_mmio();

  const uint8_t ep_num = ep.ep_num;

  DEPCMDPAR0::Get(ep_num).FromValue(0).WriteTo(mmio);
  DEPCMDPAR1::Get(ep_num).FromValue(0).WriteTo(mmio);
  DEPCMDPAR2::Get(ep_num).FromValue(0).WriteTo(mmio);
  DEPCMD::Get(ep_num)
      .FromValue(0)
      .set_CMDTYP(DEPCMD::DEPCSTALL)
      .set_CMDACT(1)
      .set_CMDIOC(1)
      .WriteTo(mmio);

  WaitForCmdAct(__func__, ep_num);
}

}  // namespace dwc3
