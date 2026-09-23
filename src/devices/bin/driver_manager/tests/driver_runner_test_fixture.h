// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BIN_DRIVER_MANAGER_TESTS_DRIVER_RUNNER_TEST_FIXTURE_H_
#define SRC_DEVICES_BIN_DRIVER_MANAGER_TESTS_DRIVER_RUNNER_TEST_FIXTURE_H_

#include <fidl/fuchsia.component.decl/cpp/test_base.h>
#include <fidl/fuchsia.component.sandbox/cpp/test_base.h>
#include <fidl/fuchsia.component/cpp/test_base.h>
#include <fidl/fuchsia.driver.framework/cpp/test_base.h>
#include <fidl/fuchsia.driver.host/cpp/test_base.h>
#include <fidl/fuchsia.driver.token/cpp/test_base.h>
#include <fidl/fuchsia.io/cpp/test_base.h>
#include <fidl/fuchsia.power.broker/cpp/fidl.h>
#include <fidl/fuchsia.power.broker/cpp/test_base.h>
#include <fidl/fuchsia.power.system/cpp/fidl.h>
#include <fidl/fuchsia.power.system/cpp/test_base.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/fit/defer.h>
#include <lib/inspect/cpp/reader.h>
#include <lib/inspect/testing/cpp/inspect.h>

#include <bind/fuchsia/platform/cpp/bind.h>

#include "src/devices/bin/driver_manager/driver_runner.h"
#include "src/devices/bin/driver_manager/testing/fake_driver_index.h"
#include "src/devices/bin/driver_manager/tests/test_pkg.h"
#include "src/lib/testing/loop_fixture/test_loop_fixture.h"

namespace driver_runner {

namespace fdfw = fuchsia_driver_framework;
namespace fdh = fuchsia_driver_host;
namespace fio = fuchsia_io;
namespace fprocess = fuchsia_process;
namespace fdecl = fuchsia_component_decl;

zx::result<fidl::ClientEnd<fuchsia_ldsvc::Loader>> LoaderFactory();
zx::result<fidl::ClientEnd<fuchsia_driver_loader::DriverHostLauncher>> DynamicLinkerFactory(
    driver_loader::Loader* loader);

const std::string root_driver_url = "fuchsia-boot:///#meta/root-driver.cm";
const std::string root_driver_binary = "driver/root-driver.so";

const std::string second_driver_url = "fuchsia-boot:///#meta/second-driver.cm";
const std::string second_driver_binary = "driver/second-driver.so";

const std::string compat_driver_url = "fuchsia-boot:///#meta/compat.cm";
const std::string compat_driver_binary = "driver/compat.so";

using driver_manager::Devfs;
using driver_manager::DriverRunner;

static const test_utils::TestPkg::Config kDefaultDriverHostPkgConfig = {
    .main_module = {.test_pkg_path = "/pkg/bin/fake_driver_host_with_bootstrap",
                    .open_path = "bin/driver_host2"},
    .expected_libs =
        {
            "libdh-deps-a.so",
            "libdh-deps-b.so",
            "libdh-deps-c.so",
        },
};

static const test_utils::TestPkg::Config kDefaultRootDriverPkgConfig = {
    .main_module = {.test_pkg_path = "/pkg/lib/fake_root_driver.so",
                    .open_path = root_driver_binary},
    .expected_libs =
        {
            "libfake_root_driver_deps.so",
        },
};

static const test_utils::TestPkg::Config kCompatDriverPkgConfig = {
    .main_module = {.test_pkg_path = "/pkg/lib/fake_compat_driver.so",
                    .open_path = compat_driver_binary},
    .expected_libs = {},
    .additional_modules = {test_utils::TestPkg::ModuleConfig{
        .test_pkg_path = "/pkg/lib/fake_v1_driver.so", .open_path = "driver/fake_v1_driver.so"}},
};

// The tests that use these configs don't actually run the driver, so we can
// just point it at the placeholder fake_driver.so that will be accepted
// by the loader library. We can replace them in future with a custom .so
// if needed.
static const test_utils::TestPkg::Config kDefaultSecondDriverPkgConfig = {
    .main_module = {.test_pkg_path = "/pkg/lib/fake_driver.so", .open_path = second_driver_binary},
    .expected_libs = {},
};

static const test_utils::TestPkg::Config kDefaultThirdDriverPkgConfig = {
    .main_module = {.test_pkg_path = "/pkg/lib/fake_driver.so",
                    .open_path = "driver/third-driver.so"},
    .expected_libs = {},
};

static const test_utils::TestPkg::Config kDefaultDriverPkgConfig = {
    .main_module = {.test_pkg_path = "/pkg/lib/fake_driver.so", .open_path = "driver/driver.so"},
    .expected_libs = {},
};

static const test_utils::TestPkg::Config kDefaultCompositeDriverPkgConfig = {
    .main_module = {.test_pkg_path = "/pkg/lib/fake_driver.so",
                    .open_path = "driver/composite-driver.so"},
    .expected_libs = {},
};

struct NodeChecker {
  std::vector<std::string> node_name;
  std::vector<std::string> child_names;
  std::map<std::string, std::string> str_properties;
  std::map<std::string, std::vector<std::string>> array_str_properties;
};

struct CreatedChild {
  std::optional<fidl::Client<fdfw::Node>> node;
  std::optional<fidl::Client<fdfw::NodeController>> node_controller;
};

void CheckNode(const inspect::Hierarchy& hierarchy, const NodeChecker& checker);

class TestRealm;
class TestElementControl final
    : public fidl::testing::TestBase<fuchsia_power_broker::ElementControl> {
 public:
  explicit TestElementControl(async_dispatcher_t* dispatcher) : dispatcher_(dispatcher) {}

  void Bind(fidl::ServerEnd<fuchsia_power_broker::ElementControl> request) {
    bindings_.AddBinding(dispatcher_, std::move(request), this, fidl::kIgnoreBindingClosure);
  }

  void RegisterDependencyToken(RegisterDependencyTokenRequest& request,
                               RegisterDependencyTokenCompleter::Sync& completer) override {
    completer.Reply(zx::ok());
  }

  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_power_broker::ElementControl> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override {}

  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) override {}

