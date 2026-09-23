// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "../fuchsia-config.h"

#include <fidl/fuchsia.driver.metadata/cpp/fidl.h>
#include <lib/driver/devicetree/testing/visitor-test-helper.h>
#include <lib/driver/devicetree/visitors/default/bind-property/bind-property.h>
#include <lib/driver/devicetree/visitors/registry.h>

#include <gtest/gtest.h>
namespace fdf_devicetree {

class FuchsiaConfigTester
    : public fdf_devicetree::testing::VisitorTestHelper<FuchsiaConfigVisitor> {
 public:
  FuchsiaConfigTester(std::string_view dtb_path)
      : fdf_devicetree::testing::VisitorTestHelper<FuchsiaConfigVisitor>(dtb_path,
                                                                         "FuchsiaConfigTest") {}
};

TEST(FuchsiaConfigTest, TestMetadataAndBindProperty) {
  fdf_devicetree::VisitorRegistry visitors;
  ASSERT_TRUE(
      visitors.RegisterVisitor(std::make_unique<fdf_devicetree::BindPropertyVisitor>()).is_ok());

  auto tester = std::make_unique<FuchsiaConfigTester>("/pkg/test-data/fuchsia-config.dtb");
  FuchsiaConfigTester* fuchsia_config_tester = tester.get();
  ASSERT_TRUE(visitors.RegisterVisitor(std::move(tester)).is_ok());

  ASSERT_EQ(ZX_OK, fuchsia_config_tester->manager()->Walk(visitors).status_value());
  ASSERT_TRUE(fuchsia_config_tester->DoPublish().is_ok());

  auto pbus_nodes = fuchsia_config_tester->GetPbusNodes("sample-device");
  ASSERT_EQ(pbus_nodes.size(), 1u);
  auto metadata = pbus_nodes[0].metadata();
  ASSERT_TRUE(metadata);
  ASSERT_EQ(1lu, metadata->size());

  std::vector<uint8_t> metadata_blob = std::move(*(*metadata)[0].data());
  fit::result dictionary = fidl::Unpersist<fuchsia_driver_metadata::Dictionary>(metadata_blob);
  ASSERT_TRUE(dictionary.is_ok());

  auto entries = dictionary->entries();
  ASSERT_TRUE(entries.has_value());

  auto find_entry = [&](const std::string& key) -> const fuchsia_driver_metadata::DictionaryValue* {
    for (const auto& entry : *entries) {
      if (entry.key() == key) {
        return &entry.value();
      }
    }
    return nullptr;
  };

  auto int_val = find_entry("int_prop");
  ASSERT_TRUE(int_val);
  ASSERT_TRUE(int_val->int64().has_value());
  EXPECT_EQ(int_val->int64().value(), 0x12345678);

  auto array_val = find_entry("array_prop");
  ASSERT_TRUE(array_val);
  ASSERT_TRUE(array_val->int64_vec().has_value());
  EXPECT_EQ(array_val->int64_vec()->size(), 4u);
  EXPECT_EQ(array_val->int64_vec()->at(0), 1);

  auto bool_val = find_entry("bool_prop");
  ASSERT_TRUE(bool_val);
  ASSERT_TRUE(bool_val->int64_vec().has_value());
  EXPECT_TRUE(bool_val->int64_vec()->empty());

  auto string_val = find_entry("string_prop");
  ASSERT_TRUE(string_val);
  ASSERT_TRUE(string_val->int64_vec().has_value());
  EXPECT_EQ(string_val->int64_vec()->size(), 2u);

  auto nested_val = find_entry("nested.prop");
  ASSERT_TRUE(nested_val);
  ASSERT_TRUE(nested_val->int64_vec().has_value());
}

}  // namespace fdf_devicetree
