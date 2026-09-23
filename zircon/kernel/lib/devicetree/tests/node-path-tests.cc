// Copyright 2022 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/devicetree/devicetree.h>

#include <array>
#include <memory>
#include <span>
#include <string_view>
#include <type_traits>

#include <zxtest/zxtest.h>

namespace {

using Comparison = devicetree::NodePath::Comparison;

auto as_bytes = [](auto& val) {
  using byte_type = std::conditional_t<std::is_const_v<std::remove_reference_t<decltype(val)>>,
                                       const uint8_t, uint8_t>;
  return std::span<byte_type>(reinterpret_cast<byte_type*>(&val), sizeof(val));
};

auto append = [](auto& vec, auto&& other) { vec.insert(vec.end(), other.begin(), other.end()); };

uint32_t AsBigEndian(uint32_t val) {
  auto bytes = as_bytes(val);
  return static_cast<uint32_t>(bytes[0]) << 24 | static_cast<uint32_t>(bytes[1]) << 16 |
         static_cast<uint32_t>(bytes[2]) << 8 | static_cast<uint32_t>(bytes[3]);
}

struct AliasContext {
  devicetree::Properties properties() {
    return devicetree::Properties(
        {property_block.data(), property_block.size()},
        std::string_view(reinterpret_cast<const char*>(string_block.data()), string_block.size()));
  }

  void Add(std::string_view alias, std::string_view absolute_path) {
    constexpr std::array<uint8_t, sizeof(uint32_t) - 1> kPadding = {};

    uint32_t name_off = AsBigEndian(static_cast<uint32_t>(string_block.size()));
    // String must be null terminated.
    append(string_block, alias);
    string_block.push_back('\0');

    // Add the null terminator.
    uint32_t len = static_cast<uint32_t>(absolute_path.size()) + 1;
    std::span<const uint8_t> padding;
    if (auto remainder = len % sizeof(uint32_t); remainder != 0) {
      padding = std::span(kPadding).subspan(0, sizeof(uint32_t) - remainder);
    }
    len = AsBigEndian(len);

    if (!property_block.empty()) {
      const uint32_t kFdtPropToken = AsBigEndian(0x00000003);
      append(property_block, as_bytes(kFdtPropToken));
    }
    append(property_block, as_bytes(len));
    append(property_block, as_bytes(name_off));
    append(property_block, absolute_path);
    property_block.push_back('\0');
    append(property_block, padding);
  }

  std::vector<uint8_t> property_block;
  std::vector<uint8_t> string_block;
};

std::vector<std::string_view> ConvertPath(std::string_view path) {
  ZX_ASSERT(!path.empty());

  std::vector<std::string_view> components;
  while (!path.empty()) {
    size_t component_end = path.find('/');
    components.push_back(path.substr(0, component_end));
    path.remove_prefix(component_end != std::string_view::npos ? component_end : path.size());
    if (!path.empty()) {
      // remove '/'
      path.remove_prefix(1);
    }
  }
  return components;
}

struct NodePathHelper {
  NodePathHelper() = default;
  NodePathHelper(const NodePathHelper&) = delete;
  NodePathHelper(NodePathHelper&&) = default;
  ~NodePathHelper() {
    while (!path.is_empty()) {
      // The pointers will get released as part of nodes destructor.
      std::ignore = path.pop_back();
    }
  }

