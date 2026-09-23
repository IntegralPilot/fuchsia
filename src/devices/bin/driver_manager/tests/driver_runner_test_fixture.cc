// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/bin/driver_manager/tests/driver_runner_test_fixture.h"

#include <fidl/fuchsia.component.decl/cpp/test_base.h>
#include <fidl/fuchsia.component/cpp/test_base.h>
#include <fidl/fuchsia.driver.framework/cpp/test_base.h>
#include <fidl/fuchsia.driver.host/cpp/test_base.h>
#include <fidl/fuchsia.io/cpp/test_base.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/fit/defer.h>
#include <lib/inspect/cpp/reader.h>
#include <lib/inspect/testing/cpp/inspect.h>

#include <bind/fuchsia/platform/cpp/bind.h>

#include "src/devices/bin/driver_manager/testing/fake_driver_index.h"
#include "src/storage/lib/vfs/cpp/synchronous_vfs.h"

namespace driver_manager {
extern bool g_use_test_process_koid;
}

namespace driver_runner {

namespace fdata = fuchsia_data;
namespace fdfw = fuchsia_driver_framework;
namespace fdh = fuchsia_driver_host;
namespace fio = fuchsia_io;
namespace fprocess = fuchsia_process;
namespace frunner = fuchsia_component_runner;
namespace fcomponent = fuchsia_component;
namespace fdecl = fuchsia_component_decl;

namespace {
zx::result<zx_koid_t> GetKoid(const zx::unowned<zx::event>& handle) {
  zx_info_handle_basic_t info{};
  if (zx_status_t status =
          handle->get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr);
      status != ZX_OK) {
    return zx::error(status);
  }
  return zx::ok(info.koid);
}
}  // namespace

void CheckNode(const inspect::Hierarchy& hierarchy, const NodeChecker& checker) {
  auto node = hierarchy.GetByPath(checker.node_name);
  ASSERT_NE(nullptr, node);

  size_t expected_children = checker.child_names.size();
  if (auto ptr = node->GetByPath({"properties"}); ptr) {
    expected_children++;
  }
  if (node->children().size() != expected_children) {
    printf("Mismatched children\n");
    for (size_t i = 0; i < node->children().size(); i++) {
      printf("Child %ld : %s\n", i, node->children()[i].name().c_str());
    }
    ASSERT_EQ(node->children().size(), expected_children);
  }

  for (auto& child : checker.child_names) {
    auto ptr = node->GetByPath({child});
    ASSERT_NE(nullptr, ptr) << "Failed to find child " << child;
  }

  for (auto& property : checker.str_properties) {
    auto prop = node->node().get_property<inspect::StringPropertyValue>(property.first);
    ASSERT_TRUE(prop) << "Failed to find property " << property.first;
    ASSERT_EQ(property.second, prop->value());
  }
  for (auto& property : checker.array_str_properties) {
    auto prop = node->node().get_property<inspect::StringArrayValue>(property.first);
    ASSERT_TRUE(prop) << "Failed to find property " << property.first;
    ASSERT_EQ(property.second, prop->value());
  }
}

zx::result<fidl::ClientEnd<fuchsia_ldsvc::Loader>> LoaderFactory() {
  auto endpoints = fidl::CreateEndpoints<fuchsia_ldsvc::Loader>();
  if (endpoints.is_error()) {
    return endpoints.take_error();
  }
  return zx::ok(std::move(endpoints->client));
}

zx::result<fidl::ClientEnd<fuchsia_driver_loader::DriverHostLauncher>> DynamicLinkerFactory(
    driver_loader::Loader* loader) {
  auto [client_end, server_end] =
      fidl::Endpoints<fuchsia_driver_loader::DriverHostLauncher>::Create();
  loader->Connect(std::move(server_end));
  return zx::ok(std::move(client_end));
}

fdecl::ChildRef CreateChildRef(std::string name, std::string collection) {
  return fdecl::ChildRef({.name = std::move(name), .collection = std::move(collection)});
}

class FakeContext : public fpromise::context {
 public:
  fpromise::executor* executor() const override {
    EXPECT_TRUE(false);
    return nullptr;
  }

  fpromise::suspended_task suspend_task() override {
    EXPECT_TRUE(false);
    return fpromise::suspended_task();
  }
};

fidl::AnyTeardownObserver TeardownWatcher(size_t index, std::vector<size_t>& indices) {
  return fidl::ObserveTeardown([&indices = indices, index] { indices.emplace_back(index); });
}

TestController::TestController(TestRealm* parent, std::string_view name,
                               std::string_view collection,
                               fidl::ServerEnd<fcomponent::Controller> controller,
                               async_dispatcher_t* dispatcher)
    : parent_(parent),
      name_(name),
      collection_(collection),
      dispatcher_(dispatcher),
      binding_(dispatcher_, std::move(controller), this, fidl::kIgnoreBindingClosure) {}

void TestController::Destroy(DestroyCompleter::Sync& completer) {
  completer.Reply(fit::ok());

  // Post this as a task since the real component manager replies ok to indicate destroy has
  // started, not that it has completed. This will allow driver manager to see the ok response
  // before the controller closing.
  async::PostTask(dispatcher_, [this]() {
    parent_->MarkChildDestroyed(name_, collection_);
    parent_->RemoveController(name_);
  });
}

void TestController::Start(StartRequest& request, StartCompleter::Sync& completer) {
  if (request.args().numbered_handles()) {
    parent_->SetHandles(std::move(request.args().numbered_handles().value()));
  }

  completer.Reply(fit::ok());
}

void TestRealm::MarkChildDestroyed(std::string_view name, std::string_view collection) {
  destroyed_children_.push_back(
      fuchsia_component_decl::ChildRef(std::string(name), {std::string(collection)}));
}

