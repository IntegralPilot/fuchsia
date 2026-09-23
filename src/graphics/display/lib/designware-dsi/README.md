# Synopsys DesignWare MIPI DSI Host Controller

## Target hardware

The Synopsys DesignWare MIPI DSI Host Controller IP block is used on the
following SoCs:

* AMLogic A311D (G12B) - on Khadas VIM3
* AMLogic S905D3 (SM1) - on Nelson
* AMLogic T931 (G12B) - on Sherlock
* AMLogic S905D2 (G12B) - on Astro

## Hardware Model

The DesignWare MIPI DSI Host Controller contains the following I/O interfaces:

* An extended MIPI DPI-2 (Display Pixel Interface) to read pixels in video mode.
* A MIPI DBI (Display Binary Interface) for command request and response
  transmission.
* A MIPI D-PHY interface that connects to a PHY transmitter to transmit output
  signals to a display peripheral.

The DSI Host Controller takes in requests / pixels from the DBI and DPI
interfaces, converts them into packets and sends the encoded packets over the
D-PHY.

## References

The code contains references to the following documents:

* [Synopsys DesignWare Cores MIPI DSI Host Controller Databook][dw-dsi-databook]
  - version 1.51a, dated May 2021; available from Synopsys. Referenced as
  "DSI Host Databook".
* Synopsys DesignWare Cores MIPI DSI Host Controller User Guide
  - version 1.31a, dated May 2015; available from Synopsys. Referenced as
  "DSI Host User Guide".
* [MIPI Alliance Specification for Display Serial Interface 2 (DSI-2)][mipi-dsi2-spec] -
  Version 2.1, dated 21 December 2022, referenced as "DSI spec"
* [MIPI Alliance Specification for D-PHY][mipi-dphy-spec] - Version 3.5, dated
  29 March 2023, referenced as "D-PHY spec"

[dw-dsi-databook]: https://www.synopsys.com/dw/doc.php/iip/DWC_mipi_dsi_host/1.51a/doc/DWC_mipi_dsi_host_databook.pdf
[mipi-dsi2-spec]: https://www.mipi.org/specifications/dsi-2
[mipi-dphy-spec]: https://www.mipi.org/specifications/d-phy
