// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/drivers/misc/goldfish/pipe_device.h"

#include <fidl/fuchsia.hardware.goldfish.pipe/cpp/fidl.h>
#include <fidl/fuchsia.hardware.goldfish/cpp/fidl.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/driver/testing/cpp/driver_runtime.h>
#include <lib/driver/testing/cpp/scoped_global_logger.h>
#include <lib/fake-bti/bti.h>
#include <lib/zx/vmar.h>

#include <cstring>
#include <memory>

#include <gtest/gtest.h>

#include "src/devices/lib/acpi/mock/mock-acpi.h"
#include "src/lib/testing/predicates/status.h"

namespace goldfish {

using MockAcpiFidl = acpi::mock::Device;

namespace {

constexpr uint32_t kPipeMinDeviceVersion = 2;
constexpr uint32_t kMaxSignalledPipes = 64;

// MMIO Registers of goldfish pipe.
// The layout should match the register offsets defined in pipe_device.cc.
struct Registers {
  uint32_t command;
  uint32_t signal_buffer_high;
  uint32_t signal_buffer_low;
  uint32_t signal_buffer_count;
  uint32_t reserved0[1];
  uint32_t open_buffer_high;
  uint32_t open_buffer_low;
  uint32_t reserved1[2];
  uint32_t version;
  uint32_t reserved2[3];
  uint32_t get_signalled;

  void DebugPrint() const {
    printf(
        "Registers [ command %08x signal_buffer: %08x %08x count %08x open_buffer: %08x %08x "
        "version %08x get_signalled %08x ]\n",
        command, signal_buffer_high, signal_buffer_low, signal_buffer_count, open_buffer_high,
        open_buffer_low, version, get_signalled);
  }
};

// A RAII memory mapping wrapper of VMO to memory.
class VmoMapping {
 public:
  VmoMapping(const zx::vmo& vmo, size_t size, size_t offset = 0,
             zx_vm_option_t perm = ZX_VM_PERM_READ | ZX_VM_PERM_WRITE)
      : vmo_(vmo), size_(size), offset_(offset), perm_(perm) {
    map();
  }

  ~VmoMapping() { unmap(); }

  void map() {
    if (!ptr_) {
      zx::vmar::root_self()->map(perm_, 0, vmo_, offset_, size_,
                                 reinterpret_cast<uintptr_t*>(&ptr_));
    }
  }

  void unmap() {
    if (ptr_) {
      zx::vmar::root_self()->unmap(reinterpret_cast<uintptr_t>(ptr_), size_);
      ptr_ = nullptr;
    }
  }

  void* ptr() const { return ptr_; }