void TestRealm::AssertDestroyedChildren(const std::vector<fdecl::ChildRef>& expected) {
  auto destroyed_children = destroyed_children_;
  for (const auto& child : expected) {
    auto it = std::find_if(destroyed_children.begin(), destroyed_children.end(),
                           [&child](const fdecl::ChildRef& other) {
                             return child.name() == other.name() &&
                                    child.collection() == other.collection();
                           });
    ASSERT_NE(it, destroyed_children.end());
    destroyed_children.erase(it);
  }
  ASSERT_EQ(destroyed_children.size(), 0ul);
}

void TestRealm::RemoveController(const std::string& name) { controllers_.erase(name); }

void TestRealm::CreateChild(CreateChildRequest& request, CreateChildCompleter::Sync& completer) {
  handles_ = std::move(request.args().numbered_handles());

  if (request.args().controller().has_value()) {
    controllers_.emplace(
        std::piecewise_construct, std::forward_as_tuple(request.decl().name().value()),
        std::forward_as_tuple(this, request.decl().name().value(), request.collection().name(),
                              std::move(request.args().controller().value()), dispatcher_));
  }

  auto offers = request.args().dynamic_offers();
  bool handled = false;
  for (auto it = create_child_handlers_.rbegin(); it != create_child_handlers_.rend(); ++it) {
    if ((*it)(request.collection(), request.decl(),
              offers.has_value() ? offers.value() : std::vector<fdecl::Offer>{})) {
      handled = true;
      break;
    }
  }

  if (!handled && create_child_handler_) {
    create_child_handler_(request.collection(), request.decl(),
                          offers.has_value() ? offers.value() : std::vector<fdecl::Offer>{});
  }
  completer.Reply(fit::ok());
}

void TestRealm::OpenExposedDir(OpenExposedDirRequest& request,
                               OpenExposedDirCompleter::Sync& completer) {
  open_exposed_dir_handler_(std::move(request.child()), std::move(request.exposed_dir()));
  completer.Reply(fidl::Response<fcomponent::Realm::OpenExposedDir>(fit::ok()));
}

void TestRealm::DestroyChild(DestroyChildRequest& request, DestroyChildCompleter::Sync& completer) {
  MarkChildDestroyed(request.child().name(), request.child().collection().value_or(""));
  RemoveController(request.child().name());
  completer.Reply(fit::ok());
}

class TestTransaction : public fidl::Transaction {
 public:
  explicit TestTransaction(std::string_view name, bool close) : name_(name), close_(close) {}

 private:
  std::unique_ptr<Transaction> TakeOwnership() override {
    return std::make_unique<TestTransaction>(name_, close_);
  }

  zx_status_t Reply(fidl::OutgoingMessage* message, fidl::WriteOptions write_options) override {
    EXPECT_TRUE(false);
    return ZX_OK;
  }

  void Close(zx_status_t epitaph) override {
    EXPECT_TRUE(close_) << "epitaph: " << zx_status_get_string(epitaph) << "\n"
                        << "name: " << name_.c_str();
  }

  std::string name_;
  bool close_;
};

void DriverHostComponentStart(driver_runner::TestRealm& realm,
                              driver_manager::DriverHostRunner& driver_host_runner,
                              fidl::ClientEnd<fuchsia_io::Directory> driver_host_pkg) {
  fidl::Arena arena;

  fidl::VectorView<fdata::wire::DictionaryEntry> program_entries(arena, 1);
  program_entries[0].key.Set(arena, "binary");
  program_entries[0].value = fdata::wire::DictionaryValue::WithStr(arena, "bin/driver_host2");
  auto program_builder = fdata::wire::Dictionary::Builder(arena);
  program_builder.entries(program_entries);

  fidl::VectorView<frunner::wire::ComponentNamespaceEntry> ns_entries(arena, 1);
  ns_entries[0] = frunner::wire::ComponentNamespaceEntry::Builder(arena)
                      .path("/pkg")
                      .directory(std::move(driver_host_pkg))
                      .Build();

  auto start_info_builder = frunner::wire::ComponentStartInfo::Builder(arena);
  start_info_builder.resolved_url("fuchsia-boot:///driver_host2#meta/driver_host2.cm")
      .program(program_builder.Build())
      .ns(ns_entries)
      .numbered_handles(realm.TakeHandles(arena));

  auto controller_endpoints = fidl::Endpoints<frunner::ComponentController>::Create();
  TestTransaction transaction("driver host", false);
  {
    fidl::WireServer<frunner::ComponentRunner>::StartCompleter::Sync completer(&transaction);
    fidl::WireRequest<frunner::ComponentRunner::Start> request{
        start_info_builder.Build(), std::move(controller_endpoints.server)};
    static_cast<fidl::WireServer<frunner::ComponentRunner>&>(driver_host_runner)
        .Start(&request, completer);
  }
}

DriverRunnerTestBase::DriverRunnerTestBase() : realm_(dispatcher()) {
  driver_manager::g_use_test_process_koid = true;
}

fidl::ClientEnd<fcomponent::Realm> DriverRunnerTestBase::ConnectToRealm() {
  auto realm_endpoints = fidl::Endpoints<fcomponent::Realm>::Create();
  realm_bindings_.AddBinding(dispatcher(), std::move(realm_endpoints.server), &realm_,
                             fidl::kIgnoreBindingClosure);
  return std::move(realm_endpoints.client);
}

fidl::ClientEnd<fcomponent::Introspector> DriverRunnerTestBase::ConnectToIntrospector() {
  auto introspector_endpoints = fidl::Endpoints<fcomponent::Introspector>::Create();
  introspector_bindings_.AddBinding(dispatcher(), std::move(introspector_endpoints.server),
                                    &introspector_, fidl::kIgnoreBindingClosure);
  return std::move(introspector_endpoints.client);
}

