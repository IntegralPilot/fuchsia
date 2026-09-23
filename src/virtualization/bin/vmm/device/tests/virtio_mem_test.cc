// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fuchsia/logger/cpp/fidl.h>
#include <fuchsia/tracing/provider/cpp/fidl.h>
#include <fuchsia/virtualization/hardware/cpp/fidl.h>
#include <lib/sys/component/cpp/testing/realm_builder.h>
#include <lib/zircon-internal/align.h>
#include <threads.h>

#include <virtio/mem.h>

#include "src/virtualization/bin/vmm/device/tests/test_with_device.h"
#include "src/virtualization/bin/vmm/device/tests/virtio_queue_fake.h"

static constexpr uint16_t kNumQueues = 1;
static constexpr uint16_t kQueueSize = 16;

const size_t kPageSize = zx_system_get_page_size();

static constexpr uint64_t kPluggedBlockSize = 2 * 1024 * 1024;
static constexpr uint64_t kRegionSize = static_cast<uint64_t>(1) * 1024 * 1024 * 1024;

class VirtioMemTest : public TestWithDevice {
 public:
  constexpr static auto kComponentName = "virtio_mem";

 protected:
  VirtioMemTest() : guest_request_queue_(phys_mem_, kPageSize * kNumQueues, kQueueSize) {}

  void SetUp() override {
    using component_testing::ChildRef;
    using component_testing::ParentRef;
    using component_testing::Protocol;
    using component_testing::RealmBuilder;
    using component_testing::RealmRoot;
    using component_testing::Route;

    constexpr auto kVirtioMemUrl = "#meta/virtio_mem.cm";
    auto realm_builder = RealmBuilder::Create();
    realm_builder.AddChild(kComponentName, kVirtioMemUrl);

    realm_builder
        .AddRoute(Route{.capabilities =
                            {
                                Protocol{fuchsia::logger::LogSink::Name_},
                                Protocol{fuchsia::tracing::provider::Registry::Name_},
                            },
                        .source = ParentRef(),
                        .targets = {ChildRef{kComponentName}}})
        .AddRoute(Route{.capabilities =
                            {
                                Protocol{fuchsia::virtualization::hardware::VirtioMem::Name_},
                            },
                        .source = ChildRef{kComponentName},
                        .targets = {ParentRef()}});

    realm_ = std::make_unique<RealmRoot>(realm_builder.Build(dispatcher()));
    mem_ = realm_->component().ConnectSync<fuchsia::virtualization::hardware::VirtioMem>();

    fuchsia::virtualization::hardware::StartInfo start_info;
    size_t vmo_size = guest_request_queue_.end();
    region_addr_ = ZX_ALIGN(vmo_size, 128 * 1024 * 1024);
    vmo_size = region_addr_ + kRegionSize;
    zx_status_t status = MakeStartInfo(vmo_size, &start_info);
    ASSERT_EQ(ZX_OK, status);

    status = mem_->Start(std::move(start_info), region_addr_, kPluggedBlockSize, kRegionSize);
    ASSERT_EQ(ZX_OK, status);

    // Configure device queues.
    guest_request_queue_.Configure(0, kPageSize);
    status = mem_->ConfigureQueue(0, guest_request_queue_.size(), guest_request_queue_.desc(),
                                  guest_request_queue_.avail(), guest_request_queue_.used());
    ASSERT_EQ(ZX_OK, status);

    status = mem_->Ready(0);
    ASSERT_EQ(ZX_OK, status);
  }

  template <typename T>
  T InspectValue(std::string value_name) {
    return GetInspect("realm_builder\\:" + realm_->component().GetChildName() + "/" +
                          kComponentName + ":root",
                      "realm_builder:" + realm_->component().GetChildName() + "/" + kComponentName)
        .GetByPath({"root", std::move(value_name)})
        .Get<T>();
  }

  void WaitForPluggedSizeEqual(uint64_t val) {
    while (val != InspectValue<uint64_t>("plugged_size_bytes")) {
      zx::nanosleep(zx::deadline_after(zx::msec(100)));
    }
  }

  void Plug(uint64_t addr, uint16_t num_blocks,
            uint16_t expected_response_type = VIRTIO_MEM_RESP_ACK) {
    uint64_t prev_plugged_size_bytes = InspectValue<uint64_t>("plugged_size_bytes");
    virtio_mem_req_t req = {.type = VIRTIO_MEM_REQ_PLUG, .addr = addr, .nb_blocks = num_blocks};
    virtio_mem_resp_t* resp;

    zx_status_t status = DescriptorChainBuilder(guest_request_queue_)
                             .AppendReadableDescriptor(&req, sizeof(req))
                             .AppendWritableDescriptor(&resp, sizeof(*resp))
                             .Build();
    EXPECT_EQ(ZX_OK, status);
    status = mem_->NotifyQueue(0);
    EXPECT_EQ(ZX_OK, status);
    status = WaitOnInterrupt();
    EXPECT_EQ(ZX_OK, status);
    EXPECT_EQ(expected_response_type, resp->type);
    uint64_t expected_plugged_size;
    if (expected_response_type == VIRTIO_MEM_RESP_ACK) {
      expected_plugged_size = prev_plugged_size_bytes + kPluggedBlockSize * num_blocks;
    } else {
      expected_plugged_size = prev_plugged_size_bytes;
    }
    WaitForPluggedSizeEqual(expected_plugged_size);
  }

