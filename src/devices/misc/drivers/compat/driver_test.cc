// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/misc/drivers/compat/driver.h"

#include <dirent.h>
#include <fidl/fuchsia.device.fs/cpp/wire_test_base.h>
#include <fidl/fuchsia.driver.framework/cpp/wire_test_base.h>
#include <fidl/fuchsia.io/cpp/wire_test_base.h>
#include <fidl/fuchsia.kernel/cpp/wire_test_base.h>
#include <fidl/fuchsia.logger/cpp/wire_test_base.h>
#include <fidl/fuchsia.scheduler/cpp/wire_test_base.h>
#include <fidl/fuchsia.system.state/cpp/wire_test_base.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/default.h>
#include <lib/async_patterns/testing/cpp/dispatcher_bound.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/driver/compat/cpp/symbols.h>
#include <lib/driver/testing/cpp/test_node.h>
#include <lib/fdf/testing.h>
#include <lib/fdio/directory.h>
#include <lib/fit/defer.h>
#include <lib/sync/cpp/completion.h>
#include <zircon/dlfcn.h>

#include <cstdarg>
#include <unordered_set>

#include <fbl/unique_fd.h>
#include <gtest/gtest.h>

#include "fidl/fuchsia.io/cpp/natural_types.h"
#include "lib/driver/testing/cpp/driver_runtime.h"
#include "src/devices/misc/drivers/compat/compat_driver_server.h"
#include "src/devices/misc/drivers/compat/loader.h"
#include "src/devices/misc/drivers/compat/v1_test.h"

namespace fdata = fuchsia_data;
namespace fdf {

using namespace fuchsia_driver_framework;

}
namespace fio = fuchsia_io;
namespace fkernel = fuchsia_kernel;
namespace flogger = fuchsia_logger;
namespace frunner = fuchsia_component_runner;

constexpr auto kOpenFlags =
    fio::wire::kPermReadable | fio::wire::kPermExecutable | fio::wire::Flags::kProtocolFile;
constexpr auto kVmoFlags = fio::wire::VmoFlags::kRead | fio::wire::VmoFlags::kExecute;

namespace {

zx::vmo GetVmo(std::string_view path) {
  auto endpoints = fidl::Endpoints<fio::File>::Create();
  zx_status_t status = fdio_open3(path.data(), static_cast<uint64_t>(kOpenFlags),
                                  endpoints.server.channel().release());
  EXPECT_EQ(status, ZX_OK) << zx_status_get_string(status);
  fidl::WireResult result = fidl::WireCall(endpoints.client)->GetBackingMemory(kVmoFlags);
  EXPECT_TRUE(result.ok()) << result.FormatDescription();
  const auto& response = result.value();
  EXPECT_TRUE(response.is_ok()) << zx_status_get_string(response.error_value());
  return std::move(response.value()->vmo);
}

class TestMmioResource : public fidl::testing::WireTestBase<fkernel::MmioResource> {
 public:
  TestMmioResource() { EXPECT_EQ(ZX_OK, zx::event::create(0, &fake_resource_)); }

  fidl::ProtocolHandler<fkernel::MmioResource> GetHandler() {
    return bindings_.CreateHandler(this, async_get_default_dispatcher(),
                                   fidl::kIgnoreBindingClosure);
  }

 private:
  void Get(GetCompleter::Sync& completer) override {
    zx::event duplicate;
    ASSERT_EQ(ZX_OK, fake_resource_.duplicate(ZX_RIGHT_SAME_RIGHTS, &duplicate));
    completer.Reply(zx::resource(duplicate.release()));
  }

  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) override {
    printf("Not implemented: MmioResource::%s\n", name.data());
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }
  fidl::ServerBindingGroup<fkernel::MmioResource> bindings_;

  // An event is similar enough that we can pretend it's the mmio resource, in that we can
  // send it over a FIDL channel.
  zx::event fake_resource_;
};

class TestPowerResource : public fidl::testing::WireTestBase<fkernel::PowerResource> {
 public:
  TestPowerResource() { EXPECT_EQ(ZX_OK, zx::event::create(0, &fake_resource_)); }

  fidl::ProtocolHandler<fkernel::PowerResource> GetHandler() {
    return bindings_.CreateHandler(this, async_get_default_dispatcher(),
                                   fidl::kIgnoreBindingClosure);
  }

 private:
  void Get(GetCompleter::Sync& completer) override {
    zx::event duplicate;
    ASSERT_EQ(ZX_OK, fake_resource_.duplicate(ZX_RIGHT_SAME_RIGHTS, &duplicate));
    completer.Reply(zx::resource(duplicate.release()));
  }

  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) override {
    printf("Not implemented: PowerResource::%s\n", name.data());
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }
  fidl::ServerBindingGroup<fkernel::PowerResource> bindings_;

  // An event is similar enough that we can pretend it's the power resource, in that we can
  // send it over a FIDL channel.
  zx::event fake_resource_;
};

class TestIommuResource : public fidl::testing::WireTestBase<fkernel::IommuResource> {
 public:
  TestIommuResource() { EXPECT_EQ(ZX_OK, zx::event::create(0, &fake_resource_)); }

  fidl::ProtocolHandler<fkernel::IommuResource> GetHandler() {
    return bindings_.CreateHandler(this, async_get_default_dispatcher(),
                                   fidl::kIgnoreBindingClosure);
  }

 private:
  void Get(GetCompleter::Sync& completer) override {
    zx::event duplicate;
    ASSERT_EQ(ZX_OK, fake_resource_.duplicate(ZX_RIGHT_SAME_RIGHTS, &duplicate));
    completer.Reply(zx::resource(duplicate.release()));
  }

  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) override {
    printf("Not implemented: IommuResource::%s\n", name.data());
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }
  fidl::ServerBindingGroup<fkernel::IommuResource> bindings_;

  // An event is similar enough that we can pretend it's the iommu resource, in that we can
  // send it over a FIDL channel.
  zx::event fake_resource_;
};

