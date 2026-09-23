// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/drivers/misc/goldfish_control/control_device.h"

#include <fidl/fuchsia.hardware.goldfish.pipe/cpp/markers.h>
#include <fidl/fuchsia.hardware.goldfish.pipe/cpp/wire.h>
#include <fidl/fuchsia.hardware.goldfish/cpp/wire_test_base.h>
#include <fidl/fuchsia.hardware.sysmem/cpp/wire.h>
#include <fidl/fuchsia.hardware.sysmem/cpp/wire_test_base.h>
#include <fidl/fuchsia.sysmem/cpp/wire.h>
#include <fidl/fuchsia.sysmem2/cpp/wire.h>
#include <fidl/fuchsia.sysmem2/cpp/wire_test_base.h>
#include <lib/async-loop/loop.h>
#include <lib/async-loop/testing/cpp/real_loop.h>
#include <lib/async/cpp/task.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/fake-bti/bti.h>
#include <lib/fidl/cpp/wire/connect_service.h>
#include <lib/fzl/vmo-mapper.h>
#include <lib/sync/cpp/completion.h>
#include <lib/zx/bti.h>
#include <lib/zx/vmar.h>
#include <lib/zx/vmo.h>
#include <zircon/compiler.h>
#include <zircon/errors.h>
#include <zircon/rights.h>

#include <cstdlib>
#include <memory>
#include <mutex>
#include <string>
#include <unordered_map>

#include <fbl/array.h>
#include <fbl/auto_lock.h>
#include <gtest/gtest.h>

#include "src/devices/lib/goldfish/pipe_headers/include/base.h"
#include "src/devices/testing/mock-ddk/mock-device.h"
#include "src/graphics/drivers/misc/goldfish_control/render_control_commands.h"
#include "src/lib/fsl/handles/object_info.h"

#define ASSERT_OK(expr) ASSERT_EQ(ZX_OK, expr)
#define EXPECT_OK(expr) EXPECT_EQ(ZX_OK, expr)

namespace goldfish {
namespace {

// TODO(https://fxbug.dev/42161009): Use //src/devices/lib/goldfish/fake_pipe instead.
class FakePipe : public fidl::WireServer<fuchsia_hardware_goldfish_pipe::Bus> {
 public:
  void Create(CreateCompleter::Sync& completer) override {
    zx::vmo vmo;
    zx_status_t status = zx::vmo::create(zx_system_get_page_size(), 0u, &vmo);
    if (status != ZX_OK) {
      completer.Close(status);
      return;
    }
    status = vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &pipe_cmd_buffer_);
    if (status != ZX_OK) {
      completer.Close(status);
      return;
    }

    pipe_created_ = true;
    completer.ReplySuccess(kPipeId, std::move(vmo));
  }

  void SetEvent(SetEventRequestView request, SetEventCompleter::Sync& completer) override {
    if (request->id != kPipeId) {
      completer.Close(ZX_ERR_INVALID_ARGS);
      return;
    }
    if (!request->pipe_event.is_valid()) {
      completer.Close(ZX_ERR_BAD_HANDLE);
      return;
    }
    pipe_event_ = std::move(request->pipe_event);
    completer.ReplySuccess();
  }

  void Destroy(DestroyRequestView request, DestroyCompleter::Sync& completer) override {
    pipe_cmd_buffer_.reset();
    completer.Reply();
  }

  void Open(OpenRequestView request, OpenCompleter::Sync& completer) override {
    auto mapping = MapCmdBuffer();
    reinterpret_cast<PipeCmdBuffer*>(mapping.start())->status = 0;

    pipe_opened_ = true;
    completer.Reply();
  }

  void Exec(ExecRequestView request, ExecCompleter::Sync& completer) override {
    auto mapping = MapCmdBuffer();
    PipeCmdBuffer* cmd_buffer = reinterpret_cast<PipeCmdBuffer*>(mapping.start());
    cmd_buffer->rw_params.consumed_size = cmd_buffer->rw_params.sizes[0];
    cmd_buffer->status = 0;

    if (cmd_buffer->cmd ==
        static_cast<int32_t>(fuchsia_hardware_goldfish_pipe::PipeCmdCode::kWrite)) {
      // Store io buffer contents.
      auto io_buffer = MapIoBuffer();
      io_buffer_contents_.emplace_back(std::vector<uint8_t>(io_buffer_size_, 0));
      memcpy(io_buffer_contents_.back().data(), io_buffer.start(), io_buffer_size_);
    }

    if (cmd_buffer->cmd ==
        static_cast<int32_t>(fuchsia_hardware_goldfish_pipe::PipeCmdCode::kRead)) {
      auto io_buffer = MapIoBuffer();
      uint32_t op = *reinterpret_cast<uint32_t*>(io_buffer.start());

      switch (op) {
        case kOP_rcCreateBuffer2:
        case kOP_rcCreateColorBuffer:
          *reinterpret_cast<uint32_t*>(io_buffer.start()) = ++buffer_id_;
          break;
        case kOP_rcMapGpaToBufferHandle2:
        case kOP_rcSetColorBufferVulkanMode2:
          *reinterpret_cast<int32_t*>(io_buffer.start()) = 0;
          break;
        default:
          ZX_ASSERT_MSG(false, "invalid renderControl command (op %u)", op);
      }
    }

    completer.Reply();
  }

