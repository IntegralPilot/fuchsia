// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.pci/cpp/wire.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/default.h>
#include <lib/async_patterns/testing/cpp/dispatcher_bound.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/ddk/debug.h>
#include <lib/device-protocol/pci.h>
#include <lib/driver/mmio/cpp/mmio.h>
#include <lib/fake-bti/bti.h>
#include <lib/pci/constants.h>
#include <lib/sync/completion.h>
#include <lib/virtio/backends/pci.h>
#include <lib/virtio/driver_utils_dfv1.h>
#include <lib/virtio/ring.h>
#include <lib/zx/bti.h>
#include <lib/zx/vmo.h>
#include <unistd.h>
#include <zircon/errors.h>
#include <zircon/status.h>

#include <ddktl/device.h>
#include <virtio/virtio.h>
#include <zxtest/zxtest.h>

#include "src/devices/pci/testing/pci_protocol_fake.h"
#include "src/devices/testing/mock-ddk/mock-device.h"

// This capability configuration comes straight from a Virtio
// device running inside FEMU.
constexpr uint8_t kCapabilityOffsets[5] = {0x84, 0x70, 0x60, 0x50, 0x40};
// Chosen just to avoid conflict with the above.
constexpr uint8_t kCapabilityOffsets64[1] = {0xC0};
// clang-format off
constexpr virtio_pci_cap_t kCapabilities[5] = {
  {
      .cap_vndr = 0x9,
      .cap_next = 0x70,
      .cap_len = 0x14,
      .cfg_type = VIRTIO_PCI_CAP_PCI_CFG,
      .bar = 0,
      .offset = 0,
      .length = 0,
  },
  {
      .cap_vndr = 0x9,
      .cap_next = 0x60,
      .cap_len = 0x14,
      .cfg_type = VIRTIO_PCI_CAP_NOTIFY_CFG,
      .bar = 4,
      .offset = 0x3000,
      .length = 0x1000,
  },
  {
      .cap_vndr = 0x9,
      .cap_next = 0x50,
      .cap_len = 0x10,
      .cfg_type = VIRTIO_PCI_CAP_DEVICE_CFG,
      .bar = 4,
      .offset = 0x2000,
      .length = 0x1000,
  },
  {
      .cap_vndr = 0x9,
      .cap_next = 0x40,
      .cap_len = 0x10,
      .cfg_type = VIRTIO_PCI_CAP_ISR_CFG,
      .bar = 4,
      .offset = 0x1000,
      .length = 0x1000,

  },
  {
      .cap_vndr = 0x9,
      .cap_next = 0x0,
      .cap_len = 0x10,
      .cfg_type = VIRTIO_PCI_CAP_COMMON_CFG,
      .bar = 4,
      .offset = 0x0000,
      .length = 0x1000,
  },
};

constexpr uint64_t kSharedMemoryOffset = 0x3000 + 0x1000;  // 0x3000 is the offset of the last capability in the bar,
                                        // and 0x1000 is the length (before the shared memory region).
constexpr uint64_t kSharedMemorySize = 0x20000;
constexpr uint64_t kExpectedBarSize = kSharedMemoryOffset + kSharedMemorySize;

constexpr virtio_pci_cap64_t kCapabilities64[1] = {
  {
    .cap = {
      .cap_vndr = 0x9,
      .cap_next = 0x0,
      .cap_len = 0x18,
      .cfg_type = VIRTIO_PCI_CAP_SHARED_MEMORY_CFG,
      .bar = 4,
      .offset = (kSharedMemoryOffset & 0xFFFF),
      .length = (kSharedMemorySize & 0xFFFF),
    },
    .offset_hi = (kSharedMemoryOffset >> 32),
    .length_hi = (kSharedMemorySize >> 32),
  }
};

// clang-format on

class TestLegacyIoInterface : public virtio::PciLegacyIoInterface {
 public:
  explicit TestLegacyIoInterface(fdf::MmioView view) : view_(view) {}
  ~TestLegacyIoInterface() override = default;