class TestIoportResource : public fidl::testing::WireTestBase<fkernel::IoportResource> {
 public:
  TestIoportResource() { EXPECT_EQ(ZX_OK, zx::event::create(0, &fake_resource_)); }

  fidl::ProtocolHandler<fkernel::IoportResource> GetHandler() {
    return bindings_.CreateHandler(this, async_get_default_dispatcher(),
                                   fidl::kIgnoreBindingClosure);
  }

 private:
  void Get(GetCompleter::Sync& completer) override {
    zx::event duplicate;
    ASSERT_EQ(ZX_OK, fake_resource_.duplicate(ZX_RIGHT_SAME_RIGHTS, &duplicate));
    completer.Reply(zx::resource(duplicate.release()));
  }

  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) override {
    printf("Not implemented: IoportResource::%s\n", name.data());
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }
  fidl::ServerBindingGroup<fkernel::IoportResource> bindings_;

  // An event is similar enough that we can pretend it's the ioport resource, in that we can
  // send it over a FIDL channel.
  zx::event fake_resource_;
};

class TestIrqResource : public fidl::testing::WireTestBase<fkernel::IrqResource> {
 public:
  TestIrqResource() { EXPECT_EQ(ZX_OK, zx::event::create(0, &fake_resource_)); }

  fidl::ProtocolHandler<fkernel::IrqResource> GetHandler() {
    return bindings_.CreateHandler(this, async_get_default_dispatcher(),
                                   fidl::kIgnoreBindingClosure);
  }

 private:
  void Get(GetCompleter::Sync& completer) override {
    zx::event duplicate;
    ASSERT_EQ(ZX_OK, fake_resource_.duplicate(ZX_RIGHT_SAME_RIGHTS, &duplicate));
    completer.Reply(zx::resource(duplicate.release()));
  }

  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) override {
    printf("Not implemented: IrqResource::%s\n", name.data());
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }
  fidl::ServerBindingGroup<fkernel::IrqResource> bindings_;

  // An event is similar enough that we can pretend it's the irq resource, in that we can
  // send it over a FIDL channel.
  zx::event fake_resource_;
};

class TestSmcResource : public fidl::testing::WireTestBase<fkernel::SmcResource> {
 public:
  TestSmcResource() { EXPECT_EQ(ZX_OK, zx::event::create(0, &fake_resource_)); }

  fidl::ProtocolHandler<fkernel::SmcResource> GetHandler() {
    return bindings_.CreateHandler(this, async_get_default_dispatcher(),
                                   fidl::kIgnoreBindingClosure);
  }

 private:
  void Get(GetCompleter::Sync& completer) override {
    zx::event duplicate;
    ASSERT_EQ(ZX_OK, fake_resource_.duplicate(ZX_RIGHT_SAME_RIGHTS, &duplicate));
    completer.Reply(zx::resource(duplicate.release()));
  }

  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) override {
    printf("Not implemented: SmcResource::%s\n", name.data());
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }
  fidl::ServerBindingGroup<fkernel::SmcResource> bindings_;

  // An event is similar enough that we can pretend it's the smc resource, in that we can
  // send it over a FIDL channel.
  zx::event fake_resource_;
};

class TestInfoResource : public fidl::testing::WireTestBase<fkernel::InfoResource> {
 public:
  TestInfoResource() { EXPECT_EQ(ZX_OK, zx::event::create(0, &fake_resource_)); }

  fidl::ProtocolHandler<fkernel::InfoResource> GetHandler() {
    return bindings_.CreateHandler(this, async_get_default_dispatcher(),
                                   fidl::kIgnoreBindingClosure);
  }

 private:
  void Get(GetCompleter::Sync& completer) override {
    zx::event duplicate;
    ASSERT_EQ(ZX_OK, fake_resource_.duplicate(ZX_RIGHT_SAME_RIGHTS, &duplicate));
    completer.Reply(zx::resource(duplicate.release()));
  }

  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) override {
    printf("Not implemented: InfoResource::%s\n", name.data());
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }
  fidl::ServerBindingGroup<fkernel::InfoResource> bindings_;

  // An event is similar enough that we can pretend it's the info resource, in that we can
  // send it over a FIDL channel.
  zx::event fake_resource_;
};

class TestMsiResource : public fidl::testing::WireTestBase<fkernel::MsiResource> {
 public:
  TestMsiResource() { EXPECT_EQ(ZX_OK, zx::event::create(0, &fake_resource_)); }

  fidl::ProtocolHandler<fkernel::MsiResource> GetHandler() {
    return bindings_.CreateHandler(this, async_get_default_dispatcher(),
                                   fidl::kIgnoreBindingClosure);
  }

 private:
  void Get(GetCompleter::Sync& completer) override {
    zx::event duplicate;
    ASSERT_EQ(ZX_OK, fake_resource_.duplicate(ZX_RIGHT_SAME_RIGHTS, &duplicate));
    completer.Reply(zx::resource(duplicate.release()));
  }

  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) override {
    printf("Not implemented: MsiResource::%s\n", name.data());
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }
  fidl::ServerBindingGroup<fkernel::MsiResource> bindings_;

  // An event is similar enough that we can pretend it's the msi resource, in that we can
  // send it over a FIDL channel.
  zx::event fake_resource_;
};

class TestFile : public fidl::testing::WireTestBase<fio::File> {
 public:
  TestFile() = default;
  TestFile(zx_status_t status, zx::vmo vmo) : status_(status), vmo_(std::move(vmo)) {}

