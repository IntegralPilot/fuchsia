# Display hardware overview

Computer systems use [**display devices**][display-device] to interact with
human users's [sight][human-visual-perception]. The display device is connected
to the computer system via a **display connection**.

[**Graphics**][computer-graphics] hardware and software "figures out" what will
be displayed to the user, and is ultimately responsible for creating
**image data**, which is a low-level description of a two-dimensional
[image][image] that can be processed by a display device.

The **display engine** is the hardware inside the computer system that drives
the display connection. The display engine's primary responsibility is turning
the image data into the electrical signals that go across the display
connection. The display device at the other end of the display connection then
displays the image, by turning the electrical signals into light (photons) that
interacts with the human user's sight.

Under normal operation, this process of transmitting and displaying images is
repeated continuously at a high frequency. Thanks to
[flicker fusion][flicker-fusion], human eyes perceive a series of images
displayed in quick succession as continuous motion. The sequence of images is
called a "moving" image, the individual images are called [**frames**][frame],
and the frequency of this process is called [**the frame rate**][frame-rate].
The display connection is said to carry **[video][video] data**, a low-level
description of the sequence of frames.

The Fuchsia display stack only supports **video displays**, which are display
devices that can represent arbitrary images (within a digital approximation).
For a contrasting example, the display stack does not support segment displays.

## The display path

The display engine's design and operation is easiest to understand in the
context of the entire path from software that intends to display images to the
human user that sees the images. The following subsections cover each part of
the path, ordered by increasing distance from the user.

### Display panel

Image data is ultimately shown to the human user by converting it into light
that reaches the user's [eyes][human-vision-system]. In modern display hardware,
the photons are produced by a **panel** in the display device.

The panel is a rectangular grid (lattice) of cells called [**pixels**][pixel],
where each pixel is a collection of a few [**subpixels**][subpixel] that work
together to produce a desired color. So, ultimately, the image displayed to the
user is a [raster graphic][raster-graphic]. The panel's most important
characteristic is its [**resolution**][display-resolution].

