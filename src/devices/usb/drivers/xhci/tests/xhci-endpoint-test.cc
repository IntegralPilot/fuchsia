// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/usb/drivers/xhci/xhci-endpoint.h"

#include <lib/async-loop/cpp/loop.h>
#include <lib/driver/fake-bti/cpp/fake-bti.h>

#include <list>

#include <fake-dma-buffer/fake-dma-buffer.h>

#include "src/devices/usb/drivers/xhci/tests/test-env.h"
#include "src/devices/usb/drivers/xhci/xhci-device-state.h"

namespace usb_xhci {

struct FakeTRB : TRB {
  std::vector<TRB> contig;
};

constexpr uint32_t kDeviceId = 0;
constexpr uint32_t kSlot = kDeviceId + 1;
constexpr uint32_t kPort = 0;

class EndpointHarness : public ::testing::Test {
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

  void Init(uint8_t ep_addr, std::optional<fuchsia_hardware_usb_descriptor::EndpointType> ep_type =
                                 fuchsia_hardware_usb_descriptor::EndpointType::kBulk) {
    loop_.StartThread("ep-thread");
    ep_ = std::make_unique<Endpoint>(driver_test().driver(), kDeviceId, ep_addr);
    if (ep_type.has_value()) {
      ep_->set_ep_type(*ep_type);
    }
    EXPECT_OK(ep_->Init(nullptr, nullptr));

    // Connect client
    auto endpoints = fidl::Endpoints<fuchsia_hardware_usb_endpoint::Endpoint>::Create();
    ep_->Connect(loop_.dispatcher(), std::move(endpoints.server));
    client_.Bind(std::move(endpoints.client));
  }

  void TearDown() override {
    if (ep_) {
      expected_cancel_all_.emplace(kDeviceId, ep_->ep_addr());
      expected_disable_endpoint_.emplace(kDeviceId, ep_->ep_addr());

      auto unused = std::move(client_);
    }
    loop_.Shutdown();
    ep_.reset();

    EXPECT_TRUE(expected_cancel_all_.empty());
    EXPECT_TRUE(expected_ring_doorbell_.empty());
    EXPECT_TRUE(expected_disable_endpoint_.empty());

    ASSERT_TRUE(driver_test().StopDriver().is_ok());
  }

  FakeTRB* CreateTRB() {
    auto it = trbs_.insert(trbs_.end(), std::make_unique<FakeTRB>());
    (*it)->control = 0;
    (*it)->ptr = 0;
    (*it)->status = 0;
    return it->get();
  }

  FakeTRB* CreateTRBs(size_t count) {
    auto it = trbs_.insert(trbs_.end(), std::make_unique<FakeTRB>());
    (*it)->control = 0;
    (*it)->ptr = 0;
    (*it)->status = 0;
    (*it)->contig.resize(count);
    return it->get();
  }

  fdf_testing::ForegroundDriverTest<EmptyTestConfig>& driver_test() { return driver_test_; }
  const std::list<std::unique_ptr<FakeTRB>>& trbs() { return trbs_; }

  sync_completion_t doorbell_;
  std::queue<std::pair<uint32_t, uint8_t>> expected_cancel_all_;
  std::queue<std::pair<uint8_t, uint8_t>> expected_ring_doorbell_;
  std::queue<std::pair<uint32_t, uint8_t>> expected_disable_endpoint_;

 protected:
  std::unique_ptr<Endpoint> ep_;
  fidl::SyncClient<fuchsia_hardware_usb_endpoint::Endpoint> client_;

