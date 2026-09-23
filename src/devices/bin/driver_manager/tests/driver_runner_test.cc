// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/bin/driver_manager/driver_runner.h"

#include <fidl/fuchsia.component.decl/cpp/test_base.h>
#include <fidl/fuchsia.component/cpp/test_base.h>
#include <fidl/fuchsia.driver.framework/cpp/test_base.h>
#include <fidl/fuchsia.driver.host/cpp/test_base.h>
#include <fidl/fuchsia.io/cpp/test_base.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/cpp/task.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/fit/defer.h>
#include <lib/inspect/cpp/reader.h>
#include <lib/inspect/testing/cpp/inspect.h>
#include <lib/sync/cpp/completion.h>

#include <bind/fuchsia/platform/cpp/bind.h>

#include "gmock/gmock.h"
#include "gtest/gtest.h"
#include "src/devices/bin/driver_manager/composite/composite_node_spec.h"
#include "src/devices/bin/driver_manager/node.h"
#include "src/devices/bin/driver_manager/testing/fake_driver_index.h"
#include "src/devices/bin/driver_manager/tests/driver_runner_test_fixture.h"

namespace driver_runner {

namespace frunner = fuchsia_component_runner;

using driver_manager::Collection;
using driver_manager::Node;
using testing::ElementsAre;

// This is a parameterized variant of |DriverRunnerTestBase|. This enables each test case being run
// twice, once with the dynamic linker and once not (the legacy path).
class DriverRunnerTest : public DriverRunnerTestBase, public ::testing::WithParamInterface<bool> {
 public:
  void SetUp() override { use_dynamic_linker_ = GetParam(); }

  void SetupDriverRunner(FakeDriverIndex fake_driver_index) {
    if (use_dynamic_linker_) {
      auto driver_host_runner =
          std::make_unique<driver_manager::DriverHostRunner>(dispatcher(), ConnectToRealm());
      DriverRunnerTestBase::SetupDriverRunnerWithDynamicLinker(
          dispatcher(), std::move(driver_host_runner), std::move(fake_driver_index));
    } else {
      DriverRunnerTestBase::SetupDriverRunner(std::move(fake_driver_index));
    }
  }

  void SetupDriverRunner() { SetupDriverRunner(CreateDriverIndex()); }

  void SetupDriverRunnerWithPower(
      fidl::ClientEnd<fuchsia_power_broker::Topology> topology,
      std::optional<fidl::ClientEnd<fuchsia_power_system::CpuElementManager>> cpu_element_mgr =
          std::nullopt) {
    auto fake_driver_index = CreateDriverIndex();
    driver_index_.emplace(std::move(fake_driver_index));
    if (use_dynamic_linker_) {
      auto driver_host_runner =
          std::make_unique<driver_manager::DriverHostRunner>(dispatcher(), ConnectToRealm());
      auto load_driver_handler =
          [](zx::unowned_channel bootstrap_sender,
             driver_loader::Loader::DynamicLinkingPassiveAbi dl_passive_abi) mutable {};
      dynamic_linker_ = driver_loader::Loader::Create(dispatcher(), std::move(load_driver_handler));
      driver_runner_.emplace(
          ConnectToRealm(), ConnectToIntrospector(), ConnectToCapabilityStore(),
          driver_index_->Connect(), inspector_, &LoaderFactory, dispatcher(), false,
          driver_manager::OfferInjector{{
              .power_inject_offer = false,
              .power_suspend_enabled = true,
          }},
          std::move(topology),
          driver_manager::DriverRunner::DynamicLinkerArgs{
              [loader = dynamic_linker_.get()]() { return DynamicLinkerFactory(loader); },
              std::move(driver_host_runner)},
          std::move(cpu_element_mgr));
    } else {
      driver_runner_.emplace(ConnectToRealm(), ConnectToIntrospector(), ConnectToCapabilityStore(),
                             driver_index_->Connect(), inspector_, &LoaderFactory, dispatcher(),
                             false,
                             driver_manager::OfferInjector{{
                                 .power_inject_offer = false,
                                 .power_suspend_enabled = true,
                             }},
                             std::move(topology), std::nullopt, std::move(cpu_element_mgr));
    }
    SetupDevfs();
    driver_runner().power_manager()->FetchCpuToken();
  }

  zx::result<StartDriverResult> StartRootDriver() {
    if (use_dynamic_linker_) {
      return DriverRunnerTestBase::StartRootDriverDynamicLinking();
    } else {
      return DriverRunnerTestBase::StartRootDriver();
    }
  }

  StartDriverResult StartSecondDriver(
      std::string_view moniker, bool colocate = false, bool host_restart_on_crash = false,
      bool use_next_vdso = false,
      fidl::ClientEnd<fuchsia_io::Directory> ns_svc = fidl::ClientEnd<fuchsia_io::Directory>()) {
    return DriverRunnerTestBase::StartSecondDriver(moniker, colocate, host_restart_on_crash,
                                                   use_next_vdso, use_dynamic_linker(),
                                                   std::move(ns_svc));
  }

  // If |use_dynamic_linker| is not provided, it will be generated from the test configuration.
  void ValidateProgram(std::optional<::fuchsia_data::Dictionary>& program, std::string_view binary,
                       std::string_view colocate, std::string_view host_restart_on_crash,
                       std::string_view use_next_vdso,
                       std::optional<std::string_view> use_dynamic_linker = std::nullopt) {
    std::string use_dynamic_linker_str = use_dynamic_linker_ ? "true" : "false";
    if (use_dynamic_linker.has_value()) {
      use_dynamic_linker_str = use_dynamic_linker.value();
    }
    return DriverRunnerTestBase::ValidateProgram(program, binary, colocate, host_restart_on_crash,
                                                 use_next_vdso, use_dynamic_linker_str);
  }

  bool use_dynamic_linker() const { return use_dynamic_linker_; }

 private:
  bool use_dynamic_linker_ = false;
};

// Test RestartWithDictionaryAndPowerDependencies supplying power dependencies and cpu override
// token.
TEST_P(DriverRunnerTest, RestartWithPowerDependenciesAndCpuOverride) {
  auto cleanup = fit::defer([this]() { realm().ClearCreateChildHandlers(); });
  SetupDriverRunner();

  // Enable the introspector, required for binding.
  EnableIntrospector();

  auto root_driver = StartRootDriver();
  ASSERT_EQ(ZX_OK, root_driver.status_value());

  PrepareRealmForSecondDriverComponentStart();
  std::shared_ptr<CreatedChild> child = root_driver->driver->AddChild("second", false, false);
  EXPECT_TRUE(RunLoopUntilIdle());

  bool did_bind_1 = false;
  child->node_controller.value()->WaitForDriver().Then(
      [&did_bind_1](fidl::Result<fuchsia_driver_framework::NodeController::WaitForDriver>& result) {
        if (result.is_ok() && result.value().driver_started_node_token().has_value()) {
          did_bind_1 = true;
          return;
        }

        ZX_ASSERT_MSG(false, " WaitForDriver failed: %s.",
                      result.error_value().FormatDescription().c_str());
      });
  auto [driver, controller] = StartSecondDriver("dev.second");
  EXPECT_TRUE(did_bind_1);
  StopListener& stop_listener = ServeStopListener(std::move(controller));

  // Create a dictionary and power dependencies.
  zx::eventpair d_ep1, d_ep2;
  ASSERT_EQ(ZX_OK, zx::eventpair::create(0, &d_ep1, &d_ep2));
  fuchsia_component_sandbox::wire::DictionaryRef dict_ref;
  dict_ref.token = std::move(d_ep1);

  zx::event token;
  ASSERT_EQ(zx::event::create(0, &token), ZX_OK);
  std::vector<fuchsia_power_broker::LevelDependency> power_deps;
  power_deps.push_back(fuchsia_power_broker::LevelDependency{{
      .dependent_level = 1,
      .requires_token = std::move(token),
      .requires_level_by_preference = std::vector<uint8_t>{1},
  }});

  zx::eventpair release_fence1, release_fence2;
  ASSERT_EQ(ZX_OK, zx::eventpair::create(0, &release_fence1, &release_fence2));

  // The second driver should be restarted.
  realm().AddCreateChildHandler([](const fdecl::CollectionRef& collection, const fdecl::Child& decl,
                                   const std::vector<fdecl::Offer>& offers) {
    if (collection.name() == "boot-drivers" && decl.name().value() == "dev.second" &&
        decl.url().value() == second_driver_url) {
      return true;
    }
    return false;
  });

  zx::event cpu_override_token;
  ASSERT_EQ(zx::event::create(0, &cpu_override_token), ZX_OK);

  driver_runner().RestartWithDictionaryAndPowerDependencies(
      "dev.second", fidl::ToNatural(std::move(dict_ref)), std::move(power_deps),
      std::move(cpu_override_token), {}, std::move(release_fence2));

  // Our driver should get closed.
  while (!stop_listener.is_stopped()) {
    RunLoopUntilIdle();
  }

  // The node should bind again now that it is restarted.
  realm().AddCreateChildHandler([](const fdecl::CollectionRef& collection, const fdecl::Child& decl,
                                   const std::vector<fdecl::Offer>& offers) {
    if (collection.name() == "boot-drivers" && decl.name().value() == "dev.second" &&
        decl.url().value() == second_driver_url) {
      return true;
    }
    return false;
  });
  bool did_bind_2 = false;
  child->node_controller.value()->WaitForDriver().Then(
      [&did_bind_2](fidl::Result<fuchsia_driver_framework::NodeController::WaitForDriver>& result) {
        if (result.is_ok() && result.value().driver_started_node_token().has_value()) {
          did_bind_2 = true;
          return;
        }

        ZX_ASSERT_MSG(false, " WaitForDriver failed: %s.",
                      result.error_value().FormatDescription().c_str());
      });

  async::Loop background_loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  background_loop.StartThread("TestTopologyLoop");

  auto svc_endpoints = fidl::Endpoints<fuchsia_io::Directory>::Create();
  auto test_topology = std::make_unique<TestTopology>(background_loop.dispatcher());
  auto svc_dir = std::make_unique<TestDirectory>(background_loop.dispatcher());
  bool did_connect = false;
  svc_dir->SetDirents({"fuchsia.power.broker.Topology"});
  svc_dir->SetOpenHandler([&test_topology, &did_connect](const std::string& path,
                                                         fidl::ServerEnd<fuchsia_io::Node> object) {
    if (path == "fuchsia.power.broker.Topology") {
      test_topology->Bind(fidl::ServerEnd<fuchsia_power_broker::Topology>(object.TakeChannel()));
      did_connect = true;
    }
  });

  libsync::Completion bound_completion;
  async::PostTask(background_loop.dispatcher(), [&]() {
    svc_dir->Bind(std::move(svc_endpoints.server));
    bound_completion.Signal();
  });
  bound_completion.Wait();

  auto [driver_2, controller_2] =
      StartSecondDriver("dev.second", false, false, false, std::move(svc_endpoints.client));
  {
    auto deadline = zx::deadline_after(zx::sec(5));
    while (!did_bind_2 && zx::clock::get_monotonic() < deadline) {
      RunLoopUntilIdle();
      zx::nanosleep(zx::deadline_after(zx::msec(10)));
    }
  }
  EXPECT_TRUE(did_bind_2);
  EXPECT_TRUE(did_connect);
  ServeStopListener(std::move(controller_2));

  // Release the fence, causing the driver to restart again without dependencies.
  release_fence1.reset();
  release_fence2.reset();
  RunLoopUntilIdle();

  // The node should bind again after the revert.
  realm().AddCreateChildHandler([](const fdecl::CollectionRef& collection, const fdecl::Child& decl,
                                   const std::vector<fdecl::Offer>& offers) {
    if (collection.name() == "boot-drivers" && decl.name().value() == "dev.second" &&
        decl.url().value() == second_driver_url) {
      return true;
    }
    return false;
  });
  bool did_bind_3 = false;
  child->node_controller.value()->WaitForDriver().Then(
      [&did_bind_3](fidl::Result<fuchsia_driver_framework::NodeController::WaitForDriver>& result) {
        if (result.is_ok() && result.value().driver_started_node_token().has_value()) {
          did_bind_3 = true;
          return;
        }

        ZX_ASSERT_MSG(false, " WaitForDriver failed: %s.",
                      result.error_value().FormatDescription().c_str());
      });
  auto [driver_3, controller_3] = StartSecondDriver("dev.second");
  {
    auto deadline = zx::deadline_after(zx::sec(5));
    while (!did_bind_3 && zx::clock::get_monotonic() < deadline) {
      RunLoopUntilIdle();
      zx::nanosleep(zx::deadline_after(zx::msec(10)));
    }
  }
  EXPECT_TRUE(did_bind_3);
  ServeStopListener(std::move(controller_3));

  libsync::Completion destroy_completion;
  async::PostTask(background_loop.dispatcher(), [&]() {
    svc_dir.reset();
    test_topology.reset();
    destroy_completion.Signal();
  });
  destroy_completion.Wait();
  background_loop.Quit();
  background_loop.JoinThreads();

  StopDriverComponent(std::move(root_driver->controller));
}

// Test RestartWithDictionaryAndPowerDependencies with cpu_token_override, but empty power
// dependencies since this is allowed.
TEST_P(DriverRunnerTest, RestartWithCpuTokenOverrideOnly) {
  auto cleanup = fit::defer([this]() { realm().ClearCreateChildHandlers(); });
  SetupDriverRunner();

  // Enable the introspector, required for binding.
  EnableIntrospector();

  auto root_driver = StartRootDriver();
  ASSERT_EQ(ZX_OK, root_driver.status_value());

  PrepareRealmForSecondDriverComponentStart();
  std::shared_ptr<CreatedChild> child = root_driver->driver->AddChild("second", false, false);
  EXPECT_TRUE(RunLoopUntilIdle());

  bool did_bind_4 = false;
  child->node_controller.value()->WaitForDriver().Then(
      [&did_bind_4](fidl::Result<fuchsia_driver_framework::NodeController::WaitForDriver>& result) {
        if (result.is_ok() && result.value().driver_started_node_token().has_value()) {
          did_bind_4 = true;
          return;
        }

        ZX_ASSERT_MSG(false, " WaitForDriver failed: %s.",
                      result.error_value().FormatDescription().c_str());
      });
  auto [driver, controller] = StartSecondDriver("dev.second");
  EXPECT_TRUE(did_bind_4);
  StopListener& stop_listener = ServeStopListener(std::move(controller));

  // Create a dictionary and power dependencies.
  zx::eventpair d_ep1, d_ep2;
  ASSERT_EQ(ZX_OK, zx::eventpair::create(0, &d_ep1, &d_ep2));
  fuchsia_component_sandbox::wire::DictionaryRef dict_ref;
  dict_ref.token = std::move(d_ep1);

  zx::event cpu_override_token;
  ASSERT_EQ(zx::event::create(0, &cpu_override_token), ZX_OK);

  zx::eventpair release_fence1, release_fence2;
  ASSERT_EQ(ZX_OK, zx::eventpair::create(0, &release_fence1, &release_fence2));

  // The second driver should be restarted.
  realm().AddCreateChildHandler([](const fdecl::CollectionRef& collection, const fdecl::Child& decl,
                                   const std::vector<fdecl::Offer>& offers) {
    if (collection.name() == "boot-drivers" && decl.name().value() == "dev.second" &&
        decl.url().value() == second_driver_url) {
      return true;
    }
    return false;
  });

  driver_runner().RestartWithDictionaryAndPowerDependencies(
      "dev.second", fidl::ToNatural(std::move(dict_ref)), {}, std::move(cpu_override_token), {},
      std::move(release_fence2));

  // Our driver should get closed.
  while (!stop_listener.is_stopped()) {
    RunLoopUntilIdle();
  }

  // The node should bind again now that it is restarted.
  realm().AddCreateChildHandler([](const fdecl::CollectionRef& collection, const fdecl::Child& decl,
                                   const std::vector<fdecl::Offer>& offers) {
    if (collection.name() == "boot-drivers" && decl.name().value() == "dev.second" &&
        decl.url().value() == second_driver_url) {
      return true;
    }
    return false;
  });
  bool did_bind_5 = false;
  child->node_controller.value()->WaitForDriver().Then(
      [&did_bind_5](fidl::Result<fuchsia_driver_framework::NodeController::WaitForDriver>& result) {
        if (result.is_ok() && result.value().driver_started_node_token().has_value()) {
          did_bind_5 = true;
          return;
        }

        ZX_ASSERT_MSG(false, " WaitForDriver failed: %s.",
                      result.error_value().FormatDescription().c_str());
      });

  async::Loop background_loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  background_loop.StartThread("TestTopologyLoop");

  auto svc_endpoints = fidl::Endpoints<fuchsia_io::Directory>::Create();
  auto test_topology = std::make_unique<TestTopology>(background_loop.dispatcher());
  auto svc_dir = std::make_unique<TestDirectory>(background_loop.dispatcher());
  bool did_connect = false;
  svc_dir->SetDirents({"fuchsia.power.broker.Topology"});
  svc_dir->SetOpenHandler([&test_topology, &did_connect](const std::string& path,
                                                         fidl::ServerEnd<fuchsia_io::Node> object) {
    if (path == "fuchsia.power.broker.Topology") {
      test_topology->Bind(fidl::ServerEnd<fuchsia_power_broker::Topology>(object.TakeChannel()));
      did_connect = true;
    }
  });

  libsync::Completion bound_completion;
  async::PostTask(background_loop.dispatcher(), [&]() {
    svc_dir->Bind(std::move(svc_endpoints.server));
    bound_completion.Signal();
  });
  bound_completion.Wait();

  auto [driver_2, controller_2] =
      StartSecondDriver("dev.second", false, false, false, std::move(svc_endpoints.client));
  {
    auto deadline = zx::deadline_after(zx::sec(5));
    while (!did_bind_5 && zx::clock::get_monotonic() < deadline) {
      RunLoopUntilIdle();
      zx::nanosleep(zx::deadline_after(zx::msec(10)));
    }
  }
  EXPECT_TRUE(did_bind_5);
  EXPECT_TRUE(did_connect);
  ServeStopListener(std::move(controller_2));

  // Release the fence, causing the driver to restart again and restore the CPU token.
  release_fence1.reset();
  release_fence2.reset();
  RunLoopUntilIdle();

  // The node should bind again after the revert.
  realm().AddCreateChildHandler([](const fdecl::CollectionRef& collection, const fdecl::Child& decl,
                                   const std::vector<fdecl::Offer>& offers) {
    if (collection.name() == "boot-drivers" && decl.name().value() == "dev.second" &&
        decl.url().value() == second_driver_url) {
      return true;
    }
    return false;
  });
  bool did_bind_6 = false;
  child->node_controller.value()->WaitForDriver().Then(
      [&did_bind_6](fidl::Result<fuchsia_driver_framework::NodeController::WaitForDriver>& result) {
        if (result.is_ok() && result.value().driver_started_node_token().has_value()) {
          did_bind_6 = true;
          return;
        }

        ZX_ASSERT_MSG(false, " WaitForDriver failed: %s.",
                      result.error_value().FormatDescription().c_str());
      });
  auto [driver_3, controller_3] = StartSecondDriver("dev.second");
  {
    auto deadline = zx::deadline_after(zx::sec(5));
    while (!did_bind_6 && zx::clock::get_monotonic() < deadline) {
      RunLoopUntilIdle();
      zx::nanosleep(zx::deadline_after(zx::msec(10)));
    }
  }
  EXPECT_TRUE(did_bind_6);
  ServeStopListener(std::move(controller_3));

  libsync::Completion destroy_completion;
  async::PostTask(background_loop.dispatcher(), [&]() {
    svc_dir.reset();
    test_topology.reset();
    destroy_completion.Signal();
  });
  destroy_completion.Wait();
  background_loop.Quit();
  background_loop.JoinThreads();

  StopDriverComponent(std::move(root_driver->controller));
}

// Verifies that RestartWithDictionaryAndPowerDependencies accepts node_power_token_overrides
// for child nodes, and that upon driver restart, the target child node's power element token
// matches the provided override token KOID. Also verifies that reverting the release fence resets
// the power token overrides.
TEST_P(DriverRunnerTest, RestartWithNodePowerTokenOverrides) {
  auto cleanup = fit::defer([this]() { realm().ClearCreateChildHandlers(); });
  SetupDriverRunner();

  EnableIntrospector();

  auto root_driver = StartRootDriver();
  ASSERT_EQ(ZX_OK, root_driver.status_value());

  PrepareRealmForSecondDriverComponentStart();
  std::shared_ptr<CreatedChild> child = root_driver->driver->AddChild("second", false, false);
  EXPECT_TRUE(RunLoopUntilIdle());

  bool did_bind_1 = false;
  child->node_controller.value()->WaitForDriver().Then(
      [&did_bind_1](fidl::Result<fuchsia_driver_framework::NodeController::WaitForDriver>& result) {
        if (result.is_ok() && result.value().driver_started_node_token().has_value()) {
          did_bind_1 = true;
          return;
        }
        ZX_ASSERT_MSG(!result.is_ok(), " WaitForDriver succeeded, but no node token provided.");
        ZX_ASSERT_MSG(false, " WaitForDriver failed: %s.",
                      result.error_value().FormatDescription().c_str());
      });
  auto [driver, controller] = StartSecondDriver("dev.second");
  EXPECT_TRUE(did_bind_1);
  StopListener& stop_listener = ServeStopListener(std::move(controller));

  zx::eventpair d_ep1, d_ep2;
  ASSERT_EQ(ZX_OK, zx::eventpair::create(0, &d_ep1, &d_ep2));
  fuchsia_component_sandbox::wire::DictionaryRef dict_ref;
  dict_ref.token = std::move(d_ep1);

  zx::event override_token;
  ASSERT_EQ(zx::event::create(0, &override_token), ZX_OK);
  zx::event override_token_copy;
  ASSERT_EQ(override_token.duplicate(ZX_RIGHT_SAME_RIGHTS, &override_token_copy), ZX_OK);

  zx_info_handle_basic_t info;
  ASSERT_EQ(override_token.get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr),
            ZX_OK);
  zx_koid_t expected_koid = info.koid;