  void GetBti(GetBtiCompleter::Sync& completer) override {
    zx::bti bti;
    zx_status_t status = fake_bti_create(bti.reset_and_get_address());
    if (status != ZX_OK) {
      completer.Close(status);
      return;
    }
    bti_ = bti.borrow();
    completer.ReplySuccess(std::move(bti));
  }

  zx_status_t SetUpPipeDevice() {
    if (!pipe_io_buffer_.is_valid()) {
      zx_status_t status = PrepareIoBuffer();
      if (status != ZX_OK) {
        return status;
      }
    }
    return ZX_OK;
  }

  fzl::VmoMapper MapCmdBuffer() const {
    fzl::VmoMapper mapping;
    mapping.Map(pipe_cmd_buffer_, 0, sizeof(PipeCmdBuffer), ZX_VM_PERM_READ | ZX_VM_PERM_WRITE);
    return mapping;
  }

  fzl::VmoMapper MapIoBuffer() {
    if (!pipe_io_buffer_.is_valid()) {
      PrepareIoBuffer();
    }
    fzl::VmoMapper mapping;
    mapping.Map(pipe_io_buffer_, 0, io_buffer_size_, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE);
    return mapping;
  }

  bool IsPipeReady() const { return pipe_created_ && pipe_opened_; }

  uint32_t CurrentBufferHandle() { return buffer_id_; }

  const std::vector<std::vector<uint8_t>>& io_buffer_contents() const {
    return io_buffer_contents_;
  }

 private:
  zx_status_t PrepareIoBuffer() {
    uint64_t num_pinned_vmos = 0u;
    std::vector<fake_bti_pinned_vmo_info_t> pinned_vmos;
    zx_status_t status = fake_bti_get_pinned_vmos(bti_->get(), nullptr, 0, &num_pinned_vmos);
    if (status != ZX_OK) {
      return status;
    }
    if (num_pinned_vmos == 0u) {
      return ZX_ERR_NOT_FOUND;
    }

    pinned_vmos.resize(num_pinned_vmos);
    status = fake_bti_get_pinned_vmos(bti_->get(), pinned_vmos.data(), num_pinned_vmos, nullptr);
    if (status != ZX_OK) {
      return status;
    }

    pipe_io_buffer_ = zx::vmo(pinned_vmos.back().vmo);
    pinned_vmos.pop_back();
    // close all the unused handles
    for (auto vmo_info : pinned_vmos) {
      zx_handle_close(vmo_info.vmo);
    }

    status = pipe_io_buffer_.get_size(&io_buffer_size_);
    return status;
  }

  zx::unowned_bti bti_;

  static constexpr int32_t kPipeId = 1;
  zx::vmo pipe_cmd_buffer_ = zx::vmo();
  zx::vmo pipe_io_buffer_ = zx::vmo();
  size_t io_buffer_size_;

  zx::event pipe_event_;

  bool pipe_created_ = false;
  bool pipe_opened_ = false;

  int32_t buffer_id_ = 0;

  std::vector<std::vector<uint8_t>> io_buffer_contents_;
};

class FakeAddressSpace : public fidl::WireServer<fuchsia_hardware_goldfish::AddressSpaceDevice> {
  void OpenChildDriver(OpenChildDriverRequestView request,
                       OpenChildDriverCompleter::Sync& completer) override {
    request->req.Close(ZX_ERR_NOT_SUPPORTED);
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }
};

class FakeAddressSpaceChild
    : public fidl::testing::WireTestBase<fuchsia_hardware_goldfish::AddressSpaceChildDriver> {
  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) override {
    ADD_FAILURE() << "unexpected call to " << name;
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }
};

class FakeSync : public fidl::WireServer<fuchsia_hardware_goldfish::SyncDevice> {
 public:
  void CreateTimeline(CreateTimelineRequestView request,
                      CreateTimelineCompleter::Sync& completer) override {
    completer.Reply();
  }
};

class FakeHardwareSysmem;

class FakeSysmemAllocator : public fidl::testing::WireTestBase<fuchsia_sysmem2::Allocator> {
 public:
  FakeSysmemAllocator(FakeHardwareSysmem& parent) : parent_(parent) {}

  virtual void GetVmoInfo(::fuchsia_sysmem2::wire::AllocatorGetVmoInfoRequest* request,
                          GetVmoInfoCompleter::Sync& completer) override;

  void NotImplemented_(const std::string& name, ::fidl::CompleterBase& completer) override {
    ADD_FAILURE() << "unexpected call to " << name;
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }

 private:
  FakeHardwareSysmem& parent_;
};

class SysmemHeapEventHandler : public fidl::WireSyncEventHandler<fuchsia_hardware_sysmem::Heap> {
 public:
  SysmemHeapEventHandler() = default;
  void OnRegister(fidl::WireEvent<fuchsia_hardware_sysmem::Heap::OnRegister>* message) override {
    if (handler != nullptr) {
      handler(message);
    }
  }
  void SetOnRegisterHandler(
      fit::function<void(fidl::WireEvent<fuchsia_hardware_sysmem::Heap::OnRegister>*)>
          new_handler) {
    handler = std::move(new_handler);
  }

