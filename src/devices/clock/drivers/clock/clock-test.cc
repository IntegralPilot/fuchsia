// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "clock.h"

#include <fidl/fuchsia.driver.metadata/cpp/fidl.h>
#include <fidl/fuchsia.hardware.clock/cpp/fidl.h>
#include <fidl/fuchsia.hardware.clockimpl/cpp/fidl.h>
#include <lib/async-default/include/lib/async/default.h>
#include <lib/ddk/metadata.h>
#include <lib/driver/metadata/cpp/metadata_server.h>
#include <lib/driver/testing/cpp/driver_test.h>
#include <lib/stdcompat/span.h>

#include <array>
#include <format>
#include <optional>

#include "src/lib/testing/predicates/status.h"

namespace {

class GenericMetadataServer final : public fidl::WireServer<fuchsia_driver_metadata::Metadata> {
 public:
  zx::result<> Serve(fdf::OutgoingDirectory& outgoing, async_dispatcher_t* dispatcher,
                     std::string service_name, fuchsia_driver_metadata::Dictionary metadata) {
    auto persisted = fidl::Persist(metadata);
    if (persisted.is_error()) {
      return zx::error(persisted.error_value().status());
    }
    metadata_bytes_ = std::move(persisted.value());

    fuchsia_driver_metadata::Service::InstanceHandler handler({
        .metadata = bindings_.CreateHandler(this, dispatcher, fidl::kIgnoreBindingClosure),
    });

    return outgoing.component().AddService(std::move(handler), std::move(service_name));
  }

 private:
  void GetPersistedMetadata(GetPersistedMetadataCompleter::Sync& completer) override {
    completer.ReplySuccess(fidl::VectorView<uint8_t>::FromExternal(metadata_bytes_));
  }

  std::vector<uint8_t> metadata_bytes_;
  fidl::ServerBindingGroup<fuchsia_driver_metadata::Metadata> bindings_;
};

class FakeClockImpl : public fdf::WireServer<fuchsia_hardware_clockimpl::ClockImpl> {
 public:
  struct FakeClock {
    std::optional<bool> enabled;
    std::optional<uint64_t> rate_hz;
    std::optional<uint32_t> input_idx;
  };

  fuchsia_hardware_clockimpl::Service::InstanceHandler GetInstanceHandler() {
    return fuchsia_hardware_clockimpl::Service::InstanceHandler(
        {.device = bindings_.CreateHandler(this, fdf::Dispatcher::GetCurrent()->get(),
                                           fidl::kIgnoreBindingClosure)});
  }

  cpp20::span<const FakeClock> clocks() const { return {clocks_.data(), clocks_.size()}; }

 private:
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_hardware_clockimpl::ClockImpl> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override {}

  void Enable(fuchsia_hardware_clockimpl::wire::ClockImplEnableRequest* request, fdf::Arena& arena,
              EnableCompleter::Sync& completer) override {
    if (request->id >= clocks_.size()) {
      completer.buffer(arena).ReplyError(ZX_ERR_OUT_OF_RANGE);
      return;
    }
    clocks_[request->id].enabled.emplace(true);
    completer.buffer(arena).ReplySuccess();
  }

  void Disable(fuchsia_hardware_clockimpl::wire::ClockImplDisableRequest* request,
               fdf::Arena& arena, DisableCompleter::Sync& completer) override {
    if (request->id >= clocks_.size()) {
      completer.buffer(arena).ReplyError(ZX_ERR_OUT_OF_RANGE);
      return;
    }
    clocks_[request->id].enabled.emplace(false);
    completer.buffer(arena).ReplySuccess();
  }

  void IsEnabled(fuchsia_hardware_clockimpl::wire::ClockImplIsEnabledRequest* request,
                 fdf::Arena& arena, IsEnabledCompleter::Sync& completer) override {
    completer.buffer(arena).ReplyError(ZX_ERR_NOT_SUPPORTED);
  }

  void SetRate(fuchsia_hardware_clockimpl::wire::ClockImplSetRateRequest* request,
               fdf::Arena& arena, SetRateCompleter::Sync& completer) override {
    if (request->id >= clocks_.size()) {
      completer.buffer(arena).ReplyError(ZX_ERR_OUT_OF_RANGE);
      return;
    }
    clocks_[request->id].rate_hz.emplace(request->hz);
    completer.buffer(arena).ReplySuccess();
  }

