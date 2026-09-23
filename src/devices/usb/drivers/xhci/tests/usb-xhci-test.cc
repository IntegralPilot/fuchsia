// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/usb/drivers/xhci/usb-xhci.h"

#include <lib/driver/fake-mmio-reg/cpp/fake-mmio-reg.h>
#include <lib/driver/fake-platform-device/cpp/fake-pdev.h>
#include <lib/driver/testing/cpp/driver_test.h>
#include <lib/sync/completion.h>
#include <lib/zx/bti.h>
#include <lib/zx/vmar.h>

#include <list>

#include <fake-dma-buffer/fake-dma-buffer.h>
#include <gtest/gtest.h>

#include "src/devices/usb/drivers/xhci/tests/test-env.h"
#include "src/lib/testing/predicates/status.h"

namespace usb_xhci {

namespace fdescriptor = fuchsia_hardware_usb_descriptor;

struct FakeTRB;
struct FakePhysAddr {
  uint64_t magic;
  FakeTRB* value;
};

struct FakeTRB : TRB {
  FakeTRB() {
    phys_addr_ = std::make_unique<FakePhysAddr>();
    phys_addr_->magic = kMagicValue;
    phys_addr_->value = this;
  }

  zx_paddr_t phys() { return reinterpret_cast<zx_paddr_t>(phys_addr_.get()); }

  static bool is_valid_paddr(zx_paddr_t phys) {
    return *reinterpret_cast<uint64_t*>(phys) == kMagicValue;
  }

  static FakeTRB* get(zx_paddr_t phys) {
    if (!is_valid_paddr(phys)) {
      return nullptr;
    }
    FakePhysAddr* addr = reinterpret_cast<FakePhysAddr*>(phys);
    return addr->value;
  }

  ~FakeTRB() {
    // Cast to volatile to prevent the compiler
    // from optimizing out this zero operation.
    *reinterpret_cast<volatile uint64_t*>(phys_addr_.get()) = 0;
  }

  static std::unique_ptr<FakeTRB> FromTRB(TRB* trb) {
    return std::unique_ptr<FakeTRB>(static_cast<FakeTRB*>(trb));
  }

  // Magic value to use for determining if a physical address is valid or not.
  // ASAN builds should also trigger an error if we try dereferencing something
  // that isn't valid. This value is a fallback for cases where ASAN isn't being used.
  static constexpr uint64_t kMagicValue = 0x12345678901ABCDEU;
  std::unique_ptr<FakePhysAddr> phys_addr_;
  std::vector<TRB> contig;
  zx_paddr_t next = 0;
  zx_paddr_t prev = 0;
};

class FakeDevice {
 public:
  FakeDevice() {
    constexpr auto kHcsParams2 = 2 * sizeof(uint32_t);
    constexpr auto kHccParams1 = 4 * sizeof(uint32_t);
    constexpr auto kXecp = 320 * sizeof(uint32_t);
    constexpr auto kOffset = 0 * sizeof(uint32_t);
    constexpr auto kHcsParams1 = 1 * sizeof(uint32_t);
    constexpr auto kOffset1 = 5 * sizeof(uint32_t);
    constexpr auto kOffset2 = 6 * sizeof(uint32_t);
    constexpr auto kUsbCmd = 7 * sizeof(uint32_t);
    constexpr auto kUsbSts = 8 * sizeof(uint32_t);
    constexpr auto kUsbPageSize = 9 * sizeof(uint32_t);
    constexpr auto kConfig = 14 * sizeof(uint32_t);
    constexpr auto kCrCr = 13 * sizeof(uint32_t);
    constexpr auto kDcbaa = 19 * sizeof(uint32_t);
    constexpr auto kDoorbellBase = 1024 * sizeof(uint32_t);
    constexpr auto kImodi = 457 * sizeof(uint32_t);

    constexpr uint8_t kCapLength = 0x1c;

    region_.emplace(sizeof(uint32_t), 2048);

    region()[kHcsParams2].SetReadCallback([]() {
      auto params = HCSPARAMS2::Get().FromValue(0);
      params.set_ERST_MAX(4);
      params.set_MAX_SCRATCHPAD_BUFFERS_LOW(1);
      return params.reg_value();
    });

    region()[kHccParams1].SetReadCallback([]() {
      auto params = HCCPARAMS1::Get().FromValue(0);
      params.set_AC64(true);
      params.set_CSZ(true);
      params.set_xECP(320);
      return params.reg_value();
    });

    region()[kXecp].SetReadCallback([=]() {
      auto xecp = (XECP::Get(HCCPARAMS1::Get().FromValue(
                                 static_cast<uint32_t>(region()[4 * sizeof(uint32_t)].Read())))
                       .FromValue(0));
      xecp.set_NEXT(0);
      xecp.set_ID(XECP::UsbLegacySupport);
      if (driver_owned_controller_) {
        xecp.set_reg_value(xecp.reg_value() | 1 << 24);
      } else {
        xecp.set_reg_value(xecp.reg_value() | 1 << 16);
      }
      return xecp.reg_value();
    });

    region()[kXecp].SetWriteCallback([=](uint64_t value) {
      if (value & (1 << 24)) {
        driver_owned_controller_ = true;
      }
    });
    region()[kOffset].SetReadCallback([=]() { return kCapLength; });

    region()[kHcsParams1].SetReadCallback([=]() {
      HCSPARAMS1 parms = HCSPARAMS1::Get().FromValue(0);
      parms.set_MaxIntrs(1);
      parms.set_MaxPorts(4);
      parms.set_MaxSlots(32);
      return parms.reg_value();
    });

    region()[kOffset1].SetReadCallback([=]() { return 0x1000; });

    region()[kOffset2].SetReadCallback([=]() { return 0x700; });

    region()[kUsbCmd].SetReadCallback([=]() {
      USBCMD cmd = USBCMD::Get(static_cast<uint8_t>(region()[0].Read())).FromValue(0);
      cmd.set_ENABLE(controller_enabled_);
      cmd.set_EWE(event_wrap_enable_);
      cmd.set_HSEE(host_system_error_enable_);
      cmd.set_INTE(irq_enable_);
      cmd.set_RESET(0);
      return cmd.reg_value();
    });

    region()[kUsbCmd].SetWriteCallback([=](uint64_t value) {
      USBCMD cmd = USBCMD::Get(static_cast<uint8_t>(region()[0].Read()))
                       .FromValue(static_cast<uint32_t>(value));
      if (cmd.RESET()) {
        controller_was_reset_ = true;
      }
      controller_enabled_ = cmd.ENABLE();
      event_wrap_enable_ = cmd.EWE();
      host_system_error_enable_ = cmd.HSEE();
      irq_enable_ = cmd.INTE();
    });

    region()[kUsbSts].SetReadCallback([=]() {
      auto sts = USBSTS::Get(kCapLength).FromValue(0);
      sts.set_HCHalted(!controller_enabled_);
      return sts.reg_value();
    });

    region()[kUsbPageSize].SetReadCallback([=]() {
      USB_PAGESIZE size = USB_PAGESIZE::Get(kCapLength).FromValue(0);
      size.set_PageSize(1);
      return size.reg_value();
    });

    region()[kConfig].SetReadCallback([=]() {
      CONFIG config = CONFIG::Get(kCapLength).FromValue(0);
      config.set_MaxSlotsEn(slots_enabled_);
      return config.reg_value();
    });

    region()[kConfig].SetWriteCallback([=](uint64_t value) {
      CONFIG config = CONFIG::Get(kCapLength).FromValue(static_cast<uint32_t>(value));
      slots_enabled_ = config.MaxSlotsEn();
    });

    region()[kCrCr].SetWriteCallback([=](uint64_t value) {
      CRCR cr = CRCR::Get(kCapLength).FromValue(value);
      crcr_ = reinterpret_cast<zx_paddr_t>(cr.PTR());
    });

    region()[kDcbaa].SetReadCallback([=]() { return dcbaa_; });
    region()[kDcbaa].SetWriteCallback([=](uint64_t value) {
      auto val = DCBAAP::Get(kCapLength).FromValue(value);
      dcbaa_ = val.PTR();
    });
    doorbell_callback_ = [](uint8_t doorbell, uint8_t target) {};
    for (size_t i = 0; i < 32; i++) {
      region()[kDoorbellBase + i * sizeof(uint32_t)].SetWriteCallback([=](uint64_t value) {
        auto buffer = mmio();
        auto bell = DOORBELL::Get(DoorbellOffset::Get().ReadFrom(&buffer), 0)
                        .FromValue(static_cast<uint32_t>(value));
        doorbell_callback_(static_cast<uint8_t>(i), static_cast<uint8_t>(bell.Target()));
      });
    }
    region()[kImodi].SetWriteCallback([=](uint64_t value) {
      auto buffer = mmio();
      auto imodi = IMODI::Get(RuntimeRegisterOffset::Get().ReadFrom(&buffer), 0)
                       .FromValue(static_cast<uint32_t>(value));
      imodi_ = static_cast<uint16_t>(imodi.MODI());
    });

    // Control register
  }

