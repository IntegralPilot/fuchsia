// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "spi.h"

#include <fidl/fuchsia.hardware.spi.businfo/cpp/wire.h>
#include <fidl/fuchsia.hardware.spiimpl/cpp/driver/wire.h>
#include <fidl/fuchsia.scheduler/cpp/fidl.h>
#include <lib/component/incoming/cpp/service.h>
#include <lib/ddk/metadata.h>
#include <lib/driver/metadata/cpp/metadata_server.h>
#include <lib/driver/outgoing/cpp/outgoing_directory.h>
#include <lib/driver/testing/cpp/driver_test.h>
#include <lib/fidl/cpp/wire/client.h>
#include <lib/spi/spi.h>
#include <zircon/errors.h>

#include <array>
#include <map>

#include <gtest/gtest.h>

#include "spi-child.h"
#include "src/devices/spi/drivers/spi/spi_config.h"
#include "src/lib/testing/predicates/status.h"

namespace spi {

class FakeSpiImplServer : public fdf::WireServer<fuchsia_hardware_spiimpl::SpiImpl> {
 public:
  static constexpr uint8_t kChipSelectCount = 2;

  fidl::ProtocolHandler<fuchsia_hardware_spiimpl::SpiImpl> GetHandler() {
    return bindings_.CreateHandler(this, fdf::Dispatcher::GetCurrent()->get(),
                                   std::mem_fn(&FakeSpiImplServer::OnClosed));
  }

  // fuchsia_hardware_spiimpl::SpiImpl methods
  void GetChipSelectCount(fdf::Arena& arena,
                          GetChipSelectCountCompleter::Sync& completer) override {
    completer.buffer(arena).Reply(kChipSelectCount);
  }
  void TransmitVector(fuchsia_hardware_spiimpl::wire::SpiImplTransmitVectorRequest* request,
                      fdf::Arena& arena, TransmitVectorCompleter::Sync& completer) override {
    size_t actual;
    auto status = SpiImplExchange(request->chip_select, request->data.data(), request->data.size(),
                                  nullptr, 0, &actual);
    if (status != ZX_OK) {
      completer.buffer(arena).ReplyError(status);
      return;
    }
    completer.buffer(arena).ReplySuccess();
  }
  void ReceiveVector(fuchsia_hardware_spiimpl::wire::SpiImplReceiveVectorRequest* request,
                     fdf::Arena& arena, ReceiveVectorCompleter::Sync& completer) override {
    std::vector<uint8_t> rxdata(request->size);
    size_t actual;
    auto status =
        SpiImplExchange(request->chip_select, nullptr, 0, rxdata.data(), rxdata.size(), &actual);
    if (status != ZX_OK) {
      completer.buffer(arena).ReplyError(status);
      return;
    }
    rxdata.resize(actual);
    completer.buffer(arena).ReplySuccess(fidl::VectorView<uint8_t>::FromExternal(rxdata));
  }
  void ExchangeVector(fuchsia_hardware_spiimpl::wire::SpiImplExchangeVectorRequest* request,
                      fdf::Arena& arena, ExchangeVectorCompleter::Sync& completer) override {
    std::vector<uint8_t> rxdata(request->txdata.size());
    size_t actual;
    auto status = SpiImplExchange(request->chip_select, request->txdata.data(),
                                  request->txdata.size(), rxdata.data(), rxdata.size(), &actual);
    if (status != ZX_OK) {
      completer.buffer(arena).ReplyError(status);
      return;
    }
    rxdata.resize(actual);
    completer.buffer(arena).ReplySuccess(fidl::VectorView<uint8_t>::FromExternal(rxdata));
  }
  void LockBus(fuchsia_hardware_spiimpl::wire::SpiImplLockBusRequest* request, fdf::Arena& arena,
               LockBusCompleter::Sync& completer) override {
    completer.buffer(arena).ReplySuccess();
  }
  void UnlockBus(fuchsia_hardware_spiimpl::wire::SpiImplUnlockBusRequest* request,
                 fdf::Arena& arena, UnlockBusCompleter::Sync& completer) override {
    completer.buffer(arena).ReplySuccess();
  }
  void RegisterVmo(fuchsia_hardware_spiimpl::wire::SpiImplRegisterVmoRequest* request,
                   fdf::Arena& arena, RegisterVmoCompleter::Sync& completer) override {
    auto status = SpiImplRegisterVmo(request->chip_select, request->vmo_id,
                                     std::move(request->vmo.vmo), request->vmo.offset,
                                     request->vmo.size, static_cast<uint32_t>(request->rights));
    if (status != ZX_OK) {
      completer.buffer(arena).ReplyError(status);
      return;
    }
    completer.buffer(arena).ReplySuccess();
  }
  void UnregisterVmo(fuchsia_hardware_spiimpl::wire::SpiImplUnregisterVmoRequest* request,
                     fdf::Arena& arena, UnregisterVmoCompleter::Sync& completer) override {
    zx::vmo vmo;
    auto status = SpiImplUnregisterVmo(request->chip_select, request->vmo_id, &vmo);
    if (status != ZX_OK) {
      completer.buffer(arena).ReplyError(status);
      return;
    }
    completer.buffer(arena).ReplySuccess(std::move(vmo));
  }
  void ReleaseRegisteredVmos(
      fuchsia_hardware_spiimpl::wire::SpiImplReleaseRegisteredVmosRequest* request,
      fdf::Arena& arena, ReleaseRegisteredVmosCompleter::Sync& completer) override {
    SpiImplReleaseRegisteredVmos(request->chip_select);
  }
  void TransmitVmo(fuchsia_hardware_spiimpl::wire::SpiImplTransmitVmoRequest* request,
                   fdf::Arena& arena, TransmitVmoCompleter::Sync& completer) override {
    auto status = SpiImplTransmitVmo(request->chip_select, request->buffer.vmo_id,
                                     request->buffer.offset, request->buffer.size);
    if (status != ZX_OK) {
      completer.buffer(arena).ReplyError(status);
      return;
    }
    completer.buffer(arena).ReplySuccess();
  }
  void ReceiveVmo(fuchsia_hardware_spiimpl::wire::SpiImplReceiveVmoRequest* request,
                  fdf::Arena& arena, ReceiveVmoCompleter::Sync& completer) override {
    auto status = SpiImplReceiveVmo(request->chip_select, request->buffer.vmo_id,
                                    request->buffer.offset, request->buffer.size);
    if (status != ZX_OK) {
      completer.buffer(arena).ReplyError(status);
      return;
    }
    completer.buffer(arena).ReplySuccess();
  }
  void ExchangeVmo(fuchsia_hardware_spiimpl::wire::SpiImplExchangeVmoRequest* request,
                   fdf::Arena& arena, ExchangeVmoCompleter::Sync& completer) override {
    ASSERT_EQ(request->tx_buffer.size, request->rx_buffer.size);
    auto status = SpiImplExchangeVmo(request->chip_select, request->tx_buffer.vmo_id,
                                     request->tx_buffer.offset, request->rx_buffer.vmo_id,
                                     request->rx_buffer.offset, request->tx_buffer.size);
    if (status != ZX_OK) {
      completer.buffer(arena).ReplyError(status);
      return;
    }
    completer.buffer(arena).ReplySuccess();
  }

