// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/debug_agent/zircon_system_interface.h"

#include <string_view>

#include <gtest/gtest.h>

#include "fidl/fuchsia.component/cpp/fidl.h"
#include "src/developer/debug/debug_agent/component_manager.h"
#include "src/developer/debug/debug_agent/system_interface.h"
#include "src/developer/debug/debug_agent/testing/runtime_dir_helper.h"
#include "src/developer/debug/debug_agent/zircon_process_handle.h"
#include "src/developer/debug/debug_agent/zircon_utils.h"
#include "src/developer/debug/ipc/filter_utils.h"
#include "src/developer/debug/ipc/records.h"
#include "src/developer/debug/shared/event_pair_watcher.h"
#include "src/developer/debug/shared/message_loop.h"
#include "src/developer/debug/shared/message_loop_fuchsia.h"
#include "src/developer/debug/shared/test_with_loop.h"

namespace debug_agent {

namespace {

// Recursively walks the process tree and returns true if there is a process
// matching the given koid. Fills the process_name if such process can be found.
// Fills the component_info if the process belongs to some component.
bool FindProcess(const debug_ipc::ProcessTreeRecord& record, zx_koid_t koid_to_find,
                 std::string* process_name, std::vector<debug_ipc::ComponentInfo>* component_info) {
  if (record.koid == koid_to_find) {
    *process_name = record.name;
    return true;
  }
  for (const auto& child : record.children) {
    if (FindProcess(child, koid_to_find, process_name, component_info)) {
      if (!record.components.empty()) {
        component_info->insert(component_info->end(), record.components.begin(),
                               record.components.end());
      }
      return true;
    }
  }
  return false;
}

}  // namespace

class ZirconSystemInterfaceTest : public debug::TestWithLoop, public debug::EventPairWatcher {
 public:
  debug::MessageLoopFuchsia& loop() {
    return reinterpret_cast<debug::MessageLoopFuchsia&>(debug::TestWithLoop::loop());
  }