fidl::ClientEnd<fuchsia_component_sandbox::CapabilityStore>
DriverRunnerTestBase::ConnectToCapabilityStore() {
  auto store_endpoints = fidl::Endpoints<fuchsia_component_sandbox::CapabilityStore>::Create();
  capstore_bindings_.AddBinding(dispatcher(), std::move(store_endpoints.server), &cap_store_,
                                fidl::kIgnoreBindingClosure);
  return std::move(store_endpoints.client);
}

fidl::ClientEnd<fuchsia_driver_token::Debug> DriverRunnerTestBase::ConnectToDebug() {
  auto debug_endpoints = fidl::Endpoints<fuchsia_driver_token::Debug>::Create();
  fidl::BindServer(dispatcher(), std::move(debug_endpoints.server), &driver_runner());
  return std::move(debug_endpoints.client);
}

FakeDriverIndex DriverRunnerTestBase::CreateDriverIndex() {
  return FakeDriverIndex(dispatcher(), [](auto args) -> zx::result<FakeDriverIndex::MatchResult> {
    if (args.name().get() == "second") {
      return zx::ok(FakeDriverIndex::MatchResult{
          .url = second_driver_url,
      });
    }

    if (args.name().get() == "dev-group-0") {
      return zx::ok(FakeDriverIndex::MatchResult{
          .spec = fdfw::CompositeParent({
              .composite = fdfw::CompositeInfo{{
                  .spec = fdfw::CompositeNodeSpec{{
                      .name = "test-group",
                      .parents = std::vector<fdfw::ParentSpec>(2),
                  }},
                  .matched_driver = fdfw::CompositeDriverMatch{{
                      .composite_driver = fdfw::CompositeDriverInfo{{
                          .composite_name = "test-composite",
                          .driver_info = fdfw::DriverInfo{{
                              .url = "fuchsia-boot:///#meta/composite-driver.cm",
                              .colocate = true,
                              .package_type = fdfw::DriverPackageType::kBoot,
                          }},
                      }},
                      .parent_names = {{"node-0", "node-1"}},
                      .primary_parent_index = 1,
                  }},
              }},
              .index = 0,
          })});
    }

    if (args.name().get() == "dev-group-1") {
      return zx::ok(FakeDriverIndex::MatchResult{
          .spec = fdfw::CompositeParent({
              .composite = fdfw::CompositeInfo{{
                  .spec = fdfw::CompositeNodeSpec{{
                      .name = "test-group",
                      .parents = std::vector<fdfw::ParentSpec>(2),
                  }},
                  .matched_driver = fdfw::CompositeDriverMatch{{
                      .composite_driver = fdfw::CompositeDriverInfo{{
                          .composite_name = "test-composite",
                          .driver_info = fdfw::DriverInfo{{
                              .url = "fuchsia-boot:///#meta/composite-driver.cm",
                              .colocate = true,
                              .package_type = fdfw::DriverPackageType::kBoot,
                          }},
                      }},
                      .parent_names = {{"node-0", "node-1"}},
                      .primary_parent_index = 1,
                  }},
              }},
              .index = 1,
          })});
    }

    return zx::error(ZX_ERR_NOT_FOUND);
  });
}
void DriverRunnerTestBase::SetupDriverRunner(FakeDriverIndex driver_index) {
  driver_index_.emplace(std::move(driver_index));
  driver_runner_.emplace(ConnectToRealm(), ConnectToIntrospector(), ConnectToCapabilityStore(),
                         driver_index_->Connect(), inspector_, &LoaderFactory, dispatcher(), false,
                         driver_manager::OfferInjector{{
                             .power_inject_offer = false,
                             .power_suspend_enabled = false,
                         }},
                         fidl::ClientEnd<fuchsia_power_broker::Topology>());
  SetupDevfs();
}

void DriverRunnerTestBase::SetupDriverRunnerWithDynamicLinker(
    async_dispatcher_t* loader_dispatcher,
    std::unique_ptr<driver_manager::DriverHostRunner> driver_host_runner,
    FakeDriverIndex driver_index, std::optional<uint32_t> wait_for_num_drivers) {
  driver_index_.emplace(std::move(driver_index));
  auto load_driver_handler =
      [num_drivers_loaded = 0, wait_for_num_drivers](
          zx::unowned_channel bootstrap_sender,
          driver_loader::Loader::DynamicLinkingPassiveAbi dl_passive_abi) mutable {
        ASSERT_EQ(ZX_OK,
                  bootstrap_sender->write(0, &dl_passive_abi, sizeof(dl_passive_abi), nullptr, 0));
        num_drivers_loaded++;
        if (wait_for_num_drivers.has_value() && (wait_for_num_drivers == num_drivers_loaded)) {
          // Send a message for the driver host to exit.
          dl_passive_abi = 0;
          ASSERT_EQ(ZX_OK, bootstrap_sender->write(0, &dl_passive_abi, sizeof(dl_passive_abi),
                                                   nullptr, 0));
        }
      };
  dynamic_linker_ =
      driver_loader::Loader::Create(loader_dispatcher, std::move(load_driver_handler));
  driver_runner_.emplace(
      ConnectToRealm(), ConnectToIntrospector(), ConnectToCapabilityStore(),
      driver_index_->Connect(), inspector_, &LoaderFactory, dispatcher(), false,
      driver_manager::OfferInjector{{
          .power_inject_offer = false,
          .power_suspend_enabled = false,
      }},
      fidl::ClientEnd<fuchsia_power_broker::Topology>(),
      driver_manager::DriverRunner::DynamicLinkerArgs{
          [loader = dynamic_linker_.get()]() { return DynamicLinkerFactory(loader); },
          std::move(driver_host_runner)});
  SetupDevfs();
}

