// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <assert.h>
#include <lib/hid-parser/item.h>
#include <lib/hid-parser/parser.h>
#include <lib/hid-parser/report.h>
#include <lib/hid-parser/units.h>
#include <lib/hid-parser/usages.h>
#include <lib/hid/boot.h>
#include <stdio.h>
#include <unistd.h>

#include <gtest/gtest.h>

#include "src/ui/input/lib/hid-parser/test/hid-report-data.h"
#include "src/ui/input/lib/hid-parser/test/util.h"

namespace {
struct Stats {
  int input_count;
  int collection[2];
};

size_t ItemizeHIDReportDesc(const uint8_t* rpt_desc, size_t desc_len, Stats* stats) {
  const uint8_t* buf = rpt_desc;
  size_t len = desc_len;
  while (len > 0) {
    size_t actual = 0;
    auto item = hid::Item::ReadNext(buf, len, &actual);
    if ((actual > len) || (actual == 0))
      break;

    if (item.tag() == hid::Item::Tag::kEndCollection)
      stats->collection[1]++;
    else if (item.tag() == hid::Item::Tag::kCollection)
      stats->collection[0]++;

    if (item.type() == hid::Item::Type::kMain && item.tag() == hid::Item::Tag::kInput)
      stats->input_count++;

    len -= actual;
    buf += actual;
  }

  return (desc_len - len);
}

TEST(HidHelperTest, ItemizeAcer12Rpt1) {
  Stats stats = {};
  size_t len = kAcer12TouchDescriptor.size();
  size_t consumed = ItemizeHIDReportDesc(kAcer12TouchDescriptor.data(), len, &stats);

  ASSERT_EQ(consumed, len);
  ASSERT_EQ(stats.input_count, 45);
  ASSERT_EQ(stats.collection[0], 13);
  ASSERT_EQ(stats.collection[1], 13);
}

TEST(HidHelperTest, ItemizeEveTabletRpt) {
  Stats stats = {};
  size_t len = kEveTabletDescriptor.size();
  size_t consumed = ItemizeHIDReportDesc(kEveTabletDescriptor.data(), len, &stats);

  ASSERT_EQ(consumed, len);
  ASSERT_EQ(stats.input_count, 2);
  ASSERT_EQ(stats.collection[0], 1);
  ASSERT_EQ(stats.collection[1], 1);
}

TEST(HidHelperTest, ParseBootMouse) {
  hid::DeviceDescriptor* dev = nullptr;
  size_t len;
  const uint8_t* report_desc = get_boot_mouse_report_desc(&len);
  hid::ParseResult res = hid::ParseReportDescriptor(report_desc, len, &dev);

  ASSERT_EQ(res, hid::ParseResult::kParseOk);

  // A single report with id zero, this means no report id.
  ASSERT_EQ(dev->rep_count, 1u);
  EXPECT_EQ(dev->report[0].report_id, 0u);

  // The only report has 6 fields and is 32 bits long.
  EXPECT_EQ(dev->report[0].input_count, 6u);
  EXPECT_EQ(dev->report[0].input_byte_sz, 3u);
  const auto fields = dev->report[0].input_fields;

  // All fields are input type with report id = 0.
  for (uint8_t ix = 0; ix != dev->report[0].input_count; ++ix) {
    EXPECT_EQ(fields[ix].report_id, 0u);
    EXPECT_EQ(fields[ix].type, hid::kInput);
  }

  // First 3 fields are the buttons, with usages 1, 2, 3, in the button page.
  auto expected_flags = hid::kData | hid::kAbsolute | hid::kScalar;

  for (uint8_t ix = 0; ix != 3; ++ix) {
    EXPECT_EQ(fields[ix].attr.usage.page, ToUnderlyingType(hid::usage::Page::kButton));
    EXPECT_EQ(fields[ix].attr.usage.usage, uint32_t{ix + 1u});
    EXPECT_EQ(fields[ix].attr.bit_sz, 1u);
    EXPECT_EQ(fields[ix].attr.offset, ix);
    EXPECT_EQ(fields[ix].attr.logc_mm.min, 0);
    EXPECT_EQ(fields[ix].attr.logc_mm.max, 1);
    EXPECT_EQ(expected_flags & fields[ix].flags, expected_flags);
  }

  // Next field is 5 bits constant. Aka padding.
  EXPECT_EQ(fields[3].attr.bit_sz, 5u);
  EXPECT_EQ(fields[3].attr.offset, 3u);
  EXPECT_EQ(hid::kConstant & fields[3].flags, hid::kConstant);

  // Next comes 'X' field, 8 bits data, relative.
  expected_flags = hid::kData | hid::kRelative | hid::kScalar;

  EXPECT_EQ(fields[4].attr.usage.page, ToUnderlyingType(hid::usage::Page::kGenericDesktop));
  EXPECT_EQ(fields[4].attr.usage.usage, ToUnderlyingType(hid::usage::GenericDesktop::kX));
  EXPECT_EQ(fields[4].attr.bit_sz, 8u);
  EXPECT_EQ(fields[4].attr.offset, 8u);
  EXPECT_EQ(fields[4].attr.logc_mm.min, -127);
  EXPECT_EQ(fields[4].attr.logc_mm.max, 127);
  EXPECT_EQ(fields[4].attr.phys_mm.min, -127);
  EXPECT_EQ(fields[4].attr.phys_mm.max, 127);
  EXPECT_EQ(expected_flags & fields[4].flags, expected_flags);

  // Last comes 'Y' field, same as 'X'.
  EXPECT_EQ(fields[5].attr.usage.page, ToUnderlyingType(hid::usage::Page::kGenericDesktop));
  EXPECT_EQ(fields[5].attr.usage.usage, ToUnderlyingType(hid::usage::GenericDesktop::kY));
  EXPECT_EQ(fields[5].attr.bit_sz, 8u);
  EXPECT_EQ(fields[5].attr.offset, 16u);
  EXPECT_EQ(fields[5].attr.logc_mm.min, -127);
  EXPECT_EQ(fields[5].attr.logc_mm.max, 127);
  EXPECT_EQ(fields[5].attr.phys_mm.min, -127);
  EXPECT_EQ(fields[5].attr.phys_mm.max, 127);
  EXPECT_EQ(expected_flags & fields[4].flags, expected_flags);

  // Now test the collections.
  // Inner collection is physical GeneticDesktop|Pointer.
  auto collection = fields[0].col;
  ASSERT_TRUE(collection != nullptr);
  EXPECT_EQ(collection->type, hid::CollectionType::kPhysical);
  EXPECT_EQ(collection->usage.page, ToUnderlyingType(hid::usage::Page::kGenericDesktop));
  EXPECT_EQ(collection->usage.usage, ToUnderlyingType(hid::usage::GenericDesktop::kPointer));

  // Outer collection is the application.
  collection = collection->parent;
  ASSERT_TRUE(collection != nullptr);
  EXPECT_EQ(collection->type, hid::CollectionType::kApplication);
  EXPECT_EQ(collection->usage.page, ToUnderlyingType(hid::usage::Page::kGenericDesktop));
  EXPECT_EQ(collection->usage.usage, ToUnderlyingType(hid::usage::GenericDesktop::kMouse));

  // No parent collection.
  EXPECT_TRUE(collection->parent == nullptr);

  // Test the helpers.
  auto app_col = hid::GetAppCollection(&dev->report[0].input_fields[0]);
  EXPECT_EQ(app_col, collection);

  hid::FreeDeviceDescriptor(dev);
}

TEST(HidHelperTest, ParseHpMouse) {
  hid::DeviceDescriptor* dev = nullptr;
  hid::ParseResult res =
      hid::ParseReportDescriptor(kHpMouseDescriptor.data(), kHpMouseDescriptor.size(), &dev);

  ASSERT_EQ(res, hid::ParseResult::kParseOk);

  // A single report with id zero, this means no report id.
  ASSERT_EQ(dev->rep_count, 1u);
  EXPECT_EQ(dev->report[0].report_id, 0u);

  // The only report has 11 fields.
  EXPECT_EQ(dev->report[0].input_count, 11u);
  const auto fields = dev->report[0].input_fields;

  // All fields are input type with report id = 0.
  for (uint8_t ix = 0; ix != dev->report[0].input_count; ++ix) {
    EXPECT_EQ(fields[ix].report_id, 0u);
    EXPECT_EQ(fields[ix].type, hid::kInput);
  }

  // First 8 fields are the buttons, with usages 1, 2, 3, 3 .. 3 in the button page.
  auto expected_flags = hid::kData | hid::kAbsolute | hid::kScalar;

  for (uint8_t ix = 0; ix != 8; ++ix) {
    auto usage = (ix < 3) ? uint32_t{ix + 1u} : uint32_t{3};
    EXPECT_EQ(fields[ix].attr.usage.page, ToUnderlyingType(hid::usage::Page::kButton));
    EXPECT_EQ(fields[ix].attr.usage.usage, usage);
    EXPECT_EQ(fields[ix].attr.bit_sz, 1u);
    EXPECT_EQ(fields[ix].attr.logc_mm.min, 0);
    EXPECT_EQ(fields[ix].attr.logc_mm.max, 1);
    EXPECT_EQ(expected_flags & fields[ix].flags, expected_flags);
  }

  // Next comes 'X' field, 8 bits data, relative.
  expected_flags = hid::kData | hid::kRelative | hid::kScalar;

  EXPECT_EQ(fields[8].attr.usage.page, ToUnderlyingType(hid::usage::Page::kGenericDesktop));
  EXPECT_EQ(fields[8].attr.usage.usage, ToUnderlyingType(hid::usage::GenericDesktop::kX));
  EXPECT_EQ(fields[8].attr.bit_sz, 8u);
  EXPECT_EQ(fields[8].attr.logc_mm.min, -127);
  EXPECT_EQ(fields[8].attr.logc_mm.max, 127);
  EXPECT_EQ(fields[8].attr.phys_mm.min, -127);
  EXPECT_EQ(fields[8].attr.phys_mm.max, 127);
  EXPECT_EQ(expected_flags & fields[8].flags, expected_flags);

  // Next comes 'Y' field, same as 'X'.
  EXPECT_EQ(fields[9].attr.usage.page, ToUnderlyingType(hid::usage::Page::kGenericDesktop));
  EXPECT_EQ(fields[9].attr.usage.usage, ToUnderlyingType(hid::usage::GenericDesktop::kY));
  EXPECT_EQ(fields[9].attr.bit_sz, 8u);
  EXPECT_EQ(fields[9].attr.logc_mm.min, -127);
  EXPECT_EQ(fields[9].attr.logc_mm.max, 127);
  EXPECT_EQ(fields[9].attr.phys_mm.min, -127);
  EXPECT_EQ(fields[9].attr.phys_mm.max, 127);
  EXPECT_EQ(expected_flags & fields[9].flags, expected_flags);

  // Last comes 'Wheel' field.
  EXPECT_EQ(fields[10].attr.usage.page, ToUnderlyingType(hid::usage::Page::kGenericDesktop));
  EXPECT_EQ(fields[10].attr.usage.usage, ToUnderlyingType(hid::usage::GenericDesktop::kWheel));
  EXPECT_EQ(fields[10].attr.bit_sz, 8u);
  EXPECT_EQ(fields[10].attr.logc_mm.min, -127);
  EXPECT_EQ(fields[10].attr.logc_mm.max, 127);
  EXPECT_EQ(fields[10].attr.phys_mm.min, -127);
  EXPECT_EQ(fields[10].attr.phys_mm.max, 127);
  EXPECT_EQ(expected_flags & fields[10].flags, expected_flags);

  // Now test the collections.
  // Inner collection is physical GeneticDesktop|Pointer.
  auto collection = fields[0].col;
  ASSERT_TRUE(collection != nullptr);
  EXPECT_EQ(collection->type, hid::CollectionType::kPhysical);
  EXPECT_EQ(collection->usage.page, ToUnderlyingType(hid::usage::Page::kGenericDesktop));
  EXPECT_EQ(collection->usage.usage, ToUnderlyingType(hid::usage::GenericDesktop::kPointer));

  // Outer collection is the application.
  collection = collection->parent;
  ASSERT_TRUE(collection != nullptr);
  EXPECT_EQ(collection->type, hid::CollectionType::kApplication);
  EXPECT_EQ(collection->usage.page, ToUnderlyingType(hid::usage::Page::kGenericDesktop));
  EXPECT_EQ(collection->usage.usage, ToUnderlyingType(hid::usage::GenericDesktop::kMouse));

  // No parent collection.
  EXPECT_TRUE(collection->parent == nullptr);

  hid::FreeDeviceDescriptor(dev);
}

TEST(HidHelperTest, ParseAdafTrinket) {
  hid::DeviceDescriptor* dev = nullptr;
  hid::ParseResult res = hid::ParseReportDescriptor(kAdafruitTrinketDescriptor.data(),
                                                    kAdafruitTrinketDescriptor.size(), &dev);

  ASSERT_EQ(res, hid::ParseResult::kParseOk);

  // Four different reports
  ASSERT_EQ(dev->rep_count, 4u);

  //////////////////////////////////////////////////////////////////////////////////
  // First report is the same as boot_mouse, except for the report id.
  EXPECT_EQ(dev->report[0].report_id, 1u);
  ASSERT_EQ(dev->report[0].input_count, 6u);
  EXPECT_EQ(dev->report[0].input_byte_sz, 4u);
  const hid::ReportField* fields = dev->report[0].input_fields;

  // All fields are scalar input type with report id = 1.
  for (uint8_t ix = 0; ix != dev->report[0].input_count; ++ix) {
    EXPECT_EQ(fields[ix].report_id, 1u);
    EXPECT_EQ(fields[ix].type, hid::kInput);
    EXPECT_EQ(hid::kScalar & fields[ix].flags, hid::kScalar);
  }

  // First 3 fields are the buttons, with usages 1, 2, 3, in the button page.
  auto expected_flags = hid::kData | hid::kAbsolute;

  for (uint8_t ix = 0; ix != 3; ++ix) {
    EXPECT_EQ(fields[ix].attr.usage.page, ToUnderlyingType(hid::usage::Page::kButton));
    EXPECT_EQ(fields[ix].attr.usage.usage, uint32_t{ix + 1u});
    EXPECT_EQ(fields[ix].attr.bit_sz, 1u);
    EXPECT_EQ(fields[ix].attr.offset, 8u + ix);
    EXPECT_EQ(fields[ix].attr.logc_mm.min, 0);
    EXPECT_EQ(fields[ix].attr.logc_mm.max, 1);
    EXPECT_EQ(expected_flags & fields[ix].flags, expected_flags);
  }

  // Next field is 5 bits constant. Aka padding.
  EXPECT_EQ(fields[3].attr.bit_sz, 5u);
  EXPECT_EQ(hid::kConstant & fields[3].flags, hid::kConstant);

  // Next comes 'X' field, 8 bits data, relative.
  expected_flags = hid::kData | hid::kRelative;

  EXPECT_EQ(fields[4].attr.usage.page, ToUnderlyingType(hid::usage::Page::kGenericDesktop));
  EXPECT_EQ(fields[4].attr.usage.usage, ToUnderlyingType(hid::usage::GenericDesktop::kX));
  EXPECT_EQ(fields[4].attr.bit_sz, 8u);
  EXPECT_EQ(fields[4].attr.offset, 16u);
  EXPECT_EQ(fields[4].attr.logc_mm.min, -127);
  EXPECT_EQ(fields[4].attr.logc_mm.max, 127);
  EXPECT_EQ(expected_flags & fields[4].flags, expected_flags);

  // Last comes 'Y' field, same as 'X'.
  EXPECT_EQ(fields[5].attr.usage.page, ToUnderlyingType(hid::usage::Page::kGenericDesktop));
  EXPECT_EQ(fields[5].attr.usage.usage, ToUnderlyingType(hid::usage::GenericDesktop::kY));
  EXPECT_EQ(fields[5].attr.bit_sz, 8u);
  EXPECT_EQ(fields[5].attr.offset, 24u);
  EXPECT_EQ(fields[5].attr.logc_mm.min, -127);
  EXPECT_EQ(fields[5].attr.logc_mm.max, 127);
  EXPECT_EQ(expected_flags & fields[4].flags, expected_flags);

  // Now test the collections.
  // Inner collection is physical GeneticDesktop|Pointer.
  auto collection = fields[0].col;
  ASSERT_TRUE(collection != nullptr);
  EXPECT_EQ(collection->type, hid::CollectionType::kPhysical);
  EXPECT_EQ(collection->usage.page, ToUnderlyingType(hid::usage::Page::kGenericDesktop));
  EXPECT_EQ(collection->usage.usage, ToUnderlyingType(hid::usage::GenericDesktop::kPointer));

  // Outer collection is the application.
  collection = collection->parent;
  ASSERT_TRUE(collection != nullptr);
  EXPECT_EQ(collection->type, hid::CollectionType::kApplication);
  EXPECT_EQ(collection->usage.page, ToUnderlyingType(hid::usage::Page::kGenericDesktop));
  EXPECT_EQ(collection->usage.usage, ToUnderlyingType(hid::usage::GenericDesktop::kMouse));

  // No parent collection.
  EXPECT_TRUE(collection->parent == nullptr);

  //////////////////////////////////////////////////////////////////////////////////
  // Second  report is a keyboard with 20 fields and is 72 bits long.
  EXPECT_EQ(dev->report[1].report_id, 2u);
  ASSERT_EQ(dev->report[1].input_count, 14u);
  EXPECT_EQ(dev->report[1].input_byte_sz, 8u);
  ASSERT_EQ(dev->report[1].output_count, 6u);
  EXPECT_EQ(dev->report[1].output_byte_sz, 2u);

  const hid::ReportField* output_fields = dev->report[1].output_fields;
  fields = dev->report[1].input_fields;

  // First 8 are input bits with usages 0xe0 to 0xe7 on the keyboard page.
  expected_flags = hid::kData | hid::kAbsolute | hid::kScalar;

  for (uint8_t ix = 0; ix != 8; ++ix) {
    EXPECT_EQ(fields[ix].type, hid::kInput);
    EXPECT_EQ(fields[ix].attr.usage.page, ToUnderlyingType(hid::usage::Page::kKeyboardKeypad));
    EXPECT_EQ(fields[ix].attr.usage.usage, uint32_t{ix + 0xe0u});
    EXPECT_EQ(fields[ix].attr.bit_sz, 1u);
    EXPECT_EQ(fields[ix].attr.offset, 8u + ix);
    EXPECT_EQ(fields[ix].attr.logc_mm.min, 0);
    EXPECT_EQ(fields[ix].attr.logc_mm.max, 1);
    EXPECT_EQ(expected_flags & fields[ix].flags, expected_flags);
  }

  // Next field is 8 bits padding (input).
  EXPECT_EQ(fields[8].attr.bit_sz, 8u);
  EXPECT_EQ(fields[8].attr.offset, 16u);
  EXPECT_EQ(fields[8].type, hid::kInput);
  EXPECT_EQ(hid::kConstant & fields[8].flags, hid::kConstant);

  // Next 5 fields are byte-sized key input array.
  expected_flags = hid::kData | hid::kAbsolute | hid::kArray;

  for (uint8_t ix = 9; ix != 14; ++ix) {
    EXPECT_EQ(fields[ix].type, hid::kInput);
    EXPECT_EQ(fields[ix].attr.usage.page, ToUnderlyingType(hid::usage::Page::kKeyboardKeypad));
    EXPECT_EQ(fields[ix].attr.bit_sz, 8u);
    EXPECT_EQ(fields[ix].attr.offset, 24u + 8u * (ix - 9u));
    EXPECT_EQ(fields[ix].attr.usage.usage, 0u);
    EXPECT_EQ(fields[ix].attr.logc_mm.min, 0);
    EXPECT_EQ(fields[ix].attr.logc_mm.max, 164);
    EXPECT_EQ(expected_flags & fields[ix].flags, expected_flags);
  }

  // Test the output fields (LED bits output, with usages NumLock(1) to Kana(5)).
  auto led_usage = ToUnderlyingType(hid::usage::LEDs::kNumLock);
  expected_flags = hid::kData | hid::kAbsolute | hid::kScalar;

  for (uint8_t ix = 0; ix != 5; ++ix) {
    EXPECT_EQ(output_fields[ix].type, hid::kOutput);
    EXPECT_EQ(output_fields[ix].attr.usage.page, ToUnderlyingType(hid::usage::Page::kLEDs));
    EXPECT_EQ(output_fields[ix].attr.bit_sz, 1u);
    EXPECT_EQ(output_fields[ix].attr.offset, 8u + ix);
    EXPECT_EQ(output_fields[ix].attr.usage.usage, led_usage++);
    EXPECT_EQ(expected_flags & output_fields[ix].flags, expected_flags);
  }

  // Next field is 3 bits padding (output).
  EXPECT_EQ(output_fields[5].attr.bit_sz, 3u);
  EXPECT_EQ(output_fields[5].attr.offset, 13u);
  EXPECT_EQ(output_fields[5].type, hid::kOutput);
  EXPECT_EQ(hid::kConstant & output_fields[5].flags, hid::kConstant);

  // All fields belong to the same collection
  collection = fields[0].col;

  for (uint8_t ix = 1; ix != 20; ++ix) {
    EXPECT_TRUE(fields[ix].col == collection);
  }

  ASSERT_TRUE(collection != nullptr);
  EXPECT_EQ(collection->type, hid::CollectionType::kApplication);
  EXPECT_EQ(collection->usage.page, ToUnderlyingType(hid::usage::Page::kGenericDesktop));
  EXPECT_EQ(collection->usage.usage, ToUnderlyingType(hid::usage::GenericDesktop::kKeyboard));
  // No parent collection.
  EXPECT_TRUE(collection->parent == nullptr);

  //////////////////////////////////////////////////////////////////////////////////
  // Third report, single 16 bit input array field (consumer control).  24 bits long.
  EXPECT_EQ(dev->report[2].report_id, 3u);
  ASSERT_EQ(dev->report[2].input_count, 1u);
  EXPECT_EQ(dev->report[2].input_byte_sz, 3u);

  fields = dev->report[2].input_fields;

  expected_flags = hid::kData | hid::kAbsolute | hid::kArray;

  EXPECT_EQ(fields[0].type, hid::kInput);
  EXPECT_EQ(fields[0].attr.usage.page, ToUnderlyingType(hid::usage::Page::kConsumer));
  EXPECT_EQ(fields[0].attr.usage.usage, 0u);
  EXPECT_EQ(fields[0].attr.logc_mm.min, 0);
  EXPECT_EQ(fields[0].attr.logc_mm.max, 572);
  EXPECT_EQ(fields[0].attr.bit_sz, 16u);
  EXPECT_EQ(fields[0].attr.offset, 8u);
  EXPECT_EQ(expected_flags & fields[0].flags, expected_flags);

  collection = fields[0].col;
  ASSERT_TRUE(collection != nullptr);
  EXPECT_EQ(collection->type, hid::CollectionType::kApplication);
  EXPECT_EQ(collection->usage.page, ToUnderlyingType(hid::usage::Page::kConsumer));
  EXPECT_EQ(collection->usage.usage, ToUnderlyingType(hid::usage::Consumer::kConsumerControl));
  // No parent collection.
  EXPECT_TRUE(collection->parent == nullptr);

  //////////////////////////////////////////////////////////////////////////////////
  // Fourth report is a 2 bit input (system control: sleep, wake-up, power-down)
  // 16 bits in total.

  EXPECT_EQ(dev->report[3].report_id, 4u);
  ASSERT_EQ(dev->report[3].input_count, 2u);
  ASSERT_EQ(dev->report[3].input_byte_sz, 2u);

  fields = dev->report[3].input_fields;

  // First field is a 2 bit input array.
  expected_flags = hid::kData | hid::kAbsolute | hid::kArray;

  EXPECT_EQ(fields[0].type, hid::kInput);
  EXPECT_EQ(fields[0].attr.usage.page, ToUnderlyingType(hid::usage::Page::kGenericDesktop));
  // TODO(cpu): The |usage.usage| as parsed is incorrect. In this particular
  // case as the array input 1,2,3 should map to 0x82, 0x81, 0x83 which is not currently
  // supported in the model.
  EXPECT_EQ(fields[0].attr.usage.usage, ToUnderlyingType(hid::usage::GenericDesktop::kSystemSleep));
  EXPECT_EQ(fields[0].attr.logc_mm.min, 1);
  EXPECT_EQ(fields[0].attr.logc_mm.max, 3);
  EXPECT_EQ(fields[0].attr.bit_sz, 2u);
  EXPECT_EQ(fields[0].attr.offset, 8u);
  EXPECT_EQ(expected_flags & fields[0].flags, expected_flags);

  // Last field is 6 bits padding (output).
  EXPECT_EQ(fields[1].attr.bit_sz, 6u);
  EXPECT_EQ(fields[1].attr.offset, 10u);
  EXPECT_EQ(fields[1].type, hid::kInput);
  EXPECT_EQ(hid::kConstant & fields[1].flags, hid::kConstant);

  collection = fields[0].col;
  ASSERT_TRUE(collection != nullptr);
  EXPECT_EQ(collection->type, hid::CollectionType::kApplication);
  EXPECT_EQ(collection->usage.page, ToUnderlyingType(hid::usage::Page::kGenericDesktop));
  EXPECT_EQ(collection->usage.usage, ToUnderlyingType(hid::usage::GenericDesktop::kSystemControl));
  // No parent collection.
  EXPECT_TRUE(collection->parent == nullptr);

  hid::FreeDeviceDescriptor(dev);
}

TEST(HidHelperTest, ParsePs3Controller) {
  hid::DeviceDescriptor* dev = nullptr;
  hid::ParseResult res = hid::ParseReportDescriptor(kPlayStation3DualshockDescriptor.data(),
                                                    kPlayStation3DualshockDescriptor.size(), &dev);

  ASSERT_EQ(res, hid::ParseResult::kParseOk);
  // Four different reports
  ASSERT_EQ(dev->rep_count, 4u);

  //////////////////////////////////////////////////////////////////////////////////
  // First report has 172 fields!!  1160 bits long!!!
  EXPECT_EQ(dev->report[0].report_id, 1u);

  ASSERT_EQ(dev->report[0].input_count, 76u);
  ASSERT_EQ(dev->report[0].input_byte_sz, 49u);

  ASSERT_EQ(dev->report[0].output_count, 48u);
  ASSERT_EQ(dev->report[0].output_byte_sz, 49u);

  ASSERT_EQ(dev->report[0].feature_count, 48u);
  ASSERT_EQ(dev->report[0].feature_byte_sz, 49u);
  const hid::ReportField* fields = dev->report[0].input_fields;
  const hid::ReportField* output_fields = dev->report[0].output_fields;
  const hid::ReportField* feature_fields = dev->report[0].feature_fields;

  // First field is 8 bits  but no usage described (which is normally padding).
  // Maybe it is a version number?
  auto expected_flags = hid::kConstant | hid::kAbsolute | hid::kScalar;

  EXPECT_EQ(fields[0].type, hid::kInput);
  EXPECT_EQ(fields[0].attr.usage.page, 0u);
  EXPECT_EQ(fields[0].attr.usage.usage, 0u);
  EXPECT_EQ(fields[0].attr.logc_mm.min, 0);
  EXPECT_EQ(fields[0].attr.logc_mm.max, 255);
  EXPECT_EQ(fields[0].attr.bit_sz, 8u);
  EXPECT_EQ(fields[0].attr.offset, 8u);
  EXPECT_EQ(expected_flags & fields[0].flags, expected_flags);

  // Next 19 fields are one-bit input representing the buttons.
  expected_flags = hid::kData | hid::kAbsolute | hid::kScalar;

  for (uint8_t ix = 1; ix != 20; ++ix) {
    EXPECT_EQ(fields[ix].type, hid::kInput);
    EXPECT_EQ(fields[ix].attr.usage.page, ToUnderlyingType(hid::usage::Page::kButton));
    EXPECT_EQ(fields[ix].attr.usage.usage, ix);
    EXPECT_EQ(fields[ix].attr.bit_sz, 1u);
    EXPECT_EQ(fields[ix].attr.offset, 16u + (ix - 1u));
    EXPECT_EQ(fields[ix].attr.logc_mm.min, 0);
    EXPECT_EQ(fields[ix].attr.logc_mm.max, 1);
    EXPECT_EQ(fields[ix].attr.phys_mm.min, 0);
    EXPECT_EQ(fields[ix].attr.phys_mm.max, 1);
    EXPECT_EQ(expected_flags & fields[ix].flags, expected_flags);
  }

  // The next 13 fields are 13 bits of constant, vendor-defined. Probably padding.
  for (uint8_t ix = 20; ix != 33; ++ix) {
    EXPECT_EQ(fields[ix].type, hid::kInput);
    EXPECT_EQ(fields[ix].attr.usage.page, 0u);
    EXPECT_EQ(fields[ix].attr.usage.usage, 0u);
    EXPECT_EQ(fields[ix].attr.bit_sz, 1u);
    EXPECT_EQ(fields[ix].attr.offset, 35u + (ix - 20u));
    EXPECT_EQ(hid::kConstant & fields[ix].flags, hid::kConstant);
  }

  expected_flags = hid::kData | hid::kAbsolute | hid::kScalar;

  // Next four 8-bit input fields are X,Y, Z and Rz.
  for (uint8_t ix = 33; ix != 37; ++ix) {
    EXPECT_EQ(fields[ix].type, hid::kInput);
    EXPECT_EQ(fields[ix].attr.usage.page, ToUnderlyingType(hid::usage::Page::kGenericDesktop));
    EXPECT_EQ(fields[ix].attr.bit_sz, 8u);
    EXPECT_EQ(fields[ix].attr.offset, 48u + 8u * (ix - 33u));
    EXPECT_EQ(fields[ix].attr.logc_mm.min, 0);
    EXPECT_EQ(fields[ix].attr.logc_mm.max, 255);
    EXPECT_EQ(fields[ix].attr.phys_mm.min, 0);
    EXPECT_EQ(fields[ix].attr.phys_mm.max, 255);
    EXPECT_EQ(expected_flags & fields[ix].flags, expected_flags);
  }

  EXPECT_EQ(fields[33].attr.usage.usage, ToUnderlyingType(hid::usage::GenericDesktop::kX));
  EXPECT_EQ(fields[34].attr.usage.usage, ToUnderlyingType(hid::usage::GenericDesktop::kY));
  EXPECT_EQ(fields[35].attr.usage.usage, ToUnderlyingType(hid::usage::GenericDesktop::kZ));
  EXPECT_EQ(fields[36].attr.usage.usage, ToUnderlyingType(hid::usage::GenericDesktop::kRz));

  // Next 39 fields are input, 8-bit pointer scalar data.
  for (uint8_t ix = 37; ix != 76; ++ix) {
    EXPECT_EQ(fields[ix].type, hid::kInput);
    EXPECT_EQ(fields[ix].attr.usage.page, ToUnderlyingType(hid::usage::Page::kGenericDesktop));
    EXPECT_EQ(fields[ix].attr.usage.usage, ToUnderlyingType(hid::usage::GenericDesktop::kPointer));
    EXPECT_EQ(fields[ix].attr.bit_sz, 8u);
    EXPECT_EQ(fields[ix].attr.offset, 80u + 8u * (ix - 37u));
    EXPECT_EQ(expected_flags & fields[ix].flags, expected_flags);
  }

  // Test the 48 8-bit scalar output pointer data.
  for (uint8_t ix = 0; ix != 48; ++ix) {
    EXPECT_EQ(output_fields[ix].type, hid::kOutput);
    EXPECT_EQ(output_fields[ix].attr.usage.page,
              ToUnderlyingType(hid::usage::Page::kGenericDesktop));
    EXPECT_EQ(output_fields[ix].attr.usage.usage,
              ToUnderlyingType(hid::usage::GenericDesktop::kPointer));
    EXPECT_EQ(output_fields[ix].attr.bit_sz, 8u);
    EXPECT_EQ(output_fields[ix].attr.offset, 8u + 8u * ix);
    EXPECT_EQ(expected_flags & output_fields[ix].flags, expected_flags);
  }

  // Test the 48 8-bit scalar feature pointer data.
  for (uint8_t ix = 0; ix != 48; ++ix) {
    EXPECT_EQ(feature_fields[ix].type, hid::kFeature);
    EXPECT_EQ(feature_fields[ix].attr.usage.page,
              ToUnderlyingType(hid::usage::Page::kGenericDesktop));
    EXPECT_EQ(feature_fields[ix].attr.usage.usage,
              ToUnderlyingType(hid::usage::GenericDesktop::kPointer));
    EXPECT_EQ(feature_fields[ix].attr.bit_sz, 8u);
    EXPECT_EQ(feature_fields[ix].attr.offset, 8u + 8u * (ix));
    EXPECT_EQ(expected_flags & feature_fields[ix].flags, expected_flags);
  }

  //////////////////////////////////////////////////////////////////////////////////
  // Second report has 48 fields. It is pretty much identical to last 48 fields
  // of the first report.  392 bits long.

  EXPECT_EQ(dev->report[1].report_id, 2u);
  ASSERT_EQ(dev->report[1].feature_count, 48u);
  ASSERT_EQ(dev->report[2].feature_byte_sz, 49u);
  feature_fields = dev->report[1].feature_fields;

  expected_flags = hid::kData | hid::kAbsolute | hid::kScalar;

  for (uint8_t ix = 0; ix != 48; ++ix) {
    EXPECT_EQ(feature_fields[ix].type, hid::kFeature);
    EXPECT_EQ(feature_fields[ix].attr.usage.page,
              ToUnderlyingType(hid::usage::Page::kGenericDesktop));
    EXPECT_EQ(feature_fields[ix].attr.usage.usage,
              ToUnderlyingType(hid::usage::GenericDesktop::kPointer));
    EXPECT_EQ(feature_fields[ix].attr.bit_sz, 8u);
    EXPECT_EQ(feature_fields[ix].attr.offset, 8u + 8u * ix);
    EXPECT_EQ(expected_flags & feature_fields[ix].flags, expected_flags);
  }

  //////////////////////////////////////////////////////////////////////////////////
  // Third report is same as the second one except for report id.

  EXPECT_EQ(dev->report[2].report_id, 0xee);
  ASSERT_EQ(dev->report[2].feature_count, 48u);
  ASSERT_EQ(dev->report[2].feature_byte_sz, 49u);
  feature_fields = dev->report[2].feature_fields;

  expected_flags = hid::kData | hid::kAbsolute | hid::kScalar;

  for (uint8_t ix = 0; ix != 48; ++ix) {
    EXPECT_EQ(feature_fields[ix].type, hid::kFeature);
    EXPECT_EQ(feature_fields[ix].attr.usage.page,
              ToUnderlyingType(hid::usage::Page::kGenericDesktop));
    EXPECT_EQ(feature_fields[ix].attr.usage.usage,
              ToUnderlyingType(hid::usage::GenericDesktop::kPointer));
    EXPECT_EQ(feature_fields[ix].attr.bit_sz, 8u);
    EXPECT_EQ(feature_fields[ix].attr.offset, 8u + 8u * ix);
    EXPECT_EQ(expected_flags & feature_fields[ix].flags, expected_flags);
  }

  //////////////////////////////////////////////////////////////////////////////////
  // Fourth report is same as the second one except for report id.

  EXPECT_EQ(dev->report[3].report_id, 0xef);
  ASSERT_EQ(dev->report[2].feature_count, 48u);
  ASSERT_EQ(dev->report[2].feature_byte_sz, 49u);
  feature_fields = dev->report[3].feature_fields;

  expected_flags = hid::kData | hid::kAbsolute | hid::kScalar;

  for (uint8_t ix = 0; ix != 48; ++ix) {
    EXPECT_EQ(feature_fields[ix].type, hid::kFeature);
    EXPECT_EQ(feature_fields[ix].attr.usage.page,
              ToUnderlyingType(hid::usage::Page::kGenericDesktop));
    EXPECT_EQ(feature_fields[ix].attr.usage.usage,
              ToUnderlyingType(hid::usage::GenericDesktop::kPointer));
    EXPECT_EQ(feature_fields[ix].attr.bit_sz, 8u);
    EXPECT_EQ(feature_fields[ix].attr.offset, 8u + 8u * ix);
    EXPECT_EQ(expected_flags & feature_fields[ix].flags, expected_flags);
  }

  // Collections test
  //
  // In the first report, The X,Y,Z, Rz fields are in a 3-level
  // deep collection physical -> logical -> app. Test that.
  auto collection = dev->report[0].input_fields[33].col;

  ASSERT_TRUE(collection != nullptr);
  EXPECT_EQ(collection->type, hid::CollectionType::kPhysical);
  EXPECT_EQ(collection->usage.page, ToUnderlyingType(hid::usage::Page::kGenericDesktop));
  EXPECT_EQ(collection->usage.usage, ToUnderlyingType(hid::usage::GenericDesktop::kPointer));

  collection = collection->parent;
  ASSERT_TRUE(collection != nullptr);
  EXPECT_EQ(collection->type, hid::CollectionType::kLogical);
  EXPECT_EQ(collection->usage.page, ToUnderlyingType(hid::usage::Page::kGenericDesktop));
  EXPECT_EQ(collection->usage.usage, 0u);

  collection = collection->parent;
  ASSERT_TRUE(collection != nullptr);
  EXPECT_EQ(collection->type, hid::CollectionType::kApplication);
  EXPECT_EQ(collection->usage.page, ToUnderlyingType(hid::usage::Page::kGenericDesktop));
  EXPECT_EQ(collection->usage.usage, ToUnderlyingType(hid::usage::GenericDesktop::kJoystick));
  EXPECT_TRUE(collection->parent == nullptr);

  // The second report first field is in a logical -> app collection.
  collection = dev->report[1].input_fields[0].col;

  ASSERT_TRUE(collection != nullptr);
  EXPECT_EQ(collection->type, hid::CollectionType::kLogical);
  EXPECT_EQ(collection->usage.page, ToUnderlyingType(hid::usage::Page::kGenericDesktop));
  EXPECT_EQ(collection->usage.usage, 0u);

  collection = collection->parent;
  ASSERT_TRUE(collection != nullptr);
  EXPECT_EQ(collection->type, hid::CollectionType::kApplication);
  EXPECT_EQ(collection->usage.page, ToUnderlyingType(hid::usage::Page::kGenericDesktop));
  EXPECT_EQ(collection->usage.usage, ToUnderlyingType(hid::usage::GenericDesktop::kJoystick));
  EXPECT_TRUE(collection->parent == nullptr);

  // The third report is the same as the second. This seems a trivial test
  // but previous parsers failed this one.
  collection = dev->report[2].input_fields[0].col;

  ASSERT_TRUE(collection != nullptr);
  EXPECT_EQ(collection->type, hid::CollectionType::kLogical);
  EXPECT_EQ(collection->usage.page, ToUnderlyingType(hid::usage::Page::kGenericDesktop));
  EXPECT_EQ(collection->usage.usage, 0u);

  collection = collection->parent;
  ASSERT_TRUE(collection != nullptr);
  EXPECT_EQ(collection->type, hid::CollectionType::kApplication);
  EXPECT_EQ(collection->usage.page, ToUnderlyingType(hid::usage::Page::kGenericDesktop));
  EXPECT_EQ(collection->usage.usage, ToUnderlyingType(hid::usage::GenericDesktop::kJoystick));
  EXPECT_TRUE(collection->parent == nullptr);

  hid::FreeDeviceDescriptor(dev);
}

TEST(HidHelperTest, ParseAcer12Touch) {
  hid::DeviceDescriptor* dd = nullptr;
  hid::ParseResult res =
      hid::ParseReportDescriptor(kAcer12TouchDescriptor.data(), kAcer12TouchDescriptor.size(), &dd);

  EXPECT_EQ(res, hid::ParseResult::kParseOk);

  hid::FreeDeviceDescriptor(dd);
}

TEST(HidHelperTest, ParseEveTablet) {
  hid::DeviceDescriptor* dev = nullptr;
  hid::ParseResult res =
      hid::ParseReportDescriptor(kEveTabletDescriptor.data(), kEveTabletDescriptor.size(), &dev);

  EXPECT_EQ(res, hid::ParseResult::kParseOk);

  // A single report, no id.
  ASSERT_EQ(dev->rep_count, 1u);
  EXPECT_EQ(dev->report[0].report_id, 0u);

  // Report has two fields.  8 bits long
  ASSERT_EQ(dev->report[0].input_count, 2u);
  ASSERT_EQ(dev->report[0].input_byte_sz, 1u);

  const hid::ReportField* fields = dev->report[0].input_fields;

  // First field is 1 bit, (tablet / no-tablet)
  auto expected_flags = hid::kData | hid::kAbsolute | hid::kScalar;

  EXPECT_EQ(fields[0].type, hid::kInput);
  EXPECT_EQ(fields[0].attr.usage.page, ToUnderlyingType(hid::usage::Page::kGenericDesktop));
  EXPECT_EQ(fields[0].attr.usage.usage, 0xff000001u);
  EXPECT_EQ(fields[0].attr.bit_sz, 1u);
  EXPECT_EQ(fields[0].attr.offset, 0u);
  EXPECT_EQ(expected_flags & fields[0].flags, expected_flags);

  // Second field is padding, 7 bits.
  expected_flags = hid::kConstant | hid::kAbsolute | hid::kScalar;

  EXPECT_EQ(fields[1].type, hid::kInput);
  EXPECT_EQ(fields[1].attr.usage.page, 0u);
  EXPECT_EQ(fields[1].attr.usage.usage, 0u);
  EXPECT_EQ(fields[1].attr.bit_sz, 7u);
  EXPECT_EQ(fields[1].attr.offset, 1u);
  EXPECT_EQ(expected_flags & fields[1].flags, expected_flags);

  hid::FreeDeviceDescriptor(dev);
}

TEST(HidHelperTest, ParseAsusTouch) {
  hid::DeviceDescriptor* dev = nullptr;
  hid::ParseResult res =
      hid::ParseReportDescriptor(kAsusTouchDescriptor.data(), kAsusTouchDescriptor.size(), &dev);
  ASSERT_EQ(res, hid::ParseResult::kParseOk);
  hid::FreeDeviceDescriptor(dev);
}

TEST(HidHelperTest, ParseEveTouchpadV2) {
  hid::DeviceDescriptor* dev = nullptr;
  hid::ParseResult res = hid::ParseReportDescriptor(kEveTouchpadV2Descriptor.data(),
                                                    kEveTouchpadV2Descriptor.size(), &dev);
  ASSERT_EQ(res, hid::ParseResult::kParseOk);
  // Check that we have one main collection.
  EXPECT_EQ(dev->rep_count, 1u);

  EXPECT_EQ(dev->report[0].report_id, 1u);
  EXPECT_EQ(dev->report[0].input_count, 47u);

  const hid::ReportField* fields = dev->report[0].input_fields;

  uint8_t ix = 0;

  // First report is a button.
  EXPECT_EQ(fields[ix].report_id, 1u);
  EXPECT_EQ(fields[ix].type, hid::kInput);
  EXPECT_EQ(fields[ix].attr.usage.page, ToUnderlyingType(hid::usage::Page::kButton));
  EXPECT_EQ(fields[ix].attr.bit_sz, 1u);
  ++ix;

  // Second report is a digitizer.
  EXPECT_EQ(fields[ix].report_id, 1u);
  EXPECT_EQ(fields[ix].type, hid::kInput);
  EXPECT_EQ(fields[ix].attr.usage.page, ToUnderlyingType(hid::usage::Page::kDigitizer));
  EXPECT_EQ(fields[ix].attr.bit_sz, 7u);
  ++ix;

  // Here are the finger collections. There are 10 items per finger.
  for (int finger = 0; finger != 5; ++finger) {
    EXPECT_EQ(fields[ix].type, hid::kInput);
    EXPECT_EQ(fields[ix].attr.usage.page, ToUnderlyingType(hid::usage::Page::kDigitizer));
    EXPECT_EQ(fields[ix].attr.usage.usage, ToUnderlyingType(hid::usage::Digitizer::kTipSwitch));
    EXPECT_EQ(fields[ix].attr.bit_sz, 1u);
    ++ix;

    EXPECT_EQ(fields[ix].type, hid::kInput);
    EXPECT_EQ(fields[ix].attr.usage.page, ToUnderlyingType(hid::usage::Page::kDigitizer));
    EXPECT_EQ(fields[ix].attr.usage.usage, ToUnderlyingType(hid::usage::Digitizer::kInRange));
    EXPECT_EQ(fields[ix].attr.bit_sz, 7u);
    ++ix;

    EXPECT_EQ(fields[ix].type, hid::kInput);
    EXPECT_EQ(fields[ix].attr.usage.page, ToUnderlyingType(hid::usage::Page::kDigitizer));
    EXPECT_EQ(fields[ix].attr.usage.usage, 0x51u);
    EXPECT_EQ(fields[ix].attr.bit_sz, 16u);
    ++ix;

    // The X coordinate.
    EXPECT_EQ(fields[ix].type, hid::kInput);
    EXPECT_EQ(fields[ix].attr.usage.page, ToUnderlyingType(hid::usage::Page::kGenericDesktop));
    EXPECT_EQ(fields[ix].attr.usage.usage, ToUnderlyingType(hid::usage::GenericDesktop::kX));
    EXPECT_EQ(fields[ix].attr.phys_mm.min, 0);
    EXPECT_EQ(fields[ix].attr.phys_mm.max, 1030);
    EXPECT_EQ(fields[ix].attr.logc_mm.max, 13184);
    EXPECT_EQ(fields[ix].attr.unit.type, 0x11u);
    EXPECT_EQ(fields[ix].attr.unit.exp, -2);
    EXPECT_EQ(fields[ix].attr.bit_sz, 16u);
    ++ix;

    // The Y Coordinate (most fields are inherited from X).
    EXPECT_EQ(fields[ix].type, hid::kInput);
    EXPECT_EQ(fields[ix].attr.usage.page, ToUnderlyingType(hid::usage::Page::kGenericDesktop));
    EXPECT_EQ(fields[ix].attr.usage.usage, ToUnderlyingType(hid::usage::GenericDesktop::kY));
    EXPECT_EQ(fields[ix].attr.phys_mm.min, 0);
    EXPECT_EQ(fields[ix].attr.phys_mm.max, 680);
    EXPECT_EQ(fields[ix].attr.logc_mm.max, 8704);
    EXPECT_EQ(fields[ix].attr.unit.type, 0x11u);
    EXPECT_EQ(fields[ix].attr.unit.exp, -2);
    EXPECT_EQ(fields[ix].attr.bit_sz, 16u);
    ++ix;

    EXPECT_EQ(fields[ix].type, hid::kInput);
    EXPECT_EQ(fields[ix].attr.usage.page, ToUnderlyingType(hid::usage::Page::kDigitizer));
    EXPECT_EQ(fields[ix].attr.usage.usage, 0x48u);
    EXPECT_EQ(fields[ix].attr.logc_mm.max, 13184);
    EXPECT_EQ(fields[ix].attr.bit_sz, 16u);
    ++ix;

    EXPECT_EQ(fields[ix].type, hid::kInput);
    EXPECT_EQ(fields[ix].attr.usage.page, ToUnderlyingType(hid::usage::Page::kDigitizer));
    EXPECT_EQ(fields[ix].attr.usage.usage, 0x49u);
    EXPECT_EQ(fields[ix].attr.logc_mm.max, 8704);
    EXPECT_EQ(fields[ix].attr.bit_sz, 16u);
    ++ix;

    EXPECT_EQ(fields[ix].type, hid::kInput);
    EXPECT_EQ(fields[ix].attr.usage.page, ToUnderlyingType(hid::usage::Page::kDigitizer));
    EXPECT_EQ(fields[ix].attr.usage.usage, ToUnderlyingType(hid::usage::Digitizer::kTipPressure));
    EXPECT_EQ(fields[ix].attr.logc_mm.max, 255);
    EXPECT_EQ(fields[ix].attr.bit_sz, 8u);
    ++ix;

    EXPECT_EQ(fields[ix].type, hid::kInput);
    EXPECT_EQ(fields[ix].attr.usage.page, ToUnderlyingType(hid::usage::Page::kDigitizer));
    EXPECT_EQ(fields[ix].attr.usage.usage, ToUnderlyingType(hid::usage::Digitizer::kAzimuth));
    EXPECT_EQ(fields[ix].attr.logc_mm.max, 360);
    EXPECT_EQ(fields[ix].attr.bit_sz, 16u);
    ++ix;
  }

  // Make sure we checked each of the report fields.
  ASSERT_EQ(ix, dev->report[0].input_count);
  hid::FreeDeviceDescriptor(dev);
}

// The HID descriptor of the Wingcool S838F touch interface doesn't fully
// comply with the HID standards because it has a toplevel collection which is
// not of "Application" type. This test case ensures that `hid-parser` can
// still parse the descriptor despite the incompliance.
TEST(HidHelperTest, ParseWingcoolS838FTouchInterface) {
  hid::DeviceDescriptor* dev = nullptr;
  hid::ParseResult res =
      hid::ParseReportDescriptor(kWingcoolS838FTouchInterfaceDescriptor.data(),
                                 kWingcoolS838FTouchInterfaceDescriptor.size(), &dev);
  ASSERT_EQ(res, hid::ParseResult::kParseOk);

  // The touch interface has totally 5 report IDs.
  EXPECT_EQ(dev->rep_count, 5u);

  EXPECT_EQ(dev->report[0].report_id, 13u);
  EXPECT_EQ(dev->report[0].input_count, 36u);
  EXPECT_EQ(dev->report[0].output_count, 0u);
  EXPECT_EQ(dev->report[0].feature_count, 0u);

  EXPECT_EQ(dev->report[1].report_id, 7u);
  EXPECT_EQ(dev->report[1].input_count, 0u);
  EXPECT_EQ(dev->report[1].output_count, 0u);
  EXPECT_EQ(dev->report[1].feature_count, 1u);

  EXPECT_EQ(dev->report[2].report_id, 33u);
  EXPECT_EQ(dev->report[2].input_count, 0u);
  EXPECT_EQ(dev->report[2].output_count, 0u);
  EXPECT_EQ(dev->report[2].feature_count, 2u);

  // Reports [3] and [4] come from the top-level collection that doesn't have
  // a correct type. These reports should still be parsed and included in the
  // parsing results.

  EXPECT_EQ(dev->report[3].report_id, 11u);
  EXPECT_EQ(dev->report[3].input_count, 504u);
  EXPECT_EQ(dev->report[3].output_count, 0u);
  EXPECT_EQ(dev->report[3].feature_count, 0u);

  EXPECT_EQ(dev->report[4].report_id, 12u);
  EXPECT_EQ(dev->report[4].input_count, 0u);
  EXPECT_EQ(dev->report[4].output_count, 0u);
  EXPECT_EQ(dev->report[4].feature_count, 1u);

  hid::FreeDeviceDescriptor(dev);
}

}  // namespace