  // EventPairWatcher implementation.
  void OnPeerClosed(zx_handle_t) override { loop().QuitNow(); }
};

TEST_F(ZirconSystemInterfaceTest, DISABLED_GetProcessTree) {
  ZirconSystemInterface system_interface;

  system_interface.zircon_component_manager().SetReadyCallback([&]() { loop().QuitNow(); });
  loop().Run();

  debug_ipc::ProcessTreeRecord root = system_interface.GetProcessTree();

  // The root node should be a job with some children.
  EXPECT_EQ(debug_ipc::TaskType::kJob, root.type);
  EXPECT_FALSE(root.children.empty());

  // Query ourself.
  auto self = zx::process::self();
  zx_koid_t self_koid = zircon::KoidForObject(*self);
  ASSERT_NE(ZX_KOID_INVALID, self_koid);

  // Our koid should be somewhere in the tree.
  std::string process_name;
  std::vector<debug_ipc::ComponentInfo> all_component_info;
  EXPECT_TRUE(FindProcess(root, self_koid, &process_name, &all_component_info));

  // The process_name and component info should match
  EXPECT_EQ(zircon::NameForObject(*self), process_name);
  ASSERT_FALSE(all_component_info.empty());
  ASSERT_EQ(all_component_info.size(), 1ull);

  const auto& component_info = all_component_info[0];
  EXPECT_EQ(".", component_info.moniker);
  // The url will include a hash that cannot be compared.
  ASSERT_FALSE(component_info.url.empty());
  std::string_view prefix = "fuchsia-pkg://fuchsia.com/debug_agent_unit_tests";
  std::string_view suffix = "#meta/debug_agent_unit_tests.cm";
  ASSERT_GE(component_info.url.size(), prefix.size() + suffix.size());
  EXPECT_EQ(prefix, component_info.url.substr(0, prefix.size()));
  EXPECT_EQ(suffix, component_info.url.substr(component_info.url.size() - suffix.size()));
}

TEST_F(ZirconSystemInterfaceTest, DISABLED_FindComponentInfo) {
  ZirconSystemInterface system_interface;

  system_interface.zircon_component_manager().SetReadyCallback([&]() { loop().QuitNow(); });
  loop().Run();

  zx::process handle;
  zx::process::self()->duplicate(ZX_RIGHT_SAME_RIGHTS, &handle);
  ZirconProcessHandle self(std::move(handle));

  auto components = system_interface.GetComponentManager().FindComponentInfo(self);
  ASSERT_EQ(components.size(), 1ull);

  auto component_info = components[0];
  EXPECT_EQ(".", component_info.moniker);
  // The url will include a hash that cannot be compared.
  ASSERT_FALSE(component_info.url.empty());
  std::string_view prefix = "fuchsia-pkg://fuchsia.com/debug_agent_unit_tests";
  std::string_view suffix = "#meta/debug_agent_unit_tests.cm";
  ASSERT_GE(component_info.url.size(), prefix.size() + suffix.size());
  EXPECT_EQ(prefix, component_info.url.substr(0, prefix.size()));
  EXPECT_EQ(suffix, component_info.url.substr(component_info.url.size() - suffix.size()));
}

TEST_F(ZirconSystemInterfaceTest, ComponentEvents) {
  ZirconSystemInterface system_interface;
  // Need a concrete instance to call |OnComponentEvent| directly.
  ZirconComponentManager* zircon_component_manager =
      reinterpret_cast<ZirconComponentManager*>(&system_interface.GetComponentManager());

  constexpr char kElfMoniker[] = "some/moniker";
  constexpr char kElfUrl[] = "fuchsia-pkg://fuchsia.com/package1#meta/component.cm";
  constexpr char kNonElfMoniker[] = "some/non_elf/moniker";
  constexpr char kNonElfUrl[] = "fuchsia-pkg://fuchsia.com/package2#meta/non_elf.cm";
  constexpr zx_koid_t kExpectedKoid = 23;

  testing::RuntimeDirHelper helper;
  helper.AddJobIdFile(kExpectedKoid);
  helper.Start(loop().dispatcher());

  // Inject an ELF component.
  fuchsia_component::EventHeader header;
  header.component_url(kElfUrl);
  header.moniker(kElfMoniker);
  header.event_type(fuchsia_component::EventType::kDebugStarted);

  fuchsia_component::Event event;
  event.header() = std::move(header);

  zx::eventpair left, right;
  ASSERT_EQ(zx_eventpair_create(0, left.reset_and_get_address(), right.reset_and_get_address()),
            ZX_OK);

  fuchsia_component::DebugStartedPayload payload;
  payload.runtime_dir() = helper.GetScopedDirectoryHandle(kExpectedKoid);
  payload.break_on_start() = std::move(right);
  event.payload() = fuchsia_component::EventPayload::WithDebugStarted(std::move(payload));

  // Inject the event.
  zircon_component_manager->OnComponentEvent(std::move(event));

  debug::MessageLoop::WatchHandle handle;
  loop().WatchEventPair(left.get(), this, &handle);
  loop().Run();

  const auto& info = system_interface.GetComponentManager().FindComponentInfo(kExpectedKoid);
  EXPECT_EQ(info.size(), 1u);
  EXPECT_EQ(info[0].moniker, kElfMoniker);
  EXPECT_EQ(info[0].url, kElfUrl);

  fuchsia_component::EventHeader stopped_header;
  stopped_header.component_url(kElfUrl);
  stopped_header.moniker(kElfMoniker);
  stopped_header.event_type(fuchsia_component::EventType::kStopped);

  fuchsia_component::Event stopped_event;
  stopped_event.header() = std::move(stopped_header);

  fuchsia_component::StoppedPayload stopped_payload;
  stopped_payload.exit_code(0);
  stopped_event.payload() =
      fuchsia_component::EventPayload::WithStopped(std::move(stopped_payload));

  zircon_component_manager->OnComponentEvent(std::move(stopped_event));

  EXPECT_TRUE(system_interface.GetComponentManager().FindComponentInfo(kExpectedKoid).empty());

  // Inject a non-ELF component.
  fuchsia_component::EventHeader non_elf_header;
  non_elf_header.component_url(kNonElfUrl);
  non_elf_header.moniker(kNonElfMoniker);
  non_elf_header.event_type(fuchsia_component::EventType::kDebugStarted);

  fuchsia_component::Event non_elf_event;
  non_elf_event.header() = std::move(non_elf_header);

  ASSERT_EQ(zx_eventpair_create(0, left.reset_and_get_address(), right.reset_and_get_address()),
            ZX_OK);

  fuchsia_component::DebugStartedPayload non_elf_payload;
  non_elf_payload.runtime_dir() = std::nullopt;
  non_elf_payload.break_on_start() = std::move(right);
  non_elf_event.payload() =
      fuchsia_component::EventPayload::WithDebugStarted(std::move(non_elf_payload));

  zircon_component_manager->OnComponentEvent(std::move(non_elf_event));

  loop().WatchEventPair(left.get(), this, &handle);
  loop().Run();

  // Still shouldn't have any ELF component info.
  EXPECT_TRUE(system_interface.GetComponentManager().FindComponentInfo(kExpectedKoid).empty());
  ASSERT_EQ(system_interface.GetComponentManager().GetNonElfComponentInfo().size(), 1u);

  const auto& it = system_interface.GetComponentManager().GetNonElfComponentInfo().begin();
  ASSERT_NE(it, system_interface.GetComponentManager().GetNonElfComponentInfo().end());

  const auto& component_info = it->second;
  EXPECT_EQ(component_info.moniker, kNonElfMoniker);
  EXPECT_EQ(component_info.url, kNonElfUrl);

  fuchsia_component::EventHeader non_elf_stopped_header;
  non_elf_stopped_header.component_url(kNonElfUrl);
  non_elf_stopped_header.moniker(kNonElfMoniker);
  non_elf_stopped_header.event_type(fuchsia_component::EventType::kStopped);

  fuchsia_component::Event non_elf_stopped_event;
  non_elf_stopped_event.header() = std::move(non_elf_stopped_header);

  fuchsia_component::StoppedPayload non_elf_stopped_payload;
  non_elf_stopped_payload.exit_code(0);
  non_elf_stopped_event.payload() =
      fuchsia_component::EventPayload::WithStopped(std::move(non_elf_stopped_payload));

  zircon_component_manager->OnComponentEvent(std::move(non_elf_stopped_event));

  loop().RunUntilNoTasks();

  EXPECT_TRUE(zircon_component_manager->GetNonElfComponentInfo().empty());
}

TEST_F(ZirconSystemInterfaceTest, FilterMatchComponents) {
  ZirconSystemInterface system_interface;

  // Create a job tree like this:
  // 1: root-job
  //   2: child_job1 fake/moniker
  //     ...
  //   5: child_job2 other/moniker
  //     ...

  constexpr zx_koid_t kRootJobKoid = 1;
  constexpr zx_koid_t kChildJob1Koid = 2;
  constexpr zx_koid_t kChildJob2Koid = 5;

  auto& parent_jobs = system_interface.parent_jobs_;
  parent_jobs.insert({kChildJob1Koid, kRootJobKoid});
  parent_jobs.insert({kChildJob2Koid, kRootJobKoid});

  auto& component_info = system_interface.zircon_component_manager().running_component_info_;
  component_info.insert(
      {kChildJob1Koid, {"fake/moniker", "fuchsia-pkg://fuchsia.com/component1#meta/component.cm"}});
  component_info.insert(
      {kChildJob2Koid,
       {"other/moniker", "fuchsia-pkg://fuchsia.com/some_other#meta/other_component.cm"}});

  system_interface.zircon_component_manager().SetReadyCallback([&]() { loop().QuitNow(); });
  loop().Run();

  debug_ipc::Filter filter{.type = debug_ipc::Filter::Type::kComponentName,
                           .pattern = "component.cm"};
  auto components = system_interface.GetComponentManager().FindComponentInfo(kChildJob1Koid);
  EXPECT_EQ(components.size(), 1u);
  EXPECT_TRUE(debug_ipc::FilterMatches(filter, "", components));

  filter = {.type = debug_ipc::Filter::Type::kComponentUrl,
            .pattern = "fuchsia-pkg://fuchsia.com/some_other#meta/other_component.cm"};
  components = system_interface.GetComponentManager().FindComponentInfo(kChildJob2Koid);
  EXPECT_EQ(components.size(), 1u);
  EXPECT_TRUE(debug_ipc::FilterMatches(filter, "", components));
}

TEST_F(ZirconSystemInterfaceTest, FilterMatchComponentsNonElf) {
  ZirconSystemInterface system_interface;

  constexpr char kMoniker[] = "some/absolute/moniker";
  constexpr char kUrl[] = "fuchsia-pkg://fuchsia.com/component2#meta/component2.cm";

  // Inject a non-ELF component.
  fuchsia_component::EventHeader header;
  header.component_url(kUrl);
  header.moniker(kMoniker);
  header.event_type(fuchsia_component::EventType::kDebugStarted);

  fuchsia_component::Event event;
  event.header() = std::move(header);

  fuchsia_component::DebugStartedPayload payload;
  payload.runtime_dir() = std::nullopt;
  event.payload() = fuchsia_component::EventPayload::WithDebugStarted(std::move(payload));

  system_interface.zircon_component_manager().OnComponentEvent(std::move(event));

  std::vector<debug_ipc::ComponentInfo> component_info;
  std::ranges::transform(system_interface.GetComponentManager().GetNonElfComponentInfo(),
                         std::back_inserter(component_info),
                         [](const auto& pair) -> debug_ipc::ComponentInfo { return pair.second; });

  ASSERT_EQ(component_info.size(), 1u);

  debug_ipc::Filter moniker_filter;
  moniker_filter.type = debug_ipc::Filter::Type::kComponentMoniker;
  moniker_filter.pattern = kMoniker;

  EXPECT_TRUE(debug_ipc::FilterMatches(moniker_filter, "", component_info));

  debug_ipc::Filter url_filter;
  url_filter.type = debug_ipc::Filter::Type::kComponentUrl;
  url_filter.pattern = kUrl;

  EXPECT_TRUE(debug_ipc::FilterMatches(url_filter, "", component_info));
}

}  // namespace debug_agent