It can be useful to think of a panel as a write-only
[DRAM (Dynamic Random Access Memory)][dram]. The panel's subpixels are similar
to the DRAM's 1-bit [memory cells][dram-memory-cell], in that they store
information, and are arranged in a rectangular grid. Both subpixels and DRAM
memory cells hold information for a short amount of time (on the order of
milliseconds), and must be "refreshed" with new values before that time runs
out. A panel subpixel stores the intensity of the light to be emitted, whereas a
DRAM memory cell stores one bit that decides
[the cell's behavior][dram-operation] during a read operation.

The subpixel refresh requirements are usually communicated indirectly, as the
panel's minimum frame rate. The implication is that the panel refreshes all its
subpixels in the process of displaying a new image. So, subpixels are guaranteed
to hold their information for the period between two frames, which is the
inverse of the minimum frame rate.

Panels differ in the materials that make up the pixels and their connections.
Understanding specific panel technologies is generally not necessary for working
on Fuchsia's Display stack. For curious readers, two popular panel technologies
at the time of this writing are [LCD (Liquid Crystal Display)][lcd-panel] and
[OLED (Organic Light-Emitting Diode)][oled-panel].

### DDIC (Display Driver Integrated Circuit)

The **DDIC** (Display Driver IC / Integrated Circuit) manages the display
device's side of the display connection. The DDIC decodes the electrical signals
received from the display connection, and converts the image information into a
form suitable for the display panel.

Most importantly, the DDIC is responsible for distributing the image data to
the panel's subpixels, in a way that meets the panel's refresh requirements.
The hardware that implements this functionality is sometimes called a TCON
(Timing Controller).

The DDIC sometimes supplies power to the panel, effectively integrating the
functionality of a [**PMIC** (Power Management Integrated Circuit)][pmic]. This
entails converting the voltage provided to the DDIC's power rails to the
voltages required by the panel's power rails, and driving the power rails to
meet the panel's power up and power down sequencing requirements.

### Display connection

The display connection carries image information, encoded as electrical signals,
from the display engine to the DDIC.

At a high level, driving a display connection can be split into the following
concerns.

- Detection - a method for the display engine to find out whether there is any
  DDIC on the other side of the display connection
- Identification - a method for the display engine to find out what display
  hardware (DDIC and panel) is on the other side of the display connection
- Configuration - setting up the initial operation parameters of the hardware
  (DDIC and panel) at the other side of the connection, and changing these
  operating parameters in response to user requests or changes in circumstances
- Video transmission - all the display connections supported by Fuchsia transmit
  video data using the [raster scan][raster-scan]{.external} pattern

In many modern use cases, such as mobile and embedded devices, the display
engine and DDIC are in the same [housing][device-housing], also known as
enclosure or "box". The display connection may be an internal cable such as a
[FPC (Flexible Printed Circuit)][fpc], or a collection of traces on a board. The
user cannot replace the display connection while the computer is powered on.
Many display connections cannot be replaced after the device leaves the factory.

In some use cases, such as desktop computers and external displays attached to
mobile devices, the display engine and DDIC are in separate enclosures. The
display engine may be in a computer or mobile device, while the DDIC is in a
monitor or TV. In these cases,
[the display connection is a cable with connectors](#display-connections-overview).
The user may replace the cable or the connected display hardware while the
computer is powered on.

### Display engine

The display engine manages the display connection from the computer's side, and
therefore serves as the bridge between the display connection and any image
producer on the computer. Image producers can be hardware components, such as
the GPU and the media decoder, or software that directly produces pixel data.

Most image producers store image data in the system's main memory (DRAM).  For
this reason, virtually all display engines are capable of retrieving image data
from DRAM.

Documentation from various vendors uses different acronyms to refer to the
display engine hardware. Some of these names are below.

- **DE** (Display Engine) - the term preferred by Fuchsia
- Display Controller (DC)
- Display Processing Unit (DPU)
- Video Processing Unit (VPU)

### GPU (Graphics Processing Unit)

Most use cases on modern systems are best met by having a [**GPU** (Graphics
Processing Unit)][gpu] compute the pixels that make up the images displayed to
the user. The GPU is in the same computer as the display engine, either on the
same [SoC (System on a Chip)][soc] or on the same [graphics
card][graphics-card].

The GPU typically stores the computed image data in system DRAM. When the GPU
and display engine are on the same graphics card, the GPU may store the computed
image data in the [**GDDR** (Graphics Double Data Rate)][gddr] memory on the
graphics card, and the display engine may read the data directly from GDDR. This
optimization eliminates the performance cost of accessing the system DRAM, for
both the GPU and the display engine.

While the GPU is usually colocated on the same silicon chip as the display
engine, it is conceptually a different piece of hardware, and it is managed by
separate driver software. Confusingly, the display driver and GPU driver are
sometimes packaged together in a "graphics driver".

### Media decoder

Some use cases, such as playing movies or video conferencing, are best met by
having dedicated **media decoder** hardware compute the pixels making up the
images displayed to the user. The media decoder may be an independent hardware
unit, or it may belong to a codec.

From a high-level system perspective, media decoders are similar to GPUs. The
media decoder hardware is usually on the same SoC or graphics card as the
display engine, and may use system DRAM or GDDR to store the image data. The
hardware also has its own driver, which may be bundled in a "graphics driver".

Some systems have a direct hardware path for passing pixel data between the
media decoder and the display engine. This optimization saves the cost of
storing image data in an intermediate memory, but requires that the media
decoder can consistently produce pixel data at the rate needed by the display
engine.

## Display engine building blocks

This section will be expanded in the future.

## Display connections {#display-connections-overview}

This section will be expanded in the future.

- [SPI (Serial Peripheral Interface)][spi]{.external} - for low-resolution displays,
  generally supported by MCUs
- [DPI (Display Pixel Interface)][dpi]{.external} - for low-resolution low-end displays;
  also called RGB in display specs
- [DSI (Display Serial Interface)][dsi]{.external} - for high-resolution mobile displays,
  supported by SoCs for wearables and phones
- [HDMI (High-Definition Multimedia Interface)][hdmi]{.external} - for TVs and monitors,
  supported by SoCs for dev boards,laptops, PCs
- [DP (DisplayPort)][display-port]{.external} - started out in computers; supported by
  phones because it’s the main standard for USB-C output
- [eDP (Embedded DisplayPort)][embedded-display-port]{.external} - DisplayPort variant for
  internal connectors, most suitable for laptop panels

[computer-graphics]: https://en.wikipedia.org/wiki/Computer_graphics
[device-housing]: https://en.wikipedia.org/wiki/Housing_(engineering)
[display-device]: https://en.wikipedia.org/wiki/Display_device
[display-port]: https://en.wikipedia.org/wiki/DisplayPort
[display-resolution]: https://en.wikipedia.org/wiki/Display_resolution
[dpi]: https://en.wikipedia.org/wiki/Display_pixel_interface
[dram]: https://en.wikipedia.org/wiki/Dynamic_random-access_memory
[dram-memory-cell]: https://en.wikipedia.org/wiki/Dynamic_random-access_memory#Memory_cell_design
[dram-operation]: https://en.wikipedia.org/wiki/Dynamic_random-access_memory#Principles_of_operation
[dsi]: https://en.wikipedia.org/wiki/Display_Serial_Interface
[embedded-display-port]: https://en.wikipedia.org/wiki/DisplayPort#eDP
[flicker-fusion]: https://en.wikipedia.org/wiki/Flicker_fusion_threshold
[fpc]: https://en.wikipedia.org/wiki/Flexible_electronics
[frame]: https://en.wikipedia.org/wiki/Film_frame
[frame-rate]: https://en.wikipedia.org/wiki/Frame_rate
[gddr]: https://en.wikipedia.org/wiki/GDDR_SDRAM
[graphics-card]: https://en.wikipedia.org/wiki/Graphics_card
[gpu]: https://en.wikipedia.org/wiki/Graphics_processing_unit
[hdmi]: https://en.wikipedia.org/wiki/HDMI
[human-visual-perception]: https://en.wikipedia.org/wiki/Visual_perception
[human-vision-system]: https://en.wikipedia.org/wiki/Visual_system
[image]: https://en.wikipedia.org/wiki/Image
[lcd-panel]: https://en.wikipedia.org/wiki/Liquid-crystal_display
[oled-panel]: https://en.wikipedia.org/wiki/OLED
[pixel]: https://en.wikipedia.org/wiki/Pixel
[pmic]: https://en.wikipedia.org/wiki/Power_management_integrated_circuit
[raster-graphic]: https://en.wikipedia.org/wiki/Raster_graphics
[raster-scan]: https://en.wikipedia.org/wiki/Raster_scan
[soc]: https://en.wikipedia.org/wiki/System_on_a_chip
[spi]: https://en.wikipedia.org/wiki/Serial_Peripheral_Interface
[subpixel]: https://en.wikipedia.org/wiki/Pixel#Subpixels
[video]: https://en.wikipedia.org/wiki/Video