  void Read(uint16_t offset, uint8_t* val) const final {
    *val = view_.Read8(offset);
    zxlogf(TRACE, "%s: %#x -> %#x", __PRETTY_FUNCTION__, offset, *val);
  }
  void Read(uint16_t offset, uint16_t* val) const final {
    *val = view_.Read16(offset);
    zxlogf(TRACE, "%s: %#x -> %#x", __PRETTY_FUNCTION__, offset, *val);
  }
  void Read(uint16_t offset, uint32_t* val) const final {
    *val = view_.Read32(offset);
    zxlogf(TRACE, "%s: %#x -> %#x", __PRETTY_FUNCTION__, offset, *val);
  }
  void Write(uint16_t offset, uint8_t val) const final {
    view_.Write8(val, offset);
    zxlogf(TRACE, "%s: %#x <- %#x", __PRETTY_FUNCTION__, offset, val);
  }
  void Write(uint16_t offset, uint16_t val) const final {
    view_.Write16(val, offset);
    zxlogf(TRACE, "%s: %#x <- %#x", __PRETTY_FUNCTION__, offset, val);
  }
  void Write(uint16_t offset, uint32_t val) const final {
    view_.Write32(val, offset);
    zxlogf(TRACE, "%s: %#x <- %#x", __PRETTY_FUNCTION__, offset, val);
  }

 private:
  fdf::MmioView view_;
};

class AsyncState {
 public:
  AsyncState() : outgoing_(async_get_default_dispatcher()) {}

  fidl::ClientEnd<fuchsia_io::Directory> SetUpProtocol() {
    auto endpoints = fidl::Endpoints<fuchsia_io::Directory>::Create();

    zx::result service_result = outgoing_.AddService<fuchsia_hardware_pci::Service>(
        fuchsia_hardware_pci::Service::InstanceHandler(
            {.device = bindings_.CreateHandler(&fake_pci, async_get_default_dispatcher(),
                                               fidl::kIgnoreBindingClosure)}));
    ZX_ASSERT(service_result.is_ok());

    ZX_ASSERT(outgoing_.Serve(std::move(endpoints.server)).is_ok());
    return std::move(endpoints.client);
  }

  pci::FakePciProtocol fake_pci;

 private:
  // |bindings_| borrows |async_|.
  fidl::ServerBindingGroup<fuchsia_hardware_pci::Device> bindings_;
  // |outgooing_| borrows |bindings_|.
  component::OutgoingDirectory outgoing_;
};

fx_log_severity_t kTestLogLevel = DDK_LOG_INFO;
class VirtioTests : public zxtest::Test {
 public:
  static constexpr uint16_t kQueueSize = 1u;
  static constexpr uint32_t kLegacyBar = 0u;
  static constexpr uint32_t kModernBar = 4u;

  VirtioTests() : async_(loop_.dispatcher(), std::in_place) {}

 protected:
  void SetUp() final {
    mock_ddk::SetMinLogSeverity(kTestLogLevel);
    fake_parent_ = MockDevice::FakeRootParent();
    ASSERT_OK(loop_.StartThread("async-state-thread"));
  }

  void TearDown() final {
    device_async_remove(fake_parent_.get());
    async_.SyncCall([](AsyncState* async) { async->fake_pci.Reset(); });
  }

  async_patterns::TestDispatcherBound<AsyncState>& async_state() { return async_; }
  std::array<std::optional<fdf::MmioBuffer>, pci::kMaxBarCount>& bars() { return bars_; }

  void SetUpProtocol() {
    fidl::ClientEnd client = async_.SyncCall(&AsyncState::SetUpProtocol);

    fake_parent_->AddFidlService(fuchsia_hardware_pci::Service::Name, std::move(client));
  }

  void SetUpModernBars() {
    size_t bar_size = kExpectedBarSize;
    async_.SyncCall([&](AsyncState* async) {
      async->fake_pci.CreateBar(kModernBar, bar_size, /*is_mmio=*/true);
    });

    zx::result pci =
        ddk::Device<void>::DdkConnectFidlProtocol<fuchsia_hardware_pci::Service::Device>(
            fake_parent_.get());
    ZX_ASSERT(pci.is_ok());
    fidl::Result result = fidl::Call(*pci)->GetBar(kModernBar);
    ZX_ASSERT(result.is_ok());

    fuchsia_hardware_pci::Bar& bar_result = result->result();
    ZX_ASSERT(bar_result.result().Which() == fuchsia_hardware_pci::BarResult::Tag::kVmo);

    zx::result mmio =
        fdf::MmioBuffer::Create(0, bar_result.size(), zx::vmo(bar_result.result().vmo()->release()),
                                ZX_CACHE_POLICY_UNCACHED_DEVICE);
    ZX_ASSERT(mmio.is_ok());
    bars_[kModernBar] = std::move(*mmio);
  }