  std::vector<fuchsia_driver_development::NodePowerTokenOverride> overrides;
  overrides.push_back(fuchsia_driver_development::NodePowerTokenOverride{{
      .target_node = "second",
      .token = std::move(override_token),
  }});

  zx::eventpair release_fence1, release_fence2;
  ASSERT_EQ(ZX_OK, zx::eventpair::create(0, &release_fence1, &release_fence2));

  realm().AddCreateChildHandler([](const fdecl::CollectionRef& collection, const fdecl::Child& decl,
                                   const std::vector<fdecl::Offer>& offers) {
    if (collection.name() == "boot-drivers" && decl.name().value() == "dev.second" &&
        decl.url().value() == second_driver_url) {
      return true;
    }
    return false;
  });

  driver_runner().RestartWithDictionaryAndPowerDependencies(
      "dev.second", fidl::ToNatural(std::move(dict_ref)), {}, std::nullopt, std::move(overrides),
      std::move(release_fence2));

  while (!stop_listener.is_stopped()) {
    RunLoopUntilIdle();
  }

  bool did_bind_2 = false;
  child->node_controller.value()->WaitForDriver().Then(
      [&did_bind_2](fidl::Result<fuchsia_driver_framework::NodeController::WaitForDriver>& result) {
        if (result.is_ok() && result.value().driver_started_node_token().has_value()) {
          did_bind_2 = true;
          return;
        }

        ZX_ASSERT_MSG(!result.is_ok(), " WaitForDriver succeeded, but no node token provided.");
        ZX_ASSERT_MSG(false, " WaitForDriver failed: %s.",
                      result.error_value().FormatDescription().c_str());
      });
  auto [driver_2, controller_2] = StartSecondDriver("dev.second");
  EXPECT_TRUE(did_bind_2);

  std::shared_ptr<driver_manager::Node> node_ptr = nullptr;
  for (const auto& c : driver_runner().root_node()->children()) {
    if (c->name() == "second") {
      node_ptr = c;
      break;
    }
  }
  ASSERT_NE(node_ptr, nullptr);
  zx::event power_token = node_ptr->DuplicatePowerToken();
  ASSERT_TRUE(power_token.is_valid());
  ASSERT_EQ(power_token.get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr),
            ZX_OK);
  EXPECT_EQ(info.koid, expected_koid);

  ServeStopListener(std::move(controller_2));
  release_fence1.reset();
  RunLoopUntilIdle();

  StopDriverComponent(std::move(root_driver->controller));
}

// Start the root driver.
TEST_P(DriverRunnerTest, StartRootDriver) {
  SetupDriverRunner();

  auto root_driver = StartRootDriver();
  ASSERT_EQ(ZX_OK, root_driver.status_value());

  StopDriverComponent(std::move(root_driver->controller));
  realm().AssertDestroyedChildren({CreateChildRef("root", "boot-drivers")});
}

// Start the root driver. Make sure that the driver is stopped before the Component is exited.
TEST_P(DriverRunnerTest, StartRootDriver_DriverStopBeforeComponentExit) {
  SetupDriverRunner();

  std::vector<size_t> event_order;

  auto root_driver = StartRootDriver();
  ASSERT_EQ(ZX_OK, root_driver.status_value());
  ServeStopListener(std::move(root_driver->controller), TeardownWatcher(1, event_order));

  root_driver->driver->SetStopHandler([&event_order]() { event_order.push_back(0); });
  root_driver->driver->DropNode();
  EXPECT_TRUE(RunLoopUntilIdle());
  // Make sure the driver was stopped before we told the component framework the driver was stopped.
  EXPECT_THAT(event_order, ElementsAre(0, 1));
}

// Start the root driver, and add a child node owned by the root driver.
TEST_P(DriverRunnerTest, StartRootDriver_AddOwnedChild) {
  SetupDriverRunner();

  auto root_driver = StartRootDriver();
  ASSERT_EQ(ZX_OK, root_driver.status_value());

  root_driver->driver->AddChild("second", true, false);
  EXPECT_TRUE(RunLoopUntilIdle());

  StopDriverComponent(std::move(root_driver->controller));
  realm().AssertDestroyedChildren({CreateChildRef("root", "boot-drivers")});
}

// Start the root driver, add a child node, then remove it.
TEST_P(DriverRunnerTest, StartRootDriver_RemoveOwnedChild) {
  SetupDriverRunner();

  auto root_driver = StartRootDriver();
  ASSERT_EQ(ZX_OK, root_driver.status_value());

  std::shared_ptr<CreatedChild> created_child =
      root_driver->driver->AddChild("second", true, false);
  EXPECT_TRUE(RunLoopUntilIdle());

  AssertNodeBound(created_child);
  AssertNodeControllerBound(created_child);

  EXPECT_TRUE(created_child->node_controller.value()->Remove().is_ok());
  EXPECT_TRUE(RunLoopUntilIdle());

  AssertNodeNotBound(created_child);
  ASSERT_NE(nullptr, root_driver->driver.get());
  EXPECT_TRUE(root_driver->driver->node().is_valid());

  StopDriverComponent(std::move(root_driver->controller));
  realm().AssertDestroyedChildren({CreateChildRef("root", "boot-drivers")});
}

// Start the root driver, and add two child nodes with duplicate names.
TEST_P(DriverRunnerTest, StartRootDriver_AddOwnedChild_DuplicateNames) {
  SetupDriverRunner();
  auto root_driver = StartRootDriver();
  ASSERT_EQ(ZX_OK, root_driver.status_value());

  std::shared_ptr<CreatedChild> child = root_driver->driver->AddChild("second", true, false);
  std::shared_ptr<CreatedChild> invalid_child = root_driver->driver->AddChild("second", true, true);
  EXPECT_TRUE(RunLoopUntilIdle());

  AssertNodeNotBound(invalid_child);
  AssertNodeBound(child);

  ASSERT_NE(nullptr, root_driver->driver.get());
  EXPECT_TRUE(root_driver->driver->node().is_valid());

  StopDriverComponent(std::move(root_driver->controller));
  realm().AssertDestroyedChildren({CreateChildRef("root", "boot-drivers")});
}

// Start the root driver, and add a child node with an offer that is missing a
// source.
TEST_P(DriverRunnerTest, StartRootDriver_AddUnownedChild_OfferMissingSource) {
  SetupDriverRunner();

  auto root_driver = StartRootDriver();
  ASSERT_EQ(ZX_OK, root_driver.status_value());

  fdfw::NodeAddArgs args({
      .name = "second",
      .offers2 =
          {
              {
                  fuchsia_driver_framework::Offer::WithZirconTransport(
                      fuchsia_component_decl::Offer::WithService(fdecl::OfferService({
                          .target_name = std::make_optional<std::string>("fuchsia.package.Renamed"),
                      }))),
              },
          },
  });
  std::shared_ptr<CreatedChild> child = root_driver->driver->AddChild(std::move(args), false, true);
  EXPECT_TRUE(RunLoopUntilIdle());
  AssertNodeControllerNotBound(child);

  StopDriverComponent(std::move(root_driver->controller));
  realm().AssertDestroyedChildren({CreateChildRef("root", "boot-drivers")});
}

// Start the root driver, and add a child node with one offer that has a source
// and another that has a target.
TEST_P(DriverRunnerTest, StartRootDriver_AddUnownedChild_OfferHasRef) {
  SetupDriverRunner();

  auto root_driver = StartRootDriver();
  ASSERT_EQ(ZX_OK, root_driver.status_value());

  fdfw::NodeAddArgs args({
      .name = "second",
      .offers2 =
          {
              {
                  fuchsia_driver_framework::Offer::WithZirconTransport(
                      fuchsia_component_decl::Offer::WithService(fdecl::OfferService({
                          .source = fdecl::Ref::WithSelf(fdecl::SelfRef()),
                          .source_name = "fuchsia.package.Protocol",
                      }))),
                  fuchsia_driver_framework::Offer::WithZirconTransport(
                      fuchsia_component_decl::Offer::WithService(fdecl::OfferService({
                          .source_name = "fuchsia.package.Protocol",
                          .target = fdecl::Ref::WithSelf(fdecl::SelfRef()),
                      }))),
              },
          },
  });
  std::shared_ptr<CreatedChild> child = root_driver->driver->AddChild(std::move(args), false, true);
  EXPECT_TRUE(RunLoopUntilIdle());
  AssertNodeControllerNotBound(child);

  StopDriverComponent(std::move(root_driver->controller));
  realm().AssertDestroyedChildren({CreateChildRef("root", "boot-drivers")});
}

// Start the root driver, and add a child node with duplicate symbols. The child
// node is unowned, so if we did not have duplicate symbols, the second driver
// would bind to it.
TEST_P(DriverRunnerTest, StartRootDriver_AddUnownedChild_DuplicateSymbols) {
  SetupDriverRunner();

  auto root_driver = StartRootDriver();
  ASSERT_EQ(ZX_OK, root_driver.status_value());

  fdfw::NodeAddArgs args({
      .name = "second",
      .symbols =
          {
              {
                  fdfw::NodeSymbol({
                      .name = "sym",
                      .address = 0xf00d,
                  }),
                  fdfw::NodeSymbol({
                      .name = "sym",
                      .address = 0xf00d,
                  }),
              },
          },
  });
  std::shared_ptr<CreatedChild> child = root_driver->driver->AddChild(std::move(args), false, true);
  EXPECT_TRUE(RunLoopUntilIdle());
  AssertNodeControllerNotBound(child);

  StopDriverComponent(std::move(root_driver->controller));
  realm().AssertDestroyedChildren({CreateChildRef("root", "boot-drivers")});
}

