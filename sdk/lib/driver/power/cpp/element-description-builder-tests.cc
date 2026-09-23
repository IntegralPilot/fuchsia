// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.power.broker/cpp/fidl.h>
#include <lib/driver/power/cpp/element-description-builder.h>
#include <lib/driver/power/cpp/power-support.h>
#include <zircon/syscalls/object.h>

#include <src/lib/testing/loop_fixture/test_loop_fixture.h>

#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)

namespace power_lib_test {
class ElementBuilderTests : public gtest::TestLoopFixture {};

void check_channels_peered(zx_handle_t c1, zx_handle_t c2) {
  zx_info_handle_basic_t basic1;
  size_t actual1;
  size_t handles1;

  zx_info_handle_basic_t basic2;
  size_t actual2;
  size_t handles2;

  zx_object_get_info(c1, ZX_INFO_HANDLE_BASIC, &basic1, sizeof(zx_info_handle_basic_t), &actual1,
                     &handles1);
  zx_object_get_info(c2, ZX_INFO_HANDLE_BASIC, &basic2, sizeof(zx_info_handle_basic_t), &actual2,
                     &handles2);
  ASSERT_EQ(basic1.koid, basic2.related_koid);
}

TEST_F(ElementBuilderTests, ElementBuilderElementRunnerFilledOut) {
  fdf_power::PowerElementConfiguration config;
  fdf_power::TokenMap tokens;

  zx_handle_t active, passive;
  zx_event_create(0, &active);
  zx_event_create(0, &passive);
  zx::event active_event(active);
  zx::event passive_event(passive);
  fidl::Endpoints<fuchsia_power_broker::Lessor> lessor =
      fidl::CreateEndpoints<fuchsia_power_broker::Lessor>().value();
  fidl::Endpoints<fuchsia_power_broker::ElementControl> element_control =
      fidl::CreateEndpoints<fuchsia_power_broker::ElementControl>().value();
  fidl::Endpoints<fuchsia_power_broker::ElementRunner> element_runner =
      fidl::CreateEndpoints<fuchsia_power_broker::ElementRunner>().value();

  fdf_power::ElementDesc desc = fdf_power::ElementDescBuilder(config, std::move(tokens))
                                    .SetAssertiveToken(active_event.borrow())
                                    .SetLessor(std::move(lessor.server))
                                    .SetElementControl(std::move(element_control.server))
                                    .SetElementRunner(std::move(element_runner.client))
                                    .Build();

  ASSERT_TRUE(desc.lessor_server.is_valid());
  ASSERT_TRUE(desc.element_control_server.is_valid());
  ASSERT_TRUE(desc.element_runner_client->is_valid());

  ASSERT_TRUE(desc.assertive_token.is_valid());

  ASSERT_EQ(desc.lessor_client, std::nullopt);
  ASSERT_EQ(desc.element_control_client, std::nullopt);

  check_channels_peered(lessor.client.handle()->get(), desc.lessor_server.handle()->get());
  check_channels_peered(element_control.client.handle()->get(),
                        desc.element_control_server.handle()->get());
  check_channels_peered(element_runner.server.handle()->get(),
                        desc.element_runner_client->handle()->get());
}

TEST_F(ElementBuilderTests, ElementBuilderMin) {
  fdf_power::PowerElementConfiguration config;
  fdf_power::TokenMap tokens;
  fdf_power::ElementDesc desc = fdf_power::ElementDescBuilder(config, std::move(tokens)).Build();

  ASSERT_NE(desc.lessor_client, std::nullopt);
  ASSERT_NE(desc.element_control_client, std::nullopt);

  ASSERT_TRUE(desc.lessor_server.is_valid());
  ASSERT_TRUE(desc.element_control_server.is_valid());

  ASSERT_TRUE(desc.assertive_token.is_valid());

  ASSERT_TRUE(desc.element_runner_server->is_valid());
  ASSERT_TRUE(desc.element_runner_client->is_valid());

  check_channels_peered(desc.element_control_client->handle()->get(),
                        desc.element_control_server.handle()->get());
  check_channels_peered(desc.element_runner_server->handle()->get(),
                        desc.element_runner_client->handle()->get());
}

}  // namespace power_lib_test

#endif