  fdf::MmioBuffer mmio() { return region_->GetMmioBuffer(); }
  fake_mmio::FakeMmioRegRegion& region() { return *region_; }

  void set_irq_signaller(zx::unowned_interrupt signaller) { irq_signaller_ = std::move(signaller); }

  void SetDoorbellCallback(fit::function<void(uint8_t, uint8_t)> callback) {
    doorbell_callback_ = std::move(callback);
  }

  FakeTRB* crcr() { return FakeTRB::get(crcr_); }

 private:
  std::optional<fake_mmio::FakeMmioRegRegion> region_;
  zx::unowned_interrupt irq_signaller_;
  bool driver_owned_controller_ = false;
  bool controller_enabled_ = false;
  bool controller_was_reset_ = false;
  bool event_wrap_enable_ = false;
  bool irq_enable_ = false;
  bool host_system_error_enable_ = false;
  uint32_t slots_enabled_ = 0;
  zx_paddr_t crcr_ = 0;
  zx_paddr_t dcbaa_ = 0;
  uint16_t imodi_;
  fit::function<void(uint8_t, uint8_t)> doorbell_callback_;
};

struct FakeUsbDevice {
  uint32_t device_id;
  uint32_t hub_id;
  usb_speed_t speed;
  bool fake_root_hub;
};

class FakeUsbBus : public ddk::UsbBusInterfaceProtocol<FakeUsbBus> {
 public:
  FakeUsbBus() = default;

  void reset() { sync_completion_reset(&completion_); }

  void wait() { sync_completion_wait(&completion_, ZX_TIME_INFINITE); }

  usb_bus_interface_protocol_t* proto() {
    proto_.ctx = this;
    proto_.ops = &usb_bus_interface_protocol_ops_;
    return &proto_;
  }

  zx_status_t UsbBusInterfaceAddDevice(uint32_t device_id, uint32_t hub_id, usb_speed_t speed) {
    FakeUsbDevice fake_device;
    fake_device.device_id = device_id;
    fake_device.hub_id = hub_id;
    fake_device.speed = speed;
    fake_device.fake_root_hub = device_id >= 32;
    devices_[device_id] = fake_device;
    sync_completion_signal(&completion_);
    return ZX_OK;
  }

