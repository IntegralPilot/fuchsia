# Devicetree Data

This directory contains devicetree source (.dts) and binary (.dtb) files for use
in devicetree tests. Every 'dtb' file is obtained from a device, and the respective
'.dts' file is decompiled from the binary format.

It is important that 'dtb's in this directory reflect the contents of real or virtual
hardware. This blobs may be constructed by bootloaders, may be broken in different
ways and recompiling them from the decompiled 'dts' is not guaranteed to provide
bit-for-bit equality.

When adding a new binary file, it shall be named after the board and include any meaningul
information, such as 'qemu-gic3'. Added files should be decompiled using the devicetree
compiler application, and submitted together with the corresponding binary file.

# Devicetree Synthetic Data

The 'synthetic' directory contains devicetree source (.dts) for use in devicetree tests.
Unlike the data contained in this directory, the synthetic 'dtb' are compiled from
handwritten 'dts' files.