  uint32_t current_test_cs_ = 0;
  bool corrupt_rx_actual_ = false;
  bool vmos_released_since_last_call_ = false;

  enum class SpiTestMode {
    kTransmit,
    kReceive,
    kExchange,
  } test_mode_;

  std::map<uint32_t, zx::vmo> cs0_vmos;
  std::map<uint32_t, zx::vmo> cs1_vmos;

 private:
  static constexpr uint8_t kTestData[] = {1, 2, 3, 4, 5, 6, 7};

  void OnClosed(fidl::UnbindInfo info) {
    // Called when a connection to this server is closed.
    // This is provided to the binding group during |CreateHandler|.
  }

  zx_status_t SpiImplExchange(uint32_t cs, const uint8_t* txdata, size_t txdata_size,
                              uint8_t* out_rxdata, size_t rxdata_size, size_t* out_rxdata_actual) {
    EXPECT_EQ(cs, current_test_cs_);

    switch (test_mode_) {
      case SpiTestMode::kTransmit:
        EXPECT_NE(txdata, nullptr);
        EXPECT_NE(txdata_size, 0ul);
        EXPECT_EQ(out_rxdata, nullptr);
        EXPECT_EQ(rxdata_size, 0ul);
        *out_rxdata_actual = 0;
        break;
      case SpiTestMode::kReceive:
        EXPECT_EQ(txdata, nullptr);
        EXPECT_EQ(txdata_size, 0ul);
        EXPECT_NE(out_rxdata, nullptr);
        EXPECT_NE(rxdata_size, 0ul);
        memset(out_rxdata, 0, rxdata_size);
        memcpy(out_rxdata, kTestData, std::min(rxdata_size, sizeof(kTestData)));
        *out_rxdata_actual = rxdata_size + (corrupt_rx_actual_ ? 1 : 0);
        break;
      case SpiTestMode::kExchange:
        EXPECT_NE(txdata, nullptr);
        EXPECT_NE(txdata_size, 0ul);
        EXPECT_NE(out_rxdata, nullptr);
        EXPECT_NE(rxdata_size, 0ul);
        EXPECT_EQ(txdata_size, rxdata_size);
        memset(out_rxdata, 0, rxdata_size);
        memcpy(out_rxdata, txdata, std::min(rxdata_size, txdata_size));
        *out_rxdata_actual = std::min(rxdata_size, txdata_size) + (corrupt_rx_actual_ ? 1 : 0);
        break;
    }

    return ZX_OK;
  }

