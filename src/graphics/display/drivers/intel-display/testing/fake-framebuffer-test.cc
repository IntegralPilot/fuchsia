// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/intel-display/testing/fake-framebuffer.h"

#include <fidl/fuchsia.boot/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/zbi-format/graphics.h>
#include <lib/zbi-format/zbi.h>
#include <zircon/syscalls.h>

#include <gtest/gtest.h>

#include "src/lib/testing/predicates/status.h"

namespace {

TEST(FakeFramebuffer, Error) {
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  fake_framebuffer::FakeBootItems boot_items;
  boot_items.SetFramebuffer({
      .status = ZX_ERR_NOT_FOUND,
  });

  auto [client_end, server_end] = fidl::Endpoints<fuchsia_boot::Items>::Create();
  boot_items.Serve(loop.dispatcher(), std::move(server_end));

  fidl::WireClient<fuchsia_boot::Items> client;
  client.Bind(std::move(client_end), loop.dispatcher());

  client->Get2(ZBI_TYPE_FRAMEBUFFER, {})
      .Then([](fidl::WireUnownedResult<fuchsia_boot::Items::Get2>& result) {
        ASSERT_STATUS(result.status(), ZX_OK);
        ASSERT_TRUE(result->is_error());
        EXPECT_STATUS(result->error_value(), ZX_ERR_NOT_FOUND);
      });
  loop.RunUntilIdle();
}

TEST(FakeFramebuffer, Ok) {
  constexpr fake_framebuffer::Framebuffer kExpectedFramebuffer = {
      .status = ZX_OK,
      .format = ZBI_PIXEL_FORMAT_RGB_888,
      .width = 0x05060708,
      .height = 0x090a0b0c,
      .stride = 0x0d0e0f10,
  };
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  fake_framebuffer::FakeBootItems boot_items;
  boot_items.SetFramebuffer(kExpectedFramebuffer);

  auto [client_end, server_end] = fidl::Endpoints<fuchsia_boot::Items>::Create();
  boot_items.Serve(loop.dispatcher(), std::move(server_end));

  fidl::WireClient<fuchsia_boot::Items> client;
  client.Bind(std::move(client_end), loop.dispatcher());

  client->Get2(ZBI_TYPE_FRAMEBUFFER, {})
      .Then([&](fidl::WireUnownedResult<fuchsia_boot::Items::Get2>& result) {
        ASSERT_STATUS(result.status(), ZX_OK);
        ASSERT_TRUE(result->is_ok());
        auto& items = result->value()->retrieved_items;
        ASSERT_GE(items.size(), 1ul);
        zbi_swfb_t fb;
        ASSERT_GE(items[0].length, sizeof(fb));
        ASSERT_OK(items[0].payload.read(&fb, 0, sizeof(fb)));
        EXPECT_EQ(fb.format, kExpectedFramebuffer.format);
        EXPECT_EQ(fb.width, kExpectedFramebuffer.width);
        EXPECT_EQ(fb.height, kExpectedFramebuffer.height);
        EXPECT_EQ(fb.stride, kExpectedFramebuffer.stride);
      });
  loop.RunUntilIdle();
}

}  // namespace
