// Copyright 2020 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/devicetree/devicetree.h>
#include <lib/devicetree/testing/loaded-dtb.h>
#include <lib/fit/result.h>

#include <array>
#include <cstddef>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <optional>
#include <source_location>
#include <span>

#include <zxtest/zxtest.h>

namespace {

using devicetree::testing::LoadDtb;

TEST(DevicetreeTest, NodeNameAndAddress) {
  {
    devicetree::Node node("abc");
    EXPECT_STREQ("abc", node.name());
    EXPECT_STREQ("", node.address());
  }
  {
    devicetree::Node node("abc@");
    EXPECT_STREQ("abc", node.name());
    EXPECT_STREQ("", node.address());
  }
  {
    devicetree::Node node("abc@def");
    EXPECT_STREQ("abc", node.name());
    EXPECT_STREQ("def", node.address());
  }
  {
    devicetree::Node node("@def");
    EXPECT_STREQ("", node.name());
    EXPECT_STREQ("def", node.address());
  }
}

TEST(DevicetreeTest, EmptyTree) {
  auto loaded_dtb = LoadDtb("empty.dtb");
  ASSERT_TRUE(loaded_dtb.is_ok(), "%s", loaded_dtb.error_value().c_str());
  devicetree::Devicetree dt = loaded_dtb->fdt();

  size_t seen = 0;
  auto walker = [&seen](const devicetree::NodePath& path, const devicetree::PropertyDecoder&) {
    if (seen++ == 0) {
      size_t size = path.size();
      EXPECT_EQ(1, size);
      if (size > 0) {
        EXPECT_TRUE(path.back().empty());  // Root node.
      }
    }
    return true;
  };
  dt.Walk(walker);
  EXPECT_EQ(1, seen);
}

struct Node {
  std::string_view name;
  size_t size = 0;
  bool prune = false;
};

// List of nodes are in pre order.
// post order does a translation for node[i](preorder) -> node[post_order[i]](postorder)
void DoAndVerifyWalk(devicetree::Devicetree& tree, std::span<const Node> nodes,
                     std::span<const size_t> post_order) {
  // PreWalk Only check.
  size_t seen = 0;
  auto pre_walker = [&](const devicetree::NodePath& path,
                        const devicetree::PropertyDecoder& props) {
    bool prune = false;
    if (seen < nodes.size()) {
      auto node = nodes[seen];
      size_t size = path.size();
      EXPECT_EQ(node.size, path.size());
      if (size > 0) {
        EXPECT_STREQ(node.name, path.back());
      }
      prune = node.prune;
    }
    ++seen;
    return !prune;
  };

  // Only pre walk.
  tree.Walk(pre_walker);
  EXPECT_EQ(seen, nodes.size());
  seen = 0;

  size_t post_seen = 0;
  auto post_walker = [&](const devicetree::NodePath& path,
                         const devicetree::PropertyDecoder& props) {
    bool prune = false;
    if (post_seen < nodes.size()) {
      auto node = nodes[post_order[post_seen]];
      size_t size = path.size();
      EXPECT_EQ(node.size, path.size());
      if (size > 0) {
        EXPECT_STREQ(node.name, path.back());
      }
      prune = node.prune;
    }
    ++post_seen;
    return !prune;
  };

  tree.Walk(pre_walker, post_walker);
  EXPECT_EQ(seen, nodes.size());
  EXPECT_EQ(seen, post_seen);
}

TEST(DevicetreeTest, NodesAreVisitedDepthFirst) {
  /*
         *
        / \
       A   E
      / \   \
     B   C   F
        /   / \
       D   G   I
          /
         H
  */
  auto loaded_dtb = LoadDtb("complex_no_properties.dtb");
  ASSERT_TRUE(loaded_dtb.is_ok(), "%s", loaded_dtb.error_value().c_str());
  devicetree::Devicetree dt = loaded_dtb->fdt();

  constexpr auto nodes = std::to_array<Node>({
      {.name = "", .size = 1},
      {.name = "A", .size = 2},
      {.name = "B", .size = 3},
      {.name = "C", .size = 3},
      {.name = "D", .size = 4},
      {.name = "E", .size = 2},
      {.name = "F", .size = 3},
      {.name = "G", .size = 4},
      {.name = "H", .size = 5},
      {.name = "I", .size = 4},
  });

  constexpr auto post_order = std::to_array<size_t>({2, 4, 3, 1, 8, 7, 9, 6, 5, 0});

  DoAndVerifyWalk(dt, nodes, post_order);
}

TEST(DevicetreeTest, SubtreesArePruned) {
  /*
         *
        / \
       A   E
      / \   \
     B   C^  F^
        /   / \
       D   G   I
          /
         H

   ^ = root of pruned subtree
  */
  auto loaded_dtb = LoadDtb("complex_no_properties.dtb");
  ASSERT_TRUE(loaded_dtb.is_ok(), "%s", loaded_dtb.error_value().c_str());
  devicetree::Devicetree dt = loaded_dtb->fdt();

  constexpr auto nodes = std::to_array<Node>({
      {.name = "", .size = 1},
      {.name = "A", .size = 2},
      {.name = "B", .size = 3},
      {.name = "C", .size = 3, .prune = true},
      {.name = "E", .size = 2},
      {.name = "F", .size = 3, .prune = true},
  });

  constexpr auto post_order = std::to_array<size_t>({2, 3, 1, 5, 4, 0});

  DoAndVerifyWalk(dt, nodes, post_order);
}

TEST(DevicetreeTest, WholeTreeIsPruned) {
  /*
           *^
          / \
         A   E
        / \   \
       B   C   F
          /   / \
         D   G   I
            /
           H

     ^ = root of pruned subtree
    */

  auto loaded_dtb = LoadDtb("complex_no_properties.dtb");
  ASSERT_TRUE(loaded_dtb.is_ok(), "%s", loaded_dtb.error_value().c_str());
  devicetree::Devicetree dt = loaded_dtb->fdt();

  constexpr auto nodes = std::to_array<Node>({
      {.name = "", .size = 1, .prune = true},
  });

  constexpr auto post_order = std::to_array<size_t>({0});

  DoAndVerifyWalk(dt, nodes, post_order);
}

TEST(DevicetreeTest, AliaesNodeIsKept) {
  auto loaded_dtb = LoadDtb("complex_with_alias_first.dtb");
  ASSERT_TRUE(loaded_dtb.is_ok(), "%s", loaded_dtb.error_value().c_str());
  // aliases:
  // foo = "/A/C";
  // bar = "/E/F";
  devicetree::Devicetree dt = loaded_dtb->fdt();

  std::optional<devicetree::ResolvedPath> resolved;
  dt.Walk([&resolved](const auto& path, const auto& decoder) {
    if (path.back() == "A") {
      if (auto res = decoder.ResolvePath("foo"); res.is_ok()) {
        resolved = *res;
      }
      return false;
    }
    return true;
  });

  ASSERT_TRUE(resolved);
  auto [foo, empty] = *resolved;
  EXPECT_EQ(foo, "/A/C");
  EXPECT_TRUE(empty.empty());
}

TEST(DevicetreeTest, PropertiesAreTranslated) {
  /*
         *
        / \
       A   C
      /     \
     B       D
  */
  auto loaded_dtb = LoadDtb("simple_with_properties.dtb");
  ASSERT_TRUE(loaded_dtb.is_ok(), "%s", loaded_dtb.error_value().c_str());
  devicetree::Devicetree dt = loaded_dtb->fdt();
  size_t seen = 0;
  auto walker = [&seen](const devicetree::NodePath& path,
                        const devicetree::PropertyDecoder& decoder) {
    auto& props = decoder.properties();
    switch (seen++) {
      case 0: {  // root
        size_t size = path.size();
        EXPECT_EQ(1, size);
        if (size > 0) {
          EXPECT_TRUE(path.back().empty());
        }

        devicetree::Properties::iterator begin;
        begin = props.begin();  // Can copy-assign.
        EXPECT_EQ(begin, props.end());

        break;
      }
      case 1: {  // A
        size_t size = path.size();
        EXPECT_EQ(2, size);
        if (size > 0) {
          EXPECT_STREQ("A", path.back());
        }
        EXPECT_EQ(props.end(), std::next(props.begin(), 2));  // 2 properties.

        auto prop1 = *props.begin();
        EXPECT_STREQ("a1", prop1.name);
        EXPECT_TRUE(prop1.value.AsBool().value());
        auto prop2 = *std::next(props.begin());
        EXPECT_STREQ("a2", prop2.name);
        EXPECT_STREQ("root", *prop2.value.AsString());
        break;
      }
      case 2: {  // B
        size_t size = path.size();
        EXPECT_EQ(3, size);
        if (size > 0) {
          EXPECT_STREQ("B", path.back());
        }
        EXPECT_EQ(props.end(), std::next(props.begin(), 3));  // 3 properties.

        auto prop1 = *props.begin();
        EXPECT_STREQ("b1", prop1.name);
        EXPECT_EQ(0x1, prop1.value.AsUint32());
        auto prop2 = *std::next(props.begin());
        EXPECT_STREQ("b2", prop2.name);
        EXPECT_EQ(0x10, prop2.value.AsUint32());
        auto prop3 = *std::next(props.begin(), 2);
        EXPECT_STREQ("b3", prop3.name);
        EXPECT_EQ(0x100, prop3.value.AsUint32());
        break;
      }
      case 3: {  // C
        size_t size = path.size();
        EXPECT_EQ(2, size);
        if (size > 0) {
          EXPECT_STREQ("C", path.back());
        }
        EXPECT_EQ(props.end(), std::next(props.begin(), 2));  // 2 properties.

        auto prop1 = *props.begin();
        EXPECT_STREQ("c1", prop1.name);
        EXPECT_STREQ("hello", *prop1.value.AsString());
        auto prop2 = *std::next(props.begin());
        EXPECT_STREQ("c2", prop2.name);
        EXPECT_STREQ("world", *prop2.value.AsString());
        break;
      }
      case 4: {  // D
        size_t size = path.size();
        EXPECT_EQ(3, size);
        if (size > 0) {
          EXPECT_STREQ("D", path.back());
        }
        EXPECT_EQ(props.end(), std::next(props.begin(), 3));  // 3 properties.

        auto prop1 = *props.begin();
        EXPECT_STREQ("d1", prop1.name);
        EXPECT_EQ(0x1000, prop1.value.AsUint64());
        auto prop2 = *std::next(props.begin());
        EXPECT_STREQ("d2", prop2.name);
        EXPECT_EQ(0x10000, prop2.value.AsUint64());
        auto prop3 = *std::next(props.begin(), 2);
        EXPECT_STREQ("d3", prop3.name);
        EXPECT_EQ(0x100000, prop3.value.AsUint64());
        break;
      }
    }
    return true;
  };
  dt.Walk(walker);
  EXPECT_EQ(5, seen);
}

TEST(DevicetreeTest, MemoryReservations) {
  auto loaded_dtb = LoadDtb("memory_reservations.dtb");
  ASSERT_TRUE(loaded_dtb.is_ok(), "%s", loaded_dtb.error_value().c_str());
  devicetree::Devicetree dt = loaded_dtb->fdt();

  unsigned int i = 0;
  for (auto [start, size] : dt.memory_reservations()) {
    switch (i++) {
      case 0:
        EXPECT_EQ(start, 0x12340000);
        EXPECT_EQ(size, 0x2000);
        break;
      case 1:
        EXPECT_EQ(start, 0x56780000);
        EXPECT_EQ(size, 0x3000);
        break;
      case 2:
        EXPECT_EQ(start, 0x7fffffff12340000);
        EXPECT_EQ(size, 0x400000000);
        break;
      case 3:
        EXPECT_EQ(start, 0x00ffffff56780000);
        EXPECT_EQ(size, 0x500000000);
        break;
      default:
        EXPECT_LT(i, 4, "too many entries");
        break;
    }
  }
  EXPECT_EQ(i, 4, "wrong number of entries");
}

TEST(DevicetreeTest, NulTerminatedStringProperty) {
  using namespace std::literals;

  constexpr std::string_view kNulTerminated = "nul-terminated\0"sv;
  ASSERT_EQ('\0', kNulTerminated.back());

  devicetree::PropertyValue prop(
      std::span{reinterpret_cast<const uint8_t*>(kNulTerminated.data()), kNulTerminated.size()});

  std::optional<std::string_view> decoded = prop.AsString();
  ASSERT_TRUE(decoded);
  EXPECT_EQ(kNulTerminated.substr(0, kNulTerminated.size() - 1), *decoded);  // Trim NUL
}

// A string property must be NUL-terminated, per the spec - but we are lenient
// on this point for pragmatism's sake (as we have encountered devicetrees in
// the wild that fail to meet this requirement, and letting this slide doesn't
// actual complicate any parsing logic).
TEST(DevicetreeTest, NonNulTerminatedStringProperty) {
  using namespace std::literals;

  constexpr std::string_view kNonNulTerminated = "non-nul-terminated"sv;
  ASSERT_NE('\0', kNonNulTerminated.back());

  devicetree::PropertyValue prop(std::span{
      reinterpret_cast<const uint8_t*>(kNonNulTerminated.data()), kNonNulTerminated.size()});

  std::optional<std::string_view> decoded = prop.AsString();
  ASSERT_TRUE(decoded);
  EXPECT_EQ(kNonNulTerminated, *decoded);
}

TEST(DevicetreeTest, StringList) {
  using namespace std::literals;

  unsigned int i = 0;
  for (auto str : devicetree::StringList(""sv)) {
    ++i;
    EXPECT_FALSE(true, "list should be empty");
    EXPECT_TRUE(str.empty());
  }
  EXPECT_EQ(i, 0);

  i = 0;
  for (auto str : devicetree::StringList("one"sv)) {
    ++i;
    EXPECT_STREQ("one", str);
  }
  EXPECT_EQ(i, 1);

  i = 0;
  for (auto str : devicetree::StringList("one\0two\0three"sv)) {
    switch (i++) {
      case 0:
        EXPECT_STREQ("one", str);
        break;
      case 1:
        EXPECT_STREQ("two", str);
        break;
      case 2:
        EXPECT_STREQ("three", str);
        break;
    }
  }
  EXPECT_EQ(i, 3);

  // By convention, there is no element following a terminal separator.
  i = 0;
  for (auto str : devicetree::StringList("one\0\0two\0"sv)) {
    switch (i++) {
      case 0:
        EXPECT_STREQ("one", str);
        break;
      case 2:
        EXPECT_STREQ("two", str);
        break;
      default:
        EXPECT_EQ(0, str.size());
    }
  }
  EXPECT_EQ(i, 3);

  i = 0;
  for (auto str : devicetree::StringList<'/'>("foo/bar/baz"sv)) {
    switch (i++) {
      case 0:
        EXPECT_STREQ("foo", str);
        break;
      case 1:
        EXPECT_STREQ("bar", str);
        break;
      case 3:
        EXPECT_STREQ("baz", str);
        break;
    }
  }
  EXPECT_EQ(i, 3);

  const devicetree::StringList string_list("one\0two\0three"sv);
  const std::vector<std::string_view> string_view_vector(string_list.begin(), string_list.end());
  ASSERT_EQ(string_view_vector.size(), 3);
  EXPECT_STREQ("one", string_view_vector[0]);
  EXPECT_STREQ("two", string_view_vector[1]);
  EXPECT_STREQ("three", string_view_vector[2]);
}

auto as_bytes = [](auto& val) {
  using byte_type = std::conditional_t<std::is_const_v<std::remove_reference_t<decltype(val)>>,
                                       const uint8_t, uint8_t>;
  return std::span<byte_type>(reinterpret_cast<byte_type*>(&val), sizeof(val));
};

auto append = [](auto& vec, auto&& other) { vec.insert(vec.end(), other.begin(), other.end()); };

uint32_t byte_swap(uint32_t val) {
  if constexpr (std::endian::native == std::endian::big) {
    return val;
  } else {
    auto bytes = as_bytes(val);
    return static_cast<uint32_t>(bytes[0]) << 24 | static_cast<uint32_t>(bytes[1]) << 16 |
           static_cast<uint32_t>(bytes[2]) << 8 | static_cast<uint32_t>(bytes[3]);
  }
}

struct RegisterBlockPropertyBuilder {
  void Add(uint64_t address, uint64_t size) {
    uint32_t address_low = byte_swap(0x0000FFFF & address);
    uint32_t size_low = byte_swap(0x0000FFFF & size);

    append(property_value, as_bytes(address_low));
    if (address_cells == 2) {
      uint32_t address_high = byte_swap(address >> 32);
      append(property_value, as_bytes(address_high));
    }

    append(property_value, as_bytes(size_low));
    if (size_cells == 2) {
      uint32_t size_high = byte_swap(size >> 32);
      append(property_value, as_bytes(size_high));
    }
  }

