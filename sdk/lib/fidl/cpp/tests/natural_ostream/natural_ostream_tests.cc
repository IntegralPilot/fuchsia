// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/test.types/cpp/natural_ostream.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/zx/bti.h>
#include <lib/zx/channel.h>
#include <lib/zx/clock.h>
#include <lib/zx/debuglog.h>
#include <lib/zx/event.h>
#include <lib/zx/eventpair.h>
#include <lib/zx/exception.h>
#include <lib/zx/fifo.h>
#include <lib/zx/guest.h>
#include <lib/zx/interrupt.h>
#include <lib/zx/iommu.h>
#include <lib/zx/job.h>
#include <lib/zx/msi.h>
#include <lib/zx/pager.h>
#include <lib/zx/pmt.h>
#include <lib/zx/port.h>
#include <lib/zx/process.h>
#include <lib/zx/profile.h>
#include <lib/zx/resource.h>
#include <lib/zx/socket.h>
#include <lib/zx/stream.h>
#include <lib/zx/suspend_token.h>
#include <lib/zx/thread.h>
#include <lib/zx/timer.h>
#include <lib/zx/vcpu.h>
#include <lib/zx/vmar.h>
#include <lib/zx/vmo.h>

#include <sstream>

#include <gtest/gtest.h>

// Use the << operator to print a value to a std::string.
template <typename T>
std::string to_string(const T& value) {
  std::ostringstream buf;
  buf << value;
  return buf.str();
}

// Format a value as it would be formatted as the member of a FIDL type.
template <typename T>
std::string fidl_string(const T& value) {
  return to_string(fidl::ostream::Formatted<T>(value));
}

TEST(NaturalOStream, Primitive) {
  EXPECT_EQ(fidl_string<uint8_t>(42), "42");
  EXPECT_EQ(fidl_string<uint16_t>(42), "42");
  EXPECT_EQ(fidl_string<uint32_t>(42), "42");
  EXPECT_EQ(fidl_string<uint64_t>(42), "42");
  EXPECT_EQ(fidl_string<int8_t>(42), "42");
  EXPECT_EQ(fidl_string<int16_t>(42), "42");
  EXPECT_EQ(fidl_string<int32_t>(42), "42");
  EXPECT_EQ(fidl_string<int64_t>(42), "42");
  EXPECT_EQ(fidl_string<int8_t>(-42), "-42");
  EXPECT_EQ(fidl_string<int16_t>(-42), "-42");
  EXPECT_EQ(fidl_string<int32_t>(-42), "-42");
  EXPECT_EQ(fidl_string<int64_t>(-42), "-42");

  EXPECT_EQ(fidl_string<bool>(false), "false");
  EXPECT_EQ(fidl_string<bool>(true), "true");

  EXPECT_EQ(fidl_string<float>(3.14F), "3.14");
  EXPECT_EQ(fidl_string<double>(3.14), "3.14");
}

TEST(NaturalOStream, String) {
  EXPECT_EQ(fidl_string<std::string>("Hello"), "\"Hello\"");
  EXPECT_EQ(fidl_string<std::optional<std::string>>("Hello"), "\"Hello\"");
  EXPECT_EQ(fidl_string<std::optional<std::string>>(std::nullopt), "null");
  EXPECT_EQ(fidl_string<std::string>("Hello\nWorld"), "\"Hello\\x0aWorld\"");
  EXPECT_EQ(fidl_string<std::string>("Hello 🌏"), "\"Hello \\xf0\\x9f\\x8c\\x8f\"");
}

TEST(NaturalOStream, Vector) {
  EXPECT_EQ(fidl_string<std::vector<uint8_t>>({2, 4, 6, 8}), "[ 2, 4, 6, 8, ]");
  EXPECT_EQ(fidl_string<std::optional<std::vector<uint8_t>>>({{2, 4, 6, 8}}), "[ 2, 4, 6, 8, ]");
  EXPECT_EQ(fidl_string<std::optional<std::vector<uint8_t>>>(std::nullopt), "null");
  EXPECT_EQ(fidl_string<std::vector<bool>>({true, false}), "[ true, false, ]");
}