  void QuerySupportedRate(
      fuchsia_hardware_clockimpl::wire::ClockImplQuerySupportedRateRequest* request,
      fdf::Arena& arena, QuerySupportedRateCompleter::Sync& completer) override {
    completer.buffer(arena).ReplyError(ZX_ERR_NOT_SUPPORTED);
  }

  void GetRate(fuchsia_hardware_clockimpl::wire::ClockImplGetRateRequest* request,
               fdf::Arena& arena, GetRateCompleter::Sync& completer) override {
    completer.buffer(arena).ReplyError(ZX_ERR_NOT_SUPPORTED);
  }

  void SetInput(fuchsia_hardware_clockimpl::wire::ClockImplSetInputRequest* request,
                fdf::Arena& arena, SetInputCompleter::Sync& completer) override {
    if (request->id >= clocks_.size()) {
      completer.buffer(arena).ReplyError(ZX_ERR_OUT_OF_RANGE);
      return;
    }
    clocks_[request->id].input_idx.emplace(request->idx);
    completer.buffer(arena).ReplySuccess();
  }

  void GetNumInputs(fuchsia_hardware_clockimpl::wire::ClockImplGetNumInputsRequest* request,
                    fdf::Arena& arena, GetNumInputsCompleter::Sync& completer) override {
    completer.buffer(arena).ReplyError(ZX_ERR_NOT_SUPPORTED);
  }

  void GetInput(fuchsia_hardware_clockimpl::wire::ClockImplGetInputRequest* request,
                fdf::Arena& arena, GetInputCompleter::Sync& completer) override {
    completer.buffer(arena).ReplyError(ZX_ERR_NOT_SUPPORTED);
  }

  void GetClockProperties(fdf::Arena& arena,
                          GetClockPropertiesCompleter::Sync& completer) override {
    completer.buffer(arena).ReplyError(ZX_ERR_NOT_SUPPORTED);
  }

  std::array<FakeClock, 6> clocks_;

  fdf::ServerBindingGroup<fuchsia_hardware_clockimpl::ClockImpl> bindings_;
};

class Environment : public fdf_testing::Environment {
 public:
  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override {
    auto* dispatcher = fdf::Dispatcher::GetCurrent()->async_dispatcher();

    {
      zx::result result = to_driver_vfs.AddService<fuchsia_hardware_clockimpl::Service>(
          clock_impl_.GetInstanceHandler());
      if (result.is_error()) {
        return result.take_error();
      }
    }

    if (clock_init_metadata_.has_value()) {
      if (zx::result result = clock_init_metadata_server_.Serve(to_driver_vfs, dispatcher,
                                                                clock_init_metadata_.value());
          result.is_error()) {
        return result.take_error();
      }
    }

    if (clock_ids_metadata_.has_value()) {
      if (zx::result result = clock_ids_metadata_server_.Serve(to_driver_vfs, dispatcher,
                                                               clock_ids_metadata_.value());
          result.is_error()) {
        return result.take_error();
      }
    }

    if (generic_metadata_.has_value()) {
      if (zx::result result = generic_metadata_server_.Serve(
              to_driver_vfs, dispatcher, "fuchsia.hardware.clockimpl.ClockIdsMetadata",
              std::move(generic_metadata_.value()));
          result.is_error()) {
        return result.take_error();
      }
    }

    return zx::ok();
  }

  void Init(fuchsia_hardware_clockimpl::InitMetadata clock_init_metadata,
            fuchsia_hardware_clockimpl::ClockIdsMetadata clock_ids_metadata) {
    clock_init_metadata_ = std::move(clock_init_metadata);
    clock_ids_metadata_ = std::move(clock_ids_metadata);
  }

