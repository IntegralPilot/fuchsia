// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <fidl/fuchsia.device.lifecycle.test/cpp/wire.h>
#include <fidl/fuchsia.device/cpp/wire.h>
#include <fidl/fuchsia.hardware.serial/cpp/wire.h>
#include <fidl/fuchsia.io/cpp/wire.h>
#include <lib/ddk/platform-defs.h>
#include <lib/driver-integration-test/fixture.h>
#include <lib/fdio/cpp/caller.h>
#include <lib/fdio/directory.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/fidl/cpp/wire/connect_service.h>
#include <lib/fit/defer.h>
#include <zircon/processargs.h>
#include <zircon/syscalls.h>

#include <sstream>
#include <vector>

#include <zxtest/zxtest.h>

using driver_integration_test::IsolatedDevmgr;
using fuchsia_device_lifecycle_test::Lifecycle;
using fuchsia_device_lifecycle_test::TestDevice;

class LifecycleTest : public zxtest::Test {
 public:
  ~LifecycleTest() override = default;
  void SetUp() override {
    IsolatedDevmgr::Args args;

    board_test::DeviceEntry dev = {};
    strlcpy(dev.name, kPlatformDeviceName, sizeof(dev.name));
    dev.vid = PDEV_VID_TEST;
    dev.pid = PDEV_PID_LIFECYCLE_TEST;
    dev.did = 0;
    args.device_list.push_back(dev);

    zx_status_t status = IsolatedDevmgr::Create(&args, &devmgr_);
    ASSERT_OK(status);
    zx::result channel = device_watcher::RecursiveWaitForFile(devmgr_.devfs_root().get(),
                                                              GetLifecycleTopoPath().c_str());
    ASSERT_OK(channel.status_value());
    chan_ = fidl::ClientEnd<TestDevice>(std::move(channel.value()));

    // Subscribe to the device lifecycle events.
    auto [local, remote] = fidl::Endpoints<Lifecycle>::Create();

    auto result = fidl::WireCall(chan_)->SubscribeToLifecycle(std::move(remote));
    ASSERT_OK(result.status());
    ASSERT_FALSE(result->is_error());
    lifecycle_chan_ = std::move(local);
  }

 protected:
  static std::string GetLifecycleTopoPath() {
    std::ostringstream path;
    path << "sys/platform/" << kPlatformDeviceName << '/' << kLifecycleDeviceName;
    return path.str();
  }

  static std::string GetLifecycleControllerTopoPath() {
    std::ostringstream path;
    path << GetLifecycleTopoPath() << "/device_controller";
    return path.str();
  }

  static std::string GetChildTopoPath() {
    std::ostringstream path;
    path << GetLifecycleTopoPath() << '/' << kChildDeviceName;
    return path.str();
  }

  void WaitPreRelease(uint64_t child_id);

  fidl::ClientEnd<TestDevice> chan_;
  IsolatedDevmgr devmgr_;

  fidl::ClientEnd<Lifecycle> lifecycle_chan_;

 private:
  static constexpr char kPlatformDeviceName[board_test::kNameLengthMax] = "ddk-platform-device";
  static constexpr std::string_view kLifecycleDeviceName = "ddk-lifecycle-test";
  static constexpr std::string_view kChildDeviceName = "ddk-lifecycle-test-child-0";
};

void LifecycleTest::WaitPreRelease(uint64_t child_id) {
  class EventHandler : public fidl::WireSyncEventHandler<Lifecycle> {
   public:
    EventHandler() = default;

    bool removed() const { return removed_; }
    uint64_t device_id() const { return device_id_; }

    void OnChildPreRelease(fidl::WireEvent<Lifecycle::OnChildPreRelease>* event) override {
      device_id_ = event->child_id;
      removed_ = true;
    }

   private:
    bool removed_ = false;
    uint64_t device_id_ = 0;
  };

  EventHandler event_handler;
  while (!event_handler.removed()) {
    ASSERT_OK(event_handler.HandleOneEvent(lifecycle_chan_).status());
  }
  ASSERT_EQ(event_handler.device_id(), child_id);
}