 private:
  async_dispatcher_t* dispatcher_;
  fidl::ServerBindingGroup<fuchsia_power_broker::ElementControl> bindings_;
};

class TestLessor final : public fidl::testing::TestBase<fuchsia_power_broker::Lessor> {
 public:
  explicit TestLessor(async_dispatcher_t* dispatcher) : dispatcher_(dispatcher) {}

  void Bind(fidl::ServerEnd<fuchsia_power_broker::Lessor> request) {
    bindings_.AddBinding(dispatcher_, std::move(request), this, fidl::kIgnoreBindingClosure);
  }

  void Lease(LeaseRequest& request, LeaseCompleter::Sync& completer) override {
    auto endpoints = fidl::Endpoints<fuchsia_power_broker::LeaseControl>::Create();
    // Intentionally dropping the server end, the test doesn't need to keep it alive.
    completer.Reply(zx::ok(fuchsia_power_broker::LessorLeaseResponse(std::move(endpoints.client))));
  }

  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_power_broker::Lessor> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override {}

  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) override {}

 private:
  async_dispatcher_t* dispatcher_;
  fidl::ServerBindingGroup<fuchsia_power_broker::Lessor> bindings_;
};

class TestCpuElementManager final
    : public fidl::testing::TestBase<fuchsia_power_system::CpuElementManager> {
 public:
  explicit TestCpuElementManager(async_dispatcher_t* dispatcher) : dispatcher_(dispatcher) {
    zx::event::create(0, &cpu_token_);
  }

  void Bind(fidl::ServerEnd<fuchsia_power_system::CpuElementManager> request) {
    bindings_.AddBinding(dispatcher_, std::move(request), this, fidl::kIgnoreBindingClosure);
  }

  void GetCpuDependencyToken(GetCpuDependencyTokenCompleter::Sync& completer) override {
    zx::event clone;
    ZX_ASSERT(cpu_token_.duplicate(ZX_RIGHT_SAME_RIGHTS, &clone) == ZX_OK);
    fuchsia_power_system::Cpu cpu;
    cpu.assertive_dependency_token(std::move(clone));
    completer.Reply(std::move(cpu));
  }

  void AddExecutionStateDependency(AddExecutionStateDependencyRequest& request,
                                   AddExecutionStateDependencyCompleter::Sync& completer) override {
    completer.Reply(fit::ok());
  }

  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_power_system::CpuElementManager> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override {}

  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) override {}

 private:
  async_dispatcher_t* dispatcher_;
  zx::event cpu_token_;
  fidl::ServerBindingGroup<fuchsia_power_system::CpuElementManager> bindings_;
};

