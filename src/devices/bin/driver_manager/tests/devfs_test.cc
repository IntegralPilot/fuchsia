// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/bin/driver_manager/devfs/devfs.h"

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/ddk/driver.h>

#include <functional>

#include <zxtest/zxtest.h>

#include "src/storage/lib/vfs/cpp/synchronous_vfs.h"

namespace {

using driver_manager::Devfs;
using driver_manager::DevfsDevice;
using driver_manager::Devnode;

class Connecter : public fidl::WireServer<fuchsia_device_fs::Connector> {
 public:
 private:
  void Connect(ConnectRequestView request, ConnectCompleter::Sync& completer) override {
    ASSERT_EQ(channel_.get(), ZX_HANDLE_INVALID);
    channel_ = std::move(request->server);
  }

  zx::channel channel_;
};

std::optional<std::reference_wrapper<const Devnode>> lookup(const Devnode& parent,
                                                            std::string_view name) {
  {
    fbl::RefPtr<fs::Vnode> out;
    switch (const zx_status_t status = parent.children().Lookup(name, &out); status) {
      case ZX_OK:
        return std::reference_wrapper(fbl::RefPtr<Devnode::VnodeImpl>::Downcast(out)->holder_);
      case ZX_ERR_NOT_FOUND:
        break;
      default:
        ADD_FAILURE("%s", zx_status_get_string(status));
        return {};
    }
  }
  const auto it = parent.children().unpublished.find(name);
  if (it != parent.children().unpublished.end()) {
    return it->second.get();
  }
  return {};
}

TEST(Devfs, Export) {
  async::Loop loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  std::optional<Devnode> root_slot;
  const Devfs devfs(root_slot, loop.dispatcher());
  ASSERT_TRUE(root_slot.has_value());
  Devnode& root_node = root_slot.value();
  std::vector<std::unique_ptr<Devnode>> out;

  ASSERT_OK(root_node.export_dir(Devnode::Target(), "one/two", {}, out));

  std::optional node_one = lookup(root_node, "one");
  ASSERT_TRUE(node_one.has_value());
  EXPECT_EQ("one", node_one->get().name());
  std::optional node_two = lookup(node_one->get(), "two");
  ASSERT_TRUE(node_two.has_value());
  EXPECT_EQ("two", node_two->get().name());
}

TEST(Devfs, Export_ExcessSeparators) {
  async::Loop loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  std::optional<Devnode> root_slot;
  const Devfs devfs(root_slot, loop.dispatcher());
  ASSERT_TRUE(root_slot.has_value());
  Devnode& root_node = root_slot.value();
  std::vector<std::unique_ptr<Devnode>> out;

  ASSERT_STATUS(root_node.export_dir(Devnode::Target(), "one//two", {}, out), ZX_ERR_INVALID_ARGS);

  ASSERT_FALSE(lookup(root_node, "one").has_value());
  ASSERT_FALSE(lookup(root_node, "two").has_value());
}

TEST(Devfs, Export_OneByOne) {
  async::Loop loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  std::optional<Devnode> root_slot;
  const Devfs devfs(root_slot, loop.dispatcher());
  ASSERT_TRUE(root_slot.has_value());
  Devnode& root_node = root_slot.value();
  std::vector<std::unique_ptr<Devnode>> out;

  ASSERT_OK(root_node.export_dir(Devnode::Target(), "one", {}, out));
  std::optional node_one = lookup(root_node, "one");
  ASSERT_TRUE(node_one.has_value());
  EXPECT_EQ("one", node_one->get().name());

  ASSERT_OK(root_node.export_dir(Devnode::Target(), "one/two", {}, out));
  std::optional node_two = lookup(node_one->get(), "two");
  ASSERT_TRUE(node_two.has_value());
  EXPECT_EQ("two", node_two->get().name());
}

TEST(Devfs, Export_InvalidPath) {
  async::Loop loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  std::optional<Devnode> root_slot;
  const Devfs devfs(root_slot, loop.dispatcher());
  ASSERT_TRUE(root_slot.has_value());
  Devnode& root_node = root_slot.value();
  std::vector<std::unique_ptr<Devnode>> out;

  ASSERT_STATUS(ZX_ERR_INVALID_ARGS, root_node.export_dir(Devnode::Target(), "", {}, out));
  ASSERT_STATUS(ZX_ERR_INVALID_ARGS, root_node.export_dir(Devnode::Target(), "/one/two", {}, out));
  ASSERT_STATUS(ZX_ERR_INVALID_ARGS, root_node.export_dir(Devnode::Target(), "one/two/", {}, out));
  ASSERT_STATUS(ZX_ERR_INVALID_ARGS, root_node.export_dir(Devnode::Target(), "/one/two/", {}, out));
}

TEST(Devfs, Export_WithProtocol) {
  async::Loop loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  std::optional<Devnode> root_slot;
  const Devfs devfs(root_slot, loop.dispatcher());
  ASSERT_TRUE(root_slot.has_value());
  Devnode& root_node = root_slot.value();

  std::vector<std::unique_ptr<Devnode>> out;
  ASSERT_OK(root_node.export_dir(Devnode::Target(), "one/two", "block", out));

  std::optional node_one = lookup(root_node, "one");
  ASSERT_TRUE(node_one.has_value());
  EXPECT_EQ("one", node_one->get().name());

  std::optional node_two = lookup(node_one->get(), "two");
  ASSERT_TRUE(node_two.has_value());
  EXPECT_EQ("two", node_two->get().name());
}

TEST(Devfs, Export_AlreadyExists) {
  async::Loop loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  std::optional<Devnode> root_slot;
  const Devfs devfs(root_slot, loop.dispatcher());
  ASSERT_TRUE(root_slot.has_value());
  Devnode& root_node = root_slot.value();
  std::vector<std::unique_ptr<Devnode>> out;

  ASSERT_OK(root_node.export_dir(Devnode::Target(), "one/two", {}, out));
  ASSERT_STATUS(ZX_ERR_ALREADY_EXISTS, root_node.export_dir(Devnode::Target(), "one/two", {}, out));
}

TEST(Devfs, Export_DropDevfs) {
  async::Loop loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  std::optional<Devnode> root_slot;
  const Devfs devfs(root_slot, loop.dispatcher());
  ASSERT_TRUE(root_slot.has_value());
  Devnode& root_node = root_slot.value();
  std::vector<std::unique_ptr<Devnode>> out;

  ASSERT_OK(root_node.export_dir(Devnode::Target(), "one/two", {}, out));

  {
    std::optional node_one = lookup(root_node, "one");
    ASSERT_TRUE(node_one.has_value());
    EXPECT_EQ("one", node_one->get().name());

    std::optional node_two = lookup(node_one->get(), "two");
    ASSERT_TRUE(node_two.has_value());
    EXPECT_EQ("two", node_two->get().name());
  }

  out.clear();

  ASSERT_FALSE(lookup(root_node, "one").has_value());
}

TEST(Devfs, PassthroughTarget) {
  async::Loop loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  fs::SynchronousVfs vfs(loop.dispatcher());

  std::optional<Devnode> root_slot;
  Devfs devfs(root_slot, loop.dispatcher());
  ASSERT_TRUE(root_slot.has_value());
  fuchsia_device_fs::ConnectionType connection_type;
  Devnode::PassThrough passthrough(
      {
          [&loop, &connection_type](zx::channel server) {
            connection_type = fuchsia_device_fs::ConnectionType::kDevice;
            loop.Quit();
            return ZX_OK;
          },
      },
      {
          [&loop, &connection_type](fidl::ServerEnd<fuchsia_device::Controller> server_end) {
            connection_type = fuchsia_device_fs::ConnectionType::kController;
            loop.Quit();
            return ZX_OK;
          },
      });

  DevfsDevice device;
  ASSERT_OK(root_slot.value().add_child("test", std::nullopt, passthrough, device));
  device.publish();

  zx::result devfs_client = devfs.Connect(vfs);
  ASSERT_OK(devfs_client);

  struct TestRun {
    const char* file_name;
    fuchsia_device_fs::ConnectionType expected;
  };

  const TestRun tests[] = {
      {
          .file_name = "test",
          .expected = fuchsia_device_fs::ConnectionType::kDevice,
      },
      {
          .file_name = "test/device_controller",
          .expected = fuchsia_device_fs::ConnectionType::kController,
      },
      {
          .file_name = "test/device_protocol",
          .expected = fuchsia_device_fs::ConnectionType::kDevice,
      },
  };

  for (const TestRun& test : tests) {
    SCOPED_TRACE(test.file_name);
    auto [_, server_end] = fidl::Endpoints<fuchsia_io::Node>::Create();

    ASSERT_OK(fidl::WireCall(devfs_client.value())
                  ->Open(fidl::StringView::FromExternal(test.file_name),
                         fuchsia_io::wire::Flags::kProtocolService, {}, server_end.TakeChannel())
                  .status());
    loop.Run();
    loop.ResetQuit();

    ASSERT_EQ(connection_type, test.expected);
  }
}

TEST(Devfs, DeviceTopologyUseAfterFree) {
  async::Loop loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  std::optional<Devnode> root_slot;
  Devfs devfs(root_slot, loop.dispatcher());
  ASSERT_TRUE(root_slot.has_value());
  Devnode& root_node = root_slot.value();

  Devnode::PassThrough passthrough(
      [](zx::channel server) { return ZX_OK; },
      [](fidl::ServerEnd<fuchsia_device::Controller> server_end) { return ZX_OK; });

  DevfsDevice child;
  ASSERT_OK(root_node.add_child("test-network", "network", passthrough, child));

  auto network_class_dir = devfs.get_class_entry("network");
  ASSERT_NE(network_class_dir, nullptr);
  ASSERT_EQ(1u, network_class_dir->unpublished.size());
  std::string instance_name(network_class_dir->unpublished.begin()->first);
  ASSERT_FALSE(instance_name.empty());

  child.publish();

  auto [client_end, server_end] = fidl::Endpoints<fuchsia_io::Directory>::Create();
  ASSERT_OK(devfs.outgoing().Serve(std::move(server_end)));

  // Unpublish the device. This destroys the Devnode.
  child.unpublish();

  // Try to connect to the device_topology protocol of the destroyed node.
  std::string topo_path = "svc/fuchsia.hardware.network.Service/" + instance_name + "/" +
                          fuchsia_device_fs::wire::kDeviceTopologyName;
  auto [topo_client, topo_server] = fidl::Endpoints<fuchsia_device_fs::TopologicalPath>::Create();

  ASSERT_OK(fidl::WireCall(client_end)
                ->Open(fidl::StringView::FromExternal(topo_path),
                       fuchsia_io::wire::Flags::kProtocolService, {}, topo_server.TakeChannel())
                .status());

  // Run the loop. If the handler was not removed, this will call the handler
  // which accesses the destroyed Devnode's path_server_, causing a UAF.
  loop.RunUntilIdle();

  // Check if the channel was closed.
  ASSERT_OK(
      topo_client.channel().wait_one(ZX_CHANNEL_PEER_CLOSED, zx::time::infinite_past(), nullptr));
}

}  // namespace
