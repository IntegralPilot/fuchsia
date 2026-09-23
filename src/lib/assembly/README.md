- [`assembled_system`](assembled_system/): The core orchestration logic that
  takes configurations and produces the assembled outputs.
# Product and Image Assembly Libraries

This directory contains the Rust libraries that implement Fuchsia's Product and
Image Assembly processes. These libraries are used by both in-tree build actions
(under Ninja/Bazel) and out-of-tree tools (such as `ffx assembly`).

For the assembly tool itself, see
[`src/developer/ffx/plugins/assembly`](../../developer/ffx/plugins/assembly/).

## Directory Structure

While there are many crates in this directory, some of the key ones include:

- [`config_schema`](config_schema/): Defines the JSON schemas for assembly input
  configuration (e.g., product, board, image assembly configs).
- [`platform_configuration`](platform_configuration/): Implements the
  configuration of platform-level areas (e.g., connectivity, diagnostics,
  security) based on the product configuration.
- [`product_bundle`](product_bundle/): Logic for creating and working with
  Product Bundles.
- [`update_package`](update_package/): Libraries for constructing the Fuchsia
  Update Package.

## References

- [Software Assembly Concepts](../../../docs/concepts/software_assembly/overview.md)
- [RFC-0072: Standalone Image Assembly](../../../docs/contribute/governance/rfcs/0072_standalone_image_assembly_tool.md)
- [RFC-0118: Software Delivery Policy at Image Assembly](../../../docs/contribute/governance/rfcs/0118_swd_policy_at_image_assembly_rfc.md)