void DriverRunnerTestBase::SetupDriverRunnerWithDynamicLinker(
    async_dispatcher_t* loader_dispatcher,
    std::unique_ptr<driver_manager::DriverHostRunner> driver_host_runner,
    std::optional<uint32_t> wait_for_num_drivers) {
  SetupDriverRunnerWithDynamicLinker(loader_dispatcher, std::move(driver_host_runner),
                                     CreateDriverIndex(), wait_for_num_drivers);
}

void DriverRunnerTestBase::SetupDriverRunner() { SetupDriverRunner(CreateDriverIndex()); }
void DriverRunnerTestBase::PrepareRealmForDriverComponentStart(const std::string& name,
                                                               const std::string& url) {
  realm().SetCreateChildHandler(
      [name, url](fdecl::CollectionRef collection, fdecl::Child decl, auto offers) {
        EXPECT_EQ("boot-drivers", collection.name());
        EXPECT_EQ(name, decl.name().value());
        EXPECT_EQ(url, decl.url().value());
      });
}
void DriverRunnerTestBase::PrepareRealmForSecondDriverComponentStart() {
  PrepareRealmForDriverComponentStart("dev.second", second_driver_url);
}
void DriverRunnerTestBase::PrepareRealmForStartDriverHost(bool use_next_vdso) {
  constexpr std::string_view kDriverHostName = "driver-host-";
  std::string coll = "driver-hosts";
  realm().SetCreateChildHandler(
      [coll, kDriverHostName, use_next_vdso](fdecl::CollectionRef collection, fdecl::Child decl,
                                             auto offers) {
        EXPECT_EQ(coll, collection.name());
        EXPECT_EQ(kDriverHostName, decl.name().value().substr(0, kDriverHostName.size()));
        if (use_next_vdso) {
          EXPECT_EQ("fuchsia-boot:///driver_host#meta/driver_host_next.cm", decl.url());
        } else {
          EXPECT_EQ("fuchsia-boot:///driver_host#meta/driver_host.cm", decl.url());
        }
      });
  realm().SetOpenExposedDirHandler(
      [this, coll, kDriverHostName](fdecl::ChildRef child, auto exposed_dir) {
        EXPECT_EQ(coll, child.collection().value_or(""));
        EXPECT_EQ(kDriverHostName, child.name().substr(0, kDriverHostName.size()));
        driver_host_dir_.Bind(std::move(exposed_dir));
      });
  driver_host_dir_.SetOpenHandler([this](const std::string& path, auto object) {
    EXPECT_EQ(fidl::DiscoverableProtocolName<fdh::DriverHost>, path);
    driver_host_bindings_.AddBinding(dispatcher(),
                                     fidl::ServerEnd<fdh::DriverHost>(object.TakeChannel()),
                                     &driver_host_, fidl::kIgnoreBindingClosure);
  });
}

void DriverRunnerTestBase::PrepareRealmForStartDriverHostDynamicLinker() {
  constexpr std::string_view kCollection = "driver-hosts";
  constexpr std::string_view kDriverHostName = "driver-host-new-";
  constexpr std::string_view kComponentUrl = "fuchsia-boot:///driver_host2#meta/driver_host2.cm";

  realm().SetCreateChildHandler(
      [kCollection, kDriverHostName, kComponentUrl](fdecl::CollectionRef collection,
                                                    fdecl::Child decl, auto offers) {
        EXPECT_EQ(kCollection, collection.name());
        EXPECT_EQ(kDriverHostName, decl.name().value().substr(0, kDriverHostName.size()));
        EXPECT_EQ(kComponentUrl, decl.url());
      });
  realm().SetOpenExposedDirHandler(
      [this, kCollection, kDriverHostName](fdecl::ChildRef child, auto exposed_dir) {
        EXPECT_EQ(kCollection, child.collection().value_or(""));
        EXPECT_EQ(kDriverHostName, child.name().substr(0, kDriverHostName.size()));
        driver_host_dir_.Bind(std::move(exposed_dir));
      });
  driver_host_dir_.SetOpenHandler([this](const std::string& path, auto object) {
    EXPECT_EQ(fidl::DiscoverableProtocolName<fdh::DriverHost>, path);
    driver_host_bindings_.AddBinding(dispatcher(),
                                     fidl::ServerEnd<fdh::DriverHost>(object.TakeChannel()),
                                     &driver_host_, fidl::kIgnoreBindingClosure);
  });
}

StopListener::StopListener(async_dispatcher_t* dispatcher,
                           fidl::ClientEnd<frunner::ComponentController> client,
                           fidl::AnyTeardownObserver observer)
    : client_(std::move(client), dispatcher, this), observer_(std::move(observer)) {}

void StopListener::on_fidl_error(::fidl::UnbindInfo error) {
  stopped_ = true;
  std::move(observer_).Notify();
}

bool StopListener::is_stopped() const { return stopped_; }

void StopListener::OnStop(fidl::WireEvent<frunner::ComponentController::OnStop>* event) {
  stopped_ = true;
  auto _ = client_.UnbindMaybeGetEndpoint();
  std::move(observer_).Notify();
}

StopListener& DriverRunnerTestBase::ServeStopListener(
    fidl::ClientEnd<frunner::ComponentController> component, fidl::AnyTeardownObserver observer) {
  return stop_listeners_.emplace_back(dispatcher(), std::move(component), std::move(observer));
}