 private:
  const zx::vmo& vmo_;
  size_t size_ = 0u;
  size_t offset_ = 0u;
  zx_vm_option_t perm_ = 0;
  void* ptr_ = nullptr;
};

// Test suite creating fake PipeDevice on a mock ACPI bus.
class PipeDeviceTest : public ::testing::Test {
 public:
  void SetUp() override {
    ASSERT_OK(fake_bti_create(acpi_bti_.reset_and_get_address()));

    constexpr size_t kCtrlSize = 4096u;
    ASSERT_OK(zx::vmo::create(kCtrlSize, 0u, &vmo_control_));

    zx::interrupt irq;
    ASSERT_OK(zx::interrupt::create(zx::resource(), 0u, ZX_INTERRUPT_VIRTUAL, &irq));
    ASSERT_OK(irq.duplicate(ZX_RIGHT_SAME_RIGHTS, &irq_));

    mock_acpi_fidl_.SetMapInterrupt(
        [this](acpi::mock::Device::MapInterruptRequestView rv,
               acpi::mock::Device::MapInterruptCompleter::Sync& completer) {
          zx::interrupt dupe;
          ASSERT_OK(zx::interrupt::create(zx::resource(), 0, ZX_INTERRUPT_VIRTUAL, &dupe));
          ASSERT_OK(irq_.duplicate(ZX_RIGHT_SAME_RIGHTS, &dupe));
          completer.ReplySuccess(std::move(dupe));
        });
    mock_acpi_fidl_.SetGetMmio([this](acpi::mock::Device::GetMmioRequestView rv,
                                      acpi::mock::Device::GetMmioCompleter::Sync& completer) {
      ASSERT_EQ(rv->index, 0u);
      zx::vmo dupe;
      ASSERT_OK(vmo_control_.duplicate(ZX_RIGHT_SAME_RIGHTS, &dupe));
      completer.ReplySuccess(fuchsia_mem::wire::Range{
          .vmo = std::move(dupe),
          .offset = 0,
          .size = kCtrlSize,
      });
    });

    mock_acpi_fidl_.SetGetBti([this](acpi::mock::Device::GetBtiRequestView rv,
                                     acpi::mock::Device::GetBtiCompleter::Sync& completer) {
      ASSERT_EQ(rv->index, 0u);
      zx::bti out_bti;
      ASSERT_OK(acpi_bti_.duplicate(ZX_RIGHT_SAME_RIGHTS, &out_bti));
      completer.ReplySuccess(std::move(out_bti));
    });

    zx::result<acpi::Client> acpi_client =
        mock_acpi_fidl_.CreateClient(env_dispatcher_->async_dispatcher());
    ASSERT_OK(acpi_client.status_value());

    fidl::ClientEnd<fuchsia_hardware_acpi::Device> acpi =
        std::move(acpi_client).value().borrow().TakeClientEnd();
    dut_ = std::make_unique<PipeDevice>(std::move(acpi), driver_dispatcher_->borrow());
    ASSERT_OK(dut_->Initialize());

    auto [bus_client, bus_server] = fidl::Endpoints<fuchsia_hardware_goldfish_pipe::Bus>::Create();
    binding_ =
        fidl::BindServer(driver_dispatcher_->async_dispatcher(), std::move(bus_server), dut_.get());
    EXPECT_TRUE(binding_.has_value());

    client_ = fidl::SyncClient(std::move(bus_client));
  }

  void TearDown() override { ASSERT_OK(dut_->PrepareStop()); }

  std::unique_ptr<VmoMapping> MapControlRegisters() const {
    return std::make_unique<VmoMapping>(vmo_control_, /*size=*/sizeof(Registers), /*offset=*/0);
  }

  template <typename T>
  static void Flush(const T* t) {
    zx_cache_flush(t, sizeof(T), ZX_CACHE_FLUSH_DATA | ZX_CACHE_FLUSH_INVALIDATE);
  }

 protected:
  fdf_testing::ScopedGlobalLogger logger_;
  fdf_testing::DriverRuntime runtime_;

  fdf::UnownedSynchronizedDispatcher env_dispatcher_{runtime_.StartBackgroundDispatcher()};
  fdf::UnownedSynchronizedDispatcher driver_dispatcher_{runtime_.StartBackgroundDispatcher()};

  acpi::mock::Device mock_acpi_fidl_;
  std::unique_ptr<PipeDevice> dut_;

  fidl::SyncClient<fuchsia_hardware_goldfish_pipe::Bus> client_;
  std::optional<fidl::ServerBindingRef<fuchsia_hardware_goldfish_pipe::Bus>> binding_;

