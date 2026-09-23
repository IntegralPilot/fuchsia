// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BIN_DRIVER_MANAGER_TESTS_BIND_MANAGER_TEST_BASE_H_
#define SRC_DEVICES_BIN_DRIVER_MANAGER_TESTS_BIND_MANAGER_TEST_BASE_H_

#include <fidl/fuchsia.driver.index/cpp/fidl.h>

#include <queue>

#include "src/devices/bin/driver_manager/bind/bind_manager.h"
#include "src/devices/bin/driver_manager/composite/composite_node_spec.h"
#include "src/devices/bin/driver_manager/tests/driver_manager_test_base.h"

// Test class to expose protected functions.
class TestBindManager : public driver_manager::BindManager {
 public:
  TestBindManager(driver_manager::BindManagerBridge* bridge,
                  driver_manager::NodeManager* node_manager, async_dispatcher_t* dispatcher)
      : BindManager(bridge, node_manager, dispatcher) {}

  std::unordered_map<driver_manager::ResourceId, std::weak_ptr<driver_manager::Resource>>
  GetOrphanedResources() const {
    return bind_resource_set().CurrentOrphanedResources();
  }

  std::unordered_map<driver_manager::ResourceId, std::weak_ptr<driver_manager::Resource>>
  GetMultibindResources() const {
    return bind_resource_set().CurrentMultibindResources();
  }

  bool IsBindOngoing() const { return bind_resource_set().is_bind_ongoing(); }

  std::vector<driver_manager::BindRequest> GetPendingRequests() const {
    return pending_bind_requests();
  }

  const std::vector<driver_manager::NodeBindingInfoResultCallback>&
  GetPendingOrphanRebindCallbacks() const {
    return pending_orphan_rebind_callbacks();
  }
};

class TestDriverIndex final : public fidl::WireServer<fuchsia_driver_index::DriverIndex> {
 public:
  explicit TestDriverIndex(async_dispatcher_t* dispatcher) : dispatcher_(dispatcher) {}

  void MatchDriver(MatchDriverRequestView request, MatchDriverCompleter::Sync& completer) override;
  void AddCompositeNodeSpec(AddCompositeNodeSpecRequestView request,
                            AddCompositeNodeSpecCompleter::Sync& completer) override;
  void RebindCompositeNodeSpec(RebindCompositeNodeSpecRequestView request,
                               RebindCompositeNodeSpecCompleter::Sync& completer) override;
  void MatchPendingNode(MatchPendingNodeRequestView request,
                        MatchPendingNodeCompleter::Sync& completer) override;
  void SetNotifier(fuchsia_driver_index::wire::DriverIndexSetNotifierRequest* request,
                   SetNotifierCompleter::Sync& completer) override;

  fidl::ClientEnd<fuchsia_driver_index::DriverIndex> Connect();

  // Pop the next completer with the |id| in |completers_| and reply with |result|.
  void ReplyWithMatch(uint32_t id, zx::result<fuchsia_driver_index::MatchDriverResult> result);

  void VerifyRequestCount(uint32_t id, size_t expected_count);
  size_t NumOfMatchRequests() const { return match_request_count_; }

 private:
  // Maps a queue of completers to its associated node's instance ID.
  // The instance ID is extracted from the node properties. A completer is added to the queue
  // whenever MatchDriver() is called.
  std::unordered_map<uint32_t, std::queue<MatchDriverCompleter::Async>> completers_;

  size_t match_request_count_ = 0;
  async_dispatcher_t* dispatcher_;
};