  std::vector<std::unique_ptr<devicetree::Node>> nodes;
  devicetree::NodePath path;
};

NodePathHelper ConvertToNodePath(std::string_view path) {
  NodePathHelper path_helper;
  auto components = ConvertPath(path);
  for (auto component : components) {
    path_helper.nodes.push_back(std::make_unique<devicetree::Node>(component));
    path_helper.path.push_back(path_helper.nodes.back().get());
  }

  return path_helper;
}

auto ToResolvedPath(std::string_view path,
                    std::optional<devicetree::Properties> aliases = std::nullopt) {
  devicetree::PropertyDecoder decoder(nullptr, devicetree::Properties(), aliases);
  auto resolved = decoder.ResolvePath(path);
  ZX_ASSERT(resolved.is_ok());
  return resolved.value();
}

TEST(NodePathTest, AbsolutePathMismatchSameLength) {
  {
    auto [nodes, node_path] = ConvertToNodePath("/A/B/C/D");
    std::string_view target_path = "/A/B/E/D";

    EXPECT_NE(target_path, node_path);
    EXPECT_FALSE(node_path.IsAncestorOf(target_path));
    EXPECT_FALSE(node_path.IsDescendentOf(target_path));
    EXPECT_EQ(Comparison::kMismatch, node_path.CompareWith(target_path));
  }
  {
    auto [nodes, node_path] = ConvertToNodePath("/A/B");
    std::string_view target_path = "/A/C/E/D";

    EXPECT_NE(target_path, node_path);
    EXPECT_FALSE(node_path.IsAncestorOf(target_path));
    EXPECT_FALSE(node_path.IsDescendentOf(target_path));
    EXPECT_EQ(Comparison::kMismatch, node_path.CompareWith(target_path));
  }
  {
    auto [nodes, node_path] = ConvertToNodePath("/A/C/E/D");
    std::string_view target_path = "/A/B";

    EXPECT_NE(target_path, node_path);
    EXPECT_FALSE(node_path.IsAncestorOf(target_path));
    EXPECT_FALSE(node_path.IsDescendentOf(target_path));
    EXPECT_EQ(Comparison::kMismatch, node_path.CompareWith(target_path));
  }
}

TEST(NodePathTest, AbsolutePathMatch) {
  auto [nodes, node_path] = ConvertToNodePath("/A/B/C/D");
  std::string_view target_path = "/A/B/C/D";

  EXPECT_EQ(target_path, node_path);
  EXPECT_FALSE(node_path.IsAncestorOf(target_path));
  EXPECT_FALSE(node_path.IsDescendentOf(target_path));
  EXPECT_EQ(Comparison::kEqual, node_path.CompareWith(target_path));
}

TEST(NodePathTest, AbsolutePathAncestor) {
  {
    auto [nodes, node_path] = ConvertToNodePath("/");
    std::string_view target_path = "/A/B/C/D";

    EXPECT_TRUE(node_path.IsAncestorOf(target_path));
    EXPECT_FALSE(node_path.IsParentOf(target_path));
    EXPECT_EQ(Comparison::kIndirectAncestor, node_path.CompareWith(target_path));

    EXPECT_NE(target_path, node_path);
    EXPECT_FALSE(node_path.IsDescendentOf(target_path));
  }
  {
    auto [nodes, node_path] = ConvertToNodePath("/A/B");
    std::string_view target_path = "/A/B/C/D";

    EXPECT_TRUE(node_path.IsAncestorOf(target_path));
    EXPECT_FALSE(node_path.IsParentOf(target_path));
    EXPECT_EQ(Comparison::kIndirectAncestor, node_path.CompareWith(target_path));

    EXPECT_NE(target_path, node_path);
    EXPECT_FALSE(node_path.IsDescendentOf(target_path));
  }
  {
    auto [nodes, node_path] = ConvertToNodePath("/A/B/C");
    std::string_view target_path = "/A/B/C/D";

    EXPECT_TRUE(node_path.IsAncestorOf(target_path));
    EXPECT_TRUE(node_path.IsParentOf(target_path));
    EXPECT_EQ(Comparison::kParent, node_path.CompareWith(target_path));

    EXPECT_NE(target_path, node_path);
    EXPECT_FALSE(node_path.IsDescendentOf(target_path));
  }
  {
    // A path is never an ancestor of itself.
    auto [nodes, node_path] = ConvertToNodePath("/A/B/C");
    std::string_view target_path = "/A/B/C";

    EXPECT_FALSE(node_path.IsAncestorOf(target_path));
    EXPECT_FALSE(node_path.IsParentOf(target_path));
  }
}

TEST(NodePathTest, AbsolutePathDescendent) {
  {
    auto [nodes, node_path] = ConvertToNodePath("/A/B/C/D");
    std::string_view target_path = "/";

    EXPECT_TRUE(node_path.IsDescendentOf(target_path));
    EXPECT_FALSE(node_path.IsChildOf(target_path));
    EXPECT_EQ(Comparison::kIndirectDescendent, node_path.CompareWith(target_path));

    EXPECT_NE(target_path, node_path);
    EXPECT_FALSE(node_path.IsAncestorOf(target_path));
  }
  {
    auto [nodes, node_path] = ConvertToNodePath("/A/B/C/D");
    std::string_view target_path = "/A/B";

    EXPECT_TRUE(node_path.IsDescendentOf(target_path));
    EXPECT_FALSE(node_path.IsChildOf(target_path));
    EXPECT_EQ(Comparison::kIndirectDescendent, node_path.CompareWith(target_path));

    EXPECT_NE(target_path, node_path);
    EXPECT_FALSE(node_path.IsAncestorOf(target_path));
  }
  {
    auto [nodes, node_path] = ConvertToNodePath("/A/B/C/D");
    std::string_view target_path = "/A/B/C";

    EXPECT_TRUE(node_path.IsDescendentOf(target_path));
    EXPECT_TRUE(node_path.IsChildOf(target_path));
    EXPECT_EQ(Comparison::kChild, node_path.CompareWith(target_path));

    EXPECT_NE(target_path, node_path);
    EXPECT_FALSE(node_path.IsAncestorOf(target_path));
  }
  {
    // A path is never a descendent of itself.
    auto [nodes, node_path] = ConvertToNodePath("/A/B/C");
    std::string_view target_path = "/A/B/C";

    EXPECT_FALSE(node_path.IsDescendentOf(target_path));
    EXPECT_FALSE(node_path.IsChildOf(target_path));
  }
}

TEST(NodePathTest, AliasedPathMismatch) {
  AliasContext aliases;
  aliases.Add("alias", "/A/B/D");

  {  // Stem mismatch with non empty leaf.
    auto [nodes, node_path] = ConvertToNodePath("/A/B/C/D");
    auto target_path = ToResolvedPath("alias/D", aliases.properties());

    EXPECT_NE(target_path, node_path);
    EXPECT_FALSE(node_path.IsAncestorOf(target_path));
    EXPECT_FALSE(node_path.IsDescendentOf(target_path));
    EXPECT_EQ(Comparison::kMismatch, node_path.CompareWith(target_path));
  }
  {  // Stem mismatch with empty leaf
    auto [nodes, node_path] = ConvertToNodePath("/A/B/C/D");
    auto target_path = ToResolvedPath("alias", aliases.properties());

    EXPECT_NE(target_path, node_path);
    EXPECT_FALSE(node_path.IsAncestorOf(target_path));
    EXPECT_FALSE(node_path.IsDescendentOf(target_path));
    EXPECT_EQ(Comparison::kMismatch, node_path.CompareWith(target_path));
  }
  {  // Stem match left mismatch.
    auto [nodes, node_path] = ConvertToNodePath("/A/B/D/D");
    auto target_path = ToResolvedPath("alias/C", aliases.properties());

    EXPECT_NE(target_path, node_path);
    EXPECT_FALSE(node_path.IsAncestorOf(target_path));
    EXPECT_FALSE(node_path.IsDescendentOf(target_path));
    EXPECT_EQ(Comparison::kMismatch, node_path.CompareWith(target_path));
  }
}

TEST(NodePathTest, AliasedPathAncestor) {
  AliasContext aliases;
  aliases.Add("alias", "/A/B/D");

  {  // Root is ancestor of every node.
    auto [nodes, node_path] = ConvertToNodePath("/");
    auto target_path = ToResolvedPath("alias", aliases.properties());

    EXPECT_TRUE(node_path.IsAncestorOf(target_path));
    EXPECT_FALSE(node_path.IsParentOf(target_path));
    EXPECT_EQ(Comparison::kIndirectAncestor, node_path.CompareWith(target_path));
  }

  {  // Ancestor of stem Empty leaf
    auto [nodes, node_path] = ConvertToNodePath("/A/B");
    auto target_path = ToResolvedPath("alias", aliases.properties());

    EXPECT_TRUE(node_path.IsAncestorOf(target_path));
    EXPECT_TRUE(node_path.IsParentOf(target_path));
    EXPECT_EQ(Comparison::kParent, node_path.CompareWith(target_path));
  }

  {  // Ancestor of stem non empty leaf
    auto [nodes, node_path] = ConvertToNodePath("/A/B");
    auto target_path = ToResolvedPath("alias/C", aliases.properties());

    EXPECT_TRUE(node_path.IsAncestorOf(target_path));
    EXPECT_FALSE(node_path.IsParentOf(target_path));
    EXPECT_EQ(Comparison::kIndirectAncestor, node_path.CompareWith(target_path));
  }

  {  // Ancestor of leaf (stem matches)
    auto [nodes, node_path] = ConvertToNodePath("/A/B/D");
    auto target_path = ToResolvedPath("alias/C", aliases.properties());

    EXPECT_TRUE(node_path.IsAncestorOf(target_path));
    EXPECT_TRUE(node_path.IsParentOf(target_path));
    EXPECT_EQ(Comparison::kParent, node_path.CompareWith(target_path));
  }
}

TEST(NodePathTest, AliasedPathDescendent) {
  AliasContext aliases;
  aliases.Add("alias", "/A");

  {  // Current node is descendant of the alias.
    auto [nodes, node_path] = ConvertToNodePath("/A/B/C");
    auto target_path = ToResolvedPath("alias", aliases.properties());

    EXPECT_TRUE(node_path.IsDescendentOf(target_path));
    EXPECT_FALSE(node_path.IsChildOf(target_path));
    EXPECT_EQ(Comparison::kIndirectDescendent, node_path.CompareWith(target_path));
  }

  {  // Current Node is descendant of the alias with the leaf.
    auto [nodes, node_path] = ConvertToNodePath("/A/B/C");
    auto target_path = ToResolvedPath("alias/B", aliases.properties());

    EXPECT_TRUE(node_path.IsDescendentOf(target_path));
    EXPECT_TRUE(node_path.IsChildOf(target_path));
    EXPECT_EQ(Comparison::kChild, node_path.CompareWith(target_path));
  }
}

TEST(NodePathTest, AliasedPathMatches) {
  AliasContext aliases;
  aliases.Add("alias", "/A");

  {  // Stem only match
    auto [nodes, node_path] = ConvertToNodePath("/A");
    auto target_path = ToResolvedPath("alias", aliases.properties());

    EXPECT_EQ(target_path, node_path);
    EXPECT_EQ(Comparison::kEqual, node_path.CompareWith(target_path));
  }

  {  // Stem and leaf match
    auto [nodes, node_path] = ConvertToNodePath("/A/B/C");
    auto target_path = ToResolvedPath("alias/B/C", aliases.properties());

    EXPECT_EQ(target_path, node_path);
    EXPECT_EQ(Comparison::kEqual, node_path.CompareWith(target_path));
  }
}

}  // namespace