 private:
  void GetBackingMemory(GetBackingMemoryRequestView request,
                        GetBackingMemoryCompleter::Sync& completer) override {
    if (status_ != ZX_OK) {
      completer.ReplyError(status_);
    } else {
      completer.ReplySuccess(std::move(vmo_));
    }
  }

  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) override {
    printf("Not implemented: File::%s\n", name.data());
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }

  zx_status_t status_ = ZX_OK;
  zx::vmo vmo_;
};

class TestDirectory : public fidl::testing::WireTestBase<fio::Directory> {
 public:
  using OpenHandler = fit::function<void(std::string_view path, fidl::ServerEnd<fio::Node> object)>;

  void SetOpenHandler(OpenHandler open_handler) { open_handler_ = std::move(open_handler); }

 private:
  void Open(OpenRequestView request, OpenCompleter::Sync& completer) override {
    open_handler_(request->path.get(), fidl::ServerEnd<fio::Node>(std::move(request->object)));
  }

  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) override {
    printf("Not implemented: Directory::%s\n", name.data());
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }

  OpenHandler open_handler_;
};

class TestDevice : public fidl::WireServer<fuchsia_driver_compat::Device> {
 public:
  struct MockProtocol {
    uint64_t ctx;
    uint64_t ops;
  };

  explicit TestDevice(std::unordered_map<uint32_t, MockProtocol> banjo_protocols = {})
      : banjo_protocols_(std::move(banjo_protocols)) {}

  void GetBanjoProtocol(GetBanjoProtocolRequestView request,
                        GetBanjoProtocolCompleter::Sync& completer) override {
    auto iter = banjo_protocols_.find(request->proto_id);
    if (iter == banjo_protocols_.end()) {
      completer.ReplyError(ZX_ERR_PROTOCOL_NOT_SUPPORTED);
      return;
    }

    auto& protocol = iter->second;
    completer.ReplySuccess(protocol.ops, protocol.ctx);
  }

  void GetMetadata(GetMetadataCompleter::Sync& completer) override {
    std::vector<fuchsia_driver_compat::wire::Metadata> metadata;

    std::vector<uint8_t> bytes_1 = {1, 2, 3};
    zx::vmo vmo_1;
    ASSERT_EQ(ZX_OK, zx::vmo::create(bytes_1.size(), 0, &vmo_1));
    vmo_1.write(bytes_1.data(), 0, bytes_1.size());
    size_t size = bytes_1.size();
    ASSERT_EQ(ZX_OK, vmo_1.set_property(ZX_PROP_VMO_CONTENT_SIZE, &size, sizeof(size)));
    metadata.push_back(fuchsia_driver_compat::wire::Metadata{.type = 1, .data = std::move(vmo_1)});

    std::vector<uint8_t> bytes_2 = {4, 5, 6};
    zx::vmo vmo_2;
    ASSERT_EQ(ZX_OK, zx::vmo::create(bytes_1.size(), 0, &vmo_2));
    vmo_2.write(bytes_2.data(), 0, bytes_2.size());
    ASSERT_EQ(ZX_OK, vmo_2.set_property(ZX_PROP_VMO_CONTENT_SIZE, &size, sizeof(size)));
    metadata.push_back(fuchsia_driver_compat::wire::Metadata{.type = 2, .data = std::move(vmo_2)});

    completer.ReplySuccess(fidl::VectorView<fuchsia_driver_compat::wire::Metadata>::FromExternal(
        metadata.data(), metadata.size()));
  }

 private:
  std::unordered_map<uint32_t, MockProtocol> banjo_protocols_;
};

class TestRoleManager : public fidl::testing::WireTestBase<fuchsia_scheduler::RoleManager> {
 public:
  TestRoleManager() = default;

  explicit TestRoleManager(std::string expected_role) : expected_role_(std::move(expected_role)) {}

  fidl::ProtocolHandler<fuchsia_scheduler::RoleManager> GetHandler() {
    return bindings_.CreateHandler(this, async_get_default_dispatcher(),
                                   fidl::kIgnoreBindingClosure);
  }

 private:
  void SetRole(SetRoleRequestView request, SetRoleCompleter::Sync& completer) override {
    if (!request->target().is_thread()) {
      completer.ReplyError(ZX_ERR_INVALID_ARGS);
      return;
    }
    if (!request->target().thread().is_valid()) {
      completer.ReplyError(ZX_ERR_INVALID_ARGS);
      return;
    }
    if (request->role().role.get() != expected_role_) {
      completer.ReplyError(ZX_ERR_BAD_PATH);
      return;
    }
    fidl::Arena arena;
    completer.ReplySuccess(
        fuchsia_scheduler::wire::RoleManagerSetRoleResponse::Builder(arena).Build());
  }

  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) override {
    printf("Not implemented: RoleManager::%s", name.data());
  }

  fidl::ServerBindingGroup<fuchsia_scheduler::RoleManager> bindings_;
  std::string expected_role_;
};

class TestSystemStateTransition
    : public fidl::testing::WireTestBase<fuchsia_system_state::SystemStateTransition> {
 public:
  fidl::ProtocolHandler<fuchsia_system_state::SystemStateTransition> GetHandler() {
    return bindings_.CreateHandler(this, async_get_default_dispatcher(),
                                   fidl::kIgnoreBindingClosure);
  }

 private:
  void GetTerminationSystemState(GetTerminationSystemStateCompleter::Sync& completer) override {
    completer.Reply(fuchsia_system_state::wire::SystemPowerState::kFullyOn);
  }

  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) override {
    printf("Not implemented: SystemStateTransition::%s", name.data());
  }

  fidl::ServerBindingGroup<fuchsia_system_state::SystemStateTransition> bindings_;
};