  zx_status_t UsbBusInterfaceRemoveDevice(uint32_t device_id) {
    devices_.erase(device_id);
    return ZX_OK;
  }

  zx_status_t UsbBusInterfaceResetPort(uint32_t hub_id, uint32_t port, bool enumerating) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  zx_status_t UsbBusInterfaceReinitializeDevice(uint32_t device_id) { return ZX_ERR_NOT_SUPPORTED; }
  const std::map<uint32_t, FakeUsbDevice>& devices() { return devices_; }

 private:
  std::map<uint32_t, FakeUsbDevice> devices_;
  sync_completion_t completion_;
  usb_bus_interface_protocol_t proto_;
};

class XhciTestEnvironment : fdf_testing::Environment {
 public:
  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override {
    device_server_.Initialize(component::kDefaultInstance);
    EXPECT_OK(
        device_server_.Serve(fdf::Dispatcher::GetCurrent()->async_dispatcher(), &to_driver_vfs));

    fdf_fake::FakePDev::Config config;
    config.mmios[0] = fake_device_.mmio();
    config.irqs[0] = {};
    EXPECT_OK(zx::interrupt::create(zx::resource(), 0, ZX_INTERRUPT_VIRTUAL, &config.irqs[0]));
    fake_device_.set_irq_signaller(config.irqs[0].borrow());
    config.use_fake_bti = true;
    pdev_server_.SetConfig(std::move(config));
    return to_driver_vfs.AddService<fuchsia_hardware_platform_device::Service>(
        pdev_server_.GetInstanceHandler(fdf::Dispatcher::GetCurrent()->async_dispatcher()), "pdev");
  }

  FakeDevice& fake_device() { return fake_device_; }

 private:
  compat::DeviceServer device_server_;
  fdf_fake::FakePDev pdev_server_;
  FakeDevice fake_device_;
};

class XhciTestConfig final {
 public:
  using DriverType = usb_xhci::UsbXhci;
  using EnvironmentType = XhciTestEnvironment;
};

using TestRequest = usb::CallbackRequest<sizeof(max_align_t)>;
class XhciHarness : public ::testing::Test {
 public:
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

  size_t GetMaxDeviceCount() { return driver_test().driver()->UsbHciGetMaxDeviceCount(); }

  void RequestQueue(TestRequest request) {
    static const usb_protocol_ops_t ops = {
        .request_queue =
            [](void* ctx, usb_request_t* usb_request,
               const usb_request_complete_callback_t* complete_cb) {
              auto* driver = static_cast<UsbXhci*>(ctx);
              driver->UsbHciRequestQueue(usb_request, complete_cb);
            },
    };
    usb_protocol_t proto = {
        .ops = &ops,
        .ctx = driver_test().driver(),
    };
    ddk::UsbProtocolClient client(&proto);
    TestRequest::Queue(std::move(request), client);
  }

  template <typename Callback>
  zx_status_t AllocateRequest(std::optional<TestRequest>* request, uint32_t device_id,
                              uint64_t data_size, uint8_t endpoint, Callback callback) {
    zx_status_t result =
        TestRequest::Alloc(request, data_size, endpoint,
                           driver_test().driver()->UsbHciGetRequestSize(), std::move(callback));
    if (result != ZX_OK) {
      return result;
    }
    void* virt;
    (*request)->Mmap(&virt);
    static_assert(sizeof(uint64_t) == sizeof(void*));
    size_t phys_count =
        fbl::round_up(data_size, zx_system_get_page_size()) / zx_system_get_page_size();
    (*request)->request()->phys_count = phys_count;
    // Need to use malloc for compatibility with the C ABI (which will eventually call free)
    (*request)->request()->phys_list =
        static_cast<zx_paddr_t*>(malloc(sizeof(zx_paddr_t) * phys_count));
    for (size_t i = 0; i < phys_count; i++) {
      auto trb = CreateTRB();
      trb->ptr = reinterpret_cast<uint64_t>(virt) + (zx_system_get_page_size() * i);
      (*request)->request()->phys_list[i] = trb->phys();
    }
    (*request)->request()->header.device_id = device_id;
    return ZX_OK;
  }

  uint8_t AllocateSlot() {
    if (slot_freelist_.empty()) {
      slot_id_++;
      return slot_id_;
    }
    uint8_t retval = slot_freelist_.back();
    slot_freelist_.pop_back();
    return retval;
  }

  FakeUsbDevice ConnectDevice(uint8_t port, usb_speed_t speed) {
    std::optional<HubInfo> hub;
    uint8_t slot = AllocateSlot();
    driver_test().driver()->GetPortState()[port - 1].is_connected = true;
    driver_test().driver()->GetPortState()[port - 1].link_active = true;
    driver_test().driver()->GetPortState()[port - 1].slot_id = slot;
    driver_test().driver()->SetDeviceInformation(slot, slot, hub);
    driver_test().driver()->AddressDeviceCommand(slot, port, hub, true);
    bus_.reset();
    driver_test().driver()->DeviceOnline(slot, port, speed);
    EXPECT_OK(driver_test().RunOnBackgroundDispatcherSync([this]() { bus_.wait(); }));
    return bus_.devices().find(slot - 1)->second;
  }

  void EnableEndpoint(uint32_t device_id, uint8_t ep_num, bool is_in_endpoint) {
    usb_endpoint_descriptor_t ep_desc = {};
    ep_desc.bm_attributes = static_cast<uint8_t>(fdescriptor::EndpointType::kBulk);
    ep_desc.b_endpoint_address = ep_num | (is_in_endpoint ? 0x80 : 0);
    driver_test().driver()->UsbHciEnableEndpoint(device_id, &ep_desc, nullptr);
  }

