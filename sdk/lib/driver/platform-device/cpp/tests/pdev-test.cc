// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.platform.device/cpp/wire_test_base.h>
#include <fidl/test.metadata/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/default.h>
#include <lib/async_patterns/testing/cpp/dispatcher_bound.h>
#include <lib/driver/fake-bti/cpp/fake-bti.h>
#include <lib/driver/fake-platform-device/cpp/fake-pdev.h>
#include <lib/driver/fake-resource/cpp/fake-resource.h>
#include <lib/driver/mmio/cpp/mmio-buffer.h>
#include <lib/driver/mmio/testing/cpp/test-helper.h>
#include <lib/driver/platform-device/cpp/pdev.h>
#include <zircon/errors.h>
#include <zircon/syscalls/object.h>
#include <zircon/types.h>

#include <zxtest/zxtest.h>

#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)

constexpr uint32_t kVid = 1;
constexpr uint32_t kPid = 1;
constexpr char kName[] = "test device";

namespace fhpd = fuchsia_hardware_platform_device;
class DeviceServer : public fidl::testing::WireTestBase<fuchsia_hardware_platform_device::Device> {
 public:
  zx_status_t Connect(fidl::ServerEnd<fhpd::Device> request) {
    if (binding_.has_value()) {
      return ZX_ERR_ALREADY_BOUND;
    }
    binding_.emplace(async_get_default_dispatcher(), std::move(request), this,
                     fidl::kIgnoreBindingClosure);
    return ZX_OK;
  }

 private:
  void NotImplemented_(const std::string& name, ::fidl::CompleterBase& completer) override {
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }
  void GetBoardInfo(GetBoardInfoCompleter::Sync& completer) override {
    fidl::Arena arena;
    completer.ReplySuccess(fuchsia_hardware_platform_device::wire::BoardInfo::Builder(arena)
                               .vid(kVid)
                               .pid(kPid)
                               .Build());
  }
  std::optional<fidl::ServerBinding<fuchsia_hardware_platform_device::Device>> binding_;
};

class FakePDevWithThread {
 public:
  zx::result<fidl::ClientEnd<fuchsia_hardware_platform_device::Device>> Start(
      fdf_fake::FakePDev::Config config) {
    loop_.StartThread("pdev-fidl-thread");
    if (zx_status_t status = server.SyncCall(&fdf_fake::FakePDev::SetConfig, std::move(config));
        status != ZX_OK) {
      return zx::error(status);
    }
    zx::result endpoints = fidl::CreateEndpoints<fuchsia_hardware_platform_device::Device>();
    if (endpoints.is_error()) {
      return endpoints.take_error();
    }
    if (zx_status_t status =
            server.SyncCall(&fdf_fake::FakePDev::Connect, std::move(endpoints->server));
        status != ZX_OK) {
      return zx::error(status);
    }
    return zx::ok(std::move(endpoints->client));
  }

  template <typename FidlType>
  void AddFidlMetadata(FidlType metadata) {
    server.SyncCall([metadata = std::move(metadata)](auto* pdev) {
      pdev->AddFidlMetadata(FidlType::kSerializableName, metadata);
    });
  }

 private:
  async::Loop loop_{&kAsyncLoopConfigNoAttachToCurrentThread};
  async_patterns::TestDispatcherBound<fdf_fake::FakePDev> server{loop_.dispatcher(), std::in_place};
};

TEST(PDevTest, GetMmios) {
  constexpr uint32_t kMmioId = 5;
  constexpr zx_off_t kMmioOffset = 10;
  constexpr size_t kMmioSize = 11;
  constexpr std::string_view kMmioName = "test-name";
  std::map<uint32_t, fdf_fake::Mmio> mmios;
  {
    fdf::PDev::MmioInfo mmio{
        .offset = kMmioOffset,
        .size = kMmioSize,
    };
    ASSERT_OK(zx::vmo::create(0, 0, &mmio.vmo));
    mmios[kMmioId] = std::move(mmio);
  }
  fdf_fake::FakePDev::ResourceNamesMap mmio_names = {{std::string(kMmioName), kMmioId}};

  FakePDevWithThread infra;
  zx::result client_channel =
      infra.Start({.mmios = std::move(mmios), .mmio_names = std::move(mmio_names)});
  ASSERT_OK(client_channel);

  fdf::PDev pdev{std::move(client_channel.value())};

  // Verify that an mmio can be retrieved using its ID.
  {
    zx::result mmio = pdev.GetMmio(kMmioId);
    ASSERT_OK(mmio.status_value());
    ASSERT_EQ(kMmioOffset, mmio->offset);
    ASSERT_EQ(kMmioSize, mmio->size);
    ASSERT_EQ(ZX_ERR_NOT_FOUND, pdev.GetMmio(4).status_value());
  }

  // Verify that an mmio can be retrieved using its name.
  {
    zx::result mmio = pdev.GetMmio(kMmioName);
    ASSERT_OK(mmio.status_value());
    ASSERT_EQ(kMmioOffset, mmio->offset);
    ASSERT_EQ(kMmioSize, mmio->size);
    ASSERT_EQ(ZX_ERR_NOT_FOUND, pdev.GetMmio("unknown-name").status_value());
  }
}