void DriverRunnerTestBase::StopDriverComponent(
    fidl::ClientEnd<frunner::ComponentController> component) {
  StopListener listener(dispatcher(), std::move(component), fidl::AnyTeardownObserver::Noop());
  auto stop_result = listener.client()->Stop();
  ASSERT_EQ(ZX_OK, stop_result.status());

  while (!listener.is_stopped()) {
    RunLoopUntilIdle();
  }
}
DriverRunnerTestBase::StartDriverResult DriverRunnerTestBase::StartDriver(
    std::string_view moniker, Driver driver, std::optional<StartDriverHandler> start_handler,
    fidl::ClientEnd<fuchsia_io::Directory> ns_pkg, fidl::ClientEnd<fuchsia_io::Directory> ns_svc,
    fidl::ClientEnd<fuchsia_io::Directory> driver_host_pkg) {
  auto started_driver = std::make_shared<std::unique_ptr<TestDriver>>();
  driver_host().SetStartHandler(
      [started_driver, dispatcher = dispatcher(), start_handler = std::move(start_handler)](
          fdfw::DriverStartArgs start_args, fidl::ServerEnd<fdh::Driver> driver) mutable {
        *started_driver =
            std::make_unique<TestDriver>(dispatcher, std::move(start_args.node().value()),
                                         std::move(start_args.node_token()), std::move(driver));
        start_args.node().reset();
        if (start_handler.has_value()) {
          start_handler.value()((*started_driver).get(), std::move(start_args));
        }
      });

  if (!driver.colocate) {
    if (driver.use_dynamic_linker) {
      PrepareRealmForStartDriverHostDynamicLinker();
    } else {
      PrepareRealmForStartDriverHost(driver.use_next_vdso);
    }
  }

  fidl::Arena arena;

  // The "compat" field is optional.
  size_t num_program_entries = (driver.compat == "") ? 5 : 6;

  fidl::VectorView<fdata::wire::DictionaryEntry> program_entries(arena, num_program_entries);
  program_entries[0].key.Set(arena, "binary");
  program_entries[0].value = fdata::wire::DictionaryValue::WithStr(arena, driver.binary);

  program_entries[1].key.Set(arena, "colocate");
  program_entries[1].value =
      fdata::wire::DictionaryValue::WithStr(arena, driver.colocate ? "true" : "false");

  program_entries[2].key.Set(arena, "host_restart_on_crash");
  program_entries[2].value =
      fdata::wire::DictionaryValue::WithStr(arena, driver.host_restart_on_crash ? "true" : "false");

  program_entries[3].key.Set(arena, "use_next_vdso");
  program_entries[3].value =
      fdata::wire::DictionaryValue::WithStr(arena, driver.use_next_vdso ? "true" : "false");

  program_entries[4].key.Set(arena, "use_dynamic_linker");
  program_entries[4].value =
      fdata::wire::DictionaryValue::WithStr(arena, driver.use_dynamic_linker ? "true" : "false");

  if (driver.compat != "") {
    program_entries[5].key.Set(arena, "compat");
    program_entries[5].value = fdata::wire::DictionaryValue::WithStr(arena, driver.compat);
  }

  auto program_builder = fdata::wire::Dictionary::Builder(arena);
  program_builder.entries(program_entries);

  auto outgoing_endpoints = fidl::CreateEndpoints<fuchsia_io::Directory>();
  EXPECT_EQ(ZX_OK, outgoing_endpoints.status_value());

  auto start_info_builder = frunner::wire::ComponentStartInfo::Builder(arena);

  size_t num_ns_entries = 0;
  if (ns_pkg.is_valid()) {
    num_ns_entries++;
  }
  if (ns_svc.is_valid()) {
    num_ns_entries++;
  }

  fidl::VectorView<frunner::wire::ComponentNamespaceEntry> ns_entries = {};
  if (num_ns_entries > 0) {
    ns_entries.Allocate(arena, num_ns_entries);
    size_t i = 0;
    if (ns_pkg.is_valid()) {
      ns_entries[i++] = frunner::wire::ComponentNamespaceEntry::Builder(arena)
                            .path("/pkg")
                            .directory(std::move(ns_pkg))
                            .Build();
    }
    if (ns_svc.is_valid()) {
      ns_entries[i++] = frunner::wire::ComponentNamespaceEntry::Builder(arena)
                            .path("/svc")
                            .directory(std::move(ns_svc))
                            .Build();
    }
  }

  start_info_builder.resolved_url(driver.url)
      .program(program_builder.Build())
      .outgoing_dir(std::move(outgoing_endpoints->server))
      .ns(ns_entries)
      .numbered_handles(realm().TakeHandles(arena))
      .component_instance(introspector_.GetTokenForName(moniker));

  auto controller_endpoints = fidl::Endpoints<frunner::ComponentController>::Create();
  TestTransaction transaction(moniker, driver.close);
  {
    fidl::WireServer<frunner::ComponentRunner>::StartCompleter::Sync completer(&transaction);
    fidl::WireRequest<frunner::ComponentRunner::Start> request{
        start_info_builder.Build(), std::move(controller_endpoints.server)};
    static_cast<fidl::WireServer<frunner::ComponentRunner>&>(driver_runner().runner_for_tests())
        .Start(&request, completer);
  }
  RunLoopUntilIdle();

  // The driver manager is waiting for the component framework to call the driver
  // host runner's component Start implementation. We need to call it
  // now to continue with starting the driver host and subsequently the driver.
  //
  // If the driver |Start| request is expected to fail (|driver.close| is true),
  // then we should not start the driver host.
  if (!driver.colocate && driver.use_dynamic_linker && !driver.close) {
    auto deadline = zx::deadline_after(zx::sec(5));
    while (!realm().HasHandles() && zx::clock::get_monotonic() < deadline) {
      RunLoopUntilIdle();
      zx::nanosleep(zx::deadline_after(zx::msec(10)));
    }
    if (realm().HasHandles()) {
      DriverHostComponentStart(realm(), *driver_runner().driver_host_runner_for_tests(),
                               std::move(driver_host_pkg));
      RunLoopUntilIdle();
    }
  }

  if (!driver.close) {
    auto deadline = zx::deadline_after(zx::sec(5));
    while (!*started_driver && zx::clock::get_monotonic() < deadline) {
      RunLoopUntilIdle();
      zx::nanosleep(zx::deadline_after(zx::msec(10)));
    }
  }

  return {std::move(*started_driver), std::move(controller_endpoints.client)};
}