TEST_F(LifecycleTest, ChildPreRelease) {
  // Add some child devices and store the returned ids.
  std::vector<uint64_t> child_ids;
  const uint32_t num_children = 10;
  for (unsigned int i = 0; i < num_children; i++) {
    auto result =
        fidl::WireCall(chan_)->AddChild(true /* complete_init */, ZX_OK /* init_status */);
    ASSERT_OK(result.status());
    ASSERT_FALSE(result->is_error());
    child_ids.push_back(result->value()->child_id);
  }

  // Remove the child devices and check the test device received the pre-release notifications.
  for (auto child_id : child_ids) {
    auto result = fidl::WireCall(chan_)->RemoveChild(child_id);
    ASSERT_OK(result.status());
    ASSERT_FALSE(result->is_error());

    // Wait for the child pre-release notification.
    ASSERT_NO_FATAL_FAILURE(WaitPreRelease(child_id));
  }
}

TEST_F(LifecycleTest, Init) {
  // Add a child device that does not complete its init hook yet.
  uint64_t child_id;
  auto result = fidl::WireCall(chan_)->AddChild(false /* complete_init */, ZX_OK /* init_status */);
  ASSERT_OK(result.status());
  ASSERT_FALSE(result->is_error());
  child_id = result->value()->child_id;

  auto remove_result = fidl::WireCall(chan_)->RemoveChild(child_id);
  ASSERT_OK(remove_result.status());
  ASSERT_FALSE(remove_result->is_error());

  auto init_result = fidl::WireCall(chan_)->CompleteChildInit(child_id);
  ASSERT_OK(init_result.status());
  ASSERT_FALSE(init_result->is_error());

  // Wait for the child pre-release notification.
  ASSERT_NO_FATAL_FAILURE(WaitPreRelease(child_id));
}

TEST_F(LifecycleTest, CloseAllConnectionsOnChildUnbind) {
  auto result = fidl::WireCall(chan_)->AddChild(true /* complete_init */, ZX_OK /* init_status */);
  ASSERT_OK(result.status());
  ASSERT_FALSE(result->is_error());
  auto child_id = result->value()->child_id;

  zx::result channel =
      device_watcher::RecursiveWaitForFile(devmgr_.devfs_root().get(), GetChildTopoPath().c_str());
  ASSERT_OK(channel.status_value());

  zx::channel chan = std::move(channel.value());
  ASSERT_TRUE(fidl::WireCall(chan_)->RemoveChild(child_id).ok());
  zx_signals_t closed;
  ASSERT_OK(chan.wait_one(ZX_CHANNEL_PEER_CLOSED, zx::time::infinite(), &closed));
  ASSERT_TRUE(closed & ZX_CHANNEL_PEER_CLOSED);
  // Wait for the child pre-release notification.
  ASSERT_NO_FATAL_FAILURE(WaitPreRelease(child_id));
}

template <typename Protocol>
zx::result<fidl::WireClient<Protocol>> MakeClient(const fbl::unique_fd& fd, const char* path,
                                                  async_dispatcher_t* dispatcher) {
  zx::result channel = device_watcher::RecursiveWaitForFile(fd.get(), path);
  if (channel.is_error()) {
    return channel.take_error();
  }
  return zx::ok(fidl::WireClient<Protocol>(fidl::ClientEnd<Protocol>(std::move(channel.value())),
                                           dispatcher));
}