class TestTopology final : public fidl::testing::TestBase<fuchsia_power_broker::Topology> {
 public:
  explicit TestTopology(async_dispatcher_t* dispatcher)
      : dispatcher_(dispatcher), element_control_(dispatcher), lessor_(dispatcher) {}

  void Bind(fidl::ServerEnd<fuchsia_power_broker::Topology> request) {
    bindings_.AddBinding(dispatcher_, std::move(request), this, fidl::kIgnoreBindingClosure);
  }

  void AddElement(AddElementRequest& request, AddElementCompleter::Sync& completer) override {
    if (request.element_control().has_value()) {
      element_control_.Bind(std::move(request.element_control().value()));
    }
    if (request.lessor_channel().has_value()) {
      lessor_.Bind(std::move(request.lessor_channel().value()));
    }
    completer.Reply(zx::ok());
  }

  void Lease(LeaseRequest& request, LeaseCompleter::Sync& completer) override {
    lease_requests_.push_back(std::move(request));
    completer.Reply(fit::ok());
  }

  std::vector<LeaseRequest>& lease_requests() { return lease_requests_; }

  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_power_broker::Topology> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override {}

  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) override {}

 private:
  async_dispatcher_t* dispatcher_;
  fidl::ServerBindingGroup<fuchsia_power_broker::Topology> bindings_;
  TestElementControl element_control_;
  TestLessor lessor_;
  std::vector<LeaseRequest> lease_requests_;
};

class TestController final : public fidl::testing::TestBase<fuchsia_component::Controller> {
 public:
  explicit TestController(TestRealm* parent, std::string_view name, std::string_view collection,
                          fidl::ServerEnd<fuchsia_component::Controller> controller,
                          async_dispatcher_t* dispatcher);

  void Destroy(DestroyCompleter::Sync& completer) override;

  void Start(StartRequest& request, StartCompleter::Sync& completer) override;

  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_component::Controller> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override {}

  void NotImplemented_(const std::string& name, ::fidl::CompleterBase& completer) override {
    printf("Not implemented: TestController::%s\n", name.c_str());
  }

 private:
  TestRealm* parent_;
  std::string name_;
  std::string collection_;
  async_dispatcher_t* dispatcher_;
  fidl::ServerBinding<fuchsia_component::Controller> binding_;
};

class TestRealm : public fidl::testing::TestBase<fuchsia_component::Realm> {
 public:
  explicit TestRealm(async_dispatcher_t* dispatcher) : dispatcher_(dispatcher) {}

  using CreateChildHandler = fit::function<void(fdecl::CollectionRef collection, fdecl::Child decl,
                                                std::vector<fdecl::Offer> offers)>;
  using OpenExposedDirHandler =
      fit::function<void(fdecl::ChildRef child, fidl::ServerEnd<fio::Directory> exposed_dir)>;

  using CreateChildHandler2 =
      fit::function<bool(const fdecl::CollectionRef& collection, const fdecl::Child& decl,
                         const std::vector<fdecl::Offer>& offers)>;

  void SetCreateChildHandler(CreateChildHandler create_child_handler) {
    create_child_handler_ = std::move(create_child_handler);
  }

  void AddCreateChildHandler(CreateChildHandler2 create_child_handler) {
    create_child_handlers_.push_back(std::move(create_child_handler));
  }

  void ClearCreateChildHandlers() { create_child_handlers_.clear(); }

  void SetOpenExposedDirHandler(OpenExposedDirHandler open_exposed_dir_handler) {
    open_exposed_dir_handler_ = std::move(open_exposed_dir_handler);
  }

  void SetHandles(std::vector<fprocess::HandleInfo> handles);

  fidl::VectorView<fprocess::wire::HandleInfo> TakeHandles(fidl::AnyArena& arena);

  bool HasHandles() const { return handles_.has_value() && !handles_->empty(); }