class IncomingNamespace {
 public:
  zx::result<> Start(std::string_view v1_driver_path, zx_status_t compat_file_response,
                     std::unordered_map<std::string, TestDevice> devices,
                     std::string expected_profile_role, fidl::ServerEnd<fio::Directory> pkg_server,
                     fidl::ServerEnd<fio::Directory> svc_server) {
    async_dispatcher_t* dispatcher = async_get_default_dispatcher();
    role_manager_.emplace(std::move(expected_profile_role));

    // Setup and bind "/pkg" directory.
    v1_test_file_ = TestFile(ZX_OK, GetVmo(v1_driver_path));
    firmware_file_ = TestFile(ZX_OK, GetVmo("/pkg/lib/firmware/test"));
    pkg_directory_.SetOpenHandler([this, dispatcher](std::string_view path, auto object) {
      fidl::ServerEnd<fio::File> server_end(object.TakeChannel());
      if (path == "driver/v1_test.so") {
        fidl::BindServer(dispatcher, std::move(server_end), &v1_test_file_);
      } else if (path == "lib/firmware/test") {
        fidl::BindServer(dispatcher, std::move(server_end), &firmware_file_);
      } else {
        FAIL() << "Unexpected file: " << path;
      }
    });
    fidl::BindServer(dispatcher, std::move(pkg_server), &pkg_directory_);

    // Setup and bind "/svc" directory.
    {
      zx::result result = outgoing.AddUnmanagedProtocol<flogger::LogSink>(
          [](fidl::ServerEnd<flogger::LogSink> server) {
            // Forward log-sink.
            ASSERT_EQ(component::Connect<flogger::LogSink>(std::move(server)).status_value(),
                      ZX_OK);
          });
      if (result.is_error()) {
        return result.take_error();
      }

      result = outgoing.AddUnmanagedProtocol<fkernel::MmioResource>(mmio_resource_.GetHandler());
      if (result.is_error()) {
        return result.take_error();
      }

      result = outgoing.AddUnmanagedProtocol<fkernel::PowerResource>(power_resource_.GetHandler());
      if (result.is_error()) {
        return result.take_error();
      }

      result = outgoing.AddUnmanagedProtocol<fkernel::IommuResource>(iommu_resource_.GetHandler());
      if (result.is_error()) {
        return result.take_error();
      }

      result =
          outgoing.AddUnmanagedProtocol<fkernel::IoportResource>(ioport_resource_.GetHandler());
      if (result.is_error()) {
        return result.take_error();
      }

      result = outgoing.AddUnmanagedProtocol<fkernel::IrqResource>(irq_resource_.GetHandler());
      if (result.is_error()) {
        return result.take_error();
      }

      result = outgoing.AddUnmanagedProtocol<fkernel::SmcResource>(smc_resource_.GetHandler());
      if (result.is_error()) {
        return result.take_error();
      }

      result = outgoing.AddUnmanagedProtocol<fkernel::InfoResource>(info_resource_.GetHandler());
      if (result.is_error()) {
        return result.take_error();
      }

      result = outgoing.AddUnmanagedProtocol<fkernel::MsiResource>(msi_resource_.GetHandler());
      if (result.is_error()) {
        return result.take_error();
      }

      result = outgoing.AddUnmanagedProtocol<fuchsia_scheduler::RoleManager>(
          role_manager_->GetHandler());
      if (result.is_error()) {
        return result.take_error();
      }

      result = outgoing.AddUnmanagedProtocol<fuchsia_system_state::SystemStateTransition>(
          system_state_transition_.GetHandler());
      if (result.is_error()) {
        return result.take_error();
      }

      devices_ = std::move(devices);
      for (auto& device : devices_) {
        TestDevice* device_ptr = &device.second;
        zx::result result = outgoing.AddService<fuchsia_driver_compat::Service>(
            fuchsia_driver_compat::Service::InstanceHandler({
                .device =
                    [device_ptr,
                     dispatcher](fidl::ServerEnd<fuchsia_driver_compat::Device> server) {
                      fidl::BindServer(dispatcher, std::move(server), device_ptr);
                    },
            }),
            device.first);
        if (result.is_error()) {
          return result.take_error();
        }
      }

      auto [outgoing_client, outgoing_server] = fidl::Endpoints<fuchsia_io::Directory>::Create();
      if (zx::result result = outgoing.Serve(std::move(outgoing_server)); result.is_error()) {
        return result.take_error();
      }
      fidl::OneWayError error =
          fidl::WireCall(outgoing_client)
              ->Open("svc",
                     fuchsia_io::wire::kPermReadable | fuchsia_io::wire::Flags::kProtocolDirectory,
                     {}, svc_server.TakeChannel());
      if (!error.ok()) {
        return zx::error(error.status());
      }
    }

    return zx::ok();
  }

 private:
  std::unordered_map<std::string, TestDevice> devices_;
  TestMmioResource mmio_resource_;
  TestPowerResource power_resource_;
  TestIommuResource iommu_resource_;
  TestIoportResource ioport_resource_;
  TestIrqResource irq_resource_;
  TestSmcResource smc_resource_;
  TestInfoResource info_resource_;
  TestMsiResource msi_resource_;
  std::optional<TestRoleManager> role_manager_;
  TestFile v1_test_file_;
  TestFile firmware_file_;
  TestDirectory pkg_directory_;
  TestSystemStateTransition system_state_transition_;
  component::OutgoingDirectory outgoing{async_get_default_dispatcher()};
};

// Log through a zx_driver logger.
void log(zx_driver_t* drv, FuchsiaLogSeverity severity, const char* tag, const char* file, int line,
         const char* msg, ...) {
  va_list args;
  va_start(args, msg);
  drv->Log(severity, tag, file, line, msg, args);
  va_end(args);
}

}  // namespace

class DriverTest : public testing::Test {
 protected:
  fdf_testing::TestNode& node() { return node_.value(); }

