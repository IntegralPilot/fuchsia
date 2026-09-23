// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/flatland/scene_dumper.h"

#include <lib/stdcompat/string_view.h>
#include <lib/ui/scenic/cpp/view_ref_pair.h>
#include <lib/zx/eventpair.h>

#include <iostream>
#include <memory>
#include <sstream>
#include <string>
#include <vector>

#include <gtest/gtest.h>

#include "gtest/gtest-matchers.h"
#include "sdk/lib/syslog/cpp/macros.h"
#include "src/lib/fsl/handles/object_info.h"
#include "src/ui/lib/escher/geometry/types.h"
#include "src/ui/scenic/lib/allocation/id.h"
#include "src/ui/scenic/lib/allocation/image_metadata.h"
#include "src/ui/scenic/lib/flatland/global_matrix_data.h"
#include "src/ui/scenic/lib/flatland/transform_handle.h"
#include "src/ui/scenic/lib/flatland/uber_struct_system.h"

using allocation::ImageMetadata;
using flatland::SrcToDest;

namespace {

// Ignored lines at the start of the dump containing user-readable formatting but no relevant
// information for testing.
constexpr int kIgnoredLinesAtStartOfDump = 3;

// Instance topologies are dumped on the same line containing this token.
constexpr char kInstanceDumpLineIdentifierToken[] = "Session";

// Images are dumped on the same line containing this token.
constexpr char kImageDumpLineIdentifierToken[] = "image:";

// Hit regions are dumped on the lines after this token.
constexpr char kHitRegionsDumpLineIdentifierToken[] = "Hit Regions";

// Hit regions are dumped on the same line containing this token.
constexpr char kHitRegionDumpLineIdentifierToken[] = "transform:";

constexpr flatland::TransformHandle::InstanceId kLinkInstanceId = 0;

// Creates a link in |links| to the the graph rooted at |instance_id:transform_id|. If transform_id
// is not specified, it is initialized as id 0.
void MakeLink(flatland::GlobalTopologyData::LinkTopologyMap& links, uint64_t instance_id,
              uint64_t transform_id = 0) {
  links[{kLinkInstanceId, instance_id}] = {instance_id, transform_id};
}

// Returns lines from a scene dump input stream. Ignores |kIgnoredLinesAtStartOfDump| lines at the
// start of the line. The returned lines begin at the root topology node.
std::vector<std::string> GetLines(std::istream& input) {
  std::vector<std::string> lines;
  std::string line;
  auto ignored_lines = 0;
  while (std::getline(input, line)) {
    if (++ignored_lines > kIgnoredLinesAtStartOfDump) {
      lines.push_back(line);
    }
  }
  return lines;
}

std::string NodeStr(flatland::TransformHandle& node) {
  std::ostringstream output;
  output << node.GetInstanceId() << ":" << node.GetTransformId();
  return output.str();
}

std::string ImageStr(ImageMetadata image) {
  std::ostringstream output;
  output << image;
  return output.str();
}

std::string GeometryStr(SrcToDest geometry) {
  std::ostringstream output;
  output << geometry;
  return output.str();
}

// The topology dump of the scene processes the scene such that each transform node is on its own
// line, with children node indented and sibling nodes at the same indentation level. For example,
// assuming, A is the root node, B and C are direct children, and D, E, F and G, H are children of B
// and C -- the output (ignoring further formatting other than indentation) appears as the
// following:
// A
//     B
//         D
//         E
//     C
//         G
//         H
//
// Any debug names for a particular node appears above the node (on a separate line) with the same
// indentation as the node. For instance:
// A
//     Node_B_Name
//     B
//     C
//
// The following helper functions test depth level (i.e. A has depth of 1; B and C have depth of 2;
// D, E, G, and H have depth of 3).

// Expect a transform handle to be dumped at the specified line. Returns the position, within the
// line, of where the transform handle begins.
size_t ExpectNodeLineNumberAndGetIndex(flatland::TransformHandle& node, size_t node_line_number,
                                       const std::vector<std::string>& line_dump) {
  std::string node_name = NodeStr(node);
  auto node_index = line_dump[node_line_number].find(node_name);
  EXPECT_NE(node_index, std::string::npos);
  return node_index;
}

// Expect a transform handle debug name at the specified line. Returns the position, within the
// line, of where the debug name begins.
size_t ExpectNameLineNumberAndGetIndex(const std::string& name, size_t name_line_number,
                                       const std::vector<std::string>& line_dump) {
  auto name_index = line_dump[name_line_number].find(name);
  EXPECT_NE(name_index, std::string::npos);
  return name_index;
}

// Expect topology node A to have less depth level than topology node B. I.e. node A is closer to
// (or is) the root node than topology node B.
void ExpectTopologyNodeHasLessDepthLevel(flatland::TransformHandle node_a,
                                         size_t node_a_line_number,
                                         flatland::TransformHandle node_b,
                                         size_t node_b_line_number,
                                         const std::vector<std::string>& line_dump) {
  ASSERT_LT(node_a_line_number, line_dump.size());
  ASSERT_LT(node_b_line_number, line_dump.size());
  const auto node_a_index = ExpectNodeLineNumberAndGetIndex(node_a, node_a_line_number, line_dump);
  const auto node_b_index = ExpectNodeLineNumberAndGetIndex(node_b, node_b_line_number, line_dump);
  EXPECT_LT(node_a_index, node_b_index);
}

// Expect topology node A and topology node B to have the same depth level. I.e. node A and node B
// are the same number of 'hops' from the root node.
void ExpectTopologyNodeHasSameDepthLevel(flatland::TransformHandle node_a,
                                         size_t node_a_line_number,
                                         flatland::TransformHandle node_b,
                                         size_t node_b_line_number,
                                         const std::vector<std::string>& line_dump) {
  ASSERT_LT(node_a_line_number, line_dump.size());
  ASSERT_LT(node_b_line_number, line_dump.size());
  const auto node_a_index = ExpectNodeLineNumberAndGetIndex(node_a, node_a_line_number, line_dump);
  const auto node_b_index = ExpectNodeLineNumberAndGetIndex(node_b, node_b_line_number, line_dump);
  EXPECT_EQ(node_a_index, node_b_index);
}

// Expect the specified node to be dumped with the specified name printed above.
void ExpectNodeNameAndKoid(flatland::TransformHandle node, size_t node_line_number,
                           const std::string& name, zx_koid_t koid,
                           const std::vector<std::string>& line_dump) {
  ASSERT_LT(node_line_number, line_dump.size());
  const auto node_index = ExpectNodeLineNumberAndGetIndex(node, node_line_number, line_dump);
  const auto name_index = ExpectNameLineNumberAndGetIndex(
      std::string("(") + name + std::string(" koid:") + std::to_string(koid) + std::string(")"),
      node_line_number, line_dump);
  EXPECT_GT(name_index, node_index);  // Name appears to right of node.
}

// Returns the line number containing an instance dump, i.e. a line beginning with
// `kInstanceDumpLineIdentifierToken` followed by the `instance_id`.
size_t FindInstanceDumpLineNumber(const std::vector<std::string>& line_dump,
                                  flatland::TransformHandle::InstanceId instance_id) {
  for (size_t i = 0; i < line_dump.size(); i++) {
    if (line_dump[i].starts_with(std::string(kInstanceDumpLineIdentifierToken) + " " +
                                 std::to_string(instance_id))) {
      return i;
    }
  }
  return -1;
}

// Checks that the total number of instances dumped (and |kInstanceDumpLineIdentifierToken|) matches
// the expectation.
void ExpectInstanceDumpCount(const std::vector<std::string>& line_dump, int expected_count) {
  int count = 0;
  for (auto& line : line_dump) {
    if (line.starts_with(kInstanceDumpLineIdentifierToken)) {
      count++;
    }
  }
  EXPECT_EQ(count, expected_count);
}

// Sets expectations that the instance is dumped alongside its associated topology.
// Returns the line number of the line following the instance dump.
void ExpectInstanceDump(flatland::TransformHandle::InstanceId instance_id, const std::string& name,
                        const std::vector<std::string>& line_dump) {
  auto line_number = FindInstanceDumpLineNumber(line_dump, instance_id);
  ASSERT_LT(line_number, line_dump.size());
  if (name.empty()) {
    // There shouldn't be an opening or closing bracket for the name.
    EXPECT_FALSE(cpp23::contains(line_dump[line_number], '('));
    EXPECT_FALSE(cpp23::contains(line_dump[line_number], ')'));
  } else {
    auto instance_str = std::string(" (") + name + std::string(")");
    ExpectNameLineNumberAndGetIndex(instance_str, line_number, line_dump);
  }
}

// Checks that the total number of hit regions dumped.
void ExpectHitRegionDumpCount(const std::vector<std::string>& line_dump, int expected_count) {
  size_t hit_region_line = 0;
  for (size_t i = 0; i < line_dump.size(); ++i) {
    if (cpp23::contains(line_dump[i], kHitRegionsDumpLineIdentifierToken)) {
      hit_region_line = i;
      break;
    }
  }
  int count = 0;
  for (size_t i = hit_region_line + 1; i < line_dump.size(); ++i) {
    if (cpp23::contains(line_dump[i], kHitRegionDumpLineIdentifierToken)) {
      count++;
    }
  }
  EXPECT_EQ(count, expected_count);
}

void ExpectHitRegionDump(const std::vector<std::string>& line_dump,
                         const flatland::TransformHandle& transform,
                         const types::RectangleF& hit_region) {
  size_t hit_region_line = 0;
  for (size_t i = 0; i < line_dump.size(); ++i) {
    if (cpp23::contains(line_dump[i], kHitRegionsDumpLineIdentifierToken)) {
      hit_region_line = i;
      break;
    }
  }

  {
    std::ostringstream expected_transform_line;
    expected_transform_line << "transform: (" << transform.GetInstanceId() << ":"
                            << transform.GetTransformId() << ")";
    const std::string& transform_line = line_dump[hit_region_line + 1];
    EXPECT_TRUE(cpp23::contains(transform_line, expected_transform_line.str()))
        << "Failed to find \"" << expected_transform_line.str() << "\" in:\n"
        << transform_line;
  }

  {
    std::ostringstream expected_region_line;
    expected_region_line << "region: {x=" << hit_region.x() << ", y=" << hit_region.y()
                         << ", width=" << hit_region.width() << ", height=" << hit_region.height()
                         << "}";
    const std::string& region_line = line_dump[hit_region_line + 2];
    EXPECT_TRUE(cpp23::contains(region_line, expected_region_line.str()))
        << "Failed to find \"" << expected_region_line.str() << "\" in:\n"
        << region_line;
  }
}

// Find the line number containing an image dump (and also |kImageDumpLineIdentifierToken|). Returns
// the line number of the following line (after the found image). This can then be specified as
// |beginning_at| to find subsequent images.
size_t FindImageDumpLineNumber(const std::vector<std::string>& line_dump, size_t beginning_at = 0) {
  for (size_t i = beginning_at; i < line_dump.size(); i++) {
    if (cpp23::contains(line_dump[i], kImageDumpLineIdentifierToken)) {
      return i;
    }
  }
  return -1;
}

// Checks that the total number of images dumped (and |kImageDumpLineIdentifierToken|) matches the
// expectation.
void ExpectImageDumpCount(const std::vector<std::string>& line_dump, int expected_count) {
  int count = 0;
  for (auto& line : line_dump) {
    if (cpp23::contains(line, kImageDumpLineIdentifierToken)) {
      count++;
    }
  }
  EXPECT_EQ(count, expected_count);
}

// Sets expectations that the image is dumped alongside its associated transform and geometry.
// Returns the line number of the line following the image dump. This can then be used to specify
// |beginning_at| to check subsequent image dumps.
size_t ExpectImageDump(ImageMetadata image, flatland::TransformHandle node, SrcToDest geometry,
                       const std::vector<std::string>& line_dump, size_t beginning_at = 0) {
  auto line_number = FindImageDumpLineNumber(line_dump, beginning_at);
  EXPECT_LE(line_number, (size_t)-1);
  EXPECT_TRUE(cpp23::contains(line_dump[line_number++], ImageStr(image)));
  EXPECT_TRUE(cpp23::contains(line_dump[line_number++], NodeStr(node)));
  EXPECT_TRUE(cpp23::contains(line_dump[line_number++], GeometryStr(geometry)));
  return line_number;
}

}  // namespace

