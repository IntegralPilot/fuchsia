// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DRIVERS_MISC_GOLDFISH_PIPE_DEVICE_H_
#define SRC_GRAPHICS_DRIVERS_MISC_GOLDFISH_PIPE_DEVICE_H_

#include <fidl/fuchsia.hardware.acpi/cpp/wire.h>
#include <fidl/fuchsia.hardware.goldfish.pipe/cpp/wire.h>
#include <fidl/fuchsia.hardware.goldfish/cpp/wire.h>
#include <lib/dma-buffer/buffer.h>
#include <lib/driver/mmio/cpp/mmio-buffer.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/zx/bti.h>
#include <lib/zx/event.h>
#include <lib/zx/interrupt.h>
#include <threads.h>
#include <zircon/compiler.h>
#include <zircon/types.h>

#include <cstdint>
#include <memory>
#include <optional>
#include <unordered_map>

#include <fbl/mutex.h>

#include "src/graphics/drivers/misc/goldfish/pipe_connection.h"

namespace goldfish {

// |PipeDevice| is the "root" ACPI device that creates pipes and executes pipe
// operations. It could create multiple |PipeChildDevice| instances using
// |CreateChildDevice| method, each having its own properties so that they can
// be bound to different drivers, but sharing the same parent |PipeDevice|.
class PipeDevice : public fidl::WireServer<fuchsia_hardware_goldfish::PipeDevice>,
                   public fidl::WireServer<fuchsia_hardware_goldfish_pipe::Bus> {
 public:
  explicit PipeDevice(fidl::ClientEnd<fuchsia_hardware_acpi::Device> acpi,
                      fdf::UnownedSynchronizedDispatcher dispatcher);
  ~PipeDevice();

  PipeDevice(const PipeDevice&) = delete;
  PipeDevice& operator=(const PipeDevice&) = delete;
  PipeDevice(PipeDevice&&) = delete;
  PipeDevice& operator=(PipeDevice&&) = delete;

  // Must be called exactly once for each `PipeDevice` instance.
  zx::result<> Initialize();

  // Corresponds to `PrepareStop()` in DFv2 driver.
  zx::result<> PrepareStop();

  zx_status_t Create(int32_t* out_id, zx::vmo* out_vmo);
  zx_status_t SetEvent(int32_t id, zx::event pipe_event);
  void Destroy(int32_t id);
  void Open(int32_t id);
  void Exec(int32_t id);
  zx_status_t GetBti(zx::bti* out_bti);

  // `fuchsia.hardware.goldfish.PipeDevice`:
  void Connect(ConnectRequestView request, ConnectCompleter::Sync& completer) override;

  // `fuchsia.hardware.goldfish.pipe.Bus`:
  void Create(CreateCompleter::Sync& completer) override;
  void SetEvent(SetEventRequestView request, SetEventCompleter::Sync& completer) override;
  void Destroy(DestroyRequestView request, DestroyCompleter::Sync& completer) override;
  void Open(OpenRequestView request, OpenCompleter::Sync& completer) override;
  void Exec(ExecRequestView request, ExecCompleter::Sync& completer) override;
  void GetBti(GetBtiCompleter::Sync& completer) override;

  int IrqHandler();

 private:
  // Storage of commands and responses to be transferred over
  // the pipe of a certain connection.
  struct CommandStorage {
    CommandStorage(zx_paddr_t paddr, zx::pmt pmt, zx::event pipe_event);
    ~CommandStorage();

    void SignalEvent(uint32_t flags) const;

    const zx_paddr_t paddr;
    zx::pmt pmt;
    zx::event pipe_event;
  };

  fidl::WireSyncClient<fuchsia_hardware_acpi::Device> acpi_;

  zx::interrupt irq_;
  zx::bti bti_;
  std::unique_ptr<dma_buffer::ContiguousBuffer> io_buffer_;
  thrd_t irq_thread_{};
  int32_t next_pipe_id_ __TA_GUARDED(pipes_lock_) = 1;

  fbl::Mutex mmio_lock_;
  std::optional<fdf::MmioBuffer> mmio_ __TA_GUARDED(mmio_lock_);

  fbl::Mutex pipes_lock_;
  std::unordered_map</* connection_id */ int32_t, std::unique_ptr<CommandStorage>> command_storages_
      __TA_GUARDED(pipes_lock_);

  std::unordered_map<PipeConnection*, std::unique_ptr<PipeConnection>> pipe_connections_;

  fdf::UnownedSynchronizedDispatcher const dispatcher_;
};

}  // namespace goldfish

#endif  // SRC_GRAPHICS_DRIVERS_MISC_GOLDFISH_PIPE_DEVICE_H_