  const uint32_t address_cells;
  const uint32_t size_cells;
  std::vector<uint8_t> property_value;
};

// Small helper so we can verify the behavior of CachedProperties.
struct PropertyBuilder {
  devicetree::Properties Build() {
    return devicetree::Properties(
        {property_block.data(), property_block.size()},
        std::string_view(reinterpret_cast<const char*>(string_block.data()), string_block.size()));
  }

  void Add(std::string_view name, uint32_t value) {
    uint32_t name_off = byte_swap(static_cast<uint32_t>(string_block.size()));
    // String must be null terminated.
    append(string_block, name);
    string_block.push_back('\0');

    uint32_t len = byte_swap(sizeof(uint32_t));

    if (!property_block.empty()) {
      const uint32_t kFdtPropToken = byte_swap(0x00000003);
      append(property_block, as_bytes(kFdtPropToken));
    }
    // this are all 32b aliagned, no padding need.
    append(property_block, as_bytes(len));
    append(property_block, as_bytes(name_off));
    uint32_t be_value = byte_swap(value);
    append(property_block, as_bytes(be_value));
  }

  void Add(std::string_view alias, std::string_view absolute_path) {
    constexpr std::array<uint8_t, sizeof(uint32_t) - 1> kPadding = {};

    uint32_t name_off = byte_swap(static_cast<uint32_t>(string_block.size()));
    // String must be null terminated.
    append(string_block, alias);
    string_block.push_back('\0');

    // Add the null terminator.
    uint32_t len = static_cast<uint32_t>(absolute_path.size()) + 1;
    std::span<const uint8_t> padding;
    if (auto remainder = len % sizeof(uint32_t); remainder != 0) {
      padding = std::span(kPadding).subspan(0, sizeof(uint32_t) - remainder);
    }
    len = byte_swap(len);

    if (!property_block.empty()) {
      const uint32_t kFdtPropToken = byte_swap(0x00000003);
      append(property_block, as_bytes(kFdtPropToken));
    }
    append(property_block, as_bytes(len));
    append(property_block, as_bytes(name_off));
    append(property_block, absolute_path);
    property_block.push_back('\0');
    append(property_block, padding);
  }