  zx_status_t CompleteCommand(TRB* trb, CommandCompletionEvent* event) {
    std::unique_ptr<TRBContext> context;
    zx_status_t status = driver_test().driver()->GetCommandRing()->CompleteTRB(trb, &context);
    if (status != ZX_OK) {
      return status;
    }
    context->completer->complete_ok(event);
    return ZX_OK;
  }

  void SetDoorbellListener(fit::function<void(uint8_t, uint8_t)> listener) {
    driver_test().RunInEnvironmentTypeContext([&](XhciTestEnvironment& env) {
      env.fake_device().SetDoorbellCallback(std::move(listener));
    });
  }

  FakeTRB* crcr_next() {
    return FakeTRB::get(driver_test().RunInEnvironmentTypeContext<zx_paddr_t>(
        [](XhciTestEnvironment& env) { return env.fake_device().crcr()->next; }));
  }

  ~XhciHarness() { trbs_.clear(); }

  fdf_testing::ForegroundDriverTest<XhciTestConfig>& driver_test() { return driver_test_; }

 protected:
  FakeUsbBus bus_;

 private:
  fdf_testing::ForegroundDriverTest<XhciTestConfig> driver_test_;
  std::vector<uint8_t> slot_freelist_;
  uint8_t slot_id_ = 0;
  std::list<std::unique_ptr<FakeTRB>> trbs_;
};

class XhciMmioHarness : public XhciHarness {
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
    driver_test().driver()->UsbHciSetBusInterface(bus_.proto());
  }

  void TearDown() override { ASSERT_TRUE(driver_test().StopDriver().is_ok()); }
};

fbl::DoublyLinkedList<std::unique_ptr<TRBContext>> TransferRing::TakePendingTRBs() {
  fbl::AutoLock al(&mutex_);
  return std::move(pending_trbs_);
}

zx::result<> UsbXhci::TestInit(void* test_harness) {
  test_harness_ = test_harness;
  return Init(ddk_fake::CreateBufferFactory());
}

void EventRing::ScheduleTask(fpromise::promise<void, zx_status_t> promise) {
  SchedulePromiseWithXhciExecutorPolicy(executor_, hci_, std::move(promise));
}

void EventRing::RunUntilIdle() { executor_.run_until_idle(); }

zx_status_t TransferRing::AllocateTRB(TRB** trb, State* state) {
  fbl::AutoLock _(&mutex_);
  if (state) {
    state->pcs = pcs_;
    state->trbs = trbs_;
  }
  auto new_trb = hci_->GetTestHarness<XhciHarness>()->CreateTRB();
  new_trb->prev = static_cast<FakeTRB*>(trbs_)->phys();
  static_cast<FakeTRB*>(trbs_)->next = new_trb->phys();
  trbs_ = new_trb;
  trbs_->ptr = 0;
  trbs_->status = pcs_;
  *trb = trbs_;
  return ZX_OK;
}

zx::result<ContiguousTRBInfo> TransferRing::AllocateContiguous(size_t count) {
  fbl::AutoLock _(&mutex_);
  auto new_trb = hci_->GetTestHarness<XhciHarness>()->CreateTRBs(count);
  new_trb->prev = static_cast<FakeTRB*>(trbs_)->phys();
  static_cast<FakeTRB*>(trbs_)->next = new_trb->phys();
  trbs_ = new_trb->contig.data();
  trbs_->ptr = 0;
  trbs_->status = pcs_;
  ContiguousTRBInfo info;
  info.trbs = cpp20::span(trbs_, count);
  return zx::ok(info);
}

constexpr auto kPeekPtr = 0x13823990000;

zx::result<CRCR> TransferRing::TransferRing::PeekCommandRingControlRegister(uint8_t cap_length) {
  fbl::AutoLock l(&mutex_);
  CRCR cr;
  cr.set_RCS(pcs_);
  cr.set_PTR(kPeekPtr);
  return zx::ok(cr);
}

zx_status_t TransferRing::CompleteTRB(TRB* trb, std::unique_ptr<TRBContext>* context) {
  fbl::AutoLock _(&mutex_);
  if (pending_trbs_.is_empty()) {
    return ZX_ERR_CANCELED;
  }
  dequeue_trb_ = trb;
  *context = pending_trbs_.pop_front();
  if (trb != (*context)->trb) {
    return ZX_ERR_IO;
  }
  return ZX_OK;
}

void TransferRing::CommitTransaction(const State& start) {}

zx_status_t TransferRing::AssignContext(TRB* trb, std::unique_ptr<TRBContext> context, TRB* first,
                                        TRB* setup) {
  fbl::AutoLock _(&mutex_);
  if (context->token != token_) {
    return ZX_ERR_INVALID_ARGS;
  }
  context->trb = trb;
  pending_trbs_.push_back(std::move(context));
  return ZX_OK;
}

zx_status_t TransferRing::Init(size_t page_size, const zx::bti& bti, EventRing* ring, bool is_32bit,
                               fdf::MmioBuffer* mmio, UsbXhci* hci) {
  fbl::AutoLock _(&mutex_);
  if (trbs_ != nullptr) {
    return ZX_ERR_BAD_STATE;
  }
  page_size_ = page_size;
  bti_ = &bti;
  ring_ = ring;
  is_32_bit_ = is_32bit;
  mmio_ = mmio;
  isochronous_ = false;
  token_++;
  stalled_ = false;
  hci_ = hci;
  trbs_ = hci_->GetTestHarness<XhciHarness>()->CreateTRB();
  static_assert(sizeof(uint64_t) == sizeof(this));
  trbs_->ptr = reinterpret_cast<uint64_t>(this);
  trbs_->status = pcs_;
  trb_start_phys_ = static_cast<FakeTRB*>(trbs_)->phys();
  return ZX_OK;
}