class TestBindManagerBridge final : public driver_manager::BindManagerBridge,
                                    public driver_manager::CompositeManagerBridge {
 public:
  struct CompositeNodeSpecData {
    driver_manager::CompositeNodeSpec* spec;
    fuchsia_driver_framework::CompositeInfo fidl_info;
  };

  explicit TestBindManagerBridge(fidl::WireClient<fuchsia_driver_index::DriverIndex> client)
      : client_(std::move(client)), composite_manager_(this) {}
  ~TestBindManagerBridge() = default;

  // BindManagerBridge implementation:
  zx::result<driver_manager::BindSpecResult> BindToParentSpec(
      fidl::AnyArena& arena, driver_manager::CompositeParents composite_parents,
      std::weak_ptr<driver_manager::Resource> resource, bool enable_multibind) override {
    return composite_manager_.BindParentSpec(arena, composite_parents, resource, enable_multibind);
  }

  zx::result<std::string> StartDriver(
      driver_manager::Node& node, fuchsia_driver_framework::wire::DriverInfo driver_info) override {
    return zx::ok("");
  }

  // CompositeManagerBridge implementation:
  void BindNodesForCompositeNodeSpec() override { bind_manager_->TryBindAllAvailable(); }

  void AddSpecToDriverIndex(fuchsia_driver_framework::wire::CompositeNodeSpec spec,
                            driver_manager::AddToIndexCallback callback) override;

  void RequestMatchFromDriverIndex(
      fuchsia_driver_index::wire::MatchDriverArgs args,
      fit::callback<void(fidl::WireUnownedResult<fuchsia_driver_index::DriverIndex::MatchDriver>&)>
          match_callback) override {
    client_->MatchDriver(args).Then(std::move(match_callback));
  }

  void AddCompositeNodeSpec(std::string composite, std::vector<std::string> parent_names,
                            std::vector<fuchsia_driver_framework::ParentSpec2> parents,
                            std::unique_ptr<driver_manager::CompositeNodeSpec> spec);

  const std::unordered_map<std::string, CompositeNodeSpecData>& specs() const { return specs_; }

  void set_bind_manager(TestBindManager* bind_manager) { bind_manager_ = bind_manager; }

 private:
  fidl::WireClient<fuchsia_driver_index::DriverIndex> client_;

  driver_manager::CompositeNodeSpecManager composite_manager_;
  std::unordered_map<std::string, CompositeNodeSpecData> specs_;

  TestBindManager* bind_manager_;
};

class TestNodeManager : public TestNodeManagerBase {
 public:
  void Bind(driver_manager::Node& node,
            std::shared_ptr<driver_manager::BindResultTracker> result_tracker) override {
    auto self_resource = node.GetSelfResource();
    ZX_ASSERT(self_resource.has_value());
    bind_manager_->Bind(*self_resource.value(), {}, std::move(result_tracker));
  }

  void set_bind_manager(TestBindManager* bind_manager) { bind_manager_ = bind_manager; }

 private:
  TestBindManager* bind_manager_;
};

class BindManagerTestBase : public DriverManagerTestBase {
 public:
  struct BindManagerData {
    size_t driver_index_request_count;
    size_t orphan_nodes_count;
    size_t pending_bind_count;
    size_t pending_orphan_rebind_count;
  };

  void SetUp() override;
  void TearDown() override;

  driver_manager::NodeManager* GetNodeManager() override { return &node_manager_; }

  BindManagerData CurrentBindManagerData() const;
  void VerifyBindManagerData(BindManagerData expected);

  // Creates a node and adds it to orphaned nodes by invoking bind with a failed match.
  // Should only be called when there's no ongoing bind. The node should not
  // already exist.
  void AddAndOrphanNode(std::string name, bool enable_multibind = false,
                        std::shared_ptr<driver_manager::BindResultTracker> tracker = nullptr);

  // Create a node and invoke Bind() for it.
  // If EXPECT_BIND_START, the function verifies that it started a new bind process.
  // If EXPECT_QUEUED, the function verifies that it queued new bind request.
  void AddAndBindNode(std::string name, bool enable_multibind = false,
                      std::shared_ptr<driver_manager::BindResultTracker> tracker = nullptr);
  void AddAndBindNode_EXPECT_BIND_START(
      std::string name, bool enable_multibind = false,
      std::shared_ptr<driver_manager::BindResultTracker> tracker = nullptr);
  void AddAndBindNode_EXPECT_QUEUED(
      std::string name, bool enable_multibind = false,
      std::shared_ptr<driver_manager::BindResultTracker> tracker = nullptr);

