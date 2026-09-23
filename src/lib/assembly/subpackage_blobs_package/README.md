# Base Package

This Rust crate constructs the Subpackage Blobs Package, which is a Fuchsia
package containing all the subpackage blobs indirectly referenced by the update
package. This allows old systems that do not understand subpackages OTA to a
version of Fuchsia that uses subpackages.