TEST(PDevTest, GetMmioBuffer) {
  constexpr uint32_t kMmioId = 5;
  constexpr zx_off_t kMmioOffset = 10;
  constexpr size_t kMmioSize = 11;
  MMIO_PTR void* mmio_vaddr;
  zx_koid_t vmo_koid;
  zx_info_handle_basic_t info;
  std::map<uint32_t, fdf_fake::Mmio> mmios;
  {
    zx::vmo vmo;
    ASSERT_OK(zx::vmo::create(kMmioSize, 0, &vmo));
    ASSERT_OK(vmo.get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr));
    zx::result<fdf::MmioBuffer> mmio =
        fdf::MmioBuffer::Create(kMmioOffset, kMmioSize, std::move(vmo), ZX_CACHE_POLICY_UNCACHED);
    ASSERT_OK(mmio.status_value());
    vmo_koid = info.koid;
    mmio_vaddr = mmio->get();
    mmios[kMmioId] = std::move(*mmio);
  }

  FakePDevWithThread infra;
  zx::result client_channel = infra.Start({
      .mmios = std::move(mmios),
  });
  ASSERT_OK(client_channel);

  fdf::PDev pdev{std::move(client_channel.value())};
  zx::result mmio = pdev.GetMmio(kMmioId);
  ASSERT_OK(mmio.status_value());
  ASSERT_NE(kMmioOffset, mmio->offset);
  ASSERT_EQ(0, mmio->size);
  ASSERT_EQ(ZX_HANDLE_INVALID, mmio->vmo);

  auto* mmio_buffer = reinterpret_cast<fdf::MmioBuffer*>(mmio->offset);
  ASSERT_EQ(kMmioOffset, mmio_buffer->get_offset());
  ASSERT_EQ(kMmioSize, mmio_buffer->get_size());
  ASSERT_EQ(mmio_vaddr, mmio_buffer->get());
  ASSERT_OK(mmio_buffer->get_vmo()->get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr,
                                             nullptr));
  ASSERT_EQ(vmo_koid, info.koid);

  ASSERT_EQ(ZX_ERR_NOT_FOUND, pdev.GetMmio(4).status_value());
}

TEST(PDevTest, InvalidMmioHandle) {
  constexpr uint32_t kMmioId = 5;
  constexpr zx_off_t kMmioOffset = 10;
  constexpr size_t kMmioSize = 11;
  std::map<uint32_t, fdf_fake::Mmio> mmios;
  mmios[kMmioId] = fdf::PDev::MmioInfo{
      .offset = kMmioOffset,
      .size = kMmioSize,
  };

  FakePDevWithThread infra;
  zx::result client_channel = infra.Start({
      .mmios = std::move(mmios),
  });
  ASSERT_OK(client_channel);

  fdf::PDev pdev{std::move(client_channel.value())};
  ASSERT_NOT_OK(pdev.GetMmio(kMmioId).status_value());
}

TEST(PDevTest, GetIrqs) {
  constexpr uint32_t kIrqId = 5;
  constexpr std::string_view kIrqName = "test-name";
  std::map<uint32_t, zx::interrupt> irqs;
  {
    zx::interrupt irq;
    ASSERT_OK(zx::interrupt::create(zx::resource(), 0, ZX_INTERRUPT_VIRTUAL, &irq));
    irqs[kIrqId] = std::move(irq);
  }
  fdf_fake::FakePDev::ResourceNamesMap irq_names = {{std::string(kIrqName), kIrqId}};

  FakePDevWithThread infra;
  zx::result client_channel = infra.Start({
      .irqs = std::move(irqs),
      .irq_names = std::move(irq_names),
  });
  ASSERT_OK(client_channel);

  fdf::PDev pdev{std::move(client_channel.value())};

  // Verify that an interrupt can be retrieved using its ID.
  ASSERT_OK(pdev.GetInterrupt(kIrqId, 0).status_value());
  ASSERT_EQ(ZX_ERR_NOT_FOUND, pdev.GetInterrupt(4, 0).status_value());

  // Verify that an interrupt can be retrieved using its name.
  ASSERT_OK(pdev.GetInterrupt(kIrqName, 0).status_value());
  ASSERT_EQ(ZX_ERR_NOT_FOUND, pdev.GetInterrupt("unknown-name", 0).status_value());
}

