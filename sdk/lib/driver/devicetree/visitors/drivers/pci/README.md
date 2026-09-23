# Generic PCI host controller

The schema is documented here:
https://www.kernel.org/doc/Documentation/devicetree/bindings/pci/host-generic-pci.yaml

This device specifies bus addresses using 3 address cells. The high cell contains
flags and configuration data. The mid and low cells encode a 64 bit physical
address. The visitor parses the "ranges" field as a RangesPropertyElement
using the mid and low address cells of the bus address along with the parent
address and size. The high cell is decoded into a separate array.