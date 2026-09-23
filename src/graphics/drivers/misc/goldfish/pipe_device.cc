// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/drivers/misc/goldfish/pipe_device.h"

#include <fidl/fuchsia.hardware.acpi/cpp/wire.h>
#include <fidl/fuchsia.hardware.goldfish/cpp/wire.h>
#include <lib/async/cpp/task.h>
#include <lib/dma-buffer/buffer.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/trace/event.h>
#include <lib/zx/bti.h>
#include <lib/zx/event.h>
#include <lib/zx/pmt.h>
#include <zircon/assert.h>
#include <zircon/status.h>
#include <zircon/threads.h>

#include <cstdint>

#include <fbl/auto_lock.h>

#include "src/devices/lib/goldfish/pipe_headers/include/base.h"
#include "src/graphics/drivers/misc/goldfish/pipe_connection.h"

namespace goldfish {
namespace {

constexpr uint32_t PIPE_DRIVER_VERSION = 4;
constexpr uint32_t PIPE_MIN_DEVICE_VERSION = 2;
constexpr uint32_t MAX_SIGNALLED_PIPES = 64;

enum PipeV2Regs {
  PIPE_V2_REG_CMD = 0,
  PIPE_V2_REG_SIGNAL_BUFFER_HIGH = 4,
  PIPE_V2_REG_SIGNAL_BUFFER = 8,
  PIPE_V2_REG_SIGNAL_BUFFER_COUNT = 12,
  PIPE_V2_REG_OPEN_BUFFER_HIGH = 20,
  PIPE_V2_REG_OPEN_BUFFER = 24,
  PIPE_V2_REG_VERSION = 36,
  PIPE_V2_REG_GET_SIGNALLED = 48,
};

// Parameters for the PIPE_CMD_OPEN command.
struct OpenCommandBuffer {
  uint64_t pa_command_buffer;
  uint32_t rw_params_max_count;
};

// Information for a single signalled pipe.
struct SignalBuffer {
  uint32_t id;
  uint32_t flags;
};

// Device-level set of buffers shared with the host.
struct CommandBuffers {
  OpenCommandBuffer open_command_buffer;
  SignalBuffer signal_buffers[MAX_SIGNALLED_PIPES];
};

uint32_t upper_32_bits(uint64_t n) { return static_cast<uint32_t>(n >> 32); }

uint32_t lower_32_bits(uint64_t n) { return static_cast<uint32_t>(n); }

}  // namespace

PipeDevice::PipeDevice(fidl::ClientEnd<fuchsia_hardware_acpi::Device> acpi,
                       fdf::UnownedSynchronizedDispatcher dispatcher)
    : acpi_(std::move(acpi)), dispatcher_(std::move(dispatcher)) {
  ZX_DEBUG_ASSERT(acpi_.is_valid());
  ZX_DEBUG_ASSERT(dispatcher_->get() != nullptr);
}

PipeDevice::~PipeDevice() = default;

zx::result<> PipeDevice::Initialize() {
  fidl::WireResult<fuchsia_hardware_acpi::Device::GetBti> bti_result = acpi_->GetBti(0);
  if (!bti_result.ok()) {
    fdf::error("GetBti FIDL transport failed: {}", bti_result.status_string());
    return zx::error(bti_result.status());
  }
  if (bti_result->is_error()) {
    zx_status_t status = bti_result->error_value();
    fdf::error("GetBti failed: {}", zx_status_get_string(status));
    return zx::error(status);
  }
  bti_ = std::move(bti_result->value()->bti);

  fidl::WireResult<fuchsia_hardware_acpi::Device::GetMmio> mmio_result = acpi_->GetMmio(0);
  if (!mmio_result.ok()) {
    fdf::error("GetMmio FIDL transport failed: {}", mmio_result.status_string());
    return zx::error(mmio_result.status());
  }
  if (mmio_result->is_error()) {
    zx_status_t status = mmio_result->error_value();
    fdf::error("GetMmio failed: {}", zx_status_get_string(status));
    return zx::error(status);
  }

  fbl::AutoLock lock(&mmio_lock_);
  auto& mmio = mmio_result->value()->mmio;
  zx::result<fdf::MmioBuffer> result = fdf::MmioBuffer::Create(
      mmio.offset, mmio.size, std::move(mmio.vmo), ZX_CACHE_POLICY_UNCACHED_DEVICE);
  if (result.is_error()) {
    fdf::error("mmiobuffer create failed: {}", result);
    return zx::error(result.status_value());
  }
  mmio_ = std::move(result.value());

  // Check device version.
  mmio_->Write32(PIPE_DRIVER_VERSION, PIPE_V2_REG_VERSION);
  uint32_t version = mmio_->Read32(PIPE_V2_REG_VERSION);
  if (version < PIPE_MIN_DEVICE_VERSION) {
    fdf::error("insufficient device version: {}", version);
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  fidl::WireResult<::fuchsia_hardware_acpi::Device::MapInterrupt> irq_result =
      acpi_->MapInterrupt(0);
  if (!irq_result.ok()) {
    fdf::error("MapInterrupt FIDL call failed: {}", irq_result.status_string());
    return zx::error(irq_result.status());
  }
  if (irq_result->is_error()) {
    zx_status_t status = irq_result->error_value();
    fdf::error("MapInterrupt failed: {}", zx_status_get_string(status));
    return zx::error(status);
  }
  irq_ = std::move(irq_result->value()->irq);

  int rc = thrd_create_with_name(
      &irq_thread_, [](void* arg) { return static_cast<PipeDevice*>(arg)->IrqHandler(); }, this,
      "goldfish_pipe_irq_thread");
  if (rc != thrd_success) {
    irq_.destroy();
    return zx::error(thrd_status_to_zx_status(rc));
  }

  std::unique_ptr<dma_buffer::BufferFactory> buffer_factory = dma_buffer::CreateBufferFactory();

  const size_t page_size = zx_system_get_page_size();
  ZX_DEBUG_ASSERT_MSG(sizeof(CommandBuffers) <= page_size, "cmds size");
  zx_status_t status =
      buffer_factory->CreateContiguous(bti_, /*size=*/page_size, /*alignment_log2=*/0,
                                       dma_buffer::CacheOptions::kEnabled, &io_buffer_);
  if (status != ZX_OK) {
    fdf::error("Failed to create contiguous IO buffer: {}", zx_status_get_string(status));
    return zx::error(status);
  }

  // Register the buffer addresses with the device.
  zx_paddr_t pa_signal_buffers = io_buffer_->phys() + offsetof(CommandBuffers, signal_buffers);
  mmio_->Write32(upper_32_bits(pa_signal_buffers), PIPE_V2_REG_SIGNAL_BUFFER_HIGH);
  mmio_->Write32(lower_32_bits(pa_signal_buffers), PIPE_V2_REG_SIGNAL_BUFFER);
  mmio_->Write32(MAX_SIGNALLED_PIPES, PIPE_V2_REG_SIGNAL_BUFFER_COUNT);
  zx_paddr_t pa_open_command_buffer =
      io_buffer_->phys() + offsetof(CommandBuffers, open_command_buffer);
  mmio_->Write32(upper_32_bits(pa_open_command_buffer), PIPE_V2_REG_OPEN_BUFFER_HIGH);
  mmio_->Write32(lower_32_bits(pa_open_command_buffer), PIPE_V2_REG_OPEN_BUFFER);

  return zx::ok();
}

zx::result<> PipeDevice::PrepareStop() {
  if (irq_.is_valid()) {
    irq_.destroy();
    thrd_join(irq_thread_, nullptr);
  }
  return zx::ok();
}

void PipeDevice::Connect(ConnectRequestView request, ConnectCompleter::Sync& completer) {
  ZX_DEBUG_ASSERT(request->pipe_request.is_valid());

  async::PostTask(
      dispatcher_->async_dispatcher(),
      [this, pipe_request = std::move(request->pipe_request)]() mutable {
        auto pipe = std::make_unique<PipeConnection>(
            this, dispatcher_->async_dispatcher(), /* OnBind */ nullptr,
            /* OnClose */ [this](PipeConnection* pipe_ptr) {
              // We know |pipe_ptr| is still alive because |pipe_ptr|
              // is still in |pipes_|.
              ZX_DEBUG_ASSERT(pipe_connections_.find(pipe_ptr) != pipe_connections_.end());
              pipe_connections_.erase(pipe_ptr);
            });

        PipeConnection* pipe_ptr = pipe.get();
        pipe_connections_.insert({pipe_ptr, std::move(pipe)});

        pipe_ptr->Bind(std::move(pipe_request));
        // Init() must be called after Bind() as it can cause an asynchronous
        // failure. The pipe will be cleaned up later by the error handler in
        // the event of a failure.
        pipe_ptr->Init();
      });
}

zx_status_t PipeDevice::Create(int32_t* out_id, zx::vmo* out_vmo) {
  TRACE_DURATION("gfx", "PipeDevice::Create");

  const size_t page_size = zx_system_get_page_size();
  ZX_DEBUG_ASSERT_MSG(sizeof(PipeCmdBuffer) <= page_size, "cmd size");
  zx::vmo vmo;
  zx_status_t status = zx::vmo::create(page_size, 0, &vmo);
  if (status != ZX_OK) {
    return status;
  }

  zx_paddr_t paddr;
  zx::pmt pmt;
  status = bti_.pin(ZX_BTI_PERM_READ | ZX_BTI_PERM_WRITE, vmo, 0, page_size, &paddr, 1, &pmt);
  if (status != ZX_OK) {
    return status;
  }

  fbl::AutoLock lock(&pipes_lock_);
  int32_t id = next_pipe_id_++;
  ZX_DEBUG_ASSERT(command_storages_.count(id) == 0);
  command_storages_[id] = std::make_unique<CommandStorage>(paddr, std::move(pmt), zx::event());

  *out_vmo = std::move(vmo);
  *out_id = id;
  return ZX_OK;
}

zx_status_t PipeDevice::SetEvent(int32_t id, zx::event pipe_event) {
  TRACE_DURATION("gfx", "PipeDevice::SetEvent");

  fbl::AutoLock lock(&pipes_lock_);

  ZX_DEBUG_ASSERT(command_storages_.count(id) == 1);
  ZX_DEBUG_ASSERT(pipe_event.is_valid());

  zx_signals_t kSignals = fuchsia_hardware_goldfish::wire::kSignalReadable |
                          fuchsia_hardware_goldfish::wire::kSignalWritable;

  zx_signals_t observed = 0u;
  // If old pipe event exists, transfer observed signal to new pipe event.
  if (command_storages_[id]->pipe_event.is_valid()) {
    zx_status_t status =
        command_storages_[id]->pipe_event.wait_one(kSignals, zx::time::infinite_past(), &observed);
    if (status != ZX_OK) {
      fdf::error("failed to transfer observed signals: {}", status);
      return status;
    }
  }

  command_storages_[id]->pipe_event = std::move(pipe_event);
  zx_status_t status = command_storages_[id]->pipe_event.signal(kSignals, observed & kSignals);
  if (status != ZX_OK) {
    fdf::error("failed to signal event: {}", status);
    return status;
  }
  return ZX_OK;
}

void PipeDevice::Destroy(int32_t id) {
  TRACE_DURATION("gfx", "PipeDevice::Destroy");

  fbl::AutoLock lock(&pipes_lock_);
  ZX_DEBUG_ASSERT(command_storages_.count(id) == 1);
  command_storages_.erase(id);
}

void PipeDevice::Open(int32_t id) {
  TRACE_DURATION("gfx", "PipeDevice::Open");

  zx_paddr_t paddr;
  {
    fbl::AutoLock lock(&pipes_lock_);
    ZX_DEBUG_ASSERT(command_storages_.count(id) == 1);
    paddr = command_storages_[id]->paddr;
  }

  fbl::AutoLock lock(&mmio_lock_);
  CommandBuffers* buffers = static_cast<CommandBuffers*>(io_buffer_->virt());
  buffers->open_command_buffer.pa_command_buffer = paddr;
  buffers->open_command_buffer.rw_params_max_count = MAX_BUFFERS_PER_COMMAND;
  mmio_->Write32(id, PIPE_V2_REG_CMD);
}

void PipeDevice::Exec(int32_t id) {
  TRACE_DURATION("gfx", "PipeDevice::Exec", "id", id);

  fbl::AutoLock lock(&mmio_lock_);
  mmio_->Write32(id, PIPE_V2_REG_CMD);
}

zx_status_t PipeDevice::GetBti(zx::bti* out_bti) {
  TRACE_DURATION("gfx", "PipeDevice::GetBti");

  return bti_.duplicate(ZX_RIGHT_SAME_RIGHTS, out_bti);
}

void PipeDevice::Create(CreateCompleter::Sync& completer) {
  int32_t id;
  zx::vmo vmo;
  zx_status_t status = Create(&id, &vmo);
  if (status == ZX_OK) {
    completer.ReplySuccess(id, std::move(vmo));
  } else {
    completer.Close(status);
  }
}

void PipeDevice::SetEvent(SetEventRequestView request, SetEventCompleter::Sync& completer) {
  zx_status_t status = SetEvent(request->id, std::move(request->pipe_event));
  if (status == ZX_OK) {
    completer.ReplySuccess();
  } else {
    completer.Close(status);
  }
}

void PipeDevice::Destroy(DestroyRequestView request, DestroyCompleter::Sync& completer) {
  Destroy(request->id);
  completer.Reply();
}

void PipeDevice::Open(OpenRequestView request, OpenCompleter::Sync& completer) {
  Open(request->id);
  completer.Reply();
}

void PipeDevice::Exec(ExecRequestView request, ExecCompleter::Sync& completer) {
  Exec(request->id);
  completer.Reply();
}

void PipeDevice::GetBti(GetBtiCompleter::Sync& completer) {
  zx::bti bti;
  zx_status_t status = GetBti(&bti);
  if (status == ZX_OK) {
    completer.ReplySuccess(std::move(bti));
  } else {
    completer.Close(status);
  }
}

int PipeDevice::IrqHandler() {
  while (true) {
    zx_status_t status = irq_.wait(nullptr);
    if (status != ZX_OK) {
      fdf::error("irq.wait() got {}", status);
      break;
    }

    uint32_t count;
    {
      fbl::AutoLock lock(&mmio_lock_);
      count = mmio_->Read32(PIPE_V2_REG_GET_SIGNALLED);
    }
    if (count > MAX_SIGNALLED_PIPES) {
      count = MAX_SIGNALLED_PIPES;
    }
    if (count) {
      TRACE_DURATION("gfx", "PipeDevice::IrqHandler::Signal", "count", count);

      fbl::AutoLock lock(&pipes_lock_);

      auto buffers = static_cast<CommandBuffers*>(io_buffer_->virt());
      for (uint32_t i = 0; i < count; ++i) {
        auto it = command_storages_.find(buffers->signal_buffers[i].id);
        if (it != command_storages_.end()) {
          it->second->SignalEvent(buffers->signal_buffers[i].flags);
        }
      }
    }
  }

  return 0;
}

PipeDevice::CommandStorage::CommandStorage(zx_paddr_t paddr, zx::pmt pmt, zx::event pipe_event)
    : paddr(paddr), pmt(std::move(pmt)), pipe_event(std::move(pipe_event)) {}

PipeDevice::CommandStorage::~CommandStorage() {
  ZX_DEBUG_ASSERT(pmt.is_valid());
  pmt.unpin();
}

void PipeDevice::CommandStorage::SignalEvent(uint32_t flags) const {
  if (!pipe_event.is_valid()) {
    return;
  }

  zx_signals_t state_set = 0;
  if (flags & static_cast<int32_t>(fuchsia_hardware_goldfish_pipe::PipeWakeFlag::kClosed)) {
    state_set |= fuchsia_hardware_goldfish::wire::kSignalHangup;
  }
  if (flags & static_cast<int32_t>(fuchsia_hardware_goldfish_pipe::PipeWakeFlag::kRead)) {
    state_set |= fuchsia_hardware_goldfish::wire::kSignalReadable;
  }
  if (flags & static_cast<int32_t>(fuchsia_hardware_goldfish_pipe::PipeWakeFlag::kWrite)) {
    state_set |= fuchsia_hardware_goldfish::wire::kSignalWritable;
  }

  zx_status_t status = pipe_event.signal(/*clear_mask=*/0u, state_set);
  if (status != ZX_OK) {
    fdf::error("zx_signal_object failed: {}", status);
  }
}

}  // namespace goldfish
