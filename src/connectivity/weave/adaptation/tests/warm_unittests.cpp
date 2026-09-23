// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fuchsia/net/cpp/fidl.h>
#include <fuchsia/net/interfaces/admin/cpp/fidl_test_base.h>
#include <fuchsia/net/interfaces/cpp/fidl_test_base.h>
#include <fuchsia/net/root/cpp/fidl_test_base.h>
#include <fuchsia/net/stack/cpp/fidl_test_base.h>
#include <lib/fidl/cpp/binding_set.h>
#include <lib/fit/function.h>
#include <lib/sys/cpp/testing/component_context_provider.h>
#include <lib/syslog/cpp/macros.h>

#include <algorithm>
#include <functional>
#include <memory>
#include <vector>

#include <gtest/gtest.h>

// clang-format off
#pragma GCC diagnostic push
#include <Weave/DeviceLayer/internal/WeaveDeviceLayerInternal.h>
#include <Weave/DeviceLayer/ConnectivityManager.h>
#include <Weave/DeviceLayer/ThreadStackManager.h>
#include <Warm/Warm.h>
#pragma GCC diagnostic pop
// clang-format on

#include "test_configuration_manager.h"
#include "test_connectivity_manager.h"
#include "test_thread_stack_manager.h"
#include "weave_test_fixture.h"

namespace nl {
namespace Weave {
namespace Warm {
namespace Platform {
namespace testing {

namespace {
using weave::adaptation::testing::TestConfigurationManager;
using weave::adaptation::testing::TestConnectivityManager;
using weave::adaptation::testing::TestThreadStackManager;

using DeviceLayer::ConfigurationMgrImpl;
using DeviceLayer::ConnectivityMgrImpl;
using DeviceLayer::PlatformMgrImpl;
using DeviceLayer::ThreadStackMgrImpl;
using DeviceLayer::Internal::testing::WeaveTestFixture;

constexpr char kTunInterfaceName[] = "weav-tun0";

constexpr uint32_t kRouteMetric_HighPriority = 0;
constexpr uint32_t kRouteMetric_LowPriority = 999;

// Comparison function to check if two instances of fuchsia::net::IpAddress
// match their address space.
bool CompareIpAddress(const ::fuchsia::net::IpAddress& right,
                      const ::fuchsia::net::IpAddress& left) {
  return std::memcmp(right.ipv6().addr.data(), left.ipv6().addr.data(), right.ipv6().addr.size()) ==
         0;
}

// Comparison function to compare Weave's Inet::IPAddress with Fuchsia's
// fuchsia::net::IpAddress match in their address space..
bool CompareIpAddress(const ::nl::Inet::IPAddress& right, const ::fuchsia::net::IpAddress& left) {
  fuchsia::net::Ipv6Address v6;
  std::memcpy(v6.addr.data(), right.Addr, v6.addr.size());
  fuchsia::net::IpAddress right_v6;
  right_v6.set_ipv6(v6);
  return CompareIpAddress(right_v6, left);
}

}  // namespace

// Forward declare the Fake FIDL impls needed by `OwnedAddress`/`OwnedInterface`
class FakeAddressStateProvider;
class FakeControl;

class TestNotifier {
 public:
  void Notify() {
    std::unique_lock lock(mu_);
    was_notified_ = true;
    cv_.notify_one();
  }
  // Returns True if the wait completed without timing out.
  bool WaitWithTimeout(std::chrono::duration<uint64_t> timeout) {
    std::unique_lock lock(mu_);
    if (was_notified_) {
      return true;
    } else {
      return cv_.wait_for(lock, timeout) != std::cv_status::timeout;
    }
  }