namespace flatland::test {

TEST(SceneDumperTest, TopologyTree) {
  UberStruct::InstanceMap uber_structs;
  GlobalTopologyData::LinkTopologyMap links;

  const TransformGraph::TopologyVector vectors[] = {
      {{{1, 0}, 2}, {{0, 2}, 0}, {{0, 5}, 0}},  // 1:0 - 0:5
                                                //    \
                                                //     0:2
                                                //
      {{{2, 0}, 2}, {{0, 3}, 0}, {{0, 4}, 0}},  // 2:0 - 0:4
                                                //    \
                                                //     0:3
                                                //
      {{{3, 0}, 0}},                            // 3:0
      {{{4, 0}, 0}},                            // 4:0
      {{{5, 0}, 0}}                             // 5:0
  };

  for (const auto& v : vectors) {
    auto uber_struct = std::make_unique<UberStruct>();
    uber_struct->local_topology.assign(v.begin(), v.end());
    uber_structs[v[0].handle.GetInstanceId()] = std::move(uber_struct);
  }

  MakeLink(links, 2);  // 0:2 - 2:0
  MakeLink(links, 3);  // 0:3 - 3:0
  MakeLink(links, 4);  // 0:4 - 4:0
  MakeLink(links, 5);  // 0:5 - 5:0

  const TransformHandle hit_region_transform(2, 0);
  const types::RectangleF hit_region({.x = 1.f, .y = 2.f, .width = 3.f, .height = 4.f});
  // In production, UberStructs are const after being produced by the Flatland session, but it's
  // convenient and harmless to modify (only one of) them here.
  std::const_pointer_cast<UberStruct>(uber_structs.begin()->second)
      ->local_hit_regions_map[hit_region_transform]
      .emplace_back(hit_region);

  std::stringstream output;

  auto topology_data =
      GlobalTopologyData::ComputeGlobalTopologyData(uber_structs, links, 0, {1, 0});

  DumpScene(uber_structs, topology_data, {}, output);
  auto lines = GetLines(output);

  // {1, 0} is the root with {2, 0} on the next line as child.
  ExpectTopologyNodeHasLessDepthLevel({1, 0}, 0, {2, 0}, 1, lines);
  // {2, 0} has two children - {3, 0} and {4, 0}.
  ExpectTopologyNodeHasLessDepthLevel({2, 0}, 1, {3, 0}, 2, lines);
  ExpectTopologyNodeHasSameDepthLevel({3, 0}, 2, {4, 0}, 3, lines);
  // {5, 0} is direct child of {1, 0} and sibling of {2, 0}.
  ExpectTopologyNodeHasLessDepthLevel({1, 0}, 0, {5, 0}, 4, lines);
  ExpectTopologyNodeHasSameDepthLevel({2, 0}, 1, {5, 0}, 4, lines);

  ExpectInstanceDumpCount(lines, 5);
  ExpectInstanceDump(1, "", lines);
  ExpectInstanceDump(2, "", lines);
  ExpectInstanceDump(3, "", lines);
  ExpectInstanceDump(4, "", lines);
  ExpectInstanceDump(5, "", lines);

  ExpectHitRegionDumpCount(lines, 1);
  ExpectHitRegionDump(lines, hit_region_transform, hit_region);
}

TEST(SceneDumperTest, TopologyTreeDeep) {
  UberStruct::InstanceMap uber_structs;
  GlobalTopologyData::LinkTopologyMap links;

  const TransformGraph::TopologyVector vectors[] = {
      {{{1, 0}, 2}, {{0, 2}, 0}, {{0, 6}, 0}},  // 1:0 - 0:2
      {{{2, 0}, 1}, {{0, 3}, 0}},               // 2:0 - 0:3
      {{{3, 0}, 1}, {{0, 4}, 0}},               // 3:0 - 4:0
      {{{4, 0}, 1}, {{0, 5}, 0}},               // 4:0 - 5:0
      {{{5, 0}, 0}},                            // 5:0
      {{{6, 0}, 0}}                             // 6:0
  };

  for (const auto& v : vectors) {
    auto uber_struct = std::make_unique<UberStruct>();
    uber_struct->local_topology.assign(v.begin(), v.end());
    uber_structs[v[0].handle.GetInstanceId()] = std::move(uber_struct);
  }

  MakeLink(links, 2);  // 0:2 - 2:0
  MakeLink(links, 3);  // 0:3 - 3:0
  MakeLink(links, 4);  // 0:4 - 4:0
  MakeLink(links, 5);  // 0:5 - 5:0
  MakeLink(links, 6);  // 0:5 - 5:0

  std::stringstream output;

  auto topology_data =
      GlobalTopologyData::ComputeGlobalTopologyData(uber_structs, links, 0, {1, 0});

  DumpScene(uber_structs, topology_data, {}, output);
  auto lines = GetLines(output);

  ExpectTopologyNodeHasLessDepthLevel({1, 0}, 0, {2, 0}, 1, lines);
  ExpectTopologyNodeHasLessDepthLevel({2, 0}, 1, {3, 0}, 2, lines);
  ExpectTopologyNodeHasLessDepthLevel({3, 0}, 2, {4, 0}, 3, lines);
  ExpectTopologyNodeHasLessDepthLevel({4, 0}, 3, {5, 0}, 4, lines);
  ExpectTopologyNodeHasSameDepthLevel({2, 0}, 1, {6, 0}, 5, lines);

  ExpectInstanceDumpCount(lines, 6);
  ExpectInstanceDump(1, "", lines);
  ExpectInstanceDump(2, "", lines);
  ExpectInstanceDump(3, "", lines);
  ExpectInstanceDump(4, "", lines);
  ExpectInstanceDump(5, "", lines);
  ExpectInstanceDump(6, "", lines);
}

TEST(SceneDumperTest, TopologyTreeWithNames) {
  UberStruct::InstanceMap uber_structs;
  GlobalTopologyData::LinkTopologyMap links;

  const TransformGraph::TopologyVector vectors[] = {
      {{{1, 0}, 2}, {{0, 2}, 0}, {{0, 5}, 0}},               // 1:0 - 0:5
                                                             //    \
                                                             //     0:2
                                                             //
      {{{2, 1}, 1}, {{2, 0}, 2}, {{0, 3}, 0}, {{0, 4}, 0}},  // 2:1 - 2:0 - 0:3
                                                             //     \
                                                             //      0:4
      {{{3, 0}, 0}},                                         // 3:0
      {{{4, 0}, 0}},                                         // 4:0
      {{{5, 0}, 0}}                                          // 5:0
  };

  const std::string names[] = {"", "2_1_ABC", "3_0_DEF", "", "5_0_GHI"};
  std::vector<zx_koid_t> koids;

  for (int i = 0; i < 5; i++) {
    auto uber_struct = std::make_unique<UberStruct>();
    uber_struct->local_topology.assign(vectors[i].begin(), vectors[i].end());
    uber_struct->debug_name = names[i];
    auto view_ref_pair = scenic::cpp::ViewRefPair::New();
    uber_struct->view_ref = std::make_shared<const ViewRef>(std::move(view_ref_pair.view_ref));
    koids.push_back(uber_struct->view_ref->koid());
    EXPECT_NE(0u, koids.back());
    uber_structs[vectors[i][0].handle.GetInstanceId()] = std::move(uber_struct);
  }

  MakeLink(links, 2, 1);  // 0:2 - 2:1
  MakeLink(links, 3);     // 0:3 - 3:0
  MakeLink(links, 4);     // 0:4 - 4:0
  MakeLink(links, 5);     // 0:5 - 5:0

  std::stringstream output;

  auto topology_data =
      GlobalTopologyData::ComputeGlobalTopologyData(uber_structs, links, 0, {1, 0});

  DumpScene(uber_structs, topology_data, {}, output);
  auto lines = GetLines(output);

  // {1, 0} is the root with {2, 1} as a child node.
  ExpectTopologyNodeHasLessDepthLevel({1, 0}, 0, {2, 1}, 1, lines);
  // {2, 1} has one child node - {2, 0}.
  ExpectTopologyNodeHasLessDepthLevel({2, 1}, 1, {2, 0}, 2, lines);
  // {2, 0} has two children - {3, 0} and {4, 0}.
  ExpectTopologyNodeHasLessDepthLevel({2, 0}, 2, {3, 0}, 3, lines);
  ExpectTopologyNodeHasSameDepthLevel({3, 0}, 3, {4, 0}, 4, lines);
  // {5, 0} is direct child of {1, 0} and sibling of {2, 1}.
  ExpectTopologyNodeHasLessDepthLevel({1, 0}, 0, {5, 0}, 5, lines);
  ExpectTopologyNodeHasSameDepthLevel({2, 1}, 1, {5, 0}, 5, lines);

  ExpectNodeNameAndKoid({2, 1}, 1, names[1], koids[1], lines);
  ExpectNodeNameAndKoid({3, 0}, 3, names[2], koids[2], lines);
  ExpectNodeNameAndKoid({5, 0}, 5, names[4], koids[4], lines);

  ExpectInstanceDumpCount(lines, 5);
  ExpectInstanceDump(1, names[0], lines);
  ExpectInstanceDump(2, names[1], lines);
  ExpectInstanceDump(3, names[2], lines);
  ExpectInstanceDump(4, names[3], lines);
  ExpectInstanceDump(5, names[4], lines);
}

TEST(SceneDumperTest, ImageMetadata) {
  UberStruct::InstanceMap uber_structs;
  GlobalTopologyData::LinkTopologyMap links;

  const TransformGraph::TopologyVector vectors[] = {
      {{{1, 0}, 2}, {{0, 2}, 0}, {{0, 3}, 0}},  // 1:0 - 0:3
                                                //    \
                                                //     0:2
                                                //
      {{{2, 0}, 0}},                            // 2:0
      {{{3, 0}, 0}},                            // 3:0
  };

  {
    auto& v = vectors[0];
    auto uber_struct = std::make_unique<UberStruct>();
    uber_struct->local_topology.assign(v.begin(), v.end());
    uber_structs[v[0].handle.GetInstanceId()] = std::move(uber_struct);
  }
  {
    auto& v = vectors[1];
    auto uber_struct = std::make_unique<UberStruct>();
    uber_struct->local_topology.assign(v.begin(), v.end());
    uber_structs[v[0].handle.GetInstanceId()] = std::move(uber_struct);
  }
  {
    auto& v = vectors[2];
    auto uber_struct = std::make_unique<UberStruct>();
    uber_struct->local_topology.assign(v.begin(), v.end());
    uber_structs[v[0].handle.GetInstanceId()] = std::move(uber_struct);
  }

  MakeLink(links, 2);  // 0:2 - 2:0
  MakeLink(links, 3);  // 0:3 - 3:0

  std::stringstream output;

  auto topology_data =
      GlobalTopologyData::ComputeGlobalTopologyData(uber_structs, links, 0, {1, 0});

  ImageMetadata image1;
  image1.collection_id = 1;
  image1.width = 800;
  image1.height = 600;
  image1.identifier = display::ImageId(1);

  ImageMetadata image2;
  image2.collection_id = 1;
  image2.width = 300;
  image2.height = 400;
  image2.identifier = display::ImageId(2);
  std::vector<SrcToDest> geometries;
  geometries.push_back(
      SrcToDest(types::RectangleF({.x = 50, .y = 60, .width = 200, .height = 300})));
  geometries.push_back(
      SrcToDest(types::RectangleF({.x = 90, .y = 100, .width = 400, .height = 500})));

  std::vector<ResolvedLayer> resolved_layers;
  resolved_layers.push_back(ResolvedLayer{
      .geometry = geometries[0],
      .multiply_color = {1.f, 1.f, 1.f, 1.f},
      .blend_mode = BlendMode::kReplace(),
      .content =
          ResolvedLayer::ImageContent{
              .image_id = image1.identifier,
              .width = image1.width,
              .height = image1.height,
          },
      .topology_index = 1,
  });
  resolved_layers.push_back(ResolvedLayer{
      .geometry = geometries[1],
      .multiply_color = {.2f, .4f, .8f, 1.f},
      .blend_mode = BlendMode::kReplace(),
      .content =
          ResolvedLayer::ImageContent{
              .image_id = image2.identifier,
              .width = image2.width,
              .height = image2.height,
          },
      .topology_index = 2,
  });

  DumpScene(uber_structs, topology_data, resolved_layers, output);
  auto lines = GetLines(output);

  // {1, 0} is the root with two child transforms {2, 0} and {3, 0}.
  ExpectTopologyNodeHasLessDepthLevel({1, 0}, 0, {2, 0}, 1, lines);
  ExpectTopologyNodeHasLessDepthLevel({1, 0}, 0, {3, 0}, 2, lines);
  ExpectTopologyNodeHasSameDepthLevel({2, 0}, 1, {3, 0}, 2, lines);

  ExpectInstanceDumpCount(lines, 3);
  ExpectInstanceDump(1, "", lines);
  ExpectInstanceDump(2, "", lines);
  ExpectInstanceDump(3, "", lines);

  ExpectImageDumpCount(lines, 2);
  // First image dump.
  const auto& node = vectors[1][0].handle;
  size_t next_image_dump_line_number = ExpectImageDump(image1, node, geometries[0], lines);
  // Second image dump.
  const auto& second_node = vectors[2][0].handle;
  next_image_dump_line_number =
      ExpectImageDump(image2, second_node, geometries[1], lines, next_image_dump_line_number);
  EXPECT_EQ(FindImageDumpLineNumber(lines, next_image_dump_line_number), (size_t)-1);
}

TEST(SceneDumperTest, DumpsSolidColorLayer) {
  UberStruct::InstanceMap uber_structs;
  GlobalTopologyData::LinkTopologyMap links;

  const TransformGraph::TopologyVector vectors[] = {
      {{{1, 0}, 1}, {{0, 2}, 0}},  // 1:0 - 0:2
      {{{2, 0}, 0}},               // 2:0
  };

  {
    auto& v = vectors[0];
    auto uber_struct = std::make_unique<UberStruct>();
    uber_struct->local_topology.assign(v.begin(), v.end());
    uber_structs[v[0].handle.GetInstanceId()] = std::move(uber_struct);
  }
  {
    auto& v = vectors[1];
    auto uber_struct = std::make_unique<UberStruct>();
    uber_struct->local_topology.assign(v.begin(), v.end());
    uber_structs[v[0].handle.GetInstanceId()] = std::move(uber_struct);
  }

  MakeLink(links, 2);  // 0:2 - 2:0

  std::stringstream output;

  auto topology_data =
      GlobalTopologyData::ComputeGlobalTopologyData(uber_structs, links, 0, {1, 0});

  std::vector<ResolvedLayer> resolved_layers;
  resolved_layers.push_back(ResolvedLayer{
      .geometry = SrcToDest(types::RectangleF({.x = 50, .y = 60, .width = 200, .height = 300})),
      .content = ResolvedLayer::SolidColorContent{.color = {.2f, .4f, .8f, 1.f}},
      .topology_index = 1,
  });

  DumpScene(uber_structs, topology_data, resolved_layers, output);
  auto lines = GetLines(output);

  // Expected output contains the solid color details.
  bool found_solid_color_line = false;
  for (size_t i = 0; i < lines.size(); ++i) {
    if (cpp23::contains(lines[i], "solid color: (0.2,0.4,0.8,1)")) {
      found_solid_color_line = true;
      EXPECT_TRUE(cpp23::contains(lines[i + 1], "transform: (2:0)"));
      EXPECT_TRUE(cpp23::contains(
          lines[i + 2],
          "geometry: SrcToDest[src:{x=0, y=0, width=0, height=0} dest:{x=50, y=60, width=200, height=300}"));
      break;
    }
  }
  EXPECT_TRUE(found_solid_color_line);
}

}  // namespace flatland::test