  void SetUp() override {
    ns_loop_.StartThread("fidl-server-thread");
    node_.emplace("root", dispatcher());
  }

  void TearDown() override {}

  struct StartDriverArgs {
    std::string_view v1_driver_path;
    zx_protocol_device_t ops = {};
    std::unordered_map<std::string, TestDevice> devices = {{"default", TestDevice()}};
    zx_status_t expected_driver_status = ZX_OK;
    zx_status_t compat_file_response = ZX_OK;
    std::string expected_profile_role;
  };
  std::unique_ptr<compat::Driver> StartDriver(StartDriverArgs args) {
    auto outgoing_dir_endpoints = fidl::CreateEndpoints<fio::Directory>();
    EXPECT_TRUE(outgoing_dir_endpoints.is_ok());
    auto pkg_endpoints = fidl::Endpoints<fio::Directory>::Create();
    auto svc_endpoints = fidl::CreateEndpoints<fio::Directory>();
    EXPECT_TRUE(svc_endpoints.is_ok());

    // Setup the node.
    zx::result node_client = node_->CreateNodeChannel();
    EXPECT_EQ(ZX_OK, node_client.status_value());

    zx::result ns_start_result =
        incoming_ns_.SyncCall(&IncomingNamespace::Start, args.v1_driver_path,
                              args.compat_file_response, args.devices, args.expected_profile_role,
                              std::move(pkg_endpoints.server), std::move(svc_endpoints->server));
    EXPECT_EQ(ZX_OK, ns_start_result.status_value());

    auto entry_pkg = frunner::ComponentNamespaceEntry(
        {.path = std::string("/pkg"), .directory = std::move(pkg_endpoints.client)});
    auto entry_svc = frunner::ComponentNamespaceEntry(
        {.path = std::string("/svc"), .directory = std::move(svc_endpoints->client)});
    std::vector<frunner::ComponentNamespaceEntry> ns_entries;
    ns_entries.push_back(std::move(entry_pkg));
    ns_entries.push_back(std::move(entry_svc));

    zx::vmo v1_driver_vmo = GetVmo(args.v1_driver_path);

    async::Loop loader_loop(&kAsyncLoopConfigNoAttachToCurrentThread);
    EXPECT_EQ(loader_loop.StartThread("loader-loop"), ZX_OK);

    auto [client_end, server_end] = fidl::Endpoints<fuchsia_ldsvc::Loader>::Create();
    fidl::ClientEnd<fuchsia_ldsvc::Loader> original_loader(
        zx::channel(dl_set_loader_service(client_end.channel().release())));
    auto reset_loader = fit::defer([&original_loader]() {
      zx::channel l = zx::channel(dl_set_loader_service(original_loader.TakeChannel().release()));
    });

    // Start loader.
    auto [file_client, file_server] = fidl::Endpoints<fuchsia_io::File>::Create();
    fdio_open3("/pkg/driver/compat.so",
               static_cast<uint64_t>(fuchsia_io::kPermReadable | fuchsia_io::kPermExecutable),
               file_server.TakeHandle().release());
    compat::Loader::OverrideMap overrides;
    overrides.emplace("libdriver.so", std::move(file_client));
    async_patterns::DispatcherBound<compat::Loader> loader(
        loader_loop.dispatcher(), std::in_place, original_loader.borrow(), std::move(overrides));
    loader.AsyncCall(&compat::Loader::Bind, std::move(server_end));

    void* v1_driver_library = dlopen_vmo(v1_driver_vmo.get(), RTLD_NOW);
    void* note = dlsym(v1_driver_library, "__zircon_driver_note__");
    void* record = dlsym(v1_driver_library, "__zircon_driver_rec__");

    constexpr char kModuleName[] = "driver/v1_test.so";

    device_ops_ = args.ops;
    std::vector<fdf::NodeSymbol> symbols({
        fdf::NodeSymbol{{
            .name = compat::kOps,
            .address = reinterpret_cast<uint64_t>(&device_ops_),
        }},
        fdf::NodeSymbol{{
            .name = "__zircon_driver_note__",
            .address = reinterpret_cast<uint64_t>(note),
            .module_name = kModuleName,
        }},
        fdf::NodeSymbol{{
            .name = "__zircon_driver_rec__",
            .address = reinterpret_cast<uint64_t>(record),
            .module_name = kModuleName,
        }},
    });

    auto program_entry = fdata::DictionaryEntry(
        "compat",
        std::make_unique<fdata::DictionaryValue>(fdata::DictionaryValue::WithStr(kModuleName)));
    std::vector<fdata::DictionaryEntry> program_vec;
    program_vec.push_back(std::move(program_entry));
    fdata::Dictionary program({.entries = std::move(program_vec)});

    fuchsia_driver_framework::DriverStartArgs start_args({
        .node = std::move(node_client.value()),
        .symbols = std::move(symbols),
        .url = std::string("fuchsia-pkg://fuchsia.com/driver#meta/driver.cm"),
        .program = std::move(program),
        .incoming = std::move(ns_entries),
        .outgoing_dir = std::move(outgoing_dir_endpoints->server),
        .config = std::nullopt,
        .node_name = "node",
        .node_offers = fuchsia_driver_framework::NodeOffersVector{},
    });

    // Start driver.
    std::optional<zx_status_t> status = std::nullopt;
    fdf::StartCompleter start_completer(
        [&status](zx::result<> result) { status.emplace(result.status_value()); });
    void* driver = compat::CompatDriverServer::CreateDriver(
        std::move(start_args),
        fdf::UnownedSynchronizedDispatcher(fdf::Dispatcher::GetCurrent()->get()),
        std::move(start_completer));

    while (status == std::nullopt) {
      fdf_testing_run_until_idle();
    };
    EXPECT_EQ(status.value(), args.expected_driver_status);
    if (status != ZX_OK) {
      EXPECT_NE(driver, nullptr);
    }

    return std::unique_ptr<compat::Driver>(static_cast<compat::Driver*>(driver));
  }