  void AddCompositeNodeSpec(std::string composite, std::vector<std::string> parents);
  void AddCompositeNodeSpec_EXPECT_BIND_START(std::string composite,
                                              std::vector<std::string> parents);
  void AddCompositeNodeSpec_EXPECT_QUEUED(std::string composite, std::vector<std::string> parents);

  // Invoke Bind() for the node with the given |name|. The node should already exist.
  // If EXPECT_BIND_START, the function verifies that it started a new bind process.
  // If EXPECT_QUEUED, the function verifies that it queued new bind request.
  void InvokeBind(std::string name,
                  std::shared_ptr<driver_manager::BindResultTracker> tracker = nullptr);
  void InvokeBind_EXPECT_BIND_START(
      std::string name, std::shared_ptr<driver_manager::BindResultTracker> tracker = nullptr);
  void InvokeBind_EXPECT_QUEUED(
      std::string name, std::shared_ptr<driver_manager::BindResultTracker> tracker = nullptr);

  // Invoke TryBindAllAvailable().
  // If EXPECT_BIND_START, the function verifies that it started a new bind process.
  // If EXPECT_QUEUED, the function verifies that it queues a TryBindAllAvailable callback.
  void InvokeTryBindAllAvailable();
  void InvokeTryBindAllAvailable_EXPECT_BIND_START();
  void InvokeTryBindAllAvailable_EXPECT_QUEUED();

  // Invoke DriverIndex reply for a match request for |node|.
  void DriverIndexReplyWithDriver(std::string node);
  void DriverIndexReplyWithComposite(std::string node,
                                     std::vector<std::pair<std::string, size_t>> specs);
  void DriverIndexReplyWithNoMatch(std::string node);

  void VerifyNoOngoingBind();
  void VerifyNoQueuedBind();

  // Verifies that there's a ongoing bind process with an expected list of requests.
  // Each pair in |expected_requests| contains the node name and expected number of match requests.
  void VerifyBindOngoingWithRequests(std::vector<std::pair<std::string, size_t>> expected_requests);

  // Verify that the orphaned nodes set in BindManager contains |expected_nodes|.
  void VerifyOrphanedNodes(std::vector<std::string> expected_nodes);

  // Verify that multibind nodes set in BindManager contains |expected_nodes|.
  void VerifyMultibindNodes(std::vector<std::string> expected_nodes);

  void VerifyCompositeNodeExists(bool expected, std::string spec_name);

  void VerifyPendingBindRequestCount(size_t expected);

  TestBindManager* bind_manager() { return bind_manager_.get(); }

 protected:
  std::unordered_map<std::string, std::shared_ptr<driver_manager::Node>> nodes() const {
    return nodes_;
  }
  std::unordered_map<std::string, uint32_t> instance_ids() const { return instance_ids_; }

 private:
  std::shared_ptr<driver_manager::Node> CreateNode(const std::string name, bool enable_multibind);

  // Gets the instance ID for |node_name| from |instance_ids_|. Adds a new entry with a
  // unique instance ID if it's missing.
  uint32_t GetOrAddInstanceId(std::string node_name);

  std::unique_ptr<TestDriverIndex> driver_index_;
  std::unique_ptr<TestBindManagerBridge> bridge_;
  TestNodeManager node_manager_;
  std::unique_ptr<TestBindManager> bind_manager_;

  std::unordered_map<std::string, std::shared_ptr<driver_manager::Node>> nodes_;

  // Maps each node to a unique instance id. The instance id is used to the node's
  // property for binding.
  std::unordered_map<std::string, uint32_t> instance_ids_;

  fidl::Arena<> arena_;
};

#endif  // SRC_DEVICES_BIN_DRIVER_MANAGER_TESTS_BIND_MANAGER_TEST_BASE_H_