  void SetUpModernCapabilities() {
    zx::unowned_vmo config = async_.SyncCall([](AsyncState* async) {
      for (uint32_t i = 0; i < std::size(kCapabilities); i++) {
        async->fake_pci.AddVendorCapability(kCapabilityOffsets[i], kCapabilities[i].cap_len);
      }
      for (uint32_t i = 0; i < std::size(kCapabilities64); i++) {
        async->fake_pci.AddVendorCapability(kCapabilityOffsets64[i],
                                            kCapabilities64[i].cap.cap_len);
      }
      return async->fake_pci.GetConfigVmo();
    });

    for (uint32_t i = 0; i < std::size(kCapabilities); i++) {
      config->write(&kCapabilities[i], kCapabilityOffsets[i], sizeof(virtio_pci_cap_t));
    }
    for (uint32_t i = 0; i < std::size(kCapabilities64); i++) {
      config->write(&kCapabilities64[i], kCapabilityOffsets64[i], sizeof(virtio_pci_cap64_t));
    }
  }

  void SetUpModernQueue(uint16_t size = kQueueSize) {
    auto& common_cfg_cap = kCapabilities[4];
    uint32_t queue_offset = offsetof(virtio_pci_common_cfg_t, queue_size);
    bars_[kModernBar]->Write(size, common_cfg_cap.offset + queue_offset);
  }

  void SetUpModernMsiX() {
    async_.SyncCall([](AsyncState* async) {
      async->fake_pci.AddMsixInterrupt();
      async->fake_pci.AddMsixInterrupt();
    });

    // Virtio stores a configuration register for MSI-X in a field in the common
    // configuration capability. We use the structures above to figure out what
    // bar that is in, and what offset it's at.
    auto& common_cfg_cap = kCapabilities[4];
    uint16_t msix_config_val = 0xFFFF;  // Set to no MSI-X vector allocated.
    uint32_t msix_offset = offsetof(virtio_pci_common_cfg_t, config_msix_vector);
    bars_[kModernBar]->Write(msix_config_val, common_cfg_cap.offset + msix_offset);
  }

  void SetUpLegacyBar() {
    zx::vmo vmo{};
    size_t bar_size = 0x64;  // Matches the bar size on GCE for Bar0.
    async_.SyncCall([&](AsyncState* async) {
      async->fake_pci.CreateBar(kLegacyBar, bar_size, /*is_mmio=*/false);
    });

    // Legacy BARs identified as IO in PCI cannot be mapped by pci::MapMmio, so we need to do it
    // by hand.
    ZX_ASSERT(async_.SyncCall([&](AsyncState* async) {
      return async->fake_pci.GetBar(kLegacyBar).duplicate(ZX_RIGHT_SAME_RIGHTS, &vmo);
    }) == ZX_OK);

    size_t size = 0;
    vmo.get_size(&size);
    zx::result<fdf::MmioBuffer> result =
        fdf::MmioBuffer::Create(0, size, std::move(vmo), ZX_CACHE_POLICY_UNCACHED_DEVICE);
    ZX_ASSERT_MSG(result.is_ok(), "Mapping BAR %u failed: %s", kLegacyBar, result.status_string());
    bars_[kLegacyBar] = std::move(result.value());
  }

  // Even in the fake device we have to deal with registers being in different
  // places depending on whether MSI has been enabled or not.
  uint16_t LegacyDeviceCfgOffset() {
    fuchsia_hardware_pci::InterruptMode interrupt_mode;
    interrupt_mode =
        async_.SyncCall([](AsyncState* async) { return async->fake_pci.GetIrqMode(); });
    return (interrupt_mode == fuchsia_hardware_pci::InterruptMode::kMsiX)
               ? VIRTIO_PCI_CONFIG_OFFSET_MSIX
               : VIRTIO_PCI_CONFIG_OFFSET_NOMSIX;
  }

  void SetUpLegacyQueue(uint16_t size = kQueueSize) {
    bars_[kLegacyBar]->Write(size, VIRTIO_PCI_QUEUE_SIZE);
  }

  void RunAssertZeroInitializedRing(std::unique_ptr<virtio::Backend> backend);
  void RunGoldenAvailAllocationPattern(std::unique_ptr<virtio::Backend> backend);
  void RunAvailUsedInterleavedOperations(std::unique_ptr<virtio::Backend> backend);

