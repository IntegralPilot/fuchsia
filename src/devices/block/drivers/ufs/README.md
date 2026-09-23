# Universal Flash Storage (UFS)
Universal Flash Storage (UFS) is a flash-based mobile storage device that provides high
performance, low power, and high reliability compared to eMMC. Unlike eMMC, which uses a parallel
bus, UFS uses a high-speed, multi-lane serial bus to improve performance. The serial interface of
UFS uses LVDS differential signaling, which enables high-speed signal transmission with high noise
immunity and low voltage.
UFS follows the UFS specification of the JEDEC standard, which is currently up to version 4.0.
Fuchsia's UFS driver is based on the UFS specification version 3.1.

## Specification
UFS: https://www.jedec.org/document_search?search_api_views_fulltext=jesd220f
UFSHCI: https://www.jedec.org/standards-documents/docs/jesd223e

## Layered Architecture
The layered architecture of UFS uses the M-PHY physical layer and the UniPro link layer, with
the UFS Transfer Protocol (UTP) transport layer on top. It also adopts a SCSI architecture model
and command protocol that supports multiple simultaneous commands and command queues on top of the
UTP transport layer. The layered architecture of UFS consists of three layers: UFS Application
Layer(UAP), UFS Transport Protocol Layer (UTP), and UFS InterConnect Layer (UIC). The UFS driver is
implemented according to this layered architecture.

### UFS Application Layer (UAP)
The UAP layer consists of the UFS Command Set Layer (UCS), Device Manager, and Task Manager.
- The UFS Command Set Layer processes SCSI commands.
- Device Manager is responsible for device settings such as data transfer and power mode control,
  and controls them through descriptor, attribute, and flag requests.
- Task Manager is responsible for processing the task management function for command queue control.

### UFS Transport Protocol Layer (UTP)
UTP layer uses UFS Protocol Information Unit (UPIU) for message delivery between UAP layer and UFS
Interconnect (UIC) layer. The UTP layer wraps information from the UAP layer in a UPIU and delivers
it to the UFS device.

### UFS InterConnect Layer (UIC)
The lowest layer, the UIC layer, handles the connection between the UFS host interface and the UFS
device. The UIC layer automatically detects and recovers I/O errors through the UniPro link layer,
and provides reliable data transfer through the M-PHY physical layer.

## TODO
* Support RPMB logical unit
* Power management - Dynamic clock scaling
* Interrupt aggregation
* Implement Ufs tools
* Support UFS 4.0 Spec (multi queue)
* Support multiple platform (exynos, snapdragon, etc.)
* Support multiple block size (4KB, 8KB, 16KB ...)

## How to test

### Ufs driver unit tests
* Build configuration for ufs tests
> $ fx set core.x64 --with //src/devices/block/drivers/ufs:tests

* Run ufs unit tests
> $ fx test ufs-unit-tests

### Test on real device
Currently, the only device that supports a PCI-based UFS host controller is the Samsung Galaxy Book
S(NT767XCL) with an Intel Lakefield CPU(i5-L16G7). The Galaxy Book S has 512GB of eUFS 3.0 storage.
Fuchsia has not been ported to the Galaxy Book S yet, so we are running Fuchsia on QEMU (which works
on the Galaxy Book S) as an alternative. We are using KVM I/O Passthrough to bind a real UFS device
(eUFS) to QEMU Fuchsia.

```
------------------------
| Fushsia + UFS driver |
|--------------|   :   |
|     QEMU     |   :   |
|--------------|   : ---- I/O Passthrough
|    Linux     |   :   |
|--------------|   :   |
| Galaxy Book S + eUFS |
------------------------
```

To use eUFS I/O passthrough, you need to proceed as follows

1. Install Ubuntu 22.04.2 on the Galaxy book S.
2. Enable VT-d in the BIOS.
3. Verify that VT-d is enabled with the command `dmesg | grep DMAR`.
4. Modify the /etc/default/grub file to enable iommu.
```
GRUB_CMDLINE_LINUX_DEFAULT="quiet splash intel_iommu=on iommu=pt"
```
5. Perform a grub update.
```
sudo update-grub
```

6. Check the Vendor id (Intel) and Device id (UFS controller).
```
$ cat /sys/bus/pci/devices/0000:00:12.5/vendor
0x8086
$ cat /sys/bus/pci/devices/0000:00:12.5/device
0x98fa
```

7. Check the address of ufshcd.
```
$ ls /sys/bus/pci/drivers/ufshcd
0000:00:12.5 bind module new_id remove_id uevent unbind
```

8. Remove the ufs attached to host and register it as a vfio device.
```
$ echo 0000:00:12.5 | sudo tee /sys/bus/pci/drivers/ufshcd/unbind
0000:00:12.5
$ echo 8086 98fa | sudo tee /sys/bus/pci/drivers/vfio-pci/new_id
8086 98fa
```

9. Use vfio to run qemu.
```
sudo fx qemu -N -k -m 4096 -- -device vfio-pci,host=00:12.5,x-no-kvm-intx=on
```