TEST(NaturalOStream, Array) {
  std::array<uint8_t, 4> numbers{2, 4, 6, 8};
  EXPECT_EQ(fidl_string(numbers), "[ 2, 4, 6, 8, ]");
  std::array<bool, 2> bools{true, false};
  EXPECT_EQ(fidl_string(bools), "[ true, false, ]");
}

// Helper function that creates a handle wrapper object with an arbitrary handle value, checks its
// string representation, and safely discards it without sending garbage to the kernel.
template <typename H>
void check_handle_string(const char* name) {
  static_assert(std::is_base_of_v<zx::object_base, H>);
  char expected[128];
  const zx_handle_t raw = 1234;
  H handle(raw);
  snprintf(expected, 128, "%s(%u)", name, raw);
  EXPECT_EQ(expected, fidl_string(handle));
  zx_handle_t returned = handle.release();
  ASSERT_EQ(raw, returned);
}

TEST(NaturalOStream, Handle) {
  check_handle_string<zx::bti>("bti");
  check_handle_string<zx::channel>("channel");
  check_handle_string<zx::clock>("clock");
  check_handle_string<zx::event>("event");
  check_handle_string<zx::eventpair>("eventpair");
  check_handle_string<zx::exception>("exception");
  check_handle_string<zx::fifo>("fifo");
  check_handle_string<zx::guest>("guest");
  check_handle_string<zx::interrupt>("interrupt");
  check_handle_string<zx::iommu>("iommu");
  check_handle_string<zx::job>("job");
  check_handle_string<zx::debuglog>("debuglog");
  check_handle_string<zx::msi>("msi");
  check_handle_string<zx::pager>("pager");
  check_handle_string<zx::pmt>("pmt");
  check_handle_string<zx::port>("port");
  check_handle_string<zx::process>("process");
  check_handle_string<zx::profile>("profile");
  check_handle_string<zx::resource>("resource");
  check_handle_string<zx::socket>("socket");
  check_handle_string<zx::stream>("stream");
  check_handle_string<zx::suspend_token>("suspend_token");
  check_handle_string<zx::thread>("thread");
  check_handle_string<zx::timer>("timer");
  check_handle_string<zx::vcpu>("vcpu");
  check_handle_string<zx::vmar>("vmar");
  check_handle_string<zx::vmo>("vmo");

  check_handle_string<zx::handle>("handle");

  EXPECT_EQ(fidl_string(zx::handle()), "handle(0)");
}

TEST(NaturalOStream, StrictBits) {
  EXPECT_EQ(to_string(test_types::StrictBits::kB), "test_types::StrictBits(kB)");
  EXPECT_EQ(to_string(test_types::StrictBits::kB | test_types::StrictBits::kD),
            "test_types::StrictBits(kB|kD)");
  EXPECT_EQ(to_string(test_types::StrictBits::kB | test_types::StrictBits(128)),
            "test_types::StrictBits(kB)");
  EXPECT_EQ(to_string(test_types::StrictBits(128)), "test_types::StrictBits()");
}

TEST(NaturalOStream, FlexibleBits) {
  EXPECT_EQ(to_string(test_types::FlexibleBits::kB), "test_types::FlexibleBits(kB)");
  EXPECT_EQ(to_string(test_types::FlexibleBits::kB | test_types::FlexibleBits::kD),
            "test_types::FlexibleBits(kB|kD)");
  EXPECT_EQ(to_string(test_types::FlexibleBits::kB | test_types::FlexibleBits(128)),
            "test_types::FlexibleBits(kB|128)");
  EXPECT_EQ(to_string(test_types::FlexibleBits(128)), "test_types::FlexibleBits(128)");
}

