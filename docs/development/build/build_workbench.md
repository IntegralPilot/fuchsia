# Build Workbench

Workbench (`workbench_eng`) is an open source reference design for Fuchsia.
Workbench is not a consumer-oriented product. Workbench is a tool for
developers to explore Fuchsia and experiment with evolving
concepts and features.

Workbench does not come with strong security, privacy, or robustness
guarantees. Bugs and rapid changes are expected (to help improve Fuchsia,
please [file bugs and send feedback][report-issue]).

## Get started with Workbench {:#get-started-with-workbench}

To get started with Workbench, you need to be familiar with how to get the
Fuchsia source code, build Fuchsia images, and run Fuchsia on a device or
emulator – the instructions in this section are based on the
[Get started with Fuchsia][get-started-with-fuchsia] flow.

Workbench is designed to be used with an Intel NUC or the Fuchsia emulator
(FEMU).

*   {Intel NUC}

    To install Workbench on an Intel NUC, do the following:

    1.  Complete the [Download the Fuchsia source code][get-fuchsia-source]
        guide.
    2.  As part of [Configure and Build Fuchsia][build-fuchsia], set your build
        configuration to use the following Workbench product:

        ```posix-terminal
        fx set workbench_eng.x64 --release
        ```

    3.  Complete the [Install Fuchsia on a NUC][intel-nuc] guide.

*   {FEMU}

    To try Workbench on the Fuchsia emulator, do the following:

    1.  Complete the [Download the Fuchsia source code][get-fuchsia-source]
        guide.
    2.  As part of [Configure and Build Fuchsia][build-fuchsia], set your build
        configuration to use the following Workbench product:

        ```posix-terminal
        fx set workbench_eng.x64 --release
        ```

        Note: Workbench does not support emulation on ARM64. To run Workbench
        on ARM64, you must use a physical hardware device like VIM3. If you
        must use the emulator on ARM64, you can run the `terminal` product
        with the `qemu-arm64` board (for example, `terminal.qemu-arm64`).

    3.  Complete the [Start the Fuchsia emulator][start-femu] guide.

<!-- Reference links -->

[report-issue]: /docs/contribute/report-issue.md
[get-started-with-fuchsia]: /docs/get-started
[get-fuchsia-source]: /docs/get-started/get_fuchsia_source.md
[build-fuchsia]: /docs/get-started/build_fuchsia.md
[intel-nuc]: /docs/development/hardware/intel_nuc.md
[start-femu]: /docs/get-started/set_up_femu.md
