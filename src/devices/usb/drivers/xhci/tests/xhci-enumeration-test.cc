// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Theory of operation:
// This file contains unit tests for xhci-enumeration.cc.
// In order to test this code, it is necessary to fake out
// everything that it interacts with (most of which is in usb-xhci.cc,
// while some of it is event ring related).
// Due to the use of TRBs to pass asynchronous state around (which are normally
// owned by the event ring), the test harness ends up owning all of the TRBs
// associated with a TRBContext. The test harness is responsible for the creation
// and destruction of TRBs, since there is no actual event ring present (normally these would
// reside inside of a DMA buffer that is shared with hardware.
// In the future -- we may want to remove this tight coupling, but this is difficult
// due to inability to pass un-instantiated templates between different object files in C++.
// This may later be solved by C++ modules, at which point we can have each callback return a
// unique template instantiation instead of passing TRBs around to everything (resulting in
// tight coupling between the event ring, UsbXhci class, the transfer ring, and the enumerator).

#include "src/devices/usb/drivers/xhci/xhci-enumeration.h"

#include <fuchsia/hardware/usb/descriptor/c/banjo.h>
#include <lib/driver/mmio/cpp/mmio-buffer.h>
#include <lib/driver/mmio/testing/cpp/test-helper.h>
#include <lib/fpromise/bridge.h>
#include <lib/fpromise/promise.h>
#include <lib/mmio-ptr/fake.h>
#include <zircon/syscalls.h>

#include <fake-dma-buffer/fake-dma-buffer.h>

#include "src/devices/usb/drivers/xhci/tests/test-env.h"
#include "src/devices/usb/drivers/xhci/xhci-event-ring.h"

namespace usb_xhci {

constexpr size_t kMaxSlabs = -1;
constexpr bool kAllocInitial = true;

struct FakeTRB : TRB {
  enum class Op {
    DisableSlot,
    EnableSlot,
    SetMaxPacketSize,
    AddressDevice,
    OnlineDevice,
    ShutdownController,
    SetDeviceInformation,
    Timeout,
  } Op;
  uint32_t slot;
  uint8_t max_packet_size;
  uint16_t port;
  usb_speed_t speed;
  zx_status_t status;
  zx::time deadline;
  std::optional<HubInfo> hub_info;
  bool bsr;
  static std::unique_ptr<FakeTRB> FromTRB(TRB* trb) {
    return std::unique_ptr<FakeTRB>(static_cast<FakeTRB*>(trb));
  }
};

struct TestState {
  TestState() : trb_context_allocator_(kMaxSlabs, kAllocInitial) {}
  fbl::DoublyLinkedList<std::unique_ptr<TRBContext>> pending_operations;
  AllocatorType trb_context_allocator_;
  uint64_t token = 0;
  uint8_t slot = 1;
  usb_speed_t speeds[32] = {};

  ~TestState() {
    while (!pending_operations.is_empty()) {
      auto op = pending_operations.pop_front();
      if (op->completer.has_value()) {
        op->completer->complete_error(ZX_ERR_IO_NOT_PRESENT);
      }
    }
  }
};

class EnumerationTests : public ::testing::Test {
 public:
  void SetUp() override {
    ASSERT_TRUE(driver_test()
                    .StartDriverWithCustomStartArgs([&](fdf::DriverStartArgs& args) {
                      xhci_config::Config fake_config;
                      fake_config.enable_suspend() = false;
                      args.config(fake_config.ToVmo());
                    })
                    .is_ok());
    ASSERT_OK(driver_test().driver()->TestInit(this));
  }

  void TearDown() override { ASSERT_TRUE(driver_test().StopDriver().is_ok()); }

  fdf_testing::ForegroundDriverTest<EmptyTestConfig>& driver_test() { return driver_test_; }
  TestState& state() { return state_; }

  std::optional<HubInfo> TestHubInfo(uint8_t hub_depth, uint8_t hub_slot, uint8_t hub_port,
                                     usb_speed_t speed, bool multi_tt) {
    auto hub_state = fbl::MakeRefCounted<DeviceState>(hub_slot - 1, driver_test().driver());
    {
      fbl::AutoLock _(&hub_state->transaction_lock());
      hub_state->SetDeviceInformation(hub_slot, hub_port, std::nullopt);
    }
    return HubInfo(std::move(hub_state), hub_depth, speed, multi_tt);
  }

  void VerifyHubInfo(std::optional<HubInfo>& hub_info, uint8_t hub_depth, uint8_t hub_slot,
                     uint8_t hub_port, usb_speed_t speed, bool multi_tt) {
    ASSERT_EQ(hub_info->hub_depth, hub_depth);
    ASSERT_EQ(hub_info->hub_state->GetSlot(), hub_slot);
    ASSERT_EQ(hub_info->hub_state->GetPort(), hub_port);
    ASSERT_EQ(hub_info->hub_speed, speed);
    ASSERT_EQ(hub_info->multi_tt, multi_tt);
  }

