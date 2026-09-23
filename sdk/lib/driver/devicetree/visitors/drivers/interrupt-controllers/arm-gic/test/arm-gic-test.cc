// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver/devicetree/testing/visitor-test-helper.h>
#include <lib/driver/devicetree/visitors/default/bind-property/bind-property.h>
#include <lib/driver/devicetree/visitors/registry.h>
#include <zircon/assert.h>

#include <cstdint>
#include <memory>
#include <string_view>

#include <gtest/gtest.h>

#include "../arm-gic-visitor.h"
#include "dts/interrupts.h"

namespace arm_gic_dt {
namespace {

using ArmGicVisitorType = fdf_devicetree::testing::VisitorTestHelper<ArmGicVisitor>;
class ArmGicVisitorTester : public ArmGicVisitorType {
 public:
  explicit ArmGicVisitorTester(std::string_view dtb_path)
      : ArmGicVisitorType(dtb_path, "ArmGicV2VisitorTest") {}
};

class ArmGicVisitorTest : public testing::Test {
 protected:
  void SetUp() final {
    visitors_ = std::make_unique<fdf_devicetree::VisitorRegistry>();
    ASSERT_TRUE(visitors_->RegisterVisitor(std::make_unique<fdf_devicetree::BindPropertyVisitor>())
                    .is_ok());

    auto tester = std::make_unique<ArmGicVisitorTester>("/pkg/test-data/interrupts.dtb");
    irq_tester_ = tester.get();
    ASSERT_TRUE(visitors_->RegisterVisitor(std::move(tester)).is_ok());

    ASSERT_EQ(ZX_OK, irq_tester_->manager()->Walk(*visitors_).status_value());
    ASSERT_TRUE(irq_tester_->DoPublish().is_ok());
  }

  ArmGicVisitorTester* irq_tester() { return irq_tester_; }