  void UnbindAndFreeDriver(std::unique_ptr<compat::Driver> driver) {
    libsync::Completion completion;

    fdf::StopCompleter completer([&completion](zx::result<>) { completion.Signal(); });
    driver->Stop(std::move(completer));

    // Keep running the test loop while we're waiting for a signal on the dispatcher thread.
    // The dispatcher thread needs to interact with our Node servers, which run on the test loop.
    while (!completion.signaled()) {
      fdf_testing_run_until_idle();
    }

    driver.reset();
  }

  void WaitForChildDeviceAdded() {
    while (node().children().empty()) {
      fdf_testing_run_until_idle();
    }
    EXPECT_FALSE(node().children().empty());
  }

  async_dispatcher_t* dispatcher() { return fdf::Dispatcher::GetCurrent()->async_dispatcher(); }

 private:
  async::Loop ns_loop_ = async::Loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  async_patterns::TestDispatcherBound<IncomingNamespace> incoming_ns_{ns_loop_.dispatcher(),
                                                                      std::in_place};
  zx_protocol_device_t device_ops_;
  fdf_testing::DriverRuntime runtime_;
  std::optional<fdf_testing::TestNode> node_;
};

class GlobalLoggerListTest : public testing::Test {
 protected:
  GlobalLoggerListTest() : loop_(&kAsyncLoopConfigNoAttachToCurrentThread) {}
  ~GlobalLoggerListTest() {
    if (svc_dir_) {
      svc_directory_destroy(svc_dir_);
    }
  }

  void SetUp() override {
    ASSERT_EQ(svc_directory_create(&svc_dir_), ZX_OK);

    constexpr std::string_view log_sink_name(
        fidl::DiscoverableProtocolName<fuchsia_logger::LogSink>);
    ASSERT_EQ(
        svc_directory_add_service(
            svc_dir_, nullptr, 0, log_sink_name.data(), log_sink_name.size(), nullptr,
            +[](void* context, const char* service_name, zx_handle_t service_request) {
              ZX_ASSERT(component::Connect<fuchsia_logger::LogSink>(
                            fidl::ServerEnd<fuchsia_logger::LogSink>(zx::channel(service_request)))
                            .is_ok());
            }),
        ZX_OK);

    loop_.StartThread();
  }

  std::shared_ptr<fdf::Logger> NewLogger(const std::string& name) {
    auto svc = fidl::CreateEndpoints<fio::Directory>();
    ZX_ASSERT(ZX_OK == svc.status_value());

    fidl::Arena arena;
    fidl::VectorView<frunner::wire::ComponentNamespaceEntry> entries(arena, 1);
    entries[0] = frunner::wire::ComponentNamespaceEntry::Builder(arena)
                     .path("/svc")
                     .directory(std::move(svc->client))
                     .Build();
    auto ns = fdf::Namespace::Create(entries);
    ZX_ASSERT(ZX_OK == ns.status_value());

    ZX_ASSERT(svc_directory_serve(svc_dir_, loop_.dispatcher(),
                                  svc->server.TakeChannel().release()) == ZX_OK);

    auto logger = fdf::Logger::Create2(*ns, dispatcher(), name, FUCHSIA_LOG_INFO);
    return std::shared_ptr<fdf::Logger>(logger.release());
  }

  static async_dispatcher_t* dispatcher() {
    return fdf::Dispatcher::GetCurrent()->async_dispatcher();
  }

 private:
  async::Loop loop_;
  svc_dir_t* svc_dir_ = nullptr;
  fdf_testing::DriverRuntime runtime_;
};

TEST_F(DriverTest, Start) {
  auto driver = StartDriver({
      .v1_driver_path = "/pkg/driver/v1_test.so",
      .ops =
          {
              .get_protocol = [](void*, uint32_t, void*) { return ZX_OK; },
          },
  });

  // Verify that v1_test.so has added a child device.
  WaitForChildDeviceAdded();

  // Verify that v1_test.so has set a context.
  std::unique_ptr<V1Test> v1_test(static_cast<V1Test*>(driver->Context()));
  ASSERT_NE(nullptr, v1_test.get());

  // Verify v1_test.so state after bind.
  EXPECT_TRUE(v1_test->did_bind);
  EXPECT_EQ(ZX_OK, v1_test->status);
  EXPECT_FALSE(v1_test->did_create);
  EXPECT_FALSE(v1_test->did_release);

  // Verify v1_test.so state after release.
  UnbindAndFreeDriver(std::move(driver));
  EXPECT_TRUE(v1_test->did_release);
}

TEST_F(DriverTest, Start_WithCreate) {
  auto driver = StartDriver({
      .v1_driver_path = "/pkg/driver/v1_create_test.so",
  });

  // Verify that v1_test.so has added a child device.
  WaitForChildDeviceAdded();

  // Verify that v1_test.so has set a context.
  std::unique_ptr<V1Test> v1_test(static_cast<V1Test*>(driver->Context()));
  ASSERT_NE(nullptr, v1_test.get());

  // Verify v1_test.so state after bind.
  EXPECT_EQ(ZX_OK, v1_test->status);
  EXPECT_FALSE(v1_test->did_bind);
  EXPECT_TRUE(v1_test->did_create);
  EXPECT_FALSE(v1_test->did_release);

  // Verify v1_test.so state after release.
  UnbindAndFreeDriver(std::move(driver));
  EXPECT_TRUE(v1_test->did_release);
}