  zx_status_t SpiImplRegisterVmo(uint32_t chip_select, uint32_t vmo_id, zx::vmo vmo,
                                 uint64_t offset, uint64_t size, uint32_t rights) {
    if (chip_select > 1) {
      return ZX_ERR_OUT_OF_RANGE;
    }

    std::map<uint32_t, zx::vmo>& map = chip_select == 0 ? cs0_vmos : cs1_vmos;
    if (map.find(vmo_id) != map.end()) {
      return ZX_ERR_ALREADY_EXISTS;
    }

    map[vmo_id] = std::move(vmo);
    return ZX_OK;
  }

  zx_status_t SpiImplUnregisterVmo(uint32_t chip_select, uint32_t vmo_id, zx::vmo* out_vmo) {
    if (chip_select > 1) {
      return ZX_ERR_OUT_OF_RANGE;
    }

    std::map<uint32_t, zx::vmo>& map = chip_select == 0 ? cs0_vmos : cs1_vmos;
    auto it = map.find(vmo_id);
    if (it == map.end()) {
      return ZX_ERR_NOT_FOUND;
    }

    if (out_vmo) {
      out_vmo->reset(std::get<1>(*it).release());
    }

    map.erase(it);
    return ZX_OK;
  }

  void SpiImplReleaseRegisteredVmos(uint32_t chip_select) { vmos_released_since_last_call_ = true; }

  zx_status_t SpiImplTransmitVmo(uint32_t chip_select, uint32_t vmo_id, uint64_t offset,
                                 uint64_t size) {
    if (chip_select > 1) {
      return ZX_ERR_OUT_OF_RANGE;
    }

    std::map<uint32_t, zx::vmo>& map = chip_select == 0 ? cs0_vmos : cs1_vmos;
    auto it = map.find(vmo_id);
    if (it == map.end()) {
      return ZX_ERR_NOT_FOUND;
    }

    uint8_t buf[sizeof(kTestData)];
    zx_status_t status = std::get<1>(*it).read(buf, offset, std::max(size, sizeof(buf)));
    if (status != ZX_OK) {
      return status;
    }

    return memcmp(buf, kTestData, std::max(size, sizeof(buf))) == 0 ? ZX_OK : ZX_ERR_IO;
  }

  zx_status_t SpiImplReceiveVmo(uint32_t chip_select, uint32_t vmo_id, uint64_t offset,
                                uint64_t size) {
    if (chip_select > 1) {
      return ZX_ERR_OUT_OF_RANGE;
    }

    std::map<uint32_t, zx::vmo>& map = chip_select == 0 ? cs0_vmos : cs1_vmos;
    auto it = map.find(vmo_id);
    if (it == map.end()) {
      return ZX_ERR_NOT_FOUND;
    }

    return std::get<1>(*it).write(kTestData, offset, std::max(size, sizeof(kTestData)));
  }

  zx_status_t SpiImplExchangeVmo(uint32_t chip_select, uint32_t tx_vmo_id, uint64_t tx_offset,
                                 uint32_t rx_vmo_id, uint64_t rx_offset, uint64_t size) {
    if (chip_select > 1) {
      return ZX_ERR_OUT_OF_RANGE;
    }

    std::map<uint32_t, zx::vmo>& map = chip_select == 0 ? cs0_vmos : cs1_vmos;
    auto tx_it = map.find(tx_vmo_id);
    auto rx_it = map.find(rx_vmo_id);

    if (tx_it == map.end() || rx_it == map.end()) {
      return ZX_ERR_NOT_FOUND;
    }

    uint8_t buf[8];
    zx_status_t status = std::get<1>(*tx_it).read(buf, tx_offset, std::max(size, sizeof(buf)));
    if (status != ZX_OK) {
      return status;
    }

    return std::get<1>(*rx_it).write(buf, rx_offset, std::max(size, sizeof(buf)));
  }

  fdf::ServerBindingGroup<fuchsia_hardware_spiimpl::SpiImpl> bindings_;
};

class TestEnvironment : public fdf_testing::Environment {
 public:
  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override {
    if (spi_metadata_.has_value()) {
      if (zx::result result = spi_metadata_server_.Serve(
              to_driver_vfs, fdf::Dispatcher::GetCurrent()->async_dispatcher(),
              spi_metadata_.value());
          result.is_error()) {
        return result.take_error();
      }
    }

    if (role_name_.has_value()) {
      if (zx::result result = scheduler_role_name_metadata_server_.Serve(
              to_driver_vfs, fdf::Dispatcher::GetCurrent()->async_dispatcher(), role_name_.value());
          result.is_error()) {
        return result.take_error();
      }
    }

    return to_driver_vfs.AddService<fuchsia_hardware_spiimpl::Service>(
        fuchsia_hardware_spiimpl::Service::InstanceHandler({
            .device = fake_spi_impl_.GetHandler(),
        }));
  }

  void SetSchedulerRoleName(std::string role_name) {
    role_name_ = fuchsia_scheduler::RoleName(std::move(role_name));
  }