  void InitGeneric(std::vector<fuchsia_hardware_clockimpl::ClockNodeDescriptor> clock_nodes) {
    std::vector<fuchsia_driver_metadata::DictionaryEntry> entries;
    entries.push_back(fuchsia_driver_metadata::DictionaryEntry(
        "clock_nodes._count", fuchsia_driver_metadata::DictionaryValue::WithInt64(
                                  static_cast<int64_t>(clock_nodes.size()))));
    for (size_t i = 0; i < clock_nodes.size(); ++i) {
      const auto& node = clock_nodes[i];
      if (node.clock_id().has_value()) {
        entries.push_back(fuchsia_driver_metadata::DictionaryEntry(
            std::format("clock_nodes.{}.id", i),
            fuchsia_driver_metadata::DictionaryValue::WithInt64(
                static_cast<int64_t>(node.clock_id().value()))));
      }
      if (node.node_id().has_value()) {
        entries.push_back(fuchsia_driver_metadata::DictionaryEntry(
            std::format("clock_nodes.{}.node_id", i),
            fuchsia_driver_metadata::DictionaryValue::WithInt64(
                static_cast<int64_t>(node.node_id().value()))));
      }
      if (node.name().has_value()) {
        entries.push_back(fuchsia_driver_metadata::DictionaryEntry(
            std::format("clock_nodes.{}.name", i),
            fuchsia_driver_metadata::DictionaryValue::WithStr(node.name().value())));
      }
    }
    generic_metadata_ = fuchsia_driver_metadata::Dictionary{{.entries = std::move(entries)}};
  }

  FakeClockImpl& clock_impl() { return clock_impl_; }

 private:
  FakeClockImpl clock_impl_;
  fdf_metadata::MetadataServer<fuchsia_hardware_clockimpl::InitMetadata>
      clock_init_metadata_server_;
  fdf_metadata::MetadataServer<fuchsia_hardware_clockimpl::ClockIdsMetadata>
      clock_ids_metadata_server_;
  GenericMetadataServer generic_metadata_server_;
  std::optional<fuchsia_hardware_clockimpl::InitMetadata> clock_init_metadata_;
  std::optional<fuchsia_hardware_clockimpl::ClockIdsMetadata> clock_ids_metadata_;
  std::optional<fuchsia_driver_metadata::Dictionary> generic_metadata_;
};

class ClockTestConfig {
 public:
  using DriverType = ClockDriver;
  using EnvironmentType = Environment;
};

class ClockTest : public ::testing::Test {
 public:
  void TearDown() override { ASSERT_OK(driver_test_.StopDriver()); }

 protected:
  void StartDriver(fuchsia_hardware_clockimpl::InitMetadata clock_init_metadata,
                   fuchsia_hardware_clockimpl::ClockIdsMetadata clock_ids_metadata,
                   zx_status_t expected_start_driver_status = ZX_OK) {
    driver_test_.RunInEnvironmentTypeContext(
        [clock_init_metadata = std::move(clock_init_metadata),
         clock_ids_metadata = std::move(clock_ids_metadata)](Environment& environment) mutable {
          environment.Init(std::move(clock_init_metadata), std::move(clock_ids_metadata));
        });
    ASSERT_EQ(driver_test_.StartDriver().status_value(), expected_start_driver_status);
  }

  void InitGeneric(std::vector<fuchsia_hardware_clockimpl::ClockNodeDescriptor> clock_nodes) {
    driver_test().RunInEnvironmentTypeContext(
        [&](Environment& environment) mutable { environment.InitGeneric(std::move(clock_nodes)); });
    ASSERT_OK(driver_test().StartDriver());
  }

  std::vector<FakeClockImpl::FakeClock> GetClocks() {
    std::vector<FakeClockImpl::FakeClock> clocks;
    driver_test_.RunInEnvironmentTypeContext([&dst = clocks](Environment& environment) mutable {
      auto src = environment.clock_impl().clocks();
      dst = std::vector<FakeClockImpl::FakeClock>(src.begin(), src.end());
    });
    return clocks;
  }

  fdf_testing::BackgroundDriverTest<ClockTestConfig>& driver_test() { return driver_test_; }

