// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/drivers/misc/goldfish_control/control_device.h"

#include <fidl/fuchsia.hardware.goldfish.pipe/cpp/wire.h>
#include <fidl/fuchsia.hardware.goldfish/cpp/markers.h>
#include <fidl/fuchsia.hardware.goldfish/cpp/wire.h>
#include <fidl/fuchsia.hardware.sysmem/cpp/fidl.h>
#include <lib/ddk/binding_driver.h>
#include <lib/ddk/debug.h>
#include <lib/ddk/driver.h>
#include <lib/ddk/platform-defs.h>
#include <lib/ddk/trace/event.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/fit/defer.h>
#include <lib/sysmem-version/sysmem-version.h>
#include <zircon/syscalls.h>

#include <memory>

#include <ddktl/fidl.h>
#include <fbl/auto_lock.h>

#include "src/devices/lib/goldfish/pipe_headers/include/base.h"
#include "src/graphics/drivers/misc/goldfish_control/device_local_heap.h"
#include "src/graphics/drivers/misc/goldfish_control/host_visible_heap.h"
#include "src/graphics/drivers/misc/goldfish_control/render_control_commands.h"

namespace goldfish {
namespace {

const char* kTag = "goldfish-control";

const char* kPipeName = "pipe:opengles";

constexpr uint32_t kClientFlags = 0;

constexpr uint32_t VULKAN_ONLY = 1;

constexpr uint32_t kInvalidBufferHandle = 0U;

zx::result<> RegisterAndBindHeap(
    fidl::UnownedClientEnd<fuchsia_hardware_sysmem::Sysmem> hardware_sysmem,
    fuchsia_sysmem::wire::HeapType heap_type, Heap* heap) {
  auto [heap_client, heap_server] = fidl::Endpoints<fuchsia_hardware_sysmem::Heap>::Create();

  fidl::OneWayStatus register_result =
      fidl::WireCall(hardware_sysmem)
          ->RegisterHeap(static_cast<uint64_t>(heap_type), std::move(heap_client));
  if (!register_result.ok()) {
    zxlogf(ERROR, "Failed to register Heap to sysmem: %s",
           register_result.FormatDescription().c_str());
    return zx::error(register_result.status());
  }

  heap->Bind(std::move(heap_server).TakeChannel());
  return zx::ok();
}

}  // namespace

// static
zx_status_t Control::Create(void* ctx, zx_device_t* device) {
  async_dispatcher_t* async_dispatcher = fdf::Dispatcher::GetCurrent()->async_dispatcher();
  auto control = std::make_unique<Control>(device, async_dispatcher);

  zx_status_t status = control->Bind();
  if (status == ZX_OK) {
    // devmgr now owns device.
    [[maybe_unused]] auto* dev = control.release();
  }
  return status;
}

Control::Control(zx_device_t* parent, async_dispatcher_t* dispatcher)
    : ControlType(parent), dispatcher_(dispatcher), outgoing_(dispatcher_) {
  // Initialize parent protocols.
  Init();
}

Control::~Control() {
  if (id_) {
    fbl::AutoLock lock(&lock_);
    if (cmd_buffer_.is_valid()) {
      for (auto& buffer : buffer_handles_) {
        CloseBufferOrColorBufferLocked(buffer.second);
      }
      auto buffer = static_cast<PipeCmdBuffer*>(cmd_buffer_.virt());
      buffer->id = id_;
      buffer->cmd = static_cast<int32_t>(fuchsia_hardware_goldfish_pipe::PipeCmdCode::kClose);
      buffer->status = static_cast<int32_t>(fuchsia_hardware_goldfish_pipe::PipeError::kInval);

      [[maybe_unused]] auto result = pipe_bus_->Exec(id_);
    }
    [[maybe_unused]] auto destroy_result = pipe_bus_->Destroy(id_);
    // We don't check the return status as the pipe is destroyed on a
    // best-effort basis.
  }
}

zx_status_t Control::Init() {
  zx::result client =
      DdkConnectFragmentFidlProtocol<fuchsia_hardware_goldfish_pipe::Service::Device>(
          "goldfish-pipe");
  if (client.is_error()) {
    zxlogf(ERROR, "%s: failed to connect to FIDL fragment: %s", kTag, client.status_string());
    return client.status_value();
  }
  pipe_bus_ = fidl::WireSyncClient(std::move(client.value()));

  zx::result address_space_client =
      DdkConnectFragmentFidlProtocol<fuchsia_hardware_goldfish::AddressSpaceService::Device>(
          "goldfish-address-space");
  if (address_space_client.is_error()) {
    zxlogf(ERROR, "%s: failed to connect to FIDL goldfish-address-space fragment: %s", kTag,
           address_space_client.status_string());
    return address_space_client.status_value();
  }
  address_space_ = fidl::WireSyncClient(std::move(address_space_client.value()));

  zx::result sync_client =
      DdkConnectFragmentFidlProtocol<fuchsia_hardware_goldfish::SyncService::Device>(
          "goldfish-sync");
  if (sync_client.is_error()) {
    zxlogf(ERROR, "%s: failed to connect to FIDL goldfish-sync fragment: %s", kTag,
           sync_client.status_string());
    return sync_client.status_value();
  }
  sync_ = fidl::WireSyncClient(std::move(sync_client.value()));

  zx::result sysmem_result = DdkConnectNsProtocol<fuchsia_sysmem2::Allocator>();
  if (sysmem_result.is_error()) {
    zxlogf(ERROR, "%s: failed to connect to FIDL fuchsia.sysmem2.Allocator fragment: %s", kTag,
           sysmem_result.status_string());
    return sysmem_result.status_value();
  }
  sysmem_ = fidl::SyncClient(std::move(sysmem_result.value()));

  return ZX_OK;
}

zx_status_t Control::InitPipeDeviceLocked() {
  if (!pipe_bus_.is_valid()) {
    zxlogf(ERROR, "%s: no pipe protocol", kTag);
    return ZX_ERR_NOT_SUPPORTED;
  }

  auto get_bti_result = pipe_bus_->GetBti();
  if (!get_bti_result.ok()) {
    zxlogf(ERROR, "%s: GetBti failed: %s", kTag, get_bti_result.status_string());
    return get_bti_result.status();
  }
  bti_ = std::move(get_bti_result->value()->bti);

  zx_status_t status =
      io_buffer_.Init(bti_.get(), zx_system_get_page_size(), IO_BUFFER_RW | IO_BUFFER_CONTIG);
  if (status != ZX_OK) {
    zxlogf(ERROR, "%s: io_buffer_init failed: %d", kTag, status);
    return status;
  }

  ZX_DEBUG_ASSERT(!pipe_event_.is_valid());
  status = zx::event::create(0u, &pipe_event_);
  if (status != ZX_OK) {
    zxlogf(ERROR, "%s: zx_event_create failed: %d", kTag, status);
    return status;
  }

  zx::event pipe_event_dup;
  status = pipe_event_.duplicate(ZX_RIGHT_SAME_RIGHTS, &pipe_event_dup);
  if (status != ZX_OK) {
    zxlogf(ERROR, "%s: zx_handle_duplicate failed: %d", kTag, status);
    return status;
  }

  auto create_result = pipe_bus_->Create();
  if (!create_result.ok()) {
    zxlogf(ERROR, "%s: pipe Create failed: %s", kTag, create_result.status_string());
    return create_result.status();
  }
  id_ = create_result->value()->id;
  zx::vmo vmo = std::move(create_result->value()->vmo);

  auto set_event_result = pipe_bus_->SetEvent(id_, std::move(pipe_event_dup));
  if (!set_event_result.ok()) {
    zxlogf(ERROR, "%s: pipe SetEvent failed: %s", kTag, set_event_result.status_string());
    return set_event_result.status();
  }

  status = cmd_buffer_.InitVmo(bti_.get(), vmo.get(), 0, IO_BUFFER_RW);
  if (status != ZX_OK) {
    zxlogf(ERROR, "%s: io_buffer_init_vmo failed: %d", kTag, status);
    return status;
  }

  auto release_buffer =
      fit::defer([this]() TA_NO_THREAD_SAFETY_ANALYSIS { cmd_buffer_.release(); });

  auto buffer = static_cast<PipeCmdBuffer*>(cmd_buffer_.virt());
  buffer->id = id_;
  buffer->cmd = static_cast<int32_t>(fuchsia_hardware_goldfish_pipe::PipeCmdCode::kOpen);
  buffer->status = static_cast<int32_t>(fuchsia_hardware_goldfish_pipe::PipeError::kInval);

  auto open_result = pipe_bus_->Open(id_);
  if (!open_result.ok()) {
    zxlogf(ERROR, "%s: transport error on Open: %s", kTag, open_result.status_string());
    return open_result.status();
  }

  if (buffer->status) {
    zxlogf(ERROR, "%s: application error on Open: %d", kTag, buffer->status);
    return ZX_ERR_INTERNAL;
  }

  // Keep buffer after successful execution of OPEN command. This way
  // we'll send CLOSE later.
  release_buffer.cancel();

  size_t length = strlen(kPipeName) + 1;
  memcpy(io_buffer_.virt(), kPipeName, length);
  int32_t consumed_size = 0;
  int32_t result = WriteLocked(static_cast<uint32_t>(length), &consumed_size);
  if (result < 0) {
    zxlogf(ERROR, "%s: failed connecting to '%s' pipe: %d", kTag, kPipeName, result);
    return ZX_ERR_INTERNAL;
  }
  ZX_DEBUG_ASSERT(consumed_size == static_cast<int32_t>(length));

  memcpy(io_buffer_.virt(), &kClientFlags, sizeof(kClientFlags));
  WriteLocked(sizeof(kClientFlags));
  return ZX_OK;
}

zx_status_t Control::InitAddressSpaceDeviceLocked() {
  if (!address_space_.is_valid()) {
    zxlogf(ERROR, "%s: no address space protocol", kTag);
    return ZX_ERR_NOT_SUPPORTED;
  }

  // Initialize address space device.
  zx::result endpoints =
      fidl::CreateEndpoints<fuchsia_hardware_goldfish::AddressSpaceChildDriver>();
  if (!endpoints.is_ok()) {
    zxlogf(ERROR, "%s: FIDL endpoints failed: %s", kTag, endpoints.status_string());
    return endpoints.status_value();
  }

  auto result = address_space_->OpenChildDriver(
      fuchsia_hardware_goldfish::AddressSpaceChildDriverType::kDefault,
      std::move(endpoints->server));
  if (!result.ok()) {
    zxlogf(ERROR, "%s: AddressSpaceDevice::OpenChildDriver failed: %s", kTag,
           zx_status_get_string(result.status()));
    return result.status();
  }

  address_space_child_ = fidl::WireSyncClient<fuchsia_hardware_goldfish::AddressSpaceChildDriver>(
      std::move(endpoints->client));

  return ZX_OK;
}

zx_status_t Control::InitSyncDeviceLocked() {
  if (!sync_.is_valid()) {
    zxlogf(ERROR, "%s: no sync protocol", kTag);
    return ZX_ERR_NOT_SUPPORTED;
  }

  // Initialize sync timeline client.
  zx::result endpoints = fidl::CreateEndpoints<fuchsia_hardware_goldfish::SyncTimeline>();
  if (!endpoints.is_ok()) {
    zxlogf(ERROR, "%s: FIDL endpoints failed: %d", kTag, endpoints.status_value());
    return endpoints.status_value();
  }

  auto result = sync_->CreateTimeline(std::move(endpoints->server));
  if (!result.ok()) {
    zxlogf(ERROR, "%s: SyncDevice::CreateTimeline failed: %s", kTag, result.status_string());
    return result.status();
  }

  sync_timeline_ =
      fidl::WireSyncClient<fuchsia_hardware_goldfish::SyncTimeline>(std::move(endpoints->client));
  return ZX_OK;
}

zx_status_t Control::Bind() {
  fbl::AutoLock lock(&lock_);

  zx_status_t status = InitPipeDeviceLocked();
  if (status != ZX_OK) {
    zxlogf(ERROR, "%s: InitPipeDeviceLocked() failed: %d", kTag, status);
    return status;
  }

  status = InitAddressSpaceDeviceLocked();
  if (status != ZX_OK) {
    zxlogf(ERROR, "%s: InitAddressSpaceDeviceLocked() failed: %d", kTag, status);
    return status;
  }

  status = InitSyncDeviceLocked();
  if (status != ZX_OK) {
    zxlogf(ERROR, "%s: InitSyncDeviceLocked() failed: %d", kTag, status);
    return status;
  }

  zx::result<fidl::ClientEnd<fuchsia_hardware_sysmem::Sysmem>> connect_hardware_sysmem_result =
      DdkConnectNsProtocol<fuchsia_hardware_sysmem::Sysmem>();
  if (connect_hardware_sysmem_result.is_error()) {
    zxlogf(ERROR, "Failed to connect to fuchsia.hardware.sysmem/Sysmem protocol: %s",
           connect_hardware_sysmem_result.status_string());
    return connect_hardware_sysmem_result.status_value();
  }
  fidl::ClientEnd<fuchsia_hardware_sysmem::Sysmem> hardware_sysmem =
      std::move(connect_hardware_sysmem_result).value();

  // Serve goldfish device-local heap allocations.
  std::unique_ptr<DeviceLocalHeap> device_local_heap = DeviceLocalHeap::Create(this);
  DeviceLocalHeap* device_local_heap_ptr = device_local_heap.get();
  zx::result<> bind_device_local_heap_result = RegisterAndBindHeap(
      hardware_sysmem, fuchsia_sysmem::wire::HeapType::kGoldfishDeviceLocal, device_local_heap_ptr);
  if (bind_device_local_heap_result.is_error()) {
    zxlogf(ERROR, "Failed to bind Goldfish Device Local heap: %s",
           bind_device_local_heap_result.status_string());
    return bind_device_local_heap_result.status_value();
  }
  heaps_.push_back(std::move(device_local_heap));

  // Serve goldfish host-visible heap allocations.
  std::unique_ptr<HostVisibleHeap> host_visible_heap = HostVisibleHeap::Create(this);
  HostVisibleHeap* host_visible_heap_ptr = host_visible_heap.get();
  zx::result<> bind_host_visible_heap_result = RegisterAndBindHeap(
      hardware_sysmem, fuchsia_sysmem::wire::HeapType::kGoldfishHostVisible, host_visible_heap_ptr);
  if (bind_host_visible_heap_result.is_error()) {
    zxlogf(ERROR, "Failed to bind Goldfish Host Visible heap: %s",
           bind_host_visible_heap_result.status_string());
    return bind_host_visible_heap_result.status_value();
  }
  heaps_.push_back(std::move(host_visible_heap));

  zx::result<> add_control_service_result =
      outgoing_.AddService<fuchsia_hardware_goldfish::ControlService>(
          fuchsia_hardware_goldfish::ControlService::InstanceHandler({
              .device = bindings_.CreateHandler(this, dispatcher_, fidl::kIgnoreBindingClosure),
          }));
  if (add_control_service_result.is_error()) {
    zxlogf(ERROR, "Failed to add goldfish ControlService to the outgoing directory: %s",
           add_control_service_result.status_string());
    return add_control_service_result.status_value();
  }

  zx::result<> add_pipe_service_result =
      outgoing_.AddService<fuchsia_hardware_goldfish_pipe::Service>(
          CreateGoldfishPipeServiceInstanceHandler());
  if (add_pipe_service_result.is_error()) {
    zxlogf(ERROR, "Failed to add goldfish pipe Service to the outgoing directory: %s",
           add_pipe_service_result.status_string());
    return add_pipe_service_result.status_value();
  }

  zx::result directory_endpoints_result = fidl::CreateEndpoints<fuchsia_io::Directory>();
  if (directory_endpoints_result.is_error()) {
    zxlogf(ERROR, "Failed to create fuchsia.io/Directory endpoints: %s",
           directory_endpoints_result.status_string());
    return directory_endpoints_result.status_value();
  }
  auto [directory_client, directory_server] = std::move(directory_endpoints_result).value();

  zx::result<> serve_result = outgoing_.Serve(std::move(directory_server));
  if (serve_result.is_error()) {
    zxlogf(ERROR, "Failed to serve the outgoing directory: %s", serve_result.status_string());
    return serve_result.status_value();
  }

  const char* kOffersArray[] = {
      fuchsia_hardware_goldfish::ControlService::Name,
      fuchsia_hardware_goldfish_pipe::Service::Name,
  };

  status = DdkAdd(ddk::DeviceAddArgs("goldfish-control")
                      .set_fidl_service_offers(kOffersArray)
                      .set_outgoing_dir(directory_client.TakeChannel())
                      .set_proto_id(ZX_PROTOCOL_GOLDFISH_CONTROL));
  if (status != ZX_OK) {
    zxlogf(ERROR, "%s: DdkAdd() failed: %s", kTag, zx_status_get_string(status));
  }
  return status;
}

void Control::RegisterBufferHandle(BufferKey buffer_key) {
  fbl::AutoLock lock(&lock_);
  buffer_handles_[buffer_key] = kInvalidBufferHandle;
}

void Control::FreeBufferHandle(BufferKey buffer_key) {
  fbl::AutoLock lock(&lock_);

  auto it = buffer_handles_.find(buffer_key);
  if (it == buffer_handles_.end()) {
    zxlogf(ERROR, "%s: invalid key", kTag);
    return;
  }

  if (it->second) {
    CloseBufferOrColorBufferLocked(it->second);
  }
  buffer_handle_info_.erase(it->second);
  buffer_handles_.erase(it);
}

Control::CreateColorBuffer2Result Control::CreateColorBuffer2(
    const zx::vmo& vmo, BufferKey buffer_key,
    fuchsia_hardware_goldfish::wire::CreateColorBuffer2Params create_params) {
  using fuchsia_hardware_goldfish::ControlDevice;

  // Check argument validity.
  if (!create_params.has_width() || !create_params.has_height() || !create_params.has_format() ||
      !create_params.has_memory_property()) {
    zxlogf(ERROR, "%s: invalid arguments: width? %d height? %d format? %d memory property? %d\n",
           kTag, create_params.has_width(), create_params.has_height(), create_params.has_format(),
           create_params.has_memory_property());
    return fpromise::ok(
        fidl::WireResponse<ControlDevice::CreateColorBuffer2>(ZX_ERR_INVALID_ARGS, -1));
  }
  if ((create_params.memory_property() &
       fuchsia_hardware_goldfish::wire::kMemoryPropertyHostVisible) &&
      !create_params.has_physical_address()) {
    zxlogf(ERROR, "%s: invalid arguments: memory_property %d, no physical address\n", kTag,
           create_params.memory_property());
    return fpromise::ok(
        fidl::WireResponse<ControlDevice::CreateColorBuffer2>(ZX_ERR_INVALID_ARGS, -1));
  }

  TRACE_DURATION("gfx", "Control::CreateColorBuffer2", "width", create_params.width(), "height",
                 create_params.height(), "format", static_cast<uint32_t>(create_params.format()),
                 "memory_property", create_params.memory_property());

  fbl::AutoLock lock(&lock_);

  auto it = buffer_handles_.find(buffer_key);
  if (it == buffer_handles_.end()) {
    return fpromise::ok(
        fidl::WireResponse<ControlDevice::CreateColorBuffer2>(ZX_ERR_INVALID_ARGS, -1));
  }

  if (it->second != kInvalidBufferHandle) {
    return fpromise::ok(
        fidl::WireResponse<ControlDevice::CreateColorBuffer2>(ZX_ERR_ALREADY_EXISTS, -1));
  }

  uint32_t id;
  zx_status_t status = CreateColorBufferLocked(create_params.width(), create_params.height(),
                                               static_cast<uint32_t>(create_params.format()), &id);
  if (status != ZX_OK) {
    zxlogf(ERROR, "%s: failed to create color buffer: %d", kTag, status);
    return fpromise::error(status);
  }

  auto close_color_buffer =
      fit::defer([this, id]() TA_NO_THREAD_SAFETY_ANALYSIS { CloseColorBufferLocked(id); });

  uint32_t result = 0;
  status =
      SetColorBufferVulkanMode2Locked(id, VULKAN_ONLY, create_params.memory_property(), &result);
  if (status != ZX_OK || result) {
    zxlogf(ERROR, "%s: failed to set vulkan mode: %d %d", kTag, status, result);
    return fpromise::error(status);
  }

  int32_t hw_address_page_offset = -1;
  if (create_params.memory_property() &
      fuchsia_hardware_goldfish::wire::kMemoryPropertyHostVisible) {
    uint64_t vmo_size;
    status = vmo.get_size(&vmo_size);
    if (status != ZX_OK) {
      zxlogf(ERROR, "%s: zx_vmo_get_size error: %d", kTag, status);
      return fpromise::error(status);
    }
    uint32_t map_result = 0;
    status =
        MapGpaToBufferHandleLocked(id, create_params.physical_address(), vmo_size, &map_result);
    if (status != ZX_OK || map_result < 0) {
      zxlogf(ERROR, "%s: failed to map gpa to color buffer: %d %d", kTag, status, map_result);
      return fpromise::error(status);
    }

    hw_address_page_offset = map_result;
  }

  close_color_buffer.cancel();
  it->second = id;
  buffer_handle_info_[id] = {
      .type = fuchsia_hardware_goldfish::wire::BufferHandleType::kColorBuffer,
      .memory_property = create_params.memory_property()};

  return fpromise::ok(
      fidl::WireResponse<ControlDevice::CreateColorBuffer2>(ZX_OK, hw_address_page_offset));
}

void Control::CreateColorBuffer2(CreateColorBuffer2RequestView request,
                                 CreateColorBuffer2Completer::Sync& completer) {
  auto buffer_key_result = GetBufferKeyForVmo(request->vmo);
  if (!buffer_key_result.is_ok()) {
    zxlogf(ERROR, "%s: GetBufferKeyForVmo failed: %d", kTag, buffer_key_result.error_value());
    if (buffer_key_result.error_value() == ZX_ERR_NOT_FOUND) {
      completer.Reply(ZX_ERR_INVALID_ARGS, 0);
      return;
    }
    completer.Close(ZX_ERR_INVALID_ARGS);
    return;
  }
  auto& buffer_key = buffer_key_result.value();

  auto result =
      CreateColorBuffer2(std::move(request->vmo), buffer_key, std::move(request->create_params));
  if (result.is_ok()) {
    completer.Reply(result.value().res, result.value().hw_address_page_offset);
  } else {
    completer.Close(result.error());
  }
}

void Control::CreateBuffer2(CreateBuffer2RequestView request,
                            CreateBuffer2Completer::Sync& completer) {
  // TODO(https://fxbug.dev/406545904): Support CreateBuffer2() render commands.
  zxlogf(ERROR, "CreateBuffer2 render control command is not supported on Fuchsia.");
  completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
}

void Control::CreateSyncFence(CreateSyncFenceRequestView request,
                              CreateSyncFenceCompleter::Sync& completer) {
  fbl::AutoLock lock(&lock_);
  uint64_t glsync = 0;
  uint64_t syncthread = 0;
  zx_status_t status = CreateSyncKHRLocked(&glsync, &syncthread);
  if (status != ZX_OK) {
    zxlogf(ERROR, "CreateSyncFence: cannot call rcCreateSyncKHR, status=%d", status);
    completer.ReplyError(ZX_ERR_INTERNAL);
    return;
  }

  auto result = sync_timeline_->TriggerHostWait(glsync, syncthread, std::move(request->event));
  if (!result.ok()) {
    zxlogf(ERROR, "TriggerHostWait: FIDL call failed, status=%d", result.status());
    completer.ReplyError(ZX_ERR_INTERNAL);
    return;
  }

  completer.ReplySuccess();
}

void Control::GetBufferHandle(GetBufferHandleRequestView request,
                              GetBufferHandleCompleter::Sync& completer) {
  TRACE_DURATION("gfx", "Control::FidlGetBufferHandle");

  uint32_t handle = kInvalidBufferHandle;
  auto handle_type = fuchsia_hardware_goldfish::wire::BufferHandleType::kInvalid;

  auto buffer_key_result = GetBufferKeyForVmo(request->vmo);
  if (!buffer_key_result.is_ok()) {
    if (buffer_key_result.error_value() == ZX_ERR_NOT_FOUND) {
      completer.Reply(ZX_ERR_INVALID_ARGS, handle, handle_type);
      return;
    }
    completer.Close(ZX_ERR_INVALID_ARGS);
    return;
  }
  auto& buffer_key = buffer_key_result.value();

  fbl::AutoLock lock(&lock_);

  auto it = buffer_handles_.find(buffer_key);
  if (it == buffer_handles_.end()) {
    completer.Reply(ZX_ERR_INVALID_ARGS, handle, handle_type);
    return;
  }

  handle = it->second;
  if (handle == kInvalidBufferHandle) {
    // Color buffer not created yet.
    completer.Reply(ZX_ERR_NOT_FOUND, handle, handle_type);
    return;
  }

  auto it_types = buffer_handle_info_.find(handle);
  if (it_types == buffer_handle_info_.end()) {
    // Color buffer type not registered yet.
    completer.Reply(ZX_ERR_NOT_FOUND, handle, handle_type);
    return;
  }

  handle_type = it_types->second.type;
  completer.Reply(ZX_OK, handle, handle_type);
}

void Control::GetBufferHandleInfo(GetBufferHandleInfoRequestView request,
                                  GetBufferHandleInfoCompleter::Sync& completer) {
  using fuchsia_hardware_goldfish::wire::BufferHandleType;
  using fuchsia_hardware_goldfish::wire::ControlDeviceGetBufferHandleInfoResponse;
  using fuchsia_hardware_goldfish::wire::ControlDeviceGetBufferHandleInfoResult;

  TRACE_DURATION("gfx", "Control::FidlGetBufferHandleInfo");

  auto buffer_key_result = GetBufferKeyForVmo(request->vmo);
  if (!buffer_key_result.is_ok()) {
    if (buffer_key_result.error_value() == ZX_ERR_NOT_FOUND) {
      completer.ReplyError(ZX_ERR_INVALID_ARGS);
      return;
    }
    completer.Close(ZX_ERR_INVALID_ARGS);
    return;
  }
  auto& buffer_key = buffer_key_result.value();

  uint32_t handle = kInvalidBufferHandle;
  fbl::AutoLock lock(&lock_);

  auto it = buffer_handles_.find(buffer_key);
  if (it == buffer_handles_.end()) {
    completer.ReplyError(ZX_ERR_INVALID_ARGS);
    return;
  }

  handle = it->second;
  if (handle == kInvalidBufferHandle) {
    // Color buffer not created yet.
    completer.ReplyError(ZX_ERR_NOT_FOUND);
    return;
  }

  auto it_types = buffer_handle_info_.find(handle);
  if (it_types == buffer_handle_info_.end()) {
    // Color buffer type not registered yet.
    completer.ReplyError(ZX_ERR_NOT_FOUND);
    return;
  }

  fidl::Arena allocator;

  ControlDeviceGetBufferHandleInfoResponse response;
  response.info.Allocate(allocator);
  response.info.set_id(handle)
      .set_memory_property(it_types->second.memory_property)
      .set_type(it_types->second.type);
  completer.Reply(::fit::ok(&response));
}

void Control::DdkRelease() { delete this; }

int32_t Control::WriteLocked(uint32_t cmd_size, int32_t* consumed_size) {
  TRACE_DURATION("gfx", "Control::Write", "cmd_size", cmd_size);

  auto buffer = static_cast<PipeCmdBuffer*>(cmd_buffer_.virt());
  buffer->id = id_;
  buffer->cmd = static_cast<int32_t>(fuchsia_hardware_goldfish_pipe::PipeCmdCode::kWrite);
  buffer->status = static_cast<int32_t>(fuchsia_hardware_goldfish_pipe::PipeError::kInval);
  buffer->rw_params.ptrs[0] = io_buffer_.phys();
  buffer->rw_params.sizes[0] = cmd_size;
  buffer->rw_params.buffers_count = 1;
  buffer->rw_params.consumed_size = 0;
  auto result = pipe_bus_->Exec(id_);
  if (!result.ok()) {
    zxlogf(ERROR, "%s: Exec pipe failed: %s", kTag, result.status_string());
    return result.status();
  }
  ZX_DEBUG_ASSERT(result.ok());
  *consumed_size = buffer->rw_params.consumed_size;
  return buffer->status;
}

void Control::WriteLocked(uint32_t cmd_size) {
  int32_t consumed_size;
  int32_t result = WriteLocked(cmd_size, &consumed_size);
  ZX_DEBUG_ASSERT(result >= 0);
  ZX_DEBUG_ASSERT(consumed_size == static_cast<int32_t>(cmd_size));
}

zx_status_t Control::ReadResultLocked(void* result, size_t size) {
  TRACE_DURATION("gfx", "Control::ReadResult");

  while (true) {
    auto buffer = static_cast<PipeCmdBuffer*>(cmd_buffer_.virt());
    buffer->id = id_;
    buffer->cmd = static_cast<int32_t>(fuchsia_hardware_goldfish_pipe::PipeCmdCode::kRead);
    buffer->status = static_cast<int32_t>(fuchsia_hardware_goldfish_pipe::PipeError::kInval);
    buffer->rw_params.ptrs[0] = io_buffer_.phys();
    buffer->rw_params.sizes[0] = static_cast<uint32_t>(size);
    buffer->rw_params.buffers_count = 1;
    buffer->rw_params.consumed_size = 0;
    auto exec_result = pipe_bus_->Exec(id_);
    if (!exec_result.ok()) {
      zxlogf(ERROR, "%s: Exec pipe failed: %s", kTag, exec_result.status_string());
      return exec_result.status();
    }

    // Positive consumed size always indicate a successful transfer.
    if (buffer->rw_params.consumed_size) {
      ZX_DEBUG_ASSERT(buffer->rw_params.consumed_size == static_cast<int32_t>(size));
      memcpy(result, io_buffer_.virt(), size);
      return ZX_OK;
    }

    // Early out if error is not because of back-pressure.
    if (buffer->status != static_cast<int32_t>(fuchsia_hardware_goldfish_pipe::PipeError::kAgain)) {
      zxlogf(ERROR, "%s: reading result failed: %d", kTag, buffer->status);
      return ZX_ERR_INTERNAL;
    }

    buffer->id = id_;
    buffer->cmd = static_cast<int32_t>(fuchsia_hardware_goldfish_pipe::PipeCmdCode::kWakeOnRead);
    buffer->status = static_cast<int32_t>(fuchsia_hardware_goldfish_pipe::PipeError::kInval);
    auto exec_result2 = pipe_bus_->Exec(id_);
    if (!exec_result2.ok()) {
      zxlogf(ERROR, "%s: Exec pipe failed: %s", kTag, exec_result2.status_string());
      return exec_result2.status();
    }
    ZX_DEBUG_ASSERT(!buffer->status);

    // Wait for pipe to become readable.
    zx_status_t status = pipe_event_.wait_one(fuchsia_hardware_goldfish::wire::kSignalHangup |
                                                  fuchsia_hardware_goldfish::wire::kSignalReadable,
                                              zx::time::infinite(), nullptr);
    if (status != ZX_OK) {
      if (status != ZX_ERR_CANCELED) {
        zxlogf(ERROR, "%s: zx_object_wait_one failed: %d", kTag, status);
      }
      return status;
    }
  }
}

zx_status_t Control::ExecuteCommandLocked(uint32_t cmd_size, uint32_t* result) {
  TRACE_DURATION("gfx", "Control::ExecuteCommand", "cnd_size", cmd_size);

  WriteLocked(cmd_size);
  return ReadResultLocked(result);
}

zx_status_t Control::CreateColorBufferLocked(uint32_t width, uint32_t height, uint32_t format,
                                             uint32_t* id) {
  TRACE_DURATION("gfx", "Control::CreateColorBuffer", "width", width, "height", height);

  auto cmd = static_cast<CreateColorBufferCmd*>(io_buffer_.virt());
  cmd->op = kOP_rcCreateColorBuffer;
  cmd->size = kSize_rcCreateColorBuffer;
  cmd->width = width;
  cmd->height = height;
  cmd->internalformat = format;

  return ExecuteCommandLocked(kSize_rcCreateColorBuffer, id);
}

void Control::CloseBufferOrColorBufferLocked(uint32_t id) {
  ZX_DEBUG_ASSERT(buffer_handle_info_.find(id) != buffer_handle_info_.end());
  auto buffer_type = buffer_handle_info_.at(id).type;
  switch (buffer_type) {
    case fuchsia_hardware_goldfish::wire::BufferHandleType::kBuffer:
      CloseBufferLocked(id);
      break;
    case fuchsia_hardware_goldfish::wire::BufferHandleType::kColorBuffer:
      CloseColorBufferLocked(id);
      break;
    default:
      // Otherwise buffer/colorBuffer was not created. We don't need to do
      // anything.
      break;
  }
}

void Control::CloseColorBufferLocked(uint32_t id) {
  TRACE_DURATION("gfx", "Control::CloseColorBuffer", "id", id);

  auto cmd = static_cast<CloseColorBufferCmd*>(io_buffer_.virt());
  cmd->op = kOP_rcCloseColorBuffer;
  cmd->size = kSize_rcCloseColorBuffer;
  cmd->id = id;

  WriteLocked(kSize_rcCloseColorBuffer);
}

void Control::CloseBufferLocked(uint32_t id) {
  TRACE_DURATION("gfx", "Control::CloseBuffer", "id", id);

  auto cmd = static_cast<CloseBufferCmd*>(io_buffer_.virt());
  cmd->op = kOP_rcCloseBuffer;
  cmd->size = kSize_rcCloseBuffer;
  cmd->id = id;

  WriteLocked(kSize_rcCloseBuffer);
}

zx_status_t Control::SetColorBufferVulkanMode2Locked(uint32_t id, uint32_t mode,
                                                     uint32_t memory_property, uint32_t* result) {
  TRACE_DURATION("gfx", "Control::SetColorBufferVulkanMode2Locked", "id", id, "mode", mode,
                 "memory_property", memory_property);

  auto cmd = static_cast<SetColorBufferVulkanMode2Cmd*>(io_buffer_.virt());
  cmd->op = kOP_rcSetColorBufferVulkanMode2;
  cmd->size = kSize_rcSetColorBufferVulkanMode2;
  cmd->id = id;
  cmd->mode = mode;
  cmd->memory_property = memory_property;

  return ExecuteCommandLocked(kSize_rcSetColorBufferVulkanMode2, result);
}

zx_status_t Control::MapGpaToBufferHandleLocked(uint32_t id, uint64_t gpa, uint64_t size,
                                                uint32_t* result) {
  TRACE_DURATION("gfx", "Control::MapGpaToBufferHandleLocked", "id", id, "gpa", gpa, "size", size);

  auto cmd = static_cast<MapGpaToBufferHandle2Cmd*>(io_buffer_.virt());
  cmd->op = kOP_rcMapGpaToBufferHandle2;
  cmd->size = kSize_rcMapGpaToBufferHandle2;
  cmd->id = id;
  cmd->gpa = gpa;
  cmd->map_size = size;

  return ExecuteCommandLocked(kSize_rcMapGpaToBufferHandle2, result);
}

zx_status_t Control::CreateSyncKHRLocked(uint64_t* glsync_out, uint64_t* syncthread_out) {
  TRACE_DURATION("gfx", "Control::CreateSyncKHRLocked");

  constexpr size_t kAttribSize = 2u;

  struct {
    CreateSyncKHRCmdHeader header;
    int32_t attribs[kAttribSize];
    CreateSyncKHRCmdFooter footer;
  } cmd = {
      .header =
          {
              .op = kOP_rcCreateSyncKHR,
              .size = kSize_rcCreateSyncKHRCmd + kAttribSize * sizeof(int32_t),
              .type = EGL_SYNC_NATIVE_FENCE_ANDROID,
              .attribs_size = kAttribSize * sizeof(int32_t),
          },
      .attribs =
          {
              EGL_SYNC_NATIVE_FENCE_FD_ANDROID,
              EGL_NO_NATIVE_FENCE_FD_ANDROID,
          },
      .footer =
          {
              .attribs_size = kAttribSize * sizeof(int32_t),
              .destroy_when_signaled = 1,
              .size_glsync_out = kSize_GlSyncOut,
              .size_syncthread_out = kSize_SyncThreadOut,
          },
  };

  auto cmd_buffer = static_cast<uint8_t*>(io_buffer_.virt());
  memcpy(cmd_buffer, &cmd, sizeof(cmd));

  WriteLocked(static_cast<uint32_t>(sizeof(cmd)));

  struct {
    uint64_t glsync;
    uint64_t syncthread;
  } result;
  zx_status_t status = ReadResultLocked(&result, kSize_GlSyncOut + kSize_SyncThreadOut);
  if (status != ZX_OK) {
    return status;
  }
  *glsync_out = result.glsync;
  *syncthread_out = result.syncthread;
  return ZX_OK;
}

void Control::RemoveHeap(Heap* heap) {
  fbl::AutoLock lock(&lock_);
  // The async loop of heap is still running when calling this method, so that
  // we cannot remove it directly from |heaps_| (otherwise async loop needs to
  // wait for this to end before shutting down the loop, causing an infinite
  // loop), instead we move it into a staging area for future deletion.
  removed_heaps_.push_back(heaps_.erase(*heap));
}

fit::result<zx_status_t, BufferKey> Control::GetBufferKeyForVmo(const zx::vmo& vmo) {
  ZX_ASSERT(sysmem_.is_valid());
  zx::vmo dup_vmo;
  zx_status_t dup_status = vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &dup_vmo);
  if (dup_status != ZX_OK) {
    zxlogf(ERROR, "%s: vmo.duplicate() failed: %d", kTag, dup_status);
    return fit::error(dup_status);
  }
  fuchsia_sysmem2::AllocatorGetVmoInfoRequest request;
  request.vmo() = std::move(dup_vmo);
  auto get_result = sysmem_->GetVmoInfo(std::move(request));
  if (!get_result.is_ok()) {
    if (get_result.error_value().is_domain_error() &&
        get_result.error_value().domain_error() == fuchsia_sysmem2::Error::kNotFound) {
      return fit::error(ZX_ERR_NOT_FOUND);
    }
    zx_status_t get_vmo_info_status =
        get_result.error_value().is_domain_error()
            ? sysmem::V1CopyFromV2Error(get_result.error_value().domain_error())
            : get_result.error_value().framework_error().status();
    zxlogf(ERROR, "%s: GetVmoInfo failed: %d", kTag, get_vmo_info_status);
    return fit::error(get_vmo_info_status);
  }
  auto& info = get_result.value();
  return fit::ok(BufferKey{
      *info.buffer_collection_id(),
      *info.buffer_index(),
  });
}