  zx::result<> SetSpiChannelCount(uint32_t count) {
    std::vector<fuchsia_hardware_spi_businfo::SpiChannel> channels;
    channels.reserve(count);
    for (uint32_t i = 0; i < count; i++) {
      channels.emplace_back(
          fuchsia_hardware_spi_businfo::SpiChannel{{.cs = i, .vid = 0, .pid = 0, .did = 0}});
    }

    spi_metadata_ = fuchsia_hardware_spi_businfo::SpiBusMetadata(
        {.channels = std::move(channels), .bus_id = 0});

    return zx::ok();
  }

  FakeSpiImplServer& fake_spi_impl() { return fake_spi_impl_; }

 private:
  FakeSpiImplServer fake_spi_impl_;
  fdf_metadata::MetadataServer<fuchsia_hardware_spi_businfo::SpiBusMetadata> spi_metadata_server_;
  fdf_metadata::MetadataServer<fuchsia_scheduler::RoleName> scheduler_role_name_metadata_server_;
  std::optional<fuchsia_hardware_spi_businfo::SpiBusMetadata> spi_metadata_;
  std::optional<fuchsia_scheduler::RoleName> role_name_;
};

struct FixtureConfig {
 public:
  using DriverType = SpiDriver;
  using EnvironmentType = TestEnvironment;
};

class SpiDeviceTest : public ::testing::Test {
 public:
  void CreateSpiDevice(uint32_t channel_count) {
    driver_test().RunInEnvironmentTypeContext([channel_count](TestEnvironment& environment) {
      ASSERT_TRUE(environment.SetSpiChannelCount(channel_count).is_ok());
    });

    zx::event node_token;
    EXPECT_OK(zx::event::create(0, &node_token));

    zx_info_handle_basic_t info;
    EXPECT_OK(node_token.get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr));
    node_token_koid_ = info.koid;

    EXPECT_TRUE(driver_test()
                    .StartDriverWithCustomStartArgs(
                        [node_token = std::move(node_token)](fdf::DriverStartArgs& args) mutable {
                          spi_config::Config config{{
                              .enable_suspend = true,
                              .expose_debug_capabilities = true,
                          }};
                          args.config(config.ToVmo());
                          args.node_token(std::move(node_token));
                        })
                    .is_ok());

    bool all_children_added =
        driver_test().RunInNodeContext<bool>([channel_count](fdf_testing::TestNode& node) {
          if (node.children().begin() == node.children().end()) {
            return false;
          }
          auto& spi_bus = node.children().begin()->second;
          return spi_bus.children().size() == channel_count;
        });
    EXPECT_TRUE(all_children_added);
  }

  // Helper Methods that access fake_spi_impl
  void set_current_test_cs(uint32_t i) {
    driver_test().RunInEnvironmentTypeContext(
        [i](TestEnvironment& environment) { environment.fake_spi_impl().current_test_cs_ = i; });
  }
  void set_test_mode(FakeSpiImplServer::SpiTestMode test_mode) {
    driver_test().RunInEnvironmentTypeContext([test_mode](TestEnvironment& environment) {
      environment.fake_spi_impl().test_mode_ = test_mode;
    });
  }
  void set_corrupt_rx_actual(bool actual) {
    driver_test().RunInEnvironmentTypeContext([actual](TestEnvironment& environment) {
      environment.fake_spi_impl().corrupt_rx_actual_ = actual;
    });
  }
  bool vmos_released_since_last_call() {
    return driver_test().RunInEnvironmentTypeContext<bool>([](TestEnvironment& environment) {
      const bool value = environment.fake_spi_impl().vmos_released_since_last_call_;
      environment.fake_spi_impl().vmos_released_since_last_call_ = false;
      return value;
    });
  }

  void TearDown() override {
    zx::result<> result = driver_test().StopDriver();
    ASSERT_EQ(ZX_OK, result.status_value());
  }

  fdf_testing::BackgroundDriverTest<FixtureConfig>& driver_test() { return driver_test_; }

 protected:
  zx_koid_t node_token_koid_ = ZX_KOID_INVALID;

 private:
  fdf_testing::BackgroundDriverTest<FixtureConfig> driver_test_;
};

TEST_F(SpiDeviceTest, SpiTest) {
  constexpr std::array<const char*, 2> kExpectedNodeNames{"spi-0-0", "spi-0-1"};

  CreateSpiDevice(kExpectedNodeNames.size());

  // test it
  uint8_t txbuf[] = {0, 1, 2, 3, 4, 5, 6};
  uint8_t rxbuf[sizeof txbuf];

  uint32_t i = 0;
  for (auto name = kExpectedNodeNames.cbegin(); name != kExpectedNodeNames.cend(); name++, i++) {
    zx::result client = driver_test().Connect<fuchsia_hardware_spi::Service::Device>(*name);
    ASSERT_TRUE(client.is_ok());

    set_current_test_cs(i);

    set_test_mode(FakeSpiImplServer::SpiTestMode::kTransmit);
    EXPECT_OK(spilib_transmit(*client, txbuf, sizeof txbuf));

    set_test_mode(FakeSpiImplServer::SpiTestMode::kReceive);
    EXPECT_OK(spilib_receive(*client, rxbuf, sizeof rxbuf));

    set_test_mode(FakeSpiImplServer::SpiTestMode::kExchange);
    EXPECT_OK(spilib_exchange(*client, txbuf, rxbuf, sizeof txbuf));
  }
}