TEST(PDevTest, GetBtis) {
  constexpr uint32_t kBtiId = 5;
  std::map<uint32_t, zx::bti> btis;
  {
    zx::result bti = fake_bti::CreateFakeBti();
    ASSERT_OK(bti);
    btis[kBtiId] = std::move(bti.value());
  }

  FakePDevWithThread infra;
  zx::result client_channel = infra.Start({
      .btis = std::move(btis),
  });
  ASSERT_OK(client_channel);

  fdf::PDev pdev{std::move(client_channel.value())};
  ASSERT_OK(pdev.GetBti(kBtiId).status_value());
  ASSERT_EQ(ZX_ERR_NOT_FOUND, pdev.GetBti(4).status_value());
}

TEST(PDevTest, GetSmc) {
  constexpr uint32_t kSmcId = 5;
  std::map<uint32_t, zx::resource> smcs;
  {
    zx::resource smc;
    ASSERT_OK(fake_resource_create(ZX_RSRC_KIND_SMC, smc.reset_and_get_address()));
    smcs[kSmcId] = std::move(smc);
  }

  FakePDevWithThread infra;
  zx::result client_channel = infra.Start({
      .smcs = std::move(smcs),
  });
  ASSERT_OK(client_channel);

  fdf::PDev pdev{std::move(client_channel.value())};
  zx::resource smc;
  ASSERT_OK(pdev.GetSmc(kSmcId).status_value());
  ASSERT_EQ(ZX_ERR_NOT_FOUND, pdev.GetSmc(4).status_value());
}

TEST(PDevTest, GetDeviceInfo) {
  FakePDevWithThread infra;
  zx::result client_channel = infra.Start({
      .device_info =
          []() {
            fdf::PDev::DeviceInfo device_info{
                .vid = kVid,
                .pid = kPid,
                .name = kName,
            };
            return device_info;
          }(),
  });
  ASSERT_OK(client_channel);

  fdf::PDev pdev{std::move(client_channel.value())};
  zx::result device_info = pdev.GetDeviceInfo();
  ASSERT_OK(device_info.status_value());
  ASSERT_EQ(kPid, device_info->pid);
  ASSERT_EQ(kVid, device_info->vid);
  ASSERT_STREQ(kName, device_info->name);
}

TEST(PDevTest, GetBoardInfo) {
  FakePDevWithThread infra;
  zx::result client_channel = infra.Start({
      .board_info =
          fdf::PDev::BoardInfo{
              .vid = kVid,
              .pid = kPid,
          },
  });
  ASSERT_OK(client_channel);

  fdf::PDev pdev{std::move(client_channel.value())};
  zx::result board_info = pdev.GetBoardInfo();
  ASSERT_OK(board_info.status_value());
  ASSERT_EQ(kPid, board_info->pid);
  ASSERT_EQ(kVid, board_info->vid);
}

TEST(PDevTest, GetPowerConfiguration) {
  fuchsia_hardware_power::PowerElement fidl_element{{.name = "test power element", .levels = {{}}}};
  fuchsia_hardware_power::PowerElementConfiguration fidl_config{
      {.element = std::move(fidl_element), .dependencies = {{}}}};
  std::vector<fuchsia_hardware_power::PowerElementConfiguration> fidl_configs{
      std::move(fidl_config)};

  FakePDevWithThread infra;
  zx::result client_channel = infra.Start({.power_elements = std::move(fidl_configs)});
  ASSERT_OK(client_channel);

  fdf::PDev pdev{std::move(client_channel.value())};
  zx::result configs = pdev.GetPowerConfiguration();
  ASSERT_OK(configs.status_value());
  ASSERT_EQ(configs->size(), 1);
  const auto& config = configs.value()[0];
  ASSERT_EQ(config.element.name, "test power element");
  ASSERT_TRUE(config.element.levels.empty());
  ASSERT_TRUE(config.dependencies.empty());
}

TEST(PDevTest, GetFidlMetadata) {
  static const test_metadata::Metadata kMetadata{{.test_field = "test value"}};
  FakePDevWithThread infra;
  zx::result client_channel = infra.Start({});
  infra.AddFidlMetadata(kMetadata);
  ASSERT_OK(client_channel);

  fdf::PDev pdev{std::move(client_channel.value())};
  zx::result metadata = pdev.GetFidlMetadata<test_metadata::Metadata>();
  ASSERT_OK(metadata);
  ASSERT_EQ(metadata.value(), kMetadata);
}

#endif  // FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