 private:
  fit::function<void(fidl::WireEvent<fuchsia_hardware_sysmem::Heap::OnRegister>*)> handler;
};

class FakeHardwareSysmem : public fidl::testing::WireTestBase<fuchsia_hardware_sysmem::Sysmem> {
 public:
  struct HeapInfo {
    bool is_registered = false;
    bool cpu_supported = false;
    bool ram_supported = false;
    bool inaccessible_supported = false;
  };

  // Blocks until all heaps listed in `heaps` are connected with a Heap server
  // connection.
  //
  // Must run on a dispatcher different from the one `FakeHardwareSysmem` is
  // bound to.
  void WaitUntilAllHeapsAreConnected(const std::vector<fuchsia_sysmem::HeapType>& heaps) {
    for (const fuchsia_sysmem::HeapType heap_type : heaps) {
      while (!IsHeapConnected(static_cast<uint64_t>(heap_type))) {
        static constexpr zx::duration kStep = zx::msec(1);
        zx::nanosleep(zx::deadline_after(kStep));
      }
    }
  }

  // Must be called after `WaitUntilAllHeapsAreConnected()`.
  zx_status_t SetupHeaps() {
    std::lock_guard lock(mutex_);
    for (const auto& [heap_type, heap_client] : heap_clients_) {
      HeapInfo& heap = heap_info_[heap_type];
      SysmemHeapEventHandler handler;
      handler.SetOnRegisterHandler(
          [&heap](fidl::WireEvent<fuchsia_hardware_sysmem::Heap::OnRegister>* message) {
            heap.is_registered = true;
            heap.cpu_supported = message->properties.coherency_domain_support().cpu_supported();
            heap.ram_supported = message->properties.coherency_domain_support().ram_supported();
            heap.inaccessible_supported =
                message->properties.coherency_domain_support().inaccessible_supported();
          });

      zx_status_t status = handler.HandleOneEvent(heap_client).status();
      if (status != ZX_OK) {
        return status;
      }
    }
    return ZX_OK;
  }

  void AddFakeVmoInfo(const zx::vmo& vmo, BufferKey buffer_key) {
    zx_koid_t koid = fsl::GetKoid(vmo.get());
    ZX_ASSERT(koid != ZX_KOID_INVALID);
    auto emplace_result = vmo_infos_.try_emplace(koid, buffer_key);
    ZX_ASSERT(emplace_result.second);
  }

  std::optional<BufferKey> LookupFakeVmoInfo(const zx::vmo& vmo) {
    zx_koid_t koid = fsl::GetKoid(vmo.get());
    auto iter = vmo_infos_.find(koid);
    if (iter == vmo_infos_.end()) {
      return std::nullopt;
    }
    return iter->second;
  }

  void RegisterHeap(RegisterHeapRequestView request,
                    RegisterHeapCompleter::Sync& completer) override {
    std::lock_guard lock(mutex_);
    heap_clients_[request->heap] = std::move(request->heap_connection);
  }

  void NotImplemented_(const std::string& name, ::fidl::CompleterBase& completer) override {
    ADD_FAILURE() << "unexpected call to " << name;
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }

  std::unordered_map<uint64_t, HeapInfo> CloneHeapInfo() const {
    std::lock_guard lock(mutex_);
    return heap_info_;
  }

 private:
  bool IsHeapConnected(uint64_t heap) {
    std::lock_guard lock(mutex_);
    return heap_clients_.find(heap) != heap_clients_.end();
  }

  using VmoInfoMap = std::unordered_map<zx_koid_t, BufferKey>;

  mutable std::mutex mutex_;
  VmoInfoMap vmo_infos_;

  std::unordered_map<uint64_t, fidl::ClientEnd<fuchsia_hardware_sysmem::Heap>> heap_clients_
      __TA_GUARDED(mutex_);
  std::unordered_map<uint64_t, HeapInfo> heap_info_ __TA_GUARDED(mutex_);
};

void FakeSysmemAllocator::GetVmoInfo(::fuchsia_sysmem2::wire::AllocatorGetVmoInfoRequest* request,
                                     GetVmoInfoCompleter::Sync& completer) {
  auto buffer_key = parent_.LookupFakeVmoInfo(request->vmo());
  if (!buffer_key.has_value()) {
    completer.ReplyError(fuchsia_sysmem2::Error::kNotFound);
    return;
  }
  fidl::Arena arena;
  auto response = fuchsia_sysmem2::wire::AllocatorGetVmoInfoResponse::Builder(arena);
  response.buffer_collection_id(buffer_key->first);
  response.buffer_index(buffer_key->second);
  completer.ReplySuccess(response.Build());
}

class ControlDeviceTest : public testing::Test, public loop_fixture::RealLoop {
 public:
  ControlDeviceTest()
      : loop_(&kAsyncLoopConfigNeverAttachToThread),
        device_loop_(&kAsyncLoopConfigNeverAttachToThread),
        pipe_server_loop_(&kAsyncLoopConfigNeverAttachToThread),
        address_space_server_loop_(&kAsyncLoopConfigNeverAttachToThread),
        sync_server_loop_(&kAsyncLoopConfigNeverAttachToThread),
        sysmem_server_loop_(&kAsyncLoopConfigNeverAttachToThread),
        sysmem_(hardware_sysmem_),
        outgoing_(dispatcher()) {}