TEST_F(SpiDeviceTest, SpiFidlVmoTest) {
  using fuchsia_hardware_sharedmemory::wire::SharedVmoRight;

  constexpr std::array<const char*, 2> kExpectedNodeNames{"spi-0-0", "spi-0-1"};
  constexpr std::array<uint8_t, 7> kTestData{1, 2, 3, 4, 5, 6, 7};

  CreateSpiDevice(kExpectedNodeNames.size());

  fidl::WireSyncClient<fuchsia_hardware_spi::Device> cs0_client, cs1_client;

  {
    zx::result client =
        driver_test().Connect<fuchsia_hardware_spi::Service::Device>(kExpectedNodeNames[0]);
    ASSERT_TRUE(client.is_ok());
    cs0_client.Bind(std::move(client.value()));
  }

  {
    zx::result client =
        driver_test().Connect<fuchsia_hardware_spi::Service::Device>(kExpectedNodeNames[1]);
    ASSERT_TRUE(client.is_ok());
    cs1_client.Bind(std::move(client.value()));
  }

  zx::vmo cs0_vmo, cs1_vmo;
  ASSERT_OK(zx::vmo::create(4096, 0, &cs0_vmo));
  ASSERT_OK(zx::vmo::create(4096, 0, &cs1_vmo));

  {
    fuchsia_mem::wire::Range vmo = {.offset = 0, .size = 4096};
    ASSERT_OK(cs0_vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &vmo.vmo));
    auto result =
        cs0_client->RegisterVmo(1, std::move(vmo), SharedVmoRight::kRead | SharedVmoRight::kWrite);
    ASSERT_OK(result.status());
    EXPECT_TRUE(result.value().is_ok());
  }

  {
    fuchsia_mem::wire::Range vmo = {.offset = 0, .size = 4096};
    ASSERT_OK(cs1_vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &vmo.vmo));
    auto result =
        cs1_client->RegisterVmo(2, std::move(vmo), SharedVmoRight::kRead | SharedVmoRight::kWrite);
    ASSERT_OK(result.status());
    EXPECT_TRUE(result.value().is_ok());
  }

  ASSERT_OK(cs0_vmo.write(kTestData.data(), 1024, kTestData.size()));
  {
    auto result = cs0_client->Exchange(
        {
            .vmo_id = 1,
            .offset = 1024,
            .size = kTestData.size(),
        },
        {
            .vmo_id = 1,
            .offset = 2048,
            .size = kTestData.size(),
        });
    ASSERT_OK(result.status());
    EXPECT_TRUE(result.value().is_ok());

    std::array<uint8_t, kTestData.size()> buf;
    ASSERT_OK(cs0_vmo.read(buf.data(), 2048, buf.size()));
    EXPECT_EQ(buf, kTestData);
  }

  ASSERT_OK(cs1_vmo.write(kTestData.data(), 1024, kTestData.size()));
  {
    auto result = cs1_client->Transmit({
        .vmo_id = 2,
        .offset = 1024,
        .size = kTestData.size(),
    });
    ASSERT_OK(result.status());
    EXPECT_TRUE(result.value().is_ok());
  }

  {
    auto result = cs0_client->Receive({
        .vmo_id = 1,
        .offset = 1024,
        .size = kTestData.size(),
    });
    ASSERT_OK(result.status());
    EXPECT_TRUE(result.value().is_ok());

    std::array<uint8_t, kTestData.size()> buf;
    ASSERT_OK(cs0_vmo.read(buf.data(), 1024, buf.size()));
    EXPECT_EQ(buf, kTestData);
  }

  {
    auto result = cs0_client->UnregisterVmo(1);
    ASSERT_OK(result.status());
    EXPECT_TRUE(result.value().is_ok());
  }

  {
    auto result = cs1_client->UnregisterVmo(2);
    ASSERT_OK(result.status());
    EXPECT_TRUE(result.value().is_ok());
  }
}

