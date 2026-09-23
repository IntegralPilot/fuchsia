// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "dump-parsed.h"

#include <lib/driver/devicetree/manager/manager.h>
#include <lib/driver/devicetree/manager/publisher-host.h>
#include <lib/driver/devicetree/visitors/default/default.h>
#include <lib/driver/devicetree/visitors/load-visitors-host.h>
#include <lib/driver/devicetree/visitors/registration.h>
#include <lib/driver/devicetree/visitors/registry.h>
#include <lib/driver/logging/cpp/logger.h>

#include <format>
#include <fstream>
#include <iostream>
#include <map>
#include <string>
#include <vector>

#define ENABLE_DEBUG_LOG 0

namespace {
template <typename... Args>
void log_debug(std::format_string<Args...> fmt, Args&&... args) {
#if ENABLE_DEBUG_LOG
  std::cout << std::format(fmt, std::forward<Args>(args)...) << std::endl;
#endif
}
}  // namespace

/**
 * devicetree-dump-parsed is a host tool that processes a devicetree blob (DTB) and
 * outputs a human-readable representation of the Fuchsia driver framework
 * objects (Platform Bus nodes, Board Child nodes, Composite Node Specs) that
 * would be published by the devicetree manager + visitors.
 */
int main(int argc, char** argv) {
  if (argc < 3) {
    std::cerr << "Usage: " << argv[0] << " <dtb_path> <output_path> [--visitor <path_to_so>...]\n";
    return 1;
  }

  std::string dtb_path = argv[1];
  std::string output_path = argv[2];
  std::vector<std::string> visitor_paths;

  for (int i = 3; i < argc; ++i) {
    if (std::string(argv[i]) == "--visitor" && i + 1 < argc) {
      visitor_paths.push_back(argv[++i]);
    }
  }

  // Initialize a global logger for host-side logging (required for FDF_LOG).
  fdf::Logger logger("devicetree-dump-parsed", FUCHSIA_LOG_ERROR);
  fdf::Logger::SetGlobalInstance(&logger);

  // 1. Load the DTB from the specified path.
  log_debug("Loading DTB from: {}", dtb_path);
  std::vector<uint8_t> dtb = fdf_devicetree::LoadBlob(dtb_path);
  fdf_devicetree::Manager manager(std::move(dtb));

  // 2. Register visitors. These visitors parse specific properties in the DTB
  // and convert them into Fuchsia driver platform bus or composite node metadata.
  auto visitors = std::make_unique<fdf_devicetree::VisitorRegistry>();

  // Default visitors handle common properties like mmio, irq, etc.
  log_debug("Loading default visitors...");
  auto status = visitors->RegisterVisitor(std::make_unique<fdf_devicetree::DefaultVisitors<>>());
  if (status.is_error()) {
    std::cerr << "Failed to register default visitors\n";
    return 1;
  }

  // Load additional visitors from shared libraries.
  for (const auto& path : visitor_paths) {
    log_debug("Loading visitor from: {}", path);
  }
  status = fdf_devicetree::LoadVisitorsHost(*visitors, visitor_paths);
  if (status.is_error()) {
    std::cerr << "Failed to load dynamic visitors\n";
    return 1;
  }

  // 3. Walk the devicetree using the registered visitors.
  status = manager.Walk(*visitors);
  if (status.is_error()) {
    std::cerr << "Failed to walk devicetree: " << status.status_value() << "\n";
    return 1;
  }
  log_debug("Devicetree walk completed.");

  // 4. Publish the results to a host-side publisher.
  // Instead of talking to the actual platform bus, PublisherHost stores the
  // nodes in memory so we can inspect and stringify them.
  fdf_devicetree::PublisherHost publisher;
  status = manager.PublishDevices(publisher);
  if (status.is_error()) {
    std::cerr << "Failed to publish devices: " << status.status_value() << "\n";
    return 1;
  }
  log_debug("All devices are published.");

  // 5. Open the output file for writing the human-readable format.
  log_debug("Creating the dt.parsed file at: {}", output_path);
  std::ofstream out(output_path);
  if (!out.is_open()) {
    std::cerr << "Failed to open output file: " << output_path << "\n";
    return 1;
  }

  // 6. Stringify Platform Bus Nodes (including metadata).
  out << "platform_bus_nodes {\n";
  auto& pbus_nodes = publisher.GetPbusNodesWithMetadata();
  for (const auto& entry : pbus_nodes) {
    fdf_devicetree::StringifyPbusNode(entry.node, entry.metadata_text, entry.power_config_text,
                                      out);
    out << "\n";
  }
  out << "}\n\n";

  // 7. Stringify Board Child Nodes.
  out << "board_child_nodes {\n";
  auto& board_child_nodes = publisher.GetBoardChildNodes();
  for (const auto& node : board_child_nodes) {
    fdf_devicetree::StringifyBoardChildNode(node, out);
    out << "\n";
  }
  out << "}\n\n";

  // 8. Stringify Composite Node Specs.
  out << "composite_node_specs {\n";
  auto& composite_node_specs = publisher.GetCompositeNodeSpecInfos();
  for (const auto& spec : composite_node_specs) {
    fdf_devicetree::StringifyCompositeNodeSpec(spec, out);
    out << "\n";
  }
  out << "}\n\n";

  // 9. Stringify IOMMUs.
  out << "iommus {\n";
  auto& iommus = publisher.GetIommus();
  std::map<uint32_t, fuchsia_hardware_platform_bus::Iommu> sorted_iommus(iommus.begin(),
                                                                         iommus.end());
  for (const auto& [id, iommu] : sorted_iommus) {
    out << "  iommu {\n";
    out << "    id = " << id << "\n";
    out << "    " << fdf_devicetree::StringifyFidl(iommu) << "\n";
    out << "  }\n";
  }
  out << "}\n";

  out.close();

  log_debug("Devicetree parsed file has been updated: {}", output_path);

  return 0;
}