  void SetUp() override {
    fake_parent_ = MockDevice::FakeRootParent();

    zx::result service_result = outgoing_.AddService<fuchsia_hardware_goldfish_pipe::Service>(
        fuchsia_hardware_goldfish_pipe::Service::InstanceHandler({
            .device = pipe_.bind_handler(pipe_server_loop_.dispatcher()),
        }));
    ASSERT_EQ(service_result.status_value(), ZX_OK);

    zx::result endpoints = fidl::CreateEndpoints<fuchsia_io::Directory>();
    ASSERT_OK(endpoints.status_value());
    ASSERT_OK(outgoing_.Serve(std::move(endpoints->server)).status_value());

    fake_parent_->AddFidlService(fuchsia_hardware_goldfish_pipe::Service::Name,
                                 std::move(endpoints->client), "goldfish-pipe");

    service_result = outgoing_.AddService<fuchsia_hardware_goldfish::AddressSpaceService>(
        fuchsia_hardware_goldfish::AddressSpaceService::InstanceHandler({
            .device = address_space_.bind_handler(address_space_server_loop_.dispatcher()),
        }));
    ASSERT_EQ(service_result.status_value(), ZX_OK);

    endpoints = fidl::CreateEndpoints<fuchsia_io::Directory>();
    ASSERT_OK(endpoints.status_value());
    ASSERT_OK(outgoing_.Serve(std::move(endpoints->server)).status_value());

    fake_parent_->AddFidlService(fuchsia_hardware_goldfish::AddressSpaceService::Name,
                                 std::move(endpoints->client), "goldfish-address-space");

    service_result = outgoing_.AddService<fuchsia_hardware_goldfish::SyncService>(
        fuchsia_hardware_goldfish::SyncService::InstanceHandler({
            .device = sync_.bind_handler(sync_server_loop_.dispatcher()),
        }));
    ASSERT_EQ(service_result.status_value(), ZX_OK);

    endpoints = fidl::CreateEndpoints<fuchsia_io::Directory>();
    ASSERT_OK(endpoints.status_value());
    ASSERT_OK(outgoing_.Serve(std::move(endpoints->server)).status_value());

    fake_parent_->AddFidlService(fuchsia_hardware_goldfish::SyncService::Name,
                                 std::move(endpoints->client), "goldfish-sync");

    fake_parent_->AddNsProtocol<fuchsia_sysmem2::Allocator>(
        sysmem_.bind_handler(sysmem_server_loop_.dispatcher()));

    fake_parent_->AddNsProtocol<fuchsia_hardware_sysmem::Sysmem>(
        hardware_sysmem_.bind_handler(sysmem_server_loop_.dispatcher()));

    device_loop_.StartThread("device-loop");
    pipe_server_loop_.StartThread("goldfish-pipe-fidl-server");
    address_space_server_loop_.StartThread("goldfish-address-space-fidl-server");
    sync_server_loop_.StartThread("goldfish-sync-fidl-server");
    sysmem_server_loop_.StartThread("sysmem-fidl-server");

    libsync::Completion device_bound;
    async::PostTask(device_loop_.dispatcher(), [this, &device_bound] {
      auto dut = std::make_unique<Control>(fake_parent_.get(), device_loop_.dispatcher());
      ZX_ASSERT(dut->Bind() == ZX_OK);
      // The device will be deleted by MockDevice when the test ends.
      dut.release();
      device_bound.Signal();
    });
    PerformBlockingWork([&device_bound] { device_bound.Wait(); });

    ASSERT_EQ(fake_parent_->child_count(), 1u);
    auto fake_dut = fake_parent_->GetLatestChild();
    dut_ = fake_dut->GetDeviceContext<Control>();

    hardware_sysmem_.WaitUntilAllHeapsAreConnected({
        fuchsia_sysmem::wire::HeapType::kGoldfishDeviceLocal,
        fuchsia_sysmem::wire::HeapType::kGoldfishHostVisible,
    });
    ASSERT_OK(hardware_sysmem_.SetupHeaps());
    ASSERT_OK(pipe_.SetUpPipeDevice());
    ASSERT_TRUE(pipe_.IsPipeReady());

    // Bind control device FIDL server.
    auto control_endpoints = fidl::CreateEndpoints<fuchsia_hardware_goldfish::ControlDevice>();
    ASSERT_TRUE(control_endpoints.is_ok());

    control_fidl_server_ =
        fidl::BindServer(loop_.dispatcher(), std::move(control_endpoints->server),
                         fake_dut->GetDeviceContext<Control>());

    loop_.StartThread("goldfish-control-device-fidl-server");

    fidl_client_ = fidl::WireSyncClient(std::move(control_endpoints->client));
  }

  void TearDown() override {
    device_async_remove(dut_->zxdev());
    libsync::Completion device_released;
    async::PostTask(device_loop_.dispatcher(), [this, &device_released] {
      mock_ddk::ReleaseFlaggedDevices(fake_parent_.get());
      device_released.Signal();
    });
    device_released.Wait();
  }

 protected:
  Control* dut_ = nullptr;

  async::Loop loop_;
  async::Loop device_loop_;
  async::Loop pipe_server_loop_;
  async::Loop address_space_server_loop_;
  async::Loop sync_server_loop_;
  async::Loop sysmem_server_loop_;