DriverRunnerTestBase::StartDriverResult DriverRunnerTestBase::StartDriverWithConfig(
    std::string_view moniker, Driver driver, std::optional<StartDriverHandler> start_handler,
    test_utils::TestPkg::Config driver_config, test_utils::TestPkg::Config driver_host_config,
    fidl::ClientEnd<fuchsia_io::Directory> ns_svc) {
  fidl::Endpoints<fuchsia_io::Directory> child_pkg_endpoints;
  std::unique_ptr<test_utils::TestPkg> child_test_pkg;
  if (driver.use_dynamic_linker) {
    child_pkg_endpoints = fidl::Endpoints<fuchsia_io::Directory>::Create();
    child_test_pkg =
        std::make_unique<test_utils::TestPkg>(std::move(child_pkg_endpoints.server), driver_config);
  }
  fidl::Endpoints<fuchsia_io::Directory> driver_host_pkg_endpoints;
  std::unique_ptr<test_utils::TestPkg> driver_host_test_pkg;
  if (!driver.colocate) {
    driver_host_pkg_endpoints = fidl::Endpoints<fuchsia_io::Directory>::Create();
    driver_host_test_pkg = std::make_unique<test_utils::TestPkg>(
        std::move(driver_host_pkg_endpoints.server), driver_host_config);
  }
  return StartDriver(moniker, driver, std::move(start_handler),
                     std::move(child_pkg_endpoints.client), std::move(ns_svc),
                     std::move(driver_host_pkg_endpoints.client));
}

zx::result<DriverRunnerTestBase::StartDriverResult> DriverRunnerTestBase::StartRootDriver() {
  realm().SetCreateChildHandler(
      [](fdecl::CollectionRef collection, fdecl::Child decl, auto offers) {
        EXPECT_EQ("boot-drivers", collection.name());
        EXPECT_EQ("root", decl.name());
        EXPECT_EQ(root_driver_url, decl.url());
      });
  auto start = driver_runner().StartRootDriver(root_driver_url);
  if (start.is_error()) {
    return start.take_error();
  }
  EXPECT_TRUE(RunLoopUntilIdle());

  StartDriverHandler start_handler = [](TestDriver* driver, fdfw::DriverStartArgs start_args) {
    ValidateProgram(start_args.program(), root_driver_binary, "false", "false", "false");
  };
  return zx::ok(StartDriver("root",
                            {
                                .url = root_driver_url,
                                .binary = root_driver_binary,
                            },
                            std::move(start_handler)));
}

zx::result<DriverRunnerTestBase::StartDriverResult>
DriverRunnerTestBase::StartRootDriverDynamicLinking(test_utils::TestPkg::Config driver_host_config,
                                                    test_utils::TestPkg::Config driver_config) {
  PrepareRealmForDriverComponentStart("root", driver_runner::root_driver_url);

  auto start = driver_runner().StartRootDriver(driver_runner::root_driver_url);
  if (start.is_error()) {
    return start.take_error();
  }
  EXPECT_TRUE(RunLoopUntilIdle());

  auto pkg_endpoints = fidl::Endpoints<fuchsia_io::Directory>::Create();
  test_utils::TestPkg test_pkg(std::move(pkg_endpoints.server), driver_config);
  StartDriverHandler start_handler = [pkg_path = driver_config.main_module.open_path](
                                         driver_runner::TestDriver* driver,
                                         fdfw::DriverStartArgs start_args) {
    ValidateProgram(start_args.program(), pkg_path, "false" /* colocate */,
                    "false" /* host_restart_on_crash */, "false" /* use_next_vdso */,
                    "true" /* use_dynamic_linker */);
  };

  auto driver_host_pkg_endpoints = fidl::Endpoints<fuchsia_io::Directory>::Create();
  test_utils::TestPkg driver_host_test_pkg(std::move(driver_host_pkg_endpoints.server),
                                           driver_host_config);

  return zx::ok(StartDriver("root",
                            {
                                .url = driver_runner::root_driver_url,
                                .binary = std::string(driver_config.main_module.open_path),
                                .use_dynamic_linker = true,
                            },
                            std::move(start_handler), std::move(pkg_endpoints.client),
                            fidl::ClientEnd<fuchsia_io::Directory>(),
                            std::move(driver_host_pkg_endpoints.client)));
}

void DriverRunnerTestBase::Unbind() {
  driver_host_bindings_.CloseAll(ZX_OK);
  EXPECT_TRUE(RunLoopUntilIdle());
  for (auto& listener : stop_listeners_) {
    if (!listener.is_stopped()) {
      auto _ = listener.client()->Stop();
    }
  }

  RunLoopUntilIdle();
}

