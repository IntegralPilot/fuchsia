# process-resolver

This component implements the `fuchsia.process.Resolver` protocol. This protocol
is used by the `#!resolve` trampoline implemented in `fdio` that can launch a binary
from any Fuchsia package.

process-resolver uses the `fuchsia.pkg.PackageResolver` capability from its namespace
to resolve packages. Product configuration determines whether this capability is
provided by the base-resolver (so only base shell packages can be resolved) or the
pkg-resolver (so base, cache, or universe shell packages can be resolved).
