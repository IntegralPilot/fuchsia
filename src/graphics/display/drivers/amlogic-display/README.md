# Display driver for AMLogic display engines

## Target hardware

Changes to the driver should be reviewed against the documentation for the
following hardware.

* AMLogic A311D (G12B) - on Khadas VIM3
* AMLogic S905D3 (SM1) - on Nelson
* AMLogic T931 (G12B) - on Sherlock
* AMLogic S905D2 (G12B) - on Astro

Datasheets for other models may be used to correct gaps and errors in the
datasheets for the target models. All information obtained in this manner must
be confirmed experimentally, especially when the datasheet we use applies to a
different design generation. For example, the AMLogic S912 datasheet fills a few
gaps, but the S912 chip uses the GXM design.

## Hardware model

AMLogic's documentation makes heavy use of acronyms. This section goes over the
acronyms for the top-level modules, and briefly describes their functionality.

The entire display engine is generally called the VPU (Video Processing Unit).
It is also called the Video Output Unit in the high-level overview (A311D
datasheet Section 2.2 "Features").

The display engine is split into the VIU (Video Input Unit), which is the
engine's frontend, and the VOUT (Video Output), which is the engine's backend.

### VIU (Video Input Unit)

The display engine can have multiple VIU instances. The AMLogic A311D has two
VIUs (VIU1, VIU2), so it can drive two displays at once.

Each VIU has multiple input channels that retrieve pixel data (scanout) and feed
it into a VPP (Video Post-Processing) unit, which performs image processing such
as scaling, blending, and CSC (color space conversion, including color and gamma
correction). In other display engines, the input channels are called planes or
layers, and the entire VIU is called a pipe.

The VIU has two types of input channels, listed below.

* OSD (On-Screen Display) channels produce RGB data
* VD (Video Display) channels produce YUV data

The VD channels support Chroma-downsampled planar formats, such as YUV 4:2:0.
The channels do Chroma upsampling before feeding the data to VPP.

Each VIU has multiple VD channels and OSD channels. The VIUs in AMLogic A311D
have two VD channels (VD1, VD2) and four OSD channels (OSD1, OSD2, OSD3, OSD4).

#### Rationalization for the naming scheme

This subsection is speculation, as there is no documentation explaining the
reasoning behind names. With that being said, a plausible explanation for the
input channel naming scheme points to the STB (TV set-top box) use case, which
is prominently mentioned in the AMLogic datasheets and quick start manuals.

The typical setup uses a VD channel to display the TV video, sourced from a
video decoder that produces YUV data. The second VD plane may be used for
picture-in-picture or to display two TV channels side-by-side.

The OSD planes display "control panels", which are rendered by software running
on the AMLogic processor. RGB is the preferred pixel format for rendering.

### VOUT (Video Output)

All documented display engines have a single VOUT instance.

Conceptually, the VOUT consists of a VENC (Video Encoder) stage that converts
processed image data into a video signal, and an analog front-end stage that
contains PHYs (physical layer) for transmitting the video signal via various
connectors.

The VOUT hosts a few encoder blocks. Each encoder is unique in terms of the
properties of its output signal. Each VIN can be connected to an encoder. There
is very little flexibility in connecting encoders to PHYs, because display
connectors require specific signals.

#### VENC (Video Encoders)

The VENC stage has the encoders below.

* ENCI (Interlaced signal encoder) - designed to produce 480i (compatible with
  NTSC, which has 483 visible lines per frame) and 576i (compatible with PAL,
  which has 576 visible lines per frame) signals
* ENCP (Progressive signal encoder) - designed for progressive encoding, but can
  also produce an 1080i signal
* ENCL (LCD panel encoder) - designed for DSI signals
* ENCT (TV panel encoder) - not documented

The ENCI and ENCP encoders output the pixel color signal and timing signals,
including Display Enabled (DE), Vertical Sync (VSYNC) and Horizontal Sync
(HSYNC). These signals are compatible with the HDMI / DVI transmitter.

#### Analog front-end

The analog front-end stage contains PHY (physical layer) transmitters for the
following display connectors.

* CVBS - outputs the ENCI signal via a VDAC (Video DAC)
* HDMI - can receive the signals from ENCI or ENCP
* MIPI DSI - receives the MIPI-DPI signal from ENCL

The HDMI block embeds a Synopsis DWC (DesignWare Core) HDMI Transmitter
Controller IP, which transcodes the encoder output to TMDS signals and sends
them to the HDMI PHY.