TEST_F(DriverTest, Start_MissingBindAndCreate) {
  auto driver = StartDriver({
      .v1_driver_path = "/pkg/driver/v1_missing_test.so",
      .expected_driver_status = ZX_ERR_BAD_STATE,
  });
  EXPECT_TRUE(node().children().empty());

  // Verify that v1_test.so has not set a context.
  EXPECT_EQ(nullptr, driver->Context());
}

TEST_F(DriverTest, Start_DeviceAddNull) {
  auto driver = StartDriver({
      .v1_driver_path = "/pkg/driver/v1_device_add_null_test.so",
  });

  // Verify that v1_test.so has added a child device.
  WaitForChildDeviceAdded();

  UnbindAndFreeDriver(std::move(driver));
}

TEST_F(DriverTest, Start_CheckCompatService) {
  auto driver = StartDriver({
      .v1_driver_path = "/pkg/driver/v1_device_add_null_test.so",
  });

  // Verify that v1_test.so has added a child device.
  WaitForChildDeviceAdded();

  // Check metadata.
  std::array<uint8_t, 3> expected_metadata;
  std::array<uint8_t, 3> metadata;
  size_t size = 0;

  ASSERT_EQ(driver->GetDevice().GetMetadata(1, metadata.data(), metadata.size(), &size), ZX_OK);
  ASSERT_EQ(size, 3ul);
  expected_metadata = {1, 2, 3};
  ASSERT_EQ(metadata, expected_metadata);

  ASSERT_EQ(driver->GetDevice().GetMetadata(2, metadata.data(), metadata.size(), &size), ZX_OK);
  ASSERT_EQ(size, 3ul);
  expected_metadata = {4, 5, 6};
  ASSERT_EQ(metadata, expected_metadata);

  UnbindAndFreeDriver(std::move(driver));
}

TEST_F(DriverTest, Start_BindFailed) {
  auto driver = StartDriver({
      .v1_driver_path = "/pkg/driver/v1_test.so",
      .expected_driver_status = ZX_ERR_PROTOCOL_NOT_SUPPORTED,
  });

  // Verify that v1_test.so has set a context.
  while (!driver->Context()) {
    fdf_testing_run_until_idle();
  }
  std::unique_ptr<V1Test> v1_test(static_cast<V1Test*>(driver->Context()));
  ASSERT_NE(nullptr, v1_test.get());

  // Verify that v1_test.so has been bound.
  while (!v1_test->did_bind) {
    fdf_testing_run_until_idle();
  }

  // Verify that v1_test.so has not added a child device.
  EXPECT_TRUE(node().children().empty());

  EXPECT_TRUE(v1_test->did_bind);
  EXPECT_EQ(ZX_ERR_PROTOCOL_NOT_SUPPORTED, v1_test->status);

  EXPECT_FALSE(v1_test->did_create);
  EXPECT_FALSE(v1_test->did_release);

  // Verify v1_test.so state after release.
  UnbindAndFreeDriver(std::move(driver));

  EXPECT_TRUE(v1_test->did_release);
}

TEST_F(DriverTest, SetProfileByRole) {
  auto driver = StartDriver({
      .v1_driver_path = "/pkg/driver/v1_test.so",
      .ops =
          {
              .get_protocol = [](void*, uint32_t, void*) { return ZX_OK; },
          },
      .expected_profile_role = "test-profile",
  });
  // Verify that v1_test.so has added a child device.
  WaitForChildDeviceAdded();

  // Verify that v1_test.so has set a context.
  std::unique_ptr<V1Test> v1_test(static_cast<V1Test*>(driver->Context()));
  ASSERT_NE(nullptr, v1_test.get());

  constexpr char kThreadName[] = "test-thread";
  zx::thread thread;
  ASSERT_EQ(ZX_OK,
            zx::thread::create(*zx::process::self(), kThreadName, sizeof(kThreadName), 0, &thread));
  ASSERT_EQ(ZX_OK, device_set_profile_by_role(v1_test->zxdev, thread.release(), "test-profile",
                                              strlen("test-profile")));

  ASSERT_EQ(ZX_OK,
            zx::thread::create(*zx::process::self(), kThreadName, sizeof(kThreadName), 0, &thread));
  ASSERT_EQ(ZX_ERR_BAD_PATH, device_set_profile_by_role(v1_test->zxdev, thread.release(),
                                                        "bad-role", strlen("bad-role")));

  UnbindAndFreeDriver(std::move(driver));
}

TEST_F(DriverTest, GetFragmentProtocol) {
  const char* kFragmentName = "fragment-name";
  const uint32_t kFragmentProtoId = ZX_PROTOCOL_BLOCK;
  const uint64_t kFragmentOps = 0x1234;
  const uint64_t kFragmentCtx = 0x4567;

  auto driver = StartDriver({
      .v1_driver_path = "/pkg/driver/v1_test.so",
      .ops =
          {
              .get_protocol = [](void*, uint32_t, void*) { return ZX_OK; },
          },
      .devices =
          {
              {kFragmentName, TestDevice({
                                  {kFragmentProtoId, {kFragmentCtx, kFragmentOps}},
                              })},
          },
  });

  // Verify that v1_test.so has added a child device.
  WaitForChildDeviceAdded();

  // Verify that v1_test.so has set a context.
  std::unique_ptr<V1Test> v1_test(static_cast<V1Test*>(driver->Context()));
  ASSERT_NE(nullptr, v1_test.get());

  struct GenericProtocol {
    const void* ops;
    void* ctx;
  } proto;
  ASSERT_EQ(ZX_OK, driver->GetFragmentProtocol(kFragmentName, kFragmentProtoId, &proto));
  ASSERT_EQ(reinterpret_cast<uint64_t>(proto.ops), kFragmentOps);
  ASSERT_EQ(reinterpret_cast<uint64_t>(proto.ctx), kFragmentCtx);
  ASSERT_EQ(ZX_ERR_NOT_FOUND,
            driver->GetFragmentProtocol("unknown-fragment", kFragmentProtoId, &proto));

  // Verify v1_test.so state after release.
  UnbindAndFreeDriver(std::move(driver));
  EXPECT_TRUE(v1_test->did_release);
}