void DriverRunnerTestBase::ValidateProgram(std::optional<::fuchsia_data::Dictionary>& program,
                                           std::string_view binary, std::string_view colocate,
                                           std::string_view host_restart_on_crash,
                                           std::string_view use_next_vdso,
                                           std::string_view use_dynamic_linker,
                                           std::string_view compat) {
  ZX_ASSERT(program.has_value());
  auto& entries_opt = program.value().entries();
  ZX_ASSERT(entries_opt.has_value());
  auto& entries = entries_opt.value();
  size_t expected_num_entries = (compat == "") ? 5u : 6u;
  EXPECT_EQ(expected_num_entries, entries.size());
  EXPECT_EQ("binary", entries[0].key());
  EXPECT_EQ(std::string(binary), entries[0].value()->str().value());
  EXPECT_EQ("colocate", entries[1].key());
  EXPECT_EQ(std::string(colocate), entries[1].value()->str().value());
  EXPECT_EQ("host_restart_on_crash", entries[2].key());
  EXPECT_EQ(std::string(host_restart_on_crash), entries[2].value()->str().value());
  EXPECT_EQ("use_next_vdso", entries[3].key());
  EXPECT_EQ(std::string(use_next_vdso), entries[3].value()->str().value());
  EXPECT_EQ("use_dynamic_linker", entries[4].key());
  EXPECT_EQ(std::string(use_dynamic_linker), entries[4].value()->str().value());
  if (compat != "") {
    EXPECT_EQ("compat", entries[5].key());
    EXPECT_EQ(std::string(compat), entries[5].value()->str().value());
  }
}
void DriverRunnerTestBase::AssertNodeBound(const std::shared_ptr<CreatedChild>& child) {
  auto& node = child->node;
  ASSERT_TRUE(node.has_value() && node.value().is_valid());
}
void DriverRunnerTestBase::AssertNodeNotBound(const std::shared_ptr<CreatedChild>& child) {
  auto& node = child->node;
  ASSERT_FALSE(node.has_value() && node.value().is_valid());
}
void DriverRunnerTestBase::AssertNodeControllerBound(const std::shared_ptr<CreatedChild>& child) {
  auto& controller = child->node_controller;
  ASSERT_TRUE(controller.has_value() && controller.value().is_valid());
}
void DriverRunnerTestBase::AssertNodeControllerNotBound(
    const std::shared_ptr<CreatedChild>& child) {
  auto& controller = child->node_controller;
  ASSERT_FALSE(controller.has_value() && controller.value().is_valid());
}
inspect::Hierarchy DriverRunnerTestBase::Inspect() {
  FakeContext context;
  auto inspector = driver_runner().Inspect()(context).take_value();
  return inspect::ReadFromInspector(inspector)(context).take_value();
}
void DriverRunnerTestBase::SetupDevfs() {
  driver_runner().root_node()->SetupDevfsForRootNode(devfs_);
}
DriverRunnerTestBase::StartDriverResult DriverRunnerTestBase::StartSecondDriver(
    std::string_view moniker, bool colocate, bool host_restart_on_crash, bool use_next_vdso,
    bool use_dynamic_linker, fidl::ClientEnd<fuchsia_io::Directory> ns_svc) {
  auto second_driver_config = kDefaultSecondDriverPkgConfig;
  std::string binary = std::string(second_driver_config.main_module.open_path);
  StartDriverHandler start_handler = [colocate, host_restart_on_crash, use_next_vdso, binary,
                                      use_dynamic_linker](TestDriver* driver,
                                                          fdfw::DriverStartArgs start_args) {
    if (!colocate) {
      EXPECT_FALSE(start_args.symbols().has_value());
    }

    ValidateProgram(start_args.program(), binary, colocate ? "true" : "false",
                    host_restart_on_crash ? "true" : "false", use_next_vdso ? "true" : "false",
                    use_dynamic_linker ? "true" : "false");
  };
  return StartDriverWithConfig(moniker,
                               {
                                   .url = second_driver_url,
                                   .binary = binary,
                                   .colocate = colocate,
                                   .host_restart_on_crash = host_restart_on_crash,
                                   .use_next_vdso = use_next_vdso,
                                   .use_dynamic_linker = use_dynamic_linker,
                               },
                               std::move(start_handler), second_driver_config,
                               kDefaultDriverHostPkgConfig, std::move(ns_svc));
}

zx::event TestIntrospector::GetTokenForName(std::string_view name) {
  zx::event evnt;
  EXPECT_EQ(ZX_OK, zx::event::create(0, &evnt));
  zx_koid_t koid = GetKoid(evnt.borrow()).value();
  entries_[koid] = name;
  return evnt;
}

void TestIntrospector::GetMoniker(GetMonikerRequest& request,
                                  GetMonikerCompleter::Sync& completer) {
  if (!enabled_) {
    ZX_ASSERT_MSG(false, " GetMoniker shouldn't get called for this test.");
  }

  auto it = entries_.find(GetKoid(request.component_instance().borrow()).value());
  if (it == entries_.end()) {
    completer.Reply(fit::error(fcomponent::Error::kInstanceNotFound));
    return;
  }

  completer.Reply(fit::ok(it->second));
}

void TestDirectory::Bind(fidl::ServerEnd<fio::Directory> request) {
  bindings_.AddBinding(dispatcher_, std::move(request), this, fidl::kIgnoreBindingClosure);
}

void TestDirectory::Open(OpenRequest& request, OpenCompleter::Sync& completer) {
  if (open_handler_) {
    open_handler_(request.path(), fidl::ServerEnd<fio::Node>(std::move(request.object())));
  }
}

void TestDirectory::ReadDirents(ReadDirentsRequest& request,
                                ReadDirentsCompleter::Sync& completer) {
  if (read_dirents_called_ || dirents_.empty()) {
    completer.Reply(fio::DirectoryReadDirentsResponse(ZX_OK, std::vector<uint8_t>{}));
    return;
  }
  read_dirents_called_ = true;

  // Create directory entries according to the format in fuchsia.io/Directory.ReadDirents.
  std::vector<uint8_t> data;
  for (auto& name : dirents_) {
    size_t size = name.size();
    size_t start = data.size();
    data.resize(start + 10 + size);

    // ino = 0 (8 bytes)
    memset(data.data() + start, 0, 8);
    // size (1 byte)
    data[start + 8] = static_cast<uint8_t>(size);
    // type (1 byte, 0 = unknown)
    data[start + 9] = 0;
    // name
    memcpy(data.data() + start + 10, name.data(), size);
  }
  completer.Reply(fio::DirectoryReadDirentsResponse(ZX_OK, data));
}

void TestDirectory::Rewind(RewindCompleter::Sync& completer) {
  read_dirents_called_ = false;
  rewind_next_ = true;
  completer.Reply(fio::DirectoryRewindResponse(ZX_OK));
}

void TestDirectory::Clone(CloneRequest& request, CloneCompleter::Sync& completer) {
  bindings_.AddBinding(dispatcher_,
                       fidl::ServerEnd<fio::Directory>(request.request().TakeChannel()), this,
                       fidl::kIgnoreBindingClosure);
}

