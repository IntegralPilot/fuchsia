// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.component.runner/cpp/wire_types.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/driver/component/cpp/tests/test_base.h>
#include <lib/driver/incoming/cpp/namespace.h>
#include <lib/fidl/cpp/binding.h>

#include <gtest/gtest.h>

namespace fio = fuchsia::io;
namespace frunner = fuchsia_component_runner;

TEST(NamespaceTest, CreateAndConnect) {
  async::Loop loop{&kAsyncLoopConfigNoAttachToCurrentThread};

  auto pkg = fidl::Endpoints<fuchsia_io::Directory>::Create();
  auto svc = fidl::Endpoints<fuchsia_io::Directory>::Create();
  fidl::Arena arena;
  fidl::VectorView<frunner::wire::ComponentNamespaceEntry> ns_entries(arena, 2);
  ns_entries[0].Allocate(arena);
  ns_entries[0].set_path(arena, "/pkg").set_directory(std::move(pkg.client));
  ns_entries[1].Allocate(arena);
  ns_entries[1].set_path(arena, "/svc").set_directory(std::move(svc.client));
  auto ns = fdf::Namespace::Create(ns_entries);
  ASSERT_TRUE(ns.is_ok());

  svc.server.TakeChannel().reset();

  fdf::testing::Directory pkg_directory;
  fidl::Binding<fio::Directory> pkg_binding(&pkg_directory);
  pkg_binding.Bind(pkg.server.TakeChannel(), loop.dispatcher());

  zx::channel server_end;
  pkg_directory.SetOpenHandler([&server_end](std::string path, auto object) {
    EXPECT_EQ("path-exists", path);
    server_end = std::move(object.TakeChannel());
  });

  auto client_end =
      ns->Open<fuchsia_io::File>("/pkg/path-exists", fuchsia_io::Flags::kPermReadBytes);
  EXPECT_TRUE(client_end.is_ok());
  loop.RunUntilIdle();
  zx_info_handle_basic_t client_info = {}, server_info = {};
  ASSERT_EQ(ZX_OK, client_end->channel().get_info(ZX_INFO_HANDLE_BASIC, &client_info,
                                                  sizeof(client_info), nullptr, nullptr));
  ASSERT_EQ(ZX_OK, server_end.get_info(ZX_INFO_HANDLE_BASIC, &server_info, sizeof(server_info),
                                       nullptr, nullptr));
  EXPECT_EQ(client_info.koid, server_info.related_koid);

  auto not_found_client_end = ns->Connect<frunner::ComponentRunner>("/svc/path-does-not-exist");
  EXPECT_EQ(ZX_OK, not_found_client_end.status_value());

  zx_signals_t observed = ZX_SIGNAL_NONE;
  EXPECT_EQ(ZX_OK, not_found_client_end->channel().wait_one(ZX_CHANNEL_PEER_CLOSED,
                                                            zx::time::infinite_past(), &observed));
  EXPECT_EQ(ZX_CHANNEL_PEER_CLOSED, observed);
}

TEST(NamespaceTest, CreateFailed) {
  async::Loop loop{&kAsyncLoopConfigNoAttachToCurrentThread};

  fidl::Arena arena;
  fidl::VectorView<frunner::wire::ComponentNamespaceEntry> ns_entries(arena, 1);
  ns_entries[0].Allocate(arena);
  ns_entries[0].set_path(arena, "/pkg").set_directory({});
  auto ns = fdf::Namespace::Create(ns_entries);
  ASSERT_TRUE(ns.is_error());
}