fuchsia_hardware_goldfish_pipe::Service::InstanceHandler
Control::CreateGoldfishPipeServiceInstanceHandler() {
  return fuchsia_hardware_goldfish_pipe::Service::InstanceHandler{{
      .device =
          [parent = parent()](fidl::ServerEnd<fuchsia_hardware_goldfish_pipe::Bus> server_end) {
            static constexpr const char kGoldfishPipeFragmentName[] = "goldfish-pipe";
            zx_status_t status = device_connect_fragment_fidl_protocol(
                parent, kGoldfishPipeFragmentName, fuchsia_hardware_goldfish_pipe::Service::Name,
                fuchsia_hardware_goldfish_pipe::Service::Device::Name,
                server_end.TakeChannel().release());
            if (status != ZX_OK) {
              zxlogf(ERROR, "Failed to connect to the Bus protocol: %s",
                     zx_status_get_string(status));
            }
          },
  }};
}
}  // namespace goldfish

static constexpr zx_driver_ops_t goldfish_control_driver_ops = []() -> zx_driver_ops_t {
  zx_driver_ops_t ops = {};
  ops.version = DRIVER_OPS_VERSION;
  ops.bind = goldfish::Control::Create;
  return ops;
}();

ZIRCON_DRIVER(goldfish_control_composite, goldfish_control_driver_ops, "zircon", "0.1");

// clang-format on