CRCR TransferRing::TransferRing::phys(uint8_t cap_length) __TA_NO_THREAD_SAFETY_ANALYSIS {
  CRCR cr = CRCR::Get(cap_length).FromValue(trb_start_phys_);
  assert(trb_start_phys_);
  cr.set_RCS(pcs_);
  return cr;
}

TransferRing::State TransferRing::SaveState() {
  fbl::AutoLock _(&mutex_);
  State state;
  state.pcs = pcs_;
  state.trbs = trbs_;
  return state;
}

void TransferRing::Restore(const State& state) {
  fbl::AutoLock _(&mutex_);
  trbs_ = state.trbs;
  pcs_ = state.pcs;
}

zx_status_t TransferRing::AddTRB(const TRB& trb, std::unique_ptr<TRBContext> context) {
  fbl::AutoLock _(&mutex_);
  if (context->token != token_) {
    return ZX_ERR_INVALID_ARGS;
  }
  FakeTRB* alloc_trb;
  alloc_trb = hci_->GetTestHarness<XhciHarness>()->CreateTRB();
  alloc_trb->prev = static_cast<FakeTRB*>(trbs_)->phys();
  static_cast<FakeTRB*>(trbs_)->next = alloc_trb->phys();
  trbs_ = alloc_trb;
  alloc_trb->control = trb.control;
  alloc_trb->ptr = trb.ptr;
  alloc_trb->status = trb.status;
  context->token = token_;
  context->trb = alloc_trb;
  pending_trbs_.push_back(std::move(context));
  return ZX_OK;
}

zx_status_t TransferRing::Deinit() {
  fbl::AutoLock _(&mutex_);
  if (!trbs_) {
    return ZX_ERR_BAD_STATE;
  }
  trbs_ = nullptr;
  dequeue_trb_ = nullptr;
  pcs_ = true;
  return ZX_OK;
}

zx_status_t TransferRing::DeinitIfActive() __TA_NO_THREAD_SAFETY_ANALYSIS {
  if (trbs_) {
    return Deinit();
  }
  return ZX_OK;
}

zx_paddr_t TransferRing::VirtToPhys(TRB* trb) {
  auto phys = static_cast<FakeTRB*>(trb)->phys();
  ZX_ASSERT(FakeTRB::is_valid_paddr(phys));
  return phys;
}

zx_status_t EventRingSegmentTable::Init(size_t page_size, const zx::bti& bti, bool is_32bit,
                                        uint32_t erst_max, ERSTSZ erst_size,
                                        const dma_buffer::BufferFactory& factory,
                                        fdf::MmioBuffer* mmio) {
  erst_size_ = erst_size;
  bti_ = &bti;
  page_size_ = page_size;
  is_32bit_ = is_32bit;
  mmio_.emplace(mmio->View(0));
  zx_status_t status = factory.CreatePaged(bti, zx_system_get_page_size(),
                                           dma_buffer::CacheOptions::kDisabled, &erst_);
  if (status != ZX_OK) {
    return status;
  }

  count_ = page_size / sizeof(ERSTEntry);
  if (count_ > erst_max) {
    count_ = erst_max;
  }
  entries_ = static_cast<ERSTEntry*>(erst_->virt());
  return ZX_OK;
}

zx_status_t EventRing::Init(size_t page_size, const zx::bti& bti, fdf::MmioBuffer* buffer,
                            bool is_32bit, uint32_t erst_max, ERSTSZ erst_size, ERDP erdp_reg,
                            IMAN iman_reg, uint8_t cap_length, HCSPARAMS1 hcs_params_1,
                            CommandRing* command_ring, DoorbellOffset doorbell_offset, UsbXhci* hci,
                            HCCPARAMS1 hcc_params_1, uint64_t* dcbaa, uint16_t interrupter,
                            inspect::Node* interrupter_node) {
  fbl::AutoLock _(&segment_mutex_);
  erdp_reg_ = erdp_reg;
  hcs_params_1_ = hcs_params_1;
  mmio_ = buffer;
  bti_ = &bti;
  page_size_ = page_size;
  is_32bit_ = is_32bit;
  mmio_ = buffer;
  iman_reg_ = iman_reg;
  cap_length_ = cap_length;
  command_ring_ = command_ring;
  doorbell_offset_ = doorbell_offset;
  hci_ = hci;
  hcc_params_1_ = hcc_params_1;
  dcbaa_ = dcbaa;
  static_assert(sizeof(zx_paddr_t) == sizeof(this));
  erdp_phys_ = reinterpret_cast<zx_paddr_t>(this);
  interrupter_ = interrupter;
  return segments_.Init(page_size, bti, is_32bit, erst_max, erst_size, hci->buffer_factory(),
                        mmio_);
}

size_t EventRing::GetPressure() { return 0; }

zx_status_t Interrupter::Init(uint16_t interrupter, size_t page_size, fdf::MmioBuffer* buffer,
                              const RuntimeRegisterOffset& offset, uint32_t erst_max,
                              DoorbellOffset doorbell_offset, UsbXhci* hci, HCCPARAMS1 hcc_params_1,
                              uint64_t* dcbaa) {
  interrupter_ = interrupter;
  hci_ = hci;
  return ZX_OK;
}

zx_status_t Interrupter::Start(const RuntimeRegisterOffset& offset,
                               fdf::MmioView interrupter_regs) {
  return ZX_OK;
}

zx_status_t Interrupter::StartIrqThread() { return ZX_OK; }

fpromise::promise<void, zx_status_t> Interrupter::Timeout(zx::time deadline) {
  return fpromise::make_result_promise<void, zx_status_t>(fpromise::ok());
}