TEST_F(LifecycleTest, CallsFailDuringUnbind) {
  auto result = fidl::WireCall(chan_)->AddChild(true /* complete_init */, ZX_OK /* init_status */);
  ASSERT_OK(result.status());
  ASSERT_FALSE(result->is_error());
  auto child_id = result->value()->child_id;

  async::Loop loop{&kAsyncLoopConfigNeverAttachToThread};

  // Connect before unbinding.

  const fbl::unique_fd& fd = devmgr_.devfs_root();
  async_dispatcher_t* const dispatcher = loop.dispatcher();

  zx::result queryable =
      MakeClient<fuchsia_unknown::Queryable>(fd, GetChildTopoPath().c_str(), dispatcher);
  ASSERT_OK(queryable);

  zx::result controller =
      MakeClient<fuchsia_device::Controller>(fd, GetChildTopoPath().c_str(), dispatcher);
  ASSERT_OK(controller);

  zx::result device =
      MakeClient<fuchsia_hardware_serial::Device>(fd, GetChildTopoPath().c_str(), dispatcher);
  ASSERT_OK(device);

  ASSERT_OK(loop.RunUntilIdle());

  ASSERT_OK(fidl::WireCall(chan_)->AsyncRemoveChild(child_id).status());

  auto quitter =
      std::make_shared<fit::deferred_action<fit::callback<void()>>>([&loop]() { loop.Quit(); });

  queryable.value()->Query().ThenExactlyOnce(
      [&, quitter](fidl::WireUnownedResult<fuchsia_unknown::Queryable::Query>& result) {
        ASSERT_STATUS(result.status(), ZX_ERR_IO_NOT_PRESENT);
      });
  controller.value()->GetTopologicalPath().ThenExactlyOnce(
      [&,
       quitter](fidl::WireUnownedResult<fuchsia_device::Controller::GetTopologicalPath>& result) {
        ASSERT_STATUS(result.status(), ZX_ERR_IO_NOT_PRESENT);
      });
  device.value()->GetClass().ThenExactlyOnce(
      [&, quitter](fidl::WireUnownedResult<fuchsia_hardware_serial::Device::GetClass>& result) {
        ASSERT_STATUS(result.status(), ZX_ERR_IO_NOT_PRESENT);
      });

  quitter.reset();

  ASSERT_STATUS(loop.Run(), ZX_ERR_CANCELED);

  ASSERT_EQ(open(GetChildTopoPath().c_str(), O_RDWR), -1);
  ASSERT_EQ(errno, ENOENT);

  ASSERT_OK(fidl::WireCall(chan_)->CompleteUnbind(child_id).status());
}

TEST_F(LifecycleTest, CloseAllConnectionsOnUnbind) {
  zx::result channel = device_watcher::RecursiveWaitForFile(
      devmgr_.devfs_root().get(), GetLifecycleControllerTopoPath().c_str());
  ASSERT_OK(channel);
  fidl::WireSyncClient controller{
      fidl::ClientEnd<fuchsia_device::Controller>(std::move(channel.value()))};
  const fidl::WireResult result = controller->ScheduleUnbind();
  ASSERT_OK(result.status());
  const fit::result response = result.value();
  ASSERT_TRUE(response.is_ok(), "%s", zx_status_get_string(response.error_value()));
  zx_signals_t closed;
  ASSERT_OK(chan_.channel().wait_one(ZX_CHANNEL_PEER_CLOSED, zx::time::infinite(), &closed));
  ASSERT_TRUE(closed & ZX_CHANNEL_PEER_CLOSED);
}

// Tests that the child device is removed if init fails.
TEST_F(LifecycleTest, FailedInit) {
  uint64_t child_id;
  auto result =
      fidl::WireCall(chan_)->AddChild(true /* complete_init */, ZX_ERR_BAD_STATE /* init_status */);
  ASSERT_OK(result.status());
  ASSERT_FALSE(result->is_error());
  child_id = result->value()->child_id;

  // Wait for the child pre-release notification.
  ASSERT_NO_FATAL_FAILURE(WaitPreRelease(child_id));
}

TEST_F(LifecycleTest, RebindNoChildren) {
  zx::result channel = device_watcher::RecursiveWaitForFile(
      devmgr_.devfs_root().get(), GetLifecycleControllerTopoPath().c_str());
  ASSERT_OK(channel);
  fidl::WireSyncClient controller{
      fidl::ClientEnd<fuchsia_device::Controller>(std::move(channel.value()))};
  fidl::WireResult result = controller->Rebind({});
  ASSERT_OK(result.status());
  ASSERT_TRUE(result.value().is_ok(), "%s", zx_status_get_string(result.value().error_value()));
}

TEST_F(LifecycleTest, RebindChildren) {
  {
    fidl::WireResult result =
        fidl::WireCall(chan_)->AddChild(true /* complete_init */, ZX_OK /* init_status */);
    ASSERT_OK(result.status());
    ASSERT_TRUE(result.value().is_ok(), "%s", zx_status_get_string(result.value().error_value()));
  }

  zx::result channel = device_watcher::RecursiveWaitForFile(
      devmgr_.devfs_root().get(), GetLifecycleControllerTopoPath().c_str());
  ASSERT_OK(channel);
  fidl::WireSyncClient controller{
      fidl::ClientEnd<fuchsia_device::Controller>(std::move(channel.value()))};
  fidl::WireResult result = controller->Rebind({});
  ASSERT_OK(result.status());
  ASSERT_TRUE(result.value().is_ok(), "%s", zx_status_get_string(result.value().error_value()));
}