// Start the root driver, and add a child node that has a symbol without an
// address.
TEST_P(DriverRunnerTest, StartRootDriver_AddUnownedChild_SymbolMissingAddress) {
  SetupDriverRunner();

  auto root_driver = StartRootDriver();
  ASSERT_EQ(ZX_OK, root_driver.status_value());

  fdfw::NodeAddArgs args({
      .name = "second",
      .symbols =
          {
              {
                  fdfw::NodeSymbol({
                      .name = std::make_optional<std::string>("sym"),
                  }),
              },
          },
  });
  std::shared_ptr<CreatedChild> child = root_driver->driver->AddChild(std::move(args), false, true);
  EXPECT_TRUE(RunLoopUntilIdle());
  AssertNodeControllerNotBound(child);

  StopDriverComponent(std::move(root_driver->controller));
  realm().AssertDestroyedChildren({CreateChildRef("root", "boot-drivers")});
}

// Start the root driver, and add a child node that has a symbol without a name.
TEST_P(DriverRunnerTest, StartRootDriver_AddUnownedChild_SymbolMissingName) {
  SetupDriverRunner();

  auto root_driver = StartRootDriver();
  ASSERT_EQ(ZX_OK, root_driver.status_value());

  fdfw::NodeAddArgs args({
      .name = "second",
      .symbols =
          {
              {
                  fdfw::NodeSymbol({
                      .name = std::nullopt,
                      .address = 0xfeed,
                  }),
              },
          },
  });
  std::shared_ptr<CreatedChild> child = root_driver->driver->AddChild(std::move(args), false, true);
  EXPECT_TRUE(RunLoopUntilIdle());
  AssertNodeControllerNotBound(child);

  StopDriverComponent(std::move(root_driver->controller));
  realm().AssertDestroyedChildren({CreateChildRef("root", "boot-drivers")});
}

// Start the root driver, and then start a second driver in a new driver host.
TEST_P(DriverRunnerTest, StartSecondDriver_NewDriverHost) {
  SetupDriverRunner();

  auto root_driver = StartRootDriver();
  ASSERT_EQ(ZX_OK, root_driver.status_value());

  realm().SetCreateChildHandler(
      [](fdecl::CollectionRef collection, fdecl::Child decl, std::vector<fdecl::Offer> offers) {
        EXPECT_EQ("boot-drivers", collection.name());
        EXPECT_EQ("dev.second", decl.name());
        EXPECT_EQ(second_driver_url, decl.url());

        EXPECT_EQ(1u, offers.size());
        ASSERT_TRUE(offers[0].Which() == fdecl::Offer::Tag::kService);
        auto& service = offers[0].service().value();

        ASSERT_TRUE(service.source().has_value());
        ASSERT_TRUE(service.source().value().Which() == fdecl::Ref::Tag::kChild);
        auto& source_ref = service.source().value().child().value();
        EXPECT_EQ("root", source_ref.name());
        EXPECT_EQ("boot-drivers", source_ref.collection().value_or("missing"));

        ASSERT_TRUE(service.source_name().has_value());
        EXPECT_EQ("fuchsia.package.Protocol", service.source_name().value());
      });

  fdfw::NodeAddArgs args({
      .name = "second",
      .symbols =
          {
              {
                  fdfw::NodeSymbol({
                      .name = "sym",
                      .address = 0xfeed,
                  }),
              },
          },
      .offers2 =
          {
              {
                  fuchsia_driver_framework::Offer::WithZirconTransport(
                      fuchsia_component_decl::Offer::WithService(fdecl::OfferService({
                          .source_name = "fuchsia.package.Protocol",
                          .source_instance_filter = std::vector<std::string>{"default"},
                          .renamed_instances =
                              std::vector<fdecl::NameMapping>{
                                  fdecl::NameMapping("default", "instance-1"),
                              },
                      }))),
              },
          },
  });

  std::shared_ptr<CreatedChild> child =
      root_driver->driver->AddChild(std::move(args), false, false);
  EXPECT_TRUE(RunLoopUntilIdle());

  bool did_bind = false;
  child->node_controller.value()->WaitForDriver().Then(
      [&did_bind](fidl::Result<fuchsia_driver_framework::NodeController::WaitForDriver>& result) {
        if (result.is_ok() && result.value().driver_started_node_token().has_value()) {
          did_bind = true;
          return;
        }

        ZX_ASSERT_MSG(false, " WaitForDriver failed: %s.",
                      result.error_value().FormatDescription().c_str());
      });

  auto [driver, controller] = StartSecondDriver("root.second");
  EXPECT_TRUE(did_bind);
  ServeStopListener(std::move(controller));

  driver->CloseBinding();
  driver->DropNode();
  StopDriverComponent(std::move(root_driver->controller));
  realm().AssertDestroyedChildren(
      {CreateChildRef("root", "boot-drivers"), CreateChildRef("dev.second", "boot-drivers")});
}

// Start the root driver, and then start a second driver in the same driver
// host.
TEST_P(DriverRunnerTest, StartSecondDriver_SameDriverHost) {
  SetupDriverRunner();

  auto root_driver = StartRootDriver();
  ASSERT_EQ(ZX_OK, root_driver.status_value());

  PrepareRealmForSecondDriverComponentStart();
  fdfw::NodeAddArgs args({
      .name = "second",
      .symbols =
          {
              {
                  fdfw::NodeSymbol({
                      .name = "sym",
                      .address = 0xfeed,
                  }),
              },
          },
      .offers2 =
          {
              {
                  fuchsia_driver_framework::Offer::WithZirconTransport(
                      fuchsia_component_decl::Offer::WithService(fdecl::OfferService({
                          .source_name = "fuchsia.package.Protocol",
                          .source_instance_filter = std::vector<std::string>{"default"},
                          .renamed_instances =
                              std::vector<fdecl::NameMapping>{
                                  fdecl::NameMapping("default", "instance-1"),
                              },
                      }))),
              },
          },
  });

  std::shared_ptr<CreatedChild> child =
      root_driver->driver->AddChild(std::move(args), false, false);
  EXPECT_TRUE(RunLoopUntilIdle());

  bool did_bind = false;
  child->node_controller.value()->WaitForDriver().Then(
      [&did_bind](fidl::Result<fuchsia_driver_framework::NodeController::WaitForDriver>& result) {
        if (result.is_ok() && result.value().driver_started_node_token().has_value()) {
          did_bind = true;
          return;
        }

        ZX_ASSERT_MSG(false, " WaitForDriver failed: %s.",
                      result.error_value().FormatDescription().c_str());
      });

  auto second_driver_config = kDefaultSecondDriverPkgConfig;
  std::string binary = std::string(second_driver_config.main_module.open_path);
  StartDriverHandler start_handler = [this, binary](TestDriver* driver,
                                                    fdfw::DriverStartArgs start_args) {
    auto& symbols = start_args.symbols().value();
    EXPECT_EQ(1u, symbols.size());
    EXPECT_EQ("sym", symbols[0].name().value());
    EXPECT_EQ(0xfeedu, symbols[0].address());
    ValidateProgram(start_args.program(), binary, "true", "false", "false");
  };
  auto [driver, controller] = StartDriverWithConfig("dev.second",
                                                    {
                                                        .url = second_driver_url,
                                                        .binary = binary,
                                                        .colocate = true,
                                                        .use_dynamic_linker = use_dynamic_linker(),
                                                    },
                                                    std::move(start_handler), second_driver_config);
  ServeStopListener(std::move(controller));

  driver->CloseBinding();
  driver->DropNode();
  StopDriverComponent(std::move(root_driver->controller));
  EXPECT_TRUE(did_bind);
  realm().AssertDestroyedChildren(
      {CreateChildRef("root", "boot-drivers"), CreateChildRef("dev.second", "boot-drivers")});
}

// Start the root driver, and then start a second driver that we match based on
// node properties.
TEST_P(DriverRunnerTest, StartSecondDriver_UseProperties) {
  FakeDriverIndex driver_index(
      dispatcher(), [](auto args) -> zx::result<FakeDriverIndex::MatchResult> {
        if (args.has_properties() && args.properties()[0].key.get() == "second_node_prop" &&
            args.properties()[0].value.is_int_value() &&
            args.properties()[0].value.int_value() == 0x2301) {
          return zx::ok(FakeDriverIndex::MatchResult{
              .url = second_driver_url,
          });
        } else {
          return zx::error(ZX_ERR_NOT_FOUND);
        }
      });
  SetupDriverRunner(std::move(driver_index));

  auto root_driver = StartRootDriver();
  ASSERT_EQ(ZX_OK, root_driver.status_value());

  PrepareRealmForSecondDriverComponentStart();
  fdfw::NodeAddArgs args({
      .name = "second",
      .properties2 =
          {
              {
                  fdf::MakeProperty2("second_node_prop", 0x2301u),
              },
          },
  });

  std::shared_ptr<CreatedChild> child =
      root_driver->driver->AddChild(std::move(args), false, false);
  EXPECT_TRUE(RunLoopUntilIdle());

  auto [driver, controller] = StartSecondDriver("dev.second", true);
  ServeStopListener(std::move(controller));

  driver->CloseBinding();
  driver->DropNode();
  StopDriverComponent(std::move(root_driver->controller));
  realm().AssertDestroyedChildren(
      {CreateChildRef("root", "boot-drivers"), CreateChildRef("dev.second", "boot-drivers")});
}

TEST_P(DriverRunnerTest, CheckOnBindNode) {
  SetupDriverRunner();

  auto root_driver = StartRootDriver();
  ASSERT_EQ(ZX_OK, root_driver.status_value());

  PrepareRealmForSecondDriverComponentStart();
  fdfw::NodeAddArgs args({
      .name = "second",
      .properties2 =
          {
              {
                  fdf::MakeProperty2("second_node_prop", 0x2301u),
              },
          },
  });

  std::shared_ptr<CreatedChild> child =
      root_driver->driver->AddChild(std::move(args), false, false);
  EXPECT_TRUE(RunLoopUntilIdle());

  std::optional<zx::event> node_token;
  child->node_controller.value()->WaitForDriver().Then(
      [&node_token](fidl::Result<fuchsia_driver_framework::NodeController::WaitForDriver>& result) {
        if (result.is_ok() && result.value().driver_started_node_token().has_value()) {
          node_token = std::move(result.value().driver_started_node_token().value());
          return;
        }

        ZX_ASSERT_MSG(false, " WaitForDriver failed: %s.",
                      result.error_value().FormatDescription().c_str());
      });

  auto [driver, controller] = StartSecondDriver("dev.second", true);
  ServeStopListener(std::move(controller));

  ASSERT_TRUE(node_token.has_value());
  zx_info_handle_basic_t info;
  ASSERT_EQ(node_token->get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr),
            ZX_OK);

  ASSERT_TRUE(driver->node_token().has_value());
  zx_info_handle_basic_t info2;
  ASSERT_EQ(
      driver->node_token()->get_info(ZX_INFO_HANDLE_BASIC, &info2, sizeof(info2), nullptr, nullptr),
      ZX_OK);

  ASSERT_EQ(info.koid, info2.koid);

  driver->CloseBinding();
  driver->DropNode();
  StopDriverComponent(std::move(root_driver->controller));
  realm().AssertDestroyedChildren(
      {CreateChildRef("root", "boot-drivers"), CreateChildRef("dev.second", "boot-drivers")});
}

// Disable the second driver and try to create a Node that would bind to it. The node should fail
// to match a driver.
TEST_P(DriverRunnerTest, SecondNodeWithDisabledSecondDriver) {
  SetupDriverRunner();

  auto root_driver = StartRootDriver();
  ASSERT_EQ(ZX_OK, root_driver.status_value());

  // Disable the second-driver url, and restart with rematching of the requested.
  driver_index().disable_driver_url(second_driver_url);

  fidl::Arena arena;
  auto devfs = fuchsia_driver_framework::wire::DevfsAddArgs::Builder(arena)
                   .connector_supports(fuchsia_device_fs::ConnectionType::kController)
                   .class_name("driver_runner_test")
                   .Build();
  auto args = fuchsia_driver_framework::wire::NodeAddArgs::Builder(arena)
                  .name(arena, "second")
                  .devfs_args(devfs)
                  .Build();
  auto child = root_driver->driver->AddChild(fidl::ToNatural(args), false, false);
  EXPECT_TRUE(RunLoopUntilIdle());

  EXPECT_EQ(1u, driver_runner().bind_manager().NumOrphanedResources());

  bool bind_failed = false;
  child->node_controller.value()->WaitForDriver().Then(
      [&bind_failed](
          fidl::Result<fuchsia_driver_framework::NodeController::WaitForDriver>& result) {
        if (result.is_ok() && result.value().match_error().has_value() &&
            result.value().match_error().value() == ZX_ERR_NOT_FOUND) {
          bind_failed = true;
          return;
        }

        ZX_ASSERT_MSG(false, " WaitForDriver did not get expected error");
      });

  // Mark the boot-up as complete so the error gets emitted out.
  driver_runner().BootupDoneForTesting();
  RunLoopUntilIdle();
  EXPECT_TRUE(bind_failed);

  StopDriverComponent(std::move(root_driver->controller));
  // Only the root node was destroyed since the second node never created a component.
  realm().AssertDestroyedChildren({CreateChildRef("root", "boot-drivers")});
}

// Start the second driver, and then disable and rematch it which should make it available for
// matching. Undisable the driver and then restart with rematch, which should get the node again.
TEST_P(DriverRunnerTest, StartSecondDriver_DisableAndRematch_UndisableAndRestart) {
  SetupDriverRunner();

  auto root_driver = StartRootDriver();
  ASSERT_EQ(ZX_OK, root_driver.status_value());

  PrepareRealmForSecondDriverComponentStart();
  std::shared_ptr<CreatedChild> child = root_driver->driver->AddChild("second", false, false);
  EXPECT_TRUE(RunLoopUntilIdle());

  auto [driver, controller] = StartSecondDriver("dev.second");
  StopListener& stop_listener = ServeStopListener(std::move(controller));

  EXPECT_EQ(0u, driver_runner().bind_manager().NumOrphanedResources());

  // Disable the second-driver url, and restart with rematching of the requested.
  driver_index().disable_driver_url(second_driver_url);
  zx::result count = driver_runner().RestartNodesColocatedWithDriverUrl(
      second_driver_url, fuchsia_driver_development::RestartRematchFlags::kRequested);
  EXPECT_EQ(1u, count.value());

  // Our driver should get closed.
  while (!stop_listener.is_stopped()) {
    RunLoopUntilIdle();
  }

  // Since we disabled the driver url, the rematch should have not gotten a match, and therefore
  // the node should haver become orphaned.
  EXPECT_EQ(1u, driver_runner().bind_manager().NumOrphanedResources());

  // Undisable the driver, and try binding all available nodes. This should cause it to get
  // started again.
  driver_index().un_disable_driver_url(second_driver_url);

  PrepareRealmForSecondDriverComponentStart();
  driver_runner().TryBindAllAvailable();
  EXPECT_TRUE(RunLoopUntilIdle());

  auto [driver_2, controller_2] = StartSecondDriver("dev.second");
  ServeStopListener(std::move(controller_2));

  // This list should be empty now that it got bound again.
  EXPECT_EQ(0u, driver_runner().bind_manager().NumOrphanedResources());

  StopDriverComponent(std::move(root_driver->controller));
  // The node was destroyed twice.
  realm().AssertDestroyedChildren({CreateChildRef("root", "boot-drivers"),
                                   CreateChildRef("dev.second", "boot-drivers"),
                                   CreateChildRef("dev.second", "boot-drivers")});
}