  void Unplug(uint64_t addr, uint16_t num_blocks,
              uint16_t expected_response_type = VIRTIO_MEM_RESP_ACK) {
    uint64_t prev_plugged_size_bytes = InspectValue<uint64_t>("plugged_size_bytes");
    virtio_mem_req_t req = {.type = VIRTIO_MEM_REQ_UNPLUG, .addr = addr, .nb_blocks = num_blocks};
    virtio_mem_resp_t* resp;

    zx_status_t status = DescriptorChainBuilder(guest_request_queue_)
                             .AppendReadableDescriptor(&req, sizeof(req))
                             .AppendWritableDescriptor(&resp, sizeof(*resp))
                             .Build();
    EXPECT_EQ(ZX_OK, status);
    status = mem_->NotifyQueue(0);
    EXPECT_EQ(ZX_OK, status);
    status = WaitOnInterrupt();
    EXPECT_EQ(ZX_OK, status);

    EXPECT_EQ(expected_response_type, resp->type);
    uint64_t expected_plugged_size;
    if (expected_response_type == VIRTIO_MEM_RESP_ACK) {
      EXPECT_GT(prev_plugged_size_bytes, num_blocks * kPluggedBlockSize);
      expected_plugged_size = prev_plugged_size_bytes - kPluggedBlockSize * num_blocks;
    } else {
      expected_plugged_size = prev_plugged_size_bytes;
    }
    WaitForPluggedSizeEqual(expected_plugged_size);
  }

  void UnplugAll() {
    virtio_mem_req_t req = {.type = VIRTIO_MEM_REQ_UNPLUG_ALL};
    virtio_mem_resp_t* resp;

    zx_status_t status = DescriptorChainBuilder(guest_request_queue_)
                             .AppendReadableDescriptor(&req, sizeof(req))
                             .AppendWritableDescriptor(&resp, sizeof(*resp))
                             .Build();
    EXPECT_EQ(ZX_OK, status);
    status = mem_->NotifyQueue(0);
    EXPECT_EQ(ZX_OK, status);
    status = WaitOnInterrupt();
    EXPECT_EQ(ZX_OK, status);

    EXPECT_EQ(VIRTIO_MEM_RESP_ACK, resp->type);
    EXPECT_EQ(0u, InspectValue<uint64_t>("plugged_size_bytes"));
  }

  uint16_t GetState(uint64_t addr, uint16_t num_blocks,
                    uint16_t expected_response_type = VIRTIO_MEM_RESP_ACK) {
    virtio_mem_req_t req = {.type = VIRTIO_MEM_REQ_STATE, .addr = addr, .nb_blocks = num_blocks};
    virtio_mem_resp_t* resp;

    zx_status_t status = DescriptorChainBuilder(guest_request_queue_)
                             .AppendReadableDescriptor(&req, sizeof(req))
                             .AppendWritableDescriptor(&resp, sizeof(*resp))
                             .Build();
    EXPECT_EQ(ZX_OK, status);
    status = mem_->NotifyQueue(0);
    EXPECT_EQ(ZX_OK, status);
    status = WaitOnInterrupt();
    EXPECT_EQ(ZX_OK, status);

    EXPECT_EQ(expected_response_type, resp->type);
    if (expected_response_type == VIRTIO_MEM_RESP_ACK) {
      return resp->state_type;
    }
    return expected_response_type;
  }

 public:
  // Note: use of sync can be problematic here if the test environment needs to handle
  // some incoming FIDL requests.
  fuchsia::virtualization::hardware::VirtioMemSyncPtr mem_;
  VirtioQueueFake guest_request_queue_;
  using TestWithDevice::WaitOnInterrupt;
  std::unique_ptr<component_testing::RealmRoot> realm_;
  uint64_t region_addr_ = 0;
};