  void MarkChildDestroyed(std::string_view name, std::string_view collection);

  void AssertDestroyedChildren(const std::vector<fdecl::ChildRef>& expected);

  void RemoveController(const std::string& name);

  void CreateChild(CreateChildRequest& request, CreateChildCompleter::Sync& completer) override;

  void OpenExposedDir(OpenExposedDirRequest& request,
                      OpenExposedDirCompleter::Sync& completer) override;

  void DestroyChild(DestroyChildRequest& request, DestroyChildCompleter::Sync& completer) override;

 private:
  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) override {
    printf("Not implemented: Realm::%s\n", name.c_str());
  }

  async_dispatcher_t* dispatcher_;
  CreateChildHandler create_child_handler_;
  std::vector<CreateChildHandler2> create_child_handlers_;
  OpenExposedDirHandler open_exposed_dir_handler_;
  std::optional<std::vector<fprocess::HandleInfo>> handles_;
  std::unordered_map<std::string, TestController> controllers_;
  std::vector<fdecl::ChildRef> destroyed_children_;
};

class StopListener final
    : public fidl::WireAsyncEventHandler<fuchsia_component_runner::ComponentController> {
 public:
  explicit StopListener(async_dispatcher_t* dispatcher,
                        fidl::ClientEnd<fuchsia_component_runner::ComponentController> client,
                        fidl::AnyTeardownObserver observer);

  bool is_stopped() const;

  fidl::WireClient<fuchsia_component_runner::ComponentController>& client() { return client_; }

  void on_fidl_error(::fidl::UnbindInfo error) override;

  void OnStop(
      fidl::WireEvent<fuchsia_component_runner::ComponentController::OnStop>* event) override;
  void handle_unknown_event(
      fidl::UnknownEventMetadata<fuchsia_component_runner::ComponentController> metadata) override {
  }

 private:
  bool stopped_ = false;
  fidl::WireClient<fuchsia_component_runner::ComponentController> client_;
  fidl::AnyTeardownObserver observer_;
};

class TestIntrospector : public fidl::testing::TestBase<fuchsia_component::Introspector> {
 public:
  void Enable() { enabled_ = true; }
  zx::event GetTokenForName(std::string_view name);
  void GetMoniker(GetMonikerRequest& request, GetMonikerCompleter::Sync& completer) override;

  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_component::Introspector> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override {}
  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) override {
    printf("Not implemented: Instrospector::%s\n", name.c_str());
  }

 private:
  std::unordered_map<zx_koid_t, std::string> entries_;
  bool enabled_ = false;
};

class TestCapStore : public fidl::testing::TestBase<fuchsia_component_sandbox::CapabilityStore> {
 public:
  void Import(ImportRequest& request, ImportCompleter::Sync& completer) override {
    completer.Reply(fit::ok());
  }
  void DictionaryCopy(DictionaryCopyRequest& request,
                      DictionaryCopyCompleter::Sync& completer) override {
    completer.Reply(fit::ok());
  }
  void Export(ExportRequest& request, ExportCompleter::Sync& completer) override {
    zx::eventpair d_ep1, d_ep2;
    ZX_ASSERT(zx::eventpair::create(0, &d_ep1, &d_ep2) == ZX_OK);
    fuchsia_component_sandbox::DictionaryRef dict_ref{{.token = std::move(d_ep1)}};
    completer.Reply(
        fit::ok(fuchsia_component_sandbox::Capability::WithDictionary(std::move(dict_ref))));
  }

 private:
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_component_sandbox::CapabilityStore> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override {}

  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) override {
    printf("Not implemented: CapabilityStore::%s\n", name.c_str());
  }
};

class TestDirectory final : public fidl::testing::TestBase<fio::Directory> {
 public:
  using OpenHandler =
      fit::function<void(const std::string& path, fidl::ServerEnd<fio::Node> object)>;

  explicit TestDirectory(async_dispatcher_t* dispatcher) : dispatcher_(dispatcher) {}

  void Bind(fidl::ServerEnd<fio::Directory> request);

  void SetOpenHandler(OpenHandler open_handler) { open_handler_ = std::move(open_handler); }

  void SetDirents(std::vector<std::string> dirents) { dirents_ = std::move(dirents); }