  zx::bti acpi_bti_;
  zx::vmo vmo_control_;
  zx::interrupt irq_;
};

TEST_F(PipeDeviceTest, Bind) {
  {
    auto mapped = MapControlRegisters();
    Registers* ctrl_regs = reinterpret_cast<Registers*>(mapped->ptr());
    ctrl_regs->version = kPipeMinDeviceVersion;
  }

  {
    auto mapped = MapControlRegisters();
    Registers* ctrl_regs = reinterpret_cast<Registers*>(mapped->ptr());
    Flush(ctrl_regs);

    zx_paddr_t signal_buffer = (static_cast<uint64_t>(ctrl_regs->signal_buffer_high) << 32u) |
                               (ctrl_regs->signal_buffer_low);
    ASSERT_NE(signal_buffer, 0u);

    uint32_t buffer_count = ctrl_regs->signal_buffer_count;
    ASSERT_EQ(buffer_count, kMaxSignalledPipes);

    zx_paddr_t open_buffer =
        (static_cast<uint64_t>(ctrl_regs->open_buffer_high) << 32u) | (ctrl_regs->open_buffer_low);
    ASSERT_NE(open_buffer, 0u);
  }
}

TEST_F(PipeDeviceTest, CreatePipe) {
  fidl::Result create_result = client_->Create();
  ASSERT_TRUE(create_result.is_ok());

  int32_t id = create_result.value().id();
  zx::vmo vmo = std::move(create_result.value().vmo());

  EXPECT_NE(id, 0);
  EXPECT_TRUE(vmo.is_valid());

  fidl::Result destroy_result = client_->Destroy(id);
  ASSERT_TRUE(destroy_result.is_ok());
}

TEST_F(PipeDeviceTest, Exec) {
  fidl::Result create_result = client_->Create();
  ASSERT_TRUE(create_result.is_ok());

  int32_t id = create_result.value().id();
  zx::vmo vmo = std::move(create_result.value().vmo());

  ASSERT_NE(id, 0);
  ASSERT_TRUE(vmo.is_valid());

  fidl::Result exec_result = client_->Exec(id);
  ASSERT_TRUE(exec_result.is_ok());

  {
    auto mapped = MapControlRegisters();
    Registers* ctrl_regs = reinterpret_cast<Registers*>(mapped->ptr());
    ASSERT_EQ(ctrl_regs->command, static_cast<uint32_t>(id));
  }

  fidl::Result destroy_result = client_->Destroy(id);
  EXPECT_TRUE(destroy_result.is_ok());
}

TEST_F(PipeDeviceTest, TransferObservedSignals) {
  fidl::Result create_result = client_->Create();
  ASSERT_TRUE(create_result.is_ok());

  int32_t id = create_result.value().id();
  zx::vmo vmo = std::move(create_result.value().vmo());

  zx::event old_event, old_event_dup;
  ASSERT_OK(zx::event::create(0u, &old_event));
  ASSERT_OK(old_event.duplicate(ZX_RIGHT_SAME_RIGHTS, &old_event_dup));

  fidl::Result set_event_result =
      client_->SetEvent({{.id = id, .pipe_event = std::move(old_event_dup)}});
  ASSERT_TRUE(set_event_result.is_ok());

  // Trigger signals on "old" event.
  old_event.signal(0u, fuchsia_hardware_goldfish::wire::kSignalReadable);

  zx::event new_event, new_event_dup;
  ASSERT_OK(zx::event::create(0u, &new_event));
  // Clear the target signal.
  ASSERT_OK(new_event.signal(fuchsia_hardware_goldfish::wire::kSignalReadable, 0u));
  ASSERT_OK(new_event.duplicate(ZX_RIGHT_SAME_RIGHTS, &new_event_dup));

  set_event_result = client_->SetEvent({{.id = id, .pipe_event = std::move(new_event_dup)}});
  ASSERT_TRUE(set_event_result.is_ok());

  // Wait for `SIGNAL_READABLE` signal on the new event.
  zx_signals_t observed;
  ASSERT_OK(new_event.wait_one(fuchsia_hardware_goldfish::wire::kSignalReadable,
                               zx::time::infinite_past(), &observed));
}

TEST_F(PipeDeviceTest, GetBti) {
  fidl::Result get_bti_result = client_->GetBti();
  ASSERT_TRUE(get_bti_result.is_ok());
  zx::bti bti = std::move(get_bti_result.value().bti());

  zx_info_bti_t goldfish_bti_info, acpi_bti_info;
  ASSERT_OK(
      bti.get_info(ZX_INFO_BTI, &goldfish_bti_info, sizeof(goldfish_bti_info), nullptr, nullptr));
  ASSERT_OK(
      acpi_bti_.get_info(ZX_INFO_BTI, &acpi_bti_info, sizeof(acpi_bti_info), nullptr, nullptr));

  ASSERT_FALSE(memcmp(&goldfish_bti_info, &acpi_bti_info, sizeof(zx_info_bti_t)));
}

}  // namespace

}  // namespace goldfish