TEST_P(DriverRunnerTest, RestartNodesCrossColocatedWithDriverUrl) {
  SetupDriverRunner();

  driver_index().set_match_callback([](auto args) -> zx::result<FakeDriverIndex::MatchResult> {
    if (args.name().get() == "second") {
      return zx::ok(FakeDriverIndex::MatchResult{
          .url = second_driver_url,
      });
    }
    if (args.name().get() == "third") {
      return zx::ok(FakeDriverIndex::MatchResult{
          .url = "fuchsia-boot:///#meta/third-driver.cm",
      });
    }
    return zx::error(ZX_ERR_NOT_FOUND);
  });

  auto root_driver = StartRootDriver();
  ASSERT_EQ(ZX_OK, root_driver.status_value());

  fdfw::NodeAddArgs args1;
  args1.name() = "second";
  args1.driver_host() = "shared-host";
  PrepareRealmForSecondDriverComponentStart();
  std::shared_ptr<CreatedChild> second =
      root_driver->driver->AddChild(std::move(args1), false, false);
  EXPECT_TRUE(RunLoopUntilIdle());
  auto [driver1, controller1] = StartSecondDriver("dev.second");
  StopListener& stop_listener1 = ServeStopListener(std::move(controller1));

  fdfw::NodeAddArgs args2;
  args2.name() = "third";
  args2.driver_host() = "shared-host";
  PrepareRealmForDriverComponentStart("dev.third", "fuchsia-boot:///#meta/third-driver.cm");
  std::shared_ptr<CreatedChild> third =
      root_driver->driver->AddChild(std::move(args2), false, false);
  EXPECT_TRUE(RunLoopUntilIdle());
  auto third_driver_config = kDefaultThirdDriverPkgConfig;
  std::string binary = std::string(third_driver_config.main_module.open_path);
  StartDriverHandler start_handler2 = [this, binary](TestDriver* driver,
                                                     fdfw::DriverStartArgs start_args) {
    EXPECT_FALSE(start_args.symbols().has_value());
    ValidateProgram(start_args.program(), binary, "false", "false", "false",
                    use_dynamic_linker() ? "true" : "false");
  };
  auto [driver2, controller2] =
      StartDriverWithConfig("dev.third",
                            {
                                .url = "fuchsia-boot:///#meta/third-driver.cm",
                                .binary = binary,
                                .colocate = false,
                                .use_dynamic_linker = use_dynamic_linker(),
                            },
                            std::move(start_handler2), third_driver_config);
  StopListener& stop_listener2 = ServeStopListener(std::move(controller2));

  EXPECT_EQ(0u, driver_runner().bind_manager().NumOrphanedResources());

  PrepareRealmForSecondDriverComponentStart();

  // Prepare for third driver at the same time as the restart will happen on both.
  realm().AddCreateChildHandler([](const fdecl::CollectionRef& collection, const fdecl::Child& decl,
                                   const std::vector<fdecl::Offer>& offers) {
    if (collection.name() == "boot-drivers" && decl.name() == "dev.third") {
      EXPECT_EQ("fuchsia-boot:///#meta/third-driver.cm", decl.url());
      return true;
    }
    return false;
  });

  zx::result count = driver_runner().RestartNodesColocatedWithDriverUrl(
      second_driver_url, fuchsia_driver_development::RestartRematchFlags::kRequested);
  EXPECT_EQ(1u, count.value());
  EXPECT_TRUE(RunLoopUntilIdle());

  EXPECT_TRUE(stop_listener1.is_stopped());
  EXPECT_TRUE(stop_listener2.is_stopped());

  StopDriverComponent(std::move(root_driver->controller));

  // The node was destroyed twice, along with the host.
  realm().AssertDestroyedChildren({
      CreateChildRef("root", "boot-drivers"),
      CreateChildRef("dev.second", "boot-drivers"),
      CreateChildRef("dev.second", "boot-drivers"),
      CreateChildRef("dev.third", "boot-drivers"),
      CreateChildRef("dev.third", "boot-drivers"),
      CreateChildRef("driver-host-shared-host", "driver-hosts"),
  });
}

// Start the second driver with host_restart_on_crash enabled, and then kill the driver host, and
// observe the node start the driver again in another host. Done by both a node client drop, and a
// driver host server binding close.
TEST_P(DriverRunnerTest, StartSecondDriverHostRestartOnCrash) {
  SetupDriverRunner();

  auto root_driver = StartRootDriver();
  ASSERT_EQ(ZX_OK, root_driver.status_value());

  PrepareRealmForSecondDriverComponentStart();
  std::shared_ptr<CreatedChild> child = root_driver->driver->AddChild("second", false, false);
  EXPECT_TRUE(RunLoopUntilIdle());

  auto [driver_1, controller_1] = StartSecondDriver("dev.second", false, true);
  StopListener& stop_listener_1 = ServeStopListener(std::move(controller_1));

  EXPECT_EQ(0u, driver_runner().bind_manager().NumOrphanedResources());

  // Stop the driver host binding.
  PrepareRealmForSecondDriverComponentStart();
  driver_1->CloseBinding();
  EXPECT_TRUE(RunLoopUntilIdle());
  EXPECT_TRUE(stop_listener_1.is_stopped());

  // The driver host and driver should be started again by the node.
  auto [driver_2, controller_2] = StartSecondDriver("dev.second", false, true);
  StopListener& stop_listener_2 = ServeStopListener(std::move(controller_2));

  // Drop the node client binding.
  PrepareRealmForSecondDriverComponentStart();
  driver_2->DropNode();
  EXPECT_TRUE(RunLoopUntilIdle());
  EXPECT_TRUE(stop_listener_2.is_stopped());

  // The driver host and driver should be started again by the node.
  auto [driver_3, controller_3] = StartSecondDriver("dev.second", false, true);
  StopListener& stop_listener_3 = ServeStopListener(std::move(controller_3));

  // Now try to drop the node and close the binding at the same time. They should not break each
  // other.
  PrepareRealmForSecondDriverComponentStart();
  driver_3->CloseBinding();
  EXPECT_TRUE(RunLoopUntilIdle());
  driver_3->DropNode();
  EXPECT_FALSE(RunLoopUntilIdle());
  EXPECT_TRUE(stop_listener_3.is_stopped());

  // The driver host and driver should be started again by the node.
  auto [driver_4, controller_4] = StartSecondDriver("dev.second", false, true);
  StopListener& stop_listener_4 = ServeStopListener(std::move(controller_4));

  // Again try to drop the node and close the binding at the same time but in opposite order.
  PrepareRealmForSecondDriverComponentStart();
  driver_4->DropNode();
  EXPECT_TRUE(RunLoopUntilIdle());
  driver_4->CloseBinding();
  EXPECT_FALSE(RunLoopUntilIdle());
  EXPECT_TRUE(stop_listener_4.is_stopped());

  // The driver host and driver should be started again by the node.
  auto [driver_5, controller_5] = StartSecondDriver("dev.second", false, true);
  StopListener& stop_listener_5 = ServeStopListener(std::move(controller_5));

  // Finally don't RunLoopUntilIdle in between the two.
  PrepareRealmForSecondDriverComponentStart();
  driver_5->CloseBinding();
  driver_5->DropNode();
  EXPECT_TRUE(RunLoopUntilIdle());
  EXPECT_TRUE(stop_listener_5.is_stopped());

  // The driver host and driver should be started again by the node.
  auto [driver_6, controller_6] = StartSecondDriver("dev.second", false, true);
  ServeStopListener(std::move(controller_6));

  StopDriverComponent(std::move(root_driver->controller));

  realm().AssertDestroyedChildren(
      {CreateChildRef("root", "boot-drivers"), CreateChildRef("dev.second", "boot-drivers"),
       CreateChildRef("dev.second", "boot-drivers"), CreateChildRef("dev.second", "boot-drivers"),
       CreateChildRef("dev.second", "boot-drivers"), CreateChildRef("dev.second", "boot-drivers"),
       CreateChildRef("dev.second", "boot-drivers")});
}

// Start the second driver with use_next_vdso enabled,
TEST_P(DriverRunnerTest, StartSecondDriver_UseNextVdso) {
  SetupDriverRunner();

  auto root_driver = StartRootDriver();
  ASSERT_EQ(ZX_OK, root_driver.status_value());

  PrepareRealmForSecondDriverComponentStart();
  std::shared_ptr<CreatedChild> child = root_driver->driver->AddChild("second", false, false);
  EXPECT_TRUE(RunLoopUntilIdle());

  auto [driver_1, controller_1] = StartSecondDriver("dev.second", true, false, true);
  ServeStopListener(std::move(controller_1));

  EXPECT_EQ(0u, driver_runner().bind_manager().NumOrphanedResources());

  StopDriverComponent(std::move(root_driver->controller));
  realm().AssertDestroyedChildren(
      {CreateChildRef("root", "boot-drivers"), CreateChildRef("dev.second", "boot-drivers")});
}

// The root driver adds a node that only binds after a RequestBind() call.
TEST_P(DriverRunnerTest, BindThroughRequest) {
  SetupDriverRunner();

  auto root_driver = StartRootDriver();
  ASSERT_EQ(ZX_OK, root_driver.status_value());

  std::shared_ptr<CreatedChild> child = root_driver->driver->AddChild("child", false, false);
  EXPECT_TRUE(RunLoopUntilIdle());

  ASSERT_EQ(1u, driver_runner().bind_manager().NumOrphanedResources());

  driver_index().set_match_callback([](auto args) -> zx::result<FakeDriverIndex::MatchResult> {
    return zx::ok(FakeDriverIndex::MatchResult{
        .url = second_driver_url,
    });
  });

  PrepareRealmForDriverComponentStart("dev.child", second_driver_url);
  AssertNodeControllerBound(child);
  child->node_controller.value()
      ->RequestBind(fdfw::NodeControllerRequestBindRequest())
      .Then([](fidl::Result<fdfw::NodeController::RequestBind>& result) {
        if (result.is_error()) {
          fdf_log::error("RequestBind error: {}", result.error_value().FormatDescription());
        }
        EXPECT_TRUE(result.is_ok());
      });
  EXPECT_TRUE(RunLoopUntilIdle());
  ASSERT_EQ(0u, driver_runner().bind_manager().NumOrphanedResources());

  auto [driver, controller] = StartSecondDriver("dev.child");
  ServeStopListener(std::move(controller));

  driver->CloseBinding();
  driver->DropNode();
  StopDriverComponent(std::move(root_driver->controller));
  realm().AssertDestroyedChildren(
      {CreateChildRef("root", "boot-drivers"), CreateChildRef("dev.child", "boot-drivers")});
}

// The root driver adds a node that only binds after a RequestBind() call. Then Restarts through
// RequestBind() with force_rebind, once without a url suffix, and another with the url suffix.
TEST_P(DriverRunnerTest, BindAndRestartThroughRequest) {
  SetupDriverRunner();

  auto root_driver = StartRootDriver();
  ASSERT_EQ(ZX_OK, root_driver.status_value());

  std::shared_ptr<CreatedChild> child = root_driver->driver->AddChild("child", false, false);
  EXPECT_TRUE(RunLoopUntilIdle());

  ASSERT_EQ(1u, driver_runner().bind_manager().NumOrphanedResources());

  driver_index().set_match_callback([](auto args) -> zx::result<FakeDriverIndex::MatchResult> {
    if (args.has_driver_url_suffix()) {
      return zx::ok(FakeDriverIndex::MatchResult{
          .url = "fuchsia-boot:///#meta/third-driver.cm",
      });
    }
    return zx::ok(FakeDriverIndex::MatchResult{
        .url = second_driver_url,
    });
  });

  // Prepare realm for the second-driver CreateChild.
  PrepareRealmForDriverComponentStart("dev.child", second_driver_url);

  // Bind the child node to the second-driver driver.
  auto bind_request = fdfw::NodeControllerRequestBindRequest();
  child->node_controller.value()
      ->RequestBind(fdfw::NodeControllerRequestBindRequest())
      .Then([](fidl::Result<fdfw::NodeController::RequestBind>& result) {
        if (result.is_error()) {
          fdf_log::error("RequestBind error: {}", result.error_value().FormatDescription());
        }
        EXPECT_TRUE(result.is_ok());
      });
  EXPECT_TRUE(RunLoopUntilIdle());
  ASSERT_EQ(0u, driver_runner().bind_manager().NumOrphanedResources());

  // Get the second-driver running.
  auto [driver_1, controller_1] = StartSecondDriver("dev.child");
  ServeStopListener(std::move(controller_1));

  // Prepare realm for the second-driver CreateChild again.
  PrepareRealmForDriverComponentStart("dev.child", second_driver_url);

  // Request rebind of the second-driver to the node.
  child->node_controller.value()
      ->RequestBind(fdfw::NodeControllerRequestBindRequest({
          .force_rebind = true,
      }))
      .Then([](fidl::Result<fdfw::NodeController::RequestBind>& result) {
        if (result.is_error()) {
          fdf_log::error("RequestBind error: {}", result.error_value().FormatDescription());
        }
        EXPECT_TRUE(result.is_ok());
      });
  EXPECT_TRUE(RunLoopUntilIdle());

  // Get the second-driver running again.
  auto [driver_2, controller_2] = StartSecondDriver("dev.child");
  ServeStopListener(std::move(controller_2));

  // Prepare realm for the third-driver CreateChild.
  PrepareRealmForDriverComponentStart("dev.child", "fuchsia-boot:///#meta/third-driver.cm");

  // Request rebind of the node with the third-driver.
  child->node_controller.value()
      ->RequestBind(fdfw::NodeControllerRequestBindRequest({
          .force_rebind = true,
          .driver_url_suffix = "third",
      }))
      .Then([](fidl::Result<fdfw::NodeController::RequestBind>& result) {
        if (result.is_error()) {
          fdf_log::error("RequestBind error: {}", result.error_value().FormatDescription());
        }
        EXPECT_TRUE(result.is_ok());
      });
  EXPECT_TRUE(RunLoopUntilIdle());

  // Get the third-driver running.
  auto third_driver_config = kDefaultThirdDriverPkgConfig;
  std::string binary = std::string(third_driver_config.main_module.open_path);
  StartDriverHandler start_handler = [&](TestDriver* driver, fdfw::DriverStartArgs start_args) {
    EXPECT_FALSE(start_args.symbols().has_value());
    ValidateProgram(start_args.program(), binary, "false", "false", "false");
  };
  auto third_driver = StartDriverWithConfig("dev.child",
                                            {
                                                .url = "fuchsia-boot:///#meta/third-driver.cm",
                                                .binary = binary,
                                                .use_dynamic_linker = use_dynamic_linker(),
                                            },
                                            std::move(start_handler), third_driver_config);
  ServeStopListener(std::move(third_driver.controller));

  StopDriverComponent(std::move(root_driver->controller));
  realm().AssertDestroyedChildren({
      CreateChildRef("root", "boot-drivers"),
      CreateChildRef("dev.child", "boot-drivers"),
      CreateChildRef("dev.child", "boot-drivers"),
      CreateChildRef("dev.child", "boot-drivers"),
  });
}

// Start the root driver, and then add a child node that does not bind to a
// second driver.
TEST_P(DriverRunnerTest, StartSecondDriver_UnknownNode) {
  SetupDriverRunner();

  // Unknown driver request in this test requires enabling the introspector's GetMoniker.
  EnableIntrospector();

  auto root_driver = StartRootDriver();
  ASSERT_EQ(ZX_OK, root_driver.status_value());

  std::shared_ptr<CreatedChild> child = root_driver->driver->AddChild("unknown-node", false, false);
  EXPECT_TRUE(RunLoopUntilIdle());

  StartDriver("root", {.close = true, .use_dynamic_linker = use_dynamic_linker()});
  ASSERT_EQ(1u, driver_runner().bind_manager().NumOrphanedResources());

  StopDriverComponent(std::move(root_driver->controller));
  realm().AssertDestroyedChildren({CreateChildRef("root", "boot-drivers")});
}