  FakePipe pipe_;
  FakeAddressSpace address_space_;
  FakeAddressSpaceChild address_space_child_;
  FakeSync sync_;
  FakeHardwareSysmem hardware_sysmem_;
  FakeSysmemAllocator sysmem_;

  std::shared_ptr<MockDevice> fake_parent_;

  component::OutgoingDirectory outgoing_;
  std::optional<fidl::ServerBindingRef<fuchsia_hardware_goldfish::ControlDevice>>
      control_fidl_server_ = std::nullopt;
  fidl::WireSyncClient<fuchsia_hardware_goldfish::ControlDevice> fidl_client_ = {};
};

TEST_F(ControlDeviceTest, Bind) {
  const std::unordered_map<uint64_t, FakeHardwareSysmem::HeapInfo> heaps =
      hardware_sysmem_.CloneHeapInfo();
  ASSERT_EQ(heaps.size(), 2u);
  ASSERT_TRUE(heaps.find(static_cast<uint64_t>(
                  fuchsia_sysmem::wire::HeapType::kGoldfishDeviceLocal)) != heaps.end());
  ASSERT_TRUE(heaps.find(static_cast<uint64_t>(
                  fuchsia_sysmem::wire::HeapType::kGoldfishHostVisible)) != heaps.end());

  const auto& device_local_heap_info =
      heaps.at(static_cast<uint64_t>(fuchsia_sysmem::wire::HeapType::kGoldfishDeviceLocal));
  EXPECT_TRUE(device_local_heap_info.is_registered);
  EXPECT_TRUE(device_local_heap_info.inaccessible_supported);

  const auto& host_visible_heap_info =
      heaps.at(static_cast<uint64_t>(fuchsia_sysmem::wire::HeapType::kGoldfishHostVisible));
  EXPECT_TRUE(host_visible_heap_info.is_registered);
  EXPECT_TRUE(host_visible_heap_info.cpu_supported);
}

// Test |fuchsia.hardware.goldfish.Control.CreateColorBuffer2| method.
class ColorBufferTest
    : public ControlDeviceTest,
      public testing::WithParamInterface<
          std::tuple<fuchsia_hardware_goldfish::wire::ColorBufferFormatType, uint32_t>> {};

TEST_P(ColorBufferTest, TestCreate) {
  constexpr uint32_t kWidth = 1024u;
  constexpr uint32_t kHeight = 768u;
  constexpr uint32_t kSize = kWidth * kHeight * 4;
  constexpr uint64_t kPhysicalAddress = 0x12345678abcd0000;
  const auto format = std::get<0>(GetParam());
  const auto memory_property = std::get<1>(GetParam());
  const bool is_host_visible =
      memory_property == fuchsia_hardware_goldfish::wire::kMemoryPropertyHostVisible;
  const BufferKey buffer_key(14, 2);

  zx::vmo buffer_vmo;
  ASSERT_OK(zx::vmo::create(kSize, 0u, &buffer_vmo));

  hardware_sysmem_.AddFakeVmoInfo(buffer_vmo, buffer_key);
  dut_->RegisterBufferHandle(buffer_key);

  fidl::Arena allocator;
  fuchsia_hardware_goldfish::wire::CreateColorBuffer2Params create_params(allocator);
  create_params.set_width(kWidth).set_height(kHeight).set_format(format).set_memory_property(
      memory_property);
  if (is_host_visible) {
    create_params.set_physical_address(allocator, kPhysicalAddress);
  }

  auto create_color_buffer_result =
      fidl_client_->CreateColorBuffer2(std::move(buffer_vmo), std::move(create_params));

  ASSERT_TRUE(create_color_buffer_result.ok());
  EXPECT_OK(create_color_buffer_result.value().res);
  const int32_t expected_page_offset = is_host_visible ? 0 : -1;
  EXPECT_EQ(create_color_buffer_result.value().hw_address_page_offset, expected_page_offset);

  CreateColorBufferCmd create_color_buffer_cmd{
      .op = kOP_rcCreateColorBuffer,
      .size = kSize_rcCreateColorBuffer,
      .width = kWidth,
      .height = kHeight,
      .internalformat = static_cast<uint32_t>(format),
  };

  SetColorBufferVulkanMode2Cmd set_vulkan_mode_cmd{
      .op = kOP_rcSetColorBufferVulkanMode2,
      .size = kSize_rcSetColorBufferVulkanMode2,
      .id = pipe_.CurrentBufferHandle(),
      .mode = 1u,  // VULKAN_ONLY
      .memory_property = memory_property,
  };

  MapGpaToBufferHandle2Cmd map_gpa_cmd{
      .op = kOP_rcMapGpaToBufferHandle2,
      .size = kSize_rcMapGpaToBufferHandle2,
      .id = pipe_.CurrentBufferHandle(),
      .gpa = kPhysicalAddress,
      .map_size = kSize,
  };

  const auto& io_buffer_contents = pipe_.io_buffer_contents();
  size_t create_color_buffer_cmd_idx = 0;
  if (is_host_visible) {
    ASSERT_GE(io_buffer_contents.size(), 3u);
    create_color_buffer_cmd_idx = io_buffer_contents.size() - 3;
  } else {
    ASSERT_GE(io_buffer_contents.size(), 2u);
    create_color_buffer_cmd_idx = io_buffer_contents.size() - 2;
  }

  EXPECT_EQ(memcmp(&create_color_buffer_cmd, io_buffer_contents[create_color_buffer_cmd_idx].data(),
                   sizeof(CreateColorBufferCmd)),
            0);
  EXPECT_EQ(memcmp(&set_vulkan_mode_cmd, io_buffer_contents[create_color_buffer_cmd_idx + 1].data(),
                   sizeof(set_vulkan_mode_cmd)),
            0);
  if (is_host_visible) {
    EXPECT_EQ(memcmp(&map_gpa_cmd, io_buffer_contents[create_color_buffer_cmd_idx + 2].data(),
                     sizeof(MapGpaToBufferHandle2Cmd)),
              0);
  }
}