  std::shared_ptr<MockDevice> fake_parent_;

 private:
  async::Loop loop_{&kAsyncLoopConfigNoAttachToCurrentThread};
  async_patterns::TestDispatcherBound<AsyncState> async_;
  std::array<std::optional<fdf::MmioBuffer>, pci::kMaxBarCount> bars_;
};

class TestVirtioDevice;
using DeviceType = ddk::Device<TestVirtioDevice>;
class TestVirtioDevice : public virtio::Device, public DeviceType {
 public:
  static const uint16_t kVirtqueueSize = 1u;

  explicit TestVirtioDevice(zx_device_t* bus_device, zx::bti bti,
                            std::unique_ptr<virtio::Backend> backend)
      : virtio::Device(std::move(bti), std::move(backend)), DeviceType(bus_device) {}

  zx_status_t Init() final {
    // Initialize the first virtqueue.
    zx_status_t status = ring_.Init(0);
    if (status != ZX_OK) {
      return status;
    }
    return DdkAdd(tag());
  }
  void IrqRingUpdate() final {}
  void IrqConfigChange() final {}
  const char* tag() const final { return "test"; }

  void DdkRelease() { delete this; }

  virtio::Ring& GetRing() { return ring_; }

 private:
  virtio::Ring ring_{this};
};

void VirtioTests::RunAssertZeroInitializedRing(std::unique_ptr<virtio::Backend> backend) {
  zx::bti bti{};
  ASSERT_OK(fake_bti_create(bti.reset_and_get_address()));
  ASSERT_OK(backend->Bind());
  auto device =
      std::make_unique<TestVirtioDevice>(fake_parent_.get(), std::move(bti), std::move(backend));
  ASSERT_OK(device->Init());

  uint8_t* ptr = reinterpret_cast<uint8_t*>(device->GetRing().DescFromIndex(0));
  size_t size = vring_size(2, zx_system_get_page_size());

  // Verify initial free list terminator (desc[0].next = 0xffff)
  EXPECT_EQ(ptr[14], 0xff);
  EXPECT_EQ(ptr[15], 0xff);

  // Verify all other bytes are zero (including desc[1].next which should be 0)
  for (size_t i = 0; i < size; ++i) {
    if (i == 14 || i == 15)
      continue;
    EXPECT_EQ(ptr[i], 0);
  }

  [[maybe_unused]] auto ptr_release = device.release();
}

void VirtioTests::RunGoldenAvailAllocationPattern(std::unique_ptr<virtio::Backend> backend) {
  zx::bti bti{};
  ASSERT_OK(fake_bti_create(bti.reset_and_get_address()));
  ASSERT_OK(backend->Bind());
  auto device =
      std::make_unique<TestVirtioDevice>(fake_parent_.get(), std::move(bti), std::move(backend));
  ASSERT_OK(device->Init());

  auto& ring = device->GetRing();
  uint8_t* base_ptr = reinterpret_cast<uint8_t*>(ring.DescFromIndex(0));

  struct vring_desc* desc = ring.DescFromIndex(0);
  EXPECT_EQ(desc[0].next, 0xffff);
  EXPECT_EQ(desc[1].next, 0);
  EXPECT_EQ(desc[2].next, 1);
  EXPECT_EQ(desc[3].next, 2);

  struct vring_avail* avail = reinterpret_cast<struct vring_avail*>(base_ptr + 64);
  avail->idx = 0xa5a5;

  uint16_t* avail_ring = reinterpret_cast<uint16_t*>(base_ptr + 68);
  avail_ring[0] = 0xa5a5;
  avail_ring[1] = 0xa5a5;

  uint16_t head_idx = 0;
  struct vring_desc* last_desc = ring.AllocDescChain(2, &head_idx);
  ASSERT_NOT_NULL(last_desc);
  EXPECT_EQ(head_idx, 3);

  EXPECT_EQ(desc[3].next, 2);
  EXPECT_TRUE(desc[3].flags & VRING_DESC_F_NEXT);
  EXPECT_EQ(desc[2].next, 0);
  EXPECT_FALSE(desc[2].flags & VRING_DESC_F_NEXT);

  ring.SubmitChain(head_idx);
  EXPECT_EQ(avail->idx, 0xa5a6);
  EXPECT_EQ(avail_ring[1], 3);
  EXPECT_EQ(avail_ring[0], 0xa5a5);

  [[maybe_unused]] auto ptr_release = device.release();
}