 private:
  void Open(OpenRequest& request, OpenCompleter::Sync& completer) override;

  void ReadDirents(ReadDirentsRequest& request, ReadDirentsCompleter::Sync& completer) override;

  void Rewind(RewindCompleter::Sync& completer) override;

  void Clone(CloneRequest& request, CloneCompleter::Sync& completer) override;

  void Watch(WatchRequest& request, WatchCompleter::Sync& completer) override;

  void handle_unknown_method(fidl::UnknownMethodMetadata<fio::Directory>,
                             fidl::UnknownMethodCompleter::Sync&) override;

  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) override {
    printf("Not implemented: Directory::%s\n", name.c_str());
  }

  async_dispatcher_t* dispatcher_;
  fidl::ServerBindingGroup<fio::Directory> bindings_;
  OpenHandler open_handler_;
  std::vector<std::string> dirents_;
  bool read_dirents_called_ = false;
  bool rewind_next_ = false;
};

struct Driver {
  std::string url;
  std::string binary;
  bool colocate = false;
  bool close = false;
  bool host_restart_on_crash = false;
  bool use_next_vdso = false;
  bool use_dynamic_linker = false;
  // The driver to load under the compatibility shim.
  std::string compat;
};

class TestDriver : public fidl::testing::TestBase<fdh::Driver> {
 public:
  explicit TestDriver(async_dispatcher_t* dispatcher, fidl::ClientEnd<fdfw::Node> node,
                      std::optional<zx::event> node_token, fidl::ServerEnd<fdh::Driver> server)
      : dispatcher_(dispatcher),
        stop_handler_([]() {}),
        node_(std::move(node), dispatcher),
        node_token_(std::move(node_token)),
        driver_binding_(dispatcher, std::move(server), this, fidl::kIgnoreBindingClosure) {}

  fidl::Client<fdfw::Node>& node() { return node_; }
  const std::optional<zx::event>& node_token() const { return node_token_; }

  using StopHandler = fit::function<void()>;
  void SetStopHandler(StopHandler handler) { stop_handler_ = std::move(handler); }

  void SetDontCloseBindingInStop() { dont_close_binding_in_stop_ = true; }

  void Stop(StopCompleter::Sync& completer) override;

  void DropNode() { node_ = {}; }
  void CloseBinding() { driver_binding_.Close(ZX_OK); }

  std::shared_ptr<CreatedChild> AddChild(std::string_view child_name, bool owned, bool expect_error,
                                         const std::string& class_name = "driver_runner_test");

  std::shared_ptr<CreatedChild> AddChild(fdfw::NodeAddArgs child_args, bool owned,
                                         bool expect_error);

 private:
  async_dispatcher_t* dispatcher_;
  StopHandler stop_handler_;
  fidl::Client<fdfw::Node> node_;
  std::optional<zx::event> node_token_;
  fidl::ServerBinding<fdh::Driver> driver_binding_;
  bool dont_close_binding_in_stop_ = false;

  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) override {
    printf("Not implemented: Driver::%s\n", name.c_str());
  }
};

class TestDriverHost : public fidl::testing::TestBase<fdh::DriverHost> {
 public:
  using StartHandler =
      fit::function<void(fdfw::DriverStartArgs start_args, fidl::ServerEnd<fdh::Driver> driver)>;

  void SetStartHandler(StartHandler start_handler) { start_handler_ = std::move(start_handler); }

  void GetProcessInfo(GetProcessInfoCompleter::Sync& completer) override {
    completer.Reply(zx::ok(fdh::ProcessInfo({
        .job_koid = 1336,
        .process_koid = process_koid_,
        .main_thread_koid = 1338,
    })));
  }

  void TriggerStackTrace(TriggerStackTraceCompleter::Sync& completer) override {
    stack_trace_triggered_ = true;
  }

  bool stack_trace_triggered() const { return stack_trace_triggered_; }
  uint64_t process_koid() const { return process_koid_; }

 private:
  void Start(StartRequest& request, StartCompleter::Sync& completer) override {
    start_handler_(std::move(request.start_args()), std::move(request.driver()));
    completer.Reply(zx::ok());
  }