INSTANTIATE_TEST_SUITE_P(
    ControlDeviceTest, ColorBufferTest,
    testing::Combine(
        testing::Values(fuchsia_hardware_goldfish::wire::ColorBufferFormatType::kRg,
                        fuchsia_hardware_goldfish::wire::ColorBufferFormatType::kRgba,
                        fuchsia_hardware_goldfish::wire::ColorBufferFormatType::kBgra,
                        fuchsia_hardware_goldfish::wire::ColorBufferFormatType::kLuminance),
        testing::Values(fuchsia_hardware_goldfish::wire::kMemoryPropertyDeviceLocal,
                        fuchsia_hardware_goldfish::wire::kMemoryPropertyHostVisible)),
    [](const testing::TestParamInfo<ColorBufferTest::ParamType>& info) {
      std::string format;
      switch (std::get<0>(info.param)) {
        case fuchsia_hardware_goldfish::wire::ColorBufferFormatType::kRg:
          format = "RG";
          break;
        case fuchsia_hardware_goldfish::wire::ColorBufferFormatType::kRgba:
          format = "RGBA";
          break;
        case fuchsia_hardware_goldfish::wire::ColorBufferFormatType::kBgra:
          format = "BGRA";
          break;
        case fuchsia_hardware_goldfish::wire::ColorBufferFormatType::kLuminance:
          format = "LUMINANCE";
          break;
        default:
          format = "UNSUPPORTED_FORMAT";
      }

      std::string memory_property;
      switch (std::get<1>(info.param)) {
        case fuchsia_hardware_goldfish::wire::kMemoryPropertyDeviceLocal:
          memory_property = "DEVICE_LOCAL";
          break;
        case fuchsia_hardware_goldfish::wire::kMemoryPropertyHostVisible:
          memory_property = "HOST_VISIBLE";
          break;
        default:
          memory_property = "UNSUPPORTED_MEMORY_PROPERTY";
      }

      return format + "_" + memory_property;
    });

TEST_F(ControlDeviceTest, CreateColorBuffer2_AlreadyExists) {
  constexpr uint32_t kWidth = 1024u;
  constexpr uint32_t kHeight = 768u;
  constexpr uint32_t kSize = kWidth * kHeight * 4;
  constexpr auto kFormat = fuchsia_hardware_goldfish::wire::ColorBufferFormatType::kRgba;
  constexpr auto kMemoryProperty = fuchsia_hardware_goldfish::wire::kMemoryPropertyDeviceLocal;
  const BufferKey buffer_key(15, 3);

  zx::vmo buffer_vmo;
  ASSERT_OK(zx::vmo::create(kSize, 0u, &buffer_vmo));

  zx::vmo copy_vmo;
  ASSERT_OK(buffer_vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &copy_vmo));

  // The object koid is the same for both VMO handles, so GetVmoInfo() will return buffer_key for
  // both VMO handles.
  hardware_sysmem_.AddFakeVmoInfo(buffer_vmo, buffer_key);
  dut_->RegisterBufferHandle(buffer_key);

  {
    fidl::Arena allocator;
    fuchsia_hardware_goldfish::wire::CreateColorBuffer2Params create_params(allocator);
    create_params.set_width(kWidth).set_height(kHeight).set_format(kFormat).set_memory_property(
        kMemoryProperty);

    auto create_color_buffer_result =
        fidl_client_->CreateColorBuffer2(std::move(buffer_vmo), std::move(create_params));

    ASSERT_TRUE(create_color_buffer_result.ok());
    EXPECT_OK(create_color_buffer_result.value().res);
  }

  {
    fidl::Arena allocator;
    fuchsia_hardware_goldfish::wire::CreateColorBuffer2Params create_params(allocator);
    create_params.set_width(kWidth).set_height(kHeight).set_format(kFormat).set_memory_property(
        kMemoryProperty);

    auto create_copy_buffer_result =
        fidl_client_->CreateColorBuffer2(std::move(copy_vmo), std::move(create_params));

    ASSERT_TRUE(create_copy_buffer_result.ok());
    ASSERT_EQ(create_copy_buffer_result.value().res, ZX_ERR_ALREADY_EXISTS);
  }
}