// Start the root driver, and then add a child node that only binds to a base driver.
TEST_P(DriverRunnerTest, StartSecondDriver_BindOrphanToBaseDriver) {
  bool base_drivers_loaded = false;
  FakeDriverIndex fake_driver_index(
      dispatcher(), [&base_drivers_loaded](auto args) -> zx::result<FakeDriverIndex::MatchResult> {
        if (base_drivers_loaded) {
          if (args.name().get() == "second") {
            return zx::ok(FakeDriverIndex::MatchResult{
                .url = second_driver_url,
            });
          }
        }
        return zx::error(ZX_ERR_NOT_FOUND);
      });
  SetupDriverRunner(std::move(fake_driver_index));

  auto root_driver = StartRootDriver();
  ASSERT_EQ(ZX_OK, root_driver.status_value());

  fdfw::NodeAddArgs args({
      .name = "second",
      .properties2 =
          {
              {
                  fdfw::NodeProperty2({
                      .key = "driver.prop-one",
                      .value = fdfw::NodePropertyValue::WithStringValue("value"),
                  }),
              },
          },
  });
  std::shared_ptr<CreatedChild> child =
      root_driver->driver->AddChild(std::move(args), false, false);
  EXPECT_TRUE(RunLoopUntilIdle());

  // Make sure the node we added was orphaned.
  ASSERT_EQ(1u, driver_runner().bind_manager().NumOrphanedResources());

  // Set the handlers for the new driver.
  PrepareRealmForSecondDriverComponentStart();

  // Tell driver index to return the second driver, and wait for base drivers to load.
  base_drivers_loaded = true;
  driver_runner().TryBindAllAvailable();
  ASSERT_TRUE(RunLoopUntilIdle());

  driver_index().InvokeWatchDriverResponse();
  ASSERT_TRUE(RunLoopUntilIdle());

  // See that we don't have an orphan anymore.
  ASSERT_EQ(0u, driver_runner().bind_manager().NumOrphanedResources());

  StopDriverComponent(std::move(root_driver->controller));
  realm().AssertDestroyedChildren(
      {CreateChildRef("root", "boot-drivers"), CreateChildRef("dev.second", "boot-drivers")});
}

// Start the second driver, and then unbind its associated node.
TEST_P(DriverRunnerTest, StartSecondDriver_UnbindSecondNode) {
  SetupDriverRunner();

  auto root_driver = StartRootDriver();
  ASSERT_EQ(ZX_OK, root_driver.status_value());

  PrepareRealmForSecondDriverComponentStart();
  std::shared_ptr<CreatedChild> child = root_driver->driver->AddChild("second", false, false);
  EXPECT_TRUE(RunLoopUntilIdle());
  auto [driver, controller] = StartSecondDriver("dev.second");
  StopListener& stop_listener = ServeStopListener(std::move(controller));

  // Unbinding the second node stops the driver bound to it.
  driver->DropNode();
  while (!stop_listener.is_stopped()) {
    RunLoopUntilIdle();
  }

  StopDriverComponent(std::move(root_driver->controller));
  realm().AssertDestroyedChildren(
      {CreateChildRef("root", "boot-drivers"), CreateChildRef("dev.second", "boot-drivers")});
}

// Start the second driver, and then close the associated Driver protocol
// channel.
TEST_P(DriverRunnerTest, StartSecondDriver_CloseSecondDriver) {
  SetupDriverRunner();

  auto root_driver = StartRootDriver();
  ASSERT_EQ(ZX_OK, root_driver.status_value());

  PrepareRealmForSecondDriverComponentStart();
  std::shared_ptr<CreatedChild> child = root_driver->driver->AddChild("second", false, false);
  EXPECT_TRUE(RunLoopUntilIdle());
  auto [driver, controller] = StartSecondDriver("dev.second");
  StopListener& stop_listener = ServeStopListener(std::move(controller));

  // Closing the Driver protocol channel of the second driver causes the driver
  // to be stopped.
  driver->CloseBinding();
  while (!stop_listener.is_stopped()) {
    RunLoopUntilIdle();
  }

  StopDriverComponent(std::move(root_driver->controller));
  realm().AssertDestroyedChildren(
      {CreateChildRef("root", "boot-drivers"), CreateChildRef("dev.second", "boot-drivers")});
}

// Start a chain of drivers, and then unbind the second driver's node.
TEST_P(DriverRunnerTest, StartDriverChain_UnbindSecondNode) {
  FakeDriverIndex driver_index(dispatcher(),
                               [](auto args) -> zx::result<FakeDriverIndex::MatchResult> {
                                 std::string name(args.name().get());
                                 return zx::ok(FakeDriverIndex::MatchResult{
                                     .url = "fuchsia-boot:///#meta/" + name + "-driver.cm",
                                 });
                               });
  SetupDriverRunner(std::move(driver_index));

  auto root_driver = StartRootDriver();
  ASSERT_EQ(ZX_OK, root_driver.status_value());

  constexpr size_t kMaxNodes = 10;

  // The drivers vector will start with the root.
  std::vector<StartDriverResult> drivers;
  drivers.reserve(kMaxNodes + 1);
  drivers.emplace_back(std::move(root_driver.value()));

  std::vector<std::shared_ptr<CreatedChild>> children;
  children.reserve(kMaxNodes);

  std::string component_moniker = "dev";
  for (size_t i = 0; i < kMaxNodes; i++) {
    auto child_name = "node-" + std::to_string(i);
    component_moniker += "." + child_name;
    PrepareRealmForDriverComponentStart(component_moniker,
                                        "fuchsia-boot:///#meta/" + child_name + "-driver.cm");
    children.emplace_back(drivers.back().driver->AddChild(child_name, false, false));
    EXPECT_TRUE(RunLoopUntilIdle());

    auto driver_config = kDefaultDriverPkgConfig;
    std::string binary = std::string(driver_config.main_module.open_path);
    StartDriverHandler start_handler = [this, binary](TestDriver* driver,
                                                      fdfw::DriverStartArgs start_args) {
      EXPECT_FALSE(start_args.symbols().has_value());
      ValidateProgram(start_args.program(), binary, "false", "false", "false");
    };
    drivers.emplace_back(StartDriverWithConfig(
        component_moniker,
        {
            .url = "fuchsia-boot:///#meta/node-" + std::to_string(i) + "-driver.cm",
            .binary = binary,
            .use_dynamic_linker = use_dynamic_linker(),
        },
        std::move(start_handler), driver_config));
  }

  // Unbinding the second node stops all drivers bound in the sub-tree, in a
  // depth-first order.
  std::vector<size_t> indices;
  size_t listeners = 0;

  // Start at 1 since 0 is the root driver.
  for (size_t i = 1; i < drivers.size(); i++) {
    ServeStopListener(std::move(drivers[i].controller), TeardownWatcher(listeners + 1, indices));
    listeners++;
  }

  drivers[1].driver->DropNode();
  EXPECT_TRUE(RunLoopUntilIdle());
  EXPECT_THAT(indices, ElementsAre(10, 9, 8, 7, 6, 5, 4, 3, 2, 1));

  StopDriverComponent(std::move(drivers[0].controller));
  realm().AssertDestroyedChildren(
      {CreateChildRef("root", "boot-drivers"), CreateChildRef("dev.node-0", "boot-drivers"),
       CreateChildRef("dev.node-0.node-1", "boot-drivers"),
       CreateChildRef("dev.node-0.node-1.node-2", "boot-drivers"),
       CreateChildRef("dev.node-0.node-1.node-2.node-3", "boot-drivers"),
       CreateChildRef("dev.node-0.node-1.node-2.node-3.node-4", "boot-drivers"),
       CreateChildRef("dev.node-0.node-1.node-2.node-3.node-4.node-5", "boot-drivers"),
       CreateChildRef("dev.node-0.node-1.node-2.node-3.node-4.node-5.node-6", "boot-drivers"),
       CreateChildRef("dev.node-0.node-1.node-2.node-3.node-4.node-5.node-6.node-7",
                      "boot-drivers"),
       CreateChildRef("dev.node-0.node-1.node-2.node-3.node-4.node-5.node-6.node-7.node-8",
                      "boot-drivers"),
       CreateChildRef("dev.node-0.node-1.node-2.node-3.node-4.node-5.node-6.node-7.node-8.node-9",
                      "boot-drivers")});
}

// Start the second driver, and then unbind the root node.
TEST_P(DriverRunnerTest, StartSecondDriver_UnbindRootNode) {
  SetupDriverRunner();

  std::vector<size_t> indices;

  auto root_driver = StartRootDriver();
  ASSERT_EQ(ZX_OK, root_driver.status_value());
  StopListener& stop_listener_root =
      ServeStopListener(std::move(root_driver->controller), TeardownWatcher(0, indices));

  PrepareRealmForSecondDriverComponentStart();
  std::shared_ptr<CreatedChild> child = root_driver->driver->AddChild("second", false, false);
  EXPECT_TRUE(RunLoopUntilIdle());
  auto [driver, controller] = StartSecondDriver("dev.second");
  StopListener& stop_listener_second =
      ServeStopListener(std::move(controller), TeardownWatcher(1, indices));

  // Unbinding the root node stops all drivers.
  root_driver->driver->DropNode();

  while (!stop_listener_second.is_stopped() || !stop_listener_root.is_stopped()) {
    RunLoopUntilIdle();
  }

  EXPECT_THAT(indices, ElementsAre(1, 0));
}

// Start the second driver, and then Stop the root node.
TEST_P(DriverRunnerTest, StartSecondDriver_StopRootNode) {
  SetupDriverRunner();

  // These represent the order that Driver::Stop is called
  std::vector<size_t> driver_stop_indices;
  // These represent the order that the component stop happens.
  std::vector<size_t> indices;

  auto root_driver = StartRootDriver();
  ASSERT_EQ(ZX_OK, root_driver.status_value());
  StopListener& stop_listener_root =
      ServeStopListener(std::move(root_driver->controller), TeardownWatcher(0, indices));

  root_driver->driver->SetStopHandler(
      [&driver_stop_indices]() { driver_stop_indices.push_back(0); });

  PrepareRealmForSecondDriverComponentStart();
  std::shared_ptr<CreatedChild> child = root_driver->driver->AddChild("second", false, false);
  EXPECT_TRUE(RunLoopUntilIdle());
  auto [driver, controller] = StartSecondDriver("dev.second");
  StopListener& stop_listener_second =
      ServeStopListener(std::move(controller), TeardownWatcher(1, indices));

  driver->SetStopHandler([&driver_stop_indices]() { driver_stop_indices.push_back(1); });

  // Simulate the Component Framework calling Stop on the root driver.
  [[maybe_unused]] auto result = stop_listener_root.client()->Stop();

  while (!stop_listener_second.is_stopped() || !stop_listener_root.is_stopped()) {
    RunLoopUntilIdle();
  }

  // Check that the driver components were shut down in order.
  EXPECT_THAT(indices, ElementsAre(1, 0));
  // Check that Driver::Stop was called in order.
  EXPECT_THAT(driver_stop_indices, ElementsAre(1, 0));
}

// Start the second driver, stop the root driver, and block while waiting on the
// second driver to shut down.
TEST_P(DriverRunnerTest, StartSecondDriver_BlockOnSecondDriver) {
  SetupDriverRunner();

  std::vector<size_t> indices;

  auto root_driver = StartRootDriver();
  ASSERT_EQ(ZX_OK, root_driver.status_value());
  StopListener& stop_listener_root =
      ServeStopListener(std::move(root_driver->controller), TeardownWatcher(0, indices));

  PrepareRealmForSecondDriverComponentStart();
  std::shared_ptr<CreatedChild> child = root_driver->driver->AddChild("second", false, false);
  EXPECT_TRUE(RunLoopUntilIdle());
  auto [driver, controller] = StartSecondDriver("dev.second");
  StopListener& stop_listener_second =
      ServeStopListener(std::move(controller), TeardownWatcher(1, indices));

  // When the second driver gets asked to stop, don't drop the binding,
  // which means DriverRunner will wait for the binding to drop.
  driver->SetDontCloseBindingInStop();

  // Stopping the root driver stops all drivers, but is blocked waiting on the
  // second driver to stop.
  [[maybe_unused]] auto result = stop_listener_root.client()->Stop();
  EXPECT_TRUE(RunLoopUntilIdle());
  // Nothing has shut down yet, since we are waiting.
  EXPECT_THAT(indices, ElementsAre());

  // Attempt to add a child node to a removed node.
  driver->AddChild("should_fail", false, true);
  EXPECT_TRUE(RunLoopUntilIdle());

  // Unbind the second node, indicating the second driver has stopped, thereby
  // continuing the stop sequence.
  driver->CloseBinding();

  while (!stop_listener_second.is_stopped() || !stop_listener_root.is_stopped()) {
    RunLoopUntilIdle();
  }

  EXPECT_THAT(indices, ElementsAre(1, 0));
}

TEST_P(DriverRunnerTest, CreateAndBindCompositeNodeSpec) {
  SetupDriverRunner();

  // Add a match for the composite node spec that we are creating.
  std::string name("test-group");

  const fuchsia_driver_framework::CompositeNodeSpec fidl_spec(
      {.name = name,
       .parents2 = std::vector<fuchsia_driver_framework::ParentSpec2>{
           fuchsia_driver_framework::ParentSpec2({
               .bind_rules = std::vector<fuchsia_driver_framework::BindRule2>(),
               .properties = std::vector<fuchsia_driver_framework::NodeProperty2>(),
           }),
           fuchsia_driver_framework::ParentSpec2({
               .bind_rules = std::vector<fuchsia_driver_framework::BindRule2>(),
               .properties = std::vector<fuchsia_driver_framework::NodeProperty2>(),
           })}});

  auto spec = std::make_unique<driver_manager::CompositeNodeSpec>(
      driver_manager::CompositeNodeSpecCreateInfo{
          .name = name,
          .parents = fidl_spec.parents2().value(),
      },
      dispatcher(), &driver_runner());
  fidl::Arena<> arena;

  driver_runner().composite_node_spec_manager().AddSpec(
      fidl::ToWire(arena, fidl_spec), std::move(spec),
      [](fit::result<fuchsia_driver_framework::CompositeNodeSpecError> result) {
        ASSERT_TRUE(result.is_ok());
      });
  EXPECT_TRUE(RunLoopUntilIdle());

  ASSERT_EQ(
      2u,
      driver_runner().composite_node_spec_manager().specs().at(name)->GetParentResources().size());

  ASSERT_FALSE(
      driver_runner().composite_node_spec_manager().specs().at(name)->GetParentResources().at(0));

  ASSERT_FALSE(
      driver_runner().composite_node_spec_manager().specs().at(name)->GetParentResources().at(1));

  auto root_driver = StartRootDriver();
  ASSERT_EQ(ZX_OK, root_driver.status_value());

  std::shared_ptr<CreatedChild> child_0 =
      root_driver->driver->AddChild("dev-group-0", false, false);
  std::shared_ptr<CreatedChild> child_1 =
      root_driver->driver->AddChild("dev-group-1", false, false);

  PrepareRealmForDriverComponentStart("dev.dev-group-1.test-group",
                                      "fuchsia-boot:///#meta/composite-driver.cm");
  EXPECT_TRUE(RunLoopUntilIdle());

  ASSERT_TRUE(
      driver_runner().composite_node_spec_manager().specs().at(name)->GetParentResources().at(0));

  ASSERT_TRUE(
      driver_runner().composite_node_spec_manager().specs().at(name)->GetParentResources().at(1));

  auto composite_driver_config = kDefaultCompositeDriverPkgConfig;
  std::string binary = std::string(composite_driver_config.main_module.open_path);
  StartDriverHandler start_handler = [this, binary](TestDriver* driver,
                                                    fdfw::DriverStartArgs start_args) {
    ValidateProgram(start_args.program(), binary, "true", "false", "false");
  };
  auto composite_driver =
      StartDriverWithConfig("dev.dev-group-1.test-group",
                            {
                                .url = "fuchsia-boot:///#meta/composite-driver.cm",
                                .binary = binary,
                                .colocate = true,
                                .use_dynamic_linker = use_dynamic_linker(),
                            },
                            std::move(start_handler), composite_driver_config);
  ServeStopListener(std::move(composite_driver.controller));

  auto hierarchy = Inspect();
  ASSERT_NO_FATAL_FAILURE(CheckNode(hierarchy, {
                                                   .node_name = {"node_topology"},
                                                   .child_names = {"dev"},
                                               }));

  ASSERT_NO_FATAL_FAILURE(CheckNode(hierarchy, {.node_name = {"node_topology", "dev"},
                                                .child_names = {"dev-group-0", "dev-group-1"},
                                                .str_properties = {
                                                    {"driver", root_driver_url},
                                                }}));

  ASSERT_NO_FATAL_FAILURE(CheckNode(
      hierarchy,
      {.node_name = {"node_topology", "dev", "dev-group-0"}, .child_names = {"test-group"}}));

  ASSERT_NO_FATAL_FAILURE(CheckNode(
      hierarchy,
      {.node_name = {"node_topology", "dev", "dev-group-1"}, .child_names = {"test-group"}}));

  ASSERT_NO_FATAL_FAILURE(
      CheckNode(hierarchy, {.node_name = {"node_topology", "dev", "dev-group-0", "test-group"},
                            .str_properties = {
                                {"driver", "fuchsia-boot:///#meta/composite-driver.cm"},
                            }}));

  StopDriverComponent(std::move(root_driver->controller));
  realm().AssertDestroyedChildren({CreateChildRef("root", "boot-drivers"),
                                   CreateChildRef("dev.dev-group-1.test-group", "boot-drivers")});
}