TEST_F(SpiDeviceTest, SpiFidlVectorTest) {
  constexpr std::array<const char*, 2> kExpectedNodeNames{"spi-0-0", "spi-0-1"};
  fidl::WireSyncClient<fuchsia_hardware_spi::Device> cs0_client, cs1_client;

  CreateSpiDevice(kExpectedNodeNames.size());

  {
    zx::result client =
        driver_test().Connect<fuchsia_hardware_spi::Service::Device>(kExpectedNodeNames[0]);
    ASSERT_TRUE(client.is_ok());
    cs0_client.Bind(std::move(client.value()));
  }

  {
    zx::result client =
        driver_test().Connect<fuchsia_hardware_spi::Service::Device>(kExpectedNodeNames[1]);
    ASSERT_TRUE(client.is_ok());
    cs1_client.Bind(std::move(client.value()));
  }

  std::vector<uint8_t> test_data{1, 2, 3, 4, 5, 6, 7};

  set_current_test_cs(0);
  set_test_mode(FakeSpiImplServer::SpiTestMode::kTransmit);
  {
    auto tx_buffer = fidl::VectorView<uint8_t>::FromExternal(test_data);
    auto result = cs0_client->TransmitVector(tx_buffer);
    ASSERT_OK(result.status());
    EXPECT_OK(result.value().status);
  }

  set_current_test_cs(1);
  set_test_mode(FakeSpiImplServer::SpiTestMode::kReceive);
  {
    auto result = cs1_client->ReceiveVector(static_cast<uint32_t>(test_data.size()));
    ASSERT_OK(result.status());
    EXPECT_OK(result.value().status);
    const std::vector<uint8_t> actual{result.value().data.cbegin(), result.value().data.cend()};
    EXPECT_EQ(actual, test_data);
  }

  set_current_test_cs(0);
  set_test_mode(FakeSpiImplServer::SpiTestMode::kExchange);
  {
    auto tx_buffer = fidl::VectorView<uint8_t>::FromExternal(test_data);
    auto result = cs0_client->ExchangeVector(tx_buffer);
    ASSERT_OK(result.status());
    EXPECT_OK(result.value().status);
    const std::vector<uint8_t> actual{result.value().rxdata.cbegin(), result.value().rxdata.cend()};
    EXPECT_EQ(actual, test_data);
  }
}

TEST_F(SpiDeviceTest, SpiFidlVectorErrorTest) {
  constexpr std::array<const char*, 2> kExpectedNodeNames{"spi-0-0", "spi-0-1"};
  fidl::WireSyncClient<fuchsia_hardware_spi::Device> cs0_client, cs1_client;

  CreateSpiDevice(kExpectedNodeNames.size());

  {
    zx::result client =
        driver_test().Connect<fuchsia_hardware_spi::Service::Device>(kExpectedNodeNames[0]);
    ASSERT_TRUE(client.is_ok());
    cs0_client.Bind(std::move(client.value()));
  }

  {
    zx::result client =
        driver_test().Connect<fuchsia_hardware_spi::Service::Device>(kExpectedNodeNames[1]);
    ASSERT_TRUE(client.is_ok());
    cs1_client.Bind(std::move(client.value()));
  }

  set_corrupt_rx_actual(true);

  uint8_t test_data[] = {1, 2, 3, 4, 5, 6, 7};

  set_current_test_cs(0);
  set_test_mode(FakeSpiImplServer::SpiTestMode::kTransmit);
  {
    auto tx_buffer = fidl::VectorView<uint8_t>::FromExternal(test_data);
    auto result = cs0_client->TransmitVector(tx_buffer);
    ASSERT_OK(result.status());
    EXPECT_OK(result.value().status);
  }

  set_current_test_cs(1);
  set_test_mode(FakeSpiImplServer::SpiTestMode::kReceive);
  {
    auto result = cs1_client->ReceiveVector(sizeof(test_data));
    ASSERT_OK(result.status());
    EXPECT_EQ(result.value().status, ZX_ERR_INTERNAL);
    EXPECT_EQ(result.value().data.size(), 0ul);
  }

  set_current_test_cs(0);
  set_test_mode(FakeSpiImplServer::SpiTestMode::kExchange);
  {
    auto tx_buffer = fidl::VectorView<uint8_t>::FromExternal(test_data);
    auto result = cs0_client->ExchangeVector(tx_buffer);
    ASSERT_OK(result.status());
    EXPECT_EQ(result.value().status, ZX_ERR_INTERNAL);
    EXPECT_EQ(result.value().rxdata.size(), 0ul);
  }
}

TEST_F(SpiDeviceTest, AssertCsWithSiblingTest) {
  constexpr std::array<const char*, 2> kExpectedNodeNames{"spi-0-0", "spi-0-1"};
  fidl::WireSyncClient<fuchsia_hardware_spi::Device> cs0_client, cs1_client;

  CreateSpiDevice(kExpectedNodeNames.size());

  {
    zx::result client =
        driver_test().Connect<fuchsia_hardware_spi::Service::Device>(kExpectedNodeNames[0]);
    ASSERT_TRUE(client.is_ok());
    cs0_client.Bind(std::move(client.value()));
  }

  {
    zx::result client =
        driver_test().Connect<fuchsia_hardware_spi::Service::Device>(kExpectedNodeNames[1]);
    ASSERT_TRUE(client.is_ok());
    cs1_client.Bind(std::move(client.value()));
  }

  {
    auto result = cs0_client->CanAssertCs();
    ASSERT_OK(result.status());
    ASSERT_FALSE(result.value().can);
  }

  {
    auto result = cs1_client->CanAssertCs();
    ASSERT_OK(result.status());
    ASSERT_FALSE(result.value().can);
  }

  {
    auto result = cs0_client->AssertCs();
    ASSERT_OK(result.status());
    ASSERT_EQ(result.value().status, ZX_ERR_NOT_SUPPORTED);
  }

  {
    auto result = cs1_client->AssertCs();
    ASSERT_OK(result.status());
    ASSERT_EQ(result.value().status, ZX_ERR_NOT_SUPPORTED);
  }

  {
    auto result = cs0_client->DeassertCs();
    ASSERT_OK(result.status());
    ASSERT_EQ(result.value().status, ZX_ERR_NOT_SUPPORTED);
  }

  {
    auto result = cs1_client->DeassertCs();
    ASSERT_OK(result.status());
    ASSERT_EQ(result.value().status, ZX_ERR_NOT_SUPPORTED);
  }
}