TEST_F(ControlDeviceTest, CreateColorBuffer2_InvalidArgs) {
  constexpr uint32_t kWidth = 1024u;
  constexpr uint32_t kHeight = 768u;
  constexpr uint32_t kSize = kWidth * kHeight * 4;
  constexpr auto kFormat = fuchsia_hardware_goldfish::wire::ColorBufferFormatType::kRgba;
  constexpr auto kMemoryProperty = fuchsia_hardware_goldfish::wire::kMemoryPropertyDeviceLocal;

  {
    const BufferKey buffer_key(16, 4);
    zx::vmo buffer_vmo;
    ASSERT_OK(zx::vmo::create(kSize, 0u, &buffer_vmo));

    hardware_sysmem_.AddFakeVmoInfo(buffer_vmo, buffer_key);
    dut_->RegisterBufferHandle(buffer_key);

    fidl::Arena allocator;
    fuchsia_hardware_goldfish::wire::CreateColorBuffer2Params create_params(allocator);
    // missing width
    create_params.set_height(kHeight).set_format(kFormat).set_memory_property(kMemoryProperty);

    auto create_color_buffer_result =
        fidl_client_->CreateColorBuffer2(std::move(buffer_vmo), std::move(create_params));

    ASSERT_TRUE(create_color_buffer_result.ok());
    EXPECT_EQ(create_color_buffer_result.value().res, ZX_ERR_INVALID_ARGS);

    dut_->FreeBufferHandle(buffer_key);
  }

  {
    const BufferKey buffer_key(17, 5);
    zx::vmo buffer_vmo;
    ASSERT_OK(zx::vmo::create(kSize, 0u, &buffer_vmo));

    hardware_sysmem_.AddFakeVmoInfo(buffer_vmo, buffer_key);
    dut_->RegisterBufferHandle(buffer_key);

    fidl::Arena allocator;
    fuchsia_hardware_goldfish::wire::CreateColorBuffer2Params create_params(allocator);
    // missing height
    create_params.set_width(kWidth).set_format(kFormat).set_memory_property(kMemoryProperty);

    auto create_color_buffer_result =
        fidl_client_->CreateColorBuffer2(std::move(buffer_vmo), std::move(create_params));

    ASSERT_TRUE(create_color_buffer_result.ok());
    EXPECT_EQ(create_color_buffer_result.value().res, ZX_ERR_INVALID_ARGS);

    dut_->FreeBufferHandle(buffer_key);
  }

  {
    const BufferKey buffer_key(18, 6);
    zx::vmo buffer_vmo;
    ASSERT_OK(zx::vmo::create(kSize, 0u, &buffer_vmo));

    hardware_sysmem_.AddFakeVmoInfo(buffer_vmo, buffer_key);
    dut_->RegisterBufferHandle(buffer_key);

    fidl::Arena allocator;
    fuchsia_hardware_goldfish::wire::CreateColorBuffer2Params create_params(allocator);
    // missing format
    create_params.set_width(kWidth).set_height(kHeight).set_memory_property(kMemoryProperty);

    auto create_color_buffer_result =
        fidl_client_->CreateColorBuffer2(std::move(buffer_vmo), std::move(create_params));

    ASSERT_TRUE(create_color_buffer_result.ok());
    EXPECT_EQ(create_color_buffer_result.value().res, ZX_ERR_INVALID_ARGS);

    dut_->FreeBufferHandle(buffer_key);
  }

  {
    const BufferKey buffer_key(19, 7);
    zx::vmo buffer_vmo;
    ASSERT_OK(zx::vmo::create(kSize, 0u, &buffer_vmo));

    hardware_sysmem_.AddFakeVmoInfo(buffer_vmo, buffer_key);
    dut_->RegisterBufferHandle(buffer_key);

    fidl::Arena allocator;
    fuchsia_hardware_goldfish::wire::CreateColorBuffer2Params create_params(allocator);
    // missing memory property
    create_params.set_width(kWidth).set_height(kHeight).set_format(kFormat);

    auto create_color_buffer_result =
        fidl_client_->CreateColorBuffer2(std::move(buffer_vmo), std::move(create_params));

    ASSERT_TRUE(create_color_buffer_result.ok());
    EXPECT_EQ(create_color_buffer_result.value().res, ZX_ERR_INVALID_ARGS);

    dut_->FreeBufferHandle(buffer_key);
  }

  {
    const BufferKey buffer_key(20, 8);
    zx::vmo buffer_vmo;
    ASSERT_OK(zx::vmo::create(kSize, 0u, &buffer_vmo));

    hardware_sysmem_.AddFakeVmoInfo(buffer_vmo, buffer_key);
    dut_->RegisterBufferHandle(buffer_key);

    fidl::Arena allocator;
    fuchsia_hardware_goldfish::wire::CreateColorBuffer2Params create_params(allocator);
    // missing physical address
    create_params.set_width(kWidth).set_height(kHeight).set_format(kFormat).set_memory_property(
        fuchsia_hardware_goldfish::wire::kMemoryPropertyHostVisible);

    auto create_color_buffer_result =
        fidl_client_->CreateColorBuffer2(std::move(buffer_vmo), std::move(create_params));

    ASSERT_TRUE(create_color_buffer_result.ok());
    EXPECT_EQ(create_color_buffer_result.value().res, ZX_ERR_INVALID_ARGS);

    dut_->FreeBufferHandle(buffer_key);
  }
}