 private:
  bool was_notified_ = false;
  std::mutex mu_;
  std::condition_variable cv_;
};

struct OwnedAddress {
  fuchsia::net::Subnet address;
  std::unique_ptr<FakeAddressStateProvider> fake_asp;
  std::shared_ptr<TestNotifier> on_hangup_notifier;
};

struct OwnedInterface {
  uint64_t id;
  std::string name;
  std::shared_ptr<std::vector<OwnedAddress>> ipv6addrs;
  std::unique_ptr<FakeControl> fake_control;
  std::shared_ptr<bool> forwarding_enabled;
};

// A fake implementation of the
// `fuchsia.net.interfaces.admin/AddressStateProvider` protocol for a single
// address.
class FakeAddressStateProvider
    : public fuchsia::net::interfaces::admin::testing::AddressStateProvider_TestBase {
 public:
  FakeAddressStateProvider() = delete;
  FakeAddressStateProvider(
      const fuchsia::net::Subnet& address, std::shared_ptr<std::vector<OwnedAddress>> addresses,
      std::shared_ptr<TestNotifier> on_hangup_notifier, async_dispatcher_t* dispatcher,
      fidl::InterfaceRequest<fuchsia::net::interfaces::admin::AddressStateProvider> request,
      std::optional<zx_status_t> add_fails_with_err) {
    address.Clone(&address_);
    addresses_ = addresses;
    on_hangup_notifier_ = on_hangup_notifier;
    add_fails_with_err_ = add_fails_with_err;

    binding_.Bind(std::move(request), dispatcher);
    binding_.set_error_handler([this](zx_status_t error) { OnHangUp(error); });
  }

  void SendOnAddressRemoved(fuchsia::net::interfaces::admin::AddressRemovalReason reason) {
    binding_.events().OnAddressRemoved(reason);
  }

  void SendOnAddressAdded() { binding_.events().OnAddressAdded(); }

 private:
  // Default implementation for any API method not explicitly overridden.
  void NotImplemented_(const std::string& name) override { FAIL() << "Not implemented: " << name; }

  void WatchAddressAssignmentState(WatchAddressAssignmentStateCallback callback) override {
    if (add_fails_with_err_.has_value()) {
      // Send the OnAddressAdded event prior the `OnAddressRemoved`, so that the
      // `AddressStateProviderEventHandler` has to handle two events before
      // knowing the removal reason. This is a regression test for
      // https://fxbug.dev/42085834.
      SendOnAddressAdded();
      SendOnAddressRemoved(
          fuchsia::net::interfaces::admin::AddressRemovalReason::INTERFACE_REMOVED);
      binding_.Close(add_fails_with_err_.value());
      OnHangUp(add_fails_with_err_.value());
    } else {
      callback(fuchsia::net::interfaces::AddressAssignmentState::ASSIGNED);
    }
  }

  // Callback to remove the address when the client hangs up.
  void OnHangUp(zx_status_t error) {
    // When removing the `OwnedAddress` from `addresses_` it's important not to
    // drop its `fake_asp`, as that is a unique pointer to this class. `self`
    // allows us to hold onto this class and prevent the destructor from running
    // until after this function exits.
    std::unique_ptr<FakeAddressStateProvider> self;
    auto it = std::remove_if(addresses_->begin(), addresses_->end(), [&](OwnedAddress& addr) {
      bool found = CompareIpAddress(addr.address.addr, address_.addr);
      if (found) {
        self.swap(addr.fake_asp);
      }
      return found;
    });
    if (it != addresses_->end()) {
      addresses_->erase(it, addresses_->end());
    }
    on_hangup_notifier_->Notify();
  }

  // When `nullopt`, adding the address will succeed. Otherwise, adding the
  // address will fail and the underlying channel will be closed with the
  // provided error.
  std::optional<zx_status_t> add_fails_with_err_;
  fuchsia::net::Subnet address_;
  std::shared_ptr<std::vector<OwnedAddress>> addresses_;
  std::shared_ptr<TestNotifier> on_hangup_notifier_;
  fidl::Binding<fuchsia::net::interfaces::admin::AddressStateProvider> binding_{this};
};

// A fake implementation of the `fuchsia.net.interfaces.admin/Control` protocol
// for a single interface.
class FakeControl : public fuchsia::net::interfaces::admin::testing::Control_TestBase {
 public:
  FakeControl() = delete;
  FakeControl(std::shared_ptr<std::vector<OwnedAddress>> addresses,
              std::shared_ptr<bool> forwarding_enabled, async_dispatcher_t* dispatcher,
              fidl::InterfaceRequest<fuchsia::net::interfaces::admin::Control> request,
              std::optional<zx_status_t> add_addresses_fail_with_err) {
    addresses_ = addresses;
    forwarding_enabled_ = forwarding_enabled;
    // Hang on to the dispatcher for later; which will allow us to spawn
    // `FakeAddressStateProvider` handlers when serving `AddAddress`.
    dispatcher_ = dispatcher;
    binding_.Bind(std::move(request), dispatcher);
    add_addresses_fail_with_err_ = add_addresses_fail_with_err;
  }

  ~FakeControl() {
    // Interface removal triggers address removal. Note that each `OwnedAddress`
    // inside of `addresses_` has a shared_ptr to `addresses_`. If `addresses_`
    // is non-empty, there would be pointer cycles leading to a memory leak.
    addresses_->clear();
  }

 private:
  // Default implementation for any API method not explicitly overridden.
  void NotImplemented_(const std::string& name) override { FAIL() << "Not implemented: " << name; }

  void AddAddress(fuchsia::net::Subnet address,
                  fuchsia::net::interfaces::admin::AddressParameters parameters,
                  fidl::InterfaceRequest<::fuchsia::net::interfaces::admin::AddressStateProvider>
                      server_end) override {
    // Confirm that the configured address is a V6 address.
    ASSERT_TRUE(address.addr.is_ipv6());

    std::shared_ptr<TestNotifier> on_hangup_notifier = std::make_shared<TestNotifier>();
    std::unique_ptr<FakeAddressStateProvider> fake_asp = std::make_unique<FakeAddressStateProvider>(
        address, addresses_, on_hangup_notifier, dispatcher_, std::move(server_end),
        add_addresses_fail_with_err_);

    // Verify that the address does not already exist.
    auto it = std::find_if(addresses_->begin(), addresses_->end(), [&](const OwnedAddress& addr) {
      return CompareIpAddress(addr.address.addr, address.addr);
    });
    if (it != addresses_->end()) {
      fake_asp->SendOnAddressRemoved(
          fuchsia::net::interfaces::admin::AddressRemovalReason::ALREADY_ASSIGNED);
      return;
    }
    addresses_->push_back({.address = std::move(address),
                           .fake_asp = std::move(fake_asp),
                           .on_hangup_notifier = on_hangup_notifier});
  }

  void SetConfiguration(::fuchsia::net::interfaces::admin::Configuration config,
                        SetConfigurationCallback callback) override {
    // The only config change made by Warm is to enable IPv6 forwarding.
    EXPECT_FALSE(config.has_ipv4());
    ASSERT_TRUE(config.has_ipv6());
    ASSERT_TRUE(config.ipv6().has_unicast_forwarding());
    ASSERT_TRUE(config.ipv6().unicast_forwarding());
    *forwarding_enabled_ = true;
    auto result = ::fuchsia::net::interfaces::admin::Control_SetConfiguration_Result();
    auto response = ::fuchsia::net::interfaces::admin::Control_SetConfiguration_Response();
    result.set_response(std::move(response));
    callback(std::move(result));
  }