void VirtioTests::RunAvailUsedInterleavedOperations(std::unique_ptr<virtio::Backend> backend) {
  zx::bti bti{};
  ASSERT_OK(fake_bti_create(bti.reset_and_get_address()));
  ASSERT_OK(backend->Bind());
  auto device =
      std::make_unique<TestVirtioDevice>(fake_parent_.get(), std::move(bti), std::move(backend));
  ASSERT_OK(device->Init());

  auto& ring = device->GetRing();
  vring& vr = ring.vring_unsafe();

  auto free_chain = [&](struct vring_used_elem* used_elem) {
    uint16_t desc_index = static_cast<uint16_t>(used_elem->id);
    while (true) {
      struct vring_desc* desc = ring.DescFromIndex(desc_index);
      uint16_t next_index = desc->next;
      bool has_next = desc->flags & VRING_DESC_F_NEXT;
      ring.FreeDesc(desc_index);
      if (!has_next) {
        break;
      }
      desc_index = next_index;
    }
  };

  EXPECT_EQ(vr.free_count, 128);
  EXPECT_EQ(vr.free_list, 127);
  EXPECT_EQ(vr.avail->idx, 0);
  EXPECT_EQ(vr.used->idx, 0);

  uint16_t head1 = 0;
  struct vring_desc* last_desc1 = ring.AllocDescChain(2, &head1);
  ASSERT_NOT_NULL(last_desc1);
  EXPECT_EQ(head1, 127);
  EXPECT_EQ(vr.free_count, 126);
  EXPECT_EQ(vr.free_list, 125);

  ring.SubmitChain(head1);
  EXPECT_EQ(vr.avail->idx, 1);
  EXPECT_EQ(vr.avail->ring[0], 127);

  uint16_t head2 = 0;
  struct vring_desc* last_desc2 = ring.AllocDescChain(3, &head2);
  ASSERT_NOT_NULL(last_desc2);
  EXPECT_EQ(head2, 125);
  EXPECT_EQ(vr.free_count, 123);
  EXPECT_EQ(vr.free_list, 122);

  ring.SubmitChain(head2);
  EXPECT_EQ(vr.avail->idx, 2);
  EXPECT_EQ(vr.avail->ring[1], 125);

  vr.used->ring[0].id = 127;
  vr.used->ring[0].len = 100;
  vr.used->idx = 1;

  ring.IrqRingUpdate(free_chain);
  EXPECT_EQ(vr.free_count, 125);
  EXPECT_EQ(vr.free_list, 126);

  uint16_t head3 = 0;
  struct vring_desc* last_desc3 = ring.AllocDescChain(1, &head3);
  ASSERT_NOT_NULL(last_desc3);
  EXPECT_EQ(head3, 126);
  EXPECT_EQ(vr.free_count, 124);
  EXPECT_EQ(vr.free_list, 127);

  ring.SubmitChain(head3);
  EXPECT_EQ(vr.avail->idx, 3);
  EXPECT_EQ(vr.avail->ring[2], 126);

  vr.used->ring[1].id = 125;
  vr.used->ring[1].len = 200;
  vr.used->idx = 2;

  ring.IrqRingUpdate(free_chain);
  EXPECT_EQ(vr.free_count, 127);
  EXPECT_EQ(vr.free_list, 123);

  vr.used->ring[2].id = 126;
  vr.used->ring[2].len = 300;
  vr.used->idx = 3;

  ring.IrqRingUpdate(free_chain);
  EXPECT_EQ(vr.free_count, 128);
  EXPECT_EQ(vr.free_list, 126);

  [[maybe_unused]] auto ptr_release = device.release();
}

class TestVirtioSharedMemoryDevice : public virtio::Device,
                                     public ddk::Device<TestVirtioSharedMemoryDevice> {
 public:
  explicit TestVirtioSharedMemoryDevice(zx_device_t* bus_device, zx::bti bti,
                                        std::unique_ptr<virtio::Backend> backend)
      : virtio::Device(std::move(bti), std::move(backend)),
        ddk::Device<TestVirtioSharedMemoryDevice>(bus_device) {}

  zx_status_t Init() final {
    zx::vmo vmo;
    if (zx_status_t status = backend_->GetSharedMemoryVmo(&vmo); status != ZX_OK) {
      return status;
    }
    uint64_t size;
    if (zx_status_t status = vmo.get_size(&size); status != ZX_OK) {
      return status;
    }
    if (size != kExpectedBarSize) {
      return ZX_ERR_BAD_STATE;
    }
    return DdkAdd(tag());
  }
  void IrqRingUpdate() final {}
  void IrqConfigChange() final {}
  const char* tag() const final { return "test"; }

  void DdkRelease() { delete this; }
};

