---
name: migrate-dma-dfv1-to-dfv2
description: Migrate DMA buffers from ddk::IoBuffer to dma-buffer in DFv2.
---

# Driver DMA Migration (DFv1 to DFv2)

## Dependencies

**GN**:
```gn
deps = [
  # Provides dma_buffer::CreateBufferFactory
  "//src/devices/lib/dma-buffer",
]
```

**Bazel**:
```bazel
deps = [
  # Provides dma_buffer::CreateBufferFactory
  "//src/devices/lib/dma-buffer",
]
```

## Implement Migration

### Update Headers
Remove the legacy header and include the new one from
[buffer.h](/src/devices/lib/dma-buffer/include/lib/dma-buffer/buffer.h):

```diff
-#include <lib/ddk/io-buffer.h>
+#include <lib/dma-buffer/buffer.h>
```

### Update Variable
For a contiguous buffer using
[dma_buffer::ContiguousBuffer](/src/devices/lib/dma-buffer/include/lib/dma-buffer/buffer.h):

```diff
- ddk::IoBuffer data_buffer;
+ std::unique_ptr<dma_buffer::ContiguousBuffer> data_buffer;
```

### Initialize the Buffer
In the initialization code (usually in `Start()`):

```diff
- status = data_buffer.Init(bti.get(), size, 0);
+ auto factory = dma_buffer::CreateBufferFactory();
+ status = factory->CreateContiguous(bti, size, 0, &data_buffer);
```

### Access Addresses
When accessing the virtual or physical addresses:

```diff
- void* virt = data_buffe_.virt();
- zx_paddr_t phys = data_buffer.phys();
+ void* virt = data_buffer->virt();
+ zx_paddr_t phys = data_buffer->phys();
```

## Common Pitfalls

* **Contiguous Buffer Allocation Size**: When using
  `dma_buffer::CreateContiguous` (or `zx_vmo_create_contiguous`), the requested
  size MUST be a multiple of the page size. If you pass a size that is not
  page-aligned, it will fail with `ZX_ERR_INVALID_ARGS` (-10). Ensure you round
  up the size to `zx_system_get_page_size()` before allocating.

## Further Reading

* [dma-buffer
  Header](/src/devices/lib/dma-buffer/include/lib/dma-buffer/buffer.h)