  fidl::Binding<fuchsia::net::interfaces::admin::Control> binding_{this};
  std::shared_ptr<std::vector<OwnedAddress>> addresses_;
  std::shared_ptr<bool> forwarding_enabled_;
  std::optional<zx_status_t> add_addresses_fail_with_err_;
  async_dispatcher_t* dispatcher_;
};

class FakeNetInterfaces : public fuchsia::net::interfaces::testing::State_TestBase,
                          public fuchsia::net::interfaces::testing::Watcher_TestBase {
 public:
  void InitializeInterfaces(const std::vector<OwnedInterface>& interfaces) {
    existing_events_.clear();
    for (const auto& interface : interfaces) {
      AddExistingInterface(interface);
    }

    fuchsia::net::interfaces::Empty idle_event;
    fuchsia::net::interfaces::Event event =
        fuchsia::net::interfaces::Event::WithIdle(std::move(idle_event));
    existing_events_.push_back(std::move(event));
  }

  void NotImplemented_(const std::string& name) override { FAIL() << "Not implemented: " << name; }

  fidl::InterfaceRequestHandler<fuchsia::net::interfaces::State> GetHandler(
      async_dispatcher_t* dispatcher) {
    dispatcher_ = dispatcher;
    return [this](fidl::InterfaceRequest<fuchsia::net::interfaces::State> request) {
      state_binding_.Bind(std::move(request), dispatcher_);
    };
  }

  void GetWatcher(fuchsia::net::interfaces::WatcherOptions options,
                  fidl::InterfaceRequest<fuchsia::net::interfaces::Watcher> watcher) override {
    events_.clear();
    for (auto& existing_event : existing_events_) {
      fuchsia::net::interfaces::Event event;
      existing_event.Clone(&event);
      events_.push_back(std::move(event));
    }
    watcher_binding_.Bind(std::move(watcher), dispatcher_);
  }

  void Watch(fuchsia::net::interfaces::Watcher::WatchCallback callback) override {
    watch_callback_ = std::move(callback);
    SendPendingEvent();
  }

  void SendPendingEvent() {
    if (events_.empty() || !watch_callback_) {
      return;
    }
    fuchsia::net::interfaces::Event event(std::move(events_.front()));
    events_.pop_front();
    watch_callback_(std::move(event));
    watch_callback_ = nullptr;
  }

  void Close(zx_status_t epitaph_value = ZX_OK) {
    watcher_binding_.Close(epitaph_value);
    state_binding_.Close(epitaph_value);
  }

 private:
  void AddExistingInterface(const OwnedInterface& interface) {
    fuchsia::net::interfaces::Event event;
    fuchsia::net::interfaces::Properties properties;
    properties.set_id(interface.id);
    properties.set_name(interface.name);
    properties.set_has_default_ipv4_route(true);
    properties.set_has_default_ipv6_route(true);
    event = fuchsia::net::interfaces::Event::WithExisting(std::move(properties));
    existing_events_.push_back(std::move(event));
  }

  async_dispatcher_t* dispatcher_;
  fuchsia::net::interfaces::Watcher::WatchCallback watch_callback_;
  std::deque<fuchsia::net::interfaces::Event> events_;
  std::vector<fuchsia::net::interfaces::Event> existing_events_;
  fidl::Binding<fuchsia::net::interfaces::State> state_binding_{this};
  fidl::Binding<fuchsia::net::interfaces::Watcher> watcher_binding_{this};
};

// The minimal set of fuchsia networking protocols required for WARM to run.
class FakeNetstack : public fuchsia::net::root::testing::Interfaces_TestBase,
                     public fuchsia::net::stack::testing::Stack_TestBase {
 private:
  // Default implementation for any API method not explicitly overridden.
  void NotImplemented_(const std::string& name) override { FAIL() << "Not implemented: " << name; }

  // TODO(https://fxbug.dev/42062982) Delete this once Weavestack no longer relies
  // on the root API.
  void GetAdmin(
      uint64_t id,
      fidl::InterfaceRequest<::fuchsia::net::interfaces::admin::Control> server_end) override {
    auto it = std::find_if(interfaces_.begin(), interfaces_.end(),
                           [&](const OwnedInterface& interface) { return id == interface.id; });
    if (it == interfaces_.end()) {
      server_end.Close(ZX_ERR_NOT_FOUND);
    } else {
      it->fake_control =
          std::make_unique<FakeControl>(it->ipv6addrs, it->forwarding_enabled, dispatcher_,
                                        std::move(server_end), add_addresses_fail_with_err_);
    }
  }

  void AddForwardingEntry(fuchsia::net::stack::ForwardingEntry route_table_entry,
                          AddForwardingEntryCallback callback) override {
    route_table_.push_back(std::move(route_table_entry));
    callback(fuchsia::net::stack::Stack_AddForwardingEntry_Result::WithResponse({}));
  }

  void DelForwardingEntry(fuchsia::net::stack::ForwardingEntry route_table_entry,
                          DelForwardingEntryCallback callback) override {
    auto it =
        std::remove_if(route_table_.begin(), route_table_.end(),
                       [&](const fuchsia::net::stack::ForwardingEntry& entry) {
                         return entry.device_id == route_table_entry.device_id &&
                                entry.metric == route_table_entry.metric &&
                                entry.subnet.prefix_len == route_table_entry.subnet.prefix_len &&
                                CompareIpAddress(entry.subnet.addr, route_table_entry.subnet.addr);
                       });
    if (it == route_table_.end()) {
      callback(fuchsia::net::stack::Stack_DelForwardingEntry_Result::WithErr(
          fuchsia::net::stack::Error::NOT_FOUND));
      return;
    }
    route_table_.erase(it, route_table_.end());
    callback(fuchsia::net::stack::Stack_DelForwardingEntry_Result::WithResponse({}));
  }

 public:
  // Mutators, accessors, and helpers for tests.

  // Add a fake interface with the given name. Does not check for duplicates.
  FakeNetstack& AddOwnedInterface(std::string name) {
    interfaces_.push_back({
        .id = ++last_id_assigned,
        .name = name,
        .ipv6addrs = std::make_shared<std::vector<OwnedAddress>>(),
        .forwarding_enabled = std::make_shared<bool>(false),
    });

    // The real Weavestack installs the Tun Interface, and provides an accessor
    // to the Control handle via the Connectivity Manager.
    if (name == kTunInterfaceName) {
      GetAdmin(last_id_assigned,
               ConnectivityMgrImpl().GetTunInterfaceControlSyncPtr()->NewRequest());
    }

    return *this;
  }

  // Remove the fake interface with the given name. If it is not present, no change occurs.
  FakeNetstack& RemoveOwnedInterface(std::string name) {
    auto it =
        std::remove_if(interfaces_.begin(), interfaces_.end(),
                       [&](const OwnedInterface& interface) { return interface.name == name; });
    interfaces_.erase(it, interfaces_.end());

    // Synchronize the Connectivity Manager, which holds a Control handle for
    // the Tun interface.
    if (name == kTunInterfaceName) {
      ConnectivityMgrImpl().GetTunInterfaceControlSyncPtr()->Unbind();
    }

    return *this;
  }

  // Inject the given failure when adding addresses.
  void InjectAddAddressFailures(zx_status_t error) {
    add_addresses_fail_with_err_ = std::make_optional(error);
  }

  // Access the current interfaces.
  const std::vector<OwnedInterface>& interfaces() const { return interfaces_; }

  // Access the current route table.
  const std::vector<fuchsia::net::stack::ForwardingEntry>& route_table() const {
    return route_table_;
  }

  // Get a pointer to an interface by name.
  OwnedInterface& GetInterfaceByName(const std::string name) {
    auto it = std::find_if(interfaces_.begin(), interfaces_.end(),
                           [&](const OwnedInterface& interface) { return interface.name == name; });
    ZX_DEBUG_ASSERT(it != interfaces_.end());
    return *it;
  }

  // Get a pointer to an interface by ID.
  OwnedInterface& GetInterfaceById(const uint64_t id) {
    auto it = std::find_if(interfaces_.begin(), interfaces_.end(),
                           [&](const OwnedInterface& interface) { return interface.id == id; });
    ZX_DEBUG_ASSERT(it != interfaces_.end());
    return *it;
  }

  // Check if interface is forwarded.
  bool IsInterfaceForwarded(uint64_t id) { return *GetInterfaceById(id).forwarding_enabled; }

  fidl::InterfaceRequestHandler<fuchsia::net::stack::Stack> GetStackHandler(
      async_dispatcher_t* dispatcher) {
    dispatcher_ = dispatcher;
    return [this](fidl::InterfaceRequest<fuchsia::net::stack::Stack> request) {
      stack_binding_.Bind(std::move(request), dispatcher_);
    };
  }

  // Check if the given interface ID and address exists in the route table.
  bool FindRouteTableEntry(uint32_t nicid, ::nl::Inet::IPAddress addr,
                           uint32_t metric = kRouteMetric_HighPriority) {
    auto it = std::find_if(route_table_.begin(), route_table_.end(),
                           [&](const fuchsia::net::stack::ForwardingEntry& route_table_entry) {
                             return nicid == route_table_entry.device_id &&
                                    metric == route_table_entry.metric &&
                                    CompareIpAddress(addr, route_table_entry.subnet.addr);
                           });

    return it != route_table_.end();
  }

  // TODO(https://fxbug.dev/42062982) Delete this once Weavestack no longer relies
  // on the root API.
  fidl::InterfaceRequestHandler<fuchsia::net::root::Interfaces> GetRootHandler(
      async_dispatcher_t* dispatcher) {
    dispatcher_ = dispatcher;
    return [this](fidl::InterfaceRequest<fuchsia::net::root::Interfaces> request) {
      root_binding_.Bind(std::move(request), dispatcher_);
    };
  }

 private:
  // TODO(https://fxbug.dev/42062982) Delete this once Weavestack no longer relies
  // on the root API.
  fidl::Binding<fuchsia::net::root::Interfaces> root_binding_{this};
  fidl::Binding<fuchsia::net::stack::Stack> stack_binding_{this};
  async_dispatcher_t* dispatcher_;
  std::vector<fuchsia::net::stack::ForwardingEntry> route_table_;
  std::vector<OwnedInterface> interfaces_;
  uint32_t last_id_assigned = 0;
  std::optional<zx_status_t> add_addresses_fail_with_err_;
};

class WarmTest : public testing::WeaveTestFixture<> {
 public:
  void SetUp() override {
    WeaveTestFixture<>::SetUp();

    // Initialize everything needed for the test.
    context_provider_.service_directory_provider()->AddService(
        fake_net_interfaces_.GetHandler(dispatcher()));
    context_provider_.service_directory_provider()->AddService(
        fake_net_stack_.GetStackHandler(dispatcher()));
    // TODO(https://fxbug.dev/42062982) Delete this once Weavestack no longer
    // relies on the root API.
    context_provider_.service_directory_provider()->AddService(
        fake_net_stack_.GetRootHandler(dispatcher()), "fuchsia.net.root.Interfaces");

    PlatformMgrImpl().SetComponentContextForProcess(context_provider_.TakeContext());
    ConfigurationMgrImpl().SetDelegate(std::make_unique<TestConfigurationManager>());
    ConnectivityMgrImpl().SetDelegate(std::make_unique<TestConnectivityManager>());
    ThreadStackMgrImpl().SetDelegate(std::make_unique<TestThreadStackManager>());
    Warm::Platform::Init(nullptr);

    // Report that thread is provisioned by default, so that WARM does not
    // always reject add-operations due to lack of provisioning.
    thread_delegate().set_is_thread_provisioned(true);

    // Populate initial fake interfaces
    AddOwnedInterface(kTunInterfaceName);
    AddOwnedInterface(TestThreadStackManager::kThreadInterfaceName);
    AddOwnedInterface(TestConnectivityManager::kWiFiInterfaceName);

    RunFixtureLoop();
  }

