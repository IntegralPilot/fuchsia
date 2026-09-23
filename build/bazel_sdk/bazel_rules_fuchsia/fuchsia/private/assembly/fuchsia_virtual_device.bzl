# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Rule for creating virtual devices for running in an emulator."""

load(":providers.bzl", "FuchsiaVirtualDeviceInfo")

ARCH = struct(
    ARM64 = "arm64",
    RISCV64 = "riscv64",
    X64 = "x64",
)

# Note: Due to bazel limitations, port numbers need to be passed as strings.
DEFAULT_PORTS = {
    "ssh": "22",
    "mdns": "5353",
    "debug": "2345",
    "adb": "5555",
}

def _fuchsia_virtual_device_impl(ctx):
    # Use Label name for output file to avoid conflicts. It's possible for different virtual device
    # targets to share the same device name.
    virtual_device_file = ctx.actions.declare_file(ctx.label.name + ".json")
    virtual_device = {
        "schema_id": "http://fuchsia.com/schemas/sdk/virtual_device.json",
        "data": {
            "type": "virtual_device",
            "name": ctx.attr.device_name,
            "description": ctx.attr.description,
            "avx2_enabled": ctx.attr.avx2_enabled,
            "hardware": {
                "cpu": {
                    "arch": ctx.attr.arch,
                    "count": ctx.attr.cpu_count,
                },
                "audio": {
                    "model": ctx.attr.audio_model,
                },
                "inputs": {
                    "pointing_device": ctx.attr.input_device,
                },
                "vsock": {
                    "enabled": ctx.attr.vsock_enabled,
                    "cid": ctx.attr.vsock_cid,
                },
                "window_size": {
                    "height": ctx.attr.window_height_px,
                    "width": ctx.attr.window_width_px,
                    "units": "pixels",
                },
                "memory": {
                    "quantity": ctx.attr.memory_quantity,
                    "units": ctx.attr.memory_unit,
                },
                "storage": {
                    "quantity": ctx.attr.storage_quantity,
                    "units": ctx.attr.storage_unit,
                },
            },
            "ports": {k: int(v) for k, v in ctx.attr.ports.items()},
        },
    }
    ctx.actions.write(virtual_device_file, json.encode(virtual_device))

    return [
        FuchsiaVirtualDeviceInfo(
            device_name = ctx.attr.device_name,
            config = virtual_device_file,
        ),
    ]

fuchsia_virtual_device = rule(
    doc = """Creates a fuchsia virtual device for running in an emulator.""",
    implementation = _fuchsia_virtual_device_impl,
    attrs = {
        "device_name": attr.string(
            doc = "Name of the virtual device.",
            mandatory = True,
        ),
        "avx2_enabled": attr.bool(
            doc = "Whether AVX2 (and AVX512) is enabled for the emulator.",
            default = True,
        ),
        "description": attr.string(
            doc = "Description of the virtual device.",
            default = "",
        ),
        "arch": attr.string(
            doc = "The architecture of the cpu.",
            values = [ARCH.X64, ARCH.ARM64, ARCH.RISCV64],
            mandatory = True,
        ),
        "cpu_count": attr.int(
            doc = "The number of CPUs",
            default = 4,
        ),
        "window_width_px": attr.int(
            doc = "Width of the virtual device's screen, in pixels.",
            default = 1200,
        ),
        "window_height_px": attr.int(
            doc = "Height of the virtual device's screen, in pixels.",
            default = 800,
        ),
        "memory_quantity": attr.int(
            doc = "Memory of the virtual device.",
            default = 8192,
        ),
        "memory_unit": attr.string(
            doc = "Unit for memory of the virtual device (e.g. megabytes, gigabytes, etc.).",
            default = "megabytes",
        ),
        "storage_quantity": attr.int(
            doc = "Storage of the virtual device.",
            default = 10,
        ),
        "storage_unit": attr.string(
            doc = "Unit for storage of the virtual device (e.g. megabytes, gigabytes, etc.).",
            default = "gigabytes",
        ),
        "vsock_enabled": attr.bool(
            doc = "Whether the virtual device should expose a vsock",
            default = False,
        ),
        "vsock_cid": attr.int(
            doc = "The context id the guest vsock should. Only used if vsock_enable = true and a value of zero indicates that a value should be generated",
            default = 0,
        ),
        "audio_model": attr.string(
            doc = "The audio device model that should be emulated.",
            values = ["ac97", "adlib", "cs4231A", "es1370", "gus", "hda", "none", "pcspk", "sb16"],
            default = "hda",
        ),
        "input_device": attr.string(
            doc = "The input device type that should be emulated.",
            values = ["mouse", "none", "touch"],
            # Touch is the default to avoid issues with mouse capture
            # especially with cloudtops.
            default = "touch",
        ),
        "ports": attr.string_dict(
            doc = "Dictionary of port names to port numbers which should be exposed to the host.",
            default = DEFAULT_PORTS,
        ),
    },
)