 private:
  std::unique_ptr<fdf_devicetree::VisitorRegistry> visitors_;
  ArmGicVisitorTester* irq_tester_ = nullptr;
};

TEST_F(ArmGicVisitorTest, TestInterruptProperty1) {
  auto nodes = irq_tester()->GetPbusNodes("sample-device-1");
  ASSERT_EQ(1lu, nodes.size());
  auto irq = nodes[0].irq();
  ASSERT_TRUE(irq);
  ASSERT_EQ(2lu, irq->size());
  ASSERT_TRUE((*irq)[0].irq().has_value());
  ASSERT_TRUE((*irq)[0].irq()->irq().has_value());
  EXPECT_EQ(static_cast<uint32_t>(IRQ1_SPI) + 32, (*irq)[0].irq()->irq().value());
  ASSERT_TRUE((*irq)[1].irq().has_value());
  ASSERT_TRUE((*irq)[1].irq()->irq().has_value());
  EXPECT_EQ(static_cast<uint32_t>(IRQ2_PPI) + 16, (*irq)[1].irq()->irq().value());
  EXPECT_EQ(static_cast<uint32_t>(IRQ1_MODE_FUCHSIA), static_cast<uint32_t>(*(*irq)[0].mode()));
  EXPECT_EQ(static_cast<uint32_t>(IRQ2_MODE_FUCHSIA), static_cast<uint32_t>(*(*irq)[1].mode()));
  ASSERT_TRUE((*irq)[0].name().has_value());
  EXPECT_EQ("interrupt-first", *(*irq)[0].name());
  ASSERT_TRUE((*irq)[1].name().has_value());
  EXPECT_EQ("interrupt-second", *(*irq)[1].name());
}

TEST_F(ArmGicVisitorTest, TestInterruptProperty2) {
  auto nodes = irq_tester()->GetPbusNodes("sample-device-2");
  ASSERT_EQ(1lu, nodes.size());
  auto irq = nodes[0].irq();
  ASSERT_TRUE(irq);
  ASSERT_EQ(2lu, irq->size());
  ASSERT_TRUE((*irq)[0].irq().has_value());
  ASSERT_TRUE((*irq)[0].irq()->irq().has_value());
  EXPECT_EQ(static_cast<uint32_t>(IRQ3_SPI) + 32, (*irq)[0].irq()->irq().value());
  ASSERT_TRUE((*irq)[1].irq().has_value());
  ASSERT_TRUE((*irq)[1].irq()->irq().has_value());
  EXPECT_EQ(static_cast<uint32_t>(IRQ4_PPI) + 16, (*irq)[1].irq()->irq().value());
  EXPECT_EQ(static_cast<uint32_t>(IRQ3_MODE_FUCHSIA), static_cast<uint32_t>(*(*irq)[0].mode()));
  EXPECT_EQ(static_cast<uint32_t>(IRQ4_MODE_FUCHSIA), static_cast<uint32_t>(*(*irq)[1].mode()));
  ASSERT_TRUE((*irq)[0].name().has_value());
  EXPECT_EQ("interrupt-first", *(*irq)[0].name());
  ASSERT_TRUE((*irq)[1].name().has_value());
  EXPECT_EQ("interrupt-second", *(*irq)[1].name());
}

TEST_F(ArmGicVisitorTest, TestInterruptProperty3) {
  auto nodes = irq_tester()->GetPbusNodes("sample-device-3");
  ASSERT_EQ(1lu, nodes.size());
  auto irq = nodes[0].irq();
  ASSERT_TRUE(irq);
  ASSERT_EQ(2lu, irq->size());
  ASSERT_TRUE((*irq)[0].irq().has_value());
  ASSERT_TRUE((*irq)[0].irq()->irq().has_value());
  EXPECT_EQ(static_cast<uint32_t>(IRQ5_SPI) + 32, (*irq)[0].irq()->irq().value());
  ASSERT_TRUE((*irq)[1].irq().has_value());
  ASSERT_TRUE((*irq)[1].irq()->irq().has_value());
  EXPECT_EQ(static_cast<uint32_t>(IRQ6_SPI) + 32, (*irq)[1].irq()->irq().value());
  EXPECT_EQ(static_cast<uint32_t>(IRQ5_MODE_FUCHSIA), static_cast<uint32_t>(*(*irq)[0].mode()));
  EXPECT_EQ(static_cast<uint32_t>(IRQ6_MODE_FUCHSIA), static_cast<uint32_t>(*(*irq)[1].mode()));
}

TEST_F(ArmGicVisitorTest, WakeVectorsWithoutInterruptNames) {
  auto nodes = irq_tester()->GetPbusNodes("wake-vectors-without-names");
  ASSERT_EQ(1lu, nodes.size());
  std::vector<fuchsia_hardware_platform_bus::Irq> irqs = (*nodes[0].irq());
  ASSERT_EQ(irqs.size(), 2u);
  for (const auto& irq : irqs) {
    EXPECT_FALSE(irq.wake_vector().value_or(false));
  }
}

TEST_F(ArmGicVisitorTest, WakeVectors) {
  auto nodes = irq_tester()->GetPbusNodes("wake-vectors");
  // We expect "wake-vectors" and "wake-vectors-without-names" in nodes.
  auto node_iter = std::find_if(nodes.begin(), nodes.end(),
                                [](const auto& node) { return node.name() == "wake-vectors"; });
  ASSERT_NE(node_iter, nodes.end());
  std::vector<fuchsia_hardware_platform_bus::Irq> irqs = (*node_iter->irq());
  ASSERT_EQ(irqs.size(), 2u);
  EXPECT_FALSE(irqs[0].wake_vector().value_or(false));
  EXPECT_TRUE(irqs[1].wake_vector().value_or(false));
}

TEST_F(ArmGicVisitorTest, TestInterruptProperty4) {
  auto nodes = irq_tester()->GetPbusNodes("sample-device-4");
  ASSERT_EQ(1lu, nodes.size());
  auto irq = nodes[0].irq();
  ASSERT_TRUE(irq);
  ASSERT_EQ(2lu, irq->size());
  ASSERT_TRUE((*irq)[0].irq().has_value());
  ASSERT_TRUE((*irq)[0].irq()->irq().has_value());
  EXPECT_EQ(static_cast<uint32_t>(IRQ7_SPI) + 32, (*irq)[0].irq()->irq().value());
  ASSERT_TRUE((*irq)[1].irq().has_value());
  ASSERT_TRUE((*irq)[1].irq()->irq().has_value());
  EXPECT_EQ(static_cast<uint32_t>(IRQ8_SPI) + 32, (*irq)[1].irq()->irq().value());
  EXPECT_EQ(static_cast<uint32_t>(IRQ7_MODE_FUCHSIA), static_cast<uint32_t>(*(*irq)[0].mode()));
  EXPECT_EQ(static_cast<uint32_t>(IRQ8_MODE_FUCHSIA), static_cast<uint32_t>(*(*irq)[1].mode()));
}

}  // namespace
}  // namespace arm_gic_dt
