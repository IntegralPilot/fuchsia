## Drivers

This repository contains drivers for wireless devices.

### Full MAC Drivers
A Full MAC driver relies on the firmware in the wireless hardware to implement
the majority of the IEEE 802.11 MLME functions. The [wlanif](./wlanif) driver
provides the platform-side implementation for Full MAC devices.

### Soft MAC Drivers
A Soft MAC driver implements the basic building blocks of communication with the
wireless hardware. The [wlansoftmac](./wlansoftmac) driver is a
hardware-independent layer that provides state machines for synchronization,
authentication, association, and other wireless networking state by leveraging
the WLAN MLME library. It communicates with a vendor-specific Soft MAC driver
to manage the hardware.

### PHY Drivers
The [wlanphy](./wlanphy) driver manages the WLAN PHY devices and provides an
interface for creating and destroying WLAN interfaces.