TEST_F(ControlDeviceTest, CreateColorBuffer2_InvalidVmo) {
  constexpr uint32_t kWidth = 1024u;
  constexpr uint32_t kHeight = 768u;
  constexpr uint32_t kSize = kWidth * kHeight * 4;
  constexpr auto kFormat = fuchsia_hardware_goldfish::wire::ColorBufferFormatType::kRgba;
  constexpr auto kMemoryProperty = fuchsia_hardware_goldfish::wire::kMemoryPropertyDeviceLocal;

  zx::vmo buffer_vmo;
  ASSERT_OK(zx::vmo::create(kSize, 0u, &buffer_vmo));

  // no sysmem_.AddVakeVmoInfo()
  // no dut_->RegisterBufferHandle()

  {
    fidl::Arena allocator;
    fuchsia_hardware_goldfish::wire::CreateColorBuffer2Params create_params(allocator);
    create_params.set_width(kWidth).set_height(kHeight).set_format(kFormat).set_memory_property(
        kMemoryProperty);

    auto create_unregistered_buffer_result =
        fidl_client_->CreateColorBuffer2(std::move(buffer_vmo), std::move(create_params));

    ASSERT_TRUE(create_unregistered_buffer_result.ok());
    EXPECT_EQ(create_unregistered_buffer_result.value().res, ZX_ERR_INVALID_ARGS);
  }

  {
    fidl::Arena allocator;
    fuchsia_hardware_goldfish::wire::CreateColorBuffer2Params create_params(allocator);
    create_params.set_width(kWidth).set_height(kHeight).set_format(kFormat).set_memory_property(
        kMemoryProperty);

    auto create_invalid_buffer_result =
        fidl_client_->CreateColorBuffer2(zx::vmo(), std::move(create_params));

    ASSERT_EQ(create_invalid_buffer_result.status(), ZX_ERR_INVALID_ARGS);
  }
}

TEST_F(ControlDeviceTest, GetBufferHandle_Invalid) {
  // Register data buffer, but don't create it.
  {
    const BufferKey buffer_key(23, 22);
    constexpr size_t kSize = 65536u;
    zx::vmo buffer_vmo;
    ASSERT_OK(zx::vmo::create(kSize, 0u, &buffer_vmo));

    hardware_sysmem_.AddFakeVmoInfo(buffer_vmo, buffer_key);
    dut_->RegisterBufferHandle(buffer_key);

    auto get_buffer_handle_result = fidl_client_->GetBufferHandle(std::move(buffer_vmo));
    ASSERT_TRUE(get_buffer_handle_result.ok());
    EXPECT_EQ(get_buffer_handle_result.value().res, ZX_ERR_NOT_FOUND);

    dut_->FreeBufferHandle(buffer_key);
  }

  // Check non-registered buffer VMO.
  {
    constexpr size_t kSize = 65536u;
    zx::vmo buffer_vmo;
    ASSERT_OK(zx::vmo::create(kSize, 0u, &buffer_vmo));

    auto get_buffer_handle_result = fidl_client_->GetBufferHandle(std::move(buffer_vmo));
    ASSERT_TRUE(get_buffer_handle_result.ok());
    EXPECT_EQ(get_buffer_handle_result.value().res, ZX_ERR_INVALID_ARGS);
  }

  // Check invalid buffer VMO.
  {
    auto get_buffer_handle_result = fidl_client_->GetBufferHandle(zx::vmo());
    ASSERT_EQ(get_buffer_handle_result.status(), ZX_ERR_INVALID_ARGS);
  }
}

TEST_F(ControlDeviceTest, GetBufferHandleInfo_Invalid) {
  // Register data buffer, but don't create it.
  {
    const BufferKey buffer_key(24, 23);
    constexpr size_t kSize = 65536u;
    zx::vmo buffer_vmo;
    ASSERT_OK(zx::vmo::create(kSize, 0u, &buffer_vmo));

    hardware_sysmem_.AddFakeVmoInfo(buffer_vmo, buffer_key);
    dut_->RegisterBufferHandle(buffer_key);

    auto get_buffer_handle_info_result = fidl_client_->GetBufferHandleInfo(std::move(buffer_vmo));
    ASSERT_TRUE(get_buffer_handle_info_result.ok());
    EXPECT_TRUE(get_buffer_handle_info_result->is_error());
    EXPECT_EQ(get_buffer_handle_info_result->error_value(), ZX_ERR_NOT_FOUND);

    dut_->FreeBufferHandle(buffer_key);
  }

  // Check non-registered buffer VMO.
  {
    constexpr size_t kSize = 65536u;
    zx::vmo buffer_vmo;
    ASSERT_OK(zx::vmo::create(kSize, 0u, &buffer_vmo));

    auto get_buffer_handle_info_result = fidl_client_->GetBufferHandleInfo(std::move(buffer_vmo));
    ASSERT_TRUE(get_buffer_handle_info_result.ok());
    EXPECT_TRUE(get_buffer_handle_info_result->is_error());
    EXPECT_EQ(get_buffer_handle_info_result->error_value(), ZX_ERR_INVALID_ARGS);
  }

  // Check invalid buffer VMO.
  {
    auto get_buffer_handle_info_result = fidl_client_->GetBufferHandleInfo(zx::vmo());
    ASSERT_EQ(get_buffer_handle_info_result.status(), ZX_ERR_INVALID_ARGS);
  }
}

}  // namespace
}  // namespace goldfish