TEST_F(GlobalLoggerListTest, TestWithoutNodeNames) {
  compat::GlobalLoggerList global_list(false);
  ASSERT_EQ(std::nullopt, global_list.loggers_count_for_testing("path_1"));

  auto logger_1 = NewLogger("logger_1");
  auto* zx_driver_1 = global_list.AddLogger("path_1", logger_1, std::nullopt);
  ASSERT_EQ(1, global_list.loggers_count_for_testing("path_1"));
  auto& node_names_res = zx_driver_1->node_names_for_testing();
  ASSERT_EQ(0ul, node_names_res.size());

  auto logger_2 = NewLogger("logger_2");
  auto* zx_driver_2 = global_list.AddLogger("path_1", logger_2, std::nullopt);
  ASSERT_EQ(2, global_list.loggers_count_for_testing("path_1"));
  ASSERT_EQ(zx_driver_1, zx_driver_2);
  auto& node_names_res_2 = zx_driver_2->node_names_for_testing();
  ASSERT_EQ(&node_names_res, &node_names_res_2);
  ASSERT_EQ(0ul, node_names_res_2.size());

  auto logger_3 = NewLogger("logger_3");
  auto* zx_driver_3 = global_list.AddLogger("path_2", logger_3, std::nullopt);
  ASSERT_EQ(1, global_list.loggers_count_for_testing("path_2"));
  ASSERT_NE(zx_driver_3, zx_driver_2);
  auto& node_names_res_3 = zx_driver_3->node_names_for_testing();
  ASSERT_NE(&node_names_res_2, &node_names_res_3);
  ASSERT_EQ(0ul, node_names_res_3.size());
  log(zx_driver_3, FUCHSIA_LOG_INFO, nullptr, __FILE__, __LINE__, "Hello!");

  global_list.RemoveLogger("path_2", logger_3, std::nullopt);
  ASSERT_EQ(0, global_list.loggers_count_for_testing("path_2"));

  global_list.RemoveLogger("path_1", logger_1, std::nullopt);
  ASSERT_EQ(1, global_list.loggers_count_for_testing("path_1"));
  ASSERT_EQ(0ul, node_names_res_2.size());

  global_list.RemoveLogger("path_1", logger_2, std::nullopt);
  ASSERT_EQ(0, global_list.loggers_count_for_testing("path_1"));

  // Make sure we can still log with the zx_drivers that we got even when it is emptied out.
  log(zx_driver_3, FUCHSIA_LOG_INFO, nullptr, __FILE__, __LINE__, "Done with test: %s",
      "TestWithoutNodeNames");
}

TEST_F(GlobalLoggerListTest, TestWithNodeNames) {
  compat::GlobalLoggerList global_list(true);
  ASSERT_EQ(std::nullopt, global_list.loggers_count_for_testing("path_1"));

  auto logger_1 = NewLogger("logger_1");
  auto* zx_driver_1 = global_list.AddLogger("path_1", logger_1, "node_1");
  ASSERT_EQ(1, global_list.loggers_count_for_testing("path_1"));
  auto& node_names_res = zx_driver_1->node_names_for_testing();
  ASSERT_EQ(1ul, node_names_res.size());
  ASSERT_EQ("node_1", node_names_res[0]);

  auto logger_2 = NewLogger("logger_2");
  auto* zx_driver_2 = global_list.AddLogger("path_1", logger_2, "node_2");
  ASSERT_EQ(2, global_list.loggers_count_for_testing("path_1"));
  ASSERT_EQ(zx_driver_1, zx_driver_2);
  auto& node_names_res_2 = zx_driver_2->node_names_for_testing();
  ASSERT_EQ(&node_names_res, &node_names_res_2);
  ASSERT_EQ(2ul, node_names_res_2.size());
  ASSERT_EQ("node_1", node_names_res_2[0]);
  ASSERT_EQ("node_2", node_names_res_2[1]);

  auto logger_3 = NewLogger("logger_3");
  auto* zx_driver_3 = global_list.AddLogger("path_2", logger_3, "node_3");
  ASSERT_EQ(1, global_list.loggers_count_for_testing("path_2"));
  ASSERT_NE(zx_driver_3, zx_driver_2);
  auto& node_names_res_3 = zx_driver_3->node_names_for_testing();
  ASSERT_NE(&node_names_res_2, &node_names_res_3);
  ASSERT_EQ(1ul, node_names_res_3.size());
  ASSERT_EQ("node_3", node_names_res_3[0]);
  log(zx_driver_3, FUCHSIA_LOG_INFO, nullptr, __FILE__, __LINE__, "Hello!");

  global_list.RemoveLogger("path_2", logger_3, "node_3");
  ASSERT_EQ(0, global_list.loggers_count_for_testing("path_2"));

  global_list.RemoveLogger("path_1", logger_1, "node_1");
  ASSERT_EQ(1, global_list.loggers_count_for_testing("path_1"));
  ASSERT_EQ(1ul, node_names_res_2.size());
  ASSERT_EQ("node_2", node_names_res_2[0]);

  global_list.RemoveLogger("path_1", logger_2, "node_2");
  ASSERT_EQ(0, global_list.loggers_count_for_testing("path_1"));

  // Make sure we can still log with the zx_drivers that we got even when it is emptied out.
  log(zx_driver_3, FUCHSIA_LOG_INFO, nullptr, __FILE__, __LINE__, "Done with test: %s",
      "TestWithNodeNames");
}
