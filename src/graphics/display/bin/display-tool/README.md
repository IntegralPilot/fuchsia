# display-tool - Display Driver Testing & Verification Tool

Command-line testing utility for Fuchsia display drivers.

## Overview & Architecture

The tool's design prioritizes minimizing the System Under Test (SUT) size.

Key decisions:

* Use the `fuchsia.hardware.display/Coordinator` FIDL interface directly.
* Use the testing client priority, so the tool takes over the display from any
  active product session.

## Build configuration

`display-tool` is included in the `tools` bundle for the display drivers stack.

```posix-terminal
fx add-test //src/graphics/display:tools
```

If you're iterating on the tool, include the display drivers' `tests` bundle.

```posix-terminal
fx add-test //src/graphics/display:tests
```

Use `balanced` or `release` builds for frame rate testing. The current scene
rendering logic can bottleneck the CPU in `debug`.

## Running

Recommended method:

```posix-terminal
ffx target ssh -- display-tool <command> [options]
```

To terminate continuous animation or vsync monitoring commands, send `SIGINT`
(Ctrl+C).

---

## Intended use for display testing

### Step 1: Display detection and timings

Test that the display drivers detect connected panels and register the correct
display modes and pixel formats.

```posix-terminal
ffx target ssh -- display-tool info
```

#### Verification

Add the `--fidl` flag to inspect the raw FIDL structure.

Checklist:

* At least one display is reported.
* Reported resolution matches the panel or emulator specifications.
* Supported pixel formats include expected types.
* Vertical refresh rates match specifications.
  Example: `60000 mHz` (in millihertz) for 60 Hz.

### Step 2: One solid color fill layer

Test the driver's ability to handle solid color fill layers. This test does not
depend on Sysmem integration or IOMMU configuration.

```posix-terminal
ffx target ssh -- display-tool color --color ff0000
```

#### Verification

* The display is filled with the requested color. Example: `ff0000` is pure red.
* The terminal outputs live refresh rate statistics:
  `Display 1 config is applied, refresh rate 60.00 Hz (16.66667 ms)`
* Vsync events arrive at a steady rate.

### Step 3: One image layer

Test that the display driver integrates with Sysmem and manages its IOMMU
correctly.

```posix-terminal
ffx target ssh -- display-tool vsync --color 00ff00 --pixel-format bgra32
```

#### Verification

* The display is filled with the chosen color. Example: `00ff00` is pure green.
* The terminal shows a stable VSync frequency. No dropped event warnings.
* System logs do not include any Sysmem buffer negotiation constraint errors.

### Step 4: Swapchain with double-buffering

Test that the display driver submits a sequence of display configurations
correctly to the hardware.

```posix-terminal
ffx target ssh -- display-tool squares
```

#### Expected visuals

Four colored squares animate and bounce off the display borders against a black
background.

* **Orange square** (`#ff6400`): starts at top-left
* **Fuchsia square** (`#ff00ff`): starts at top-right
* **Green square** (`#64ff00`): starts at bottom-left
* **Blue square** (`#0064ff`): starts at bottom-right

#### Verification

* Smooth animation. No screen tearing, jitter, or display corruption.
* The terminal outputs continuous FPS updates matching the display refresh rate.
  Example: `Display 60.00 fps (16.66667 ms)`.

### Step 5: Display compositing (multiple layers)

Test the display engine driver's support for multi-layer hardware composition,
layer z-ordering, and hardware alpha blending modes.

```posix-terminal
ffx target ssh -- display-tool multilayer-squares
```

#### Expected visuals

* **Bottom layer**: Bouncing **fuchsia square** (`#ff00ff`), four times as large
  as the smaller squares described below. Fully opaque (alpha blending
  disabled).
* **Top layer**: Three bouncing squares. Premultiplied alpha blending.
    * **Orange square** (`#ff6400`): opaque
    * **Green square** (`#64ff00`): semi-transparent (alpha set to 150)
    * **Blue square** (`#0064ff`): opaque

#### Verification

* The semi-transparent green square blends correctly with the bottom layer.
* Opaque areas in the top layer properly occlude the bottom layer.
* The terminal outputs a steady frame rate.

### Step 6: Frame rate

Test that the display engine does not drop or skip committed frames.

```posix-terminal
ffx target ssh -- display-tool frame-rate-test
```

#### Expected visuals

10x6 rectangular grid. A single colored cell traverses every grid location
sequentially following a continuous Hamiltonian cycle.

Cell color:

* **First loop frame**: **Blue** (`#0000ff`).
* **Intermediate frames**: **White** (`#ffffff`).
* **Final frame of loop**: **Red** (`#ff0000`).

#### Verification

* The terminal outputs the selected display mode rate.
  Example:  `Expected frame rate: 60.000 fps`.
* The moving cell advances smoothly by exactly one position per frame. No
  skipped cells. No stuttering.