// Enumerates a device as specified in xHCI section 4.3 starting from step 4
// This method should be called once the physical port of a device has been
// initialized.
fpromise::promise<void, zx_status_t> EnumerateDevice(UsbXhci* hci, uint8_t port,
                                                     std::optional<HubInfo> hub_info) {
  fpromise::bridge<void, zx_status_t> bridge;
  return bridge.consumer.promise();
}

struct alignas(4096) FakeVMO {
  size_t size;
  uint32_t alignment_log2;
  bool enable_cache;
  zx::vmo backing_storage;
  void* virt;
};

TEST_F(XhciMmioHarness, QueueControlRequest) {
  ConnectDevice(1, USB_SPEED_HIGH);
  bool rang = false;
  SetDoorbellListener([&](uint8_t doorbell, uint8_t target) {
    if (doorbell == 1 && target == 1) {
      rang = true;
    }
  });

  std::optional<TestRequest> request;
  bool invoked = false;
  AllocateRequest(&request, 0, zx_system_get_page_size() * 2, 0, [&](TestRequest request) {
    invoked = true;
    void** parameters;
    request.Mmap(reinterpret_cast<void**>(&parameters));
    EXPECT_EQ(*parameters, parameters);
  });
  request->request()->setup.bm_request_type = USB_DIR_IN | USB_TYPE_STANDARD | USB_RECIP_DEVICE;
  request->request()->setup.b_request = USB_REQ_GET_DESCRIPTOR;
  request->request()->setup.w_value = USB_DT_DEVICE << 8;
  ASSERT_LE(zx_system_get_page_size() * 2, static_cast<uint32_t>(UINT16_MAX));
  request->request()->setup.w_length = static_cast<uint16_t>(zx_system_get_page_size() * 2);
  RequestQueue(std::move(*request));
  ASSERT_TRUE(rang);
  // Find slot context pointer in address device command
  auto cr = crcr_next();
  Control control_trb = Control::FromTRB(cr);
  ASSERT_EQ(control_trb.Type(), Control::AddressDeviceCommand);
  auto control = static_cast<unsigned char*>(reinterpret_cast<FakeVMO*>(cr->ptr)->virt);
  auto endpoint_context = reinterpret_cast<EndpointContext*>(control + (64 * 2));
  auto ring_phys =
      static_cast<zx_paddr_t>(static_cast<uint64_t>(endpoint_context->dequeue_pointer_a) |
                              (static_cast<uint64_t>(endpoint_context->dequeue_pointer_b) << 32)) &
      (~1);
  auto trb = FakeTRB::get(ring_phys);
  auto initial_trb = trb;
  // Setup
  trb = FakeTRB::get(trb->next);
  auto setup_trb = static_cast<struct Setup*>(static_cast<TRB*>(trb));
  ASSERT_EQ(setup_trb->length(), 8UL);
  ASSERT_EQ(setup_trb->IDT(), 1UL);
  ASSERT_EQ(setup_trb->TRT(), Setup::IN);
  // Data
  trb = FakeTRB::get(trb->next);
  auto data_trb = static_cast<ControlData*>(static_cast<TRB*>(trb));
  ASSERT_EQ(data_trb->DIRECTION(), 1UL);
  ASSERT_EQ(data_trb->INTERRUPTER(), 0UL);
  ASSERT_EQ(data_trb->LENGTH(), zx_system_get_page_size());
  ASSERT_EQ(data_trb->SIZE(), 1UL);
  ASSERT_TRUE(data_trb->ISP());
  ASSERT_TRUE(data_trb->NO_SNOOP());
  void** virt = reinterpret_cast<void**>(FakeTRB::get(static_cast<zx_paddr_t>(data_trb->ptr))->ptr);
  *virt = virt;
  // Normal
  trb = FakeTRB::get(trb->next);
  auto normal_trb = static_cast<Normal*>(static_cast<TRB*>(trb));
  ASSERT_EQ(normal_trb->INTERRUPTER(), 0UL);
  ASSERT_EQ(normal_trb->LENGTH(), zx_system_get_page_size());
  ASSERT_EQ(normal_trb->SIZE(), 0UL);
  ASSERT_TRUE(normal_trb->ISP());
  ASSERT_TRUE(normal_trb->NO_SNOOP());
  // Status
  trb = FakeTRB::get(trb->next);
  auto status_trb = static_cast<Status*>(static_cast<TRB*>(trb));
  ASSERT_EQ(status_trb->DIRECTION(), 0UL);
  ASSERT_EQ(status_trb->INTERRUPTER(), 0UL);
  ASSERT_TRUE(status_trb->IOC());
  // Interrupt on completion
  TransferRing* ring = reinterpret_cast<TransferRing*>(initial_trb->ptr);
  std::unique_ptr<TRBContext> context;
  ring->CompleteTRB(trb, &context);
  std::get<Request>(*context->request).Complete(ZX_OK, sizeof(void*));
  ASSERT_TRUE(invoked);
}