 private:
  async::Loop loop_{&kAsyncLoopConfigNeverAttachToThread};
  fdf_testing::ForegroundDriverTest<EmptyTestConfig> driver_test_;
  std::list<std::unique_ptr<FakeTRB>> trbs_;
};

// Fake implementations of UsbXhci
zx::result<> UsbXhci::Init(std::unique_ptr<dma_buffer::BufferFactory> buffer_factory) {
  buffer_factory_ = std::move(buffer_factory);

  slot_size_bytes_ = 64;

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

  // Create device_state_
  auto state = fbl::MakeRefCounted<DeviceState>(kDeviceId, this);
  fbl::AutoLock _(&state->transaction_lock());
  state->SetDeviceInformation(kSlot, kPort, std::nullopt);
  state->AddressDeviceCommand(this, kSlot, kPort, std::nullopt, nullptr, 0, nullptr, nullptr,
                              false);
  GetDeviceState()[kDeviceId] = std::move(state);

  zx::result bti = fake_bti::CreateFakeBti();
  EXPECT_OK(bti);
  bti_ = std::move(bti.value());
  return zx::ok();
}
void UsbXhci::Shutdown(zx_status_t status) {}
void UsbXhci::UsbHciRequestQueue(usb_request_t* usb_request,
                                 const usb_request_complete_callback_t* complete_cb) {}
zx_status_t UsbXhci::UsbHciCancelAll(uint32_t device_id, uint8_t ep_address) {
  auto* test_harness = GetTestHarness<EndpointHarness>();
  EXPECT_FALSE(test_harness->expected_cancel_all_.empty());
  EXPECT_EQ(test_harness->expected_cancel_all_.front().first, device_id);
  EXPECT_EQ(test_harness->expected_cancel_all_.front().second, ep_address);
  test_harness->expected_cancel_all_.pop();
  return ZX_OK;
}
fpromise::promise<void, zx_status_t> UsbXhci::UsbHciDisableEndpoint(uint32_t device_id,
                                                                    uint8_t ep_addr) {
  auto* test_harness = GetTestHarness<EndpointHarness>();
  EXPECT_FALSE(test_harness->expected_disable_endpoint_.empty());
  EXPECT_EQ(test_harness->expected_disable_endpoint_.front().first, device_id);
  EXPECT_EQ(test_harness->expected_disable_endpoint_.front().second, ep_addr);
  test_harness->expected_disable_endpoint_.pop();
  return fpromise::make_result_promise<void, zx_status_t>(fpromise::ok());
}
bool UsbXhci::Running() const { return true; }
void UsbXhci::RingDoorbell(uint8_t slot, uint8_t target) {
  auto* test_harness = GetTestHarness<EndpointHarness>();
  EXPECT_FALSE(test_harness->expected_ring_doorbell_.empty());
  EXPECT_EQ(test_harness->expected_ring_doorbell_.front().first, slot);
  EXPECT_EQ(
      reinterpret_cast<EndpointHarness*>(test_harness_)->expected_ring_doorbell_.front().second,
      target);
  test_harness->expected_ring_doorbell_.pop();

  sync_completion_signal(&test_harness->doorbell_);
}

// Fake implementations of DeviceState
TRBPromise DeviceState::AddressDeviceCommand(UsbXhci* hci, uint8_t slot_id, uint8_t port_id,
                                             std::optional<HubInfo> hub_info, uint64_t* dcbaa,
                                             uint16_t interrupter_target, CommandRing* command_ring,
                                             fdf::MmioBuffer* mmio, bool bsr) {
  interrupter_target_ = interrupter_target;
  EXPECT_OK(hci->buffer_factory().CreatePaged(
      hci->bti(), zx_system_get_page_size(), dma_buffer::CacheOptions::kDisabled, &input_context_));
  return fpromise::make_result_promise(fpromise::result<TRB*, zx_status_t>(fpromise::ok(nullptr)))
      .box();
}

// Fake implementations of EventRing
void EventRing::RemovePressure() {}
zx_status_t EventRing::AddSegmentIfNone() { return ZX_ERR_NOT_SUPPORTED; }
void EventRing::ScheduleTask(fpromise::promise<void, zx_status_t> promise) {
  SchedulePromiseWithXhciExecutorPolicy(executor_, hci_, std::move(promise));
}
void EventRing::RunUntilIdle() { executor_.run_until_idle(); }

// Fake implementations of TransferRing
zx_status_t TransferRing::Init(size_t page_size, const zx::bti& bti, EventRing* ring, bool is_32bit,
                               fdf::MmioBuffer* mmio, UsbXhci* hci) {
  hci_ = hci;
  fbl::AutoLock _(&mutex_);
  if (trbs_ != nullptr) {
    return ZX_ERR_BAD_STATE;
  }
  trbs_ = hci_->GetTestHarness<EndpointHarness>()->CreateTRB();
  static_assert(sizeof(uint64_t) == sizeof(this));
  trbs_->ptr = reinterpret_cast<uint64_t>(this);
  trbs_->status = pcs_;
  stalled_ = false;
  return ZX_OK;
}
zx_status_t TransferRing::DeinitIfActive() { return ZX_OK; }
zx_status_t TransferRing::AssignContext(TRB* trb, std::unique_ptr<TRBContext> context,
                                        TRB* first_trb, TRB* setup) {
  return ZX_OK;
}
void TransferRing::CommitTransaction(const State& start) {}
TransferRing::State TransferRing::SaveState() { return {}; }
void TransferRing::Restore(const State& state) {}
zx_status_t TransferRing::AllocateTRB(TRB** trb, State* state) {
  fbl::AutoLock _(&mutex_);
  if (state) {
    state->pcs = pcs_;
    state->trbs = trbs_;
  }
  trbs_ = hci_->GetTestHarness<EndpointHarness>()->CreateTRB();
  trbs_->ptr = 0;
  trbs_->status = pcs_;
  *trb = trbs_;
  return ZX_OK;
}
zx::result<ContiguousTRBInfo> TransferRing::AllocateContiguous(size_t count) {
  fbl::AutoLock _(&mutex_);
  trbs_ = hci_->GetTestHarness<EndpointHarness>()->CreateTRBs(count)->contig.data();
  trbs_->ptr = 0;
  trbs_->status = pcs_;
  ContiguousTRBInfo info;
  info.trbs = cpp20::span(trbs_, count);
  return zx::ok(info);
}

TEST_F(EndpointHarness, Empty) {}

TEST_F(EndpointHarness, Init) { Init(1); }

TEST_F(EndpointHarness, GetInfo) {
  Init(1);

  auto result = client_->GetInfo();
  ASSERT_TRUE(result.is_ok());
  ASSERT_TRUE(result->info().bulk().has_value());
  EXPECT_TRUE(result->info().bulk()->supports_scatter_gather().value());
}

TEST_F(EndpointHarness, QueueControlRequest) {
  // ep_addr needs to be 0 to queue a control request.
  Init(0);

  std::vector<fuchsia_hardware_usb_endpoint::VmoInfo> vmo_info;
  vmo_info.emplace_back(std::move(
      fuchsia_hardware_usb_endpoint::VmoInfo().id(8).size(2 * zx_system_get_page_size())));
  {
    auto result = client_->RegisterVmos({std::move(vmo_info)});
    ASSERT_TRUE(result.is_ok());
    EXPECT_EQ(result->vmos().size(), 1UL);
    EXPECT_EQ(result->vmos().at(0).id(), 8UL);
  }

  expected_ring_doorbell_.emplace(kSlot, 1);
  zx::vmo vmo;
  EXPECT_OK(zx::vmo::create(zx_system_get_page_size() * 2, 0, &vmo));
  std::vector<fuchsia_hardware_usb_request::Request> requests;
  ASSERT_LE(zx_system_get_page_size() * 2, static_cast<uint32_t>(UINT16_MAX));
  requests.emplace_back()
      .defer_completion(false)
      .information(fuchsia_hardware_usb_request::RequestInfo::WithControl(
          fuchsia_hardware_usb_request::ControlRequestInfo().setup(
              fuchsia_hardware_usb_descriptor::UsbSetup()
                  .bm_request_type(USB_DIR_IN | USB_TYPE_STANDARD | USB_RECIP_DEVICE)
                  .b_request(USB_REQ_GET_DESCRIPTOR)
                  .w_value(USB_DT_DEVICE << 8)
                  .w_length(static_cast<uint16_t>(zx_system_get_page_size() * 2)))))
      .data()
      .emplace()
      .emplace_back()
      .offset(0)
      .size(zx_system_get_page_size() * 2)
      .buffer(fuchsia_hardware_usb_request::Buffer::WithVmoId(8));
  auto result = client_->QueueRequests(std::move(requests));
  ASSERT_TRUE(result.is_ok());

  sync_completion_wait(&doorbell_, zx::time::infinite().get());

  // Setup
  auto iter = trbs().begin();
  auto setup_trb = static_cast<struct Setup*>(static_cast<TRB*>((++iter)->get()));
  EXPECT_EQ(setup_trb->length(), 8UL);
  EXPECT_EQ(setup_trb->IDT(), 1UL);
  EXPECT_EQ(setup_trb->TRT(), Setup::IN);
  // Data
  auto data_trb = static_cast<ControlData*>(static_cast<TRB*>((++iter)->get()));
  EXPECT_EQ(data_trb->DIRECTION(), 1UL);
  EXPECT_EQ(data_trb->INTERRUPTER(), 0UL);
  EXPECT_EQ(data_trb->LENGTH(), zx_system_get_page_size());
  EXPECT_EQ(data_trb->SIZE(), 1UL);
  EXPECT_TRUE(data_trb->ISP());
  EXPECT_TRUE(data_trb->NO_SNOOP());
  // Normal
  auto normal_trb = static_cast<Normal*>(static_cast<TRB*>((++iter)->get()));
  EXPECT_EQ(normal_trb->INTERRUPTER(), 0UL);
  EXPECT_EQ(normal_trb->LENGTH(), zx_system_get_page_size());
  EXPECT_EQ(normal_trb->SIZE(), 0UL);
  EXPECT_TRUE(normal_trb->ISP());
  EXPECT_TRUE(normal_trb->NO_SNOOP());
  // Status
  auto status_trb = static_cast<Status*>(static_cast<TRB*>((++iter)->get()));
  EXPECT_EQ(status_trb->DIRECTION(), 0UL);
  EXPECT_EQ(status_trb->INTERRUPTER(), 0UL);
  EXPECT_TRUE(status_trb->IOC());
}

TEST_F(EndpointHarness, QueueNormalRequest) {
  // ep_addr needs to be non-0 to queue a normal request.
  Init(1);

  std::vector<fuchsia_hardware_usb_endpoint::VmoInfo> vmo_info;
  vmo_info.emplace_back(std::move(
      fuchsia_hardware_usb_endpoint::VmoInfo().id(8).size(2 * zx_system_get_page_size())));
  {
    auto result = client_->RegisterVmos({std::move(vmo_info)});
    ASSERT_TRUE(result.is_ok());
    EXPECT_EQ(result->vmos().size(), 1UL);
    EXPECT_EQ(result->vmos().at(0).id(), 8UL);
  }

  expected_ring_doorbell_.emplace(kSlot, 2 + kDeviceId);
  std::vector<fuchsia_hardware_usb_request::Request> requests;
  requests.emplace_back()
      .defer_completion(false)
      .information(fuchsia_hardware_usb_request::RequestInfo::WithBulk(
          fuchsia_hardware_usb_request::BulkRequestInfo()))
      .data()
      .emplace()
      .emplace_back()
      .offset(0)
      .size(zx_system_get_page_size() * 2)
      .buffer(fuchsia_hardware_usb_request::Buffer::WithVmoId(8));
  auto result = client_->QueueRequests(std::move(requests));
  ASSERT_TRUE(result.is_ok());

  sync_completion_wait(&doorbell_, zx::time::infinite().get());

  // Data (page 0)
  auto trb = (++trbs().begin())->get()->contig.data();
  EXPECT_EQ(Control::FromTRB(trb).Type(), Control::Normal);
  auto data_trb = static_cast<Normal*>(trb);
  EXPECT_EQ(data_trb->IOC(), 0UL);
  EXPECT_EQ(data_trb->ISP(), 1UL);
  EXPECT_EQ(data_trb->INTERRUPTER(), 0UL);
  EXPECT_EQ(data_trb->LENGTH(), zx_system_get_page_size());
  EXPECT_EQ(data_trb->SIZE(), 1UL);
  EXPECT_TRUE(data_trb->NO_SNOOP());

  // Data (page 1, contiguous)
  data_trb = static_cast<Normal*>(++trb);
  EXPECT_EQ(data_trb->IOC(), 1UL);
  EXPECT_EQ(data_trb->ISP(), 1UL);
  EXPECT_EQ(data_trb->INTERRUPTER(), 0UL);
  EXPECT_EQ(data_trb->LENGTH(), zx_system_get_page_size());
  EXPECT_EQ(data_trb->SIZE(), 0UL);
  EXPECT_TRUE(data_trb->NO_SNOOP());
}

}  // namespace usb_xhci
