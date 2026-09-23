// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/usb/drivers/xhci/xhci-event-ring.h"

#include <lib/driver/fake-mmio-reg/cpp/fake-mmio-reg.h>
#include <lib/fpromise/bridge.h>
#include <lib/fpromise/promise.h>

#include <fake-dma-buffer/fake-dma-buffer.h>
#include <gtest/gtest.h>
#include <usb/usb.h>

#include "src/devices/usb/drivers/xhci/tests/test-env.h"

namespace usb_xhci {

constexpr size_t kErstMax = 42;
constexpr size_t kShortTransferLength0 = 97;
constexpr size_t kShortTransferLength1 = 102;
constexpr size_t kFinalTransferLength = 87;
constexpr size_t kShortTransferLength = 800;
constexpr size_t kFakeTrb = 0x3924ff0913;
constexpr size_t kFakeTrbVirt = 0x8411487132;
constexpr auto kPortNo = 5;

enum class CommandType {
  EnumerateDevice,
  ResetEndpoint,
};

struct Command : public fbl::DoublyLinkedListable<std::unique_ptr<Command>> {
  uint8_t port;
  std::optional<HubInfo> hub;
  fpromise::completer<TRB*, zx_status_t> completer;
  CommandType type;
  uint8_t endpoint;
  uint32_t device_id;
};

class EventRingHarness : public ::testing::Test {
 public:
  EventRingHarness() : trb_context_allocator_(-1, true) {}
  void SetUp() override {
    // Globals
    constexpr auto kRuntimeRegisterOffset = 6 * sizeof(uint32_t);
    constexpr auto kErdp = 2062 * sizeof(uint32_t);
    region_.emplace(sizeof(uint32_t), 4096);
    buffer_.emplace(region_->GetMmioBuffer());

    // Core registers
    (*region_)[kRuntimeRegisterOffset].SetReadCallback([=]() { return 0x2000; });
    (*region_)[kErdp].SetReadCallback([=]() { return erdp_; });
    (*region_)[kErdp].SetWriteCallback([=](uint64_t value) {
      ERDP reg;
      reg.set_reg_value(value);
      erdp_ = reg.Pointer();
    });

    // Port/per-slot registers
    (*region_)[PORTSC::Get(0, kPortNo).addr()].SetReadCallback(
        [=]() { return port_sc_.reg_value(); });
    (*region_)[PORTSC::Get(0, kPortNo).addr()].SetWriteCallback([=](uint64_t value) {
      PORTSC sc;
      sc.set_reg_value(static_cast<uint32_t>(value));
      if (sc.PR()) {
        port_sc_.set_PR(true);
      }
    });
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

  void Interrupt() { ring_->HandleIRQ(); }

  void AddTRB(const TRB& trb) {
    auto ptr = ddk_fake::PhysToVirt<TRB*>(erdp_);
    *ptr = trb;
    Control::FromTRB(ptr).set_Cycle(1).ToTrb(ptr);
    erdp_ += sizeof(TRB);
  }

  void ConnectDevice() {
    port_sc_.set_CCS(true);
    port_sc_.set_PED(true);
    port_sc_.set_PR(false);
    port_sc_.set_PLS(PORTSC::Polling);
    port_sc_.set_CSC(true);
  }

  void DisconnectDevice() {
    port_sc_.set_CCS(false);
    port_sc_.set_CSC(true);
  }

  void MakeQuantumPort() {
    // Port is enabled and disabled simultaneously
    port_sc_.set_PLS(PORTSC::Disabled);
    port_sc_.set_PED(1);
  }

  void LinkUp() {
    port_sc_.set_PR(false);
    port_sc_.set_PLS(PORTSC::U0);
  }

  bool PortResetPending() { return port_sc_.PR(); }

  void InsertPortStatusChangeEvent() {
    TRB trb;
    trb.ptr = 0;
    Control::FromTRB(&trb).set_Type(Control::PortStatusChangeEvent).ToTrb(&trb);
    auto evt = static_cast<PortStatusChangeEvent*>(&trb);
    evt->set_PortID(kPortNo);
    auto command_completion = static_cast<CommandCompletionEvent*>(&trb);
    command_completion->set_CompletionCode(CommandCompletionEvent::Success);
    AddTRB(trb);
  }

  TRB* trb() { return ddk_fake::PhysToVirt<TRB*>(erdp_); }

  using TestRequest = usb::CallbackRequest<sizeof(max_align_t)>;
  template <typename Callback>
  zx_status_t AllocateRequest(std::optional<TestRequest>* request, uint32_t device_id,
                              uint64_t data_size, uint8_t endpoint, Callback callback) {
    zx_status_t status =
        TestRequest::Alloc(request, data_size, endpoint,
                           driver_test().driver()->UsbHciGetRequestSize(), std::move(callback));
    if (status != ZX_OK) {
      return status;
    }
    request->value().request()->header.device_id = device_id;
    return ZX_OK;
  }

  std::unique_ptr<TRBContext> AllocateContext() { return trb_context_allocator_.New(); }

  void RequestQueue(usb_request_t* usb_request,
                    const usb_request_complete_callback_t* complete_cb) {
    pending_req_ = Request(usb_request, *complete_cb, sizeof(usb_request_t));
  }

  Request Borrow(TestRequest request) {
    static const usb_protocol_ops_t ops = {
        .request_queue = [](void* ctx, usb_request_t* usb_request,
                             const usb_request_complete_callback_t* complete_cb) {
          auto* harness = static_cast<EventRingHarness*>(ctx);
          harness->RequestQueue(usb_request, complete_cb);
        },
    };
    usb_protocol_t proto = {
        .ops = &ops,
        .ctx = this,
    };
    ddk::UsbProtocolClient client(&proto);
    TestRequest::Queue(std::move(request), client);
    return std::move(*pending_req_);
  }

  zx_status_t InitRing(EventRing* ring) {
    ring_ = ring;
    auto regoffset = RuntimeRegisterOffset::Get().ReadFrom(&buffer_.value());
    zx_status_t status =
        ring->Init(zx_system_get_page_size(), kFakeBti, &buffer_.value(), false, kErstMax,
                   ERSTSZ::Get(regoffset, 0).ReadFrom(&buffer_.value()),
                   ERDP::Get(regoffset, 0).ReadFrom(&buffer_.value()),
                   IMAN::Get(regoffset, 0).ReadFrom(&buffer_.value()),
                   CapLength::Get().ReadFrom(&buffer_.value()).Length(),
                   HCSPARAMS1::Get().ReadFrom(&buffer_.value()), &command_ring_,
                   DoorbellOffset::Get().ReadFrom(&buffer_.value()), driver_test().driver(),
                   HCCPARAMS1::Get().ReadFrom(&buffer_.value()), dcbaa_, 0, nullptr);
    if (status != ZX_OK) {
      return status;
    }
    status = ring_->AddSegmentIfNoneLock();
    if (status != ZX_OK) {
      return status;
    }
    ERDP reg;
    reg.set_Pointer(ddk_fake::PhysToVirt<zx_paddr_t*>(ring_->erst())[0]);
    (*region_)[2062 * sizeof(uint32_t)].Write(reg.reg_value());
    return ZX_OK;
  }
  void SetShortPacketHandler(fit::function<void(usb_xhci::TRB*, size_t, usb_xhci::TRB**)> handler) {
    short_packet_handler_ = std::move(handler);
  }

  void HandleShortPacket(TRB* short_trb, size_t short_length, TRB** first_trb) {
    (*short_packet_handler_)(short_trb, short_length, first_trb);
  }

  zx_status_t CompleteTRB(TRB* trb, std::unique_ptr<TRBContext>* context) {
    if (trb != expected_completion_) {
      return ZX_ERR_IO;
    }
    *context = pending_contexts_.pop_front();
    return ZX_OK;
  }

  void SetCompletion(TRB* expected) { expected_completion_ = expected; }

  void AddContext(std::unique_ptr<TRBContext> context) {
    pending_contexts_.push_back(std::move(context));
  }

  void AddCommand(std::unique_ptr<Command> command) { commands_.push_back(std::move(command)); }

  std::unique_ptr<Command> GetCommand() { return commands_.pop_front(); }

  bool HasCommand() { return !commands_.is_empty(); }

  void InitSlot(uint8_t i) {
    // Enable SlotID i for testing with arbitrary port 1.
    auto& state = driver_test().driver()->GetDeviceState()[i - 1];
    ASSERT_TRUE(state);
    fbl::AutoLock _(&state->transaction_lock());
    state->SetDeviceInformation(i, 1, std::nullopt);
  }

  fdf_testing::ForegroundDriverTest<EmptyTestConfig>& driver_test() { return driver_test_; }

 private:
  fdf_testing::ForegroundDriverTest<EmptyTestConfig> driver_test_;
  std::optional<Request> pending_req_;
  std::optional<fit::function<void(usb_xhci::TRB*, size_t, usb_xhci::TRB**)>> short_packet_handler_;
  using AllocatorTraits = fbl::InstancedSlabAllocatorTraits<std::unique_ptr<TRBContext>, 4096U>;
  using AllocatorType = fbl::SlabAllocator<AllocatorTraits>;
  AllocatorType trb_context_allocator_;
  TRB* expected_completion_ = nullptr;
  fbl::DoublyLinkedList<std::unique_ptr<Command>> commands_;
  fbl::DoublyLinkedList<std::unique_ptr<TRBContext>> pending_contexts_;
  std::optional<fdf::MmioBuffer> buffer_;
  EventRing* ring_;
  PORTSC port_sc_;
  CommandRing command_ring_;
  uint64_t dcbaa_[128];
  uint64_t erdp_;
  std::optional<fake_mmio::FakeMmioRegRegion> region_;
};

// UsbXhci Methods
zx::result<> UsbXhci::Init(std::unique_ptr<dma_buffer::BufferFactory> buffer_factory) {
  buffer_factory_ = std::move(buffer_factory);

  fbl::AllocChecker ac;
  interrupters_ = fbl::MakeArray<Interrupter>(&ac, 1);
  if (!ac.check()) {
    return zx::error(ZX_ERR_NO_MEMORY);
  }
  max_slots_ = 32;
  device_state_ = fbl::MakeArray<fbl::RefPtr<DeviceState>>(&ac, max_slots_);
  if (!ac.check()) {
    return zx::error(ZX_ERR_NO_MEMORY);
  }
  for (size_t i = 0; i < max_slots_; i++) {
    device_state_[i] = fbl::MakeRefCounted<DeviceState>(static_cast<uint32_t>(i), this);
    fbl::AutoLock l(&device_state_[i]->transaction_lock());
    for (size_t c = 0; c < max_slots_; c++) {
      device_state_[i]->InitEndpoint(static_cast<uint8_t>(c), nullptr, nullptr);
    }
  }
  port_state_ = fbl::MakeArray<PortState>(&ac, 32);
  if (!ac.check()) {
    return zx::error(ZX_ERR_NO_MEMORY);
  }
  zx_status_t status = GetTestHarness<EventRingHarness>()->InitRing(&interrupters_[0].ring());
  if (status != ZX_OK) {
    return zx::error(status);
  }
  return zx::ok();
}

fpromise::promise<void, zx_status_t> UsbXhci::UsbHciResetEndpointAsync(uint32_t device_id,
                                                                       uint8_t ep_address) {
  auto harness = GetTestHarness<EventRingHarness>();
  std::unique_ptr<Command> command = std::make_unique<Command>();
  fpromise::bridge<TRB*, zx_status_t> bridge;
  command->type = CommandType::ResetEndpoint;
  command->device_id = device_id;
  command->endpoint = ep_address;
  command->completer = std::move(bridge.completer);
  harness->AddCommand(std::move(command));
  return bridge.consumer.promise().discard_value();
}

void UsbXhci::UsbHciRequestQueue(usb_request_t* usb_request,
                                 const usb_request_complete_callback_t* complete_cb) {}

zx_status_t UsbXhci::UsbHciCancelAll(uint32_t device_id, uint8_t ep_address) {
  return ZX_ERR_NOT_SUPPORTED;
}

fpromise::promise<void, zx_status_t> UsbXhci::Timeout(uint16_t target_interrupter,
                                                      zx::time deadline) {
  return fpromise::make_result_promise<void, zx_status_t>(fpromise::ok());
}

fpromise::promise<void, zx_status_t> UsbXhci::DeviceOffline(uint32_t slot) {
  return fpromise::make_error_promise<zx_status_t>(ZX_ERR_NOT_SUPPORTED);
}

void UsbXhci::Shutdown(zx_status_t status) {}

// TransferRing Methods
zx_status_t TransferRing::Init(size_t page_size, const zx::bti& bti, EventRing* ring, bool is_32bit,
                               fdf::MmioBuffer* mmio, UsbXhci* hci) {
  fbl::AutoLock _(&mutex_);
  hci_ = hci;
  return ZX_OK;
}

zx_status_t TransferRing::HandleShortPacket(TRB* short_trb, size_t short_length, TRB** first_trb) {
  hci_->GetTestHarness<EventRingHarness>()->HandleShortPacket(short_trb, short_length, first_trb);
  return ZX_OK;
}

fbl::DoublyLinkedList<std::unique_ptr<TRBContext>> TransferRing::TakePendingTRBsUntil(TRB* end) {
  fbl::DoublyLinkedList<std::unique_ptr<TRBContext>> empty;
  return empty;
}

fbl::DoublyLinkedList<std::unique_ptr<TRBContext>> TransferRing::TakePendingTRBs() {
  fbl::DoublyLinkedList<std::unique_ptr<TRBContext>> empty;
  return empty;
}

TRB* TransferRing::PhysToVirt(zx_paddr_t paddr) {
  return (paddr == kFakeTrb) ? reinterpret_cast<TRB*>(kFakeTrbVirt) : nullptr;
}

zx_status_t TransferRing::CompleteTRB(TRB* trb, std::unique_ptr<TRBContext>* context) {
  return hci_->GetTestHarness<EventRingHarness>()->CompleteTRB(trb, context);
}

zx_status_t TransferRing::DeinitIfActive() { return ZX_OK; }

fpromise::promise<void, zx_status_t> EnumerateDevice(UsbXhci* hci, uint8_t port,
                                                     std::optional<HubInfo> hub_info) {
  auto harness = hci->GetTestHarness<EventRingHarness>();
  std::unique_ptr<Command> command = std::make_unique<Command>();
  fpromise::bridge<TRB*, zx_status_t> bridge;
  command->type = CommandType::EnumerateDevice;
  command->port = port;
  command->hub = hub_info;
  command->completer = std::move(bridge.completer);
  harness->AddCommand(std::move(command));
  return bridge.consumer.promise().discard_value();
}

TEST_F(EventRingHarness, ShortTransferTest) {
  InitSlot(1);
  TRB* start = trb();
  TRB trb;
  trb.ptr = kFakeTrb;
  Control::FromTRB(&trb).set_Type(Control::TransferEvent).ToTrb(&trb);
  TransferEvent* evt = static_cast<TransferEvent*>(&trb);
  evt->set_SlotID(1);
  evt->set_EndpointID(2);
  evt->set_CompletionCode(CommandCompletionEvent::ShortPacket);
  evt->set_TransferLength(kShortTransferLength0);
  AddTRB(trb);
  evt->set_TransferLength(kShortTransferLength1);
  AddTRB(trb);
  evt->set_CompletionCode(CommandCompletionEvent::Success);
  evt->set_TransferLength(kFinalTransferLength);
  AddTRB(trb);
  size_t index = 0;
  TRB* trb_list[2];
  size_t shorts[2];
  SetShortPacketHandler([&](TRB* short_trb, size_t short_length, usb_xhci::TRB** first_trb) {
    *first_trb = nullptr;
    trb_list[index] = short_trb;
    shorts[index] = short_length;
    index++;
  });
  auto ctx = AllocateContext();
  size_t transfer_len;
  zx_status_t transfer_status;
  ctx->trb = start;
  std::optional<TestRequest> request;
  AllocateRequest(&request, 1, zx_system_get_page_size() * 3, 5, [&](TestRequest request) {
    transfer_status = request.request()->response.status;
    transfer_len = request.request()->response.actual;
  });
  ctx->short_transfer_len = kShortTransferLength;
  ctx->request = Borrow(std::move(*request));
  AddContext(std::move(ctx));
  SetCompletion(reinterpret_cast<TRB*>(kFakeTrbVirt));
  Interrupt();
  ASSERT_OK(transfer_status);
  ASSERT_EQ(transfer_len, kShortTransferLength);
  ASSERT_EQ(shorts[0], kShortTransferLength0);
  ASSERT_EQ(shorts[1], kShortTransferLength1);
  ASSERT_EQ(trb_list[0], reinterpret_cast<TRB*>(kFakeTrbVirt));
  ASSERT_EQ(trb_list[1], reinterpret_cast<TRB*>(kFakeTrbVirt));
}

TEST_F(EventRingHarness, NormalStall) {
  InitSlot(1);
  TRB* start = trb();
  TRB trb;
  trb.ptr = kFakeTrb;
  Control::FromTRB(&trb).set_Type(Control::TransferEvent).ToTrb(&trb);
  TransferEvent* evt = static_cast<TransferEvent*>(&trb);
  evt->set_SlotID(1);
  evt->set_EndpointID(2);
  evt->set_CompletionCode(CommandCompletionEvent::StallError);
  evt->set_TransferLength(0);
  AddTRB(trb);
  auto ctx = AllocateContext();
  size_t transfer_len;
  zx_status_t transfer_status;
  ctx->trb = start;
  std::optional<TestRequest> request;
  AllocateRequest(&request, 1, zx_system_get_page_size() * 3, 5, [&](TestRequest request) {
    transfer_status = request.request()->response.status;
    transfer_len = request.request()->response.actual;
  });
  ctx->request = Borrow(std::move(*request));
  AddContext(std::move(ctx));
  SetCompletion(reinterpret_cast<TRB*>(kFakeTrbVirt));
  Interrupt();
  ASSERT_EQ(transfer_status, ZX_ERR_IO_REFUSED);
  ASSERT_EQ(transfer_len, 0UL);
}

TEST_F(EventRingHarness, BadHubStallOnDtDeviceQualifier) {
  InitSlot(1);
  TRB* start = trb();
  TRB trb;
  trb.ptr = kFakeTrb;
  Control::FromTRB(&trb).set_Type(Control::TransferEvent).ToTrb(&trb);
  TransferEvent* evt = static_cast<TransferEvent*>(&trb);
  evt->set_SlotID(1);
  evt->set_EndpointID(2);
  evt->set_CompletionCode(CommandCompletionEvent::StallError);
  evt->set_TransferLength(0);
  AddTRB(trb);
  auto ctx = AllocateContext();
  size_t transfer_len;
  zx_status_t transfer_status;
  ctx->trb = start;
  std::optional<TestRequest> request;
  usb_device_qualifier_descriptor_t descriptor;
  AllocateRequest(&request, 1, sizeof(usb_device_qualifier_descriptor_t), 0,
                  [&](TestRequest request) {
                    transfer_status = request.request()->response.status;
                    transfer_len = request.request()->response.actual;
                    usb_device_qualifier_descriptor_t* result;
                    request.Mmap(reinterpret_cast<void**>(&result));
                    memcpy(&descriptor, result, sizeof(usb_device_qualifier_descriptor_t));
                  });
  request->request()->setup.b_request = USB_REQ_GET_DESCRIPTOR;
  request->request()->setup.w_index = 0;
  request->request()->setup.w_value = USB_DT_DEVICE_QUALIFIER << 8;
  request->request()->header.length = sizeof(usb_device_qualifier_descriptor_t);

  ctx->request = Borrow(std::move(*request));
  AddContext(std::move(ctx));
  SetCompletion(reinterpret_cast<TRB*>(kFakeTrbVirt));
  Interrupt();
  auto cmd = GetCommand();
  ASSERT_EQ(cmd->type, CommandType::ResetEndpoint);
  ASSERT_EQ(cmd->endpoint, 0UL);
  ASSERT_EQ(cmd->device_id, 1UL);
  cmd->completer.complete_ok(nullptr);
  Interrupt();

  ASSERT_EQ(transfer_status, ZX_OK);
  ASSERT_EQ(transfer_len, sizeof(usb_device_qualifier_descriptor_t));
  ASSERT_EQ(descriptor.b_device_protocol, 0);
}

TEST_F(EventRingHarness, USB2DeviceAttach) {
  ConnectDevice();
  InsertPortStatusChangeEvent();
  Interrupt();
  ASSERT_TRUE(PortResetPending());
  ASSERT_FALSE(HasCommand());
  LinkUp();
  InsertPortStatusChangeEvent();
  Interrupt();
  ASSERT_TRUE(HasCommand());
  auto cmd = GetCommand();
  ASSERT_EQ(cmd->port, kPortNo);
  ASSERT_EQ(cmd->type, CommandType::EnumerateDevice);
  ASSERT_FALSE(cmd->hub);
}

TEST_F(EventRingHarness, USB2DeviceAttachBadStateTransition) {
  ConnectDevice();
  InsertPortStatusChangeEvent();
  Interrupt();
  ASSERT_TRUE(PortResetPending());
  ASSERT_FALSE(HasCommand());
  LinkUp();
  MakeQuantumPort();
  InsertPortStatusChangeEvent();
  Interrupt();
  ASSERT_FALSE(HasCommand());
  LinkUp();
  InsertPortStatusChangeEvent();
  Interrupt();
  ASSERT_TRUE(HasCommand());
  auto cmd = GetCommand();
  ASSERT_EQ(cmd->port, kPortNo);
  ASSERT_EQ(cmd->type, CommandType::EnumerateDevice);
  ASSERT_FALSE(cmd->hub);
}

TEST_F(EventRingHarness, Usb3DeviceAttach) {
  ConnectDevice();
  LinkUp();
  InsertPortStatusChangeEvent();
  Interrupt();
  ASSERT_TRUE(HasCommand());
  auto cmd = GetCommand();
  ASSERT_EQ(cmd->port, kPortNo);
  ASSERT_EQ(cmd->type, CommandType::EnumerateDevice);
  ASSERT_FALSE(cmd->hub);
}

}  // namespace usb_xhci