The AMLogic documentation refers to the DesignWare IP as "HDMI TX Controller
IP", and to the integration glue as "HDMITX Top-Level" (HDMI_TOP and
HDMITX_TOP). Driving the HDMI block entails configuring registers in both the
AMLogic TOP and in the DesignWare IP.

The MIPI DSI block follows a similar structure.

The analog frontend conceptually belongs to the HHI (undocumented acronym),
which hosts a variety of analog-digital circuits including PHYs, power gates,
and PLLs (Phased Lock Loops) used by the clock tree.

### The canvas table

The VPU accesses DRAM using the on-chip DMC (DRAM Memory Controller). The VPU is
connected to the DMC via AMBus, a specialized bus optimized for burst transfers.
For example, on A311D, the VPU has 5 dedicated AMBus channels (3 for read
requests, 2 for write requests) to the DMC.

AMBus transactions to the DMC use the canvas table for address translation. Each
entry in the canvas table describes a canvas, which is a contiguous region in
DRAM designated for storing pixel data, associated with metadata describing a
specific pixel format. Each AMBus transaction executes in the context of a
canvas table entry, identified by a canvas index.

The canvas table is accessed using registers in the DMC's MMIO address space.
The table is likely stored in SRAM inside the DMC, because AMLogic datasheets
state that canvas translation is latency-free.

### RDMA (Register Direct Memory Access)

The RDMA (Register Direct Memory Access) engine accelerates flips (configuring
the VPU for displaying the next frame, after a VSync), which entail writing a
block of VPU registers. The RDMA engine out-performs a sequence of MMIO writes
because it has a direct access path to the VCBus (Video Controller Bus) that
the VPU registers are attached to.

## Mapping to Intel display engine concepts

For historical reasons, many Fuchsia developers are familiar with Intel's
display engines. The following mapping may give a head start to these
developers.

* OSD, VD (VIU input channels) - display plane streamers
* VIU - display pipe
* VPP - image processing logic (CSC, scaler, LUTs) in planes and pipes
* VENC, ENCI, ENCP, ENCL, ECNT - transcoders
* HHI - DDIs (digital display interfaces) including PLLs, PHYs, and power gates
* Canvas - surface (source of pixel data for a plane)
* Canvas table - GGTT (Global Graphics Translation Table)
* RDMA engine - DSB (Display State Buffer) engine

Just like "no model is entirely correct, but some models are useful", this
mapping is not perfect, as the display engines don't have identical structures.

## References

The code contains references to the following documents.

* [AMLogic A311D datasheet][a311d-datasheet] - revision 08, released 2021-10-18;
  distributed by Khadas for the VIM3, referenced as "A311D datasheet"
* [AMLogic A311D2 datasheet][a311d2-datasheet] - revision 0.6, released
  2021-11-30; distributed by Khadas for the VIM4, referenced as "A311D2
  datasheet"
* [AMLogic S905D3 datasheet][s905d3-datasheet] - revision 0.2, released
  2019-05-14; distributed by Khadas for the VIM3L, referenced as "S905D3
  datasheet"
* [AMLogic S905Y4 datasheet][s905y4-datasheet] - revision 0.7, released
  2022-09-08, distributed by Khadas for the VIM1S, referenced as "S905Y4
  datasheet"
* [AMLogic S912 datasheet][s912-datasheet] - revision 0.1, released 3/14/2017;
  distributed by Khadas for the VIM2, referenced as "S912 datasheet"
* [Synopsis DesignWare Cores HDMI Transmitter Controller Databook][dw-hdmi-databook]
  - version 2.12a, dated April 2016; available from Synopsis
* [Synopsis DesignWare Cores MIPI DSI Host Controller Databook][dw-dsi-databook]
  - version 1.51a, dated May 2021; available from Synopsis
* [High-Definition Multimedia Interface (HDMI) Specification][hdmi1-spec]
  - version 1.4b, dated October 11 2011, referenced as "HDMI 1.4b spec"
* [HDMI Forum High-Definition Multimedia Interface (HDMI) Specification][hdmi2-spec]
  - version 2.1b, dated July 20 2023, referenced as "HDMI 2 spec"
* [MIPI Alliance Specification for Display Serial Interface 2 (DSI-2)][mipi-dsi2-spec] -
  Version 2.1, dated 21 December 2022, referenced as "DSI spec"