TEST_F(VirtioTests, FailureNoProtocol) {
  ASSERT_EQ(ZX_ERR_NOT_FOUND, virtio::CreateAndBind<TestVirtioDevice>(nullptr, fake_parent_.get()));
}

TEST_F(VirtioTests, FailureNoCapabilities) {
  SetUpProtocol();
  ASSERT_EQ(ZX_ERR_PEER_CLOSED,
            virtio::CreateAndBind<TestVirtioDevice>(nullptr, fake_parent_.get()));
}

TEST_F(VirtioTests, FailureNoBar) {
  SetUpProtocol();
  SetUpModernCapabilities();
  ASSERT_EQ(ZX_ERR_BAD_STATE, virtio::CreateAndBind<TestVirtioDevice>(nullptr, fake_parent_.get()));
}

TEST_F(VirtioTests, LegacyInterruptBindSuccess) {
  SetUpProtocol();
  SetUpModernCapabilities();
  SetUpModernBars();
  SetUpModernQueue();
  async_state().SyncCall([&](AsyncState* async) { async->fake_pci.AddLegacyInterrupt(); });

  ASSERT_OK(virtio::CreateAndBind<TestVirtioDevice>(nullptr, fake_parent_.get()));
}

TEST_F(VirtioTests, FailureOneMsixBind) {
  SetUpProtocol();
  SetUpModernCapabilities();
  SetUpModernBars();
  async_state().SyncCall([&](AsyncState* async) { async->fake_pci.AddMsixInterrupt(); });

  ASSERT_EQ(ZX_ERR_BAD_STATE, virtio::CreateAndBind<TestVirtioDevice>(nullptr, fake_parent_.get()));
}

TEST_F(VirtioTests, TwoMsixBindSuccess) {
  SetUpProtocol();
  SetUpModernCapabilities();
  SetUpModernBars();
  SetUpModernQueue();
  SetUpModernMsiX();

  // With everything set up this should succeed.
  ASSERT_OK(virtio::CreateAndBind<TestVirtioDevice>(nullptr, fake_parent_.get()));
}

TEST_F(VirtioTests, SharedMemoryBar) {
  SetUpProtocol();
  SetUpModernCapabilities();
  SetUpModernBars();
  SetUpModernMsiX();

  ASSERT_OK(virtio::CreateAndBind<TestVirtioSharedMemoryDevice>(nullptr, fake_parent_.get()));
}

// Ensure that the Legacy interface looks for IO Bar 0 and succeeds up until it
// tries to make IO writes using in/out instructions.
TEST_F(VirtioTests, DISABLED_LegacyIoBackendError) {
  async_state().SyncCall([&](AsyncState* async) { async->fake_pci.AddLegacyInterrupt(); });
  SetUpProtocol();
  SetUpLegacyBar();
  SetUpLegacyQueue();
  auto backend_result = virtio::GetBtiAndBackend(fake_parent_.get());
  ASSERT_OK(backend_result.status_value());
  // This should fail on x64 because of failure to access IO ports.
#ifdef __x86_64__
  TestVirtioDevice device(fake_parent_.get(), std::move(backend_result->first),
                          std::move(backend_result->second));
  ASSERT_DEATH([&device] { device.Init(); });
#endif
}

TEST_F(VirtioTests, LegacyIoBackendSuccess) {
  SetUpProtocol();
  SetUpLegacyBar();
  SetUpLegacyQueue();
  async_state().SyncCall([&](AsyncState* async) { async->fake_pci.AddLegacyInterrupt(); });

  // With a manually crafted backend using the test interface it should succeed.
  zx::result pci = ddk::Device<void>::DdkConnectFidlProtocol<fuchsia_hardware_pci::Service::Device>(
      fake_parent_.get());
  ASSERT_TRUE(pci.is_ok());
  auto info = fidl::Call(*pci)->GetDeviceInfo();
  ASSERT_TRUE(info.is_ok());
  zx::bti bti{};
  ASSERT_OK(fake_bti_create(bti.reset_and_get_address()));

  // Feed the same vmo backing AsyncState's BAR 0 into the interface so the view
  // from PCI, Virtio, and the test all align.
  TestLegacyIoInterface interface(bars()[kLegacyBar]->View(0));

  auto backend =
      std::make_unique<virtio::PciLegacyBackend>(std::move(*pci), info->info(), &interface);
  ASSERT_OK(backend->Bind());

  auto device =
      std::make_unique<TestVirtioDevice>(fake_parent_.get(), std::move(bti), std::move(backend));
  ASSERT_OK(device->Init());
  // Owned by the framework now.
  [[maybe_unused]] auto ptr = device.release();
}