 private:
  fdf_testing::BackgroundDriverTest<ClockTestConfig> driver_test_;
};

TEST_F(ClockTest, ConfigureClocks) {
  fuchsia_hardware_clockimpl::InitMetadata metadata{
      {.steps = {{{.id = 3, .call = fuchsia_hardware_clockimpl::InitCall::WithEnable({})}},
                 {{.id = 3, .call = fuchsia_hardware_clockimpl::InitCall::WithInputIdx(100)}},
                 {{.id = 3, .call = fuchsia_hardware_clockimpl::InitCall::WithRateHz(500'000'000)}},
                 {{.id = 1, .call = fuchsia_hardware_clockimpl::InitCall::WithEnable({})}},
                 {{.id = 1, .call = fuchsia_hardware_clockimpl::InitCall::WithInputIdx(99)}},
                 {{.id = 1, .call = fuchsia_hardware_clockimpl::InitCall::WithRateHz(400'000'000)}},
                 {{.id = 1, .call = fuchsia_hardware_clockimpl::InitCall::WithDisable({})}},
                 {{.id = 1, .call = fuchsia_hardware_clockimpl::InitCall::WithInputIdx(101)}},
                 {{.id = 1, .call = fuchsia_hardware_clockimpl::InitCall::WithRateHz(600'000'000)}},
                 {{.id = 2, .call = fuchsia_hardware_clockimpl::InitCall::WithDisable({})}},
                 {{.id = 2, .call = fuchsia_hardware_clockimpl::InitCall::WithInputIdx(1)}},
                 {{.id = 4, .call = fuchsia_hardware_clockimpl::InitCall::WithRateHz(100'000)}}}}};

  StartDriver(metadata, {});

  auto clocks = GetClocks();
  ASSERT_TRUE(clocks[3].enabled.has_value());
  EXPECT_TRUE(clocks[3].enabled.value());

  ASSERT_TRUE(clocks[3].input_idx.has_value());
  EXPECT_EQ(clocks[3].input_idx.value(), 100u);

  ASSERT_TRUE(clocks[3].rate_hz.has_value());
  EXPECT_EQ(clocks[3].rate_hz.value(), 500'000'000u);

  ASSERT_TRUE(clocks[1].enabled.has_value());
  EXPECT_FALSE(clocks[1].enabled.value());

  ASSERT_TRUE(clocks[1].input_idx.has_value());
  EXPECT_EQ(clocks[1].input_idx.value(), 101u);

  ASSERT_TRUE(clocks[1].rate_hz.has_value());
  EXPECT_EQ(clocks[1].rate_hz.value(), 600'000'000u);

  ASSERT_TRUE(clocks[2].enabled.has_value());
  EXPECT_FALSE(clocks[2].enabled.value());

  ASSERT_TRUE(clocks[2].input_idx.has_value());
  EXPECT_EQ(clocks[2].input_idx.value(), 1u);

  EXPECT_FALSE(clocks[2].rate_hz.has_value());

  ASSERT_TRUE(clocks[4].rate_hz.has_value());
  EXPECT_EQ(clocks[4].rate_hz.value(), 100'000u);

  EXPECT_FALSE(clocks[4].enabled.has_value());
  EXPECT_FALSE(clocks[4].input_idx.has_value());

  EXPECT_FALSE(clocks[0].enabled.has_value());
  EXPECT_FALSE(clocks[0].rate_hz.has_value());
  EXPECT_FALSE(clocks[0].input_idx.has_value());

  EXPECT_FALSE(clocks[5].enabled.has_value());
  EXPECT_FALSE(clocks[5].rate_hz.has_value());
  EXPECT_FALSE(clocks[5].input_idx.has_value());
}

TEST_F(ClockTest, ConfigureClocksError) {
  fuchsia_hardware_clockimpl::InitMetadata metadata{
      {.steps = {
           {{.id = 3, .call = fuchsia_hardware_clockimpl::InitCall::WithEnable({})}},
           {{.id = 3, .call = fuchsia_hardware_clockimpl::InitCall::WithInputIdx(100)}},
           {{.id = 3, .call = fuchsia_hardware_clockimpl::InitCall::WithRateHz(500'000'000)}},
           {{.id = 1, .call = fuchsia_hardware_clockimpl::InitCall::WithEnable({})}},
           // This step should return an error due to the clock index being out of range.
           {{.id = 10, .call = fuchsia_hardware_clockimpl::InitCall::WithInputIdx(99)}},
           {{.id = 1, .call = fuchsia_hardware_clockimpl::InitCall::WithRateHz(400'000'000)}},
           {{.id = 2, .call = fuchsia_hardware_clockimpl::InitCall::WithDisable({})}},
           {{.id = 2, .call = fuchsia_hardware_clockimpl::InitCall::WithInputIdx(1)}},
           {{.id = 4, .call = fuchsia_hardware_clockimpl::InitCall::WithRateHz(100'000)}},
       }}};

  StartDriver(metadata, {}, ZX_ERR_OUT_OF_RANGE);
}

TEST_F(ClockTest, HandleDuplicates) {
  fuchsia_hardware_clockimpl::ClockIdsMetadata metadata{
      {.clock_nodes = std::vector<fuchsia_hardware_clockimpl::ClockNodeDescriptor>{}}};

  ASSERT_TRUE(metadata.clock_nodes().has_value());

  metadata.clock_nodes()->emplace_back(
      fuchsia_hardware_clockimpl::ClockNodeDescriptor{{.clock_id = 2, .node_id = 1}});
  metadata.clock_nodes()->emplace_back(
      fuchsia_hardware_clockimpl::ClockNodeDescriptor{{.clock_id = 1}});
  metadata.clock_nodes()->emplace_back(
      fuchsia_hardware_clockimpl::ClockNodeDescriptor{{.clock_id = 2, .node_id = 3}});

  StartDriver({}, metadata);

  // No suffix is added if this is the only instance.
  zx::result clk1_client_end =
      driver_test().Connect<fuchsia_hardware_clock::Service::Clock>("clock-1");
  EXPECT_TRUE(clk1_client_end.is_ok());

  // Suffixes added for duplicate entries.
  zx::result clk2_0_client_end =
      driver_test().Connect<fuchsia_hardware_clock::Service::Clock>("clock-2_1");
  zx::result clk2_1_client_end =
      driver_test().Connect<fuchsia_hardware_clock::Service::Clock>("clock-2_3");
  EXPECT_TRUE(clk2_0_client_end.is_ok());
  EXPECT_TRUE(clk2_1_client_end.is_ok());

  fidl::WireSyncClient<fuchsia_hardware_clock::Clock> clk1_client;
  fidl::WireSyncClient<fuchsia_hardware_clock::Clock> clk2_0_client;
  fidl::WireSyncClient<fuchsia_hardware_clock::Clock> clk2_1_client;

  clk1_client.Bind(std::move(clk1_client_end.value()));
  clk2_0_client.Bind(std::move(clk2_0_client_end.value()));
  clk2_1_client.Bind(std::move(clk2_1_client_end.value()));

  {
    auto result = clk1_client->SetRate(1000u);
    ASSERT_TRUE(result.ok());
  }

  {
    auto result = clk2_0_client->SetRate(1234u);
    ASSERT_TRUE(result.ok());
  }

  driver_test().runtime().RunUntilIdle();

  // Make sure that clk2_0_client and clk2_1_client refer to the same physical clock.
  {
    auto result = clk2_0_client->SetRate(4321u);
    ASSERT_TRUE(result.ok());
  }

  driver_test().runtime().RunUntilIdle();

  auto clocks = GetClocks();
  ASSERT_TRUE(clocks[1].rate_hz.has_value());
  EXPECT_EQ(clocks[1].rate_hz.value(), 1000u);

  ASSERT_TRUE(clocks[2].rate_hz.has_value());
  EXPECT_EQ(clocks[2].rate_hz.value(), 4321u);
}

TEST_F(ClockTest, GenericMetadataTest) {
  std::vector<fuchsia_hardware_clockimpl::ClockNodeDescriptor> nodes;
  nodes.push_back(fuchsia_hardware_clockimpl::ClockNodeDescriptor{
      {.clock_id = 1, .node_id = 10, .name = "clk-a"}});
  nodes.push_back(fuchsia_hardware_clockimpl::ClockNodeDescriptor{
      {.clock_id = 2, .node_id = 20, .name = "clk-b"}});

  InitGeneric(std::move(nodes));

  zx::result clk1 = driver_test().Connect<fuchsia_hardware_clock::Service::Clock>("clock-1_10");
  EXPECT_TRUE(clk1.is_ok());

  zx::result clk2 = driver_test().Connect<fuchsia_hardware_clock::Service::Clock>("clock-2_20");
  EXPECT_TRUE(clk2.is_ok());
}

}  // namespace