TEST_F(SpiDeviceTest, AssertCsNoSiblingTest) {
  constexpr std::array<const char*, 1> kExpectedNodeNames{"spi-0-0"};

  fidl::WireSyncClient<fuchsia_hardware_spi::Device> cs0_client;

  CreateSpiDevice(kExpectedNodeNames.size());

  {
    zx::result client =
        driver_test().Connect<fuchsia_hardware_spi::Service::Device>(kExpectedNodeNames[0]);
    ASSERT_TRUE(client.is_ok());
    cs0_client.Bind(std::move(client.value()));
  }

  {
    auto result = cs0_client->CanAssertCs();
    ASSERT_OK(result.status());
    ASSERT_TRUE(result.value().can);
  }

  {
    auto result = cs0_client->AssertCs();
    ASSERT_OK(result.status());
    ASSERT_OK(result.value().status);
  }

  {
    auto result = cs0_client->DeassertCs();
    ASSERT_OK(result.status());
    ASSERT_OK(result.value().status);
  }
}

TEST_F(SpiDeviceTest, OneClient) {
  constexpr std::array<const char*, 1> kExpectedNodeNames{"spi-0-0"};
  const std::vector<std::string> kDevfsPath({"spi", kExpectedNodeNames[0]});

  fidl::WireSyncClient<fuchsia_hardware_spi::Device> cs0_client;

  CreateSpiDevice(kExpectedNodeNames.size());

  // Establish a FIDL connection and verify that it works.
  {
    zx::result client =
        driver_test().Connect<fuchsia_hardware_spi::Service::Device>(kExpectedNodeNames[0]);
    ASSERT_TRUE(client.is_ok());
    cs0_client.Bind(std::move(client.value()));
  }

  {
    // Synchronize with the driver and environment by making a FIDL call that hits the fake spiimpl
    // device. After this we can verify that the one-way call to release VMOs has been made.
    auto result = cs0_client->AssertCs();
    ASSERT_OK(result.status());
  }

  // VMOs should have been released in the driver's Start hook.
  EXPECT_TRUE(vmos_released_since_last_call());

  {
    auto result = cs0_client->CanAssertCs();
    ASSERT_OK(result.status());
    ASSERT_TRUE(result.value().can);
  }

  // Trying to make a new connection should fail.
  {
    zx::result client =
        driver_test().Connect<fuchsia_hardware_spi::Service::Device>(kExpectedNodeNames[0]);
    ASSERT_TRUE(client.is_ok());
    fidl::WireSyncClient cs0_client_1(std::move(client.value()));
    EXPECT_EQ(cs0_client_1->UnregisterVmo(1).status(), ZX_ERR_PEER_CLOSED);
  }

  EXPECT_FALSE(vmos_released_since_last_call());

  // Close the first client so that another one can connect.
  cs0_client = {};

  // We don't know when the driver will be ready for a new client, just loop
  // until the connection is established.
  for (;;) {
    zx::result client =
        driver_test().Connect<fuchsia_hardware_spi::Service::Device>(kExpectedNodeNames[0]);
    ASSERT_TRUE(client.is_ok());
    cs0_client.Bind(std::move(client.value()));

    // By the time this connection succeeds, the SPI device has already called to its parent to
    // release registered VMOs. UnregisterVmo will then make a synchronous call to the parent, so
    // waiting for the UnregisterVmo response is sufficient to guarantee that the first call to the
    // parent (the fake spiimpl device) has been processed, and that vmos_released_since_last_call
    // will return the correct value.
    auto result = cs0_client->UnregisterVmo(1);
    if (result.ok()) {
      break;
    }
    EXPECT_EQ(result.status(), ZX_ERR_PEER_CLOSED);
    cs0_client = {};
  }

  EXPECT_TRUE(vmos_released_since_last_call());

  // OpenSession should fail when another client is connected.
  {
    zx::result controller =
        driver_test().ConnectThroughDevfs<fuchsia_hardware_spi::Controller>(kDevfsPath);
    ASSERT_TRUE(controller.is_ok());
    {
      auto [device, server] = fidl::Endpoints<fuchsia_hardware_spi::Device>::Create();
      ASSERT_OK(fidl::WireCall(*controller)->OpenSession(std::move(server)).status());
      ASSERT_EQ(fidl::WireCall(device)->CanAssertCs().status(), ZX_ERR_PEER_CLOSED);
    }
  }

  // Close the first client and make sure OpenSession now works.
  cs0_client = {};

  fidl::ClientEnd<fuchsia_hardware_spi::Device> device;
  while (true) {
    zx::result controller =
        driver_test().ConnectThroughDevfs<fuchsia_hardware_spi::Controller>(kDevfsPath);
    ASSERT_TRUE(controller.is_ok());
    {
      zx::result server = fidl::CreateEndpoints<fuchsia_hardware_spi::Device>(&device);
      ASSERT_OK(server.status_value());
      ASSERT_OK(fidl::WireCall(*controller)->OpenSession(std::move(server.value())).status());
      auto result = fidl::WireCall(device)->UnregisterVmo(1);
      if (result.ok()) {
        break;
      }
      ASSERT_EQ(result.status(), ZX_ERR_PEER_CLOSED);
    }
  }

  EXPECT_TRUE(vmos_released_since_last_call());

  // FIDL clients shouldn't be able to connect, and calling OpenSession a second time should fail.
  {
    zx::result client =
        driver_test().Connect<fuchsia_hardware_spi::Service::Device>(kExpectedNodeNames[0]);
    ASSERT_TRUE(client.is_ok());
    fidl::WireSyncClient cs0_client_1(std::move(client.value()));
    EXPECT_EQ(cs0_client_1->CanAssertCs().status(), ZX_ERR_PEER_CLOSED);
  }

  {
    zx::result controller =
        driver_test().ConnectThroughDevfs<fuchsia_hardware_spi::Controller>(kDevfsPath);
    ASSERT_TRUE(controller.is_ok());
    {
      auto [device, server] = fidl::Endpoints<fuchsia_hardware_spi::Device>::Create();
      ASSERT_OK(fidl::WireCall(*controller)->OpenSession(std::move(server)).status());
      ASSERT_EQ(fidl::WireCall(device)->CanAssertCs().status(), ZX_ERR_PEER_CLOSED);
    }
  }

  // Close the open session and make sure that a new client can now connect.
  device = {};

  for (;;) {
    zx::result client =
        driver_test().Connect<fuchsia_hardware_spi::Service::Device>(kExpectedNodeNames[0]);
    ASSERT_TRUE(client.is_ok());
    cs0_client.Bind(std::move(client.value()));

    auto result = cs0_client->UnregisterVmo(1);
    if (result.ok()) {
      break;
    }

    EXPECT_EQ(result.status(), ZX_ERR_PEER_CLOSED);
    cs0_client = {};
  }

  EXPECT_TRUE(vmos_released_since_last_call());
}

