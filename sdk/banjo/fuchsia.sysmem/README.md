# `fuchsia.sysmem` Banjo library

# TODO(b/306258166): Delete this library when clients have moved away from using
# these old sysmem(1) banjo types.

This library is based on [FIDL library](/sdk/fidl/fuchsia.sysmem). Copied files
were edited to remove constant declarations so that they don't conflict with
their FIDL counterparts in contexts where both Banjo and FIDL bindings are used.
Protocols are removed leaving only types.

All these types are deprecated. Client code still using these old types can
define equivalent local types instead (essentially just copying the banjo
codegen types locally as a starting point), or migrate to FIDL types in [FIDL
library](/sdk/fidl/fuchsia.sysmem2) or [FIDL
library](/sdk/fidl/fuchsia.images2).

The most authoritative/automated translation to sysmem2 and images2 types (for
reference at least) is what you get when you treat these types as FIDL
/sdk/fidl/fuchsia.sysmem "wire" types (bit for bit), then use sysmem-version.h
to convert from the sysmem(1) types to sysmem2 / images2 types.

Ultimately the present library will disappear when no more clients are using
these banjo types.