  void TearDown() override {
    StopFixtureLoop();
    ConfigurationMgrImpl().SetDelegate(nullptr);
    ConnectivityMgrImpl().SetDelegate(nullptr);
    ThreadStackMgrImpl().SetDelegate(nullptr);
    WeaveTestFixture<>::TearDown();
  }

 protected:
  FakeNetInterfaces& fake_net_interfaces() { return fake_net_interfaces_; }
  FakeNetstack& fake_net_stack() { return fake_net_stack_; }

  TestThreadStackManager& thread_delegate() {
    return *reinterpret_cast<TestThreadStackManager*>(ThreadStackMgrImpl().GetDelegate());
  }

  void AddOwnedInterface(std::string name) {
    fake_net_stack_.AddOwnedInterface(name);
    fake_net_interfaces_.InitializeInterfaces(fake_net_stack_.interfaces());
  }

  void RemoveOwnedInterface(std::string name) {
    fake_net_stack_.RemoveOwnedInterface(name);
    fake_net_interfaces_.InitializeInterfaces(fake_net_stack_.interfaces());
  }

  OwnedInterface& GetTunnelInterface() {
    return fake_net_stack_.GetInterfaceByName(kTunInterfaceName);
  }

  uint32_t GetTunnelInterfaceId() { return GetTunnelInterface().id; }