TEST_F(SpiDeviceTest, SchedulerRoleName) {
  constexpr std::array<const char*, 1> kExpectedNodeNames{"spi-0-0"};

  // Add scheduler role metadata that will cause the core driver to create a new driver dispatcher.
  // Verify that FIDL calls can still be made.
  driver_test().RunInEnvironmentTypeContext([](TestEnvironment& environment) {
    environment.SetSchedulerRoleName("no.such.scheduler.role");
  });

  fidl::WireSyncClient<fuchsia_hardware_spi::Device> cs0_client;

  CreateSpiDevice(kExpectedNodeNames.size());

  {
    zx::result client =
        driver_test().Connect<fuchsia_hardware_spi::Service::Device>(kExpectedNodeNames[0]);
    ASSERT_TRUE(client.is_ok());
    ASSERT_OK(client.status_value());
    cs0_client.Bind(std::move(client.value()));
  }

  {
    auto result = cs0_client->AssertCs();
    ASSERT_OK(result.status());
    EXPECT_OK(result.value().status);
  }
}

TEST_F(SpiDeviceTest, TestService) {
  constexpr std::array<const char*, 1> kExpectedNodeNames{"spi-0-0"};

  CreateSpiDevice(kExpectedNodeNames.size());

  zx::result test_client_end = driver_test().Connect<fuchsia_hardware_spi::TestService::Test>();
  ASSERT_TRUE(test_client_end.is_ok());
  fidl::WireSyncClient<fuchsia_hardware_spi::Test> test_client(std::move(test_client_end.value()));

  {
    auto result = test_client->Get();
    ASSERT_OK(result.status());
    ASSERT_TRUE(result.value().is_ok());
    zx_info_handle_basic_t info;
    ASSERT_OK(result.value()->token.get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr,
                                             nullptr));
    EXPECT_EQ(info.koid, node_token_koid_);
  }

  auto [device_client, device_server] = fidl::Endpoints<fuchsia_hardware_spi::Device>::Create();

  auto connect_result = test_client->ConnectSpiLoopback(std::move(device_server));
  ASSERT_OK(connect_result.status());
  ASSERT_TRUE(connect_result.value().is_ok());

  set_current_test_cs(fuchsia_hardware_spiimpl::kLoopbackChipSelect);
  set_test_mode(FakeSpiImplServer::SpiTestMode::kTransmit);

  uint8_t txbuf[] = {0, 1, 2, 3, 4, 5, 6};
  EXPECT_OK(spilib_transmit(device_client, txbuf, sizeof(txbuf)));
}

}  // namespace spi
