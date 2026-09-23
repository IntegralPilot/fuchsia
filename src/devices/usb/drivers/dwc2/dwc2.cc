// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/usb/drivers/dwc2/dwc2.h"

#include <fidl/fuchsia.hardware.platform.device/cpp/fidl.h>
#include <fidl/fuchsia.hardware.usb.dci/cpp/fidl.h>
#include <fidl/fuchsia.hardware.usb.descriptor/cpp/fidl.h>
#include <fidl/fuchsia.hardware.usb.phy/cpp/fidl.h>
#include <lib/ddk/metadata.h>
#include <lib/dma-buffer/buffer.h>
#include <lib/driver/compat/cpp/metadata.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/fit/function.h>
#include <lib/zx/clock.h>
#include <lib/zx/profile.h>
#include <lib/zx/result.h>
#include <lib/zx/time.h>
#include <threads.h>
#include <zircon/status.h>
#include <zircon/syscalls-next.h>
#include <zircon/syscalls.h>
#include <zircon/threads.h>

#include <cstdlib>
#include <memory>
#include <mutex>
#include <span>

#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/designware/platform/cpp/bind.h>

#include "src/devices/usb/drivers/dwc2/usb_dwc_regs.h"

namespace dwc2 {

namespace fdci = fuchsia_hardware_usb_dci;
namespace fdescriptor = fuchsia_hardware_usb_descriptor;
namespace fpdev = fuchsia_hardware_platform_device;
namespace fphy = fuchsia_hardware_usb_phy;

void Dwc2::dump_regs() {
  const auto& mmio = *mmio_;

  DUMP_REG(GOTGCTL, mmio)
  DUMP_REG(GOTGINT, mmio)
  DUMP_REG(GAHBCFG, mmio)
  DUMP_REG(GUSBCFG, mmio)
  DUMP_REG(GRSTCTL, mmio)
  DUMP_REG(GINTSTS, mmio)
  DUMP_REG(GINTMSK, mmio)
  DUMP_REG(GRXSTSP, mmio)
  DUMP_REG(GRXFSIZ, mmio)
  DUMP_REG(GNPTXFSIZ, mmio)
  DUMP_REG(GNPTXSTS, mmio)
  DUMP_REG(GSNPSID, mmio)
  DUMP_REG(GHWCFG1, mmio)
  DUMP_REG(GHWCFG2, mmio)
  DUMP_REG(GHWCFG3, mmio)
  DUMP_REG(GHWCFG4, mmio)
  DUMP_REG(GDFIFOCFG, mmio)
  DUMP_REG(DCFG, mmio)
  DUMP_REG(DCTL, mmio)
  DUMP_REG(DSTS, mmio)
  DUMP_REG(DIEPMSK, mmio)
  DUMP_REG(DOEPMSK, mmio)
  DUMP_REG(DAINT, mmio)
  DUMP_REG(DAINTMSK, mmio)
  DUMP_REG(PCGCCTL, mmio)

  for (uint32_t i = 0; i < std::size(metadata_.tx_fifo_sizes()); i++) {
    DUMP_REG_W_IDX(DTXFSIZ, i + 1, mmio)
  }
  for (uint32_t i = 0; i < DWC_MAX_EPS; i++) {
    DUMP_REG_W_IDX(DEPCTL, i, mmio)
    DUMP_REG_W_IDX(DEPTSIZ, i, mmio)
    DUMP_REG_W_IDX(DEPDMA, i, mmio)
  }
  for (uint32_t i = 0; i < MAX_EPS_CHANNELS; i++) {
    DUMP_REG_W_IDX(DIEPINT, i, mmio)
  }
  for (uint32_t i = 0; i < MAX_EPS_CHANNELS; i++) {
    DUMP_REG_W_IDX(DOEPINT, i + DWC_EP_OUT_SHIFT, mmio)
  }
}

zx::result<> Dwc2::Start(fdf::DriverContext context) {
  config_ = context.take_config<dwc2_config::Config>();

  zx::result dispatcher = fdf::SynchronizedDispatcher::Create(
      fdf::SynchronizedDispatcher::Options::kAllowSyncCalls, "irq-dispatcher",
      fit::bind_member<&Dwc2::DispatcherShutdownHandler>(this),
      "fuchsia.devices.usb.drivers.dwc2.interrupt");
  if (dispatcher.is_error()) {
    fdf::error("could not create irq-dispatcher: {}", dispatcher);
    return dispatcher.take_error();
  }
  irq_dispatcher_ = std::move(*dispatcher);

  zx_status_t status = Init(context, config_);
  if (status != ZX_OK) {
    fdf::error("Init(): {}", zx_status_get_string(status));
    return zx::error(status);
  }

  auto thunk = [this]() { this->IrqThread(); };
  status = async::PostTask(irq_dispatcher_.async_dispatcher(), std::move(thunk));
  if (status != ZX_OK) {
    fdf::error("could not post IrqThread() task: {}", zx_status_get_string(status));
    return zx::error(status);
  }

  return zx::ok();
}

void Dwc2::Stop(fdf::StopCompleter completer) {
  irq_.destroy();
  irq_dispatcher_.ShutdownAsync();
  irq_thread_stopped_.Wait();
  completer(zx::ok());

  {
    std::lock_guard<std::mutex> guard(lock_);
    const zx::result result = ResetCore();
    ZX_ASSERT_MSG(
        result.is_ok(),
        "Failed to reset DWC2 core during Stop (%s), self terminating to avoid runaway DMA\n",
        result.status_string());
  }
}

zx::result<> Dwc2::ResetCore() {
  // "DesignWare Cores USB 2.0 Hi-Speed On-The-Go (OTG) v. 4.30a" Table 7-10's
  // description of GRSTCTL.CSftRst describes the reset sequence for silicon
  // revisions >= 4.20a.  It says:
  //
  // ```
  // The application can write to this bit any time it wants to reset the core.
  // The application must clear this bit after checking the bit 29 of this
  // register (Core Soft Reset Done). Software must also must check that bit 31
  // of this register is 1 (AHB Master is IDLE) before starting any operation.
  // ```
  //
  // Additionally, details in GRSTCTL.CSftRstDone say:
  //
  // ```
  // The core sets this bit when all the necessary logic is reset in the
  // core.  This bit is cleared by the application along with GRSTCTL.CSftRst
  // (bit 0)
  // ```
  //
  // Prior to 4.20a, instead of waiting for CSftRstDone becoming asserted, we
  // are supposed to wait for the HW to clear CSftRst for us instead.
  //
  // Either way, afterwards we need to wait for AHBIdle to be asserted.
  //

  // Set the main reset bit.
  GRSTCTL::Get().ReadFrom(get_mmio()).set_csftrst(1).WriteTo(get_mmio());

  if (cached_gsnpsid_.version() < 0x420a) {
    // Old Silicon, wait for CSftRst to be cleared.
    if (WaitForRegisterPredicate(GRSTCTL::Get(), [](GRSTCTL reg) { return reg.csftrst() == 0; }) ==
        false) {
      return zx::error{ZX_ERR_TIMED_OUT};
    }
  } else {
    // New Silicon, wait for CSftRstDone to be set
    if (WaitForRegisterPredicate(GRSTCTL::Get(),
                                 [](GRSTCTL reg) { return reg.csftrstdone() == 1; }) == false) {
      return zx::error{ZX_ERR_TIMED_OUT};
    }

    // Now clear CSftReset (R/W) as well as CSftRstDone (R/W1C)
    GRSTCTL::Get().ReadFrom(get_mmio()).set_csftrst(0).set_csftrstdone(1).WriteTo(get_mmio());
  }

  // Wait for AHBIdle
  if (WaitForRegisterPredicate(GRSTCTL::Get(), [](GRSTCTL reg) { return reg.ahbidle() == 1; }) ==
      false) {
    return zx::error{ZX_ERR_TIMED_OUT};
  }

  return zx::ok();
}

void Dwc2::DispatcherShutdownHandler(fdf_dispatcher_t* dispatcher) { irq_thread_stopped_.Signal(); }

// Handler for usbreset interrupt.
void Dwc2::HandleReset() {
  auto* mmio = get_mmio();

  // TODO(b/355271738): Downgrade back to SERIAL when done debugging b/355271738.
  fdf::info("\nRESET");

  ep0_state_ = Ep0State::DISCONNECTED;
  configured_ = false;

  // Clear remote wakeup signalling
  DCTL::Get().ReadFrom(mmio).set_rmtwkupsig(0).WriteTo(mmio);

  for (uint32_t i = 0; i < MAX_EPS_CHANNELS; i++) {
    auto diepctl = DEPCTL::Get(i).ReadFrom(mmio);

    // Disable IN endpoints
    if (diepctl.epena()) {
      diepctl.set_snak(1);
      diepctl.set_epdis(1);
      diepctl.WriteTo(mmio);
    }

    // Clear snak on OUT endpoints
    DEPCTL::Get(i + DWC_EP_OUT_SHIFT).ReadFrom(mmio).set_snak(1).WriteTo(mmio);
  }

  {
    std::lock_guard<std::mutex> guard(lock_);
    // Flush endpoint zero TX FIFO
    FlushTxFifo(0);

    // Flush All other endpoint TX FIFOs.
    FlushTxFifo(0x10);

    // Flush the learning queue
    GRSTCTL::Get().FromValue(0).set_intknqflsh(1).WriteTo(mmio);
  }

  // Enable interrupts for only EPO IN and OUT
  DAINTMSK::Get().FromValue((1 << DWC_EP0_IN) | (1 << DWC_EP0_OUT)).WriteTo(mmio);

  // Enable various endpoint specific interrupts
  DOEPMSK::Get()
      .FromValue(0)
      .set_setup(1)
      .set_stsphsercvd(1)
      .set_xfercompl(1)
      .set_ahberr(1)
      .set_epdisabled(1)
      .WriteTo(mmio);
  DIEPMSK::Get()
      .FromValue(0)
      .set_xfercompl(1)
      .set_timeout(1)
      .set_ahberr(1)
      .set_epdisabled(1)
      .WriteTo(mmio);

  // Clear device address
  DCFG::Get().ReadFrom(mmio).set_devaddr(0).WriteTo(mmio);

  SetConnected(false);
}

// Handler for usbsuspend interrupt.
void Dwc2::HandleSuspend() {
  // TODO(b/355271738): Logs added to debug b/355271738. Remove when fixed.
  fdf::info("{}", __func__);
  SetConnected(false);
}

// Handler for enumdone interrupt.
void Dwc2::HandleEnumDone() {
  // TODO(b/355271738): Logs added to debug b/355271738. Remove when fixed.
  fdf::info("{}", __func__);
  SetConnected(true);

  auto* mmio = get_mmio();

  ep0_state_ = Ep0State::IDLE;

  endpoints_[DWC_EP0_IN]->max_packet_size = 64;
  endpoints_[DWC_EP0_OUT]->max_packet_size = 64;
  endpoints_[DWC_EP0_IN]->phys = static_cast<uint32_t>(ep0_buffer_->phys());
  endpoints_[DWC_EP0_OUT]->phys = static_cast<uint32_t>(ep0_buffer_->phys());

  DEPCTL0::Get(DWC_EP0_IN).ReadFrom(mmio).set_mps(DEPCTL0::MPS_64).WriteTo(mmio);
  DEPCTL0::Get(DWC_EP0_OUT).ReadFrom(mmio).set_mps(DEPCTL0::MPS_64).WriteTo(mmio);

  DCTL::Get().ReadFrom(mmio).set_cgnpinnak(1).WriteTo(mmio);

  GUSBCFG::Get().ReadFrom(mmio).set_usbtrdtim(metadata_.usb_turnaround_time()).WriteTo(mmio);

  if (dci_intf_.is_valid()) {
    fidl::Arena arena;
    auto result = dci_intf_.buffer(arena)->SetSpeed(fdescriptor::wire::UsbSpeed::kHigh);
    ZX_ASSERT(result.ok());  // Never expected to fail.
  }
  StartEp0();
}

// Handler for inepintr interrupt.
void Dwc2::HandleInEpInterrupt() {
  auto* mmio = get_mmio();
  uint8_t ep_num = 0;

  // Read bits indicating which endpoints have inepintr active
  uint32_t ep_bits = DAINT::Get().ReadFrom(mmio).reg_value();
  ep_bits &= DAINTMSK::Get().ReadFrom(mmio).reg_value();
  ep_bits &= DWC_EP_IN_MASK;

  // Acknowledge the endpoint bits
  DAINT::Get().FromValue(DWC_EP_IN_MASK).WriteTo(mmio);

  // Loop through IN endpoints and handle those with interrupt raised
  while (ep_bits) {
    if (ep_bits & 1) {
      auto diepint = DIEPINT::Get(ep_num).ReadFrom(mmio);
      auto diepmsk = DIEPMSK::Get().ReadFrom(mmio);
      diepint.set_reg_value(diepint.reg_value() & diepmsk.reg_value());

      if (diepint.xfercompl()) {
        DIEPINT::Get(ep_num).FromValue(0).set_xfercompl(1).WriteTo(mmio);

        if (ep_num == DWC_EP0_IN) {
          HandleEp0TransferComplete(true);
        } else {
          HandleTransferComplete(ep_num);
          if (diepint.nak()) {
            fdf::error("Unhandled interrupt diepint.nak ep_num {}", ep_num);
            DIEPINT::Get(ep_num).ReadFrom(mmio).set_nak(1).WriteTo(mmio);
          }
        }
      }

      // TODO(voydanoff) Implement error recovery for these interrupts
      if (diepint.epdisabled()) {
        fdf::error("Unhandled interrupt diepint.epdisabled for ep_num {}", ep_num);
        DIEPINT::Get(ep_num).ReadFrom(mmio).set_epdisabled(1).WriteTo(mmio);
      }
      if (diepint.ahberr()) {
        fdf::error("Unhandled interrupt diepint.ahberr for ep_num {}", ep_num);
        DIEPINT::Get(ep_num).ReadFrom(mmio).set_ahberr(1).WriteTo(mmio);
      }
      if (diepint.timeout()) {
        fdf::error("(diepint.timeout) (ep{}) DIEPINT=0x{:08x} DIEPMSK=0x{:08x}", ep_num,
                   diepint.reg_value(), diepmsk.reg_value());

        // The timeout is due to one of two cases:
        //   1. The core never received an ACK to sent IN-data. In this case, the host
        //      successfully received IN-data, and will subsequently ACK the transmission. That
        //      ACK was lost in transit to the core.
        //   2. IN-data was lost in transmission to the host. In this case, the host will
        //      re-issue an IN-token requesting the data be retransmitted.
        //
        // In the case of #1, the core is in a state where it NAKs all incoming tokens on
        // OUT-EP0. It needs to clear NAK state and prepare to receive an ACK token from the
        // host. In the case of #2, the core needs to prepare to retransmit the lost data (which
        // remains in the FIFO).
        //
        // The actual recovery logic proved difficult to get right without the ability to locally
        // reproduce the issue outside of the CI/CQ lab. I'll probably need access to bench test
        // equipment capable of synthesizing the issue locally (which I don't have). In the
        // meantime, we'll service DIEPINT.timeout by issuing a soft-disconnect, and reset the
        // controller. This appears to the host as an unplug/re-plug port event.
        HandleEp0TimeoutRecovery();

        // The recovery logic currently clobbers all controller state, including pending interrupts.
        // Since there's no more work to perform, this IRQ handler can return.
        return;
      }
      if (diepint.intktxfemp()) {
        fdf::error("Unhandled interrupt diepint.intktxfemp for ep_num {}", ep_num);
        DIEPINT::Get(ep_num).ReadFrom(mmio).set_intktxfemp(1).WriteTo(mmio);
      }
      if (diepint.intknepmis()) {
        fdf::error("Unhandled interrupt diepint.intknepmis for ep_num {}", ep_num);
        DIEPINT::Get(ep_num).ReadFrom(mmio).set_intknepmis(1).WriteTo(mmio);
      }
      if (diepint.inepnakeff()) {
        printf("Unhandled interrupt diepint.inepnakeff for ep_num %u\n", ep_num);
        DIEPINT::Get(ep_num).ReadFrom(mmio).set_inepnakeff(1).WriteTo(mmio);
      }
    }
    ep_num++;
    ep_bits >>= 1;
  }
}

// Handler for outepintr interrupt.
void Dwc2::HandleOutEpInterrupt() {
  auto* mmio = get_mmio();

  uint8_t ep_num = DWC_EP0_OUT;

  // Read bits indicating which endpoints have outepintr active
  auto ep_bits = DAINT::Get().ReadFrom(mmio).reg_value();
  auto ep_mask = DAINTMSK::Get().ReadFrom(mmio).reg_value();
  ep_bits &= ep_mask;
  ep_bits &= DWC_EP_OUT_MASK;
  ep_bits >>= DWC_EP_OUT_SHIFT;

  // Acknowledge the endpoint bits
  DAINT::Get().FromValue(DWC_EP_OUT_MASK).WriteTo(mmio);

  // Loop through OUT endpoints and handle those with interrupt raised
  while (ep_bits) {
    if (ep_bits & 1) {
      auto doepint = DOEPINT::Get(ep_num).ReadFrom(mmio);
      doepint.set_reg_value(doepint.reg_value() & DOEPMSK::Get().ReadFrom(mmio).reg_value());

      if (doepint.sr()) {
        DOEPINT::Get(ep_num).ReadFrom(mmio).set_sr(1).WriteTo(mmio);
      }

      if (doepint.stsphsercvd()) {
        DOEPINT::Get(ep_num).ReadFrom(mmio).set_stsphsercvd(1).WriteTo(mmio);
      }

      if (doepint.setup()) {
        // TODO(voydanoff):   On this interrupt, the application must read the DOEPTSIZn
        // register to determine the number of SETUP packets received and process the last
        // received SETUP packet.
        DOEPINT::Get(ep_num).ReadFrom(mmio).set_setup(1).WriteTo(mmio);

        if (auto setup = ep0_buffer_->ReadStruct<decltype(cur_setup_)>(); setup.is_ok()) {
          cur_setup_ = *setup;
        } else {
          fdf::error("ReadStruct failed: {}", setup.status_string());
          StallEp0();
          return;
        }
        fdf::debug(
            "SETUP bm_request_type: 0x{:02x} b_request: {} w_value: {} w_index: {} "
            "w_length: %u\n",
            cur_setup_.bm_request_type, cur_setup_.b_request, cur_setup_.w_value,
            cur_setup_.w_index, cur_setup_.w_length);

        HandleEp0Setup();
      }
      if (doepint.xfercompl()) {
        DOEPINT::Get(ep_num).FromValue(0).set_xfercompl(1).WriteTo(mmio);

        if (ep_num == DWC_EP0_OUT) {
          if (!doepint.setup()) {
            HandleEp0TransferComplete(false);
          }
        } else {
          HandleTransferComplete(ep_num);
        }
      }
      // TODO(voydanoff) Implement error recovery for these interrupts
      if (doepint.epdisabled()) {
        fdf::error("Unhandled interrupt doepint.epdisabled for ep_num {}", ep_num);
        DOEPINT::Get(ep_num).ReadFrom(mmio).set_epdisabled(1).WriteTo(mmio);
      }
      if (doepint.ahberr()) {
        fdf::error("Unhandled interrupt doepint.ahberr for ep_num {}", ep_num);
        DOEPINT::Get(ep_num).ReadFrom(mmio).set_ahberr(1).WriteTo(mmio);
      }
    }
    ep_num++;
    ep_bits >>= 1;
  }
}

// Handles setup requests from the host.
zx_status_t Dwc2::HandleSetupRequest(size_t* out_actual) {
  zx_status_t status;

  zx::duration elapsed;
  zx::time_boot now;
  if (cur_setup_.bm_request_type == (USB_DIR_OUT | USB_TYPE_STANDARD | USB_RECIP_DEVICE)) {
    // Handle some special setup requests in this driver
    switch (cur_setup_.b_request) {
      case USB_REQ_SET_ADDRESS:
        fdf::info("SET_ADDRESS {}", cur_setup_.w_value);
        SetAddress(static_cast<uint8_t>(cur_setup_.w_value));
        now = zx::clock::get_boot();
        elapsed = now - irq_timestamp_;
        fdf::info(
            "Took {} microseconds to reply to SET_ADDRESS interrupt\nStarted waiting at {:x}\nGot "
            "hardware IRQ at {:x}\nFinished processing at {:x}, context switch happened at {:x}",
            static_cast<int>(elapsed.to_usecs()), wait_start_time_.get(), irq_timestamp_.get(),
            now.get(), irq_dispatch_timestamp_.get());
        if (elapsed.to_msecs() > 2) {
          fdf::error("Handling SET_ADDRESS took greater than 2ms");
        }
        *out_actual = 0;
        return ZX_OK;
      case USB_REQ_SET_CONFIGURATION:
        fdf::info("SET_CONFIGURATION {}", cur_setup_.w_value);
        configured_ = true;
        if (dci_intf_.is_valid()) {
          status = DoControlRead(0, out_actual);
        } else {
          status = ZX_ERR_NOT_SUPPORTED;
        }
        if (status == ZX_OK && cur_setup_.w_value) {
          StartEndpoints();
        } else {
          configured_ = false;
        }
        return status;
      default:
        // fall through to dci_intf_->Control()
        break;
    }
  }

  bool is_in = ((cur_setup_.bm_request_type & USB_DIR_MASK) == USB_DIR_IN);
  auto length = le16toh(cur_setup_.w_length);

  if (dci_intf_.is_valid()) {
    if (length == 0) {
      status = DoControlRead(0, out_actual);
    } else if (is_in) {
      status = DoControlRead(length, out_actual);
    } else {
      status = ZX_ERR_NOT_SUPPORTED;
    }
  } else {
    status = ZX_ERR_NOT_SUPPORTED;
  }
  if (status == ZX_OK) {
    auto& ep = endpoints_[DWC_EP0_OUT];
    ep->req_offset = 0;
    if (is_in) {
      ep->req_length = static_cast<uint32_t>(*out_actual);
    }
  }
  return status;
}

// Programs the device address received from the SET_ADDRESS command from the host
void Dwc2::SetAddress(uint8_t address) {
  auto* mmio = get_mmio();

  DCFG::Get().ReadFrom(mmio).set_devaddr(address).WriteTo(mmio);
}

// Reads number of bytes transfered on specified endpoint
uint32_t Dwc2::ReadTransfered(Endpoint* ep) {
  auto* mmio = get_mmio();
  return ep->req_xfersize - DEPTSIZ::Get(ep->ep_addr()).ReadFrom(mmio).xfersize();
}

// Prepares to receive next control request on endpoint zero.
void Dwc2::StartEp0() {
  auto* mmio = get_mmio();
  auto& ep = endpoints_[DWC_EP0_OUT];
  ep->req_offset = 0;
  ep->req_xfersize = 3 * sizeof(usb_setup_info_t);

  if (auto status = ep0_buffer_->CacheFlushInvalidate(0, sizeof(cur_setup_)); status.is_error()) {
    fdf::error("CacheFlushInvalidate failed: {}", status.status_string());
    return;
  }

  DEPDMA::Get(DWC_EP0_OUT)
      .FromValue(0)
      .set_addr(static_cast<uint32_t>(ep0_buffer_->phys()))
      .WriteTo(get_mmio());

  DEPTSIZ0::Get(DWC_EP0_OUT)
      .FromValue(0)
      .set_supcnt(3)
      .set_pktcnt(1)
      .set_xfersize(ep->req_xfersize)
      .WriteTo(mmio);

  DEPCTL::Get(DWC_EP0_OUT).ReadFrom(mmio).set_epena(1).WriteTo(mmio);
}

// Queues the next USB request for the specified endpoint
void Dwc2::QueueNextRequest(Endpoint* ep) {
  if (ep->current_req || ep->queued_reqs.empty()) {
    return;
  }

  ep->current_req.emplace(std::move(ep->queued_reqs.front()));
  ep->queued_reqs.pop();

  ep->pending_zlp = false;

  auto status =
      std::visit([this](auto&& req) -> zx_status_t { return req.PhysMap(bti_); }, *ep->current_req);
  ZX_ASSERT_MSG(status == ZX_OK, "PhysMap failed");
  auto iters = ep->get_iter(*ep->current_req, zx_system_get_page_size());
  if (iters.is_error()) {
    fdf::error("get_iter failed: {}", iters.status_string());
    auto req = std::move(*ep->current_req);
    ep->current_req.reset();
    std::visit([ep, status = iters.status_value()](
                   auto&& r) { ep->RequestComplete(status, 0, std::move(r)); },
               req);
    return;
  }
  if (iters->size() != 1) {
    fdf::error("dwc2 only supports single segment requests (segments: {})", iters->size());
    auto req = std::move(*ep->current_req);
    ep->current_req.reset();
    std::visit([ep](auto&& r) { ep->RequestComplete(ZX_ERR_NOT_SUPPORTED, 0, std::move(r)); }, req);
    return;
  }
  auto iter = iters->at(0).begin();

  ep->phys = static_cast<uint32_t>((*iter).first);
  ep->req_offset = 0;
  ep->req_length = static_cast<uint32_t>((*iter).second);

  bool is_in = DWC_EP_IS_IN(ep->ep_addr());
  bool is_short =
      std::visit([](auto&& req) { return req->short_().value_or(false); }, *ep->current_req);
  if (is_in && is_short && ep->req_length > 0 && (ep->req_length % ep->max_packet_size == 0)) {
    ep->pending_zlp = true;
  }

  StartTransfer(ep, ep->req_length);
}

void Dwc2::StartTransfer(Endpoint* ep, uint32_t length) {
  auto ep_num = ep->ep_addr();
  auto* mmio = get_mmio();
  bool is_in = DWC_EP_IS_IN(ep_num);

  // Non-control endpoint flushing is the responsibility of the usb-function driver.
  if (length > 0 && !ep->current_req) {
    if (is_in) {
      if (ep_num == DWC_EP0_IN) {
        if (auto status = ep0_buffer_->CacheFlush(ep->req_offset, length); status.is_error()) {
          fdf::error("CacheFlush failed: {}", status.status_string());
          return;
        }
      }
    } else {
      if (ep_num == DWC_EP0_OUT) {
        if (auto status = ep0_buffer_->CacheFlushInvalidate(ep->req_offset, length);
            status.is_error()) {
          fdf::error("CacheFlushInvalidate failed: {}", status.status_string());
          return;
        }
      }
    }
  }

  // Program DMA address
  DEPDMA::Get(ep_num).FromValue(0).set_addr(ep->phys + ep->req_offset).WriteTo(mmio);

  uint32_t ep_mps = ep->max_packet_size;
  auto deptsiz = DEPTSIZ::Get(ep_num).FromValue(0);

  if (length == 0) {
    deptsiz.set_xfersize(is_in ? 0 : ep_mps);
    deptsiz.set_pktcnt(1);
  } else {
    deptsiz.set_pktcnt((length + (ep_mps - 1)) / ep_mps);
    deptsiz.set_xfersize(length);
  }
  deptsiz.set_mc(is_in ? 1 : 0);
  ep->req_xfersize = deptsiz.xfersize();
  deptsiz.WriteTo(mmio);

  DEPCTL::Get(ep_num).ReadFrom(mmio).set_cnak(1).set_epena(1).WriteTo(mmio);
}

void Dwc2::FlushTxFifo(uint32_t fifo_num) {
  auto* mmio = get_mmio();

  auto grstctl = GRSTCTL::Get().FromValue(0).set_txfflsh(1).set_txfnum(fifo_num).WriteTo(mmio);

  uint32_t count = 0;
  do {
    grstctl.ReadFrom(mmio);
    // Retry count of 10000 comes from Amlogic bootloader driver.
    if (++count > 10000) {
      fdf::error("took more than 10k cycles to TX-FIFO flush for FIFO-{}", fifo_num);
      break;
    }
  } while (grstctl.txfflsh() == 1);

  zx::nanosleep(zx::deadline_after(zx::usec(1)));
}

void Dwc2::FlushRxFifo() {
  auto* mmio = get_mmio();
  auto grstctl = GRSTCTL::Get().FromValue(0).set_rxfflsh(1).WriteTo(mmio);

  uint32_t count = 0;
  do {
    grstctl.ReadFrom(mmio);
    if (++count > 10000)
      break;
  } while (grstctl.rxfflsh() == 1);

  zx::nanosleep(zx::deadline_after(zx::usec(1)));
}

void Dwc2::FlushTxFifoRetryIndefinite(uint32_t fifo_num) {
  auto* mmio = get_mmio();

  auto grstctl = GRSTCTL::Get().FromValue(0).set_txfflsh(1).set_txfnum(fifo_num).WriteTo(mmio);

  do {
    grstctl.ReadFrom(mmio);
  } while (grstctl.txfflsh() == 1);

  zx::nanosleep(zx::deadline_after(zx::usec(1)));
}

void Dwc2::FlushRxFifoRetryIndefinite() {
  auto* mmio = get_mmio();
  auto grstctl = GRSTCTL::Get().FromValue(0).set_rxfflsh(1).WriteTo(mmio);

  do {
    grstctl.ReadFrom(mmio);
  } while (grstctl.rxfflsh() == 1);

  zx::nanosleep(zx::deadline_after(zx::usec(1)));
}

void Dwc2::StartEndpoints() {
  for (uint8_t ep_num = 1; ep_num < std::size(endpoints_); ep_num++) {
    auto& ep = endpoints_[ep_num];
    if (ep->enabled) {
      EnableEp(ep_num, true);

      std::lock_guard<std::mutex> _(ep->lock);
      QueueNextRequest(&*ep);
    }
  }
}

void Dwc2::EnableEp(uint8_t ep_num, bool enable) {
  auto* mmio = get_mmio();

  std::lock_guard<std::mutex> _(lock_);

  uint32_t bit = 1 << ep_num;

  auto mask = DAINTMSK::Get().ReadFrom(mmio).reg_value();
  if (enable) {
    auto daint = DAINT::Get().ReadFrom(mmio).reg_value();
    daint |= bit;
    DAINT::Get().FromValue(daint).WriteTo(mmio);
    mask |= bit;
  } else {
    mask &= ~bit;
  }
  DAINTMSK::Get().FromValue(mask).WriteTo(mmio);
}

void Dwc2::HandleEp0Setup() {
  auto length = letoh16(cur_setup_.w_length);
  bool is_in = ((cur_setup_.bm_request_type & USB_DIR_MASK) == USB_DIR_IN);
  size_t actual = 0;

  // No data to read, can handle setup now
  if (length == 0 || is_in) {
    zx_status_t status = HandleSetupRequest(&actual);
    if (status != ZX_OK) {
      StallEp0();
      return;
    }
  }

  if (length > 0) {
    ep0_state_ = Ep0State::DATA;
    auto& ep = endpoints_[is_in ? DWC_EP0_IN : DWC_EP0_OUT];
    ep->req_offset = 0;

    if (is_in) {
      ep->req_length = static_cast<uint32_t>(actual);
      std::lock_guard<std::mutex> _(ep->lock);
      StartTransfer(&*ep, (ep->req_length > 127 ? ep->max_packet_size : ep->req_length));
    } else {
      ep->req_length = length;
      std::lock_guard<std::mutex> _(ep->lock);
      StartTransfer(&*ep, (length > 127 ? ep->max_packet_size : length));
    }
  } else {
    // no data phase
    // status in IN direction
    HandleEp0Status(true);
  }
}

void Dwc2::StallEp0() {
  auto* mmio = get_mmio();

  // Stall OUT EP0
  auto depctl_out = DEPCTL::Get(DWC_EP0_OUT).ReadFrom(mmio);
  depctl_out.set_stall(1);
  depctl_out.WriteTo(mmio);

  // Stall IN EP0
  auto depctl_in = DEPCTL::Get(DWC_EP0_IN).ReadFrom(mmio);
  depctl_in.set_stall(1);
  depctl_in.WriteTo(mmio);

  ep0_state_ = Ep0State::IDLE;
  StartEp0();
}

// Handles status phase of a setup request
void Dwc2::HandleEp0Status(bool is_in) {
  ep0_state_ = Ep0State::STATUS;
  uint8_t ep_num = (is_in ? DWC_EP0_IN : DWC_EP0_OUT);
  auto& ep = endpoints_[ep_num];
  std::lock_guard<std::mutex> _(ep->lock);
  StartTransfer(&*ep, 0);

  if (is_in) {
    StartEp0();
  }
}

// Handles transfer complete events for endpoint zero
void Dwc2::HandleEp0TransferComplete(bool is_in) {
  switch (ep0_state_) {
    case Ep0State::IDLE: {
      StartEp0();
      break;
    }
    case Ep0State::DATA: {
      auto& ep = endpoints_[is_in ? DWC_EP0_IN : DWC_EP0_OUT];
      auto transfered = ReadTransfered(&*ep);
      ep->req_offset += transfered;

      if (is_in) {  // data direction is IN-type (to the host).
        if (ep->req_offset == ep->req_length) {
          HandleEp0Status(false);
          return;
        }

        auto length = ep->req_length - ep->req_offset;
        length = std::min<uint32_t>(length, 64);

        // It's possible the data to be transmitted never makes it to the host. For all but the
        // last packet's worth of data, the core handles retransmission internally. To prepare to
        // (potentially) retransmit data, the last transmission's size is recorded.
        last_transmission_len_ = length;

        std::lock_guard<std::mutex> _(ep->lock);
        StartTransfer(&*ep, length);
      } else {  // data direction is OUT-type (from the host).
        if (ep->req_offset == ep->req_length) {
          if (!dci_intf_.is_valid()) {
            StallEp0();
            return;
          }
          zx_status_t status = DoControlWrite(ep->req_length);
          if (status != ZX_OK) {
            StallEp0();
            return;
          }
          HandleEp0Status(true);
          return;
        }

        auto length = ep->req_length - ep->req_offset;
        // Strangely, the controller can transfer up to 127 bytes in a single transaction.
        // But if length is > 127, the transfer must be done in multiple chunks, and those
        // chunks must be 64 bytes long.
        if (length > 127) {
          length = 64;
        }
        std::lock_guard<std::mutex> _(ep->lock);
        StartTransfer(&*ep, length);
      }
      break;
    }
    case Ep0State::STATUS:
      ep0_state_ = Ep0State::IDLE;
      if (!is_in) {
        StartEp0();
      }
      break;
    case Ep0State::TIMEOUT_RECOVERY: {
      if (is_in) {
        // Timeout was due to lost data.
        auto& ep = endpoints_[DWC_EP0_IN];
        ep->req_offset += ReadTransfered(&*ep);
        ZX_ASSERT(ep->req_offset == ep->req_length);
        HandleEp0Status(false);
      } else {
        // Timeout was due to lost ACK. Prepare the core to receive STATUS data.
        HandleEp0Status(false);
      }
      break;
    }
    case Ep0State::STALL:
    default:
      fdf::error("EP0 state is {}, should not get here", static_cast<int>(ep0_state_));
      break;
  }
}

// Executes a soft port disconnect and issues a core reset.
void Dwc2::SoftDisconnect() {
  auto* mmio = get_mmio();

  fdf::warn("executing USB port soft-disconnect and controller reset");
  DCTL::Get().ReadFrom(mmio).set_sftdiscon(1).WriteTo(mmio);
  zx::nanosleep(zx::deadline_after(zx::msec(5)));

  const zx::result result = ResetCore();
  ZX_ASSERT_MSG(
      result.is_ok(),
      "Failed to reset DWC2 core during SoftDisconnect (%s), self terminating to avoid runaway DMA\n",
      result.status_string());
}

// Handles the case where the core experiences a timeout due to lost data or ACK. For the time
// being, the recovery logic involves a soft port disconnect and controller reset. This appears to
// the host as a unplug-replug event.
void Dwc2::HandleEp0TimeoutRecovery() {
  std::lock_guard<std::mutex> _(lock_);
  SetConnected(false);
  SoftDisconnect();
  ep0_state_ = Ep0State::DISCONNECTED;
  zx::nanosleep(zx::deadline_after(zx::msec(50)));

  const zx::result result = InitController();  // Clears the GRSTCTRL.sftdiscon condition.
  if (result.is_error()) {
    fdf::warn("DWC2 core failed InitController ({}) during {}\n", result.status_string(),
              __PRETTY_FUNCTION__);
  }
  fdf::info("USB port soft-disconnect and controller reset sequence complete");
}

// Handles transfer complete events for endpoints other than endpoint zero
void Dwc2::HandleTransferComplete(uint8_t ep_num) {
  ZX_DEBUG_ASSERT(ep_num != DWC_EP0_IN && ep_num != DWC_EP0_OUT);
  auto& ep = endpoints_[ep_num];

  ep->lock.lock();

  ep->req_offset += ReadTransfered(&*ep);

  if (ep->pending_zlp) {
    ep->pending_zlp = false;
    StartTransfer(&*ep, 0);  // Zero-length transfer results in a ZLP.
    ep->lock.unlock();
    return;
  }

  // Make a copy since this is used outside the critical section.
  auto actual = ep->req_offset;

  if (ep->current_req) {
    auto req = std::move(ep->current_req.value());
    ep->current_req.reset();
    // It is necessary to set current_req = nullptr in order to make this re-entrant safe and
    // thread-safe. When we call request.Complete the callee may immediately re-queue this request.
    // if it is already in current_req it could be completed twice (since QueueNextRequest
    // would attempt to re-queue it, or CancelAll could take the lock on a separate thread and
    // forcefully complete it after we've already completed it).
    ep->lock.unlock();
    ep->RequestComplete(ZX_OK, actual, std::move(req));
    ep->lock.lock();

    QueueNextRequest(&*ep);
  }
  ep->lock.unlock();
}

zx::result<> Dwc2::InitController() {
  auto* mmio = get_mmio();

  if ((cached_gsnpsid_.version() != 0x400a) && (cached_gsnpsid_.version() != 0x330a)) {
    fdf::warn(
        "DWC2 driver has not been tested with IP version 0x{:08x}. "
        "The IP has quirks, so things may not work as expected\n",
        cached_gsnpsid_.reg_value());
  }

  // These should have been checked at init time
  ZX_DEBUG_ASSERT(cached_ghwcfg2_.dynamic_fifo());
  ZX_DEBUG_ASSERT(cached_ghwcfg4_.ded_fifo_en());

  // Reset the controller
  if (const zx::result result = ResetCore(); result.is_error()) {
    fdf::error("Failed to reset DWC2 core: {}", result.status_string());
    return result;
  }

  zx::nanosleep(zx::deadline_after(zx::msec(10)));

  // Enable DMA
  GAHBCFG::Get()
      .FromValue(0)
      .set_dmaenable(1)
      .set_hburstlen(static_cast<uint32_t>(metadata_.dma_burst_len()))
      .set_nptxfemplvl_txfemplvl(1)
      .WriteTo(mmio);

  // Set turnaround time based on metadata
  GUSBCFG::Get().ReadFrom(mmio).set_usbtrdtim(metadata_.usb_turnaround_time()).WriteTo(mmio);
  DCFG::Get()
      .ReadFrom(mmio)
      .set_devaddr(0)
      .set_epmscnt(2)
      .set_descdma(0)
      .set_devspd(0)
      .set_perfrint(DCFG::PERCENT_80)
      .WriteTo(mmio);

  DCTL::Get().ReadFrom(mmio).set_sftdiscon(1).WriteTo(mmio);
  DCTL::Get().ReadFrom(mmio).set_sftdiscon(0).WriteTo(mmio);

  // Reset phy clock
  PCGCCTL::Get().FromValue(0).WriteTo(mmio);

  // Set fifo sizes based on metadata.
  GRXFSIZ::Get().FromValue(0).set_size(metadata_.rx_fifo_size()).WriteTo(mmio);
  GNPTXFSIZ::Get()
      .FromValue(0)
      .set_depth(metadata_.nptx_fifo_size())
      .set_startaddr(metadata_.rx_fifo_size())
      .WriteTo(mmio);

  auto fifo_base = metadata_.rx_fifo_size() + metadata_.nptx_fifo_size();
  auto dfifo_end = GHWCFG3::Get().ReadFrom(mmio).dfifo_depth();

  uint32_t total_tx_fifo_size = 0;
  for (uint32_t i = 0; i < std::size(metadata_.tx_fifo_sizes()); i++) {
    auto fifo_size = metadata_.tx_fifo_sizes()[i];

    DTXFSIZ::Get(i + 1).FromValue(0).set_startaddr(fifo_base).set_depth(fifo_size).WriteTo(mmio);
    fifo_base += fifo_size;
    total_tx_fifo_size += fifo_size;
  }

  GDFIFOCFG::Get().FromValue(0).set_gdfifocfg(dfifo_end).set_epinfobase(fifo_base).WriteTo(mmio);
  // Guard against going past the total RAM we have.
  if (fifo_base + metadata_.tx_fifo_sizes().size() > dfifo_end) {
    fdf::error(
        "Insufficient RAM for FIFO configuration: \
            rx fifo size: {}\n \
            nptx fifo size: {}\n \
            total tx fifo size: {}\n \
            epinfo_base {}\n \
            dfifo_end: {}",
        metadata_.rx_fifo_size(), metadata_.nptx_fifo_size(), total_tx_fifo_size, fifo_base,
        dfifo_end);
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  // Flush all FIFOs
  FlushTxFifo(0x10);
  FlushRxFifo();

  GRSTCTL::Get().FromValue(0).set_intknqflsh(1).WriteTo(mmio);

  // Clear all pending device interrupts
  DIEPMSK::Get().FromValue(0).WriteTo(mmio);
  DOEPMSK::Get().FromValue(0).WriteTo(mmio);
  DAINT::Get().FromValue(0xFFFFFFFF).WriteTo(mmio);
  DAINTMSK::Get().FromValue(0).WriteTo(mmio);

  for (uint32_t i = 0; i < DWC_MAX_EPS; i++) {
    DEPCTL::Get(i).FromValue(0).WriteTo(mmio);
    DEPTSIZ::Get(i).FromValue(0).WriteTo(mmio);
  }

  // Clear all pending OTG and global interrupts
  GOTGINT::Get().FromValue(0xFFFFFFFF).WriteTo(mmio);
  GINTSTS::Get().FromValue(0xFFFFFFFF).WriteTo(mmio);

  // Enable selected global interrupts
  GINTMSK::Get()
      .FromValue(0)
      .set_usbreset(1)
      .set_enumdone(1)
      .set_inepintr(1)
      .set_outepintr(1)
      .set_usbsuspend(1)
      .set_erlysuspend(1)
      .WriteTo(mmio);

  // Enable global interrupts
  GAHBCFG::Get().ReadFrom(mmio).set_glblintrmsk(1).WriteTo(mmio);

  return zx::ok();
}

void Dwc2::SetConnected(bool connected) {
  if (connected == connected_) {
    return;
  }

  if (dci_intf_.is_valid()) {
    fidl::Arena arena;
    auto result = dci_intf_.buffer(arena)->SetConnected(connected);
    ZX_ASSERT_MSG(result.ok(), "SetConnected failed: %s",
                  result.status_string());  // Never expected to fail.
  }
  if (phy_.is_valid()) {
    auto connect = phy_->ConnectStatusChanged({{.connected = connected, .wake_lease = {}}});
    if (connect.is_error()) {
      fdf::warn("Call to ConnectStatusChanged on usb phy failed: {}",
                connect.error_value().FormatDescription().c_str());
      // Continue despite failure.
    }
  }

  if (!connected) {
    // Complete any pending requests
    for (size_t i = 0; i < std::size(endpoints_); i++) {
      auto& ep = endpoints_[i];

      std::queue<usb::RequestVariant> complete_reqs;
      {
        std::lock_guard<std::mutex> _(ep->lock);
        complete_reqs.swap(ep->queued_reqs);

        if (ep->current_req) {
          complete_reqs.emplace(std::move(*ep->current_req));
          ep->current_req.reset();
        }

        ep->enabled = false;
        ep->pending_zlp = false;
      }

      // Requests must be completed outside of the lock.
      while (true) {
        if (complete_reqs.empty()) {
          break;
        }

        ep->RequestComplete(ZX_ERR_IO_NOT_PRESENT, 0, std::move(complete_reqs.front()));
        complete_reqs.pop();
      }
    }
  }

  connected_ = connected;
}

zx_status_t Dwc2::Init(fdf::DriverContext& context, const dwc2_config::Config& config) {
  std::lock_guard<std::mutex> _(lock_);

  auto incoming = std::shared_ptr<fdf::Namespace>(context.take_incoming());
  zx::result pdev = incoming->Connect<fpdev::Service::Device>("pdev");
  if (pdev.is_error()) {
    fdf::error("Connect(): {}", pdev);
    return pdev.status_value();
  }
  pdev_ = std::make_unique<fdf::PDev>(std::move(*pdev));

  // First thing, map our registers and verify that this is the hardware we are
  // looking for.
  zx::result mmio = pdev_->MapMmio(0);
  if (mmio.is_error()) {
    fdf::error("Failed to map mmio: {}", mmio);
    return mmio.status_value();
  }
  mmio_ = std::move(mmio.value());
  cached_gsnpsid_ = GSNPSID::Get().ReadFrom(get_mmio());
  cached_ghwcfg2_ = GHWCFG2::Get().ReadFrom(get_mmio());
  cached_ghwcfg4_ = GHWCFG4::Get().ReadFrom(get_mmio());

  // All revisions of the Synopsis DWC2 core ID should have 0x4f54 (ascii ==
  // 'OT') in their upper bits.
  if (cached_gsnpsid_.ot() != 0x4f54) {
    fdf::error("Unrecognized Synopsis ID in DWC2 core: 0x{:08x}", cached_gsnpsid_.reg_value());
    return ZX_ERR_NOT_SUPPORTED;
  }

  // Now explicitly reset our core so that we are in a known state and are
  // certain that all DMA has been stopped.
  if (const zx::result result = ResetCore(); result.is_error()) {
    fdf::error("Failed to reset DWC2 core: {}", result.status_string());
    return result.status_value();
  }

  // Now that the core has been reset, grab our BTI and release any quarantined
  // pages.
  zx::result bti = pdev_->GetBti(0);
  if (bti.is_error()) {
    fdf::error("Failed to get bti: {}", bti);
    return bti.status_value();
  }
  bti_ = std::move(bti.value());
  bti_.release_quarantine();

  // If the HW was not instantiated with the specific silicon features this
  // driver needs to operate, go no further.
  if (!cached_ghwcfg2_.dynamic_fifo()) {
    fdf::error("DWC2 driver requires dynamic FIFO support (GHWCFG2 = 0x{:08x})",
               cached_ghwcfg2_.reg_value());
    return ZX_ERR_NOT_SUPPORTED;
  }

  if (!cached_ghwcfg4_.ded_fifo_en()) {
    fdf::error("DWC2 driver requires dedicated FIFO support (GHWCFG4 = 0x{:08x})",
               cached_ghwcfg4_.reg_value());
    return ZX_ERR_NOT_SUPPORTED;
  }

  // Initialize mac address metadata server.
  if (zx::result result =
          mac_address_metadata_server_.ForwardAndServe(*outgoing(), dispatcher(), incoming, "pdev");
      result.is_error()) {
    fdf::error("Failed to forward mac address metadata: {}", result);
    return result.status_value();
  }

  // Initialize serial number metadata server.
  if (zx::result result = serial_number_metadata_server_.ForwardAndServe(*outgoing(), dispatcher(),
                                                                         incoming, "pdev");
      result.is_error()) {
    fdf::error("Failed to forward serial number metadata: {}", result);
    return result.status_value();
  }

  // USB PHY protocol is optional.
  zx::result phy = incoming->Connect<fphy::Service::Device>("dwc2-phy");
  if (phy.is_ok()) {
    phy_.Bind(std::move(*phy));
  }

  for (uint8_t i = 0; i < std::size(endpoints_); i++) {
    endpoints_[i].emplace(i, this);
  }

  zx::result metadata = pdev_->GetFidlMetadata<fuchsia_hardware_usb_dwc2::Metadata>();
  if (metadata.is_error()) {
    fdf::error("Failed to get metadata: {}", metadata);
    return ZX_ERR_INTERNAL;
  }
  metadata_ = std::move(metadata.value());

  // Validate FIFO configuration against hardware limits.
  uint32_t num_dev_ep = cached_ghwcfg2_.num_dev_ep();
  uint32_t total_tx_fifo_size = 0;
  for (size_t i = 0; i < metadata_.tx_fifo_sizes().size(); i++) {
    if (metadata_.tx_fifo_sizes()[i] > 0) {
      if (i >= num_dev_ep - 1) {
        fdf::error("Tx FIFO {} configured with size {} but hardware only supports {} Tx FIFOs", i,
                   metadata_.tx_fifo_sizes()[i], num_dev_ep - 1);
        return ZX_ERR_INVALID_ARGS;
      }
      total_tx_fifo_size += metadata_.tx_fifo_sizes()[i];
    }
  }

  auto dfifo_depth = GHWCFG3::Get().ReadFrom(get_mmio()).dfifo_depth();
  uint32_t required_ram =
      metadata_.rx_fifo_size() + metadata_.nptx_fifo_size() + total_tx_fifo_size;
  if (required_ram > dfifo_depth) {
    fdf::error("Insufficient RAM for FIFO configuration: required {}, available {}", required_ram,
               dfifo_depth);
    return ZX_ERR_NO_MEMORY;
  }

  zx::result interrupt = pdev_->GetInterrupt(0, 0);
  if (interrupt.is_error()) {
    fdf::error("Failed to get interrupt: {}", interrupt);
    return interrupt.status_value();
  }
  irq_ = std::move(interrupt.value());

  zx_status_t status = dma_buffer::CreateBufferFactory()->CreateContiguous(
      bti_, kEp0BufferSize, 12, dma_buffer::CacheOptions::kEnabled, &ep0_buffer_);
  if (status != ZX_OK) {
    fdf::error("dma_buffer::CreateBufferFactory()->CreateContiguous(): {}",
               zx_status_get_string(status));
    return status;
  }

  zx::result result =
      outgoing()->AddService<fdci::UsbDciService>(fdci::UsbDciService::InstanceHandler({
          .device = bindings_.CreateHandler(this, dispatcher(), fidl::kIgnoreBindingClosure),
      }));
  if (result.is_error()) {
    fdf::error("Failed to add service {}", result);
    return result.status_value();
  }

  std::vector props{
      fdf::MakeProperty2(bind_fuchsia::PLATFORM_DEV_VID,
                         bind_fuchsia_designware_platform::BIND_PLATFORM_DEV_VID_DESIGNWARE),
      fdf::MakeProperty2(bind_fuchsia::PLATFORM_DEV_DID,
                         bind_fuchsia_designware_platform::BIND_PLATFORM_DEV_DID_DWC2),
      fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.usb.dci.UsbDciService"),
  };

  std::vector offers = {fdf::MakeOffer2<fdci::UsbDciService>()};
  std::optional mac_address_offer = mac_address_metadata_server_.CreateOffer();
  if (mac_address_offer.has_value()) {
    offers.push_back(std::move(mac_address_offer.value()));
  }
  std::optional serial_number_offer = serial_number_metadata_server_.CreateOffer();
  if (serial_number_offer.has_value()) {
    offers.push_back(std::move(serial_number_offer.value()));
  }

  zx::result child = AddChild(name(), props, offers);
  if (child.is_error()) {
    fdf::error("AddChild(): {}", child);
    return child.error_value();
  }
  child_.Bind(std::move(*child));

  return ZX_OK;
}

int Dwc2::IrqThread() {
  auto* mmio = get_mmio();

  while (1) {
    wait_start_time_ = zx::clock::get_boot();
    auto wait_res = irq_.wait(&irq_timestamp_);
    irq_dispatch_timestamp_ = zx::clock::get_boot();
    if (wait_res == ZX_ERR_CANCELED) {
      break;
    }
    if (wait_res != ZX_OK) {
      fdf::error("dwc_usb: irq wait failed, retcode = {}", wait_res);
    }

    // It doesn't seem that this inner loop should be necessary,
    // but without it we miss interrupts on some versions of the IP.
    while (1) {
      auto gintsts = GINTSTS::Get().ReadFrom(mmio);
      auto gintmsk = GINTMSK::Get().ReadFrom(mmio);
      gintsts.WriteTo(mmio);
      gintsts.set_reg_value(gintsts.reg_value() & gintmsk.reg_value());

      if (gintsts.reg_value() == 0) {
        break;
      }

      if (gintsts.usbreset()) {
        HandleReset();
      }
      if (gintsts.usbsuspend()) {
        HandleSuspend();
      }
      if (gintsts.enumdone()) {
        HandleEnumDone();
      }
      if (gintsts.inepintr()) {
        HandleInEpInterrupt();
      }
      if (gintsts.outepintr()) {
        HandleOutEpInterrupt();
      }
    }
  }

  fdf::info("dwc_usb: irq thread finished");
  return 0;
}

zx_status_t Dwc2::DoControl(size_t write_size, size_t read_size, size_t* out_read_actual) {
  ZX_ASSERT(dci_intf_.is_valid());

  // Ensure the provided buffers are large enough for the requested sizes.
  if (write_size > ep0_buffer_->buffer().size()) {
    fdf::error("DoControl: write_size ({}) exceeds ep0_buffer_ size ({})", write_size,
               ep0_buffer_->buffer().size());
    return ZX_ERR_BUFFER_TOO_SMALL;
  }
  if (read_size > ep0_buffer_->buffer().size()) {
    fdf::error("DoControl: read_size ({}) exceeds ep0_buffer_ size ({})", read_size,
               ep0_buffer_->buffer().size());
    return ZX_ERR_BUFFER_TOO_SMALL;
  }

  fidl::Arena arena;
  fidl::VectorView<uint8_t> fwrite;

  if (write_size > 0) {
    uint8_t* copy_buf = arena.AllocateVector<uint8_t>(write_size);
    // Copy the data from the DMA buffer to a temporary FIDL arena buffer.
    if (auto status = ep0_buffer_->buffer().Read(0, write_size, copy_buf); status.is_error()) {
      fdf::error("DoControl: failed to read from ep0_buffer_: {}", status.status_string());
      return status.error_value();
    }
    fwrite = fidl::VectorView<uint8_t>::FromExternal(copy_buf, write_size);
  }

  auto result = dci_intf_.buffer(arena)->Control(cur_setup_, fwrite);
  if (!result.ok()) {
    fdf::error("DoControl: Control FIDL call failed: {}", result.status_string());
    return ZX_ERR_INTERNAL;  // framework error.
  }
  if (result->is_error()) {
    return result->error_value();
  }

  std::span<uint8_t> read_data = result.value()->read.get();

  if (!read_data.empty()) {
    // Ensure we don't overflow the destination buffer.
    if (read_data.size_bytes() > ep0_buffer_->buffer().size()) {
      fdf::error("DoControl: received read data size ({}) exceeds ep0_buffer_ size ({})",
                 read_data.size_bytes(), ep0_buffer_->buffer().size());
      return ZX_ERR_BUFFER_TOO_SMALL;
    }
    // Also check against the requested read_size if it was set.
    if (read_size > 0 && read_data.size_bytes() > read_size) {
      fdf::error("DoControl: received read data size ({}) exceeds requested read_size ({})",
                 read_data.size_bytes(), read_size);
      return ZX_ERR_BUFFER_TOO_SMALL;
    }
    if (auto status = ep0_buffer_->buffer().Write(read_data.data(), 0, read_data.size_bytes());
        status.is_error()) {
      fdf::error("DoControl: failed to write to ep0_buffer_: {}", status.status_string());
      return status.error_value();
    }
    if (out_read_actual) {
      *out_read_actual = read_data.size_bytes();
    }
  }

  return ZX_OK;
}

void Dwc2::ConnectToEndpoint(ConnectToEndpointRequest& request,
                             ConnectToEndpointCompleter::Sync& completer) {
  uint8_t ep_num = DWC_ADDR_TO_INDEX(request.ep_addr());
  if (ep_num == DWC_EP0_IN || ep_num == DWC_EP0_OUT || ep_num >= std::size(endpoints_)) {
    fdf::error("Dwc2::UsbDciRequestQueue: bad ep address 0x{:02X}", request.ep_addr());
    completer.Reply(fit::as_error(ZX_ERR_IO_NOT_PRESENT));
    return;
  }

  endpoints_[ep_num]->Connect(dispatcher(), std::move(request.ep()));
  completer.Reply(fit::ok());
}

void Dwc2::SetInterface(SetInterfaceRequest& request, SetInterfaceCompleter::Sync& completer) {
  if (!request.interface().is_valid()) {
    fdf::error("Interface should be valid");
    completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
    return;
  }

  if (dci_intf_.is_valid()) {
    fdf::error("{}: dci_intf_ already set!", __func__);
    completer.Reply(zx::error(ZX_ERR_ALREADY_BOUND));
    return;
  }
  dci_intf_.Bind(std::move(request.interface()));

  completer.Reply(zx::ok());
}

void Dwc2::StartController(StartControllerCompleter::Sync& completer) {
  const zx::result result = [this]() {
    std::lock_guard<std::mutex> guard(lock_);
    return InitController();
  }();

  completer.Reply(result);
}

void Dwc2::StopController(StopControllerCompleter::Sync& completer) {
  std::lock_guard<std::mutex> _(lock_);
  SetConnected(false);
  SoftDisconnect();
  ep0_state_ = Ep0State::DISCONNECTED;
  zx::nanosleep(zx::deadline_after(zx::msec(50)));

  completer.Reply(zx::ok());
}

void Dwc2::ConfigureEndpoint(ConfigureEndpointRequest& request,
                             ConfigureEndpointCompleter::Sync& completer) {
  auto* mmio = get_mmio();

  uint8_t ep_addr = request.ep_descriptor().b_endpoint_address();
  uint8_t ep_num = DWC_ADDR_TO_INDEX(ep_addr);

  if (ep_num == DWC_EP0_IN || ep_num == DWC_EP0_OUT || ep_num >= std::size(endpoints_)) {
    fdf::error("Dwc2::ConfigureEndpoint: bad ep address 0x{:02X}", ep_addr);
    completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
    return;
  }

  bool is_in = usb_ep_direction2(ep_addr);
  fdescriptor::EndpointType ep_type = usb_ep_type2(request.ep_descriptor());
  uint16_t max_packet_size = usb_ep_max_packet2(request.ep_descriptor());

  if (ep_type == fdescriptor::EndpointType::kIsochronous) {
    fdf::error("Dwc2::ConfigureEndpoint: isochronous endpoints are not supported");
    completer.Reply(zx::error(ZX_ERR_NOT_SUPPORTED));
    return;
  }

  // Check if there is enough TX FIFO space for the IN endpoint.
  if (is_in) {
    if (ep_num > metadata_.tx_fifo_sizes().size()) {
      fdf::error("Dwc2::ConfigureEndpoint: no allocated TX FIFO space for IN endpoint {}", ep_num);
      completer.Reply(zx::error(ZX_ERR_NO_RESOURCES));
      return;
    }
    if (max_packet_size > (metadata_.tx_fifo_sizes()[ep_num - 1] * 4)) {
      fdf::error(
          "Dwc2::ConfigureEndpoint: IN  endpoint {} max packet size {} is larger than "
          "allocated TX FIFO space {}",
          ep_num, max_packet_size, metadata_.tx_fifo_sizes()[ep_num - 1] * 4);
      completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
      return;
    }
  }

  auto& ep = endpoints_[ep_num];
  std::lock_guard<std::mutex> _(ep->lock);

  ep->max_packet_size = max_packet_size;
  ep->enabled = true;

  DEPCTL::Get(ep_num)
      .FromValue(0)
      .set_mps(ep->max_packet_size)
      .set_eptype(static_cast<uint8_t>(ep_type))
      .set_setd0pid(1)
      .set_txfnum(is_in ? ep_num : 0)
      .set_usbactep(1)
      .WriteTo(mmio);

  EnableEp(ep_num, true);

  if (configured_) {
    QueueNextRequest(&*ep);
  }

  completer.Reply(zx::ok());
}

void Dwc2::DisableEndpoint(DisableEndpointRequest& request,
                           DisableEndpointCompleter::Sync& completer) {
  auto* mmio = get_mmio();

  unsigned ep_num = DWC_ADDR_TO_INDEX(request.ep_address());
  if (ep_num == DWC_EP0_IN || ep_num == DWC_EP0_OUT || ep_num >= std::size(endpoints_)) {
    fdf::error("Dwc2::UsbDciConfigEp: bad ep address 0x{:02X}", request.ep_address());
    completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
    return;
  }

  auto& ep = endpoints_[ep_num];

  std::lock_guard<std::mutex> _(ep->lock);

  DEPCTL::Get(ep_num).ReadFrom(mmio).set_usbactep(0).WriteTo(mmio);
  ep->enabled = false;
  ep->pending_zlp = false;
  completer.Reply(zx::ok());
}

void Dwc2::EndpointSetStall(EndpointSetStallRequest& request,
                            EndpointSetStallCompleter::Sync& completer) {
  completer.Reply(zx::error(ZX_ERR_NOT_SUPPORTED));
}

void Dwc2::EndpointClearStall(EndpointClearStallRequest& request,
                              EndpointClearStallCompleter::Sync& completer) {
  completer.Reply(zx::error(ZX_ERR_NOT_SUPPORTED));
}

void Dwc2::CancelAll(CancelAllRequest& request, CancelAllCompleter::Sync& completer) {
  uint8_t ep_num = DWC_ADDR_TO_INDEX(request.ep_address());
  endpoints_[ep_num]->CancelAll();
  completer.Reply(zx::ok());
}

void Dwc2::Endpoint::QueueRequests(QueueRequestsRequest& request,
                                   QueueRequestsCompleter::Sync& completer) {
  for (auto& req : request.req()) {
    QueueRequest(usb::FidlRequest{std::move(req)});
  }
}

void Dwc2::Endpoint::QueueRequest(usb::FidlRequest request) {
  dwc2_->lock_.lock();
  if (dwc2_->shutting_down_) {
    dwc2_->lock_.unlock();
    RequestComplete(ZX_ERR_IO_NOT_PRESENT, 0, std::move(request));
    return;
  }
  dwc2_->lock_.unlock();

  // OUT transactions must have length > 0 and multiple of max packet size
  if (DWC_EP_IS_OUT(ep_addr())) {
    size_t length = request.length();
    if (length == 0 || length % max_packet_size != 0) {
      fdf::error("dwc_ep_queue: OUT transfers must be multiple of max packet size");
      RequestComplete(ZX_ERR_INVALID_ARGS, 0, std::move(request));
      return;
    }
  }

  std::lock_guard<std::mutex> _(lock);

  if (!enabled) {
    fdf::error("dwc_ep_queue ep not enabled!");
    RequestComplete(ZX_ERR_BAD_STATE, 0, std::move(request));
    return;
  }

  if (!dwc2_->configured_) {
    fdf::error("dwc_ep_queue not configured!");
    RequestComplete(ZX_ERR_BAD_STATE, 0, std::move(request));
    return;
  }

  queued_reqs.push(std::move(request));
  dwc2_->QueueNextRequest(this);
}

void Dwc2::Endpoint::CancelAll() {
  std::queue<usb::RequestVariant> queue;
  {
    std::lock_guard<std::mutex> ep_guard(lock);
    std::lock_guard<std::mutex> dwc2_guard(dwc2_->lock_);
    pending_zlp = false;
    if (DWC_EP_IS_OUT(ep_addr())) {
      dwc2_->FlushRxFifoRetryIndefinite();
    } else {
      dwc2_->FlushTxFifoRetryIndefinite(ep_addr());
    }
    queue = std::move(queued_reqs);
    if (current_req) {
      queue.push(std::move(current_req.value()));
      current_req.reset();
    }
  }

  while (!queue.empty()) {
    auto req = std::move(queue.front());
    queue.pop();
    RequestComplete(ZX_ERR_IO_NOT_PRESENT, 0, std::move(req));
  }
}

void Dwc2::Endpoint::OnUnbound(
    fidl::UnbindInfo info, fidl::ServerEnd<fuchsia_hardware_usb_endpoint::Endpoint> server_end) {
  // Deliberately do NOT call usb::EndpointServer::OnUnbound.  This will unpin
  // any pinned memory which this endpoint is using.  In theory, we should be
  // stopping the endpoint HW here, but that is far more complicated than it
  // should be given the technical debt which has accumulated here (as well as
  // the nature of the DWC2 core itself, which does not really seem to expect
  // routinely resetting individual endpoints).
  //
  // So, instead, we just leak the memory instead.  It is not _technically_
  // leaked, but instead it has been placed into quarantine (or, on a system
  // with an IOMMU, simply returned to the pool after the while the HW's access
  // rights have been revoked)
  //
  // The driver has also been updated to unconditionally reset the HW at early
  // Init() time, and then release the quarantine once the HW has been
  // explicitly placed in a safe state, so in the case of a restart, the memory
  // will be recovered.
}

void Dwc2::GetHardwareInfo(GetHardwareInfoCompleter::Sync& completer) {
  std::vector<fuchsia_hardware_usb_dci::EndpointInfo> endpoints;

  // IN endpoints.
  // Limited by the number of Tx FIFOs configured in metadata and the hardware endpoint limit.
  uint32_t num_dev_ep = cached_ghwcfg2_.num_dev_ep();
  size_t num_in_eps =
      std::min(metadata_.tx_fifo_sizes().size(), static_cast<size_t>(num_dev_ep - 1));
  std::vector<fuchsia_hardware_usb_dci::SupportedEndpointInfo> in_supported_types(2);
  in_supported_types[0].endpoint_type(fuchsia_hardware_usb_descriptor::EndpointType::kBulk);
  in_supported_types[1].endpoint_type(fuchsia_hardware_usb_descriptor::EndpointType::kInterrupt);

  for (size_t i = 0; i < num_in_eps; i++) {
    uint16_t max_packet = static_cast<uint16_t>(metadata_.tx_fifo_sizes()[i] * kWordSizeBytes);
    in_supported_types[0].max_packet_size_limit(max_packet);
    in_supported_types[1].max_packet_size_limit(max_packet);

    fuchsia_hardware_usb_dci::EndpointInfo ep_info;
    ep_info.ep_address(static_cast<uint8_t>(0x80 | (i + 1)));
    ep_info.supported_types(in_supported_types);
    endpoints.push_back(std::move(ep_info));
  }

  // OUT endpoints share the Rx FIFO.
  // Limited by the number of device endpoints supported by the hardware.
  std::vector<fuchsia_hardware_usb_dci::SupportedEndpointInfo> out_supported_types(2);
  out_supported_types[0].endpoint_type(fuchsia_hardware_usb_descriptor::EndpointType::kBulk);
  out_supported_types[0].max_packet_size_limit(kMaxOutPacketSizeLimit);
  out_supported_types[1].endpoint_type(fuchsia_hardware_usb_descriptor::EndpointType::kInterrupt);
  out_supported_types[1].max_packet_size_limit(kMaxOutPacketSizeLimit);

  for (uint32_t i = 1; i < num_dev_ep; i++) {
    fuchsia_hardware_usb_dci::EndpointInfo ep_info;
    ep_info.ep_address(static_cast<uint8_t>(i));
    ep_info.supported_types(out_supported_types);
    endpoints.push_back(std::move(ep_info));
  }

  fuchsia_hardware_usb_dci::DciHardwareInfo info;
  info.endpoints(std::move(endpoints));
  info.supports_dynamic_ep_sizing(false);

  fuchsia_hardware_usb_dci::UsbDciGetHardwareInfoResponse response;
  response.info(std::move(info));

  completer.Reply(zx::ok(std::move(response)));
}

void Dwc2::AllocEndpoint(AllocEndpointRequest& request, AllocEndpointCompleter::Sync& completer) {
  completer.Reply(zx::error(ZX_ERR_NOT_SUPPORTED));
}

void Dwc2::FreeEndpoint(FreeEndpointRequest& request, FreeEndpointCompleter::Sync& completer) {
  completer.Reply(zx::error(ZX_ERR_NOT_SUPPORTED));
}

}  // namespace dwc2

FUCHSIA_DRIVER_EXPORT2(dwc2::Dwc2);