  void Add(std::string_view name, std::span<const uint8_t> byte_array) {
    uint32_t name_off = byte_swap(static_cast<uint32_t>(string_block.size()));
    // String must be null terminated.
    append(string_block, name);
    string_block.push_back('\0');

    uint32_t len = byte_swap(static_cast<uint32_t>(byte_array.size()));

    if (!property_block.empty()) {
      const uint32_t kFdtPropToken = byte_swap(0x00000003);
      append(property_block, as_bytes(kFdtPropToken));
    }
    // this are all 32b aliagned, no padding need.
    append(property_block, as_bytes(len));
    append(property_block, as_bytes(name_off));
    append(property_block, byte_array);
  }

  std::vector<uint8_t> property_block;
  std::vector<uint8_t> string_block;
};

auto check_prop = [](auto& prop, uint32_t val,
                     std::source_location loc = std::source_location::current()) {
  ASSERT_TRUE(prop, "at %s:%u", loc.file_name(), loc.line());
  auto pv = prop->AsUint32();
  ASSERT_TRUE(pv, "at %s:%u", loc.file_name(), loc.line());
  EXPECT_EQ(*pv, val, "at %s:%u", loc.file_name(), loc.line());
};

TEST(PropertyDecoderTest, FindProperties) {
  PropertyBuilder builder;
  builder.Add("property_1", 1);
  builder.Add("property_2", 2);
  builder.Add("property_3", 3);
  auto props = builder.Build();

  devicetree::PropertyDecoder decoder(props);

  auto [p1, p2, p3, u4, rep_p3] =
      decoder.FindProperties("property_1", "property_2", "property_3", "unknown", "property_3");

  check_prop(p1, 1);
  check_prop(p2, 2);
  check_prop(p3, 3);
  ASSERT_FALSE(u4);
  ASSERT_FALSE(rep_p3);
}

TEST(PropertyDecoderTest, FindProperty) {
  PropertyBuilder builder;
  builder.Add("property_1", 1);
  builder.Add("property_2", 2);
  builder.Add("property_3", 3);
  auto props = builder.Build();

  devicetree::PropertyDecoder decoder(props);

  auto prop = decoder.FindProperty("property_1");
  auto not_found = decoder.FindProperty("not in there");

  check_prop(prop, 1);
  ASSERT_FALSE(not_found);
}

TEST(PropertyDecoderTest, ResolvePathWithAlias) {
  PropertyBuilder builder;
  builder.Add("foo", "/foo/bar");
  builder.Add("bar", "/bar/baz");
  builder.Add("baz", "/baz/foo");
  builder.Add("empty", "");
  std::optional<devicetree::Properties> aliases(builder.Build());

  devicetree::PropertyDecoder decoder(nullptr, devicetree::Properties(), aliases);

  auto foo_path = decoder.ResolvePath("foo");
  ASSERT_TRUE(foo_path.is_ok());

  auto [foo_prefix, foo_suffix] = *foo_path;
  EXPECT_EQ(foo_prefix, "/foo/bar");
  EXPECT_TRUE(foo_suffix.empty());

  auto bar_path = decoder.ResolvePath("bar/baz");
  ASSERT_TRUE(bar_path.is_ok());

  auto [bar_prefix, bar_suffix] = *bar_path;
  EXPECT_EQ(bar_prefix, "/bar/baz");
  EXPECT_EQ(bar_suffix, "baz");

  auto baz_path = decoder.ResolvePath("baz/baz/foo");
  ASSERT_TRUE(baz_path.is_ok());

  auto [baz_prefix, baz_suffix] = *baz_path;
  EXPECT_EQ(baz_prefix, "/baz/foo");
  EXPECT_EQ(baz_suffix, "baz/foo");

  // Alias not found.
  auto not_found = decoder.ResolvePath("foobar");
  ASSERT_TRUE(not_found.is_error());
  EXPECT_EQ(not_found.error_value(), devicetree::PropertyDecoder::PathResolveError::kBadAlias);

  // Absolute path with alias
  auto absolute_path = decoder.ResolvePath("/foo");
  ASSERT_TRUE(absolute_path.is_ok());

  auto [abs_prefix, empty_suffix] = *absolute_path;
  EXPECT_EQ(abs_prefix, "/foo");
  EXPECT_TRUE(empty_suffix.empty());

  // empty path
  auto empty_path = decoder.ResolvePath("empty/baz");
  ASSERT_TRUE(empty_path.is_error());
  EXPECT_EQ(empty_path.error_value(), devicetree::PropertyDecoder::PathResolveError::kBadAlias);
}

TEST(PropertyDecoderTest, ResolvePathNoAlias) {
  std::optional<devicetree::Properties> aliases;

  devicetree::PropertyDecoder decoder(nullptr, devicetree::Properties(), aliases);
  auto path_with_alias = decoder.ResolvePath("foo");

  ASSERT_TRUE(path_with_alias.is_error());
  ASSERT_EQ(path_with_alias.error_value(),
            devicetree::PropertyDecoder::PathResolveError::kNoAliases);

  auto absolute_path = decoder.ResolvePath("/foo");

  ASSERT_TRUE(absolute_path.is_ok());
  auto [abs_prefix, empty_suffix] = *absolute_path;

  EXPECT_EQ(abs_prefix, "/foo");
  EXPECT_TRUE(empty_suffix.empty());
}

TEST(PropertyDecoderTest, CellCountsAndStatusAreCached) {
  // This test relies on manipulating the underlying data, to verify
  // at which point are the properties actually cached.
  PropertyBuilder builder;
  builder.Add("#address-cells", 1);
  builder.Add("#size-cells", 2);
  builder.Add("#interrupt-cells", 3);
  builder.Add("status", "okay");

  auto properties = builder.Build();

  devicetree::PropertyDecoder decoder(properties);
  {
    auto address_cells = decoder.num_address_cells();
    ASSERT_TRUE(address_cells);
    EXPECT_EQ(*address_cells, 1);

    auto size_cells = decoder.num_size_cells();
    ASSERT_TRUE(size_cells);
    EXPECT_EQ(*size_cells, 2);

    auto interrupt_cells = decoder.num_interrupt_cells();
    ASSERT_TRUE(interrupt_cells);
    EXPECT_EQ(*interrupt_cells, 3);

    auto status = decoder.status();
    ASSERT_TRUE(status);
    EXPECT_TRUE(status->is_okay());
  }
  // Update the underlying properties, if the property is not cached, it should
  // return the updated value.
  PropertyBuilder mod_builder;
  mod_builder.Add("#address-cells", 3);
  mod_builder.Add("#size-cells", 4);
  mod_builder.Add("#interrupt-cells", 5);
  mod_builder.Add("status", "barz");

  std::ignore = mod_builder.Build();

  // copy contents to the previous property block.
  memcpy(builder.property_block.data(), mod_builder.property_block.data(),
         mod_builder.property_block.size());
  memcpy(builder.string_block.data(), mod_builder.string_block.data(),
         mod_builder.string_block.size());

  // Safe check that the underlying properties have been updated.
  {
    auto address_cells = decoder.FindProperty("#address-cells");
    ASSERT_TRUE(address_cells);
    auto val = address_cells->AsUint32();
    ASSERT_TRUE(val);
    EXPECT_EQ(*val, 3);
  }

  // Now double check address cell still has the previous value.
  {
    auto address_cells = decoder.num_address_cells();
    ASSERT_TRUE(address_cells);
    EXPECT_EQ(*address_cells, 1);

    auto size_cells = decoder.num_size_cells();
    ASSERT_TRUE(size_cells);
    EXPECT_EQ(*size_cells, 2);

    auto interrupt_cells = decoder.num_interrupt_cells();
    ASSERT_TRUE(interrupt_cells);
    EXPECT_EQ(*interrupt_cells, 3);

    auto status = decoder.status();
    ASSERT_TRUE(status);
    EXPECT_TRUE(status->is_okay());
  }
}

TEST(PropertyDecoderTest, CellCountsNullOptWhenNotPresent) {
  PropertyBuilder builder;
  {
    auto props = builder.Build();
    devicetree::PropertyDecoder decoder(props);
    EXPECT_FALSE(decoder.num_address_cells());
    EXPECT_FALSE(decoder.num_size_cells());
    EXPECT_FALSE(decoder.num_interrupt_cells());
    EXPECT_FALSE(decoder.status());
  }

  builder.Add("#address-cells", 3);
  {
    auto props = builder.Build();
    devicetree::PropertyDecoder decoder(props);
    EXPECT_TRUE(decoder.num_address_cells());
    EXPECT_FALSE(decoder.num_size_cells());
    EXPECT_FALSE(decoder.num_interrupt_cells());
    EXPECT_FALSE(decoder.status());
  }

  builder.Add("#size-cells", 4);
  {
    auto props = builder.Build();
    devicetree::PropertyDecoder decoder(props);
    EXPECT_TRUE(decoder.num_address_cells());
    EXPECT_TRUE(decoder.num_size_cells());
    EXPECT_FALSE(decoder.num_interrupt_cells());
    EXPECT_FALSE(decoder.status());
  }

  builder.Add("#interrupt-cells", 5);
  {
    auto props = builder.Build();
    devicetree::PropertyDecoder decoder(props);
    EXPECT_TRUE(decoder.num_address_cells());
    EXPECT_TRUE(decoder.num_size_cells());
    EXPECT_TRUE(decoder.num_interrupt_cells());
    EXPECT_FALSE(decoder.status());
  }

  builder.Add("status", "disabled");
  {
    auto props = builder.Build();
    devicetree::PropertyDecoder decoder(props);
    EXPECT_TRUE(decoder.num_address_cells());
    EXPECT_TRUE(decoder.num_size_cells());
    EXPECT_TRUE(decoder.num_interrupt_cells());
    EXPECT_TRUE(decoder.status());
  }
}

TEST(PropertyDecoder, AddressTranslation) {
  struct Range {
    std::array<uint32_t, 1> child;
    std::array<uint32_t, 1> parent;
    std::array<uint32_t, 1> length;
  };
  static_assert(std::has_unique_object_representations_v<Range>);
  static_assert(sizeof(Range) == 12);

  std::array<Range, 2> ranges_1 = {
      Range{
          .child = {byte_swap(2000)},
          .parent = {byte_swap(1000)},
          .length = {byte_swap(50)},
      },
      Range{
          .child = {byte_swap(3000)},
          .parent = {byte_swap(1050)},
          .length = {byte_swap(50)},
      },
  };

  std::array<Range, 2> ranges_2 = {
      Range{
          .child = {byte_swap(1000)},
          .parent = {byte_swap(2000)},
          .length = {byte_swap(50)},
      },
      Range{
          .child = {byte_swap(1050)},
          .parent = {byte_swap(3000)},
          .length = {byte_swap(50)},
      },
  };

  // Creates a structure
  // root
  //   node_1 ranges_1
  //     node_2 ranges _2
  //       node_3
  //
  // The translation should of an arbitrary addres on child of node, should apply two
  // transformation to reach the view from the root, which are reverse transformations,
  // meaning |decoder.TranslateAddress(a)| = a.

  PropertyBuilder root_builder;
  root_builder.Add("#address-cells", 1);
  devicetree::PropertyDecoder root_decoder(root_builder.Build());

  devicetree::ByteView range_1_view(reinterpret_cast<uint8_t*>(ranges_1.data()),
                                    ranges_1.size() * sizeof(Range));
  PropertyBuilder node_1_builder;
  node_1_builder.Add("#address-cells", 1);
  node_1_builder.Add("ranges", range_1_view);
  devicetree::PropertyDecoder node_1_decoder(&root_decoder, node_1_builder.Build());

  devicetree::ByteView range_2_view(reinterpret_cast<uint8_t*>(ranges_2.data()),
                                    ranges_2.size() * sizeof(Range));
  PropertyBuilder node_2_builder;
  node_2_builder.Add("#address-cells", 1);
  node_2_builder.Add("ranges", range_2_view);
  devicetree::PropertyDecoder node_2_decoder(&node_1_decoder, node_2_builder.Build());

  devicetree::PropertyDecoder node_3_decoder(&node_2_decoder, {});

  // node 1 has the same domain as the root so no translation is necessary.
  EXPECT_EQ(node_1_decoder.TranslateAddress(999), 999);
  EXPECT_EQ(*node_1_decoder.TranslateAddress(1000), 1000);
  EXPECT_EQ(*node_1_decoder.TranslateAddress(1001), 1001);
  EXPECT_EQ(*node_1_decoder.TranslateAddress(1050), 1050);
  EXPECT_EQ(*node_1_decoder.TranslateAddress(1051), 1051);
  EXPECT_EQ(node_1_decoder.TranslateAddress(2000), 2000);

  //  node 2 address are subject to node 1's ranges, such that its domain
  // its properly translated into node 1's domain. Unlike node 1, node 2 translation
  // is only possible by the the ranges property set in its parent, hence 999 and 2000
  // are out of the domain.
  EXPECT_FALSE(node_2_decoder.TranslateAddress(1999));
  EXPECT_EQ(*node_2_decoder.TranslateAddress(2000), 1000);
  EXPECT_EQ(*node_2_decoder.TranslateAddress(2001), 1001);
  EXPECT_FALSE(node_2_decoder.TranslateAddress(2051));
  EXPECT_FALSE(node_2_decoder.TranslateAddress(2999));
  EXPECT_EQ(*node_2_decoder.TranslateAddress(3000), 1050);
  EXPECT_EQ(*node_2_decoder.TranslateAddress(3001), 1051);
  EXPECT_FALSE(node_2_decoder.TranslateAddress(3050));

  // Just like the relationship node 2 has with node 1, node 3 has to node 2,
  // address are transformed from one domain to the other.
  EXPECT_FALSE(node_3_decoder.TranslateAddress(999));
  EXPECT_EQ(*node_3_decoder.TranslateAddress(1000), 1000);
  EXPECT_EQ(*node_3_decoder.TranslateAddress(1001), 1001);
  EXPECT_EQ(*node_3_decoder.TranslateAddress(1050), 1050);
  EXPECT_EQ(*node_3_decoder.TranslateAddress(1051), 1051);
  EXPECT_FALSE(node_3_decoder.TranslateAddress(2000));
}

TEST(RegisterBlockPropertyTest, Accessors) {
  RegisterBlockPropertyBuilder register_block{.address_cells = 1, .size_cells = 1};
  register_block.Add(0xACED, 0xD1CE);
  register_block.Add(0xDEED, 0xFEE7);
  register_block.Add(0xDEAD, 0xBEEF);

  PropertyBuilder parent_builder;
  parent_builder.Add("#address-cells", 1);
  parent_builder.Add("#size-cells", 1);
  auto parent_props = parent_builder.Build();
  devicetree::PropertyDecoder parent_decoder(parent_props);

  PropertyBuilder builder;
  builder.Add("#address-cells", 2);
  builder.Add("#size-cells", 2);
  builder.Add("reg", register_block.property_value);
  auto props = builder.Build();
  devicetree::PropertyDecoder decoder(&parent_decoder, props);

  auto reg = decoder.FindProperty("reg");
  ASSERT_TRUE(reg);

  auto reg_block = reg->AsReg(decoder);
  ASSERT_TRUE(reg_block);

  ASSERT_EQ(reg_block->size(), 3);
  EXPECT_EQ(*(*reg_block)[0].address(), 0xACED);
  EXPECT_EQ(*(*reg_block)[0].size(), 0xD1CE);

  EXPECT_EQ(*(*reg_block)[1].address(), 0xDEED);
  EXPECT_EQ(*(*reg_block)[1].size(), 0xFEE7);

  EXPECT_EQ(*(*reg_block)[2].address(), 0xDEAD);
  EXPECT_EQ(*(*reg_block)[2].size(), 0xBEEF);
}

TEST(RegisterBlockPropertyTest, AccessorsMultipleAddressCells) {
  RegisterBlockPropertyBuilder register_block{.address_cells = 2, .size_cells = 1};
  register_block.Add(0x0000ACED0000ACED, 0xD1CE);
  register_block.Add(0x0000DEED0000ACED, 0xFEE7);
  register_block.Add(0x0000DEAD0000ACED, 0xBEEF);

  PropertyBuilder parent_builder;
  parent_builder.Add("#address-cells", 2);
  parent_builder.Add("#size-cells", 1);
  auto parent_props = parent_builder.Build();
  devicetree::PropertyDecoder parent_decoder(parent_props);

  PropertyBuilder builder;
  // Random values for cells, should pick parent cells.
  builder.Add("#address-cells", 1);
  builder.Add("#size-cells", 2);
  builder.Add("reg", register_block.property_value);
  auto props = builder.Build();
  devicetree::PropertyDecoder decoder(&parent_decoder, props);

  auto reg = decoder.FindProperty("reg");
  ASSERT_TRUE(reg);

  auto reg_block = reg->AsReg(decoder);
  ASSERT_TRUE(reg_block);

  ASSERT_EQ(reg_block->size(), 3);
  EXPECT_EQ(*(*reg_block)[0].address(), 0xACED0000ACED);
  EXPECT_EQ(*(*reg_block)[0].size(), 0xD1CE);

  EXPECT_EQ(*(*reg_block)[1].address(), 0xACED0000DEED);
  EXPECT_EQ(*(*reg_block)[1].size(), 0xFEE7);

  EXPECT_EQ(*(*reg_block)[2].address(), 0xACED0000DEAD);
  EXPECT_EQ(*(*reg_block)[2].size(), 0xBEEF);
}

TEST(RegisterBlockPropertyTest, IncorrectCellCountIsNullopt) {
  // Malformed: missing one cell (24 bytes).
  RegisterBlockPropertyBuilder register_block{.address_cells = 1, .size_cells = 1};
  register_block.Add(0xACED, 0xD1CE);
  register_block.Add(0xDEED, 0xFEE7);
  register_block.Add(0xDEAD, 0xBEEF);

  // Expecting one reg element that is 4 cells (32 bytes).
  PropertyBuilder parent_builder;
  parent_builder.Add("#address-cells", 2);
  parent_builder.Add("#size-cells", 2);
  auto parent_props = parent_builder.Build();
  devicetree::PropertyDecoder parent_decoder(parent_props);

  PropertyBuilder builder;
  builder.Add("reg", register_block.property_value);
  auto props = builder.Build();
  devicetree::PropertyDecoder decoder(&parent_decoder, props);

  auto reg = decoder.FindProperty("reg");
  ASSERT_TRUE(reg);

  auto reg_block = reg->AsReg(decoder);
  ASSERT_FALSE(reg_block);
}

TEST(RangesPropertyTest, Accessors) {
  struct Range {
    std::array<uint32_t, 2> child;
    std::array<uint32_t, 1> parent;
    std::array<uint32_t, 1> length;
  };

  std::array<Range, 4> data = {
      Range{
          .child =
              {
                  byte_swap(2),
                  byte_swap(1),
              },
          .parent =
              {
                  byte_swap(4),
              },
          .length = {byte_swap(6)},
      },
      Range{
          .child =
              {
                  byte_swap(8),
                  byte_swap(7),
              },
          .parent =
              {
                  byte_swap(10),
              },
          .length = {byte_swap(12)},
      },
      Range{
          .child =
              {
                  byte_swap(14),
                  byte_swap(13),
              },
          .parent =
              {
                  byte_swap(16),
              },
          .length = {byte_swap(18)},
      },
  };

  devicetree::ByteView view(reinterpret_cast<uint8_t*>(data.data()), data.size() * sizeof(Range));
  auto ranges_property = devicetree::RangesProperty::Create(2, 1, 1, view);
  ASSERT_TRUE(ranges_property);
  auto ranges = *ranges_property;

  auto range_0 = ranges[0];
  EXPECT_EQ(range_0.child_bus_address(), uint64_t(2) << 32 | 1);
  EXPECT_EQ(range_0.parent_bus_address(), uint64_t(4));
  EXPECT_EQ(range_0.length(), 6);

  auto range_1 = ranges[1];
  EXPECT_EQ(range_1.child_bus_address(), uint64_t(8) << 32 | 7);
  EXPECT_EQ(range_1.parent_bus_address(), uint64_t(10));
  EXPECT_EQ(range_1.length(), 12);

  auto range_2 = ranges[2];
  EXPECT_EQ(range_2.child_bus_address(), uint64_t(14) << 32 | 13);
  EXPECT_EQ(range_2.parent_bus_address(), uint64_t(16));
  EXPECT_EQ(range_2.length(), 18);
}

TEST(RangesPropertyTest, AddressTranslation) {
  struct Range {
    std::array<uint32_t, 1> child;
    std::array<uint32_t, 1> parent;
    std::array<uint32_t, 1> length;
  };
  static_assert(std::has_unique_object_representations_v<Range>);
  static_assert(sizeof(Range) == 12);

  std::array<Range, 2> data = {
      Range{
          .child = {byte_swap(1000)},
          .parent = {byte_swap(1111)},
          .length = {byte_swap(50)},
      },
      Range{
          .child = {byte_swap(1050)},
          .parent = {byte_swap(2222)},
          .length = {byte_swap(50)},
      },
  };

  devicetree::ByteView view(reinterpret_cast<uint8_t*>(data.data()), data.size() * sizeof(Range));
  auto ranges_property = devicetree::RangesProperty::Create(1, 1, 1, view);
  ASSERT_TRUE(ranges_property);
  auto ranges = *ranges_property;

  EXPECT_FALSE(ranges.TranslateChildAddress(999));
  EXPECT_EQ(ranges.TranslateChildAddress(1000), 1111);
  EXPECT_EQ(ranges.TranslateChildAddress(1001), 1112);

  EXPECT_EQ(ranges.TranslateChildAddress(1050), 2222);
  EXPECT_EQ(ranges.TranslateChildAddress(1051), 2223);
  EXPECT_FALSE(ranges.TranslateChildAddress(2100));
}

TEST(RangesPropertyTest, EmptyRanges) {
  devicetree::ByteView view;
  auto ranges_property = devicetree::RangesProperty::Create(2, 1, 1, view);
  ASSERT_TRUE(ranges_property);
  auto ranges = *ranges_property;

  EXPECT_EQ(ranges.TranslateChildAddress(999), 999);
  EXPECT_EQ(ranges.TranslateChildAddress(1000), 1000);
  EXPECT_EQ(ranges.TranslateChildAddress(1001), 1001);
  EXPECT_EQ(ranges.TranslateChildAddress(1050), 1050);
  EXPECT_EQ(ranges.TranslateChildAddress(1051), 1051);
  EXPECT_EQ(ranges.TranslateChildAddress(2100), 2100);
}

TEST(PropertyValueTest, AsRegisterBlockWithBadSizeIsNullopt) {
  RegisterBlockPropertyBuilder register_block{.address_cells = 1, .size_cells = 1};
  register_block.Add(0xACED, 0xD1CE);

  PropertyBuilder builder;
  builder.Add("reg", register_block.property_value);
  auto props = builder.Build();
  devicetree::PropertyDecoder decoder(props);

  auto reg = decoder.FindProperty("reg");
  ASSERT_TRUE(reg);

  auto reg_block = reg->AsReg(decoder);
  ASSERT_FALSE(reg_block);
}

TEST(PropertyEncodedArrayTest, DecodeFields) {
  struct Triplet {
    std::array<uint32_t, 2> field_1;
    uint32_t field_2;
    std::array<uint32_t, 2> field_3;
  };
  static_assert(std::has_unique_object_representations_v<Triplet>);

  std::array<Triplet, 4> raw_data = {
      Triplet{
          // 12  | 32 << 32
          .field_1 =
              {
                  byte_swap(12),
                  byte_swap(32),
              },
          .field_2 = byte_swap(16),
          .field_3 =
              {
                  byte_swap(0),
                  byte_swap(1),
              },
      },
      Triplet{
          .field_1 =
              {
                  byte_swap(45),
                  byte_swap(3),
              },
          .field_2 = byte_swap(17),
          .field_3 =
              {
                  byte_swap(1),
                  byte_swap(21),
              },
      },
      Triplet{
          .field_1 =
              {
                  byte_swap(451),
                  byte_swap(31),
              },
          .field_2 = byte_swap(18),
          .field_3 =
              {
                  byte_swap(15),
                  byte_swap(22),
              },
      },
      Triplet{
          .field_1 =
              {
                  byte_swap(454),
                  byte_swap(34),
              },
          .field_2 = byte_swap(155),
          .field_3 =
              {
                  byte_swap(150),
                  byte_swap(220),
              },
      },
  };
  devicetree::ByteView data(reinterpret_cast<uint8_t*>(raw_data.data()),
                            raw_data.size() * sizeof(Triplet));
  devicetree::PropEncodedArray<devicetree::PropEncodedArrayElement<3>>
      triplet_property_encoded_array(data, /*field_1 num cells*/ 2, /*field_2 num cells*/ 1,
                                     /*field_3 num_cells*/ 2);

  auto encoded_triplet_0 = triplet_property_encoded_array[0];

  EXPECT_EQ(*encoded_triplet_0[0], uint64_t(12) << 32 | 32);
  EXPECT_EQ(*encoded_triplet_0[1], uint64_t(16));
  EXPECT_EQ(*encoded_triplet_0[2], uint64_t(0) << 32 | 1);

  auto encoded_triplet_1 = triplet_property_encoded_array[1];

  EXPECT_EQ(*encoded_triplet_1[0], uint64_t(45) << 32 | 3);
  EXPECT_EQ(*encoded_triplet_1[1], uint64_t(17));
  EXPECT_EQ(*encoded_triplet_1[2], uint64_t(1) << 32 | 21);

  auto encoded_triplet_2 = triplet_property_encoded_array[2];

  EXPECT_EQ(*encoded_triplet_2[0], uint64_t(451) << 32 | 31);
  EXPECT_EQ(*encoded_triplet_2[1], uint64_t(18));
  EXPECT_EQ(*encoded_triplet_2[2], uint64_t(15) << 32 | 22);

  auto encoded_triplet_3 = triplet_property_encoded_array[3];

  EXPECT_EQ(*encoded_triplet_3[0], uint64_t(454) << 32 | 34);
  EXPECT_EQ(*encoded_triplet_3[1], uint64_t(155));
  EXPECT_EQ(*encoded_triplet_3[2], uint64_t(150) << 32 | 220);
}

TEST(PropertyEncodedArrayTest, DecodeFieldsWithZeroSize) {
  struct Triplet {
    std::array<uint32_t, 2> field_1;
    // field_2 0 size.
    uint32_t field_3;
  };

  std::array<Triplet, 2> raw_data = {
      Triplet{.field_1 = {byte_swap(456), byte_swap(123)}, .field_3 = byte_swap(2)},
      Triplet{.field_1 = {byte_swap(456), byte_swap(124)}, .field_3 = byte_swap(3)},
  };

  devicetree::ByteView data(reinterpret_cast<uint8_t*>(raw_data.data()),
                            raw_data.size() * sizeof(Triplet));
  devicetree::PropEncodedArray<devicetree::PropEncodedArrayElement<3>>
      triplet_property_encoded_array(data, /*field_1 num cells*/ 2, /*field_2 num cells*/ 0,
                                     /*field_3 num_cells*/ 1);

  auto encoded_triplet_0 = triplet_property_encoded_array[0];

  EXPECT_EQ(*encoded_triplet_0[0], uint64_t(456) << 32 | 123);
  EXPECT_FALSE(encoded_triplet_0[1].has_value());
  EXPECT_EQ(*encoded_triplet_0[2], 2);

  auto encoded_triplet_1 = triplet_property_encoded_array[1];

  EXPECT_EQ(*encoded_triplet_1[0], uint64_t(456) << 32 | 124);
  EXPECT_FALSE(encoded_triplet_1[1].has_value());
  EXPECT_EQ(*encoded_triplet_1[2], 3);
}

TEST(StatusPropertyTest, CreateNullopt) {
  auto prop = devicetree::StatusProperty::Create(std::nullopt);
  ASSERT_FALSE(prop);
}

TEST(StatusPropertyTest, CreateFromOkay) {
  auto prop = devicetree::StatusProperty::Create("okay");
  EXPECT_TRUE(prop);
  EXPECT_TRUE(prop->is_okay());
  EXPECT_FALSE(prop->is_fail());
  EXPECT_FALSE(prop->is_disabled());
  EXPECT_FALSE(prop->fail_code());
}

TEST(StatusPropertyTest, CreateFromOk) {
  auto prop = devicetree::StatusProperty::Create("ok");
  EXPECT_TRUE(prop);
  EXPECT_TRUE(prop->is_okay());
  EXPECT_FALSE(prop->is_fail());
  EXPECT_FALSE(prop->is_disabled());
  EXPECT_FALSE(prop->fail_code());
}

TEST(StatusPropertyTest, CreateFromDisabled) {
  auto prop = devicetree::StatusProperty::Create("disabled");
  ASSERT_TRUE(prop);
  EXPECT_TRUE(prop->is_disabled());
  EXPECT_FALSE(prop->is_fail());
  EXPECT_FALSE(prop->is_okay());
  EXPECT_FALSE(prop->fail_code());
}

TEST(StatusPropertyTest, CreateFromFail) {
  auto prop = devicetree::StatusProperty::Create("fail");
  ASSERT_TRUE(prop);
  EXPECT_TRUE(prop->is_fail());
  EXPECT_FALSE(prop->is_disabled());
  EXPECT_FALSE(prop->is_okay());
  EXPECT_FALSE(prop->fail_code());
}

TEST(StatusPropertyTest, CreateFromFailWithCode) {
  auto prop = devicetree::StatusProperty::Create("fail-abc");
  ASSERT_TRUE(prop);
  EXPECT_TRUE(prop->is_fail());
  EXPECT_FALSE(prop->is_disabled());
  EXPECT_FALSE(prop->is_okay());
  EXPECT_EQ(prop->fail_code(), "abc");
}

TEST(StatusPropertyTest, CreateFromBadStr) {
  auto prop = devicetree::StatusProperty::Create("what-is-this");
  ASSERT_FALSE(prop);
}

}  // namespace