TEST_F(XhciMmioHarness, QueueNormalRequest) {
  ConnectDevice(1, USB_SPEED_FULL);
  EnableEndpoint(0, 1, true);
  bool rang = false;
  SetDoorbellListener([&](uint8_t doorbell, uint8_t target) {
    if (doorbell == 1 && target == 3) {
      rang = true;
    }
  });

  std::optional<TestRequest> request;
  bool invoked = false;
  AllocateRequest(&request, 0, zx_system_get_page_size() * 2, 1 | 0x80, [&](TestRequest request) {
    invoked = true;
    void** parameters;
    request.Mmap(reinterpret_cast<void**>(&parameters));
    EXPECT_EQ(*parameters, parameters);
  });

  RequestQueue(std::move(*request));
  ASSERT_TRUE(rang);
  // Find slot context pointer in address device command
  auto cr = crcr_next();
  Control control_trb = Control::FromTRB(cr);
  ASSERT_EQ(control_trb.Type(), Control::AddressDeviceCommand);
  [[maybe_unused]] auto control =
      static_cast<unsigned char*>(reinterpret_cast<FakeVMO*>(cr->ptr)->virt);
  auto endpoint_context = reinterpret_cast<EndpointContext*>(control + (64 * 4));
  auto ring_phys =
      static_cast<zx_paddr_t>(static_cast<uint64_t>(endpoint_context->dequeue_pointer_a) |
                              (static_cast<uint64_t>(endpoint_context->dequeue_pointer_b) << 32)) &
      (~1);

  auto trb_start = FakeTRB::get(ring_phys);

  // Data (page 0)
  auto trb = FakeTRB::get(trb_start->next)->contig.data();
  auto data_trb = static_cast<Normal*>(static_cast<TRB*>(trb));
  ASSERT_EQ(Control::FromTRB(data_trb).Type(), Control::Normal);
  ASSERT_EQ(data_trb->IOC(), 0UL);
  ASSERT_EQ(data_trb->ISP(), 1UL);
  ASSERT_EQ(data_trb->INTERRUPTER(), 0UL);
  ASSERT_EQ(data_trb->LENGTH(), zx_system_get_page_size());
  ASSERT_EQ(data_trb->SIZE(), 1UL);
  ASSERT_TRUE(data_trb->NO_SNOOP());
  void** virt = reinterpret_cast<void**>(FakeTRB::get(static_cast<zx_paddr_t>(data_trb->ptr))->ptr);
  *virt = virt;

  // Data (page 1, contiguous)
  trb++;
  data_trb = static_cast<Normal*>(static_cast<TRB*>(trb));
  ASSERT_EQ(data_trb->IOC(), 1UL);
  ASSERT_EQ(data_trb->ISP(), 1UL);
  ASSERT_EQ(data_trb->INTERRUPTER(), 0UL);
  ASSERT_EQ(data_trb->LENGTH(), zx_system_get_page_size());
  ASSERT_EQ(data_trb->SIZE(), 0UL);
  ASSERT_TRUE(data_trb->NO_SNOOP());

  // Interrupt on completion
  TransferRing* ring = reinterpret_cast<TransferRing*>(trb_start->ptr);
  std::unique_ptr<TRBContext> context;
  ring->CompleteTRB(trb, &context);
  std::get<Request>(*context->request).Complete(ZX_OK, sizeof(void*));
  ASSERT_TRUE(invoked);
}

TEST_F(XhciMmioHarness, CancelAllOnDisabledEndpoint) {
  ConnectDevice(1, USB_SPEED_HIGH);
  zx_status_t cancel_status;
  auto cr = crcr_next();
  Control control_trb = Control::FromTRB(cr);
  EXPECT_EQ(control_trb.Type(), Control::AddressDeviceCommand);
  CommandCompletionEvent event;
  event.set_CompletionCode(CommandCompletionEvent::Success);
  EXPECT_OK(CompleteCommand(cr, &event));
  bool got_stop_endpoint = false;
  SetDoorbellListener([&](uint8_t doorbell, uint8_t target) {
    if (doorbell == 0) {
      cr = FakeTRB::get(cr->next);
      Control control = Control::FromTRB(cr);
      switch (control.Type()) {
        case Control::StopEndpointCommand: {
          auto cancel_command = reinterpret_cast<StopEndpoint*>(cr);
          EXPECT_EQ(cancel_command->ENDPOINT(), 2UL);
          EXPECT_EQ(cancel_command->SLOT(), 1UL);
          got_stop_endpoint = true;
          EXPECT_OK(CompleteCommand(cr, &event));
        } break;
      }
    }
  });
  cancel_status = driver_test().driver()->UsbHciCancelAll(0, 1);
  EXPECT_TRUE(got_stop_endpoint);
  EXPECT_EQ(cancel_status, ZX_ERR_IO_NOT_PRESENT);
}

TEST_F(XhciMmioHarness, ResetEndpointTestSuccessCase) {
  ConnectDevice(1, USB_SPEED_HIGH);
  EnableEndpoint(0, 1, false);
  uint64_t paddr;
  {
    auto& state = driver_test().driver()->GetDeviceState()[0];
    ASSERT_TRUE(state);
    fbl::AutoLock l(&state->transaction_lock());
    auto& transfer_ring = state->GetEndpoint(0).transfer_ring();
    transfer_ring.set_stall(true);
    paddr = transfer_ring.PeekCommandRingControlRegister(0).value().reg_value();
  }

  zx_status_t reset_status;
  auto cr = crcr_next();
  Control control_trb = Control::FromTRB(cr);
  EXPECT_EQ(control_trb.Type(), Control::AddressDeviceCommand);
  CommandCompletionEvent event;
  event.set_CompletionCode(CommandCompletionEvent::Success);
  EXPECT_OK(CompleteCommand(cr, &event));
  cr = FakeTRB::get(cr->next);
  control_trb = Control::FromTRB(cr);
  EXPECT_EQ(control_trb.Type(), Control::ConfigureEndpointCommand);
  EXPECT_OK(CompleteCommand(cr, &event));

  bool got_reset_endpoint = false;
  bool got_set_tr_dequeue_ptr = false;
  SetDoorbellListener([&](uint8_t doorbell, uint8_t target) {
    if (doorbell == 0) {
      cr = FakeTRB::get(cr->next);
      Control control = Control::FromTRB(cr);
      switch (control.Type()) {
        case Control::ResetEndpointCommand: {
          auto reset_command = reinterpret_cast<ResetEndpoint*>(cr);
          EXPECT_EQ(reset_command->ENDPOINT(), 2UL);
          EXPECT_EQ(reset_command->SLOT(), 1UL);
          got_reset_endpoint = true;
          EXPECT_OK(CompleteCommand(cr, &event));
        } break;
        case Control::SetTrDequeuePointerCommand: {
          // ResetEndpoint should be sent prior to SetTrDequeuePointer
          EXPECT_TRUE(got_reset_endpoint);
          auto set_cmd = reinterpret_cast<SetTRDequeuePointer*>(cr);
          EXPECT_EQ(set_cmd->ENDPOINT(), 2UL);
          EXPECT_EQ(set_cmd->ptr, paddr);
          EXPECT_OK(CompleteCommand(cr, &event));
          got_set_tr_dequeue_ptr = true;
        } break;
      }
    }
  });

  reset_status = driver_test().driver()->UsbHciResetEndpoint(0, 1);

  EXPECT_TRUE(got_reset_endpoint);
  EXPECT_TRUE(got_set_tr_dequeue_ptr);
  EXPECT_OK(reset_status);
}

