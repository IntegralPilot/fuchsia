# Virtcon

Virtcon is the system terminal. It is a critical part of bringup and
provides graphical output with minimal hardware requrements.

## Design

Virtcon is written in Rust and powered by Carnelian. Carnelian enables
advanced vector graphics and truetype text rendering while maintaining
minimal hardware requirements.

## Goals

* Minimal resource usage.
* Maximize code reuse with Terminal app.
* Runtime product configuration.
* Flicker free single framebuffer mode.

## Usage

### System reboot

Devices with keyboards may issue a system reboot request by pressing the
Ctrl + Alt + Delete key combination.

Alternatively, devices with a power button may reboot the system by pressing
the power button repeatedly. To trigger a reboot, the power button must be
pressed 3 times in a row, with no more than 2 seconds between presses.

The type of reboot depends on the duration of the third button press:

* To reboot the system, the third press must be a short press (1 second or
  less).

* To reboot to the bootloader, the third press must be a long press (more than
  1 second).

## Testing

Configure

    fx set core.x64 --with //src/bringup/bin/virtcon:tests

Then test

    fx test virtual_console_tests