// Start a driver and inspect the driver runner.
TEST_P(DriverRunnerTest, StartAndInspect) {
  SetupDriverRunner();

  auto root_driver = StartRootDriver();
  ASSERT_EQ(ZX_OK, root_driver.status_value());

  PrepareRealmForSecondDriverComponentStart();
  fdfw::NodeAddArgs args(
      {
          .name = "second",
          .symbols =
              {
                  {
                      fdfw::NodeSymbol({
                          .name = "symbol-A",
                          .address = 0x2301,
                      }),
                      fdfw::NodeSymbol({
                          .name = "symbol-B",
                          .address = 0x1985,
                      }),
                  },
              },
          .offers2 =
              {
                  {
                      fuchsia_driver_framework::Offer::WithZirconTransport(
                          fuchsia_component_decl::Offer::WithService(fdecl::OfferService({
                              .source_name = "fuchsia.package.ProtocolA",
                              .source_instance_filter = std::vector<std::string>{"default"},
                              .renamed_instances =
                                  std::vector<fdecl::NameMapping>{
                                      fdecl::NameMapping("default", "instance-1"),
                                  },
                          }))),
                      fuchsia_driver_framework::Offer::WithZirconTransport(
                          fuchsia_component_decl::Offer::WithService(
                              fdecl::OfferService(
                                  {
                                      .source_name = "fuchsia.package.ProtocolB",
                                      .source_instance_filter = std::vector<std::string>{"default"},
                                      .renamed_instances =
                                          std::vector<fdecl::NameMapping>{
                                              fdecl::NameMapping("default", "instance-1"),
                                          },
                                  }))),
                  },
              },
      });
  std::shared_ptr<CreatedChild> child =
      root_driver->driver->AddChild(std::move(args), false, false);
  EXPECT_TRUE(RunLoopUntilIdle());

  auto hierarchy = Inspect();
  ASSERT_EQ("root", hierarchy.node().name());
  ASSERT_EQ(3ul, hierarchy.children().size());

  ASSERT_NO_FATAL_FAILURE(CheckNode(hierarchy, {
                                                   .node_name = {"node_topology"},
                                                   .child_names = {"dev"},
                                               }));

  ASSERT_NO_FATAL_FAILURE(CheckNode(hierarchy, {.node_name = {"node_topology", "dev"},
                                                .child_names = {"second"},
                                                .str_properties = {
                                                    {"driver", root_driver_url},
                                                }}));

  ASSERT_NO_FATAL_FAILURE(CheckNode(
      hierarchy, {.node_name = {"node_topology", "dev", "second"},
                  .child_names = {},
                  .str_properties = {{"driver", "unbound"}},
                  .array_str_properties = {
                      {"offers", {"fuchsia.package.ProtocolA", "fuchsia.package.ProtocolB"}},
                      {"symbols", {"symbol-A", "symbol-B"}},
                  }}));

  ASSERT_NO_FATAL_FAILURE(CheckNode(hierarchy, {
                                                   .node_name = {"orphan_nodes"},
                                               }));
  ASSERT_NO_FATAL_FAILURE(CheckNode(hierarchy, {
                                                   .node_name = {"composite_node_specs"},
                                               }));

  StopDriverComponent(std::move(root_driver->controller));
  realm().AssertDestroyedChildren(
      {CreateChildRef("root", "boot-drivers"), CreateChildRef("dev.second", "boot-drivers")});
}

TEST_P(DriverRunnerTest, TestTearDownNodeTreeWithManyChildren) {
  SetupDriverRunner();

  auto root_driver = StartRootDriver();
  ASSERT_EQ(ZX_OK, root_driver.status_value());

  std::vector<std::shared_ptr<CreatedChild>> children;
  for (size_t i = 0; i < 100; i++) {
    children.emplace_back(root_driver->driver->AddChild("child" + std::to_string(i), false, false));
    EXPECT_TRUE(RunLoopUntilIdle());
  }

  Unbind();
}

TEST_P(DriverRunnerTest, TestBindResultTracker) {
  bool callback_called = false;
  bool* callback_called_ptr = &callback_called;

  auto callback = [callback_called_ptr](
                      fidl::VectorView<fuchsia_driver_development::wire::NodeBindingInfo> results) {
    ASSERT_EQ(std::string_view("node_name"), results[0].node_name().get());
    ASSERT_EQ(std::string_view("driver_url"), results[0].driver_url().get());
    ASSERT_EQ(1ul, results.size());
    *callback_called_ptr = true;
  };

  driver_manager::BindResultTracker tracker(3, std::move(callback));
  ASSERT_EQ(false, callback_called);
  tracker.ReportNoBind();
  ASSERT_EQ(false, callback_called);
  tracker.ReportSuccessfulBind(std::string_view("node_name"), "driver_url");
  ASSERT_EQ(false, callback_called);
  tracker.ReportNoBind();
  ASSERT_EQ(true, callback_called);

  callback_called = false;
  auto callback_two =
      [callback_called_ptr](
          fidl::VectorView<fuchsia_driver_development::wire::NodeBindingInfo> results) {
        ASSERT_EQ(0ul, results.size());
        *callback_called_ptr = true;
      };

  driver_manager::BindResultTracker tracker_two(3, std::move(callback_two));
  ASSERT_EQ(false, callback_called);
  tracker_two.ReportNoBind();
  ASSERT_EQ(false, callback_called);
  tracker_two.ReportNoBind();
  ASSERT_EQ(false, callback_called);
  tracker_two.ReportNoBind();
  ASSERT_EQ(true, callback_called);

  callback_called = false;
  auto callback_three =
      [callback_called_ptr](
          fidl::VectorView<fuchsia_driver_development::wire::NodeBindingInfo> results) {
        ASSERT_EQ(std::string_view("node_name"), results[0].node_name().get());
        ASSERT_EQ(std::string_view("test_spec"),
                  results[0].composite_parents()[0].composite().spec().name().get());
        ASSERT_EQ(std::string_view("test_spec_2"),
                  results[0].composite_parents()[1].composite().spec().name().get());
        ASSERT_EQ(1ul, results.size());
        *callback_called_ptr = true;
      };

  driver_manager::BindResultTracker tracker_three(3, std::move(callback_three));
  ASSERT_EQ(false, callback_called);
  tracker_three.ReportNoBind();
  ASSERT_EQ(false, callback_called);
  tracker_three.ReportNoBind();
  ASSERT_EQ(false, callback_called);

  {
    tracker_three.ReportSuccessfulBind(std::string_view("node_name"),
                                       std::vector{
                                           fdfw::CompositeParent{{
                                               .composite = fdfw::CompositeInfo{{
                                                   .spec = fdfw::CompositeNodeSpec{{
                                                       .name = "test_spec",
                                                   }},
                                               }},
                                           }},
                                           fdfw::CompositeParent{{
                                               .composite = fdfw::CompositeInfo{{
                                                   .spec = fdfw::CompositeNodeSpec{{
                                                       .name = "test_spec_2",
                                                   }},
                                               }},
                                           }},
                                       });
  }

  ASSERT_EQ(true, callback_called);
}

// Start the root driver, add a child node, and verify that the child node's device controller is
// reachable.
TEST_P(DriverRunnerTest, ConnectToDeviceController) {
  SetupDriverRunner();

  auto root_driver = StartRootDriver();
  ASSERT_EQ(ZX_OK, root_driver.status_value());

  const char* kChildName = "node-1";
  std::shared_ptr<CreatedChild> created_child =
      root_driver->driver->AddChild(kChildName, true, false);
  EXPECT_TRUE(RunLoopUntilIdle());

  auto device_controller = ConnectToDeviceController(kChildName);

  // Call one of the device controller's method in order to verify that the controller works.
  device_controller->GetTopologicalPath().Then(
      [](fidl::WireUnownedResult<fuchsia_device::Controller::GetTopologicalPath>& reply) {
        ASSERT_EQ(reply.status(), ZX_OK);
        ASSERT_TRUE(reply->is_ok());
        ASSERT_EQ(reply.value()->path.get(), "/dev/node-1");
      });
  EXPECT_TRUE(RunLoopUntilIdle());
}

// Start the root driver, add a child node, and verify that calling the child's device controller's
// `ConnectToController` FIDL method works.
TEST_P(DriverRunnerTest, ConnectToControllerFidlMethod) {
  SetupDriverRunner();

  auto root_driver = StartRootDriver();
  ASSERT_EQ(ZX_OK, root_driver.status_value());

  const char* kChildName = "node-1";
  std::shared_ptr<CreatedChild> created_child =
      root_driver->driver->AddChild(kChildName, true, false);
  EXPECT_TRUE(RunLoopUntilIdle());

  auto device_controller_1 = ConnectToDeviceController(kChildName);

  auto controller_endpoints = fidl::Endpoints<fuchsia_device::Controller>::Create();
  fidl::OneWayStatus result =
      device_controller_1->ConnectToController(std::move(controller_endpoints.server));
  ASSERT_TRUE(RunLoopUntilIdle());
  ASSERT_EQ(result.status(), ZX_OK);

  fidl::WireClient<fuchsia_device::Controller> device_controller_2{
      std::move(controller_endpoints.client), dispatcher()};

  // Verify that the two device controllers connect to the same device server.
  // This is done by verifying the topological paths returned by the device controllers are the
  // same.
  std::string topological_path_1;
  device_controller_1->GetTopologicalPath().Then(
      [&](fidl::WireUnownedResult<fuchsia_device::Controller::GetTopologicalPath>& reply) {
        ASSERT_EQ(reply.status(), ZX_OK);
        ASSERT_TRUE(reply->is_ok());
        topological_path_1 = reply.value()->path.get();
      });

  std::string topological_path_2;
  device_controller_2->GetTopologicalPath().Then(
      [&](fidl::WireUnownedResult<fuchsia_device::Controller::GetTopologicalPath>& reply) {
        ASSERT_EQ(reply.status(), ZX_OK);
        ASSERT_TRUE(reply->is_ok());
        topological_path_2 = reply.value()->path.get();
      });

  ASSERT_TRUE(RunLoopUntilIdle());
  ASSERT_EQ(topological_path_1, topological_path_2);
}

// Verify that device controller's Bind FIDL method works.
TEST_P(DriverRunnerTest, DeviceControllerBind) {
  SetupDriverRunner();

  auto root_driver = StartRootDriver();
  ASSERT_EQ(ZX_OK, root_driver.status_value());

  std::shared_ptr<CreatedChild> child = root_driver->driver->AddChild("child", false, false);
  EXPECT_TRUE(RunLoopUntilIdle());

  ASSERT_EQ(1u, driver_runner().bind_manager().NumOrphanedResources());

  driver_index().set_match_callback([](auto args) -> zx::result<FakeDriverIndex::MatchResult> {
    EXPECT_EQ(args.driver_url_suffix().get(), second_driver_url);
    return zx::ok(FakeDriverIndex::MatchResult{
        .url = second_driver_url,
    });
  });

  PrepareRealmForDriverComponentStart("dev.child", second_driver_url);
  AssertNodeControllerBound(child);

  // Bind the driver.
  ASSERT_EQ(1u, driver_runner().bind_manager().NumOrphanedResources());
  auto device_controller = ConnectToDeviceController("child");
  device_controller->Bind(fidl::StringView::FromExternal(second_driver_url))
      .Then([](fidl::WireUnownedResult<fuchsia_device::Controller::Bind>& reply) {
        ASSERT_EQ(reply.status(), ZX_OK);
      });
  ASSERT_TRUE(RunLoopUntilIdle());

  // Verify the driver was bound.
  ASSERT_EQ(0u, driver_runner().bind_manager().NumOrphanedResources());
  auto [driver, controller] = StartSecondDriver("dev.child");
  ServeStopListener(std::move(controller));

  driver->CloseBinding();
  driver->DropNode();
  StopDriverComponent(std::move(root_driver->controller));
  realm().AssertDestroyedChildren(
      {CreateChildRef("root", "boot-drivers"), CreateChildRef("dev.child", "boot-drivers")});
}

TEST_P(DriverRunnerTest, LogStackTrace_Success) {
  SetupDriverRunner();

  auto root_driver = StartRootDriver();
  ASSERT_EQ(ZX_OK, root_driver.status_value());

  auto& node_token = root_driver->driver->node_token();
  ASSERT_TRUE(node_token.has_value());

  zx::event token_copy;
  ASSERT_EQ(ZX_OK, node_token->duplicate(ZX_RIGHT_SAME_RIGHTS, &token_copy));

  auto debug_client = fidl::WireClient<fuchsia_driver_token::Debug>(ConnectToDebug(), dispatcher());

  bool callback_called = false;
  debug_client->LogStackTrace(std::move(token_copy))
      .ThenExactlyOnce(
          [&callback_called](
              fidl::WireUnownedResult<fuchsia_driver_token::Debug::LogStackTrace>& result) {
            ASSERT_TRUE(result.ok());
            ASSERT_TRUE(result->is_ok());
            callback_called = true;
          });

  EXPECT_TRUE(RunLoopUntilIdle());
  EXPECT_TRUE(callback_called);
  EXPECT_TRUE(driver_host().stack_trace_triggered());
}

TEST_P(DriverRunnerTest, LogStackTrace_NotFound) {
  SetupDriverRunner();

  auto root_driver = StartRootDriver();
  ASSERT_EQ(ZX_OK, root_driver.status_value());

  zx::event random_token;
  ASSERT_EQ(ZX_OK, zx::event::create(0, &random_token));

  auto debug_client = fidl::WireClient<fuchsia_driver_token::Debug>(ConnectToDebug(), dispatcher());

  bool callback_called = false;
  debug_client->LogStackTrace(std::move(random_token))
      .ThenExactlyOnce(
          [&callback_called](
              fidl::WireUnownedResult<fuchsia_driver_token::Debug::LogStackTrace>& result) {
            ASSERT_TRUE(result.ok());
            ASSERT_TRUE(result->is_error());
            ASSERT_EQ(ZX_ERR_NOT_FOUND, result->error_value());
            callback_called = true;
          });

  EXPECT_TRUE(RunLoopUntilIdle());
  EXPECT_TRUE(callback_called);
  EXPECT_FALSE(driver_host().stack_trace_triggered());
}

TEST_P(DriverRunnerTest, GetHostKoid_Success) {
  SetupDriverRunner();

  auto root_driver = StartRootDriver();
  ASSERT_EQ(ZX_OK, root_driver.status_value());

  auto& node_token = root_driver->driver->node_token();
  ASSERT_TRUE(node_token.has_value());

  zx::event token_copy;
  ASSERT_EQ(ZX_OK, node_token->duplicate(ZX_RIGHT_SAME_RIGHTS, &token_copy));

  auto debug_client = fidl::WireClient<fuchsia_driver_token::Debug>(ConnectToDebug(), dispatcher());

  bool callback_called = false;
  debug_client->GetHostKoid(std::move(token_copy))
      .ThenExactlyOnce(
          [&callback_called,
           this](fidl::WireUnownedResult<fuchsia_driver_token::Debug::GetHostKoid>& result) {
            ASSERT_TRUE(result.ok());
            ASSERT_TRUE(result->is_ok());
            EXPECT_EQ(driver_host().process_koid(), result->value()->host_koid);
            callback_called = true;
          });

  EXPECT_TRUE(RunLoopUntilIdle());
  EXPECT_TRUE(callback_called);
}