TEST(NaturalOStream, StructEnum) {
  EXPECT_EQ(to_string(test_types::StrictEnum::kB), "test_types::StrictEnum::kB");
  EXPECT_EQ(to_string(test_types::StrictEnum::kD), "test_types::StrictEnum::kD");
}

TEST(NaturalOStream, FlexibleEnum) {
  EXPECT_EQ(to_string(test_types::FlexibleEnum::kB), "test_types::FlexibleEnum::kB");
  EXPECT_EQ(to_string(test_types::FlexibleEnum::kD), "test_types::FlexibleEnum::kD");
  EXPECT_EQ(to_string(test_types::FlexibleEnum(43)), "test_types::FlexibleEnum::UNKNOWN(43)");
}

TEST(NaturalOStream, Struct) {
  EXPECT_EQ(to_string(test_types::CopyableStruct{{.x = 42}}),
            "test_types::CopyableStruct{ x = 42, }");
  EXPECT_EQ(to_string(test_types::StructWithoutPadding({.a = 1, .b = 2, .c = 3, .d = 4})),
            "test_types::StructWithoutPadding{ a = 1, b = 2, c = 3, d = 4, }");
  EXPECT_EQ(to_string(test_types::VectorStruct({.v = {1, 2, 3, 4, 5, 6, 7}})),
            "test_types::VectorStruct{ v = [ 1, 2, 3, 4, 5, 6, 7, ], }");
}

TEST(NaturalOStream, Table) {
  EXPECT_EQ(to_string(test_types::TableMaxOrdinal3WithReserved2{}),
            "test_types::TableMaxOrdinal3WithReserved2{ }");
  EXPECT_EQ(to_string(test_types::TableMaxOrdinal3WithReserved2({.field_1 = 23, .field_3 = 42})),
            "test_types::TableMaxOrdinal3WithReserved2{ field_1 = 23, field_3 = 42, }");

  test_types::TableWithSubTables twst;
  EXPECT_EQ(fidl_string(twst.t()), "null");
  twst.t() = test_types::SampleTable({.x = 23});
  EXPECT_EQ(fidl_string(twst.t()), "test_types::SampleTable{ x = 23, }");
}

TEST(NaturalOStream, Union) {
  EXPECT_EQ(to_string(test_types::TestUnion::WithPrimitive(42)),
            "test_types::TestUnion::primitive(42)");
  EXPECT_EQ(to_string(test_types::TestUnion::WithCopyable({{.x = 23}})),
            "test_types::TestUnion::copyable(test_types::CopyableStruct{ x = 23, })");
  EXPECT_EQ(
      to_string(test_types::TestXUnion(fidl::internal::DefaultConstructPossiblyInvalidObjectTag{})),
      "test_types::TestXUnion::Unknown");
}

TEST(NaturalOStream, Protocol) {
  auto endpoints = fidl::CreateEndpoints<test_types::TypesTest>();
  char buf[128];
  snprintf(buf, 128, "ClientEnd<test_types::TypesTest>(%u)", endpoints->client.channel().get());
  EXPECT_EQ(to_string(endpoints->client), buf);
  snprintf(buf, 128, "ServerEnd<test_types::TypesTest>(%u)", endpoints->server.channel().get());
  EXPECT_EQ(fidl_string(endpoints->server), buf);
}

TEST(NaturalOStream, Optionals) {
  test_types::StructOfOptionals optionals{};
  EXPECT_EQ(to_string(optionals), "test_types::StructOfOptionals{ s = null, v = null, t = null, }");

  optionals.s() = "hello";
  optionals.v() = std::vector<uint32_t>{1, 2, 3};
  optionals.t() = std::make_unique<test_types::CopyableStruct>();

  EXPECT_EQ(
      to_string(optionals),
      "test_types::StructOfOptionals{ s = \"hello\", v = [ 1, 2, 3, ], t = test_types::CopyableStruct{ x = 0, }, }");
}