  void InstallLoader(InstallLoaderRequest& request,
                     InstallLoaderCompleter::Sync& completer) override {}

  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) override {
    printf("Not implemented: DriverHost::%s\n", name.data());
  }

  StartHandler start_handler_;
  bool stack_trace_triggered_ = false;
  uint64_t process_koid_ = 1337;
};

// Calls the driver host runner's component Start implementation.
void DriverHostComponentStart(driver_runner::TestRealm& realm,
                              driver_manager::DriverHostRunner& driver_host_runner,
                              fidl::ClientEnd<fuchsia_io::Directory> driver_host_pkg);

fidl::AnyTeardownObserver TeardownWatcher(size_t index, std::vector<size_t>& indices);
fdecl::ChildRef CreateChildRef(std::string name, std::string collection);

struct Driver;

class DriverRunnerTestBase : public gtest::TestLoopFixture {
 public:
  DriverRunnerTestBase();
  void TearDown() override { Unbind(); }

 protected:
  TestRealm& realm() { return realm_; }
  TestIntrospector& introspector() { return introspector_; }
  TestDirectory& driver_dir() { return driver_dir_; }
  TestDriverHost& driver_host() { return driver_host_; }

  fidl::WireClient<fuchsia_device::Controller> ConnectToDeviceController(
      std::string_view child_name);

  fidl::ClientEnd<fuchsia_component::Realm> ConnectToRealm();
  fidl::ClientEnd<fuchsia_component::Introspector> ConnectToIntrospector();
  fidl::ClientEnd<fuchsia_component_sandbox::CapabilityStore> ConnectToCapabilityStore();
  fidl::ClientEnd<fuchsia_driver_token::Debug> ConnectToDebug();

  void EnableIntrospector() { introspector_.Enable(); }

  FakeDriverIndex CreateDriverIndex();

  void SetupDriverRunner(FakeDriverIndex driver_index);

  // If |wait_for_num_drivers| is set , the driver host will be sent a message to exit after that
  // many drivers have been loaded. This only needs to be set if the test is explicitly waiting for
  // the driver host process to exit, usually to verify the exit value.
  void SetupDriverRunnerWithDynamicLinker(
      async_dispatcher_t* loader_dispatcher,
      std::unique_ptr<driver_manager::DriverHostRunner> driver_host_runner,
      FakeDriverIndex fake_driver_index,
      std::optional<uint32_t> wait_for_num_drivers = std::nullopt);

  void SetupDriverRunnerWithDynamicLinker(
      async_dispatcher_t* loader_dispatcher,
      std::unique_ptr<driver_manager::DriverHostRunner> driver_host_runner,
      std::optional<uint32_t> wait_for_num_drivers = std::nullopt);

  void SetupDriverRunner();

  void PrepareRealmForDriverComponentStart(const std::string& name, const std::string& url);

  void PrepareRealmForSecondDriverComponentStart();

  void PrepareRealmForStartDriverHost(bool use_next_vdso);

  void PrepareRealmForStartDriverHostDynamicLinker();

  StopListener& ServeStopListener(
      fidl::ClientEnd<fuchsia_component_runner::ComponentController> component,
      fidl::AnyTeardownObserver observer = fidl::AnyTeardownObserver::Noop());

  void StopDriverComponent(
      fidl::ClientEnd<fuchsia_component_runner::ComponentController> component);

  struct StartDriverResult {
    std::unique_ptr<TestDriver> driver;
    fidl::ClientEnd<fuchsia_component_runner::ComponentController> controller;
  };

  using StartDriverHandler = fit::function<void(TestDriver*, fdfw::DriverStartArgs)>;

  // If |ns_pkg| is set, it will be provided as the /pkg directory in the driver component's
  // namespace.
  // If a new driver host is required to be started (i.e. the driver is not colocated),
  // and dynamic linking is enabled, |driver_host_pkg| will be provided as the /pkg directory in the
  // driver host component's namespace.
  StartDriverResult StartDriver(
      std::string_view moniker, Driver driver,
      std::optional<StartDriverHandler> start_handler = std::nullopt,
      fidl::ClientEnd<fuchsia_io::Directory> ns_pkg = fidl::ClientEnd<fuchsia_io::Directory>(),
      fidl::ClientEnd<fuchsia_io::Directory> ns_svc = fidl::ClientEnd<fuchsia_io::Directory>(),
      fidl::ClientEnd<fuchsia_io::Directory> driver_host_pkg =
          fidl::ClientEnd<fuchsia_io::Directory>());

