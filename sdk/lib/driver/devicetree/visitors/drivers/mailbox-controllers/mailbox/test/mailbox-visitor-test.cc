// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "../mailbox-visitor.h"

#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/node_properties.h>
#include <lib/driver/devicetree/testing/visitor-test-helper.h>
#include <lib/driver/devicetree/visitors/default/bind-property/bind-property.h>
#include <lib/driver/devicetree/visitors/default/mmio/mmio.h>
#include <lib/driver/devicetree/visitors/registry.h>

#include <optional>
#include <string_view>

#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/mailbox/cpp/bind.h>
#include <gtest/gtest.h>

namespace {

std::optional<fuchsia_hardware_mailbox::ChannelInfo> FindChannel(
    uint32_t channel, const fuchsia_hardware_mailbox::ControllerInfo& controller_info) {
  auto it =
      std::find_if(controller_info.channels()->cbegin(), controller_info.channels()->cend(),
                   [channel](const fuchsia_hardware_mailbox::ChannelInfo& channel_info) -> bool {
                     return channel_info.channel() && channel_info.channel() == channel;
                   });
  if (it == controller_info.channels()->end()) {
    return std::nullopt;
  }
  return {*it};
}

}  // namespace

namespace mailbox_dt {

class MailboxVisitorTester : public fdf_devicetree::testing::VisitorTestHelper<MailboxVisitor> {
 public:
  explicit MailboxVisitorTester(std::string_view dtb_path)
      : fdf_devicetree::testing::VisitorTestHelper<MailboxVisitor>(dtb_path,
                                                                   "MailboxBusVisitorTest") {}
};

TEST(MailboxVisitorTest, TwoControllers) {
  fdf_devicetree::VisitorRegistry visitors;
  ASSERT_TRUE(
      visitors.RegisterVisitor(std::make_unique<fdf_devicetree::BindPropertyVisitor>()).is_ok());
  ASSERT_TRUE(visitors.RegisterVisitor(std::make_unique<fdf_devicetree::MmioVisitor>()).is_ok());

  MailboxVisitorTester* const mailbox_tester =
      new MailboxVisitorTester("/pkg/test-data/mailbox.dtb");
  ASSERT_TRUE(
      visitors.RegisterVisitor(std::unique_ptr<MailboxVisitorTester>{mailbox_tester}).is_ok());

  ASSERT_TRUE(mailbox_tester->manager()->Walk(visitors).is_ok());
  ASSERT_TRUE(mailbox_tester->DoPublish().is_ok());

  // First controller metadata
  auto pbus_node_0_list = mailbox_tester->GetPbusNodes("mailbox-abcd0000");
  ASSERT_EQ(1u, pbus_node_0_list.size());
  const auto& pbus_node_0 = pbus_node_0_list[0];

  ASSERT_TRUE(pbus_node_0.metadata());
  ASSERT_EQ(pbus_node_0.metadata()->size(), 1u);

  ASSERT_TRUE((*pbus_node_0.metadata())[0].id());
  EXPECT_EQ(*(*pbus_node_0.metadata())[0].id(),
            fuchsia_hardware_mailbox::ControllerInfo::kSerializableName);

  ASSERT_TRUE((*pbus_node_0.metadata())[0].data());
  const std::vector<uint8_t>& metadata_0 = *(*pbus_node_0.metadata())[0].data();

  const auto controller_0 = fidl::Unpersist<fuchsia_hardware_mailbox::ControllerInfo>(
      {metadata_0.data(), metadata_0.size()});
  ASSERT_TRUE(controller_0.is_ok());

  ASSERT_TRUE(controller_0->id());
  const uint32_t controller_0_id = *controller_0->id();

  ASSERT_TRUE(controller_0->channels());
  ASSERT_EQ(controller_0->channels()->size(), 2u);

  EXPECT_TRUE(FindChannel(0x1234, *controller_0));
  EXPECT_TRUE(FindChannel(0x5678, *controller_0));

  // Second controller metadata
  auto pbus_node_1_list = mailbox_tester->GetPbusNodes("mailbox-abce0000");
  ASSERT_EQ(1u, pbus_node_1_list.size());
  const auto& pbus_node_1 = pbus_node_1_list[0];

  ASSERT_TRUE(pbus_node_1.metadata());
  ASSERT_EQ(pbus_node_1.metadata()->size(), 1u);

  ASSERT_TRUE((*pbus_node_1.metadata())[0].id());
  EXPECT_EQ(*(*pbus_node_1.metadata())[0].id(),
            fuchsia_hardware_mailbox::ControllerInfo::kSerializableName);

  ASSERT_TRUE((*pbus_node_1.metadata())[0].data());
  const std::vector<uint8_t>& metadata_1 = *(*pbus_node_1.metadata())[0].data();

  const auto controller_1 = fidl::Unpersist<fuchsia_hardware_mailbox::ControllerInfo>(
      {metadata_1.data(), metadata_1.size()});
  ASSERT_TRUE(controller_1.is_ok());

  ASSERT_TRUE(controller_1->id());
  const uint32_t controller_1_id = *controller_1->id();

  ASSERT_TRUE(controller_1->channels());
  ASSERT_EQ(controller_1->channels()->size(), 2u);

  EXPECT_TRUE(FindChannel(0x9abc, *controller_1));
  EXPECT_TRUE(FindChannel(0x1234, *controller_1));

  // First client composite node specs
  auto client_0_list = mailbox_tester->GetCompositeNodeSpecs("node-abcf0000");
  ASSERT_EQ(1u, client_0_list.size());
  const auto& client_0 = client_0_list[0];

  ASSERT_TRUE(client_0.parents2());
  ASSERT_EQ(client_0.parents2()->size(), 4u);

  // The 0th composite parent has the `compatible` string and is added by the default visitor.
  EXPECT_TRUE(fdf_devicetree::testing::CheckHasBindRules(
      {
          fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.mailbox.Service"),
          fdf::MakeAcceptBindRule(bind_fuchsia_mailbox::CONTROLLER_ID, controller_0_id),
          fdf::MakeAcceptBindRule(bind_fuchsia::ID, 0x1234u),
      },
      (*client_0.parents2())[1].bind_rules(), false));
  EXPECT_TRUE(fdf_devicetree::testing::CheckHasProperties(
      {
          fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.mailbox.Service"),
          fdf::MakeProperty2(bind_fuchsia::ID, 0x1234u),
          fdf::MakeProperty2(bind_fuchsia::NAME, "mailbox-1-1234"),
      },
      (*client_0.parents2())[1].properties(), false));

  EXPECT_TRUE(fdf_devicetree::testing::CheckHasBindRules(
      {
          fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.mailbox.Service"),
          fdf::MakeAcceptBindRule(bind_fuchsia_mailbox::CONTROLLER_ID, controller_0_id),
          fdf::MakeAcceptBindRule(bind_fuchsia::ID, 0x5678u),
      },
      (*client_0.parents2())[2].bind_rules(), false));
  EXPECT_TRUE(fdf_devicetree::testing::CheckHasProperties(
      {
          fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.mailbox.Service"),
          fdf::MakeProperty2(bind_fuchsia::ID, 0x5678u),
          fdf::MakeProperty2(bind_fuchsia::NAME, "mailbox-1-5678"),
      },
      (*client_0.parents2())[2].properties(), false));

  EXPECT_TRUE(fdf_devicetree::testing::CheckHasBindRules(
      {
          fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.mailbox.Service"),
          fdf::MakeAcceptBindRule(bind_fuchsia_mailbox::CONTROLLER_ID, controller_1_id),
          fdf::MakeAcceptBindRule(bind_fuchsia::ID, 0x9abcu),
      },
      (*client_0.parents2())[3].bind_rules(), false));
  EXPECT_TRUE(fdf_devicetree::testing::CheckHasProperties(
      {
          fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.mailbox.Service"),
          fdf::MakeProperty2(bind_fuchsia::ID, 0x9abcu),
          fdf::MakeProperty2(bind_fuchsia::NAME, "mailbox-2-9abc"),
      },
      (*client_0.parents2())[3].properties(), false));

  // Second client composite node specs
  auto client_1_list = mailbox_tester->GetCompositeNodeSpecs("node-abd00000");
  ASSERT_EQ(1u, client_1_list.size());
  const auto& client_1 = client_1_list[0];

  ASSERT_TRUE(client_1.parents2());
  ASSERT_EQ(client_1.parents2()->size(), 2u);

  EXPECT_TRUE(fdf_devicetree::testing::CheckHasBindRules(
      {
          fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.mailbox.Service"),
          fdf::MakeAcceptBindRule(bind_fuchsia_mailbox::CONTROLLER_ID, controller_1_id),
          fdf::MakeAcceptBindRule(bind_fuchsia::ID, 0x1234u),
      },
      (*client_1.parents2())[1].bind_rules(), false));
  EXPECT_TRUE(fdf_devicetree::testing::CheckHasProperties(
      {
          fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.mailbox.Service"),
          fdf::MakeProperty2(bind_fuchsia::ID, 0x1234u),
          fdf::MakeProperty2(bind_fuchsia::NAME, "mailbox"),
      },
      (*client_1.parents2())[1].properties(), false));
}

TEST(MailboxVisitorTest, TwoCells) {
  fdf_devicetree::VisitorRegistry visitors;
  ASSERT_TRUE(
      visitors.RegisterVisitor(std::make_unique<fdf_devicetree::BindPropertyVisitor>()).is_ok());
  ASSERT_TRUE(visitors.RegisterVisitor(std::make_unique<fdf_devicetree::MmioVisitor>()).is_ok());

  MailboxVisitorTester* const mailbox_tester =
      new MailboxVisitorTester("/pkg/test-data/mailbox.dtb");
  ASSERT_TRUE(
      visitors.RegisterVisitor(std::unique_ptr<MailboxVisitorTester>{mailbox_tester}).is_ok());

  ASSERT_TRUE(mailbox_tester->manager()->Walk(visitors).is_ok());
  ASSERT_TRUE(mailbox_tester->DoPublish().is_ok());

  // Third controller metadata
  auto pbus_node_2_list = mailbox_tester->GetPbusNodes("mailbox-abcf1000");
  ASSERT_EQ(1u, pbus_node_2_list.size());
  const auto& pbus_node_2 = pbus_node_2_list[0];

  ASSERT_TRUE(pbus_node_2.metadata());
  ASSERT_EQ(pbus_node_2.metadata()->size(), 1u);

  ASSERT_TRUE((*pbus_node_2.metadata())[0].data());
  const std::vector<uint8_t>& metadata_2 = *(*pbus_node_2.metadata())[0].data();

  const auto controller_2 = fidl::Unpersist<fuchsia_hardware_mailbox::ControllerInfo>(
      {metadata_2.data(), metadata_2.size()});
  ASSERT_TRUE(controller_2.is_ok());

  ASSERT_TRUE(controller_2->channels());
  ASSERT_EQ(controller_2->channels()->size(), 1u);

  auto channel = (*controller_2->channels())[0];
  EXPECT_EQ(channel.channel(), 0x1111u);
  EXPECT_EQ(channel.client(), 0x2222u);

  // Third client composite node specs
  auto client_2_list = mailbox_tester->GetCompositeNodeSpecs("node-abd10000");
  ASSERT_EQ(1u, client_2_list.size());
  const auto& client_2 = client_2_list[0];

  ASSERT_TRUE(client_2.parents2());
  ASSERT_EQ(client_2.parents2()->size(), 2u);

  EXPECT_TRUE(fdf_devicetree::testing::CheckHasBindRules(
      {
          fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.mailbox.Service"),
          fdf::MakeAcceptBindRule(bind_fuchsia_mailbox::CONTROLLER_ID, *controller_2->id()),
          fdf::MakeAcceptBindRule(bind_fuchsia::ID, 0x1111u),
          fdf::MakeAcceptBindRule(bind_fuchsia_mailbox::CLIENT, 0x2222u),
      },
      (*client_2.parents2())[1].bind_rules(), false));
  EXPECT_TRUE(fdf_devicetree::testing::CheckHasProperties(
      {
          fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.mailbox.Service"),
          fdf::MakeProperty2(bind_fuchsia::ID, 0x1111u),
          fdf::MakeProperty2(bind_fuchsia::NAME, "mailbox-3-1111-2222"),
      },
      (*client_2.parents2())[1].properties(), false));
}

}  // namespace mailbox_dt