TEST_F(VirtioTests, AssertZeroInitializedRingLegacy) {
  SetUpProtocol();
  SetUpLegacyBar();
  SetUpLegacyQueue(2);
  async_state().SyncCall([&](AsyncState* async) { async->fake_pci.AddLegacyInterrupt(); });

  zx::result pci = ddk::Device<void>::DdkConnectFidlProtocol<fuchsia_hardware_pci::Service::Device>(
      fake_parent_.get());
  ASSERT_TRUE(pci.is_ok());
  auto info = fidl::Call(*pci)->GetDeviceInfo();
  ASSERT_TRUE(info.is_ok());

  TestLegacyIoInterface interface(bars()[kLegacyBar]->View(0));
  auto backend =
      std::make_unique<virtio::PciLegacyBackend>(std::move(*pci), info->info(), &interface);

  RunAssertZeroInitializedRing(std::move(backend));
}

TEST_F(VirtioTests, AssertZeroInitializedRingModern) {
  SetUpProtocol();
  SetUpModernCapabilities();
  SetUpModernBars();
  SetUpModernQueue(2);
  SetUpModernMsiX();

  zx::result pci = ddk::Device<void>::DdkConnectFidlProtocol<fuchsia_hardware_pci::Service::Device>(
      fake_parent_.get());
  ASSERT_TRUE(pci.is_ok());
  auto info = fidl::Call(*pci)->GetDeviceInfo();
  ASSERT_TRUE(info.is_ok());

  auto backend = std::make_unique<virtio::PciModernBackend>(std::move(*pci), info->info());

  RunAssertZeroInitializedRing(std::move(backend));
}

TEST_F(VirtioTests, GoldenAvailAllocationPatternLegacy) {
  SetUpProtocol();
  SetUpLegacyBar();
  SetUpLegacyQueue(4);
  async_state().SyncCall([&](AsyncState* async) { async->fake_pci.AddLegacyInterrupt(); });

  zx::result pci = ddk::Device<void>::DdkConnectFidlProtocol<fuchsia_hardware_pci::Service::Device>(
      fake_parent_.get());
  ASSERT_TRUE(pci.is_ok());
  auto info = fidl::Call(*pci)->GetDeviceInfo();
  ASSERT_TRUE(info.is_ok());

  TestLegacyIoInterface interface(bars()[kLegacyBar]->View(0));
  auto backend =
      std::make_unique<virtio::PciLegacyBackend>(std::move(*pci), info->info(), &interface);

  RunGoldenAvailAllocationPattern(std::move(backend));
}

TEST_F(VirtioTests, GoldenAvailAllocationPatternModern) {
  SetUpProtocol();
  SetUpModernCapabilities();
  SetUpModernBars();
  SetUpModernQueue(4);
  SetUpModernMsiX();

  zx::result pci = ddk::Device<void>::DdkConnectFidlProtocol<fuchsia_hardware_pci::Service::Device>(
      fake_parent_.get());
  ASSERT_TRUE(pci.is_ok());
  auto info = fidl::Call(*pci)->GetDeviceInfo();
  ASSERT_TRUE(info.is_ok());

  auto backend = std::make_unique<virtio::PciModernBackend>(std::move(*pci), info->info());

  RunGoldenAvailAllocationPattern(std::move(backend));
}

TEST_F(VirtioTests, AvailUsedInterleavedOperationsLegacy) {
  SetUpProtocol();
  SetUpLegacyBar();
  SetUpLegacyQueue(128);
  async_state().SyncCall([&](AsyncState* async) { async->fake_pci.AddLegacyInterrupt(); });

  zx::result pci = ddk::Device<void>::DdkConnectFidlProtocol<fuchsia_hardware_pci::Service::Device>(
      fake_parent_.get());
  ASSERT_TRUE(pci.is_ok());
  auto info = fidl::Call(*pci)->GetDeviceInfo();
  ASSERT_TRUE(info.is_ok());

  TestLegacyIoInterface interface(bars()[kLegacyBar]->View(0));
  auto backend =
      std::make_unique<virtio::PciLegacyBackend>(std::move(*pci), info->info(), &interface);

  RunAvailUsedInterleavedOperations(std::move(backend));
}

