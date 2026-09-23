# Driver for the onomni FUSB302 USB PD (Power Delivery) controller

## Overall approach

This driver is currently targeted at serving the needs of the
[Khadas VIM3][vim3] supported by Fuchsia. The board uses a FUSB302BMPX as the
USB PD controller on the USB Type C port that typically powers the board, and
delegates the responsibility of driving the FUSB302B chip to the application
processor that runs Fuchsia.

This setup leads to the following constraints.

1. The driver must avoid getting a Hard Reset at all costs, because Hard Reset
   implies power cycling, which leads to a reboot.
2. The driver must avoid breaking the USB Type C connection at all costs,
   because that would imply losing VBUS power, which leads to a reboot.
3. It takes 5-10 seconds from the moment the VIM3 is powered on, which
   establishes the USB Type C connection, to the moment when this driver is
   brought up. We must attempt to re-negotiate a PD Contract at that point, in
   the hope that we can get more than the 15W (5V @ 3A) of power offered by the
   Type C connection's implicit contract.

We first use the FUSB302's automated power role detection, which figures out
whether we're a Source or a Sink (we're always a sink) and also figures out
which of the two CC pins (CC1, CC2) in the Type C receptable is connected to the
CC wire in the Type C cable. After power role detection completes, we configure
the FUSB302 for the situation we detected. In particular, we point the BMC PHY
to the correct CC pin.

Once we have a CC channel set up, we first wait for a relatively long time
(currently 10 seconds) for a Source_Capabilities announcement. If we get an
announcement, we can follow the message flow in the USB PD spec to get a PD
contract. Type C ports on laptops tend to send Source_Capabilities announcements
indefinitely, and this flow gets used there.

If we don't receive a Source_Capabilities, we try to issue a Soft Reset. When
we succeed, this restarts the message flow that gets us to a PD Contract. Most
power supplies only send Source_Capabilities announcements for the minimum time
mandated by the USB PD spec, which is 7.5 seconds (nCapsCount = 50 messages
every tTypeCSendSourceCap = 150 ms), and is not sufficient for us to get control
of the FUSB302 chip. Some power supplies implement Soft Reset, and we're able to
get a PD contract in those cases. Other power supplies ignore the Soft Reset, in
which case we remain with an implicit contract.

## Source map

This driver combines two modules that should (and eventually) live in separate
components: a USB PD (Power Delivery) implementation, and the code driving the
FUSB302 chip.

The driver code has the following high-level structure.

* `registers` has low-level descriptions for all the registers that make up the
  I2C API between the chip and the driver; the descriptions include
  documentation that represents our interpretation of the datasheet, and what we
  learned from experiments while bringing up the driver
* `fusb302-sensors` covers registers that can be modeled as sensors, such as
  voltage meters; these always have a well-defined value
* `fusb302-signals` covers registers that can be modeled as signals, which
  indicate that specific events have occurred
* `fusb302-controls` covers registers that configure the chip's functional
  units, such as the switches that connect CC pins to various electrical blocks
* `fusb302-fifos` covers the FIFOs used by the chip's BMC PHY
* `fusb302-protocol` is built on top of `fusb302-fifos` and drives the chip's
  logic to present a full PD protocol layer implementation
* `fusb302-identity` reports the chip version via Inspect and logs
* `fusb302` has "controller" logic that glues all the pieces above with the USB
  PD driver

The USB PD implementation has the following high-level structure.

* `usb-pd-defs` defines geeneral concepts in the USB Type C and USB PD specs
* `usb-pd-message` implements the USB PD message structure
* `usb-pd-message-type` lists out PD message types; it is separated from
  `usb-pd-message` because the lists are rather bulky, and risk distracting the
  reader
* `usb-pd-message-objects` implements the data object formats in the PD specs;
  currently, this is the minimum set of objects needed to establish a PD
  Contract and avoid getting a Hard Reset
* `state-machine-base` extracts functionality common to the implementation of
  all state machines described by the USB PD spec
* `typec-port-state-machine` implements the state machine that maintains the USB
  Type C Port State; it's mostly responsible for resetting the PD Policy Engine
  state machine when the Type C Port connection is established
* `pd-sink-state-machine` implements the PD Policy Engine state machine; it's
  responsible for establishing and maintaining a PD Contract

## Testing Tips

The biggest hurdle in testing this driver is that bugs tend to lead to Hard
Reset. When the VIM3 is powered from the USB-C port, Hard Resets cause power
cycling, which prevents the driver's logs from being transmitted to the
development computer.

This hurdle can be mitigated using the following:

* Power the VIM3 through the VIN port.
    * Ensure that your revision of the VIM3 board supports using the VIN and
      USB-C ports at the same time. [Revision 13][vim3-v13-update] and
      [revision 14][vim3-v14-update] support concurrent use. All board revisions
      below 13 deviate from the USB-C electrical specification when the VIN port
      is connected, and will likely damage USB-C equipment.
    * Build a charging circuit that plugs into the VIN port. An expedient
      approach entails [splicing a VIN-to-VIN cable][vim3-vin-splicing].
* Connect to the VIM3 using both serial (usually via a Serial TTL to USB
  cable) and Ethernet cables; logs over serial come earlier, but Ethernet tends
  to carry more messages when the VIM3 loses power due to Hard Resets
* When feasible, artificially delay the PD negotiation step that's failing, so
  the backlog of boot-time logs on the serial connection has time to drain.
* A reliable USB PD sniffer is the only source of clues when the above fail.
  We recommend
  [the CY4500 from Cypress / Infineon Technologies][usb-pd-sniffer-cy4500], or
  any high-quality implementation of the
  [the "Twinkie" design][usb-pd-sniffer-twinkie] produced by the Chromium OS
  project. Both recommendations include hardware that reliably captures CC
  traffic, and software that correctly implements USB PD message decoding.

## References

The code contains references to the following documents.

* [the FUSB302B datasheet][datasheet] - Revision 5, August 2021, publication
  order number FUSB302B/D - referenced as "Rev 5 datasheet"
* [the USB Power Delivery Specification][usb-pd-spec] - Revision 3.1,
  Version 1.7, January 2023 - referenced as `usbpd3.1`
* [the USB Type C Cable and Connector Specification][usb-type-c-spec] -
  Release 2.2, October 2022 - referenced as `typec2.2`
* [the Universal Serial Bus 3.2 Specification][usb32-spec] - Revision 1.1, June
  2022 - referenced as `usb3.2`

[datasheet]: https://www.onsemi.com/pdf/datasheet/fusb302b-d.pdf
[usb-pd-spec]: https://usb.org/document-library/usb-power-delivery
[usb-type-c-spec]: https://usb.org/document-library/usb-type-cr-cable-and-connector-specification-release-22
[usb32-spec]: https://usb.org/document-library/usb-32-revision-11-june-2022
[vim3]: https://docs.khadas.com/products/sbc/vim3/hardware/start
[vim3-v13-update]: https://www.khadas.com/post/vim3-v13-whats-new
[vim3-v14-update]: https://www.khadas.com/post/vim3-v14-what-s-new
[vim3-vin-splicing]: https://forum.khadas.com/t/powering-the-vim3/5202/23
[usb-pd-sniffer-cy4500]: https://www.infineon.com/cms/en/product/evaluation-boards/cy4500/
[usb-pd-sniffer-twinkie]: https://www.chromium.org/chromium-os/twinkie/