void TestDirectory::Watch(WatchRequest& request, WatchCompleter::Sync& completer) {
  std::vector<uint8_t> buffer;
  for (const auto& entry : dirents_) {
    buffer.push_back(static_cast<uint8_t>(fuchsia_io::wire::WatchEvent::kExisting));
    buffer.push_back(static_cast<uint8_t>(entry.size()));
    for (char c : entry) {
      buffer.push_back(static_cast<uint8_t>(c));
    }
  }
  buffer.push_back(static_cast<uint8_t>(fuchsia_io::wire::WatchEvent::kIdle));
  buffer.push_back(0);
  request.watcher().channel().write(0, buffer.data(), static_cast<uint32_t>(buffer.size()), nullptr,
                                    0);
  completer.Reply(ZX_OK);
}

void TestDirectory::handle_unknown_method(fidl::UnknownMethodMetadata<fio::Directory>,
                                          fidl::UnknownMethodCompleter::Sync&) {}
void TestDriver::Stop(StopCompleter::Sync& completer) {
  stop_handler_();
  if (!dont_close_binding_in_stop_) {
    driver_binding_.Close(ZX_OK);
  }
}

std::shared_ptr<CreatedChild> TestDriver::AddChild(std::string_view child_name, bool owned,
                                                   bool expect_error,
                                                   const std::string& class_name) {
  fidl::Arena arena;
  auto devfs = fuchsia_driver_framework::wire::DevfsAddArgs::Builder(arena)
                   .connector_supports(fuchsia_device_fs::ConnectionType::kController)
                   .class_name(class_name)
                   .Build();
  auto args = fuchsia_driver_framework::wire::NodeAddArgs::Builder(arena)
                  .name(arena, child_name)
                  .devfs_args(devfs)
                  .Build();
  return AddChild(fidl::ToNatural(args), owned, expect_error);
}

std::shared_ptr<CreatedChild> TestDriver::AddChild(fdfw::NodeAddArgs child_args, bool owned,
                                                   bool expect_error) {
  auto controller_endpoints = fidl::Endpoints<fdfw::NodeController>::Create();

  auto child_node_endpoints = fidl::CreateEndpoints<fdfw::Node>();
  ZX_ASSERT(ZX_OK == child_node_endpoints.status_value());

  fidl::ServerEnd<fdfw::Node> child_node_server = {};
  if (owned) {
    child_node_server = std::move(child_node_endpoints->server);
  }

  node_
      ->AddChild({std::move(child_args), std::move(controller_endpoints.server),
                  std::move(child_node_server)})
      .Then([expect_error](fidl::Result<fdfw::Node::AddChild> result) {
        if (expect_error) {
          EXPECT_TRUE(result.is_error());
        } else {
          EXPECT_TRUE(result.is_ok());
        }
      });

  class NodeEventHandler : public fidl::AsyncEventHandler<fdfw::Node> {
   public:
    explicit NodeEventHandler(std::shared_ptr<CreatedChild> child) : child_(std::move(child)) {}
    void on_fidl_error(::fidl::UnbindInfo error) override {
      child_->node.reset();
      delete this;
    }
    void handle_unknown_event(fidl::UnknownEventMetadata<fdfw::Node> metadata) override {}

   private:
    std::shared_ptr<CreatedChild> child_;
  };

  class ControllerEventHandler : public fidl::AsyncEventHandler<fdfw::NodeController> {
   public:
    explicit ControllerEventHandler(std::shared_ptr<CreatedChild> child)
        : child_(std::move(child)) {}
    void OnBind(fdfw::NodeControllerOnBindRequest& request) override {}
    void on_fidl_error(::fidl::UnbindInfo error) override {
      child_->node_controller.reset();
      delete this;
    }
    void handle_unknown_event(fidl::UnknownEventMetadata<fdfw::NodeController> metadata) override {}

   private:
    std::shared_ptr<CreatedChild> child_;
  };

  std::shared_ptr<CreatedChild> child = std::make_shared<CreatedChild>();
  child->node_controller.emplace(std::move(controller_endpoints.client), dispatcher_,
                                 new ControllerEventHandler(child));
  if (owned) {
    child->node.emplace(std::move(child_node_endpoints->client), dispatcher_,
                        new NodeEventHandler(child));
  }

  return child;
}

void TestRealm::SetHandles(std::vector<fprocess::HandleInfo> handles) {
  handles_ = std::move(handles);
}

fidl::VectorView<fprocess::wire::HandleInfo> TestRealm::TakeHandles(fidl::AnyArena& arena) {
  if (handles_.has_value()) {
    return fidl::ToWire(arena, std::move(handles_));
  }

  return fidl::VectorView<fprocess::wire::HandleInfo>(arena, 0);
}

fidl::WireClient<fuchsia_device::Controller> DriverRunnerTestBase::ConnectToDeviceController(
    std::string_view child_name) {
  fs::SynchronousVfs vfs(dispatcher());
  zx::result dev_res = devfs().Connect(vfs);
  EXPECT_EQ(dev_res.status_value(), ZX_OK);
  fidl::WireClient<fuchsia_io::Directory> dev{std::move(*dev_res), dispatcher()};
  auto [client, server] = fidl::Endpoints<fuchsia_device::Controller>::Create();
  auto device_controller_path = std::string(child_name) + "/device_controller";
  EXPECT_EQ(dev->Open(fidl::StringView::FromExternal(device_controller_path),
                      fio::wire::Flags::kProtocolService, {}, server.TakeChannel())
                .status(),
            ZX_OK);
  EXPECT_TRUE(RunLoopUntilIdle());
  return fidl::WireClient<fuchsia_device::Controller>{std::move(client), dispatcher()};
}

}  // namespace driver_runner