  // Variant of |StartDriver| that takes in a test pkg config rather than the pkg directory client.
  // If the driver has opted into dynamic linking, the fake /pkg directory will be provided to
  // the driver component's namespace.
  StartDriverResult StartDriverWithConfig(
      std::string_view moniker, Driver driver,
      std::optional<StartDriverHandler> start_handler = std::nullopt,
      test_utils::TestPkg::Config driver_config = kDefaultRootDriverPkgConfig,
      test_utils::TestPkg::Config driver_host_config = kDefaultDriverHostPkgConfig,
      fidl::ClientEnd<fuchsia_io::Directory> ns_svc = fidl::ClientEnd<fuchsia_io::Directory>());

  zx::result<StartDriverResult> StartRootDriver();
  zx::result<StartDriverResult> StartRootDriverDynamicLinking(
      test_utils::TestPkg::Config driver_host_config = kDefaultDriverHostPkgConfig,
      test_utils::TestPkg::Config driver_config = kDefaultRootDriverPkgConfig);

  StartDriverResult StartSecondDriver(
      std::string_view moniker, bool colocate = false, bool host_restart_on_crash = false,
      bool use_next_vdso = false, bool use_dynamic_linker = false,
      fidl::ClientEnd<fuchsia_io::Directory> ns_svc = fidl::ClientEnd<fuchsia_io::Directory>());

  void Unbind();

  // Simulates a (compromised) driver_host process closing its
  // fuchsia.driver.host/DriverHost server channel while keeping its per-driver
  // fuchsia.driver.host/Driver channel(s) open. This triggers
  // DriverHostComponent's ObserveTeardown lambda, which erases (and frees) the
  // DriverHostComponent from DriverRunner::driver_hosts_ without clearing any
  // Node::driver_host_ raw pointers.
  void CloseDriverHostBindings() { driver_host_bindings_.CloseAll(ZX_OK); }

  static void ValidateProgram(std::optional<::fuchsia_data::Dictionary>& program,
                              std::string_view binary, std::string_view colocate,
                              std::string_view host_restart_on_crash,
                              std::string_view use_next_vdso,
                              std::string_view use_dynamic_linker = "false",
                              std::string_view compat = "");

  static void AssertNodeBound(const std::shared_ptr<CreatedChild>& child);

  static void AssertNodeNotBound(const std::shared_ptr<CreatedChild>& child);

  static void AssertNodeControllerBound(const std::shared_ptr<CreatedChild>& child);

  static void AssertNodeControllerNotBound(const std::shared_ptr<CreatedChild>& child);

  inspect::Hierarchy Inspect();

  void SetupDevfs();

  Devfs& devfs() {
    ZX_ASSERT(devfs_);
    return *devfs_;
  }

  DriverRunner& driver_runner() { return driver_runner_.value(); }

  FakeDriverIndex& driver_index() { return driver_index_.value(); }

 protected:
  TestRealm realm_;
  TestIntrospector introspector_;
  TestCapStore cap_store_;
  TestDirectory driver_host_dir_{dispatcher()};
  TestDirectory driver_dir_{dispatcher()};
  TestDriverHost driver_host_;
  fidl::ServerBindingGroup<fuchsia_component::Realm> realm_bindings_;
  fidl::ServerBindingGroup<fuchsia_component::Introspector> introspector_bindings_;
  fidl::ServerBindingGroup<fuchsia_component_sandbox::CapabilityStore> capstore_bindings_;
  fidl::ServerBindingGroup<fdh::DriverHost> driver_host_bindings_;

  std::shared_ptr<Devfs> devfs_;
  inspect::ComponentInspector inspector_{dispatcher(), {}};
  std::optional<FakeDriverIndex> driver_index_;
  std::optional<DriverRunner> driver_runner_;
  std::unique_ptr<driver_loader::Loader> dynamic_linker_;

  std::list<StopListener> stop_listeners_;
};

}  // namespace driver_runner

#endif  // SRC_DEVICES_BIN_DRIVER_MANAGER_TESTS_DRIVER_RUNNER_TEST_FIXTURE_H_
