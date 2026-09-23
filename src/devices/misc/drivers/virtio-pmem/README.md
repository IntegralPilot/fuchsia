# virtio-pmem

https://docs.oasis-open.org/virtio/virtio/v1.3/csd01/virtio-v1.3-csd01.html#x1-68900019

Quoting the specification:

The virtio pmem device is a persistent memory (NVDIMM) device that provides a
virtio based asynchronous flush mechanism. This avoids the need for a separate
page cache in the guest and keeps the page cache only in the host. Under memory
pressure, the host makes use of efficient memory reclaim decisions for page
cache pages of all the guests. This helps to reduce the memory footprint and
fits more guests in the host system.

The virtio pmem device provides access to byte-addressable persistent memory.
The persistent memory is a directly accessible range of system memory. Data
written to this memory is made persistent by separately sending a flush command.
Writes that have been flushed are preserved across device reset and power
failure.
