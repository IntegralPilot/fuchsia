# escher/impl/

This directory (mostly) contains fairily generic Vulkan middleware to ease working with:
- command buffers and pools
- descriptor sets and pools
- memory allocation
- etc.

This was originally intended to contain implementation details which wouldn't be visible to Escher
clients.  However, don't be surprised if you see external code using it.