TEST_P(DriverRunnerTest, GetHostKoid_NotFound) {
  SetupDriverRunner();

  auto root_driver = StartRootDriver();
  ASSERT_EQ(ZX_OK, root_driver.status_value());

  zx::event random_token;
  ASSERT_EQ(ZX_OK, zx::event::create(0, &random_token));

  auto debug_client = fidl::WireClient<fuchsia_driver_token::Debug>(ConnectToDebug(), dispatcher());

  bool callback_called = false;
  debug_client->GetHostKoid(std::move(random_token))
      .ThenExactlyOnce(
          [&callback_called](
              fidl::WireUnownedResult<fuchsia_driver_token::Debug::GetHostKoid>& result) {
            ASSERT_TRUE(result.ok());
            ASSERT_TRUE(result->is_error());
            ASSERT_EQ(ZX_ERR_NOT_FOUND, result->error_value());
            callback_called = true;
          });

  EXPECT_TRUE(RunLoopUntilIdle());
  EXPECT_TRUE(callback_called);
}

TEST(CompositeServiceOfferTest, WorkingOffer) {
  const std::string_view kServiceName = "fuchsia.service";

  auto offer = driver_manager::NodeOffer{
      .source_collection = driver_manager::Collection::kBoot,
      .transport = driver_manager::OfferTransport::ZirconTransport,
      .service_name = std::string(kServiceName),
      .source_instance_filter = std::vector<std::string>{"default", "instance-2"},
      .renamed_instances =
          std::vector<fdecl::NameMapping>{
              fdecl::NameMapping("instance-1", "default"),
              fdecl::NameMapping("instance-1", "instance-2"),
          },
  };
  auto new_offer = driver_manager::CreateCompositeOffer(offer, "parent_node", false);
  ASSERT_EQ(2ul, new_offer.renamed_instances.size());
  // Check that the default instance got renamed.
  ASSERT_EQ(std::string("instance-1"), new_offer.renamed_instances[0].source_name());
  ASSERT_EQ(std::string("parent_node"), new_offer.renamed_instances[0].target_name());

  // Check that a non-default instance stayed the same.
  ASSERT_EQ(std::string("instance-1"), new_offer.renamed_instances[1].source_name());
  ASSERT_EQ(std::string("instance-2"), new_offer.renamed_instances[1].target_name());

  ASSERT_EQ(2ul, new_offer.source_instance_filter.size());
  // Check that the default filter got renamed.
  ASSERT_EQ(std::string("parent_node"), new_offer.source_instance_filter[0]);

  // Check that a non-default filter stayed the same.
  ASSERT_EQ(std::string("instance-2"), new_offer.source_instance_filter[1]);
}

TEST(CompositeServiceOfferTest, WorkingOfferPrimary) {
  const std::string_view kServiceName = "fuchsia.service";

  auto offer = driver_manager::NodeOffer{
      .source_collection = driver_manager::Collection::kBoot,
      .transport = driver_manager::OfferTransport::ZirconTransport,
      .service_name = std::string(kServiceName),
      .source_instance_filter = std::vector<std::string>{"default", "instance-2"},
      .renamed_instances =
          std::vector<fdecl::NameMapping>{
              fdecl::NameMapping("instance-1", "default"),
              fdecl::NameMapping("instance-1", "instance-2"),
          },
  };
  auto new_offer = driver_manager::CreateCompositeOffer(offer, "parent_node", true);

  ASSERT_EQ(3ul, new_offer.renamed_instances.size());
  // Check that the default instance stayed the same (because we're primary).
  ASSERT_EQ(std::string("instance-1"), new_offer.renamed_instances[0].source_name());
  ASSERT_EQ(std::string("default"), new_offer.renamed_instances[0].target_name());

  // Check that the default instance got renamed.
  ASSERT_EQ(std::string("instance-1"), new_offer.renamed_instances[1].source_name());
  ASSERT_EQ(std::string("parent_node"), new_offer.renamed_instances[1].target_name());

  // Check that a non-default instance stayed the same.
  ASSERT_EQ(std::string("instance-1"), new_offer.renamed_instances[2].source_name());
  ASSERT_EQ(std::string("instance-2"), new_offer.renamed_instances[2].target_name());

  ASSERT_EQ(3ul, new_offer.source_instance_filter.size());
  // Check that the default filter stayed the same (because we're primary).
  EXPECT_EQ(std::string("default"), new_offer.source_instance_filter[0]);

  // Check that the default filter got renamed.
  EXPECT_EQ(std::string("parent_node"), new_offer.source_instance_filter[1]);

  // Check that a non-default filter stayed the same.
  EXPECT_EQ(std::string("instance-2"), new_offer.source_instance_filter[2]);
}

TEST(NodeTest, ToCollection) {
  async::Loop loop{&kAsyncLoopConfigNeverAttachToThread};
  constexpr char kGrandparentName[] = "grandparent";
  std::shared_ptr<Node> grandparent =
      std::make_shared<Node>(kGrandparentName, std::weak_ptr<Node>{}, nullptr, loop.dispatcher());

  constexpr char kParentName[] = "parent";
  std::shared_ptr<Node> parent =
      std::make_shared<Node>(kParentName, grandparent, nullptr, loop.dispatcher());

  constexpr char kChild1Name[] = "child1";
  std::shared_ptr<Node> child1 =
      std::make_shared<Node>(kChild1Name, parent, nullptr, loop.dispatcher());

  constexpr char kChild2Name[] = "child2";
  std::shared_ptr<Node> child2 = std::make_shared<Node>(
      kChild2Name,
      std::unordered_map<std::string, std::weak_ptr<Node>>{{"parent", parent}, {"child1", child1}},
      std::vector<std::string>{"parent", "child1"}, nullptr, loop.dispatcher(), 0);

  // Test parentless
  EXPECT_EQ(ToCollection(*grandparent, fdfw::DriverPackageType::kBoot), Collection::kBoot);
  EXPECT_EQ(ToCollection(*grandparent, fdfw::DriverPackageType::kBase), Collection::kPackage);
  EXPECT_EQ(ToCollection(*grandparent, fdfw::DriverPackageType::kCached), Collection::kFullPackage);
  EXPECT_EQ(ToCollection(*grandparent, fdfw::DriverPackageType::kUniverse),
            Collection::kFullPackage);

  // // Test single parent with grandparent collection set to none
  grandparent->set_collection(Collection::kNone);
  parent->set_collection(Collection::kNone);
  EXPECT_EQ(ToCollection(*child1, fdfw::DriverPackageType::kBoot), Collection::kBoot);
  EXPECT_EQ(ToCollection(*child1, fdfw::DriverPackageType::kBase), Collection::kPackage);
  EXPECT_EQ(ToCollection(*child1, fdfw::DriverPackageType::kCached), Collection::kFullPackage);
  EXPECT_EQ(ToCollection(*child1, fdfw::DriverPackageType::kUniverse), Collection::kFullPackage);

  grandparent->set_collection(Collection::kNone);
  parent->set_collection(Collection::kBoot);
  EXPECT_EQ(ToCollection(*child1, fdfw::DriverPackageType::kBoot), Collection::kBoot);
  EXPECT_EQ(ToCollection(*child1, fdfw::DriverPackageType::kBase), Collection::kPackage);
  EXPECT_EQ(ToCollection(*child1, fdfw::DriverPackageType::kCached), Collection::kFullPackage);
  EXPECT_EQ(ToCollection(*child1, fdfw::DriverPackageType::kUniverse), Collection::kFullPackage);

  grandparent->set_collection(Collection::kNone);
  parent->set_collection(Collection::kPackage);
  EXPECT_EQ(ToCollection(*child1, fdfw::DriverPackageType::kBoot), Collection::kPackage);
  EXPECT_EQ(ToCollection(*child1, fdfw::DriverPackageType::kBase), Collection::kPackage);
  EXPECT_EQ(ToCollection(*child1, fdfw::DriverPackageType::kCached), Collection::kFullPackage);
  EXPECT_EQ(ToCollection(*child1, fdfw::DriverPackageType::kUniverse), Collection::kFullPackage);

  grandparent->set_collection(Collection::kNone);
  parent->set_collection(Collection::kFullPackage);
  EXPECT_EQ(ToCollection(*child1, fdfw::DriverPackageType::kBoot), Collection::kFullPackage);
  EXPECT_EQ(ToCollection(*child1, fdfw::DriverPackageType::kBase), Collection::kFullPackage);
  EXPECT_EQ(ToCollection(*child1, fdfw::DriverPackageType::kCached), Collection::kFullPackage);
  EXPECT_EQ(ToCollection(*child1, fdfw::DriverPackageType::kUniverse), Collection::kFullPackage);

  // Test single parent with parent collection set to none
  grandparent->set_collection(Collection::kBoot);
  parent->set_collection(Collection::kNone);
  EXPECT_EQ(ToCollection(*child1, fdfw::DriverPackageType::kBoot), Collection::kBoot);
  EXPECT_EQ(ToCollection(*child1, fdfw::DriverPackageType::kBase), Collection::kPackage);
  EXPECT_EQ(ToCollection(*child1, fdfw::DriverPackageType::kCached), Collection::kFullPackage);
  EXPECT_EQ(ToCollection(*child1, fdfw::DriverPackageType::kUniverse), Collection::kFullPackage);

  grandparent->set_collection(Collection::kPackage);
  parent->set_collection(Collection::kNone);
  EXPECT_EQ(ToCollection(*child1, fdfw::DriverPackageType::kBoot), Collection::kPackage);
  EXPECT_EQ(ToCollection(*child1, fdfw::DriverPackageType::kBase), Collection::kPackage);
  EXPECT_EQ(ToCollection(*child1, fdfw::DriverPackageType::kCached), Collection::kFullPackage);
  EXPECT_EQ(ToCollection(*child1, fdfw::DriverPackageType::kUniverse), Collection::kFullPackage);

  grandparent->set_collection(Collection::kFullPackage);
  parent->set_collection(Collection::kNone);
  EXPECT_EQ(ToCollection(*child1, fdfw::DriverPackageType::kBoot), Collection::kFullPackage);
  EXPECT_EQ(ToCollection(*child1, fdfw::DriverPackageType::kBase), Collection::kFullPackage);
  EXPECT_EQ(ToCollection(*child1, fdfw::DriverPackageType::kCached), Collection::kFullPackage);
  EXPECT_EQ(ToCollection(*child1, fdfw::DriverPackageType::kUniverse), Collection::kFullPackage);

  // Test multi parent
  grandparent->set_collection(Collection::kNone);
  parent->set_collection(Collection::kNone);
  child1->set_collection(Collection::kNone);
  EXPECT_EQ(ToCollection(*child2, fdfw::DriverPackageType::kBoot), Collection::kBoot);
  EXPECT_EQ(ToCollection(*child2, fdfw::DriverPackageType::kBase), Collection::kPackage);
  EXPECT_EQ(ToCollection(*child2, fdfw::DriverPackageType::kCached), Collection::kFullPackage);
  EXPECT_EQ(ToCollection(*child2, fdfw::DriverPackageType::kUniverse), Collection::kFullPackage);

  grandparent->set_collection(Collection::kNone);
  parent->set_collection(Collection::kBoot);
  child1->set_collection(Collection::kNone);
  EXPECT_EQ(ToCollection(*child2, fdfw::DriverPackageType::kBoot), Collection::kBoot);
  EXPECT_EQ(ToCollection(*child2, fdfw::DriverPackageType::kBase), Collection::kPackage);
  EXPECT_EQ(ToCollection(*child2, fdfw::DriverPackageType::kCached), Collection::kFullPackage);
  EXPECT_EQ(ToCollection(*child2, fdfw::DriverPackageType::kUniverse), Collection::kFullPackage);

  grandparent->set_collection(Collection::kNone);
  parent->set_collection(Collection::kNone);
  child1->set_collection(Collection::kPackage);
  EXPECT_EQ(ToCollection(*child2, fdfw::DriverPackageType::kBoot), Collection::kPackage);
  EXPECT_EQ(ToCollection(*child2, fdfw::DriverPackageType::kBase), Collection::kPackage);
  EXPECT_EQ(ToCollection(*child2, fdfw::DriverPackageType::kCached), Collection::kFullPackage);
  EXPECT_EQ(ToCollection(*child2, fdfw::DriverPackageType::kUniverse), Collection::kFullPackage);

  grandparent->set_collection(Collection::kFullPackage);
  parent->set_collection(Collection::kFullPackage);
  child1->set_collection(Collection::kFullPackage);
  EXPECT_EQ(ToCollection(*child2, fdfw::DriverPackageType::kBoot), Collection::kFullPackage);
  EXPECT_EQ(ToCollection(*child2, fdfw::DriverPackageType::kBase), Collection::kFullPackage);
  EXPECT_EQ(ToCollection(*child2, fdfw::DriverPackageType::kCached), Collection::kFullPackage);
  EXPECT_EQ(ToCollection(*child2, fdfw::DriverPackageType::kUniverse), Collection::kFullPackage);

  // Test multi parent with one parent collection set to none
  grandparent->set_collection(Collection::kBoot);
  parent->set_collection(Collection::kNone);
  child1->set_collection(Collection::kBoot);
  EXPECT_EQ(ToCollection(*child2, fdfw::DriverPackageType::kBoot), Collection::kBoot);
  EXPECT_EQ(ToCollection(*child2, fdfw::DriverPackageType::kBase), Collection::kPackage);
  EXPECT_EQ(ToCollection(*child2, fdfw::DriverPackageType::kCached), Collection::kFullPackage);
  EXPECT_EQ(ToCollection(*child2, fdfw::DriverPackageType::kUniverse), Collection::kFullPackage);

  grandparent->set_collection(Collection::kPackage);
  parent->set_collection(Collection::kNone);
  child1->set_collection(Collection::kBoot);
  EXPECT_EQ(ToCollection(*child2, fdfw::DriverPackageType::kBoot), Collection::kPackage);
  EXPECT_EQ(ToCollection(*child2, fdfw::DriverPackageType::kBase), Collection::kPackage);
  EXPECT_EQ(ToCollection(*child2, fdfw::DriverPackageType::kCached), Collection::kFullPackage);
  EXPECT_EQ(ToCollection(*child2, fdfw::DriverPackageType::kUniverse), Collection::kFullPackage);

  grandparent->set_collection(Collection::kFullPackage);
  parent->set_collection(Collection::kNone);
  child1->set_collection(Collection::kBoot);
  EXPECT_EQ(ToCollection(*child2, fdfw::DriverPackageType::kBoot), Collection::kFullPackage);
  EXPECT_EQ(ToCollection(*child2, fdfw::DriverPackageType::kBase), Collection::kFullPackage);
  EXPECT_EQ(ToCollection(*child2, fdfw::DriverPackageType::kCached), Collection::kFullPackage);
  EXPECT_EQ(ToCollection(*child2, fdfw::DriverPackageType::kUniverse), Collection::kFullPackage);
}

// Verify that attempting to start a colocated driver on a node with a torn-down
// driver host fails gracefully.
TEST_P(DriverRunnerTest, ColocateAfterDriverHostTeardownFails) {
  SetupDriverRunner();

  // Start the root driver.
  auto root_driver = StartRootDriver();
  ASSERT_EQ(ZX_OK, root_driver.status_value());

  PrepareRealmForSecondDriverComponentStart();
  std::shared_ptr<CreatedChild> child =
      root_driver->driver->AddChild("second", /*owned=*/false, /*expect_error=*/false);
  EXPECT_TRUE(RunLoopUntilIdle());

  // Simulates a driver_host process closing its DriverHost channel.
  CloseDriverHostBindings();
  RunLoopUntilIdle();

  // Attempt to start the colocated second driver, expecting the start to fail.
  auto [driver, controller] =
      StartDriver("dev.second", {
                                    .url = second_driver_url,
                                    .binary = second_driver_binary,
                                    .colocate = true,
                                    .close = true,
                                    .use_dynamic_linker = use_dynamic_linker(),
                                });

  // Verify that the controller channel is closed because the start failed.
  zx_signals_t pending;
  EXPECT_EQ(controller.channel().wait_one(ZX_CHANNEL_PEER_CLOSED, zx::deadline_after(zx::sec(5)),
                                          &pending),
            ZX_OK);
  EXPECT_TRUE(pending & ZX_CHANNEL_PEER_CLOSED);
}

