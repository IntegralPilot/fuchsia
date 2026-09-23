# Factoryfs

This directory contains the remnants of factoryfs, a simple, readonly filesystem format intended to
store data that might be written during manufacturing. The filesystem itself is gone
(https://fxbug.dev/42078358), but it still contains export_ffs, which is a library that can take a directory
from another filesystem and flatten it into a factoryfs image. The library is still here to both
document the original format used for the filesystem, and because there are still out-of-tree
dependencies on it for now.