TEST_F(VirtioMemTest, PlugAndUnplugSuccess) {
  EXPECT_EQ(InspectValue<uint64_t>("plugged_size_bytes"), 0u);

  Plug(region_addr_, 7);
  EXPECT_EQ(VIRTIO_MEM_STATE_PLUGGED, GetState(region_addr_ + 1 * kPluggedBlockSize, 1));
  EXPECT_EQ(VIRTIO_MEM_STATE_MIXED, GetState(region_addr_ + 1 * kPluggedBlockSize, 7));
  EXPECT_EQ(VIRTIO_MEM_STATE_UNPLUGGED, GetState(region_addr_ + 7 * kPluggedBlockSize, 1));
  Plug(region_addr_ + 31 * kPluggedBlockSize, 3);
  EXPECT_EQ(VIRTIO_MEM_STATE_MIXED, GetState(region_addr_, 34));
  EXPECT_EQ(VIRTIO_MEM_STATE_PLUGGED, GetState(region_addr_ + 31 * kPluggedBlockSize, 3));
  EXPECT_EQ(VIRTIO_MEM_STATE_PLUGGED, GetState(region_addr_ + 32 * kPluggedBlockSize, 2));
  EXPECT_EQ(VIRTIO_MEM_STATE_PLUGGED, GetState(region_addr_ + 33 * kPluggedBlockSize, 1));
  EXPECT_EQ(VIRTIO_MEM_STATE_PLUGGED, GetState(region_addr_ + 31 * kPluggedBlockSize, 1));
  Unplug(region_addr_ + 32 * kPluggedBlockSize, 1);
  EXPECT_EQ(VIRTIO_MEM_STATE_MIXED, GetState(region_addr_ + 31 * kPluggedBlockSize, 3));
  EXPECT_EQ(VIRTIO_MEM_STATE_PLUGGED, GetState(region_addr_ + 31 * kPluggedBlockSize, 1));
  EXPECT_EQ(VIRTIO_MEM_STATE_PLUGGED, GetState(region_addr_ + 33 * kPluggedBlockSize, 1));
  Unplug(region_addr_ + 5 * kPluggedBlockSize, 2);
  Unplug(region_addr_, 5);
  EXPECT_EQ(VIRTIO_MEM_STATE_UNPLUGGED, GetState(region_addr_ + 1 * kPluggedBlockSize, 2));
  Plug(region_addr_ + 1 * kPluggedBlockSize, 2);
  EXPECT_EQ(VIRTIO_MEM_STATE_PLUGGED, GetState(region_addr_ + 1 * kPluggedBlockSize, 2));
  UnplugAll();
  EXPECT_EQ(VIRTIO_MEM_STATE_UNPLUGGED, GetState(region_addr_, kRegionSize / kPluggedBlockSize));
  Plug(region_addr_, 1);
  EXPECT_EQ(VIRTIO_MEM_STATE_PLUGGED, GetState(region_addr_, 1));
  EXPECT_EQ(VIRTIO_MEM_STATE_UNPLUGGED, GetState(region_addr_ + 1 * kPluggedBlockSize, 2));
  EXPECT_EQ(VIRTIO_MEM_STATE_UNPLUGGED, GetState(region_addr_ + 3 * kPluggedBlockSize, 1));
  UnplugAll();
  EXPECT_EQ(VIRTIO_MEM_STATE_UNPLUGGED, GetState(region_addr_, kRegionSize / kPluggedBlockSize));
  Plug(region_addr_ + kRegionSize - kPluggedBlockSize, 1);
  EXPECT_EQ(VIRTIO_MEM_STATE_MIXED, GetState(region_addr_, kRegionSize / kPluggedBlockSize));
  EXPECT_EQ(VIRTIO_MEM_STATE_PLUGGED, GetState(region_addr_ + kRegionSize - kPluggedBlockSize, 1));
  EXPECT_EQ(VIRTIO_MEM_STATE_UNPLUGGED,
            GetState(region_addr_, kRegionSize / kPluggedBlockSize - 1));
}

TEST_F(VirtioMemTest, PlugAndUnplugErrors) {
  // TODO(https://fxbug.dev/42051237): Find a way to suppress error logging from the virtio_mem
  // component just for this test out of bounds plugs
  Plug(region_addr_ + kRegionSize - kPluggedBlockSize, 2, VIRTIO_MEM_RESP_ERROR);
  Plug(region_addr_, kRegionSize / kPluggedBlockSize + 1, VIRTIO_MEM_RESP_ERROR);
  Plug(region_addr_ - kPluggedBlockSize, 1, VIRTIO_MEM_RESP_ERROR);
  // attempt to unplug non-plugged memory
  Unplug(region_addr_ + kPluggedBlockSize, 5, VIRTIO_MEM_RESP_ERROR);
  // double plug
  Plug(region_addr_ + kPluggedBlockSize, 5);
  Plug(region_addr_ + kPluggedBlockSize * 3, 1, VIRTIO_MEM_RESP_ERROR);
  Unplug(region_addr_ + kPluggedBlockSize * 3, 2);
  // double unplug
  Unplug(region_addr_ + kPluggedBlockSize * 3, 1, VIRTIO_MEM_RESP_ERROR);

  UnplugAll();
  // this block was unplugged by unplug_all
  Unplug(region_addr_ + kPluggedBlockSize, 1, VIRTIO_MEM_RESP_ERROR);
  // repeated unplug all is always successful
  UnplugAll();

  EXPECT_EQ(VIRTIO_MEM_RESP_ERROR,
            GetState(region_addr_, kRegionSize / kPluggedBlockSize + 1, VIRTIO_MEM_RESP_ERROR));
  EXPECT_EQ(VIRTIO_MEM_RESP_ERROR, GetState(region_addr_ - 10, 1, VIRTIO_MEM_RESP_ERROR));
}