TEST_F(VirtioTests, AvailUsedInterleavedOperationsModern) {
  SetUpProtocol();
  SetUpModernCapabilities();
  SetUpModernBars();
  SetUpModernQueue(128);
  SetUpModernMsiX();

  zx::result pci = ddk::Device<void>::DdkConnectFidlProtocol<fuchsia_hardware_pci::Service::Device>(
      fake_parent_.get());
  ASSERT_TRUE(pci.is_ok());
  auto info = fidl::Call(*pci)->GetDeviceInfo();
  ASSERT_TRUE(info.is_ok());

  auto backend = std::make_unique<virtio::PciModernBackend>(std::move(*pci), info->info());

  RunAvailUsedInterleavedOperations(std::move(backend));
}

TEST_F(VirtioTests, LegacyMsiX) {
  SetUpProtocol();
  SetUpLegacyBar();
  SetUpLegacyQueue();
  async_state().SyncCall([](AsyncState* async) {
    async->fake_pci.AddMsixInterrupt();
    async->fake_pci.AddMsixInterrupt();
  });

  zx::result pci = ddk::Device<void>::DdkConnectFidlProtocol<fuchsia_hardware_pci::Service::Device>(
      fake_parent_.get());
  ASSERT_TRUE(pci.is_ok());
  auto info = fidl::Call(*pci)->GetDeviceInfo();
  ASSERT_TRUE(info.is_ok());
  zx::bti bti{};
  ASSERT_OK(fake_bti_create(bti.reset_and_get_address()));

  TestLegacyIoInterface interface(bars()[kLegacyBar]->View(0));
  auto backend =
      std::make_unique<virtio::PciLegacyBackend>(std::move(*pci), info->info(), &interface);
  ASSERT_OK(backend->Bind());

  auto device =
      std::make_unique<TestVirtioDevice>(fake_parent_.get(), std::move(bti), std::move(backend));
  ASSERT_OK(device->Init());
  // Owned by the framework now.
  [[maybe_unused]] auto ptr = device.release();

  // Verify MSI-X state
  ASSERT_EQ(async_state().SyncCall([](AsyncState* async) { return async->fake_pci.GetIrqMode(); }),
            fuchsia_hardware_pci::InterruptMode::kMsiX);
  uint16_t value{};
  value = bars()[kLegacyBar]->Read16(VIRTIO_PCI_MSI_CONFIG_VECTOR);
  ASSERT_EQ(value, virtio::PciBackend::kMsiConfigVector);
  value = bars()[kLegacyBar]->Read16(VIRTIO_PCI_MSI_QUEUE_VECTOR);
  ASSERT_EQ(value, virtio::PciBackend::kMsiQueueVector);
}

int main(int argc, char* argv[]) {
  // TODO(https://fxbug.dev/42166778): Remove custom main once mock-ddk works with
  // --min-severity-logs.
  int opt;
  int v_position = 0;
  while (!v_position && (opt = getopt(argc, argv, "hv")) != -1) {
    switch (opt) {
      case 'v':
        if (kTestLogLevel > DDK_LOG_TRACE) {
          kTestLogLevel -= (DDK_LOG_DEBUG - DDK_LOG_TRACE);
        }

        v_position = optind - 1;
        break;
      case 'h':
        fprintf(stderr,
                "    Test-Specific Usage: %s [OPTIONS]\n\n"
                "    [OPTIONS]\n"
                "    -v                                                  Enable DEBUG logs\n"
                "    -vv                                                 Enable TRACE logs\n\n",
                argv[0]);
        break;
    }
  }

  // Do the minimal amount of work to forward the rest of the args to zxtest
  // with our consumed '-v' / '-vv' removed. Don't worry about additional -v
  // usage because the zxtest help will point out the invalid nature of it.
  if (v_position) {
    for (int p = v_position; p < argc - 1; p++) {
      argv[p] = argv[p + 1];
    }
    argc--;
  }
  return RUN_ALL_TESTS(argc, argv);
}