  OwnedInterface& GetWiFiInterface() {
    return fake_net_stack_.GetInterfaceByName(TestConnectivityManager::kWiFiInterfaceName);
  }

  uint32_t GetWiFiInterfaceId() { return GetWiFiInterface().id; }

 private:
  FakeNetstack fake_net_stack_;
  FakeNetInterfaces fake_net_interfaces_;
  sys::testing::ComponentContextProvider context_provider_;
};

TEST_F(WarmTest, AddRemoveAddressTunnel) {
  constexpr char kSubnetIp[] = "2001:0DB8:0042::";
  constexpr uint8_t kPrefixLength = 48;
  Inet::IPAddress addr;

  // Sanity check - no addresses assigned.
  OwnedInterface& weave_tun = GetTunnelInterface();
  EXPECT_EQ(weave_tun.ipv6addrs->size(), 0u);

  // Attempt to add the address.
  ASSERT_TRUE(Inet::IPAddress::FromString(kSubnetIp, addr));
  auto result = AddRemoveHostAddress(kInterfaceTypeTunnel, addr, kPrefixLength, /*add*/ true);
  EXPECT_EQ(result, kPlatformResultSuccess);

  // Confirm that it worked.
  ASSERT_EQ(weave_tun.ipv6addrs->size(), 1u);
  EXPECT_TRUE(CompareIpAddress(addr, (*weave_tun.ipv6addrs)[0].address.addr));

  // Attempt to remove the address.
  std::shared_ptr<TestNotifier> on_hangup_notifier = (*weave_tun.ipv6addrs)[0].on_hangup_notifier;
  result = AddRemoveHostAddress(kInterfaceTypeTunnel, addr, kPrefixLength, /*add*/ false);
  EXPECT_EQ(result, kPlatformResultSuccess);

  // Confirm that it worked.
  EXPECT_TRUE(on_hangup_notifier->WaitWithTimeout(std::chrono::seconds(1)));
  EXPECT_EQ(weave_tun.ipv6addrs->size(), 0u);
}

TEST_F(WarmTest, AddRemoveAddressWiFi) {
  constexpr char kSubnetIp[] = "2001:0DB8:0042::";
  constexpr uint8_t kPrefixLength = 48;
  Inet::IPAddress addr;

  // Sanity check - no addresses assigned.
  OwnedInterface& wlan = GetWiFiInterface();
  EXPECT_EQ(wlan.ipv6addrs->size(), 0u);

  // Attempt to add the address.
  ASSERT_TRUE(Inet::IPAddress::FromString(kSubnetIp, addr));
  auto result = AddRemoveHostAddress(kInterfaceTypeWiFi, addr, kPrefixLength, /*add*/ true);
  EXPECT_EQ(result, kPlatformResultSuccess);

  // Confirm that it worked.
  ASSERT_EQ(wlan.ipv6addrs->size(), 1u);
  EXPECT_TRUE(CompareIpAddress(addr, (*wlan.ipv6addrs)[0].address.addr));

  // Attempt to remove the address.
  std::shared_ptr<TestNotifier> on_hangup_notifier = (*wlan.ipv6addrs)[0].on_hangup_notifier;
  result = AddRemoveHostAddress(kInterfaceTypeWiFi, addr, kPrefixLength, /*add*/ false);
  EXPECT_EQ(result, kPlatformResultSuccess);

  // Confirm that it worked.
  EXPECT_TRUE(on_hangup_notifier->WaitWithTimeout(std::chrono::seconds(1)));
  EXPECT_EQ(wlan.ipv6addrs->size(), 0u);
}

// Verify Weavestack gracefully handles a Netstack disconnection while waiting
// for an address to become assigned.
TEST_F(WarmTest, AddAddressPeerClosed) {
  constexpr char kSubnetIp[] = "2001:0DB8:0042::";
  constexpr uint8_t kPrefixLength = 48;
  Inet::IPAddress addr;

  // Sanity check - no addresses assigned.
  OwnedInterface& wlan = GetWiFiInterface();
  EXPECT_EQ(wlan.ipv6addrs->size(), 0u);

  // Configure the Netstack to fail to add addresses.
  fake_net_stack().InjectAddAddressFailures(ZX_ERR_PEER_CLOSED);

  // Attempt to add the address.
  ASSERT_TRUE(Inet::IPAddress::FromString(kSubnetIp, addr));
  auto result = AddRemoveHostAddress(kInterfaceTypeWiFi, addr, kPrefixLength, /*add*/ true);
  EXPECT_EQ(result, kPlatformResultFailure);
}

TEST_F(WarmTest, AddRemoveSameAddress) {
  constexpr char kSubnetIp[] = "2001:0DB8:0042::";
  constexpr uint8_t kPrefixLength = 48;
  Inet::IPAddress addr;

  // Sanity check - no addresses assigned.
  OwnedInterface& wlan = GetWiFiInterface();
  EXPECT_EQ(wlan.ipv6addrs->size(), 0u);

  // Attempt to add the address.
  ASSERT_TRUE(Inet::IPAddress::FromString(kSubnetIp, addr));
  auto result = AddRemoveHostAddress(kInterfaceTypeWiFi, addr, kPrefixLength, /*add*/ true);
  EXPECT_EQ(result, kPlatformResultSuccess);

  // Attempt to add the address again, which should silently ignore the request.
  ASSERT_TRUE(Inet::IPAddress::FromString(kSubnetIp, addr));
  result = AddRemoveHostAddress(kInterfaceTypeWiFi, addr, kPrefixLength, /*add*/ true);
  EXPECT_EQ(result, kPlatformResultSuccess);

  // Confirm that it worked and only added a single address.
  ASSERT_EQ(wlan.ipv6addrs->size(), 1u);
  EXPECT_TRUE(CompareIpAddress(addr, (*wlan.ipv6addrs)[0].address.addr));

  // Attempt to remove the address.
  std::shared_ptr<TestNotifier> on_hangup_notifier = (*wlan.ipv6addrs)[0].on_hangup_notifier;
  result = AddRemoveHostAddress(kInterfaceTypeWiFi, addr, kPrefixLength, /*add*/ false);
  EXPECT_EQ(result, kPlatformResultSuccess);

  // Attempt to remove the address again, which should silently ignore the
  // request. An already removed address results in UNKNOWN_INTERFACE.
  result = AddRemoveHostAddress(kInterfaceTypeWiFi, addr, kPrefixLength, /*add*/ false);
  EXPECT_EQ(result, kPlatformResultSuccess);

  // Confirm that it worked.
  EXPECT_TRUE(on_hangup_notifier->WaitWithTimeout(std::chrono::seconds(1)));
  EXPECT_EQ(wlan.ipv6addrs->size(), 0u);
}

TEST_F(WarmTest, RemoveAddressTunnelNotFound) {
  constexpr char kSubnetIp[] = "2001:0DB8:0042::";
  constexpr uint8_t kPrefixLength = 48;
  Inet::IPAddress addr;

  // Sanity check - no addresses assigned.
  OwnedInterface& weave_tun = GetTunnelInterface();
  EXPECT_EQ(weave_tun.ipv6addrs->size(), 0u);

  // Attempt to remove the address, expecting success - if the interface isn't
  // available, assume it's removed. WARM may invoke us after the interface is
  // down. This is distinct from the 'add' case, where it represents a failure.
  ASSERT_TRUE(Inet::IPAddress::FromString(kSubnetIp, addr));
  auto result = AddRemoveHostAddress(kInterfaceTypeTunnel, addr, kPrefixLength, /*add*/ false);
  EXPECT_EQ(result, kPlatformResultSuccess);

  // Sanity check - still no addresses assigned.
  EXPECT_EQ(weave_tun.ipv6addrs->size(), 0u);
}

TEST_F(WarmTest, RemoveAddressWiFiNotFound) {
  constexpr char kSubnetIp[] = "2001:0DB8:0042::";
  constexpr uint8_t kPrefixLength = 48;
  Inet::IPAddress addr;

  // Sanity check - no addresses assigned.
  OwnedInterface& wlan = GetWiFiInterface();
  EXPECT_EQ(wlan.ipv6addrs->size(), 0u);

  // Attempt to remove the address, expecting success - if the interface isn't
  // available, assume it's removed. WARM may invoke us after the interface is
  // down. This is distinct from the 'add' case, where it represents a failure.
  ASSERT_TRUE(Inet::IPAddress::FromString(kSubnetIp, addr));
  auto result = AddRemoveHostAddress(kInterfaceTypeWiFi, addr, kPrefixLength, /*add*/ false);
  EXPECT_EQ(result, kPlatformResultSuccess);

  // Sanity check - still no addresses assigned.
  EXPECT_EQ(wlan.ipv6addrs->size(), 0u);
}

TEST_F(WarmTest, AddAddressTunnelNoInterface) {
  constexpr char kSubnetIp[] = "2001:0DB8:0042::";
  constexpr uint8_t kPrefixLength = 48;
  Inet::IPAddress addr;

  RemoveOwnedInterface(kTunInterfaceName);

  // Attempt to add to the interface when there's no Tunnel interface. Expect failure.
  ASSERT_TRUE(Inet::IPAddress::FromString(kSubnetIp, addr));
  auto result = AddRemoveHostAddress(kInterfaceTypeTunnel, addr, kPrefixLength, /*add*/ true);
  EXPECT_EQ(result, kPlatformResultFailure);
}

TEST_F(WarmTest, RemoveAddressTunnelNoInterface) {
  constexpr char kSubnetIp[] = "2001:0DB8:0042::";
  constexpr uint8_t kPrefixLength = 48;
  Inet::IPAddress addr;

  RemoveOwnedInterface(kTunInterfaceName);

  // Attempt to remove from the interface when there's no Tunnel interface. Expect success.
  ASSERT_TRUE(Inet::IPAddress::FromString(kSubnetIp, addr));
  auto result = AddRemoveHostAddress(kInterfaceTypeTunnel, addr, kPrefixLength, /*add*/ false);
  EXPECT_EQ(result, kPlatformResultSuccess);
}

TEST_F(WarmTest, AddAddressWiFiNoInterface) {
  constexpr char kSubnetIp[] = "2001:0DB8:0042::";
  constexpr uint8_t kPrefixLength = 48;
  Inet::IPAddress addr;

  RemoveOwnedInterface(TestConnectivityManager::kWiFiInterfaceName);

  // Attempt to add to the interface when there's no WiFi interface. Expect failure.
  ASSERT_TRUE(Inet::IPAddress::FromString(kSubnetIp, addr));
  auto result = AddRemoveHostAddress(kInterfaceTypeWiFi, addr, kPrefixLength, /*add*/ true);
  EXPECT_EQ(result, kPlatformResultFailure);
}

TEST_F(WarmTest, RemoveAddressWiFiNoInterface) {
  constexpr char kSubnetIp[] = "2001:0DB8:0042::";
  constexpr uint8_t kPrefixLength = 48;
  Inet::IPAddress addr;

  RemoveOwnedInterface(TestConnectivityManager::kWiFiInterfaceName);

  // Attempt to remove from the interface when there's no WiFi interface. Expect success.
  ASSERT_TRUE(Inet::IPAddress::FromString(kSubnetIp, addr));
  auto result = AddRemoveHostAddress(kInterfaceTypeWiFi, addr, kPrefixLength, /*add*/ false);
  EXPECT_EQ(result, kPlatformResultSuccess);
}

TEST_F(WarmTest, CheckInterfaceNotForwarding) {
  constexpr char kSubnetIp[] = "2001:0DB8:0042::";
  constexpr uint8_t kPrefixLength = 48;
  Inet::IPPrefix prefix;

  ASSERT_TRUE(Inet::IPAddress::FromString(kSubnetIp, prefix.IPAddr));
  prefix.Length = kPrefixLength;

  // Sanity check - confirm no routes to the Tunnel interface exist.
  uint64_t tunnel_iface_id = GetTunnelInterfaceId();
  ASSERT_NE(tunnel_iface_id, 0u);
  EXPECT_FALSE(fake_net_stack().FindRouteTableEntry(tunnel_iface_id, prefix.IPAddr));

  auto delegate = reinterpret_cast<TestConfigurationManager*>(ConfigurationMgrImpl().GetDelegate());
  delegate->set_is_ipv6_forwarding_enabled(false);

  // Attempt to add a route to the Tunnel interface.
  auto result = AddRemoveHostRoute(kInterfaceTypeTunnel, prefix, kRoutePriorityHigh, /*add*/ true);
  EXPECT_EQ(result, kPlatformResultSuccess);

  // Confirm that this interface is not forwarded, consistent with configuration.
  EXPECT_FALSE(fake_net_stack().IsInterfaceForwarded(tunnel_iface_id));
}

TEST_F(WarmTest, CheckInterfaceForwarding) {
  constexpr char kSubnetIp[] = "2001:0DB8:0042::";
  constexpr uint8_t kPrefixLength = 48;
  Inet::IPPrefix prefix;

  ASSERT_TRUE(Inet::IPAddress::FromString(kSubnetIp, prefix.IPAddr));
  prefix.Length = kPrefixLength;

  // Sanity check - confirm no routes to the Tunnel interface exist.
  uint64_t tunnel_iface_id = GetTunnelInterfaceId();
  ASSERT_NE(tunnel_iface_id, 0u);
  EXPECT_FALSE(fake_net_stack().FindRouteTableEntry(tunnel_iface_id, prefix.IPAddr));

  auto delegate = reinterpret_cast<TestConfigurationManager*>(ConfigurationMgrImpl().GetDelegate());
  delegate->set_is_ipv6_forwarding_enabled(true);

  // Attempt to add a route to the Tunnel interface.
  auto result = AddRemoveHostRoute(kInterfaceTypeTunnel, prefix, kRoutePriorityHigh, /*add*/ true);
  EXPECT_EQ(result, kPlatformResultSuccess);

  // Confirm that this interface is forwarded, consistent with configuration.
  EXPECT_TRUE(fake_net_stack().IsInterfaceForwarded(tunnel_iface_id));
}

TEST_F(WarmTest, AddRemoveHostRouteTunnel) {
  constexpr char kSubnetIp[] = "2001:0DB8:0042::";
  constexpr uint8_t kPrefixLength = 48;
  Inet::IPPrefix prefix;

  ASSERT_TRUE(Inet::IPAddress::FromString(kSubnetIp, prefix.IPAddr));
  prefix.Length = kPrefixLength;

  // Sanity check - confirm no routes to the Tunnel interface exist.
  uint64_t tunnel_iface_id = GetTunnelInterfaceId();
  ASSERT_NE(tunnel_iface_id, 0u);
  EXPECT_FALSE(fake_net_stack().FindRouteTableEntry(tunnel_iface_id, prefix.IPAddr));

  // Attempt to add a route to the Tunnel interface.
  auto result = AddRemoveHostRoute(kInterfaceTypeTunnel, prefix, kRoutePriorityHigh, /*add*/ true);
  EXPECT_EQ(result, kPlatformResultSuccess);

  // Confirm that a route exists to the Tunnel interface with the given IP.
  EXPECT_TRUE(fake_net_stack().FindRouteTableEntry(tunnel_iface_id, prefix.IPAddr));

  // Confirm that this interface is forwarded/not, consistent with device configuration.
  EXPECT_EQ(fake_net_stack().IsInterfaceForwarded(tunnel_iface_id),
            ConfigurationMgrImpl().IsIPv6ForwardingEnabled());

  // Remove the route to the Tunnel interface.
  result = AddRemoveHostRoute(kInterfaceTypeTunnel, prefix, kRoutePriorityHigh, /*add*/ false);
  EXPECT_EQ(result, kPlatformResultSuccess);

  // Confirm that the removal worked.
  EXPECT_FALSE(fake_net_stack().FindRouteTableEntry(tunnel_iface_id, prefix.IPAddr));
}

TEST_F(WarmTest, AddRemoveHostRouteWiFi) {
  constexpr char kSubnetIp[] = "2001:0DB8:0042::";
  constexpr uint8_t kPrefixLength = 48;
  Inet::IPPrefix prefix;

  ASSERT_TRUE(Inet::IPAddress::FromString(kSubnetIp, prefix.IPAddr));
  prefix.Length = kPrefixLength;

  // Sanity check - confirm no routes to the WiFi interface exist.
  uint64_t wlan_iface_id = GetWiFiInterfaceId();
  ASSERT_NE(wlan_iface_id, 0u);
  EXPECT_FALSE(fake_net_stack().FindRouteTableEntry(wlan_iface_id, prefix.IPAddr));

  // Attempt to add a route to the WiFi interface.
  auto result = AddRemoveHostRoute(kInterfaceTypeWiFi, prefix, kRoutePriorityHigh, /*add*/ true);
  EXPECT_EQ(result, kPlatformResultSuccess);

  // Confirm that a route exists to the WiFi interface with the given IP.
  EXPECT_TRUE(fake_net_stack().FindRouteTableEntry(wlan_iface_id, prefix.IPAddr));

  // Confirm that this interface is NOT forwarded.
  EXPECT_FALSE(fake_net_stack().IsInterfaceForwarded(wlan_iface_id));

  // Remove the route to the WiFi interface.
  result = AddRemoveHostRoute(kInterfaceTypeWiFi, prefix, kRoutePriorityHigh, /*add*/ false);
  EXPECT_EQ(result, kPlatformResultSuccess);

  // Confirm that the removal worked.
  EXPECT_FALSE(fake_net_stack().FindRouteTableEntry(wlan_iface_id, prefix.IPAddr));
}

TEST_F(WarmTest, RemoveHostRouteTunnelNotFound) {
  constexpr char kSubnetIp[] = "2001:0DB8:0042::";
  constexpr uint8_t kPrefixLength = 48;
  Inet::IPPrefix prefix;

  ASSERT_TRUE(Inet::IPAddress::FromString(kSubnetIp, prefix.IPAddr));
  prefix.Length = kPrefixLength;

  // Sanity check - confirm no routes to the Tunnel interface exist.
  uint64_t tunnel_iface_id = GetTunnelInterfaceId();
  ASSERT_NE(tunnel_iface_id, 0u);
  EXPECT_FALSE(fake_net_stack().FindRouteTableEntry(tunnel_iface_id, prefix.IPAddr));

  // Remove the non-existent route to the Tunnel interface, expect failure.
  auto result = AddRemoveHostRoute(kInterfaceTypeTunnel, prefix, kRoutePriorityHigh, /*add*/ false);
  EXPECT_EQ(result, kPlatformResultFailure);

  // Confirm that the interface is not forwarded.
  EXPECT_FALSE(fake_net_stack().IsInterfaceForwarded(tunnel_iface_id));

  // Sanity check - confirm still no routes to the Tunnel interface exist.
  EXPECT_FALSE(fake_net_stack().FindRouteTableEntry(tunnel_iface_id, prefix.IPAddr));
}

TEST_F(WarmTest, RemoveHostRouteWiFiNotFound) {
  constexpr char kSubnetIp[] = "2001:0DB8:0042::";
  constexpr uint8_t kPrefixLength = 48;
  Inet::IPPrefix prefix;

  ASSERT_TRUE(Inet::IPAddress::FromString(kSubnetIp, prefix.IPAddr));
  prefix.Length = kPrefixLength;

  // Sanity check - confirm no routes to the WiFi interface exist.
  uint64_t wlan_iface_id = GetWiFiInterfaceId();
  ASSERT_NE(wlan_iface_id, 0u);
  EXPECT_FALSE(fake_net_stack().FindRouteTableEntry(wlan_iface_id, prefix.IPAddr));

  // Remove the non-existent route to the WiFi interface, expect failure.
  auto result = AddRemoveHostRoute(kInterfaceTypeWiFi, prefix, kRoutePriorityHigh, /*add*/ false);
  EXPECT_EQ(result, kPlatformResultFailure);

  // Confirm that the interface is not forwarded.
  EXPECT_FALSE(fake_net_stack().IsInterfaceForwarded(wlan_iface_id));

  // Sanity check - confirm still no routes to the WiFi interface exist.
  EXPECT_FALSE(fake_net_stack().FindRouteTableEntry(wlan_iface_id, prefix.IPAddr));
}

TEST_F(WarmTest, AddHostRouteTunnelRoutePriorities) {
  constexpr char kSubnetIp[] = "2001:0DB8:0042::";
  constexpr uint8_t kPrefixLength = 48;
  Inet::IPPrefix prefix;

  ASSERT_TRUE(Inet::IPAddress::FromString(kSubnetIp, prefix.IPAddr));
  prefix.Length = kPrefixLength;

  // Sanity check - confirm no routes to the tunnel interface exist.
  uint64_t tunnel_iface_id = GetTunnelInterfaceId();
  ASSERT_NE(tunnel_iface_id, 0u);
  EXPECT_FALSE(fake_net_stack().FindRouteTableEntry(tunnel_iface_id, prefix.IPAddr));

  // Add a high-priority route to the tunnel interface.
  auto result = AddRemoveHostRoute(kInterfaceTypeTunnel, prefix, kRoutePriorityHigh, /*add*/ true);
  EXPECT_EQ(result, kPlatformResultSuccess);

  // Add a low-priority route to the tunnel interface.
  result = AddRemoveHostRoute(kInterfaceTypeTunnel, prefix, kRoutePriorityLow, /*add*/ true);
  EXPECT_EQ(result, kPlatformResultSuccess);

  // Confirm all three priority routes exist.
  EXPECT_TRUE(fake_net_stack().FindRouteTableEntry(tunnel_iface_id, prefix.IPAddr,
                                                   kRouteMetric_HighPriority));
  EXPECT_TRUE(fake_net_stack().FindRouteTableEntry(tunnel_iface_id, prefix.IPAddr,
                                                   kRouteMetric_LowPriority));
}

}  // namespace testing
}  // namespace Platform
}  // namespace Warm
}  // namespace Weave
}  // namespace nl