* [MIPI Alliance Specification for D-PHY][mipi-dphy-spec] - Version 3.5, dated
  29 March 2023, referenced as "D-PHY spec"
* [MIPI Alliance Specification for Display Command Set (MIPI DCS)][mipi-dcs-spec] -
  Version 2.0, dated 6 November 2022, referenced as "DCS spec"

### Panel references

The code also contains references to the following documents covering display
panels. These documents will likely move to different drivers in the future.

* BOE Technology TV070WSM-TG1 Product Specification - Revision P4,
  issued 2020.07.10, referenced as "BOE TV070WSM-TG1 spec"
* BOE Technology TV101WXM-AG0 Product Specification - Revision P0,
  issued 2018.02.28, referenced as "BOE TV101WXM-AG0 spec"
* Innolux Corp P070ACB-DB0 Specification - Version 07, dated 2018/01/04,
  referenced as "Innolux P070ACB-DB0 spec"
* Shenzhen K&D Technology KD070D82-39TI-A010 Specification - Version A1,
  dated 2019.12.20, referenced as "K&D KD070D82-39TI-A010 spec"
* [Microtech MTF050FHDI-03 Specification Sheet][ts050-panel-spec] - Version 1.0,
  dated July 7, 2015, distributed by icbanq.com

The code also contains references to the following documents covering DDICs
(Display Driver ICs). These documents will likely move to different drivers in
the future.

* Fitipower Jadard JD9364 Data Sheet - Version 1.07, dated 2021/2/26,
  referenced as "JD9364 datasheet"
* Fitipower Jadard JD9364 User Guide - Version 1.01, dated 2018/5/31,
  referenced as "JD9364 user guide"
* Fitipower Jadard JD9364 IP User Guide - Version 0.03, dated 2018/4/2,
  referenced as "JD9364 IP user guide"
* [Fitipower Jadard JD9365D Data Sheet][jd9365-datasheet] - Version 0.01,
  dated 2017/4/27, distributed by PINE64, referenced as "JD9365 datasheet"
* Fitipower Jadard JD9365 User Guide - Version 0.11, dated 2015/10/28,
  referenced as "JD9365 user guide"
* [Novatek NT35596 Data Sheet][nt35596-datasheet] - Version 0.05,
  dated 2012/06/22, distributed by Khadas for TS-050, referenced as
  "NT35596 datasheet"

[a311d-datasheet]: https://dl.khadas.com/products/vim3/datasheet/a311d_datasheet_08_wesion.pdf
[a311d2-datasheet]: https://dl.khadas.com/products/vim4/datasheet/amlogic_a311d2_datasheet_v06.pdf
[s905d3-datasheet]: https://dl.khadas.com/products/vim3l/datasheet/s905d3_datasheet_0.2_wesion.pdf
[s905y4-datasheet]: https://dl.khadas.com/products/vim1s/datasheet/amlogic_s905y4_datasheet_v0.7.pdf
[s912-datasheet]: https://dl.khadas.com/products/vim2/datasheet/s912_datasheet_v0.220170314publicversion-wesion.pdf
[dw-hmdi-databook]: https://www.synopsys.com/dw/doc.php/iip/DWC_hdmi_tx/2.12a/doc/DWC_hdmi_tx_databook.pdf
[dw-dsi-databook]: https://www.synopsys.com/dw/doc.php/iip/DWC_mipi_dsi_host/1.51a/doc/DWC_mipi_dsi_host_databook.pdf
[hdmi1-spec]: https://www.hdmi.org/spec/hdmi1_4b
[hdmi2-spec]: https://www.hdmi.org/spec/hdmi2_1
[jd9365-datasheet]: https://files.pine64.org/doc/datasheet/pinephone/JD9365D_DS_Preliminary_V0.01_20170427.pdf
[mipi-dsi2-spec]: https://www.mipi.org/specifications/dsi-2
[mipi-dphy-spec]: https://www.mipi.org/specifications/d-phy
[mipi-dcs-spec]: https://www.mipi.org/specifications/display-command-set
[nt35596-datasheet]: https://dl.khadas.com/products/add-ons/ts050/nt35596_datasheet_v0.0511.pdf
[ts050-panel-spec]: https://www.icbanq.com/icdownload/data/ICBShop/Board/MTF050FHDI-03-spec(350%20nits).pdf