 private:
  fdf_testing::ForegroundDriverTest<EmptyTestConfig> driver_test_;
  TestState state_;
};

// EventRing Methods
void EventRing::ScheduleTask(fpromise::promise<void, zx_status_t> promise) {
  SchedulePromiseWithXhciExecutorPolicy(executor_, hci_, std::move(promise));
}

void EventRing::RunUntilIdle() { executor_.run_until_idle(); }

// Interrupter Methods
zx_status_t Interrupter::Init(uint16_t interrupter, size_t page_size, fdf::MmioBuffer* buffer,
                              const RuntimeRegisterOffset& offset, uint32_t erst_max,
                              DoorbellOffset doorbell_offset, UsbXhci* hci, HCCPARAMS1 hcc_params_1,
                              uint64_t* dcbaa) {
  hci_ = hci;
  return ZX_OK;
}

zx_status_t Interrupter::Start(const RuntimeRegisterOffset& offset,
                               fdf::MmioView interrupter_regs) {
  return ZX_OK;
}

fpromise::promise<void, zx_status_t> Interrupter::Timeout(zx::time deadline) {
  fpromise::bridge<TRB*, zx_status_t> bridge;
  auto& state = hci_->GetTestHarness<EnumerationTests>()->state();
  auto context = state.trb_context_allocator_.New();
  FakeTRB* trb = new FakeTRB();
  trb->Op = FakeTRB::Op::Timeout;
  trb->deadline = deadline;
  context->trb = trb;
  context->completer = std::move(bridge.completer);
  state.pending_operations.push_back(std::move(context));
  return bridge.consumer.promise().discard_value();
}

// UsbXhci Methods
void UsbXhci::SetDeviceInformation(uint8_t slot, uint8_t port, const std::optional<HubInfo>& hub) {
  auto& state = GetTestHarness<EnumerationTests>()->state();
  auto context = state.trb_context_allocator_.New();
  FakeTRB* trb = new FakeTRB();
  trb->Op = FakeTRB::Op::SetDeviceInformation;
  trb->slot = slot;
  trb->port = port;
  trb->hub_info = hub;
  // NOTE: The TRB for the purposes of the test is owned by our test harness.
  // In a real environment, this would be owned by the transfer ring (it would be
  // a TRB that would be inside of a DMA buffer, since it is shared between the
  // device and the CPU)
  context->trb = trb;
  state.pending_operations.push_back(std::move(context));

  // slot must exist in device_state_ so it's reported as connected.
  device_state_[slot - 1] = fbl::MakeRefCounted<DeviceState>(slot - 1, this);
}

std::optional<usb_speed_t> UsbXhci::GetDeviceSpeed(uint8_t slot) {
  auto& state = GetTestHarness<EnumerationTests>()->state();
  return state.speeds[slot - 1];
}

zx_status_t UsbXhci::DeviceOnline(uint32_t slot, uint16_t port, usb_speed_t speed) {
  auto& state = GetTestHarness<EnumerationTests>()->state();
  auto context = state.trb_context_allocator_.New();
  FakeTRB* trb = new FakeTRB();
  trb->Op = FakeTRB::Op::OnlineDevice;
  trb->slot = slot;
  trb->port = port;
  trb->speed = speed;
  // NOTE: The TRB for the purposes of the test is owned by our test harness.
  // In a real environment, this would be owned by the transfer ring (it would be
  // a TRB that would be inside of a DMA buffer, since it is shared between the
  // device and the CPU)
  context->trb = trb;
  state.pending_operations.push_back(std::move(context));
  return ZX_OK;
}

void UsbXhci::Shutdown(zx_status_t status) {
  auto& state = GetTestHarness<EnumerationTests>()->state();
  auto context = state.trb_context_allocator_.New();
  FakeTRB* trb = new FakeTRB();
  trb->Op = FakeTRB::Op::ShutdownController;
  trb->status = status;
  // NOTE: The TRB for the purposes of the test is owned by our test harness.
  // In a real environment, this would be owned by the transfer ring (it would be
  // a TRB that would be inside of a DMA buffer, since it is shared between the
  // device and the CPU)
  context->trb = trb;
  state.pending_operations.push_back(std::move(context));
}

TRBPromise UsbXhci::AddressDeviceCommand(uint8_t slot_id, uint8_t port_id,
                                         std::optional<HubInfo> hub_info, bool bsr) {
  fpromise::bridge<TRB*, zx_status_t> bridge;
  auto& state = GetTestHarness<EnumerationTests>()->state();
  auto context = state.trb_context_allocator_.New();
  FakeTRB* trb = new FakeTRB();
  trb->Op = FakeTRB::Op::AddressDevice;
  trb->slot = slot_id;
  trb->port = port_id;
  trb->hub_info = std::move(hub_info);
  trb->bsr = bsr;
  // NOTE: The TRB for the purposes of the test is owned by our test harness.
  // In a real environment, this would be owned by the transfer ring (it would be
  // a TRB that would be inside of a DMA buffer, since it is shared between the
  // device and the CPU)
  context->trb = trb;
  context->completer = std::move(bridge.completer);
  state.pending_operations.push_back(std::move(context));
  return bridge.consumer.promise();
}

TRBPromise UsbXhci::AddressDeviceCommand(uint8_t slot_id, uint8_t port_id) {
  fpromise::bridge<TRB*, zx_status_t> bridge;
  auto& state = GetTestHarness<EnumerationTests>()->state();
  auto context = state.trb_context_allocator_.New();
  FakeTRB* trb = new FakeTRB();
  trb->Op = FakeTRB::Op::AddressDevice;
  trb->slot = slot_id;
  trb->port = port_id;
  // NOTE: The TRB for the purposes of the test is owned by our test harness.
  // In a real environment, this would be owned by the transfer ring (it would be
  // a TRB that would be inside of a DMA buffer, since it is shared between the
  // device and the CPU)
  context->trb = trb;
  context->completer = std::move(bridge.completer);
  state.pending_operations.push_back(std::move(context));
  return bridge.consumer.promise();
}

TRBPromise UsbXhci::SetMaxPacketSizeCommand(uint8_t slot_id, uint8_t bMaxPacketSize0) {
  fpromise::bridge<TRB*, zx_status_t> bridge;
  auto& state = GetTestHarness<EnumerationTests>()->state();
  auto context = state.trb_context_allocator_.New();
  FakeTRB* trb = new FakeTRB();
  trb->Op = FakeTRB::Op::SetMaxPacketSize;
  trb->slot = slot_id;
  trb->max_packet_size = bMaxPacketSize0;
  // NOTE: The TRB for the purposes of the test is owned by our test harness.
  // In a real environment, this would be owned by the transfer ring (it would be
  // a TRB that would be inside of a DMA buffer, since it is shared between the
  // device and the CPU)
  context->trb = trb;
  context->completer = std::move(bridge.completer);
  state.pending_operations.push_back(std::move(context));
  return bridge.consumer.promise();
}

TRBPromise UsbXhci::EnableSlotCommand() {
  fpromise::bridge<TRB*, zx_status_t> bridge;
  auto& state = GetTestHarness<EnumerationTests>()->state();
  auto context = state.trb_context_allocator_.New();
  FakeTRB* trb = new FakeTRB();
  trb->Op = FakeTRB::Op::EnableSlot;
  trb->slot = state.slot;
  state.slot++;
  // NOTE: The TRB for the purposes of the test is owned by our test harness.
  // In a real environment, this would be owned by the transfer ring (it would be
  // a TRB that would be inside of a DMA buffer, since it is shared between the
  // device and the CPU)
  context->trb = trb;
  context->completer = std::move(bridge.completer);
  state.pending_operations.push_back(std::move(context));
  return bridge.consumer.promise();
}

fpromise::promise<void, zx_status_t> UsbXhci::DisableSlotCommand(uint32_t slot) {
  fpromise::bridge<TRB*, zx_status_t> bridge;
  auto& state = GetTestHarness<EnumerationTests>()->state();
  auto context = state.trb_context_allocator_.New();
  FakeTRB* trb = new FakeTRB();
  trb->Op = FakeTRB::Op::DisableSlot;
  trb->slot = slot;
  // NOTE: The TRB for the purposes of the test is owned by our test harness.
  // In a real environment, this would be owned by the transfer ring (it would be
  // a TRB that would be inside of a DMA buffer, since it is shared between the
  // device and the CPU)
  context->trb = trb;
  context->completer = std::move(bridge.completer);
  state.pending_operations.push_back(std::move(context));
  return bridge.consumer.promise().discard_value();
}

void UsbXhci::UsbHciRequestQueue(usb_request_t* usb_request,
                                 const usb_request_complete_callback_t* complete_cb) {
  auto& state = GetTestHarness<EnumerationTests>()->state();
  auto context = state.trb_context_allocator_.New();
  context->request = Request(usb_request, *complete_cb, sizeof(usb_request_t));
  context->token = state.token;
  state.pending_operations.push_back(std::move(context));
}

zx_status_t UsbXhci::UsbHciCancelAll(uint32_t device_id, uint8_t ep_address) {
  return ZX_ERR_NOT_SUPPORTED;
}

// Interrupter takes a View which needs to refer back to a MmioBuffer that
// hasn't gone out of scope, so since there's no other scaffolding here we're
// going to just hold it here.
fdf::MmioBuffer kInterruptorMmio = fdf_testing::CreateMmioBuffer(zx_system_get_page_size());

zx::result<> UsbXhci::Init(std::unique_ptr<dma_buffer::BufferFactory> buffer_factory) {
  buffer_factory_ = std::move(buffer_factory);

  interrupters_ = fbl::MakeArray<Interrupter>(1);
  interrupters_[0].Init(0, 0, nullptr, {}, 0, {}, this, {}, nullptr);
  interrupters_[0].Start(RuntimeRegisterOffset::Get().FromValue(0), kInterruptorMmio.View(0));
  device_state_ = fbl::MakeArray<fbl::RefPtr<DeviceState>>(32);
  return zx::ok();
}

fpromise::promise<OwnedRequest, void> UsbXhci::UsbHciRequestQueue(OwnedRequest usb_request) {
  fpromise::bridge<OwnedRequest, void> bridge;
  usb_request_complete_callback_t completion;
  completion.callback = [](void* ctx, usb_request_t* req) {
    auto completer = static_cast<fpromise::completer<OwnedRequest, void>*>(ctx);
    completer->complete_ok(OwnedRequest(req, sizeof(usb_request_t)));
    delete completer;
  };
  completion.ctx = new fpromise::completer<OwnedRequest, void>(std::move(bridge.completer));
  UsbHciRequestQueue(usb_request.take(), &completion);
  return bridge.consumer.promise().box();
}

fpromise::promise<void, zx_status_t> UsbXhci::Timeout(uint16_t target_interrupter,
                                                      zx::time deadline) {
  return interrupter(target_interrupter).Timeout(deadline);
}

void UsbXhci::CreateDeviceInspectNode(uint32_t slot, uint16_t vendor_id, uint16_t product_id) {}

// TransferRing Methods
zx_status_t TransferRing::DeinitIfActive() { return ZX_OK; }

TEST_F(EnumerationTests, EnableSlotCommandPassesThroughFailureCode) {
  std::optional<HubInfo> hub_info;
  constexpr uint8_t kPort = 5;
  auto enumeration_task = EnumerateDevice(driver_test().driver(), kPort, std::move(hub_info));
  auto enable_slot_task = state().pending_operations.pop_front();
  auto enum_slot_trb = FakeTRB::FromTRB(enable_slot_task->trb);
  ASSERT_EQ(enum_slot_trb->Op, FakeTRB::Op::EnableSlot);
  enable_slot_task->completer->complete_error(ZX_ERR_UNAVAILABLE);
  ASSERT_EQ(driver_test().driver()->RunSynchronously(0, std::move(enumeration_task)),
            ZX_ERR_UNAVAILABLE);
}

TEST_F(EnumerationTests, EnableSlotCommandReturnsIOErrorOnFailure) {
  std::optional<HubInfo> hub_info;
  constexpr uint8_t kPort = 5;
  auto enumeration_task = EnumerateDevice(driver_test().driver(), kPort, std::move(hub_info));
  auto enable_slot_task = state().pending_operations.pop_front();
  auto enum_slot_trb = FakeTRB::FromTRB(enable_slot_task->trb);
  ASSERT_EQ(enum_slot_trb->Op, FakeTRB::Op::EnableSlot);
  reinterpret_cast<CommandCompletionEvent*>(enum_slot_trb.get())
      ->set_CompletionCode(CommandCompletionEvent::UndefinedError);
  enable_slot_task->completer->complete_ok(enum_slot_trb.get());
  ASSERT_EQ(driver_test().driver()->RunSynchronously(0, std::move(enumeration_task)), ZX_ERR_IO);
}

TEST_F(EnumerationTests, EnableSlotCommandSetsDeviceInformationOnSuccess) {
  constexpr uint8_t kPort = 5;
  constexpr uint8_t kHubDepth = 52;
  constexpr uint8_t kHubSlot = 28;
  constexpr uint8_t kHubPort = 39;
  constexpr usb_speed_t kSpeed = USB_SPEED_HIGH;
  constexpr bool kMultiTT = false;
  auto enumeration_task = EnumerateDevice(
      driver_test().driver(), kPort, TestHubInfo(kHubDepth, kHubSlot, kHubPort, kSpeed, kMultiTT));
  auto enable_slot_task = state().pending_operations.pop_front();
  auto enum_slot_trb = FakeTRB::FromTRB(enable_slot_task->trb);
  ASSERT_EQ(enum_slot_trb->Op, FakeTRB::Op::EnableSlot);
  reinterpret_cast<CommandCompletionEvent*>(enum_slot_trb.get())
      ->set_CompletionCode(CommandCompletionEvent::Success);
  reinterpret_cast<CommandCompletionEvent*>(enum_slot_trb.get())->set_SlotID(1);
  enable_slot_task->completer->complete_ok(enum_slot_trb.get());
  driver_test().driver()->ScheduleTask(0, std::move(enumeration_task));
  driver_test().driver()->RunUntilIdle(0);
  auto device_information = FakeTRB::FromTRB(state().pending_operations.pop_front()->trb);
  ASSERT_EQ(device_information->Op, FakeTRB::Op::SetDeviceInformation);
  VerifyHubInfo(device_information->hub_info, kHubDepth, kHubSlot, kHubPort, kSpeed, kMultiTT);
  ASSERT_EQ(device_information->port, kPort);
  ASSERT_EQ(device_information->slot, 1UL);
  driver_test().driver()->RunUntilIdle(0);
  auto address_device_op = FakeTRB::FromTRB(state().pending_operations.pop_front()->trb);
  ASSERT_EQ(address_device_op->Op, FakeTRB::Op::AddressDevice);
}

TEST_F(EnumerationTests, AddressDeviceCommandPassesThroughFailureCode) {
  // EnableSlot
  constexpr uint8_t kPort = 5;
  constexpr uint8_t kHubDepth = 52;
  constexpr uint8_t kHubSlot = 28;
  constexpr uint8_t kHubPort = 39;
  constexpr usb_speed_t kSpeed = USB_SPEED_HIGH;
  constexpr bool kMultiTT = false;
  zx_status_t completion_code = -1;
  auto enumeration_task =
      EnumerateDevice(driver_test().driver(), kPort,
                      TestHubInfo(kHubDepth, kHubSlot, kHubPort, kSpeed, kMultiTT))
          .then([&](fpromise::result<void, zx_status_t>& result) {
            if (result.is_ok()) {
              completion_code = ZX_OK;
            } else {
              completion_code = result.error();
            }
            return result;
          });
  auto enable_slot_task = state().pending_operations.pop_front();
  auto enum_slot_trb = FakeTRB::FromTRB(enable_slot_task->trb);
  ASSERT_EQ(enum_slot_trb->Op, FakeTRB::Op::EnableSlot);
  reinterpret_cast<CommandCompletionEvent*>(enum_slot_trb.get())
      ->set_CompletionCode(CommandCompletionEvent::Success);
  reinterpret_cast<CommandCompletionEvent*>(enum_slot_trb.get())->set_SlotID(1);
  enable_slot_task->completer->complete_ok(enum_slot_trb.get());
  driver_test().driver()->ScheduleTask(0, std::move(enumeration_task));
  driver_test().driver()->RunUntilIdle(0);
  auto device_information = FakeTRB::FromTRB(state().pending_operations.pop_front()->trb);
  ASSERT_EQ(device_information->Op, FakeTRB::Op::SetDeviceInformation);
  VerifyHubInfo(device_information->hub_info, kHubDepth, kHubSlot, kHubPort, kSpeed, kMultiTT);
  ASSERT_EQ(device_information->port, kPort);
  ASSERT_EQ(device_information->slot, 1UL);
  driver_test().driver()->RunUntilIdle(0);

  // AddressDevice
  auto address_device = state().pending_operations.pop_front();
  auto address_device_op = FakeTRB::FromTRB(address_device->trb);
  ASSERT_EQ(address_device_op->Op, FakeTRB::Op::AddressDevice);
  ASSERT_EQ(address_device_op->slot, 1UL);
  ASSERT_EQ(address_device_op->port, kPort);
  VerifyHubInfo(address_device_op->hub_info, kHubDepth, kHubSlot, kHubPort, kSpeed, kMultiTT);
  address_device->completer->complete_error(ZX_ERR_IO_OVERRUN);
  driver_test().driver()->RunUntilIdle(0);
  auto disable_trb = FakeTRB::FromTRB(state().pending_operations.pop_front()->trb);
  ASSERT_EQ(disable_trb->Op, FakeTRB::Op::DisableSlot);
  ASSERT_EQ(disable_trb->slot, 1UL);
  ASSERT_EQ(completion_code, ZX_ERR_IO_OVERRUN);
}

TEST_F(EnumerationTests, AddressDeviceCommandReturnsErrorOnFailure) {
  // EnableSlot
  constexpr uint8_t kPort = 5;
  constexpr uint8_t kHubDepth = 52;
  constexpr uint8_t kHubSlot = 28;
  constexpr uint8_t kHubPort = 39;
  constexpr usb_speed_t kSpeed = USB_SPEED_HIGH;
  constexpr bool kMultiTT = false;
  zx_status_t completion_code = -1;
  auto enumeration_task =
      EnumerateDevice(driver_test().driver(), kPort,
                      TestHubInfo(kHubDepth, kHubSlot, kHubPort, kSpeed, kMultiTT))
          .then([&](fpromise::result<void, zx_status_t>& result) {
            if (result.is_ok()) {
              completion_code = ZX_OK;
            } else {
              completion_code = result.error();
            }
            return result;
          });
  auto enable_slot_task = state().pending_operations.pop_front();
  auto enum_slot_trb = FakeTRB::FromTRB(enable_slot_task->trb);
  ASSERT_EQ(enum_slot_trb->Op, FakeTRB::Op::EnableSlot);
  reinterpret_cast<CommandCompletionEvent*>(enum_slot_trb.get())
      ->set_CompletionCode(CommandCompletionEvent::Success);
  reinterpret_cast<CommandCompletionEvent*>(enum_slot_trb.get())->set_SlotID(1);
  enable_slot_task->completer->complete_ok(enum_slot_trb.get());
  driver_test().driver()->ScheduleTask(0, std::move(enumeration_task));
  driver_test().driver()->RunUntilIdle(0);
  auto device_information = FakeTRB::FromTRB(state().pending_operations.pop_front()->trb);
  ASSERT_EQ(device_information->Op, FakeTRB::Op::SetDeviceInformation);
  VerifyHubInfo(device_information->hub_info, kHubDepth, kHubSlot, kHubPort, kSpeed, kMultiTT);
  ASSERT_EQ(device_information->port, kPort);
  ASSERT_EQ(device_information->slot, 1UL);
  driver_test().driver()->RunUntilIdle(0);

  // AddressDevice
  auto address_device = state().pending_operations.pop_front();
  auto address_device_op = FakeTRB::FromTRB(address_device->trb);
  ASSERT_EQ(address_device_op->Op, FakeTRB::Op::AddressDevice);
  ASSERT_EQ(address_device_op->slot, 1UL);
  ASSERT_EQ(address_device_op->port, kPort);
  VerifyHubInfo(address_device_op->hub_info, kHubDepth, kHubSlot, kHubPort, kSpeed, kMultiTT);
  reinterpret_cast<CommandCompletionEvent*>(address_device_op.get())
      ->set_CompletionCode(CommandCompletionEvent::Stopped);
  address_device->completer->complete_ok(address_device_op.get());
  driver_test().driver()->RunUntilIdle(0);
  auto disable_trb = FakeTRB::FromTRB(state().pending_operations.pop_front()->trb);
  ASSERT_EQ(disable_trb->Op, FakeTRB::Op::DisableSlot);
  ASSERT_EQ(disable_trb->slot, 1UL);
  ASSERT_EQ(completion_code, ZX_ERR_IO);
}

TEST_F(EnumerationTests, AddressDeviceCommandShouldOnlineDeviceUponCompletion) {
  // EnableSlot
  constexpr uint8_t kPort = 5;
  constexpr uint8_t kHubDepth = 52;
  constexpr uint8_t kHubSlot = 28;
  constexpr uint8_t kHubPort = 39;
  constexpr usb_speed_t kSpeed = USB_SPEED_HIGH;
  constexpr bool kMultiTT = false;
  state().speeds[0] = kSpeed;
  zx_status_t completion_code = -1;
  auto enumeration_task =
      EnumerateDevice(driver_test().driver(), kPort,
                      TestHubInfo(kHubDepth, kHubSlot, kHubPort, kSpeed, kMultiTT))
          .then([&](fpromise::result<void, zx_status_t>& result) {
            if (result.is_ok()) {
              completion_code = ZX_OK;
            } else {
              completion_code = result.error();
            }
            return result;
          });
  auto enable_slot_task = state().pending_operations.pop_front();
  auto enum_slot_trb = FakeTRB::FromTRB(enable_slot_task->trb);
  ASSERT_EQ(enum_slot_trb->Op, FakeTRB::Op::EnableSlot);
  reinterpret_cast<CommandCompletionEvent*>(enum_slot_trb.get())
      ->set_CompletionCode(CommandCompletionEvent::Success);
  reinterpret_cast<CommandCompletionEvent*>(enum_slot_trb.get())->set_SlotID(1);
  enable_slot_task->completer->complete_ok(enum_slot_trb.get());
  driver_test().driver()->ScheduleTask(0, std::move(enumeration_task));
  driver_test().driver()->RunUntilIdle(0);
  auto device_information = FakeTRB::FromTRB(state().pending_operations.pop_front()->trb);
  ASSERT_EQ(device_information->Op, FakeTRB::Op::SetDeviceInformation);
  VerifyHubInfo(device_information->hub_info, kHubDepth, kHubSlot, kHubPort, kSpeed, kMultiTT);
  ASSERT_EQ(device_information->port, kPort);
  ASSERT_EQ(device_information->slot, 1UL);

  // AddressDevice
  auto address_device = state().pending_operations.pop_front();
  auto address_device_op = FakeTRB::FromTRB(address_device->trb);
  ASSERT_EQ(address_device_op->Op, FakeTRB::Op::AddressDevice);
  ASSERT_EQ(address_device_op->slot, 1UL);
  ASSERT_EQ(address_device_op->port, kPort);
  VerifyHubInfo(address_device_op->hub_info, kHubDepth, kHubSlot, kHubPort, kSpeed, kMultiTT);
  reinterpret_cast<CommandCompletionEvent*>(address_device_op.get())
      ->set_CompletionCode(CommandCompletionEvent::Success);
  address_device->completer->complete_ok(address_device_op.get());
  driver_test().driver()->RunUntilIdle(0);
  // Timeout
  auto timeout = state().pending_operations.pop_front();
  ASSERT_TRUE(FakeTRB::FromTRB(timeout->trb)->deadline.get());
  timeout->completer->complete_ok(address_device_op.get());
  driver_test().driver()->RunUntilIdle(0);
  // GetMaxPacketSize
  auto get_max_packet_size = state().pending_operations.pop_front();
  auto get_max_packet_size_request = std::move(std::get<Request>(*get_max_packet_size->request));
  ASSERT_EQ(get_max_packet_size_request.request()->header.device_id, 0UL);
  ASSERT_EQ(get_max_packet_size_request.request()->header.ep_address, 0UL);
  ASSERT_EQ(get_max_packet_size_request.request()->header.length, 8UL);
  ASSERT_EQ(get_max_packet_size_request.request()->setup.bm_request_type,
            USB_DIR_IN | USB_TYPE_STANDARD | USB_RECIP_DEVICE);
  ASSERT_EQ(get_max_packet_size_request.request()->setup.w_value, USB_DT_DEVICE << 8);
  ASSERT_EQ(get_max_packet_size_request.request()->setup.w_index, 0UL);
  ASSERT_EQ(get_max_packet_size_request.request()->setup.b_request, USB_REQ_GET_DESCRIPTOR);
  ASSERT_EQ(get_max_packet_size_request.request()->setup.w_length, 8UL);
  ASSERT_TRUE(get_max_packet_size_request.request()->direct);
  usb_device_descriptor_t* descriptor;
  ASSERT_OK(get_max_packet_size_request.Mmap(reinterpret_cast<void**>(&descriptor)));
  descriptor->b_descriptor_type = USB_DT_DEVICE;
  descriptor->b_max_packet_size0 = 42;
  get_max_packet_size_request.Complete(ZX_OK, 8);
  driver_test().driver()->RunUntilIdle(0);

  // GetDeviceDescriptor
  auto get_descriptor = state().pending_operations.pop_front();
  auto get_descriptor_request = std::move(std::get<Request>(*get_descriptor->request));
  ASSERT_EQ(get_descriptor_request.request()->header.device_id, 0UL);
  ASSERT_EQ(get_descriptor_request.request()->header.ep_address, 0UL);
  ASSERT_EQ(get_descriptor_request.request()->header.length, sizeof(usb_device_descriptor_t));
  ASSERT_EQ(get_descriptor_request.request()->setup.bm_request_type,
            USB_DIR_IN | USB_TYPE_STANDARD | USB_RECIP_DEVICE);
  ASSERT_EQ(get_descriptor_request.request()->setup.w_value, USB_DT_DEVICE << 8);
  ASSERT_EQ(get_descriptor_request.request()->setup.w_index, 0UL);
  ASSERT_EQ(get_descriptor_request.request()->setup.b_request, USB_REQ_GET_DESCRIPTOR);
  ASSERT_EQ(get_descriptor_request.request()->setup.w_length, sizeof(usb_device_descriptor_t));
  ASSERT_TRUE(get_descriptor_request.request()->direct);
  ASSERT_OK(get_descriptor_request.Mmap(reinterpret_cast<void**>(&descriptor)));
  descriptor->b_descriptor_type = USB_DT_DEVICE;
  get_descriptor_request.Complete(ZX_OK, sizeof(usb_device_descriptor_t));
  driver_test().driver()->RunUntilIdle(0);

  // Online Device
  auto online_trb = FakeTRB::FromTRB(state().pending_operations.pop_front()->trb);
  ASSERT_EQ(online_trb->Op, FakeTRB::Op::OnlineDevice);
  ASSERT_EQ(online_trb->slot, 1UL);
  ASSERT_EQ(online_trb->port, kPort);
  ASSERT_EQ(online_trb->speed, USB_SPEED_HIGH);
  driver_test().driver()->RunUntilIdle(0UL);
  ASSERT_EQ(completion_code, ZX_OK);
  ASSERT_TRUE(state().pending_operations.is_empty());
}

TEST_F(EnumerationTests, AddressDeviceCommandShouldOnlineDeviceAfterSuccessfulRetry) {
  // EnableSlot
  constexpr uint8_t kPort = 5;
  constexpr uint8_t kHubDepth = 52;
  constexpr uint8_t kHubSlot = 28;
  constexpr uint8_t kHubPort = 39;
  constexpr usb_speed_t kSpeed = USB_SPEED_FULL;
  constexpr bool kMultiTT = false;
  state().speeds[0] = kSpeed;
  state().speeds[1] = kSpeed;
  zx_status_t completion_code = -1;
  auto enumeration_task =
      EnumerateDevice(driver_test().driver(), kPort,
                      TestHubInfo(kHubDepth, kHubSlot, kHubPort, kSpeed, kMultiTT))
          .then([&](fpromise::result<void, zx_status_t>& result) {
            if (result.is_ok()) {
              completion_code = ZX_OK;
            } else {
              completion_code = result.error();
            }
            return result;
          });
  auto enable_slot_task = state().pending_operations.pop_front();
  auto enum_slot_trb = FakeTRB::FromTRB(enable_slot_task->trb);
  ASSERT_EQ(enum_slot_trb->Op, FakeTRB::Op::EnableSlot);
  reinterpret_cast<CommandCompletionEvent*>(enum_slot_trb.get())
      ->set_CompletionCode(CommandCompletionEvent::Success);
  reinterpret_cast<CommandCompletionEvent*>(enum_slot_trb.get())->set_SlotID(1);
  enable_slot_task->completer->complete_ok(enum_slot_trb.get());
  driver_test().driver()->ScheduleTask(0, std::move(enumeration_task));
  driver_test().driver()->RunUntilIdle(0);
  auto device_information = FakeTRB::FromTRB(state().pending_operations.pop_front()->trb);
  ASSERT_EQ(device_information->Op, FakeTRB::Op::SetDeviceInformation);
  VerifyHubInfo(device_information->hub_info, kHubDepth, kHubSlot, kHubPort, kSpeed, kMultiTT);
  ASSERT_EQ(device_information->port, kPort);
  ASSERT_EQ(device_information->slot, 1UL);

  // AddressDevice
  auto address_device = state().pending_operations.pop_front();
  auto address_device_op = FakeTRB::FromTRB(address_device->trb);
  ASSERT_EQ(address_device_op->Op, FakeTRB::Op::AddressDevice);
  ASSERT_EQ(address_device_op->slot, 1UL);
  ASSERT_EQ(address_device_op->port, kPort);
  VerifyHubInfo(address_device_op->hub_info, kHubDepth, kHubSlot, kHubPort, kSpeed, kMultiTT);
  reinterpret_cast<CommandCompletionEvent*>(address_device_op.get())
      ->set_CompletionCode(CommandCompletionEvent::UsbTransactionError);
  address_device->completer->complete_ok(address_device_op.get());
  driver_test().driver()->RunUntilIdle(0UL);

  // DisableSlot
  auto disable_op = state().pending_operations.pop_front();
  auto disable_trb = FakeTRB::FromTRB(disable_op->trb);
  ASSERT_EQ(disable_trb->Op, FakeTRB::Op::DisableSlot);
  ASSERT_EQ(disable_trb->slot, 1UL);
  reinterpret_cast<CommandCompletionEvent*>(disable_trb.get())
      ->set_CompletionCode(CommandCompletionEvent::UsbTransactionError);
  disable_op->completer->complete_ok(disable_trb.get());
  driver_test().driver()->RunUntilIdle(0);

  // EnableSlot
  enable_slot_task = state().pending_operations.pop_front();
  enum_slot_trb = FakeTRB::FromTRB(enable_slot_task->trb);
  ASSERT_EQ(enum_slot_trb->Op, FakeTRB::Op::EnableSlot);
  reinterpret_cast<CommandCompletionEvent*>(enum_slot_trb.get())
      ->set_CompletionCode(CommandCompletionEvent::Success);
  reinterpret_cast<CommandCompletionEvent*>(enum_slot_trb.get())->set_SlotID(2);
  enable_slot_task->completer->complete_ok(enum_slot_trb.get());
  driver_test().driver()->RunUntilIdle(0);

  // Set device information
  device_information = FakeTRB::FromTRB(state().pending_operations.pop_front()->trb);
  ASSERT_EQ(device_information->Op, FakeTRB::Op::SetDeviceInformation);
  VerifyHubInfo(device_information->hub_info, kHubDepth, kHubSlot, kHubPort, kSpeed, kMultiTT);
  ASSERT_EQ(device_information->port, kPort);
  ASSERT_EQ(device_information->slot, 2UL);
  driver_test().driver()->RunUntilIdle(0);

  // AddressDevice with BSR = 1
  address_device = state().pending_operations.pop_front();
  address_device_op = FakeTRB::FromTRB(address_device->trb);
  ASSERT_EQ(address_device_op->Op, FakeTRB::Op::AddressDevice);
  ASSERT_TRUE(address_device_op->bsr);
  ASSERT_EQ(address_device_op->slot, 2UL);
  ASSERT_EQ(address_device_op->port, kPort);
  VerifyHubInfo(address_device_op->hub_info, kHubDepth, kHubSlot, kHubPort, kSpeed, kMultiTT);
  reinterpret_cast<CommandCompletionEvent*>(address_device_op.get())
      ->set_CompletionCode(CommandCompletionEvent::Success);
  address_device->completer->complete_ok(address_device_op.get());
  driver_test().driver()->RunUntilIdle(0);

  // GetMaxPacketSize
  auto get_max_packet_size = state().pending_operations.pop_front();
  auto get_max_packet_size_request = std::move(std::get<Request>(*get_max_packet_size->request));
  ASSERT_EQ(get_max_packet_size_request.request()->header.device_id, 1UL);
  ASSERT_EQ(get_max_packet_size_request.request()->header.ep_address, 0UL);
  ASSERT_EQ(get_max_packet_size_request.request()->header.length, 8UL);
  ASSERT_EQ(get_max_packet_size_request.request()->setup.bm_request_type,
            USB_DIR_IN | USB_TYPE_STANDARD | USB_RECIP_DEVICE);
  ASSERT_EQ(get_max_packet_size_request.request()->setup.w_value, USB_DT_DEVICE << 8);
  ASSERT_EQ(get_max_packet_size_request.request()->setup.w_index, 0UL);
  ASSERT_EQ(get_max_packet_size_request.request()->setup.b_request, USB_REQ_GET_DESCRIPTOR);
  ASSERT_EQ(get_max_packet_size_request.request()->setup.w_length, 8UL);
  ASSERT_TRUE(get_max_packet_size_request.request()->direct);
  usb_device_descriptor_t* descriptor;
  ASSERT_OK(get_max_packet_size_request.Mmap(reinterpret_cast<void**>(&descriptor)));
  descriptor->b_descriptor_type = USB_DT_DEVICE;
  descriptor->b_max_packet_size0 = 42;
  get_max_packet_size_request.Complete(ZX_OK, 8);
  driver_test().driver()->RunUntilIdle(0);

  // SetMaxPacketSize
  auto set_max_packet_size = state().pending_operations.pop_front();
  auto set_max_packet_size_trb = FakeTRB::FromTRB(set_max_packet_size->trb);
  ASSERT_EQ(set_max_packet_size_trb->Op, FakeTRB::Op::SetMaxPacketSize);
  ASSERT_EQ(set_max_packet_size_trb->slot, 2UL);
  ASSERT_EQ(set_max_packet_size_trb->max_packet_size, 42UL);
  reinterpret_cast<CommandCompletionEvent*>(set_max_packet_size_trb.get())
      ->set_CompletionCode(CommandCompletionEvent::Success);
  set_max_packet_size->completer->complete_ok(set_max_packet_size_trb.get());
  driver_test().driver()->RunUntilIdle(0);

  // AddressDevice with BSR = 0
  address_device = state().pending_operations.pop_front();
  address_device_op = FakeTRB::FromTRB(address_device->trb);
  ASSERT_EQ(address_device_op->Op, FakeTRB::Op::AddressDevice);
  ASSERT_FALSE(address_device_op->bsr);
  ASSERT_EQ(address_device_op->slot, 2UL);
  reinterpret_cast<CommandCompletionEvent*>(address_device_op.get())
      ->set_CompletionCode(CommandCompletionEvent::Success);
  address_device->completer->complete_ok(address_device_op.get());
  driver_test().driver()->RunUntilIdle(0);

  // Timeout
  auto timeout = state().pending_operations.pop_front();
  ASSERT_TRUE(FakeTRB::FromTRB(timeout->trb)->deadline.get());
  timeout->completer->complete_ok(address_device_op.get());
  driver_test().driver()->RunUntilIdle(0);

  // GetMaxPacketSize
  get_max_packet_size = state().pending_operations.pop_front();
  get_max_packet_size_request = std::move(std::get<Request>(*get_max_packet_size->request));
  ASSERT_EQ(get_max_packet_size_request.request()->header.device_id, 1UL);
  ASSERT_EQ(get_max_packet_size_request.request()->header.ep_address, 0UL);
  ASSERT_EQ(get_max_packet_size_request.request()->header.length, 8UL);
  ASSERT_EQ(get_max_packet_size_request.request()->setup.bm_request_type,
            USB_DIR_IN | USB_TYPE_STANDARD | USB_RECIP_DEVICE);
  ASSERT_EQ(get_max_packet_size_request.request()->setup.w_value, USB_DT_DEVICE << 8);
  ASSERT_EQ(get_max_packet_size_request.request()->setup.w_index, 0UL);
  ASSERT_EQ(get_max_packet_size_request.request()->setup.b_request, USB_REQ_GET_DESCRIPTOR);
  ASSERT_EQ(get_max_packet_size_request.request()->setup.w_length, 8UL);
  ASSERT_TRUE(get_max_packet_size_request.request()->direct);
  ASSERT_OK(get_max_packet_size_request.Mmap(reinterpret_cast<void**>(&descriptor)));
  descriptor->b_descriptor_type = USB_DT_DEVICE;
  descriptor->b_max_packet_size0 = 32;
  get_max_packet_size_request.Complete(ZX_OK, 8);
  driver_test().driver()->RunUntilIdle(0);

  // SetMaxPacketSize (full-speed device requires setting this again)
  set_max_packet_size = state().pending_operations.pop_front();
  set_max_packet_size_trb = FakeTRB::FromTRB(set_max_packet_size->trb);
  ASSERT_EQ(set_max_packet_size_trb->Op, FakeTRB::Op::SetMaxPacketSize);
  ASSERT_EQ(set_max_packet_size_trb->slot, 2UL);
  ASSERT_EQ(set_max_packet_size_trb->max_packet_size, 32UL);
  reinterpret_cast<CommandCompletionEvent*>(set_max_packet_size_trb.get())
      ->set_CompletionCode(CommandCompletionEvent::Success);
  set_max_packet_size->completer->complete_ok(set_max_packet_size_trb.get());
  driver_test().driver()->RunUntilIdle(0);

  // GetDeviceDescriptor
  auto get_descriptor = state().pending_operations.pop_front();
  auto get_descriptor_request = std::move(std::get<Request>(*get_descriptor->request));
  ASSERT_EQ(get_descriptor_request.request()->header.device_id, 1UL);
  ASSERT_EQ(get_descriptor_request.request()->header.ep_address, 0UL);
  ASSERT_EQ(get_descriptor_request.request()->header.length, sizeof(usb_device_descriptor_t));
  ASSERT_EQ(get_descriptor_request.request()->setup.bm_request_type,
            USB_DIR_IN | USB_TYPE_STANDARD | USB_RECIP_DEVICE);
  ASSERT_EQ(get_descriptor_request.request()->setup.w_value, USB_DT_DEVICE << 8);
  ASSERT_EQ(get_descriptor_request.request()->setup.w_index, 0UL);
  ASSERT_EQ(get_descriptor_request.request()->setup.b_request, USB_REQ_GET_DESCRIPTOR);
  ASSERT_EQ(get_descriptor_request.request()->setup.w_length, sizeof(usb_device_descriptor_t));
  ASSERT_TRUE(get_descriptor_request.request()->direct);
  ASSERT_OK(get_descriptor_request.Mmap(reinterpret_cast<void**>(&descriptor)));
  descriptor->b_descriptor_type = USB_DT_DEVICE;
  get_descriptor_request.Complete(ZX_OK, sizeof(usb_device_descriptor_t));
  driver_test().driver()->RunUntilIdle(0);

  // Online Device
  auto online_trb = FakeTRB::FromTRB(state().pending_operations.pop_front()->trb);
  ASSERT_EQ(online_trb->Op, FakeTRB::Op::OnlineDevice);
  ASSERT_EQ(online_trb->slot, 2UL);
  ASSERT_EQ(online_trb->port, 5UL);
  ASSERT_EQ(online_trb->speed, USB_SPEED_FULL);
  driver_test().driver()->RunUntilIdle(0);
  ASSERT_EQ(completion_code, ZX_OK);
  ASSERT_TRUE(state().pending_operations.is_empty());
}

TEST_F(EnumerationTests, DisableSlotAfterFailedRetry) {
  constexpr uint8_t kHubDepth = 52;
  constexpr uint8_t kHubSlot = 28;
  constexpr uint8_t kHubPort = 39;
  constexpr usb_speed_t kSpeed = USB_SPEED_FULL;
  constexpr bool kMultiTT = false;
  state().speeds[0] = kSpeed;
  state().speeds[1] = kSpeed;
  zx_status_t completion_code = ZX_OK;
  auto enumeration_task =
      EnumerateDevice(driver_test().driver(), 5,
                      TestHubInfo(kHubDepth, kHubSlot, kHubPort, kSpeed, kMultiTT))
          .then([&](fpromise::result<void, zx_status_t>& result) {
            if (result.is_ok()) {
              completion_code = ZX_OK;
            } else {
              completion_code = result.error();
            }
            return result;
          });

  // Enable slot.
  auto enable_slot_task = state().pending_operations.pop_front();
  auto enum_slot_trb = FakeTRB::FromTRB(enable_slot_task->trb);
  ASSERT_EQ(enum_slot_trb->Op, FakeTRB::Op::EnableSlot);
  reinterpret_cast<CommandCompletionEvent*>(enum_slot_trb.get())
      ->set_CompletionCode(CommandCompletionEvent::Success);
  reinterpret_cast<CommandCompletionEvent*>(enum_slot_trb.get())->set_SlotID(1);
  enable_slot_task->completer->complete_ok(enum_slot_trb.get());
  driver_test().driver()->ScheduleTask(0, std::move(enumeration_task));
  driver_test().driver()->RunUntilIdle(0);

  auto device_information = FakeTRB::FromTRB(state().pending_operations.pop_front()->trb);
  ASSERT_EQ(device_information->Op, FakeTRB::Op::SetDeviceInformation);
  ASSERT_EQ(device_information->slot, 1UL);

  // AddressDevice. Return USB Transaction Error to force a retry.
  auto address_device = state().pending_operations.pop_front();
  auto address_device_op = FakeTRB::FromTRB(address_device->trb);
  ASSERT_EQ(address_device_op->Op, FakeTRB::Op::AddressDevice);
  reinterpret_cast<CommandCompletionEvent*>(address_device_op.get())
      ->set_CompletionCode(CommandCompletionEvent::UsbTransactionError);
  address_device->completer->complete_ok(address_device_op.get());
  driver_test().driver()->RunUntilIdle(0);

  // DisableSlot
  auto disable_op = state().pending_operations.pop_front();
  auto disable_trb = FakeTRB::FromTRB(disable_op->trb);
  ASSERT_EQ(disable_trb->Op, FakeTRB::Op::DisableSlot);
  ASSERT_EQ(disable_trb->slot, 1UL);
  reinterpret_cast<CommandCompletionEvent*>(disable_trb.get())
      ->set_CompletionCode(CommandCompletionEvent::UsbTransactionError);
  disable_op->completer->complete_ok(disable_trb.get());
  driver_test().driver()->RunUntilIdle(0);

  // EnableSlot
  enable_slot_task = state().pending_operations.pop_front();
  enum_slot_trb = FakeTRB::FromTRB(enable_slot_task->trb);
  ASSERT_EQ(enum_slot_trb->Op, FakeTRB::Op::EnableSlot);
  reinterpret_cast<CommandCompletionEvent*>(enum_slot_trb.get())
      ->set_CompletionCode(CommandCompletionEvent::Success);
  reinterpret_cast<CommandCompletionEvent*>(enum_slot_trb.get())->set_SlotID(2);
  enable_slot_task->completer->complete_ok(enum_slot_trb.get());
  driver_test().driver()->RunUntilIdle(0);

  // Set device information
  device_information = FakeTRB::FromTRB(state().pending_operations.pop_front()->trb);
  ASSERT_EQ(device_information->slot, 2UL);
  driver_test().driver()->RunUntilIdle(0);

  // AddressDevice. Return a failure to trigger DisableSlot.
  address_device = state().pending_operations.pop_front();
  address_device_op = FakeTRB::FromTRB(address_device->trb);
  reinterpret_cast<CommandCompletionEvent*>(address_device_op.get())
      ->set_CompletionCode(CommandCompletionEvent::CommandAborted);
  address_device->completer->complete_ok(address_device_op.get());
  driver_test().driver()->RunUntilIdle(0);

  // DisableSlot
  disable_trb = FakeTRB::FromTRB(state().pending_operations.pop_front()->trb);
  ASSERT_EQ(disable_trb->Op, FakeTRB::Op::DisableSlot);
  ASSERT_EQ(disable_trb->slot, 2UL);
  EXPECT_NE(completion_code, ZX_OK);
}

}  // namespace usb_xhci