TEST_P(DriverRunnerTest, NodeManagerAddNodeWithDependency) {
  SetupDriverRunner();

  // Start the root driver.
  auto root_driver = StartRootDriver();
  ASSERT_EQ(ZX_OK, root_driver.status_value());

  // 1. Provide a resource on the root node.
  fuchsia_driver_framework::NodeProperty2 prop{{
      .key = "my_key",
      .value = fuchsia_driver_framework::NodePropertyValue::WithStringValue("my_val"),
  }};
  fuchsia_driver_framework::ResourceArgs resource_args({
      .name = "my_resource",
      .properties = std::vector<fuchsia_driver_framework::NodeProperty2>{std::move(prop)},
      .offers = std::vector<fuchsia_driver_framework::Offer>(),
  });

  auto endpoints = fidl::Endpoints<fuchsia_driver_framework::ResourceController>::Create();
  bool provide_resource_called = false;
  root_driver->driver->node()
      ->ProvideResource({std::move(resource_args), std::move(endpoints.server)})
      .Then([&provide_resource_called](auto& result) {
        ASSERT_TRUE(result.is_ok());
        provide_resource_called = true;
      });
  EXPECT_TRUE(RunLoopUntilIdle());
  ASSERT_TRUE(provide_resource_called);

  // 2. Connect to the NodeManager protocol.
  fidl::Endpoints<fuchsia_driver_framework::NodeManager> nm_endpoints =
      fidl::Endpoints<fuchsia_driver_framework::NodeManager>::Create();
  fidl::BindServer(dispatcher(), std::move(nm_endpoints.server), &driver_runner());
  auto nm_client = fidl::WireClient<fuchsia_driver_framework::NodeManager>(
      std::move(nm_endpoints.client), dispatcher());

  // 3. Construct a Node2 that depends on our provided resource.
  fuchsia_driver_framework::ResourceProperty prop2(
      "my_key", fuchsia_driver_framework::ResourcePropertyValue::WithStringValue("my_val"));

  fuchsia_driver_framework::Selector selector;
  selector.include_properties() =
      std::vector<fuchsia_driver_framework::ResourceProperty>{std::move(prop2)};

  fuchsia_driver_framework::Dependency dep;
  dep.selector() = std::move(selector);

  fuchsia_driver_framework::Node2 node2;
  node2.name() = "composite_node_2";
  node2.dependencies() = std::vector<fuchsia_driver_framework::Dependency>{std::move(dep)};

  auto controller_endpoints = fidl::Endpoints<fuchsia_driver_framework::NodeController>::Create();
  auto node_endpoints = fidl::Endpoints<fuchsia_driver_framework::Node>::Create();

  fidl::Arena arena;
  bool add_node_called = false;
  nm_client
      ->AddNode(fidl::ToWire(arena, node2), std::move(controller_endpoints.server),
                std::move(node_endpoints.server))
      .Then([&add_node_called](auto& result) {
        ASSERT_TRUE(result.ok());
        ASSERT_FALSE(result->is_error());
        add_node_called = true;
      });
  EXPECT_TRUE(RunLoopUntilIdle());
  ASSERT_TRUE(add_node_called);

  static_cast<driver_manager::NodeManager&>(driver_runner()).TryResolvePendingNodes();
  RunLoopUntilIdle();

  // 4. Verify that the composite node was instantiated in the topology as a child of the root node
  // (owner of the resource).
  bool found = false;
  for (const auto& child : driver_runner().root_node()->children()) {
    if (child->name() == "composite_node_2") {
      found = true;
      break;
    }
  }
  EXPECT_TRUE(found);
}

TEST_P(DriverRunnerTest, NodeManagerAddNodeMatchedToDriver) {
  FakeDriverIndex driver_index(
      dispatcher(),
      [](auto args) -> zx::result<FakeDriverIndex::MatchResult> {
        return zx::error(ZX_ERR_NOT_FOUND);
      },
      [](fidl::AnyArena& arena,
         auto args) -> zx::result<fuchsia_driver_framework::wire::CompositeDriverMatch> {
        auto driver_info = fuchsia_driver_framework::wire::DriverInfo::Builder(arena)
                               .url(arena, "fuchsia-boot:///#meta/composite-driver.cm")
                               .colocate(true)
                               .package_type(fdfw::DriverPackageType::kBoot)
                               .Build();
        auto composite_driver = fuchsia_driver_framework::wire::CompositeDriverInfo::Builder(arena)
                                    .composite_name(arena, "test-composite")
                                    .driver_info(driver_info)
                                    .Build();

        auto parent_names = fidl::VectorView<fidl::StringView>(arena, 1);
        parent_names[0] = fidl::StringView(arena, "my_resource");

        auto match = fuchsia_driver_framework::wire::CompositeDriverMatch::Builder(arena)
                         .composite_driver(composite_driver)
                         .parent_names(parent_names)
                         .primary_parent_index(0)
                         .Build();
        return zx::ok(match);
      });

  SetupDriverRunner(std::move(driver_index));

  // Start the root driver.
  auto root_driver = StartRootDriver();
  ASSERT_EQ(ZX_OK, root_driver.status_value());

  // 1. Provide a resource on the root node.
  fuchsia_driver_framework::NodeProperty2 prop{{
      .key = "my_key",
      .value = fuchsia_driver_framework::NodePropertyValue::WithStringValue("my_val"),
  }};
  fuchsia_driver_framework::ResourceArgs resource_args({
      .name = "my_resource",
      .properties = std::vector<fuchsia_driver_framework::NodeProperty2>{std::move(prop)},
      .offers = std::vector<fuchsia_driver_framework::Offer>(),
  });

  auto endpoints = fidl::Endpoints<fuchsia_driver_framework::ResourceController>::Create();
  bool provide_resource_called = false;
  root_driver->driver->node()
      ->ProvideResource({std::move(resource_args), std::move(endpoints.server)})
      .Then([&provide_resource_called](auto& result) {
        ASSERT_TRUE(result.is_ok());
        provide_resource_called = true;
      });
  EXPECT_TRUE(RunLoopUntilIdle());
  ASSERT_TRUE(provide_resource_called);

  // 2. Connect to the NodeManager protocol.
  fidl::Endpoints<fuchsia_driver_framework::NodeManager> nm_endpoints =
      fidl::Endpoints<fuchsia_driver_framework::NodeManager>::Create();
  fidl::BindServer(dispatcher(), std::move(nm_endpoints.server), &driver_runner());
  auto nm_client = fidl::WireClient<fuchsia_driver_framework::NodeManager>(
      std::move(nm_endpoints.client), dispatcher());

  // 3. Construct a Node2 that depends on our provided resource.
  fuchsia_driver_framework::ResourceProperty prop2(
      "my_key", fuchsia_driver_framework::ResourcePropertyValue::WithStringValue("my_val"));

  fuchsia_driver_framework::Selector selector;
  selector.include_properties() =
      std::vector<fuchsia_driver_framework::ResourceProperty>{std::move(prop2)};

  fuchsia_driver_framework::Dependency dep;
  dep.selector() = std::move(selector);

  fuchsia_driver_framework::Node2 node2;
  node2.name() = "composite_node_2";
  node2.dependencies() = std::vector<fuchsia_driver_framework::Dependency>{std::move(dep)};

  auto controller_endpoints = fidl::Endpoints<fuchsia_driver_framework::NodeController>::Create();

  PrepareRealmForDriverComponentStart("dev.composite_node_2",
                                      "fuchsia-boot:///#meta/composite-driver.cm");

  auto composite_driver_config = kDefaultCompositeDriverPkgConfig;
  std::string binary = std::string(composite_driver_config.main_module.open_path);
  StartDriverHandler start_handler = [this, binary](TestDriver* driver,
                                                    fdfw::DriverStartArgs start_args) {
    ValidateProgram(start_args.program(), binary, "true", "false", "false");
  };

  fidl::Arena arena;
  bool add_node_called = false;
  // Node ref is invalid so it matches a driver.
  nm_client->AddNode(fidl::ToWire(arena, node2), std::move(controller_endpoints.server), {})
      .Then([&add_node_called](auto& result) {
        ASSERT_TRUE(result.ok());
        ASSERT_FALSE(result->is_error());
        add_node_called = true;
      });
  EXPECT_TRUE(RunLoopUntilIdle());
  ASSERT_TRUE(add_node_called);

  auto composite_driver =
      StartDriverWithConfig("dev.composite_node_2",
                            {
                                .url = "fuchsia-boot:///#meta/composite-driver.cm",
                                .binary = binary,
                                .colocate = true,
                                .use_dynamic_linker = use_dynamic_linker(),
                            },
                            std::move(start_handler), composite_driver_config);
  ServeStopListener(std::move(composite_driver.controller));

  // 4. Verify that the composite node was instantiated in the topology as a child of the root node
  // (owner of the resource).
  bool found = false;
  for (const auto& child : driver_runner().root_node()->children()) {
    if (child->name() == "composite_node_2") {
      found = true;
      break;
    }
  }
  EXPECT_TRUE(found);
}

TEST_P(DriverRunnerTest, NodeManagerAddNodeMissingSelector) {
  SetupDriverRunner();

  // Start the root driver.
  auto root_driver = StartRootDriver();
  ASSERT_EQ(ZX_OK, root_driver.status_value());

  // Connect to the NodeManager protocol.
  fidl::Endpoints<fuchsia_driver_framework::NodeManager> nm_endpoints =
      fidl::Endpoints<fuchsia_driver_framework::NodeManager>::Create();
  fidl::BindServer(dispatcher(), std::move(nm_endpoints.server), &driver_runner());
  auto nm_client = fidl::WireClient<fuchsia_driver_framework::NodeManager>(
      std::move(nm_endpoints.client), dispatcher());

  // Construct a Node2 that has a dependency but without a selector.
  fuchsia_driver_framework::Dependency dep;
  // Note: we do NOT set dep.selector().

  fuchsia_driver_framework::Node2 node2;
  node2.name() = "invalid_node";
  node2.dependencies() = std::vector<fuchsia_driver_framework::Dependency>{std::move(dep)};

  auto controller_endpoints = fidl::Endpoints<fuchsia_driver_framework::NodeController>::Create();
  auto node_endpoints = fidl::Endpoints<fuchsia_driver_framework::Node>::Create();

  fidl::Arena arena;
  bool add_node_called = false;
  nm_client
      ->AddNode(fidl::ToWire(arena, node2), std::move(controller_endpoints.server),
                std::move(node_endpoints.server))
      .Then([&add_node_called](auto& result) {
        ASSERT_TRUE(result.ok());
        ASSERT_TRUE(result->is_error());
        EXPECT_EQ(fuchsia_driver_framework::NodeError::kUnsupportedArgs, result->error_value());
        add_node_called = true;
      });
  EXPECT_TRUE(RunLoopUntilIdle());
  ASSERT_TRUE(add_node_called);
}

TEST_P(DriverRunnerTest, NodeManagerAddNodeInvalidOffer) {
  SetupDriverRunner();

  // Start the root driver.
  auto root_driver = StartRootDriver();
  ASSERT_EQ(ZX_OK, root_driver.status_value());

  // Connect to the NodeManager protocol.
  fidl::Endpoints<fuchsia_driver_framework::NodeManager> nm_endpoints =
      fidl::Endpoints<fuchsia_driver_framework::NodeManager>::Create();
  fidl::BindServer(dispatcher(), std::move(nm_endpoints.server), &driver_runner());
  auto nm_client = fidl::WireClient<fuchsia_driver_framework::NodeManager>(
      std::move(nm_endpoints.client), dispatcher());

  // Construct a Node2 that has a dependency with an invalid offer (using Service with mismatched
  // target/source name).
  fuchsia_driver_framework::Offer offer = fuchsia_driver_framework::Offer::WithZirconTransport(
      fuchsia_component_decl::Offer::WithService(fdecl::OfferService({
          .source_name = "fuchsia.package.Service",
          .target_name = "fuchsia.package.Renamed",
      })));

  fuchsia_driver_framework::Selector selector;
  selector.offers() = std::vector<fuchsia_driver_framework::Offer>{std::move(offer)};

  fuchsia_driver_framework::Dependency dep;
  dep.selector() = std::move(selector);

  fuchsia_driver_framework::Node2 node2;
  node2.name() = "invalid_node";
  node2.dependencies() = std::vector<fuchsia_driver_framework::Dependency>{std::move(dep)};

  auto controller_endpoints = fidl::Endpoints<fuchsia_driver_framework::NodeController>::Create();
  auto node_endpoints = fidl::Endpoints<fuchsia_driver_framework::Node>::Create();

  fidl::Arena arena;
  bool add_node_called = false;
  nm_client
      ->AddNode(fidl::ToWire(arena, node2), std::move(controller_endpoints.server),
                std::move(node_endpoints.server))
      .Then([&add_node_called](auto& result) {
        ASSERT_TRUE(result.ok());
        ASSERT_TRUE(result->is_error());
        EXPECT_EQ(fuchsia_driver_framework::NodeError::kUnsupportedArgs, result->error_value());
        add_node_called = true;
      });
  EXPECT_TRUE(RunLoopUntilIdle());
  ASSERT_TRUE(add_node_called);
}

TEST_P(DriverRunnerTest, LeaseAllDriversForShutdown) {
  auto cleanup = fit::defer([this]() { realm().ClearCreateChildHandlers(); });

  // Create endpoints for Topology.
  auto topology_endpoints = fidl::Endpoints<fuchsia_power_broker::Topology>::Create();

  // Initialize the TestTopology server on the dispatcher.
  TestTopology test_topology(dispatcher());
  test_topology.Bind(std::move(topology_endpoints.server));

  // Create endpoints for CpuElementManager.
  auto cpu_endpoints = fidl::Endpoints<fuchsia_power_system::CpuElementManager>::Create();

  // Initialize the TestCpuElementManager server on the dispatcher.
  TestCpuElementManager test_cpu_element_manager(dispatcher());
  test_cpu_element_manager.Bind(std::move(cpu_endpoints.server));

  // Initialize DriverRunner with power enabled and the topology client endpoint.
  SetupDriverRunnerWithPower(std::move(topology_endpoints.client), std::move(cpu_endpoints.client));

  // Start the root driver.
  auto root_driver = StartRootDriver();
  ASSERT_EQ(ZX_OK, root_driver.status_value());

  // Add a child node.
  PrepareRealmForSecondDriverComponentStart();
  std::shared_ptr<CreatedChild> child = root_driver->driver->AddChild("second", false, false);
  EXPECT_TRUE(RunLoopUntilIdle());

  // Start the second driver so the child node is bound.
  bool did_bind = false;
  child->node_controller.value()->WaitForDriver().Then(
      [&did_bind](fidl::Result<fuchsia_driver_framework::NodeController::WaitForDriver>& result) {
        if (result.is_ok() && result.value().driver_started_node_token().has_value()) {
          did_bind = true;
          return;
        }
        ZX_ASSERT_MSG(false, "WaitForDriver failed");
      });
  auto [driver, controller] = StartSecondDriver("dev.second");
  EXPECT_TRUE(did_bind);

  // Call LeaseStatecontrolShutdown.
  bool lease_completed = false;
  driver_runner().LeaseAllDriversForShutdown([&lease_completed]() { lease_completed = true; });

  // Run the loop until all async work is done.
  EXPECT_TRUE(RunLoopUntilIdle());

  // Verify that the callback was called.
  EXPECT_TRUE(lease_completed);

  // Verify that 2 lease requests were made (one for root, one for second).
  EXPECT_EQ(2u, test_topology.lease_requests().size());

  // Clean up.
  ServeStopListener(std::move(controller));
  StopDriverComponent(std::move(root_driver->controller));
}

// The tests are parameterized on whether to use the dynamic linker or not.
INSTANTIATE_TEST_SUITE_P(/* no prefix */, DriverRunnerTest, testing::Values(true, false),
                         [](const testing::TestParamInfo<bool>& info) {
                           if (info.param) {
                             return "DynamicLinker";
                           }
                           return "Legacy";
                         });

}  // namespace driver_runner
