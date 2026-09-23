// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "ot_radio.h"

#include <lib/async-loop/default.h>
#include <lib/async/cpp/task.h>
#include <lib/ddk/binding_driver.h>
#include <lib/ddk/debug.h>
#include <lib/ddk/driver.h>
#include <lib/ddk/trace/event.h>
#include <lib/driver-unit-test/utils.h>
#include <lib/fidl/cpp/wire/server.h>
#include <lib/fit/defer.h>
#include <lib/zx/clock.h>
#include <lib/zx/time.h>
#include <stdio.h>
#include <sys/types.h>
#include <zircon/compiler.h>
#include <zircon/status.h>

#include <iterator>

#include <ddktl/fidl.h>
#include <fbl/auto_lock.h>
#include <fbl/ref_counted.h>
#include <fbl/ref_ptr.h>

#include "src/connectivity/openthread/drivers/ot-radio/ot_radio_bootloader.h"

namespace ot {
namespace {
constexpr char kOtRadioTraceCategory[] = "ot-radio";
}  // namespace

namespace lowpan_spinel_fidl = fuchsia_lowpan_spinel;

constexpr uint32_t kRcpBusyPollingDelayMs = 3;
constexpr uint8_t kRcpHardResetPinAssertTimeMs = 100;
constexpr zx::duration kRcpHardResetRetryWaitTime = zx::msec(700);

OtRadioDevice::LowpanSpinelDeviceFidlImpl::LowpanSpinelDeviceFidlImpl(OtRadioDevice& ot_radio)
    : ot_radio_obj_(ot_radio) {}

void OtRadioDevice::LowpanSpinelDeviceFidlImpl::Bind(
    async_dispatcher_t* dispatcher, fidl::ServerEnd<lowpan_spinel_fidl::Device> channel) {
  fidl::OnUnboundFn<LowpanSpinelDeviceFidlImpl> on_unbound =
      [](LowpanSpinelDeviceFidlImpl* server, fidl::UnbindInfo /*unused*/,
         fidl::ServerEnd<lowpan_spinel_fidl::Device> /*unused*/) {
        server->ot_radio_obj_.fidl_impl_obj_.release();
      };
  ot_radio_obj_.fidl_binding_ =
      fidl::BindServer(dispatcher, std::move(channel), this, std::move(on_unbound));
}

void OtRadioDevice::ResetFrameInspectData() {
  bool invalid_state = false;
  for (size_t i = 0; i < pending_tid_->size(); i++) {
    if (pending_tid_->at(i)) {
      zxlogf(WARNING, "ot-radio: pending tid %zu while resetting the radio", i);
      pending_tid_->at(i) = false;
      invalid_state = true;
    }
  }
  if (!invalid_state) {
    zxlogf(INFO, "ot-radio: no pending tid while resetting the radio");
  }
}

void OtRadioDevice::LowpanSpinelDeviceFidlImpl::Open(OpenCompleter::Sync& completer) {
  ot_radio_obj_.ResetFrameInspectData();
  {
    // Clear the tx queue as those packets are no longer needed after Thread stack is
    // (re)initialized
    fbl::AutoLock lock(&ot_radio_obj_.spi_tx_lock_);
    ot_radio_obj_.spi_tx_queue_.clear();
  }

  // Initialize state before triggering hardware reset to avoid race conditions
  // with `RadioThread`.
  ot_radio_obj_.power_status_ = OT_SPINEL_DEVICE_ON;
  ot_radio_obj_.inbound_allowance_ = 0;
  ot_radio_obj_.outbound_allowance_ = 0;
  ot_radio_obj_.inbound_cnt_ = 0;
  ot_radio_obj_.outbound_cnt_ = 0;
  ot_radio_obj_.inbound_frame_available_ = false;

  zx_status_t res = ot_radio_obj_.Reset();
  if (res == ZX_OK) {
    zxlogf(DEBUG, "open succeed, returning");
    completer.ReplySuccess();
  } else {
    zxlogf(ERROR, "Error in handling FIDL close req: %s, power status: %u",
           zx_status_get_string(res), ot_radio_obj_.power_status_);
    completer.ReplyError(lowpan_spinel_fidl::wire::Error::kUnspecified);
  }
}

void OtRadioDevice::LowpanSpinelDeviceFidlImpl::Close(CloseCompleter::Sync& completer) {
  zx_status_t res = ot_radio_obj_.AssertResetPin();
  if (res == ZX_OK) {
    ot_radio_obj_.power_status_ = OT_SPINEL_DEVICE_OFF;
    completer.ReplySuccess();
  } else {
    zxlogf(ERROR, "Error in handling FIDL close req: %s, power status: %u",
           zx_status_get_string(res), ot_radio_obj_.power_status_);
    completer.ReplyError(lowpan_spinel_fidl::wire::Error::kUnspecified);
  }
}

void OtRadioDevice::LowpanSpinelDeviceFidlImpl::GetMaxFrameSize(
    GetMaxFrameSizeCompleter::Sync& completer) {
  completer.Reply(kMaxFrameSize);
}

void OtRadioDevice::LowpanSpinelDeviceFidlImpl::SendFrame(SendFrameRequestView request,
                                                          SendFrameCompleter::Sync& completer) {
  [[maybe_unused]] auto data_count = request->data.size();
  TRACE_DURATION(kOtRadioTraceCategory, __func__, "ot_radio_obj_.power_status_",
                 ot_radio_obj_.power_status_, "request->data.count()", data_count,
                 "ot_radio_obj_.outbound_allowance", ot_radio_obj_.outbound_allowance_.load(),
                 "ot_radio_obj_.outbound_cnt", ot_radio_obj_.outbound_cnt_);
  if (ot_radio_obj_.power_status_ == OT_SPINEL_DEVICE_OFF) {
    // TODO(https://fxbug.dev/42176667): Consider handling errors instead of ignoring them.
    (void)fidl::WireSendEvent(*ot_radio_obj_.fidl_binding_)
        ->OnError(lowpan_spinel_fidl::wire::Error::kClosed, false);
  } else if (request->data.size() > kMaxFrameSize) {
    // TODO(https://fxbug.dev/42176667): Consider handling errors instead of ignoring them.
    (void)fidl::WireSendEvent(*ot_radio_obj_.fidl_binding_)
        ->OnError(lowpan_spinel_fidl::wire::Error::kOutboundFrameTooLarge, false);
  } else if (ot_radio_obj_.outbound_allowance_ == 0) {
    // Client violates the protocol, close FIDL channel and device. Will not send OnError event.
    ot_radio_obj_.power_status_ = OT_SPINEL_DEVICE_OFF;
    ot_radio_obj_.AssertResetPin();
    ot_radio_obj_.fidl_binding_->Close(ZX_ERR_IO_OVERRUN);
    completer.Close(ZX_ERR_IO_OVERRUN);
  } else {
    // All good, send out the frame.
    zx_status_t res = ot_radio_obj_.RadioPacketTx(request->data.begin(), request->data.size());
    if (res != ZX_OK) {
      zxlogf(ERROR, "Error in handling send frame req: %s", zx_status_get_string(res));
    } else {
      ot_radio_obj_.outbound_allowance_--;
      ot_radio_obj_.outbound_cnt_++;
      zxlogf(DEBUG, "Successfully Txed pkt, total tx pkt %lu", ot_radio_obj_.outbound_cnt_);
      if ((ot_radio_obj_.outbound_cnt_ & 1) == 0) {
        ot_radio_obj_.outbound_allowance_ += kOutboundAllowanceInc;
        (void)fidl::WireSendEvent(*ot_radio_obj_.fidl_binding_)
            ->OnReadyForSendFrames(kOutboundAllowanceInc);
      }
    }
  }
}

void OtRadioDevice::LowpanSpinelDeviceFidlImpl::ReadyToReceiveFrames(
    ReadyToReceiveFramesRequestView request, ReadyToReceiveFramesCompleter::Sync& completer) {
  TRACE_DURATION(kOtRadioTraceCategory, __func__, "request->number_of_frames",
                 request->number_of_frames, "ot_radio_obj_.inbound_allowance",
                 ot_radio_obj_.inbound_allowance_.load());
  zxlogf(DEBUG, "ot-radio: allow to receive %u frame", request->number_of_frames);
  ot_radio_obj_.inbound_allowance_ += request->number_of_frames;
  if (ot_radio_obj_.inbound_allowance_ > 0 && ot_radio_obj_.spinel_framer_.get()) {
    ot_radio_obj_.spinel_framer_->SetInboundAllowanceStatus(true);
    // Trigger the `RadioThread` to poll and process any incoming frames.
    ot_radio_obj_.InvokeInterruptHandler();
  }
}

OtRadioDevice::OtRadioDevice(zx_device_t* device)
    : DeviceType(device), loop_(&kAsyncLoopConfigNoAttachToCurrentThread) {}

void OtRadioDevice::SetChannel(SetChannelRequestView request,
                               SetChannelCompleter::Sync& completer) {
  if (fidl_impl_obj_ != nullptr) {
    zxlogf(ERROR, "ot-radio: channel already set");
    completer.ReplyError(ZX_ERR_ALREADY_BOUND);
    return;
  }
  if (!request->req.is_valid()) {
    completer.ReplyError(ZX_ERR_BAD_HANDLE);
    return;
  }

  // Reset all allowances and status to ensure a clean start for the new channel.
  inbound_allowance_ = 0;
  outbound_allowance_ = 0;
  inbound_cnt_ = 0;
  outbound_cnt_ = 0;
  inbound_frame_available_ = false;

  fidl_impl_obj_ = std::make_unique<LowpanSpinelDeviceFidlImpl>(*this);
  fidl_impl_obj_->Bind(loop_.dispatcher(), std::move(request->req));
  completer.ReplySuccess();
}

zx_status_t OtRadioDevice::StartLoopThread() {
  zxlogf(DEBUG, "Start loop thread");
  zx_status_t status = loop_.StartThread("ot-stack-loop");
  if (status == ZX_OK) {
    thrd_status_.loop_thrd_running = true;
  }
  return status;
}

bool OtRadioDevice::RunUnitTests(void* ctx, zx_device_t* parent, zx_handle_t channel) {
  return driver_unit_test::RunZxTests("OtRadioTests", parent, channel);
}

zx_status_t OtRadioDevice::Init() {
  const char* kSpiFragmentName = "spi";
  zx::result spi = DdkConnectFragmentFidlProtocol<fuchsia_hardware_spi::Service::Device>(
      parent(), kSpiFragmentName);
  if (spi.is_error()) {
    zxlogf(ERROR, "ot-radio %s: failed to acquire spi", __func__);
    return ZX_ERR_NOT_SUPPORTED;
  }
  spi_.Bind(std::move(spi.value()));

  const char* kInterruptGpioFragmentName = "gpio-int";
  zx::result gpio_int = DdkConnectFragmentFidlProtocol<fuchsia_hardware_gpio::Service::Device>(
      parent(), kInterruptGpioFragmentName);
  if (gpio_int.is_error()) {
    zxlogf(ERROR, "Failed to get gpio protocol from fragment %s: %s", kInterruptGpioFragmentName,
           gpio_int.status_string());
    return ZX_ERR_NO_RESOURCES;
  }
  gpio_[OT_RADIO_INT_PIN].Bind(std::move(gpio_int.value()));

  {
    fidl::WireResult result =
        gpio_[OT_RADIO_INT_PIN]->SetBufferMode(fuchsia_hardware_gpio::BufferMode::kInput);
    if (!result.ok()) {
      zxlogf(ERROR, "Failed to send SetBufferMode request to interrupt gpio: %s",
             result.status_string());
      return result.status();
    }
    if (result->is_error()) {
      zxlogf(ERROR, "Failed to configure interrupt gpio to input: %s",
             zx_status_get_string(result->error_value()));
      return result->error_value();
    }
  }

  fidl::Arena arena;
  auto interrupt_config = fuchsia_hardware_gpio::wire::InterruptConfiguration::Builder(arena)
                              .mode(fuchsia_hardware_gpio::InterruptMode::kLevelLow)
                              .Build();
  fidl::WireResult configure_interrupt_result =
      gpio_[OT_RADIO_INT_PIN]->ConfigureInterrupt(interrupt_config);
  if (!configure_interrupt_result.ok()) {
    zxlogf(ERROR, "Failed to send ConfigureInterrupt request to interrupt gpio: %s",
           configure_interrupt_result.status_string());
    return configure_interrupt_result.status();
  }
  if (configure_interrupt_result->is_error()) {
    zxlogf(ERROR, "Failed to configure interrupt gpio: %s",
           zx_status_get_string(configure_interrupt_result->error_value()));
    return configure_interrupt_result->error_value();
  }

  fidl::WireResult interrupt_result = gpio_[OT_RADIO_INT_PIN]->GetInterrupt({});
  if (!interrupt_result.ok()) {
    zxlogf(ERROR, "Failed to send GetInterrupt request to interrupt gpio: %s",
           interrupt_result.status_string());
    return interrupt_result.status();
  }
  if (interrupt_result->is_error()) {
    zxlogf(ERROR, "Failed to get interrupt from interrupt gpio: %s",
           zx_status_get_string(interrupt_result->error_value()));
    return interrupt_result->error_value();
  }
  interrupt_ = std::move(interrupt_result.value()->interrupt);

  const char* kResetGpioFragmentName = "gpio-reset";
  zx::result gpio_reset = DdkConnectFragmentFidlProtocol<fuchsia_hardware_gpio::Service::Device>(
      parent(), kResetGpioFragmentName);
  if (gpio_reset.is_error()) {
    zxlogf(ERROR, "Failed to get gpio protocol from fragment %s: %s", kResetGpioFragmentName,
           gpio_reset.status_string());
    return ZX_ERR_NO_RESOURCES;
  }
  gpio_[OT_RADIO_RESET_PIN].Bind(std::move(gpio_reset.value()));

  {
    fidl::WireResult result =
        gpio_[OT_RADIO_RESET_PIN]->SetBufferMode(fuchsia_hardware_gpio::BufferMode::kOutputHigh);
    if (!result.ok()) {
      zxlogf(ERROR, "Failed to send SetBufferMode request to reset gpio: %s",
             result.status_string());
      return result.status();
    }
    if (result->is_error()) {
      zxlogf(ERROR, "Failed to configure reset gpio to output: %s",
             zx_status_get_string(result->error_value()));
      return result->error_value();
    }
  }

  const char* kBootloaderGpioFragmentName = "gpio-bootloader";
  zx::result gpio_bootloader =
      DdkConnectFragmentFidlProtocol<fuchsia_hardware_gpio::Service::Device>(
          parent(), kBootloaderGpioFragmentName);
  if (gpio_bootloader.is_error()) {
    zxlogf(ERROR, "Failed to get gpio protocol from fragment %s: %s", kBootloaderGpioFragmentName,
           gpio_bootloader.status_string());
    return ZX_ERR_NO_RESOURCES;
  }
  gpio_[OT_RADIO_BOOTLOADER_PIN].Bind(std::move(gpio_bootloader.value()));

  {
    fidl::WireResult result = gpio_[OT_RADIO_BOOTLOADER_PIN]->SetBufferMode(
        fuchsia_hardware_gpio::BufferMode::kOutputHigh);
    if (!result.ok()) {
      zxlogf(ERROR, "Failed to send SetBufferMode request to bootloader gpio: %s",
             result.status_string());
      return result.status();
    }
    if (result->is_error()) {
      zxlogf(ERROR, "Failed to configure bootloader gpio to output: %s",
             zx_status_get_string(result->error_value()));
      return result->error_value();
    }
  }

  spinel_framer_ = std::make_unique<ot::SpinelFramer>();
  spinel_framer_->Init(&spi_);
  pending_tid_ = std::make_unique<std::vector<bool>>(kNumberOfTid, false);
  return ZX_OK;
}

zx_status_t OtRadioDevice::ReadRadioPacket() {
  [[maybe_unused]] auto is_packet_present = spinel_framer_->IsPacketPresent();
  TRACE_DURATION(kOtRadioTraceCategory, __func__, "inbound_allowance_", inbound_allowance_.load(),
                 "is_packet_present", is_packet_present, "spi_rx_buffer_len_", spi_rx_buffer_len_);
  if ((inbound_allowance_.load() > 0) && (spinel_framer_->IsPacketPresent())) {
    spinel_framer_->ReceivePacketFromRadio(spi_rx_buffer_, &spi_rx_buffer_len_);
    if (spi_rx_buffer_len_ > 0) {
      if (thrd_status_.loop_thrd_running) {
        inbound_allowance_--;
        if ((inbound_allowance_ == 0) && spinel_framer_.get()) {
          spinel_framer_->SetInboundAllowanceStatus(false);
        }

        // Deep copy the buffer into a thread-isolated std::vector.
        std::vector<uint8_t> pkt(spi_rx_buffer_, spi_rx_buffer_ + spi_rx_buffer_len_);

        async::PostTask(loop_.dispatcher(), [this, pkt = std::move(pkt)]() {
          this->HandleRadioRxFrame(const_cast<uint8_t*>(pkt.data()),
                                   static_cast<uint16_t>(pkt.size()));
        });
      } else {
        // Loop thread is not running, this is one off case to be handled
        // Results either when running test or getting NCP version
        inbound_allowance_ = 0;
        spinel_framer_->SetInboundAllowanceStatus(false);
      }

      // Signal to driver test, waiting for a response
      sync_completion_signal(&spi_rx_complete_);
    }
    inbound_frame_available_ = true;
  } else {
    inbound_frame_available_ = false;
  }
  return ZX_OK;
}

void OtRadioDevice::InspectInboundFrame(uint8_t* frame_buffer, uint16_t length) {
  uint8_t tid = frame_buffer[0] & 0xF;
  if (tid != 0 && !pending_tid_->at(tid)) {
    zxlogf(ERROR, "ot-radio: receiving non-pending tid %hhu from radio", tid);
  }
  pending_tid_->at(tid) = false;
}

zx_status_t OtRadioDevice::HandleRadioRxFrame(uint8_t* frameBuffer, uint16_t length) {
  TRACE_DURATION(kOtRadioTraceCategory, __func__, "power_status_", power_status_,
                 "inbound_allowance_", inbound_allowance_.load(), "inbound_cnt_", inbound_cnt_);
  zxlogf(DEBUG, "ot-radio: received frame of len:%d", length);
  if (power_status_ == OT_SPINEL_DEVICE_ON) {
    auto data = fidl::VectorView<uint8_t>::FromExternal(frameBuffer, length);
    fidl::Status result = fidl::WireSendEvent(*fidl_binding_)->OnReceiveFrame(std::move(data));
    InspectInboundFrame(frameBuffer, length);
    if (!result.ok()) {
      zxlogf(ERROR, "ot-radio: failed to send OnReceive() event due to %s",
             result.FormatDescription().c_str());
    }
    inbound_cnt_++;
  } else {
    zxlogf(ERROR, "OtRadioDevice::HandleRadioRxFrame(): Radio is off");
  }
  return ZX_OK;
}

void OtRadioDevice::InspectOutboundFrame(uint8_t* frame_buffer, uint16_t length) {
  uint8_t tid = frame_buffer[0] & 0xF;
  if (tid != 0 && pending_tid_->at(tid)) {
    zxlogf(ERROR, "ot-radio: sending pending tid %hhu again to radio", tid);
  }
  pending_tid_->at(tid) = true;
}

zx_status_t OtRadioDevice::RadioPacketTx(uint8_t* frameBuffer, uint16_t length) {
  TRACE_DURATION(kOtRadioTraceCategory, __func__);
  zxlogf(DEBUG, "ot-radio: RadioPacketTx");
  zx_port_packet packet = {PORT_KEY_TX_TO_RADIO, ZX_PKT_TYPE_USER, ZX_OK, {}};
  if (!port_.is_valid()) {
    return ZX_ERR_BAD_STATE;
  }
  std::vector<uint8_t> vec(frameBuffer, frameBuffer + length);
  InspectOutboundFrame(frameBuffer, length);
  {
    fbl::AutoLock lock(&spi_tx_lock_);
    spi_tx_queue_.emplace_back(std::move(vec));
  }
  return port_.queue(&packet);
}

zx_status_t OtRadioDevice::InvokeInterruptHandler() {
  TRACE_DURATION(kOtRadioTraceCategory, __func__);
  zxlogf(INFO, "ot-radio: InvokeInterruptHandler");
  zx_port_packet packet = {PORT_KEY_RADIO_IRQ, ZX_PKT_TYPE_USER, ZX_OK, {}};
  if (!port_.is_valid()) {
    return ZX_ERR_BAD_STATE;
  }
  return port_.queue(&packet);
}

zx::result<bool> OtRadioDevice::IsInterruptAsserted() {
  TRACE_DURATION(kOtRadioTraceCategory, __func__);
  fidl::WireResult result = gpio_[OT_RADIO_INT_PIN]->Read();
  if (!result.ok()) {
    zxlogf(ERROR, "Failed to send Read request to interrupt gpio: %s", result.status_string());
    return zx::error(result.status());
  }
  if (result->is_error()) {
    zxlogf(ERROR, "Failed to read interrupt gpio: %s", zx_status_get_string(result->error_value()));
    return zx::error(result->error_value());
  }
  return zx::ok(!result.value()->value);
}

zx_status_t OtRadioDevice::DriverUnitTestGetNCPVersion() { return GetNCPVersion(); }

void OtRadioDevice::SetMaxInboundAllowance() {
  spinel_framer_->SetInboundAllowanceStatus(true);
  inbound_allowance_ = kOutboundAllowanceInit;
}

zx_status_t OtRadioDevice::GetNCPVersion() {
  SetMaxInboundAllowance();
  uint8_t get_ncp_version_cmd[] = {0x80, 0x02, 0x02};  // HEADER, CMD ID, PROPERTY ID
  // populate TID (lower 4 bits in header)
  get_ncp_version_cmd[0] = (get_ncp_version_cmd[0] & 0xf0) | (kGetNcpVersionTID & 0x0f);
  return RadioPacketTx(get_ncp_version_cmd, sizeof(get_ncp_version_cmd));
}

zx_status_t OtRadioDevice::DriverUnitTestGetResetEvent() {
  SetMaxInboundAllowance();
  return Reset();
}

zx_status_t OtRadioDevice::AssertResetPin() {
  TRACE_DURATION(kOtRadioTraceCategory, __func__);
  zx_status_t status = ZX_OK;
  zxlogf(DEBUG, "ot-radio: assert reset pin");

  fidl::WireResult result =
      gpio_[OT_RADIO_RESET_PIN]->SetBufferMode(fuchsia_hardware_gpio::BufferMode::kOutputLow);
  if (!result.ok()) {
    zxlogf(ERROR, "Failed to send SetBufferMode request to reset gpio: %s", result.status_string());
    return result.status();
  }
  if (result->is_error()) {
    zxlogf(ERROR, "Failed to write to reset gpio: %s", zx_status_get_string(result->error_value()));
    return result->error_value();
  }
  zx::nanosleep(zx::deadline_after(zx::msec(kRcpHardResetPinAssertTimeMs)));
  return status;
}

zx_status_t OtRadioDevice::Reset() {
  zx_status_t status = ZX_OK;
  zxlogf(DEBUG, "ot-radio: reset");

  BeginResetRetryTimer();
  status = AssertResetPin();
  if (status != ZX_OK) {
    zxlogf(ERROR, "ot-radio: assert reset pin failed");
    return status;
  }

  fidl::WireResult result =
      gpio_[OT_RADIO_RESET_PIN]->SetBufferMode(fuchsia_hardware_gpio::BufferMode::kOutputHigh);
  if (!result.ok()) {
    zxlogf(ERROR, "Failed to send SetBufferMode request to reset gpio: %s", result.status_string());
    return result.status();
  }
  if (result->is_error()) {
    zxlogf(ERROR, "Failed to write to reset gpio: %s", zx_status_get_string(result->error_value()));
    return result->error_value();
  }

  return ZX_OK;
}

uint32_t OtRadioDevice::GetTimeoutMs() {
  uint32_t timeout = spinel_framer_->GetTimeoutMs();
  if (!(IsHardResetting() && !inbound_frame_available_.load())) {
    return timeout;
  }
  // If reset is in progress and inbound frame is not available
  // apply short timeout
  timeout = std::min(kRcpBusyPollingDelayMs, timeout);
  return timeout;
}

void OtRadioDevice::HandleResetRetry() {
  if (ShouldRetryReset()) {
    zxlogf(INFO, "ot-radio: rcp unresponsive, firing hard reset again!");
    Reset();
  }
}

void OtRadioDevice::BeginResetRetryTimer() {
  hard_reset_end_.store((zx::clock::get_monotonic() + kRcpHardResetRetryWaitTime).get());
}

bool OtRadioDevice::IsHardResetting() { return hard_reset_end_.load() != 0; }

bool OtRadioDevice::ShouldRetryReset() {
  if (!IsHardResetting()) {
    return false;
  }
  if (inbound_frame_available_.load()) {
    zxlogf(INFO, "ot-radio: rcp back online");
    hard_reset_end_.store(0);
    outbound_allowance_.store(kOutboundAllowanceInit);
    // TODO(https://fxbug.dev/42176667): Consider handling errors instead of ignoring them.
    (void)fidl::WireSendEvent(*fidl_binding_)->OnReadyForSendFrames(kOutboundAllowanceInit);
    return false;
  }
  zx_time_t cur_time_us = zx::clock::get_monotonic().get();
  if (hard_reset_end_.load() > cur_time_us) {
    zxlogf(DEBUG, "ot-radio: waiting for rcp to back online");
    return false;
  }
  return true;
}

zx_status_t OtRadioDevice::RadioThread() {
  zx_status_t status = ZX_OK;
  zxlogf(INFO, "ot-radio: entered thread");

  while (true) {
    TRACE_DURATION_BEGIN(kOtRadioTraceCategory, "OtRadioDevice::RadioThread_port_wait");
    zx_port_packet_t packet = {};
    uint32_t timeout_ms = GetTimeoutMs();
    auto status = port_.wait(zx::deadline_after(zx::msec(timeout_ms)), &packet);
    TRACE_DURATION_END(kOtRadioTraceCategory, "OtRadioDevice::RadioThread_port_wait");
    TRACE_DURATION(kOtRadioTraceCategory, "OtRadioDevice::RadioThread_handle_packet", "status",
                   status);

    if (status == ZX_ERR_TIMED_OUT) {
      spinel_framer_->TrySpiTransaction();
      ReadRadioPacket();
      HandleResetRetry();
      continue;
    } else if (status != ZX_OK) {
      zxlogf(ERROR, "ot-radio: port wait failed: %d", status);
      return thrd_error;
    }

    // TODO(https://fxbug.dev/356951180): We are still using an implementation that potentially
    // starving TX transactions. That requires a fix.
    if (packet.key == PORT_KEY_EXIT_THREAD) {
      break;
    } else if (packet.key == PORT_KEY_RADIO_IRQ) {
      zxlogf(DEBUG, "ot-radio: interrupt");
      // Since we are using level based interrupt triggering, after handling the interrupt
      // and re-enable it, if there are more frames available in SPI, another packet will
      // be queued to `port_` to let this loop keeps polling the frames.
      spinel_framer_->HandleInterrupt();
      ReadRadioPacket();
      HandleResetRetry();
      interrupt_.ack();
    } else if (packet.key == PORT_KEY_TX_TO_RADIO) {
      fbl::AutoLock lock(&spi_tx_lock_);
      if (spi_tx_queue_.size() > 0) {
        zxlogf(DEBUG, "ot-radio: transmitting data of size: %ld", spi_tx_queue_.front().size());
        if (ZX_OK != spinel_framer_->TryBufferTxPacket(spi_tx_queue_.front().data(),
                                                       spi_tx_queue_.front().size())) {
          lock.release();
          // Cannot write to TX buffer at this point because
          // (1) There is a spinel frame in RX buffer pending processing.
          // (2) There is already a spinel frame in TX buffer that hasn't been sent out yet.

          // Handle the packet that is not polled from the RX buffer in spinel framer.
          ReadRadioPacket();
          // Try send out pending frame in TX buffer.
          spinel_framer_->TrySpiTransaction();

          // zx::port is thread-safe: Ok to queue directly to the port.
          zx_port_packet packet = {PORT_KEY_TX_TO_RADIO, ZX_PKT_TYPE_USER, ZX_OK, {}};
          if (!port_.is_valid()) {
            return ZX_ERR_BAD_STATE;
          }
          port_.queue(&packet);
        } else {
          spinel_framer_->TrySpiTransaction();
          spi_tx_queue_.pop_front();
        }
      }
    }
  }
  zxlogf(DEBUG, "ot-radio: exiting");

  return status;
}

zx_status_t OtRadioDevice::CreateBindAndStart(void* ctx, zx_device_t* parent) {
  std::unique_ptr<OtRadioDevice> ot_radio_dev;
  zx_status_t status = Create(ctx, parent, &ot_radio_dev);
  if (status != ZX_OK) {
    return status;
  }

  status = ot_radio_dev->Bind();
  if (status != ZX_OK) {
    return status;
  }
  // device intentionally leaked as it is now held by DevMgr
  auto dev_ptr = ot_radio_dev.release();

  status = dev_ptr->Start();
  if (status != ZX_OK) {
    return status;
  }

  return status;
}

zx_status_t OtRadioDevice::Create(void* ctx, zx_device_t* parent,
                                  std::unique_ptr<OtRadioDevice>* out) {
  auto dev = std::make_unique<OtRadioDevice>(parent);
  zx_status_t status = dev->Init();

  if (status != ZX_OK) {
    zxlogf(ERROR, "ot-radio: Driver init failed %d", status);
    return status;
  }

  *out = std::move(dev);

  return ZX_OK;
}

zx_status_t OtRadioDevice::Bind() {
  zx_status_t status = DdkAdd(ddk::DeviceAddArgs("ot-radio")
                                  .set_proto_id(ZX_PROTOCOL_OT_RADIO)
                                  .set_inspect_vmo(spinel_framer_->InspectorDuplicateVmo()));
  if (status != ZX_OK) {
    zxlogf(ERROR, "ot-radio: Could not create device: %d", status);
    return status;
  } else {
    zxlogf(DEBUG, "ot-radio: Added device");
  }

  return status;
}

zx_status_t OtRadioDevice::CreateAndBindPortToIntr() {
  zx_status_t status = zx::port::create(ZX_PORT_BIND_TO_INTERRUPT, &port_);
  if (status != ZX_OK) {
    zxlogf(ERROR, "ot-radio: port create failed %d", status);
    return status;
  }

  status = interrupt_.bind(port_, PORT_KEY_RADIO_IRQ, 0);
  if (status != ZX_OK) {
    zxlogf(ERROR, "ot-radio: interrupt bind failed %d", status);
    return status;
  }

  return ZX_OK;
}

void OtRadioDevice::StartRadioThread() {
  auto callback = [](void* cookie) {
    return reinterpret_cast<OtRadioDevice*>(cookie)->RadioThread();
  };
  int ret = thrd_create_with_name(&thread_, callback, this, "ot-radio-thread");
  ZX_DEBUG_ASSERT(ret == thrd_success);

  // Set status flag so shutdown can take appropriate action
  // cleared by StopRadioThread
  thrd_status_.radio_thrd_running = true;
}

zx_status_t OtRadioDevice::Start() {
  zx_status_t status = CreateAndBindPortToIntr();
  if (status != ZX_OK) {
    return status;
  }

  StartRadioThread();
  auto cleanup = fit::defer([&]() { ShutDown(); });

#ifdef INTERNAL_ACCESS
  // Update the NCP Firmware if new version is available
  bool update_fw = false;
  status = CheckFWUpdateRequired(&update_fw);
  if (status != ZX_OK) {
    zxlogf(ERROR, "ot-radio: CheckFWUpdateRequired failed with status: %d", status);
    return status;
  }

  if (update_fw) {
    // Print as it may be useful, expected to be rare occurrence,
    zxlogf(INFO, "ot-radio: Will start FW update");

    // Stop the loop for handling port events, so port can be used by bootloader
    StopRadioThread();

    // Update firmware here
    OtRadioDeviceBootloader dev_bl(this);
    OtRadioBlResult result = dev_bl.UpdateRadioFirmware();
    if (result != BL_RET_SUCCESS) {
      zxlogf(ERROR, "ot-radio: radio firmware update failed with %d. Last zx_status %d", result,
             dev_bl.GetLastZxStatus());
      return ZX_ERR_INTERNAL;
    }

    zxlogf(INFO, "ot-radio: FW update done successfully");

    // Restart the Radio thread:
    StartRadioThread();
  } else {
    zxlogf(DEBUG, "ot-radio: NCP firmware is already up-to-date");
  }
#endif

  status = StartLoopThread();
  if (status != ZX_OK) {
    zxlogf(ERROR, "ot-radio: Could not start loop thread");
    return status;
  }

  zxlogf(DEBUG, "ot-radio: Started thread");

  cleanup.cancel();

  return status;
}

#ifdef INTERNAL_ACCESS
zx_status_t OtRadioDevice::CheckFWUpdateRequired(bool* update_fw) {
  *update_fw = false;

  // Get the new firmware version:
  std::string new_fw_version = GetNewFirmwareVersion();
  if (new_fw_version.size() == 0) {
    // Invalid version string indicates invalid firmware
    zxlogf(ERROR, "ot-radio: The new firmware is invalid");
    *update_fw = false;
    // Return error instead of ZX_OK and just not-updating,
    // may point to some bug
    return ZX_ERR_NO_RESOURCES;
  }

  int attempts;
  bool response_received = false;

  // Now get the ncp version
  auto status = GetNCPVersion();
  if (status != ZX_OK) {
    zxlogf(ERROR, "ot-radio: get ncp version failed with status: %d", status);
    return status;
  }

  for (attempts = 0; attempts < kGetNcpVersionMaxRetries; attempts++) {
    zxlogf(DEBUG, "ot-radio: waiting for response for GetNCPVersion cmd, attempt: %d / %d",
           attempts + 1, kGetNcpVersionMaxRetries);

    // Simply update the allowance for each attempt. Radio will send
    // response to GetNCPVersion to us eventually. Ignore any response to
    // earlier commands.
    SetMaxInboundAllowance();

    // Wait for response to arrive, signaled by spi_rx_complete_
    status = sync_completion_wait(&spi_rx_complete_, ZX_SEC(10));
    sync_completion_reset(&spi_rx_complete_);
    if (status != ZX_OK) {
      zxlogf(ERROR,
             "ot-radio: sync_completion_wait failed with status: %d, \
         this means firmware may be behaving incorrectly\n",
             status);
      // We want to update fw in this case
      *update_fw = true;
      return ZX_OK;
    }

    // Check for matching TID in spinel header
    if ((spi_rx_buffer_[0] & 0xf) == kGetNcpVersionTID) {
      response_received = true;
      break;
    }
  }

  if (!response_received) {
    zxlogf(ERROR, "ot-radio: no matching response is received for get ncp version command.");
    zxlogf(ERROR, "ot-radio: updating the the firmware");
    // This can again mean bad firmware, so update the firmware:
    *update_fw = true;
    return ZX_OK;
  }

  // Response is received, copy it to the string cur_fw_version
  // First make sure that last character is null
  ZX_DEBUG_ASSERT(spi_rx_buffer_len_ <= kMaxFrameSize);
  spi_rx_buffer_[spi_rx_buffer_len_ - 1] = '\0';
  zxlogf(DEBUG, "ot-radio: response received size = %d, value : %s", spi_rx_buffer_len_,
         reinterpret_cast<char*>(&(spi_rx_buffer_[3])));
  std::string cur_fw_version;
  cur_fw_version.assign(reinterpret_cast<char*>(&(spi_rx_buffer_[3])));
  zxlogf(INFO, "ot-radio: cur_fw_version: %s", cur_fw_version.c_str());

  // We want to update firmware if the versions don't match
  *update_fw = (cur_fw_version.compare(new_fw_version) != 0);
  if (*update_fw) {
    zxlogf(INFO, "ot-radio: fw update required, new_fw_version: %s", new_fw_version.c_str());
  }
  return ZX_OK;
}
#endif

void OtRadioDevice::DdkRelease() { delete this; }

void OtRadioDevice::DdkUnbind(ddk::UnbindTxn txn) {
  ShutDown();
  txn.Reply();
}

void OtRadioDevice::StopRadioThread() {
  if (thrd_status_.radio_thrd_running) {
    zx_port_packet packet = {PORT_KEY_EXIT_THREAD, ZX_PKT_TYPE_USER, ZX_OK, {}};
    port_.queue(&packet);
    thrd_join(thread_, NULL);
    thrd_status_.radio_thrd_running = false;
  }
}

void OtRadioDevice::StopLoopThread() {
  if (thrd_status_.loop_thrd_running) {
    loop_.Shutdown();
    thrd_status_.loop_thrd_running = false;
  }
}

zx_status_t OtRadioDevice::ShutDown() {
  StopRadioThread();

  fidl::WireResult result = gpio_[OT_RADIO_INT_PIN]->ReleaseInterrupt();
  if (!result.ok()) {
    zxlogf(ERROR, "Failed to send ReleaseInterrupt request to interrupt gpio: %s",
           result.status_string());
    return result.status();
  }
  if (result->is_error()) {
    zxlogf(ERROR, "Failed to release interrupt for interrupt gpio: %s",
           zx_status_get_string(result->error_value()));
    return result->error_value();
  }

  interrupt_.destroy();

  StopLoopThread();

  return ZX_OK;
}

static constexpr zx_driver_ops_t driver_ops = []() {
  zx_driver_ops_t ops = {};
  ops.version = DRIVER_OPS_VERSION;
  ops.bind = OtRadioDevice::CreateBindAndStart;
  ops.run_unit_tests = OtRadioDevice::RunUnitTests;
  return ops;
}();

}  // namespace ot

ZIRCON_DRIVER(ot, ot::driver_ops, "ot_radio", "0.1");