TEST_F(XhciMmioHarness, ResetEndpointFailsIfNotStalled) {
  ConnectDevice(1, USB_SPEED_HIGH);
  EnableEndpoint(0, 1, false);
  {
    auto& state = driver_test().driver()->GetDeviceState()[0];
    ASSERT_TRUE(state);
    fbl::AutoLock l(&state->transaction_lock());
    state->GetEndpoint(0).transfer_ring().set_stall(false);
  }
  ASSERT_EQ(driver_test().driver()->UsbHciResetEndpoint(0, 1), ZX_ERR_INVALID_ARGS);
}

// ResetEndpoint propagates TR dequeue pointer failure to the caller (doorbell completions are
// synchronous in this harness).
TEST_F(XhciMmioHarness, ResetEndpointSetDequeuePointerFails) {
  ConnectDevice(1, USB_SPEED_HIGH);
  EnableEndpoint(0, 1, false);
  {
    auto& state = driver_test().driver()->GetDeviceState()[0];
    ASSERT_TRUE(state);
    fbl::AutoLock l(&state->transaction_lock());
    state->GetEndpoint(0).transfer_ring().set_stall(true);
  }

  CommandCompletionEvent success_event;
  success_event.set_CompletionCode(CommandCompletionEvent::Success);
  CommandCompletionEvent fail_event;
  fail_event.set_CompletionCode(CommandCompletionEvent::ContextStateError);
  auto cr = crcr_next();
  EXPECT_EQ(Control::FromTRB(cr).Type(), Control::AddressDeviceCommand);
  EXPECT_OK(CompleteCommand(cr, &success_event));
  cr = FakeTRB::get(cr->next);
  EXPECT_EQ(Control::FromTRB(cr).Type(), Control::ConfigureEndpointCommand);
  EXPECT_OK(CompleteCommand(cr, &success_event));

  bool got_reset_endpoint = false;
  bool got_set_tr_dequeue_ptr = false;
  SetDoorbellListener([&](uint8_t doorbell, uint8_t target) {
    if (doorbell != 0) {
      return;
    }
    cr = FakeTRB::get(cr->next);
    switch (Control::FromTRB(cr).Type()) {
      case Control::ResetEndpointCommand:
        got_reset_endpoint = true;
        EXPECT_OK(CompleteCommand(cr, &success_event));
        break;
      case Control::SetTrDequeuePointerCommand:
        EXPECT_TRUE(got_reset_endpoint);
        got_set_tr_dequeue_ptr = true;
        EXPECT_OK(CompleteCommand(cr, &fail_event));
        break;
    }
  });

  zx_status_t reset_status = driver_test().driver()->UsbHciResetEndpoint(0, 1);

  EXPECT_TRUE(got_reset_endpoint);
  EXPECT_TRUE(got_set_tr_dequeue_ptr);
  EXPECT_EQ(reset_status, ZX_ERR_IO);
}

// CancelAll propagates StopEndpoint command failure to the caller.
TEST_F(XhciMmioHarness, CancelAllStopEndpointFails) {
  ConnectDevice(1, USB_SPEED_HIGH);
  EnableEndpoint(0, 1, false);

  CommandCompletionEvent success_event;
  success_event.set_CompletionCode(CommandCompletionEvent::Success);
  CommandCompletionEvent fail_event;
  fail_event.set_CompletionCode(CommandCompletionEvent::ContextStateError);
  auto cr = crcr_next();
  EXPECT_EQ(Control::FromTRB(cr).Type(), Control::AddressDeviceCommand);
  EXPECT_OK(CompleteCommand(cr, &success_event));
  cr = FakeTRB::get(cr->next);
  EXPECT_EQ(Control::FromTRB(cr).Type(), Control::ConfigureEndpointCommand);
  EXPECT_OK(CompleteCommand(cr, &success_event));

  bool got_stop_endpoint = false;
  SetDoorbellListener([&](uint8_t doorbell, uint8_t target) {
    if (doorbell != 0) {
      return;
    }
    cr = FakeTRB::get(cr->next);
    if (Control::FromTRB(cr).Type() == Control::StopEndpointCommand) {
      got_stop_endpoint = true;
      EXPECT_OK(CompleteCommand(cr, &fail_event));
    }
  });

  zx_status_t cancel_status = driver_test().driver()->UsbHciCancelAll(0, 1);

  EXPECT_TRUE(got_stop_endpoint);
  EXPECT_EQ(cancel_status, ZX_ERR_IO);
}

TEST_F(XhciMmioHarness, GetMaxDeviceCount) { ASSERT_EQ(GetMaxDeviceCount(), 34UL); }

}  // namespace usb_xhci
