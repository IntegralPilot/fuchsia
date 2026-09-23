# GN Build Arguments

## All builds

### acpica_debug_output

Enable debug output in the ACPI library (used by the ACPI bus driver).

**Current value (from the default):** `false`

From //zircon/system/ulib/acpica/acpica.gni:7

### add_qemu_to_build_archives

Whether to include images necessary to run Fuchsia in QEMU in build
archives.

**Current value (from the default):** `false`

From //build/images/args.gni:35

### additional_bazel_sdk_labels

Extra generate_fuchsia_bazel_sdk targets to be included in the
`bazel_sdk_info` API module. This allows defining bazel SDKs outside of the
main repository.

**Current value (from the default):** `[]`

From //BUILD.gn:105

### additional_default_targets

Platform builders can add targets to this list so that they get built with
the //:default target

**Current value (from the default):** `[]`

From //BUILD.gn:121

### additional_tefmocheck_labels

List of additional tefmocheck target labels to include in the manifest.
Defaults to empty.

**Current value (from the default):** `[]`

From //BUILD.gn:125

### all_cpu_kernel_boot_tests

Cause //zircon/kernel:boot_tests to generate the phys boot tests
for all supported CPUs, not just $target_cpu.

**Current value (from the default):** `false`

From //zircon/kernel/BUILD.gn:23

### all_cpu_phys_boot_tests

Cause //zircon/kernel/phys:boot_tests to generate the phys boot tests
for all supported CPUs, not just $target_cpu.

**Current value (from the default):** `false`

From //zircon/kernel/phys/BUILD.gn:26

### all_font_file_paths

List of file paths to every font asset. Populated in fonts.gni.

**Current value (from the default):** `[]`

From //src/fonts/build/font_args.gni:35

### all_toolchain_variants

*These should never be set as a build argument.*
It will be set below and passed to other toolchains through toolchain_args
(see variant_toolchain.gni).

**Current value (from the default):** `[]`

From //build/config/BUILDCONFIG.gn:787

### allowed_test_device_types

A list of device types this build is allowed to run tests on. If set, only
these device types will be used for tests.

**Current value (from the default):** `[]`

From //build/testing/test_spec.gni:14

### allowed_test_host_device_types

A list of host device types this build is allowed to run tests on. If set,
only these host device types will be used for tests.

**Current value (from the default):** `[]`

From //build/testing/test_spec.gni:18

### amlogic_decoder_firmware_path

Path to the amlogic decoder firmware file. Overrides the default in the
build.

**Current value (from the default):** `""`

From //src/media/drivers/amlogic_decoder/BUILD.gn:13

### archivist_max_cached_logs_bytes

**Current value (from the default):** `4194304`

From //src/diagnostics/archivist/configs.gni:6

### arm_sdk_tools

If true, then the arm64 host tools are included in the SDK.

**Current value (from the default):** `false`

From //src/developer/ffx/plugins/emulator/emu_companion.gni:9

### asan_default_options

Default [AddressSanitizer](https://clang.llvm.org/docs/AddressSanitizer.html)
options (before the `ASAN_OPTIONS` environment variable is read at
runtime).  This can be set as a build argument to affect most "asan"
variants in $variants (which see), or overridden in $toolchain_args in
one of those variants.  This can be a list of strings or a single string.

Note that even if this is empty, programs in this build **cannot** define
their own `__asan_default_options` C function.  Instead, they can use a
sanitizer_extra_options() target in their `deps` and then any options
injected that way can override that option's setting in this list.

**Current value (from the default):** `["detect_stack_use_after_return=1", "quarantine_size_mb=32"]`

From //build/config/sanitizers/sanitizer_default_options.gni:18

### assembly_board_configs

Platform builders should populate this list in their product.gni file.
The result will be built and uploaded to CIPD by infra.

**Current value (from the default):** `[]`

From //BUILD.gn:109

### assembly_partitions_configs

Platform builders should populate this list in their product.gni file.
The result will be built and uploaded to CIPD by infra.

**Current value (from the default):** `[]`

From //BUILD.gn:117

### assembly_product_configs

Platform builders should populate this list in their product.gni file.
The result will be built and uploaded to CIPD by infra.

**Current value (from the default):** `[]`

From //BUILD.gn:113

### avb_atx_metadata

AVB metadata which will be used to validate public key

**Current value for `target_cpu = "arm64"`:** `"//third_party/android/platform/external/avb/test/data/atx_metadata.bin"`

From //boards/arm64.gni:35

**Overridden from the default:** `""`

From //build/images/vbmeta.gni:20

**Current value (from the default):** `""`

From //build/images/vbmeta.gni:20

### avb_key

a key which will be used to sign VBMETA and images for AVB

**Current value for `target_cpu = "arm64"`:** `"//third_party/android/platform/external/avb/test/data/testkey_atx_psk.pem"`

From //boards/arm64.gni:37

**Overridden from the default:** `""`

From //build/images/vbmeta.gni:17

**Current value (from the default):** `""`

From //build/images/vbmeta.gni:17

### basic_env_names

The list of environment names to include in "basic_envs".

**Current value (from the default):** `["emu"]`

From //build/testing/environments.gni:7

### bazel_auto_refresh_compdb

Automatically refreshes compile_commands.json after bazel_actions are built.

NOTE: Enabling will add a several seconds of overhead to the build.

**Current value (from the default):** `false`

From //build/bazel/bazel_action.gni:15

### bazel_execution_logs

If true, emit additional execution logs, which contains information
about remote executions and their action digests, cache status,
remote inputs, and more.

**Current value (from the default):** `true`

From //build/bazel/config/logging.gni:9

### bazel_product_bundle_board

**Current value for `target_cpu = "arm64"`:** `"arm64"`

From //boards/arm64.gni:25

**Overridden from the default:** `false`

From //build/images/args.gni:71

**Current value for `target_cpu = "riscv64"`:** `"riscv64"`

From //boards/riscv64.gni:32

**Overridden from the default:** `false`

From //build/images/args.gni:71

**Current value for `target_cpu = "x64"`:** `"x64"`

From //boards/x64.gni:28

**Overridden from the default:** `false`

From //build/images/args.gni:71

### bazel_product_bundle_full

bazel_product_bundle_[full|root|prefix|board] together identifies the
bazel_product_bundle target in GN target to use in Bazel assembly. The
actual target used is:

  ${bazel_product_bundle_full}.${bazel_product_bundle_board}
  if ${bazel_product_bundle_full} is defined, else
  ${bazel_product_bundle_root}/${bazel_product_bundle_prefix}.${bazel_product_bundle_board}

NOTE: bazel_product_bundle_prefix should contain the fully qualified path
prefix to the target. Setting both arguments is a prerequisite to enable
Bazel assembly.

For example, given:

  bazel_product_bundle_root = "//"
  bazel_product_bundle_prefix = "build/bazel/assembly:minimal"
  bazel_product_bundle_board = "x64"

The actual bazel_product_bundle used for Bazel assembly is:

  //build/bazel/assembly:minimal.x64


**Current value (from the default):** `false`

From //build/images/args.gni:68

### bazel_product_bundle_prefix

**Current value for `target_cpu = "arm64"`:** `"//products/core:product_bundle"`

From //products/core.gni:26

**Overridden from the default:** `false`

From //build/images/args.gni:70

**Current value for `target_cpu = "riscv64"`:** `"//products/minimal:product_bundle"`

From //products/minimal.gni:34

**Overridden from the default:** `false`

From //build/images/args.gni:70

**Current value for `target_cpu = "x64"`:** `"//products/core:product_bundle"`

From //products/core.gni:26

**Overridden from the default:** `false`

From //build/images/args.gni:70

### bazel_product_bundle_root

**Current value (from the default):** `"//"`

From //build/images/args.gni:69

### bazel_rbe_download_outputs

Control what bazel remote-built outputs are downloaded.
See https://bazel.build/reference/command-line-reference#flag--remote_download_outputs
Valid options: all, minimal, toplevel (default since Bazel 7.1)
- 'toplevel' and 'minimal' can save significant download bandwidth
- 'all' is useful for debugging remote build issues

**Current value (from the default):** `"toplevel"`

From //build/bazel/remote_services.gni:32

### bazel_rbe_exec_strategy

When bazel is configured to use RBE, this controls the execution strategy
that is used.

Supported options:
  "remote": on cache-miss, build remotely (default)
  "local": on cache-miss, build locally
  "nocache": force execution, as if cache-miss.

**Current value (from the default):** `"remote"`

From //build/bazel/remote_services.gni:25

### bazel_upload_build_events

Configure bazel to stream build events and results to a service.
This is useful for sharing build results and invocation details
for reproducing and triaging issues.
This option uses direct network access and requires authentication.
The _infra variants are intended for use in build infrastructure.
More information can be found at:
https://bazel.build/remote/bep#build-event-service

Valid options:
  "": do not stream (default)
  "sponge": uploads to Sponge2 (for users)
  "sponge_infra": uploads to Sponge2 (for infra)
  "resultstore": uploads to ResultStore (for users)
  "resultstore_infra": uploads to ResultStore (for infra)

**Current value (from the default):** `""`

From //build/bazel/remote_services.gni:48

### blobfs_num_pager_threads

The number of pager threads to spawn for blobfs.

**Current value (from the default):** `2`

From //src/storage/blobfs/bin/BUILD.gn:20

### blobfs_page_in_metrics_recording

Set this to true when configuring gn args to enable blobfs page-in metrics
recording. This will also increase the inspect VMO size for blobfs to 2 MiB,
to accommodate the large number of metrics entries.

**Current value (from the default):** `false`

From //src/storage/blobfs/BUILD.gn:12

### board_configuration_label

The label for the board configuration target to use with Product Assembly

**Current value for `target_cpu = "arm64"`:** `"//boards/arm64"`

From //boards/arm64.gni:24

**Overridden from the default:** `false`

From //build/board.gni:28

**Current value for `target_cpu = "riscv64"`:** `"//boards/riscv64"`

From //boards/riscv64.gni:24

**Overridden from the default:** `false`

From //build/board.gni:28

**Current value for `target_cpu = "x64"`:** `"//boards/x64"`

From //boards/x64.gni:27

**Overridden from the default:** `false`

From //build/board.gni:28

### board_description

Human readable board description corresponding to the board name.

**Current value for `target_cpu = "arm64"`:** `"A generic emulated arm64 device."`

From //boards/arm64.gni:28

**Overridden from the default:** `""`

From //build/board.gni:14

**Current value for `target_cpu = "riscv64"`:** `"A generic emulated riscv64 device."`

From //boards/riscv64.gni:27

**Overridden from the default:** `""`

From //build/board.gni:14

**Current value for `target_cpu = "x64"`:** `"A generic x64 device"`

From //boards/x64.gni:24

**Overridden from the default:** `""`

From //build/board.gni:14

### board_is_emu

Whether or not the board supports emulator devices.
This is used to determine if product bundle metadata should generate a
virtual device spec or both.

**Current value for `target_cpu = "arm64"`:** `true`

From //boards/arm64.gni:31

**Overridden from the default:** `false`

From //build/board.gni:33

**Current value for `target_cpu = "riscv64"`:** `true`

From //boards/riscv64.gni:30

**Overridden from the default:** `false`

From //build/board.gni:33

**Current value for `target_cpu = "x64"`:** `true`

From //boards/x64.gni:31

**Overridden from the default:** `false`

From //build/board.gni:33

### board_name

Board name used for paving and amber updates.

**Current value for `target_cpu = "arm64"`:** `"arm64"`

From //boards/arm64.gni:27

**Overridden from the default:** `""`

From //build/board.gni:11

**Current value for `target_cpu = "riscv64"`:** `"riscv64"`

From //boards/riscv64.gni:26

**Overridden from the default:** `""`

From //build/board.gni:11

**Current value for `target_cpu = "x64"`:** `"x64"`

From //boards/x64.gni:23

**Overridden from the default:** `""`

From //build/board.gni:11

### board_tools

List of paths to board-specific tools to include in the build output.

Most development tools can just be used in-tree and do not need to be
included here. This arg is only meant for tools which may need to be
distributed along with the build files, for example tools for flashing
from SoC recovery mode.

Assets included in this way are included best-effort only and do not form
any kind of stable contract for users of the archive.

**Current value (from the default):** `[]`

From //build/board.gni:25

### build_all_vp9_file_decoder_conformance_tests

**Current value (from the default):** `false`

From //src/media/codec/examples/BUILD.gn:11

### build_id_format

Build ID algorithm to use for Fuchsia-target code.  This does not apply
to host or guest code.  The value is the argument to the linker's
`--build-id=...` switch.  If left empty (the default), the linker's
default format is used.

**Current value (from the default):** `""`

From //build/config/build_id.gni:10

### build_info_board

Board configuration of the current build

**Current value for `target_cpu = "arm64"`:** `"arm64"`

From //out/not-default/args.gn:8

**Overridden from the default:** `"arm64"`

From //build/info/info.gni:13

**Current value for `target_cpu = "riscv64"`:** `"riscv64"`

From //out/not-default/args.gn:8

**Overridden from the default:** `"riscv64"`

From //build/info/info.gni:13

**Current value for `target_cpu = "x64"`:** `"x64"`

From //out/not-default/args.gn:8

**Overridden from the default:** `"x64"`

From //build/info/info.gni:13

### build_info_product

LINT.IfChange
Product configuration of the current build

**Current value for `target_cpu = "arm64"`:** `"core"`

From //out/not-default/args.gn:9

**Overridden from the default:** `""`

From //build/info/info.gni:10

**Current value for `target_cpu = "riscv64"`:** `"minimal"`

From //out/not-default/args.gn:9

**Overridden from the default:** `""`

From //build/info/info.gni:10

**Current value for `target_cpu = "x64"`:** `"core"`

From //out/not-default/args.gn:9

**Overridden from the default:** `""`

From //build/info/info.gni:10

### build_info_version

Logical version of the current build. If not set, defaults to the timestamp
of the most recent update.

**Current value (from the default):** `""`

From //build/info/info.gni:17

### build_only_labels

These labels are added as dependencies of '//:default' transitively via
'//:build_only'.  These are used to add targets that need to be built
but aren't part of any product, board, etc.

These also serve as an alternative to '//:default' for sub-builds that
only want to build and define a small subset of the tree.

**Current value for `target_cpu = "arm64"`:** `[]`

From //out/not-default/args.gn:13

**Overridden from the default:** `[]`

From //BUILD.gn:133

**Current value for `target_cpu = "riscv64"`:** `[]`

From //out/not-default/args.gn:13

**Overridden from the default:** `[]`

From //BUILD.gn:133

**Current value for `target_cpu = "x64"`:** `[]`

From //out/not-default/args.gn:13

**Overridden from the default:** `[]`

From //BUILD.gn:133

### build_should_trace_actions

If enabled, all filesystem activity by actions will be traced and checked
against their declared inputs and outputs and depfiles (if present).
An action that accesses undeclared inputs or outputs will fail the build.

**Current value (from the default):** `false`

From //build/tracer/tracer.gni:12

### call_graph_section

Whether to emit a call graph section in the output file
https://clang.llvm.org/docs/ClangCommandLineReference.html#cmdoption-clang-fexperimental-call-graph-section
TODO(https://issues.fuchsia.dev/497876388): Enable call graph section by default after the next toolchain roll.

**Current value (from the default):** `false`

From //build/config/clang/call_graph_section.gni:9

### camera_debug

**Current value (from the default):** `false`

From //src/camera/debug.gni:6

### camera_gym_configuration_cycle_interval_ms

**Current value (from the default):** `10000`

From //src/camera/bin/camera-gym/BUILD.gn:13

### carnelian_enable_vulkan_validation

Include the vulkan validation layers in carnelian examples.

**Current value (from the default):** `false`

From //src/lib/ui/carnelian/BUILD.gn:13

### carnelian_static_images_extras

Point this to the location of external image files to be included as extras

**Current value (from the default):** `[]`

From //src/lib/ui/carnelian/BUILD.gn:16

### carnelian_static_rives_extras

Point this to the location of external rive files to be included as extras

**Current value (from the default):** `[]`

From //src/lib/ui/carnelian/BUILD.gn:19

### carnelian_static_txts_extras

Point this to the location of external txt files to be included as extras

**Current value (from the default):** `[]`

From //src/lib/ui/carnelian/BUILD.gn:22

### channel_message_body_tracing_enabled

Include partial channel message bodies in IPC traces. # This is intended as
a local debugging feature and is not intended for production use.

**Current value (from the default):** `false`

From //zircon/kernel/params.gni:124

### check_output_dir_leaks

If enabled, check that the output dir path does not leak into
the command or any of its output files.  This is important for
remote build consistency and caching.

**Current value (from the default):** `true`

From //build/tracer/tracer.gni:21

### check_repeatability

If enabled, run each affected action twice (once with renamed outputs)
and compare the outputs' contents for reproducibility.

**Current value (from the default):** `false`

From //build/tracer/tracer.gni:16

### check_vtables_in_rodata

Check that all vtables in fuchsia binaries listed in binaries.json are in
readonly data sections. This check will be run at the end of a full build.

This is primarily meant to be used by the clang canary builders.

**Current value (from the default):** `false`

From //build/images/args.gni:12

### chromium_build_dir

This variable specifies a fully qualified Chromium build output directory,
such as `/home/$USER/chrome/src/out/fuchsia`, from which `web_engine` will
be obtained.
All of those targets must exist in the output directory.
If unset, the prebuilt packages from CIPD will be used.

**Current value (from the default):** `""`

From //src/chromium/build_args.gni:11

### cipd_assembly_artifact_targets

Targets to be traversed by //:cipd_assembly_artifacts for GN metadata only.
These targets are expected to set "assembly_inputs" in metadata, which can
include a JSON file describing artifacts to be uploaded to CPID.

NOTE: These targets are for GN metadata walk only. If the artifacts need to
be built, they should be included in the build graph through other means.

**Current value (from the default):** `["//build/images:main_assembly"]`

From //build/product.gni:21

### clang_embed_bitcode

Embed LLVM bitcode as .llvmbc section in ELF files. This is intended
primarily for external tools that use bitcode for analysis.

**Current value (from the default):** `false`

From //build/config/clang/clang.gni:12

### clang_enable_error_reproducers

Enable reproducers on error. This provides crash-like reproducers on
compiler errors in addition to crashes.
Note, this flag should be used by very few people at the moment
because it depends on features that are not yet in prebuilt clang.
It is only useful for clang canary builders, and folks with a custom
clang.

**Current value (from the default):** `false`

From //build/config/clang/clang.gni:20

### clang_ml_inliner

Controls whether to use the ML inliner in Clang to reduce size.

**Current value (from the default):** `true`

From //build/config/clang/clang.gni:23

### clang_prefix

The default clang toolchain provided by the prebuilt. This variable is
additionally consumed by the Go toolchain.
LINT.IfChange

**Current value (from the default):** `"//prebuilt/third_party/clang/linux-x64/bin"`

From //build/config/clang/clang_prefix.gni:11

### clang_tool_dir

Directory where the Clang toolchain binaries ("clang", "llvm-nm", etc.) are
found.  If this is "", then the behavior depends on $clang_prefix.
This toolchain is expected to support both Fuchsia targets and the host.

**Current value (from the default):** `""`

From //build/toolchain/zircon/clang.gni:11

### clang_toolchain_info

A scope that contains information about the current Clang toolchain.
This should never be set as a build argument.

**Current value (from the default):**

```none
{
  aarch64_unknown_fuchsia = {
  libclang_rt_profile_a = "lib/clang/24/lib/aarch64-unknown-fuchsia/libclang_rt.profile.a"
  libunwind_so = "lib/aarch64-unknown-fuchsia/libunwind.so.1.0"
  resource_dir = "lib/clang/24"
  variants = {
  asan = {
  shared = {
  clang_rt = "lib/clang/24/lib/aarch64-unknown-fuchsia/libclang_rt.asan.so"
}
  static = {
  clang_rt = "lib/clang/24/lib/aarch64-unknown-fuchsia/libclang_rt.asan.a"
  clang_rt_cxx = "lib/clang/24/lib/aarch64-unknown-fuchsia/libclang_rt.asan_cxx.a"
}
}
  hwasan = {
  shared = {
  clang_rt = "lib/clang/24/lib/aarch64-unknown-fuchsia/libclang_rt.hwasan.so"
}
  static = {
  clang_rt = "lib/clang/24/lib/aarch64-unknown-fuchsia/libclang_rt.hwasan.a"
  clang_rt_cxx = "lib/clang/24/lib/aarch64-unknown-fuchsia/libclang_rt.hwasan_cxx.a"
}
}
  lsan = {
  shared = {
  clang_rt = "../../../../out/not-default/libclang_rt.lsan.so"
}
  static = {
  clang_rt = "lib/clang/24/lib/aarch64-unknown-fuchsia/libclang_rt.lsan.a"
  clang_rt_cxx = ""
}
}
  tsan = {
  shared = {
  clang_rt = "../../../../out/not-default/libclang_rt.tsan.so"
}
  static = {
  clang_rt = "../../../../out/not-default/libclang_rt.tsan.a"
  clang_rt_cxx = "../../../../out/not-default/libclang_rt.tsan_cxx.a"
}
}
  ubsan = {
  shared = {
  clang_rt = "lib/clang/24/lib/aarch64-unknown-fuchsia/libclang_rt.ubsan_standalone.so"
}
  static = {
  clang_rt = "lib/clang/24/lib/aarch64-unknown-fuchsia/libclang_rt.ubsan_standalone.a"
  clang_rt_cxx = "lib/clang/24/lib/aarch64-unknown-fuchsia/libclang_rt.ubsan_standalone_cxx.a"
}
}
}
}
  aarch64_unknown_linux_gnu = {
  libclang_rt_profile_a = "lib/clang/24/lib/aarch64-unknown-linux-gnu/libclang_rt.profile.a"
  libunwind_so = "../../../../out/not-default/libunwind.so"
  resource_dir = "lib/clang/24"
  variants = {
  asan = {
  shared = {
  clang_rt = "lib/clang/24/lib/aarch64-unknown-linux-gnu/libclang_rt.asan.so"
}
  static = {
  clang_rt = "lib/clang/24/lib/aarch64-unknown-linux-gnu/libclang_rt.asan.a"
  clang_rt_cxx = "lib/clang/24/lib/aarch64-unknown-linux-gnu/libclang_rt.asan_cxx.a"
}
}
  hwasan = {
  shared = {
  clang_rt = "lib/clang/24/lib/aarch64-unknown-linux-gnu/libclang_rt.hwasan.so"
}
  static = {
  clang_rt = "lib/clang/24/lib/aarch64-unknown-linux-gnu/libclang_rt.hwasan.a"
  clang_rt_cxx = "lib/clang/24/lib/aarch64-unknown-linux-gnu/libclang_rt.hwasan_cxx.a"
}
}
  lsan = {
  shared = {
  clang_rt = "../../../../out/not-default/libclang_rt.lsan.so"
}
  static = {
  clang_rt = "lib/clang/24/lib/aarch64-unknown-linux-gnu/libclang_rt.lsan.a"
  clang_rt_cxx = "../../../../out/not-default/libclang_rt.lsan_cxx.a"
}
}
  tsan = {
  shared = {
  clang_rt = "lib/clang/24/lib/aarch64-unknown-linux-gnu/libclang_rt.tsan.so"
}
  static = {
  clang_rt = "lib/clang/24/lib/aarch64-unknown-linux-gnu/libclang_rt.tsan.a"
  clang_rt_cxx = "lib/clang/24/lib/aarch64-unknown-linux-gnu/libclang_rt.tsan_cxx.a"
}
}
  ubsan = {
  shared = {
  clang_rt = "lib/clang/24/lib/aarch64-unknown-linux-gnu/libclang_rt.ubsan_standalone.so"
}
  static = {
  clang_rt = "lib/clang/24/lib/aarch64-unknown-linux-gnu/libclang_rt.ubsan_standalone.a"
  clang_rt_cxx = "lib/clang/24/lib/aarch64-unknown-linux-gnu/libclang_rt.ubsan_standalone_cxx.a"
}
}
}
}
  armv7_unknown_linux_gnueabihf = {
  libclang_rt_profile_a = "lib/clang/24/lib/armv7-unknown-linux-gnueabihf/libclang_rt.profile.a"
  libunwind_so = "../../../../out/not-default/libunwind.so"
  resource_dir = "lib/clang/24"
  variants = {
  asan = {
  shared = {
  clang_rt = "lib/clang/24/lib/armv7-unknown-linux-gnueabihf/libclang_rt.asan.so"
}
  static = {
  clang_rt = "lib/clang/24/lib/armv7-unknown-linux-gnueabihf/libclang_rt.asan.a"
  clang_rt_cxx = "lib/clang/24/lib/armv7-unknown-linux-gnueabihf/libclang_rt.asan_cxx.a"
}
}
  hwasan = {
  shared = {
  clang_rt = "../../../../out/not-default/libclang_rt.hwasan.so"
}
  static = {
  clang_rt = "../../../../out/not-default/libclang_rt.hwasan.a"
  clang_rt_cxx = "../../../../out/not-default/libclang_rt.hwasan_cxx.a"
}
}
  lsan = {
  shared = {
  clang_rt = "../../../../out/not-default/libclang_rt.lsan.so"
}
  static = {
  clang_rt = "lib/clang/24/lib/armv7-unknown-linux-gnueabihf/libclang_rt.lsan.a"
  clang_rt_cxx = "../../../../out/not-default/libclang_rt.lsan_cxx.a"
}
}
  tsan = {
  shared = {
  clang_rt = "../../../../out/not-default/libclang_rt.tsan.so"
}
  static = {
  clang_rt = ""
  clang_rt_cxx = "../../../../out/not-default/libclang_rt.tsan_cxx.a"
}
}
  ubsan = {
  shared = {
  clang_rt = "lib/clang/24/lib/armv7-unknown-linux-gnueabihf/libclang_rt.ubsan_standalone.so"
}
  static = {
  clang_rt = "lib/clang/24/lib/armv7-unknown-linux-gnueabihf/libclang_rt.ubsan_standalone.a"
  clang_rt_cxx = "lib/clang/24/lib/armv7-unknown-linux-gnueabihf/libclang_rt.ubsan_standalone_cxx.a"
}
}
}
}
  fallback = {
  libclang_rt_profile_a = ""
  libunwind_so = "../../../../out/not-default/libunwind.so"
  resource_dir = "lib/clang/24"
  variants = { }
}
  riscv64_unknown_fuchsia = {
  libclang_rt_profile_a = "lib/clang/24/lib/riscv64-unknown-fuchsia/libclang_rt.profile.a"
  libunwind_so = "lib/riscv64-unknown-fuchsia/libunwind.so.1.0"
  resource_dir = "lib/clang/24"
  variants = {
  asan = {
  shared = {
  clang_rt = "lib/clang/24/lib/riscv64-unknown-fuchsia/libclang_rt.asan.so"
}
  static = {
  clang_rt = "lib/clang/24/lib/riscv64-unknown-fuchsia/libclang_rt.asan.a"
  clang_rt_cxx = "lib/clang/24/lib/riscv64-unknown-fuchsia/libclang_rt.asan_cxx.a"
}
}
  hwasan = {
  shared = {
  clang_rt = "lib/clang/24/lib/riscv64-unknown-fuchsia/libclang_rt.hwasan.so"
}
  static = {
  clang_rt = "lib/clang/24/lib/riscv64-unknown-fuchsia/libclang_rt.hwasan.a"
  clang_rt_cxx = "lib/clang/24/lib/riscv64-unknown-fuchsia/libclang_rt.hwasan_cxx.a"
}
}
  lsan = {
  shared = {
  clang_rt = "../../../../out/not-default/libclang_rt.lsan.so"
}
  static = {
  clang_rt = "lib/clang/24/lib/riscv64-unknown-fuchsia/libclang_rt.lsan.a"
  clang_rt_cxx = "../../../../out/not-default/libclang_rt.lsan_cxx.a"
}
}
  tsan = {
  shared = {
  clang_rt = "../../../../out/not-default/libclang_rt.tsan.so"
}
  static = {
  clang_rt = "../../../../out/not-default/libclang_rt.tsan.a"
  clang_rt_cxx = "../../../../out/not-default/libclang_rt.tsan_cxx.a"
}
}
  ubsan = {
  shared = {
  clang_rt = "lib/clang/24/lib/riscv64-unknown-fuchsia/libclang_rt.ubsan_standalone.so"
}
  static = {
  clang_rt = "lib/clang/24/lib/riscv64-unknown-fuchsia/libclang_rt.ubsan_standalone.a"
  clang_rt_cxx = "lib/clang/24/lib/riscv64-unknown-fuchsia/libclang_rt.ubsan_standalone_cxx.a"
}
}
}
}
  riscv64_unknown_linux_gnu = {
  libclang_rt_profile_a = "lib/clang/24/lib/riscv64-unknown-linux-gnu/libclang_rt.profile.a"
  libunwind_so = "../../../../out/not-default/libunwind.so"
  resource_dir = "lib/clang/24"
  variants = {
  asan = {
  shared = {
  clang_rt = "lib/clang/24/lib/riscv64-unknown-linux-gnu/libclang_rt.asan.so"
}
  static = {
  clang_rt = "lib/clang/24/lib/riscv64-unknown-linux-gnu/libclang_rt.asan.a"
  clang_rt_cxx = "lib/clang/24/lib/riscv64-unknown-linux-gnu/libclang_rt.asan_cxx.a"
}
}
  hwasan = {
  shared = {
  clang_rt = "lib/clang/24/lib/riscv64-unknown-linux-gnu/libclang_rt.hwasan.so"
}
  static = {
  clang_rt = "lib/clang/24/lib/riscv64-unknown-linux-gnu/libclang_rt.hwasan.a"
  clang_rt_cxx = "lib/clang/24/lib/riscv64-unknown-linux-gnu/libclang_rt.hwasan_cxx.a"
}
}
  lsan = {
  shared = {
  clang_rt = "../../../../out/not-default/libclang_rt.lsan.so"
}
  static = {
  clang_rt = "lib/clang/24/lib/riscv64-unknown-linux-gnu/libclang_rt.lsan.a"
  clang_rt_cxx = "../../../../out/not-default/libclang_rt.lsan_cxx.a"
}
}
  tsan = {
  shared = {
  clang_rt = "lib/clang/24/lib/riscv64-unknown-linux-gnu/libclang_rt.tsan.so"
}
  static = {
  clang_rt = "lib/clang/24/lib/riscv64-unknown-linux-gnu/libclang_rt.tsan.a"
  clang_rt_cxx = "lib/clang/24/lib/riscv64-unknown-linux-gnu/libclang_rt.tsan_cxx.a"
}
}
  ubsan = {
  shared = {
  clang_rt = "lib/clang/24/lib/riscv64-unknown-linux-gnu/libclang_rt.ubsan_standalone.so"
}
  static = {
  clang_rt = "lib/clang/24/lib/riscv64-unknown-linux-gnu/libclang_rt.ubsan_standalone.a"
  clang_rt_cxx = "lib/clang/24/lib/riscv64-unknown-linux-gnu/libclang_rt.ubsan_standalone_cxx.a"
}
}
}
}
  runtimes = [{
  cflags = []
  ldflags = []
  runtime = [{
  breakpad = "debug/.build-id/91/f19b71b331be4e54a8b8bdfea1e366032a8530.sym"
  debug = "debug/.build-id/91/f19b71b331be4e54a8b8bdfea1e366032a8530.debug"
  dist = "aarch64-unknown-fuchsia/libc++.so.2"
  name = "libc++"
  soname = "libc++.so.2"
}, {
  breakpad = "debug/.build-id/d7/ddc951dd96b03fa8e3d226bd4dc49812cdbc0c.sym"
  debug = "debug/.build-id/d7/ddc951dd96b03fa8e3d226bd4dc49812cdbc0c.debug"
  dist = "aarch64-unknown-fuchsia/libc++abi.so.1"
  name = "libc++abi"
  soname = "libc++abi.so.1"
}, {
  breakpad = "debug/.build-id/e2/8dfe2944bc812be62fa7b0fb8917bd9045f12a.sym"
  debug = "debug/.build-id/e2/8dfe2944bc812be62fa7b0fb8917bd9045f12a.debug"
  dist = "aarch64-unknown-fuchsia/libunwind.so.1"
  name = "libunwind"
  soname = "libunwind.so.1"
}]
  target = ["aarch64-unknown-fuchsia", "aarch64-fuchsia"]
}, {
  cflags = ["-fsanitize=address"]
  ldflags = []
  runtime = [{
  breakpad = "debug/.build-id/d1/0717f1a365b4e222da1e89e7783cc7932f5149.sym"
  debug = "debug/.build-id/d1/0717f1a365b4e222da1e89e7783cc7932f5149.debug"
  dist = "clang/24/lib/aarch64-unknown-fuchsia/libclang_rt.asan.so"
  soname = "libclang_rt.asan.so"
}, {
  breakpad = "debug/.build-id/02/ba22fcefd7ca35a6f3b92435934684f0ec3edd.sym"
  debug = "debug/.build-id/02/ba22fcefd7ca35a6f3b92435934684f0ec3edd.debug"
  dist = "aarch64-unknown-fuchsia/asan/libc++.so.2"
  name = "libc++"
  soname = "libc++.so.2"
}, {
  breakpad = "debug/.build-id/31/8e24c30f8421373fbe319a28397f5c16d66261.sym"
  debug = "debug/.build-id/31/8e24c30f8421373fbe319a28397f5c16d66261.debug"
  dist = "aarch64-unknown-fuchsia/asan/libc++abi.so.1"
  name = "libc++abi"
  soname = "libc++abi.so.1"
}, {
  breakpad = "debug/.build-id/a4/1c0e45ac46be96ef71aca5a83004f6e3d7098d.sym"
  debug = "debug/.build-id/a4/1c0e45ac46be96ef71aca5a83004f6e3d7098d.debug"
  dist = "aarch64-unknown-fuchsia/asan/libunwind.so.1"
  name = "libunwind"
  soname = "libunwind.so.1"
}]
  target = ["aarch64-unknown-fuchsia", "aarch64-fuchsia"]
}, {
  cflags = ["-fsanitize=undefined"]
  ldflags = []
  runtime = [{
  breakpad = "debug/.build-id/c8/6d9055252956eaacecab2ca9466e7e2685e877.sym"
  debug = "debug/.build-id/c8/6d9055252956eaacecab2ca9466e7e2685e877.debug"
  dist = "clang/24/lib/aarch64-unknown-fuchsia/libclang_rt.ubsan_standalone.so"
  soname = "libclang_rt.ubsan_standalone.so"
}, {
  breakpad = "debug/.build-id/91/f19b71b331be4e54a8b8bdfea1e366032a8530.sym"
  debug = "debug/.build-id/91/f19b71b331be4e54a8b8bdfea1e366032a8530.debug"
  dist = "aarch64-unknown-fuchsia/libc++.so.2"
  name = "libc++"
  soname = "libc++.so.2"
}, {
  breakpad = "debug/.build-id/d7/ddc951dd96b03fa8e3d226bd4dc49812cdbc0c.sym"
  debug = "debug/.build-id/d7/ddc951dd96b03fa8e3d226bd4dc49812cdbc0c.debug"
  dist = "aarch64-unknown-fuchsia/libc++abi.so.1"
  name = "libc++abi"
  soname = "libc++abi.so.1"
}, {
  breakpad = "debug/.build-id/e2/8dfe2944bc812be62fa7b0fb8917bd9045f12a.sym"
  debug = "debug/.build-id/e2/8dfe2944bc812be62fa7b0fb8917bd9045f12a.debug"
  dist = "aarch64-unknown-fuchsia/libunwind.so.1"
  name = "libunwind"
  soname = "libunwind.so.1"
}]
  target = ["aarch64-unknown-fuchsia", "aarch64-fuchsia"]
}, {
  cflags = ["-fsanitize=hwaddress"]
  ldflags = []
  runtime = [{
  breakpad = "debug/.build-id/09/2a487122b944a2dbcf312ac3c17611e1e7cc62.sym"
  debug = "debug/.build-id/09/2a487122b944a2dbcf312ac3c17611e1e7cc62.debug"
  dist = "clang/24/lib/aarch64-unknown-fuchsia/libclang_rt.hwasan.so"
  soname = "libclang_rt.hwasan.so"
}, {
  breakpad = "debug/.build-id/f8/94fc4023b8547b12b1c3f7fa0dfc82e3eaf02a.sym"
  debug = "debug/.build-id/f8/94fc4023b8547b12b1c3f7fa0dfc82e3eaf02a.debug"
  dist = "aarch64-unknown-fuchsia/hwasan/libc++.so.2"
  name = "libc++"
  soname = "libc++.so.2"
}, {
  breakpad = "debug/.build-id/4a/4b40b1e14744bf0be96a893e61fc4a86d3afc8.sym"
  debug = "debug/.build-id/4a/4b40b1e14744bf0be96a893e61fc4a86d3afc8.debug"
  dist = "aarch64-unknown-fuchsia/hwasan/libc++abi.so.1"
  name = "libc++abi"
  soname = "libc++abi.so.1"
}, {
  breakpad = "debug/.build-id/29/dcdbf8dad4f0f073b6c336534afeb1cfc01404.sym"
  debug = "debug/.build-id/29/dcdbf8dad4f0f073b6c336534afeb1cfc01404.debug"
  dist = "aarch64-unknown-fuchsia/hwasan/libunwind.so.1"
  name = "libunwind"
  soname = "libunwind.so.1"
}]
  target = ["aarch64-unknown-fuchsia", "aarch64-fuchsia"]
}, {
  cflags = []
  ldflags = ["-static-libstdc++"]
  runtime = []
  target = ["aarch64-unknown-fuchsia", "aarch64-fuchsia"]
}, {
  cflags = ["-fsanitize=address"]
  ldflags = ["-static-libstdc++"]
  runtime = [{
  breakpad = "debug/.build-id/d1/0717f1a365b4e222da1e89e7783cc7932f5149.sym"
  debug = "debug/.build-id/d1/0717f1a365b4e222da1e89e7783cc7932f5149.debug"
  dist = "clang/24/lib/aarch64-unknown-fuchsia/libclang_rt.asan.so"
  soname = "libclang_rt.asan.so"
}, {
  breakpad = "debug/.build-id/02/ba22fcefd7ca35a6f3b92435934684f0ec3edd.sym"
  debug = "debug/.build-id/02/ba22fcefd7ca35a6f3b92435934684f0ec3edd.debug"
  dist = "aarch64-unknown-fuchsia/asan/libc++.so.2"
  name = "libc++"
  soname = "libc++.so.2"
}, {
  breakpad = "debug/.build-id/31/8e24c30f8421373fbe319a28397f5c16d66261.sym"
  debug = "debug/.build-id/31/8e24c30f8421373fbe319a28397f5c16d66261.debug"
  dist = "aarch64-unknown-fuchsia/asan/libc++abi.so.1"
  name = "libc++abi"
  soname = "libc++abi.so.1"
}, {
  breakpad = "debug/.build-id/a4/1c0e45ac46be96ef71aca5a83004f6e3d7098d.sym"
  debug = "debug/.build-id/a4/1c0e45ac46be96ef71aca5a83004f6e3d7098d.debug"
  dist = "aarch64-unknown-fuchsia/asan/libunwind.so.1"
  name = "libunwind"
  soname = "libunwind.so.1"
}]
  target = ["aarch64-unknown-fuchsia", "aarch64-fuchsia"]
}, {
  cflags = ["-fsanitize=undefined"]
  ldflags = ["-static-libstdc++"]
  runtime = [{
  breakpad = "debug/.build-id/c8/6d9055252956eaacecab2ca9466e7e2685e877.sym"
  debug = "debug/.build-id/c8/6d9055252956eaacecab2ca9466e7e2685e877.debug"
  dist = "clang/24/lib/aarch64-unknown-fuchsia/libclang_rt.ubsan_standalone.so"
  soname = "libclang_rt.ubsan_standalone.so"
}, {
  breakpad = "debug/.build-id/91/f19b71b331be4e54a8b8bdfea1e366032a8530.sym"
  debug = "debug/.build-id/91/f19b71b331be4e54a8b8bdfea1e366032a8530.debug"
  dist = "aarch64-unknown-fuchsia/libc++.so.2"
  name = "libc++"
  soname = "libc++.so.2"
}, {
  breakpad = "debug/.build-id/d7/ddc951dd96b03fa8e3d226bd4dc49812cdbc0c.sym"
  debug = "debug/.build-id/d7/ddc951dd96b03fa8e3d226bd4dc49812cdbc0c.debug"
  dist = "aarch64-unknown-fuchsia/libc++abi.so.1"
  name = "libc++abi"
  soname = "libc++abi.so.1"
}, {
  breakpad = "debug/.build-id/e2/8dfe2944bc812be62fa7b0fb8917bd9045f12a.sym"
  debug = "debug/.build-id/e2/8dfe2944bc812be62fa7b0fb8917bd9045f12a.debug"
  dist = "aarch64-unknown-fuchsia/libunwind.so.1"
  name = "libunwind"
  soname = "libunwind.so.1"
}]
  target = ["aarch64-unknown-fuchsia", "aarch64-fuchsia"]
}, {
  cflags = ["-fsanitize=hwaddress"]
  ldflags = ["-static-libstdc++"]
  runtime = [{
  breakpad = "debug/.build-id/09/2a487122b944a2dbcf312ac3c17611e1e7cc62.sym"
  debug = "debug/.build-id/09/2a487122b944a2dbcf312ac3c17611e1e7cc62.debug"
  dist = "clang/24/lib/aarch64-unknown-fuchsia/libclang_rt.hwasan.so"
  soname = "libclang_rt.hwasan.so"
}, {
  breakpad = "debug/.build-id/f8/94fc4023b8547b12b1c3f7fa0dfc82e3eaf02a.sym"
  debug = "debug/.build-id/f8/94fc4023b8547b12b1c3f7fa0dfc82e3eaf02a.debug"
  dist = "aarch64-unknown-fuchsia/hwasan/libc++.so.2"
  name = "libc++"
  soname = "libc++.so.2"
}, {
  breakpad = "debug/.build-id/4a/4b40b1e14744bf0be96a893e61fc4a86d3afc8.sym"
  debug = "debug/.build-id/4a/4b40b1e14744bf0be96a893e61fc4a86d3afc8.debug"
  dist = "aarch64-unknown-fuchsia/hwasan/libc++abi.so.1"
  name = "libc++abi"
  soname = "libc++abi.so.1"
}, {
  breakpad = "debug/.build-id/29/dcdbf8dad4f0f073b6c336534afeb1cfc01404.sym"
  debug = "debug/.build-id/29/dcdbf8dad4f0f073b6c336534afeb1cfc01404.debug"
  dist = "aarch64-unknown-fuchsia/hwasan/libunwind.so.1"
  name = "libunwind"
  soname = "libunwind.so.1"
}]
  target = ["aarch64-unknown-fuchsia", "aarch64-fuchsia"]
}, {
  cflags = []
  ldflags = []
  runtime = [{
  debug = "debug/.build-id/62/fe6e43cb7f262d190386744cd0423c72ec62ec.debug"
  dist = "riscv64-unknown-fuchsia/libc++.so.2"
  name = "libc++"
  soname = "libc++.so.2"
}, {
  debug = "debug/.build-id/17/d39e1846930235f9435a91d2bb42d68ab7d065.debug"
  dist = "riscv64-unknown-fuchsia/libc++abi.so.1"
  name = "libc++abi"
  soname = "libc++abi.so.1"
}, {
  debug = "debug/.build-id/d7/c113830a04ad2499a2e2c909c507da2a1682df.debug"
  dist = "riscv64-unknown-fuchsia/libunwind.so.1"
  name = "libunwind"
  soname = "libunwind.so.1"
}]
  target = ["riscv64-unknown-fuchsia", "riscv64-fuchsia"]
}, {
  cflags = ["-fsanitize=address"]
  ldflags = []
  runtime = [{
  debug = "debug/.build-id/53/9d747c8582946972ca50b0004deef434909b60.debug"
  dist = "clang/24/lib/riscv64-unknown-fuchsia/libclang_rt.asan.so"
  soname = "libclang_rt.asan.so"
}, {
  debug = "debug/.build-id/bd/83bf9e421bfc028b1d10a9075647647ff2eebc.debug"
  dist = "riscv64-unknown-fuchsia/asan/libc++.so.2"
  name = "libc++"
  soname = "libc++.so.2"
}, {
  debug = "debug/.build-id/a3/cb5ba70820e9f74fef30121184264def75f85f.debug"
  dist = "riscv64-unknown-fuchsia/asan/libc++abi.so.1"
  name = "libc++abi"
  soname = "libc++abi.so.1"
}, {
  debug = "debug/.build-id/8e/7c0d9d30b512880749ce31e2b1c2204179c6f4.debug"
  dist = "riscv64-unknown-fuchsia/asan/libunwind.so.1"
  name = "libunwind"
  soname = "libunwind.so.1"
}]
  target = ["riscv64-unknown-fuchsia", "riscv64-fuchsia"]
}, {
  cflags = ["-fsanitize=undefined"]
  ldflags = []
  runtime = [{
  debug = "debug/.build-id/4a/8b8e3c531f8db9d3ff9f19b6ce9c8af26a09e6.debug"
  dist = "clang/24/lib/riscv64-unknown-fuchsia/libclang_rt.ubsan_standalone.so"
  soname = "libclang_rt.ubsan_standalone.so"
}, {
  debug = "debug/.build-id/62/fe6e43cb7f262d190386744cd0423c72ec62ec.debug"
  dist = "riscv64-unknown-fuchsia/libc++.so.2"
  name = "libc++"
  soname = "libc++.so.2"
}, {
  debug = "debug/.build-id/17/d39e1846930235f9435a91d2bb42d68ab7d065.debug"
  dist = "riscv64-unknown-fuchsia/libc++abi.so.1"
  name = "libc++abi"
  soname = "libc++abi.so.1"
}, {
  debug = "debug/.build-id/d7/c113830a04ad2499a2e2c909c507da2a1682df.debug"
  dist = "riscv64-unknown-fuchsia/libunwind.so.1"
  name = "libunwind"
  soname = "libunwind.so.1"
}]
  target = ["riscv64-unknown-fuchsia", "riscv64-fuchsia"]
}, {
  cflags = ["-fsanitize=hwaddress"]
  ldflags = []
  runtime = [{
  debug = "debug/.build-id/6e/4bce0bc423606edc38a8e924d4b83d750cb0c6.debug"
  dist = "clang/24/lib/riscv64-unknown-fuchsia/libclang_rt.hwasan.so"
  soname = "libclang_rt.hwasan.so"
}, {
  debug = "debug/.build-id/62/fe6e43cb7f262d190386744cd0423c72ec62ec.debug"
  dist = "riscv64-unknown-fuchsia/hwasan/libc++.so.2"
  name = "libc++"
  soname = "libc++.so.2"
}, {
  debug = "debug/.build-id/17/d39e1846930235f9435a91d2bb42d68ab7d065.debug"
  dist = "riscv64-unknown-fuchsia/hwasan/libc++abi.so.1"
  name = "libc++abi"
  soname = "libc++abi.so.1"
}, {
  debug = "debug/.build-id/21/378a2e3087b5822ce113874d8899129d4cabc5.debug"
  dist = "riscv64-unknown-fuchsia/hwasan/libunwind.so.1"
  name = "libunwind"
  soname = "libunwind.so.1"
}]
  target = ["riscv64-unknown-fuchsia", "riscv64-fuchsia"]
}, {
  cflags = []
  ldflags = ["-static-libstdc++"]
  runtime = []
  target = ["riscv64-unknown-fuchsia", "riscv64-fuchsia"]
}, {
  cflags = ["-fsanitize=address"]
  ldflags = ["-static-libstdc++"]
  runtime = [{
  debug = "debug/.build-id/53/9d747c8582946972ca50b0004deef434909b60.debug"
  dist = "clang/24/lib/riscv64-unknown-fuchsia/libclang_rt.asan.so"
  soname = "libclang_rt.asan.so"
}, {
  debug = "debug/.build-id/bd/83bf9e421bfc028b1d10a9075647647ff2eebc.debug"
  dist = "riscv64-unknown-fuchsia/asan/libc++.so.2"
  name = "libc++"
  soname = "libc++.so.2"
}, {
  debug = "debug/.build-id/a3/cb5ba70820e9f74fef30121184264def75f85f.debug"
  dist = "riscv64-unknown-fuchsia/asan/libc++abi.so.1"
  name = "libc++abi"
  soname = "libc++abi.so.1"
}, {
  debug = "debug/.build-id/8e/7c0d9d30b512880749ce31e2b1c2204179c6f4.debug"
  dist = "riscv64-unknown-fuchsia/asan/libunwind.so.1"
  name = "libunwind"
  soname = "libunwind.so.1"
}]
  target = ["riscv64-unknown-fuchsia", "riscv64-fuchsia"]
}, {
  cflags = ["-fsanitize=undefined"]
  ldflags = ["-static-libstdc++"]
  runtime = [{
  debug = "debug/.build-id/4a/8b8e3c531f8db9d3ff9f19b6ce9c8af26a09e6.debug"
  dist = "clang/24/lib/riscv64-unknown-fuchsia/libclang_rt.ubsan_standalone.so"
  soname = "libclang_rt.ubsan_standalone.so"
}, {
  debug = "debug/.build-id/62/fe6e43cb7f262d190386744cd0423c72ec62ec.debug"
  dist = "riscv64-unknown-fuchsia/libc++.so.2"
  name = "libc++"
  soname = "libc++.so.2"
}, {
  debug = "debug/.build-id/17/d39e1846930235f9435a91d2bb42d68ab7d065.debug"
  dist = "riscv64-unknown-fuchsia/libc++abi.so.1"
  name = "libc++abi"
  soname = "libc++abi.so.1"
}, {
  debug = "debug/.build-id/d7/c113830a04ad2499a2e2c909c507da2a1682df.debug"
  dist = "riscv64-unknown-fuchsia/libunwind.so.1"
  name = "libunwind"
  soname = "libunwind.so.1"
}]
  target = ["riscv64-unknown-fuchsia", "riscv64-fuchsia"]
}, {
  cflags = ["-fsanitize=hwaddress"]
  ldflags = ["-static-libstdc++"]
  runtime = [{
  debug = "debug/.build-id/6e/4bce0bc423606edc38a8e924d4b83d750cb0c6.debug"
  dist = "clang/24/lib/riscv64-unknown-fuchsia/libclang_rt.hwasan.so"
  soname = "libclang_rt.hwasan.so"
}, {
  debug = "debug/.build-id/62/fe6e43cb7f262d190386744cd0423c72ec62ec.debug"
  dist = "riscv64-unknown-fuchsia/hwasan/libc++.so.2"
  name = "libc++"
  soname = "libc++.so.2"
}, {
  debug = "debug/.build-id/17/d39e1846930235f9435a91d2bb42d68ab7d065.debug"
  dist = "riscv64-unknown-fuchsia/hwasan/libc++abi.so.1"
  name = "libc++abi"
  soname = "libc++abi.so.1"
}, {
  debug = "debug/.build-id/21/378a2e3087b5822ce113874d8899129d4cabc5.debug"
  dist = "riscv64-unknown-fuchsia/hwasan/libunwind.so.1"
  name = "libunwind"
  soname = "libunwind.so.1"
}]
  target = ["riscv64-unknown-fuchsia", "riscv64-fuchsia"]
}, {
  cflags = []
  ldflags = []
  runtime = [{
  breakpad = "debug/.build-id/3c/f40c832e6d227141915876bef158925a6ee3b4.sym"
  debug = "debug/.build-id/3c/f40c832e6d227141915876bef158925a6ee3b4.debug"
  dist = "x86_64-unknown-fuchsia/libc++.so.2"
  name = "libc++"
  soname = "libc++.so.2"
}, {
  breakpad = "debug/.build-id/d5/ba91065ab072948f0ccb652abaf042655b304c.sym"
  debug = "debug/.build-id/d5/ba91065ab072948f0ccb652abaf042655b304c.debug"
  dist = "x86_64-unknown-fuchsia/libc++abi.so.1"
  name = "libc++abi"
  soname = "libc++abi.so.1"
}, {
  breakpad = "debug/.build-id/4f/353112985ec520fc3106e962a6e4f20d143a9e.sym"
  debug = "debug/.build-id/4f/353112985ec520fc3106e962a6e4f20d143a9e.debug"
  dist = "x86_64-unknown-fuchsia/libunwind.so.1"
  name = "libunwind"
  soname = "libunwind.so.1"
}]
  target = ["x86_64-unknown-fuchsia", "x86_64-fuchsia"]
}, {
  cflags = ["-fsanitize=address"]
  ldflags = []
  runtime = [{
  breakpad = "debug/.build-id/02/59d96b279c9ae7398f32bd7a35210076ee62ff.sym"
  debug = "debug/.build-id/02/59d96b279c9ae7398f32bd7a35210076ee62ff.debug"
  dist = "clang/24/lib/x86_64-unknown-fuchsia/libclang_rt.asan.so"
  soname = "libclang_rt.asan.so"
}, {
  breakpad = "debug/.build-id/1b/9708d13b6cd4701ca51d705a078e475f2db514.sym"
  debug = "debug/.build-id/1b/9708d13b6cd4701ca51d705a078e475f2db514.debug"
  dist = "x86_64-unknown-fuchsia/asan/libc++.so.2"
  name = "libc++"
  soname = "libc++.so.2"
}, {
  breakpad = "debug/.build-id/38/5b157a9d216187038d95d9701a064ac17f61a4.sym"
  debug = "debug/.build-id/38/5b157a9d216187038d95d9701a064ac17f61a4.debug"
  dist = "x86_64-unknown-fuchsia/asan/libc++abi.so.1"
  name = "libc++abi"
  soname = "libc++abi.so.1"
}, {
  breakpad = "debug/.build-id/3e/47bdfd7a49549b1bcf3bc319996ac32ff16f22.sym"
  debug = "debug/.build-id/3e/47bdfd7a49549b1bcf3bc319996ac32ff16f22.debug"
  dist = "x86_64-unknown-fuchsia/asan/libunwind.so.1"
  name = "libunwind"
  soname = "libunwind.so.1"
}]
  target = ["x86_64-unknown-fuchsia", "x86_64-fuchsia"]
}, {
  cflags = ["-fsanitize=undefined"]
  ldflags = []
  runtime = [{
  breakpad = "debug/.build-id/82/448ab83b0c742640542ba810213f9aeda8a803.sym"
  debug = "debug/.build-id/82/448ab83b0c742640542ba810213f9aeda8a803.debug"
  dist = "clang/24/lib/x86_64-unknown-fuchsia/libclang_rt.ubsan_standalone.so"
  soname = "libclang_rt.ubsan_standalone.so"
}, {
  breakpad = "debug/.build-id/3c/f40c832e6d227141915876bef158925a6ee3b4.sym"
  debug = "debug/.build-id/3c/f40c832e6d227141915876bef158925a6ee3b4.debug"
  dist = "x86_64-unknown-fuchsia/libc++.so.2"
  name = "libc++"
  soname = "libc++.so.2"
}, {
  breakpad = "debug/.build-id/d5/ba91065ab072948f0ccb652abaf042655b304c.sym"
  debug = "debug/.build-id/d5/ba91065ab072948f0ccb652abaf042655b304c.debug"
  dist = "x86_64-unknown-fuchsia/libc++abi.so.1"
  name = "libc++abi"
  soname = "libc++abi.so.1"
}, {
  breakpad = "debug/.build-id/4f/353112985ec520fc3106e962a6e4f20d143a9e.sym"
  debug = "debug/.build-id/4f/353112985ec520fc3106e962a6e4f20d143a9e.debug"
  dist = "x86_64-unknown-fuchsia/libunwind.so.1"
  name = "libunwind"
  soname = "libunwind.so.1"
}]
  target = ["x86_64-unknown-fuchsia", "x86_64-fuchsia"]
}, {
  cflags = []
  ldflags = ["-static-libstdc++"]
  runtime = []
  target = ["x86_64-unknown-fuchsia", "x86_64-fuchsia"]
}, {
  cflags = ["-fsanitize=address"]
  ldflags = ["-static-libstdc++"]
  runtime = [{
  breakpad = "debug/.build-id/02/59d96b279c9ae7398f32bd7a35210076ee62ff.sym"
  debug = "debug/.build-id/02/59d96b279c9ae7398f32bd7a35210076ee62ff.debug"
  dist = "clang/24/lib/x86_64-unknown-fuchsia/libclang_rt.asan.so"
  soname = "libclang_rt.asan.so"
}, {
  breakpad = "debug/.build-id/1b/9708d13b6cd4701ca51d705a078e475f2db514.sym"
  debug = "debug/.build-id/1b/9708d13b6cd4701ca51d705a078e475f2db514.debug"
  dist = "x86_64-unknown-fuchsia/asan/libc++.so.2"
  name = "libc++"
  soname = "libc++.so.2"
}, {
  breakpad = "debug/.build-id/38/5b157a9d216187038d95d9701a064ac17f61a4.sym"
  debug = "debug/.build-id/38/5b157a9d216187038d95d9701a064ac17f61a4.debug"
  dist = "x86_64-unknown-fuchsia/asan/libc++abi.so.1"
  name = "libc++abi"
  soname = "libc++abi.so.1"
}, {
  breakpad = "debug/.build-id/3e/47bdfd7a49549b1bcf3bc319996ac32ff16f22.sym"
  debug = "debug/.build-id/3e/47bdfd7a49549b1bcf3bc319996ac32ff16f22.debug"
  dist = "x86_64-unknown-fuchsia/asan/libunwind.so.1"
  name = "libunwind"
  soname = "libunwind.so.1"
}]
  target = ["x86_64-unknown-fuchsia", "x86_64-fuchsia"]
}, {
  cflags = ["-fsanitize=undefined"]
  ldflags = ["-static-libstdc++"]
  runtime = [{
  breakpad = "debug/.build-id/82/448ab83b0c742640542ba810213f9aeda8a803.sym"
  debug = "debug/.build-id/82/448ab83b0c742640542ba810213f9aeda8a803.debug"
  dist = "clang/24/lib/x86_64-unknown-fuchsia/libclang_rt.ubsan_standalone.so"
  soname = "libclang_rt.ubsan_standalone.so"
}, {
  breakpad = "debug/.build-id/3c/f40c832e6d227141915876bef158925a6ee3b4.sym"
  debug = "debug/.build-id/3c/f40c832e6d227141915876bef158925a6ee3b4.debug"
  dist = "x86_64-unknown-fuchsia/libc++.so.2"
  name = "libc++"
  soname = "libc++.so.2"
}, {
  breakpad = "debug/.build-id/d5/ba91065ab072948f0ccb652abaf042655b304c.sym"
  debug = "debug/.build-id/d5/ba91065ab072948f0ccb652abaf042655b304c.debug"
  dist = "x86_64-unknown-fuchsia/libc++abi.so.1"
  name = "libc++abi"
  soname = "libc++abi.so.1"
}, {
  breakpad = "debug/.build-id/4f/353112985ec520fc3106e962a6e4f20d143a9e.sym"
  debug = "debug/.build-id/4f/353112985ec520fc3106e962a6e4f20d143a9e.debug"
  dist = "x86_64-unknown-fuchsia/libunwind.so.1"
  name = "libunwind"
  soname = "libunwind.so.1"
}]
  target = ["x86_64-unknown-fuchsia", "x86_64-fuchsia"]
}]
  x86_64_pc_windows_msvc = {
  libclang_rt_profile_a = "../../../../out/not-default/libclang_rt.profile.lib"
  libunwind_so = "../../../../out/not-default/libunwind.dll"
  resource_dir = "lib/clang/24"
  variants = { }
}
  x86_64_unknown_fuchsia = {
  libclang_rt_profile_a = "lib/clang/24/lib/x86_64-unknown-fuchsia/libclang_rt.profile.a"
  libunwind_so = "lib/x86_64-unknown-fuchsia/libunwind.so.1.0"
  resource_dir = "lib/clang/24"
  variants = {
  asan = {
  shared = {
  clang_rt = "lib/clang/24/lib/x86_64-unknown-fuchsia/libclang_rt.asan.so"
}
  static = {
  clang_rt = "lib/clang/24/lib/x86_64-unknown-fuchsia/libclang_rt.asan.a"
  clang_rt_cxx = "lib/clang/24/lib/x86_64-unknown-fuchsia/libclang_rt.asan_cxx.a"
}
}
  hwasan = {
  shared = {
  clang_rt = "lib/clang/24/lib/x86_64-unknown-fuchsia/libclang_rt.hwasan.so"
}
  static = {
  clang_rt = "lib/clang/24/lib/x86_64-unknown-fuchsia/libclang_rt.hwasan.a"
  clang_rt_cxx = "lib/clang/24/lib/x86_64-unknown-fuchsia/libclang_rt.hwasan_cxx.a"
}
}
  lsan = {
  shared = {
  clang_rt = "../../../../out/not-default/libclang_rt.lsan.so"
}
  static = {
  clang_rt = "lib/clang/24/lib/x86_64-unknown-fuchsia/libclang_rt.lsan.a"
  clang_rt_cxx = "../../../../out/not-default/libclang_rt.lsan_cxx.a"
}
}
  tsan = {
  shared = {
  clang_rt = "../../../../out/not-default/libclang_rt.tsan.so"
}
  static = {
  clang_rt = "../../../../out/not-default/libclang_rt.tsan.a"
  clang_rt_cxx = "../../../../out/not-default/libclang_rt.tsan_cxx.a"
}
}
  ubsan = {
  shared = {
  clang_rt = "lib/clang/24/lib/x86_64-unknown-fuchsia/libclang_rt.ubsan_standalone.so"
}
  static = {
  clang_rt = "lib/clang/24/lib/x86_64-unknown-fuchsia/libclang_rt.ubsan_standalone.a"
  clang_rt_cxx = "lib/clang/24/lib/x86_64-unknown-fuchsia/libclang_rt.ubsan_standalone_cxx.a"
}
}
}
}
  x86_64_unknown_linux_gnu = {
  libclang_rt_profile_a = "lib/clang/24/lib/x86_64-unknown-linux-gnu/libclang_rt.profile.a"
  libunwind_so = "../../../../out/not-default/libunwind.so"
  resource_dir = "lib/clang/24"
  variants = {
  asan = {
  shared = {
  clang_rt = "lib/clang/24/lib/x86_64-unknown-linux-gnu/libclang_rt.asan.so"
}
  static = {
  clang_rt = "lib/clang/24/lib/x86_64-unknown-linux-gnu/libclang_rt.asan.a"
  clang_rt_cxx = "lib/clang/24/lib/x86_64-unknown-linux-gnu/libclang_rt.asan_cxx.a"
}
}
  hwasan = {
  shared = {
  clang_rt = "lib/clang/24/lib/x86_64-unknown-linux-gnu/libclang_rt.hwasan.so"
}
  static = {
  clang_rt = "lib/clang/24/lib/x86_64-unknown-linux-gnu/libclang_rt.hwasan.a"
  clang_rt_cxx = "lib/clang/24/lib/x86_64-unknown-linux-gnu/libclang_rt.hwasan_cxx.a"
}
}
  lsan = {
  shared = {
  clang_rt = "../../../../out/not-default/libclang_rt.lsan.so"
}
  static = {
  clang_rt = "lib/clang/24/lib/x86_64-unknown-linux-gnu/libclang_rt.lsan.a"
  clang_rt_cxx = "../../../../out/not-default/libclang_rt.lsan_cxx.a"
}
}
  tsan = {
  shared = {
  clang_rt = "lib/clang/24/lib/x86_64-unknown-linux-gnu/libclang_rt.tsan.so"
}
  static = {
  clang_rt = "lib/clang/24/lib/x86_64-unknown-linux-gnu/libclang_rt.tsan.a"
  clang_rt_cxx = "lib/clang/24/lib/x86_64-unknown-linux-gnu/libclang_rt.tsan_cxx.a"
}
}
  ubsan = {
  shared = {
  clang_rt = "lib/clang/24/lib/x86_64-unknown-linux-gnu/libclang_rt.ubsan_standalone.so"
}
  static = {
  clang_rt = "lib/clang/24/lib/x86_64-unknown-linux-gnu/libclang_rt.ubsan_standalone.a"
  clang_rt_cxx = "lib/clang/24/lib/x86_64-unknown-linux-gnu/libclang_rt.ubsan_standalone_cxx.a"
}
}
}
}
}
```

From //build/config/clang/clang_toolchain_info.gni:42

### clippy_cause_failure

Makes clippy targets fail to build when any "deny" lints are found

**Current value (from the default):** `true`

From //build/rust/config.gni:78

### clippy_force_warn_all

Force the lint level for all clippy lints to "warn".
Note: this overrides both source attributes and our default lint levels, and
should only be used to collect stats about clippy lints in our source tree.

**Current value (from the default):** `false`

From //build/rust/config.gni:75

### clippy_ignore_rustc

By default building clippy targets prints lints as well as any rustc
diagnostics that were emitted by check-building the crate. This flag makes
the output in the build only contain lints to avoid duplicating the
diagnostics from rustc (which will be emitted when the actual target is
built). Note that `fx clippy` will still emit rustc diagnostics alongside
clippy lints, they just wont show up in ninja's stderr

**Current value (from the default):** `false`

From //build/rust/config.gni:95

### clippy_warn_all

Set the lint level for all clippy lints to "warn".
Note: setting lint levels in source takes precedence over this.

**Current value (from the default):** `false`

From //build/rust/config.gni:70

### cobalt_environment

Selects the Cobalt environment to send data to. Choices:
  "LOCAL" - record log data locally to a file
  "DEVEL" - the non-prod environment for use in testing
  "PROD" - the production environment
  false - use the default environment supplied by assembly

**Current value (from the default):** `false`

From //src/cobalt/bin/app/BUILD.gn:16

### comparison_diagnostics_dir

When any of the {Rust,C++} {determinism,consistency} checks fail,
copy the artifacts' difference-pairs to this directory for exporting
from infra builds, and later inspection.

**Current value (from the default):** `"//out/not-default/comparison-reports"`

From //build/toolchain/rbe.gni:229

### compilation_mode

The overall compilation mode to use.  The valid values are:
 * `debug`: for debug-enabled builds.
 * `balanced`: some optimizations, but prioritizing compilation speed over
                runtime performance.
 * `release`: all the optimizations, used for product releases.
LINT.IfChange

**Current value for `target_cpu = "arm64"`:** `"release"`

From //out/not-default/args.gn:5

**Overridden from the default:** `"balanced"`

From //build/config/compilation_modes.gni:19

**Current value for `target_cpu = "riscv64"`:** `"release"`

From //out/not-default/args.gn:5

**Overridden from the default:** `"balanced"`

From //build/config/compilation_modes.gni:19

**Current value for `target_cpu = "x64"`:** `"release"`

From //out/not-default/args.gn:5

**Overridden from the default:** `"balanced"`

From //build/config/compilation_modes.gni:19

### compilation_settings_overrides

Overridden settings for the compilation mode.  This is a set of override
values for variables whose default values are set by the chosen compilation
mode (above).
  * optimize:  The optimization mode to use.  Valid values are:
      * `none`: really unoptimized, usually only build-tested and not run
      * `debug`: "optimized for debugging", light enough to avoid confusion
      * `moderate`: moderate optimization level (clang's default -O2)
      * `size`:  optimized for space rather than purely for speed
      * `size_thinlto`:  optimize for space and use Thin LTO
      * `size_lto`:  optimize for space and use LTO
      * `speed`: optimized purely for speed
      * `sanitizer`: optimized for sanitizers (ASan, etc.)
      * `profile`: optimized for coverage/profile data collection
      * `coverage`: optimized for coverage data collection


**Current value (from the default):** `{ }`

From //build/config/compilation_modes.gni:38

### compress_debuginfo

Enable compression of debug sections.

**Current value (from the default):** `"zstd"`

From //build/config/compiler.gni:123

### config_example_cpp_greeting

Set this in args.gn to override the greeting emitted by this example.

**Current value (from the default):** `"World"`

From //examples/components/config/cpp/BUILD.gn:10

### config_example_rust_greeting

Set this in args.gn to override the greeting emitted by this example.

**Current value (from the default):** `"World"`

From //examples/components/config/rust/BUILD.gn:11

### config_have_heap

Tells openweave to include files that require heap access.

**Current value (from the default):** `true`

From //third_party/openweave-core/config.gni:32

### console_enable_history

Controls whether the Zircon kernel console compiles in command history
storage and shell navigation features. Defaults to true, but can be disabled
on a per-build basis.

**Current value (from the default):** `true`

From //zircon/kernel/lib/console/config.gni:11

### crash_diagnostics_dir

Clang crash reports directory path. Use empty path to disable altogether.

**Current value (from the default):** `"//out/not-default/clang-crashreports"`

From //build/config/clang/crash_diagnostics.gni:7

### crashpad_dependencies

**Current value (from the default):** `"fuchsia"`

From //third_party/crashpad/src/build/crashpad_buildconfig.gni:22

### crashpad_http_transport_impl

**Current value (from the default):** `"socket"`

From //third_party/crashpad/src/util/net/tls.gni:19

### crashpad_use_boringssl_for_http_transport_socket

**Current value (from the default):** `true`

From //third_party/crashpad/src/util/net/tls.gni:30

### ctf_api_level

**Current value (from the default):** `"NEXT"`

From //sdk/ctf/build/ctf_api_level.gni:6

### ctf_output_directory

**Current value (from the default):** `"frozen-ctf-artifacts-subbuild"`

From //sdk/ctf/build/ctf_output_directory.gni:6

### current_build_target_api_level

The target API level of the current build or sub-build.

The default is "PLATFORM", which corresponds to the main platform build.

Other valid values are "NEXT", "HEAD", or a string that is the decimal
representation of a positive integer corresponding to a currently Supported
(not Sunset) API level.

It should only be set to values other than "PLATFORM" by IDK sub-builds,
which configure a narrow GN graph. See //sdk/working_with_subbuilds.md.
Targets outside that graph may fail when it is set to a non-default value.

The arg is used by templates that generate artifacts for the IDK. Other
uses, including by individual target definitions, are discouraged, and IDK
atom target definitions MUST NOT use this GN arg. See
https://fxbug.dev/448185526 for details.

**Current value (from the default):** `"PLATFORM"`

From //build/config/fuchsia/target_api_level.gni:22

### current_cpu

**Current value (from the default):** `""`

### current_os

**Current value (from the default):** `""`

### custom_vulkan_loader_library_name

**Current value (from the default):** `""`

From //third_party/Vulkan-Loader/src/BUILD.gn:20

### cxx_rbe_check

Run one of the more expensive checks, intended for CI.
All of these require cxx_rbe_enable=true.

One of:

  * "none": No additional check.

  * "determinism":
      Check of determinism of C++ targets by running locally twice
      and comparing outputs, failing if any differences are found.
      Even though this check doesn't involve RBE, it uses the same
      wrapper script, which knows what output files to expect and
      compare.

      Build outputs that depend on time are discouraged because they
      impact caching.  Known bad preprocessing macros include
      __DATE__ and __TIME__.

  * "consistency":
      Check consistency between local and remote C++ compiles,
      by running both and comparing results.


**Current value (from the default):** `"none"`

From //build/toolchain/rbe.gni:214

### cxx_rbe_download_obj_files

Controls whether or not to download intermediate .o files.
When downloading is disabled, the build produces stubs
that be used to retrieve remote artifacts later using build/rbe/dlwrap.py.
TODO(b/284994230): This option is only available to developers,
and not restricted environments that lack direct network access.

**Current value (from the default):** `true`

From //build/toolchain/rbe.gni:236

### cxx_rbe_enable

Set to true to enable distributed compilation of C++ using RBE.
Remote execution offers increased build parallelism and caching.

**Current value (from the default):** `false`

From //build/toolchain/rbe.gni:164

### cxx_rbe_exec_strategy

One of:

  * "remote": Execute action remotely on cache miss.
        The remote cache is always updated with this result.

  * "local": Lookup action in the remote cache, but execute action
        locally on cache miss.  The locally produced result is
        not uploaded to the remote cache.

  * "remote_local_fallback": Execute action remotely first.
        If that fails, run locally instead.  The locally produced
        results are not uploaded to the remote cache.

  * "racing": Race local vs. remote execution, take the first to finish.

  * "nocache": Force remote execution without using cached results.
        This can be useful for benchmarking cache-miss scenarios.

  (There are other rewrapper options that are not exposed.)

**Current value (from the default):** `"remote_local_fallback"`

From //build/toolchain/rbe.gni:190

### cxx_rbe_full_toolchain

reclient owns the logic for deciding what inputs are needed for
remote compilation, but in some cases, it may fall behind
upstream toolchain development.
This option forces the *entire* toolchain directory to be included
as an input, which is generally guaranteed to work as it bears
no assumptions about how the toolchain works, but it comes at the
cost of performance overhead.
Use this primarily for debugging and as an emergency workaround.

**Current value (from the default):** `false`

From //build/toolchain/rbe.gni:224

### cxx_rbe_minimalist_wrapper

Set to true to use a fast, minimalist wrapper, that lacks features
of the python-based wrapper, and is close to a bare call to rewrapper.
This flag is only meaningful when `cxx_rbe_enable` is true.

**Current value (from the default):** `false`

From //build/toolchain/rbe.gni:169

### data_filesystem_format

Set to one of "minfs", "fxfs", "f2fs".
If set to anything other than "minfs", any existing minfs partition will be
migrated in-place to the specified format when fshost mounts it.

**Current value (from the default):** `"fxfs"`

From //src/storage/fshost/generated_fshost_config.gni:12

### debuginfo

* `none` means no debugging information
* `backtrace` means sufficient debugging information to symbolize backtraces
* `debug` means debugging information suited for debugging

**Current value (from the default):** `"debug"`

From //build/config/compiler.gni:88

### default_configs

**Current value (from the default):** `[]`

From //third_party/pigweed/src/pw_build/gn_internal/defaults.gni:34

### default_public_deps

**Current value (from the default):** `[]`

From //third_party/pigweed/src/pw_build/gn_internal/defaults.gni:35

### delivery_blob_type

Controls what type of delivery blob pkg-resolver fetches and blobfs accepts.
Supported types can be found in //src/storage/blobfs/delivery_blob.h
Valid values are integers, for example: 1
This arg is for local developer only, products should not set this arg.

**Current value (from the default):** `1`

From //build/images/args.gni:41

### deny_warnings

Controls whether to promote warnings to errors.

**Current value (from the default):** `true`

From //build/config/BUILD.gn:26

### developer_test_labels

A developer-only argument that is used to add tests to the build without
going through the test-type validate that the above sets of tests are.
These are always a dependency of the main product assembly.

**Current value (from the default):** `[]`

From //BUILD.gn:83

### dir_docker

**Current value (from the default):** `"//third_party/pigweed/src/docker"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:30

### dir_pigweed

Location of the Pigweed repository.

**Current value (from the default):** `"//third_party/pigweed/src"`

From //build_overrides/pigweed.gni:11

### dir_pw_alignment

**Current value (from the default):** `"//third_party/pigweed/src/pw_alignment"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:31

### dir_pw_allocator

**Current value (from the default):** `"//third_party/pigweed/src/pw_allocator"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:32

### dir_pw_allocator_zephyr

**Current value (from the default):** `"//third_party/pigweed/src/pw_allocator_zephyr"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:33

### dir_pw_analog

**Current value (from the default):** `"//third_party/pigweed/src/pw_analog"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:34

### dir_pw_android_toolchain

**Current value (from the default):** `"//third_party/pigweed/src/pw_android_toolchain"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:35

### dir_pw_arduino_build

**Current value (from the default):** `"//third_party/pigweed/src/pw_arduino_build"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:36

### dir_pw_assert

**Current value (from the default):** `"//third_party/pigweed/src/pw_assert"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:37

### dir_pw_assert_basic

**Current value (from the default):** `"//third_party/pigweed/src/pw_assert_basic"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:38

### dir_pw_assert_fuchsia

**Current value (from the default):** `"//third_party/pigweed/src/pw_assert_fuchsia"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:39

### dir_pw_assert_log

**Current value (from the default):** `"//third_party/pigweed/src/pw_assert_log"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:40

### dir_pw_assert_tokenized

**Current value (from the default):** `"//third_party/pigweed/src/pw_assert_tokenized"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:41

### dir_pw_assert_trap

**Current value (from the default):** `"//third_party/pigweed/src/pw_assert_trap"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:42

### dir_pw_assert_zephyr

**Current value (from the default):** `"//third_party/pigweed/src/pw_assert_zephyr"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:43

### dir_pw_async

**Current value (from the default):** `"//third_party/pigweed/src/pw_async"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:44

### dir_pw_async2

**Current value (from the default):** `"//third_party/pigweed/src/pw_async2"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:45

### dir_pw_async_basic

**Current value (from the default):** `"//third_party/pigweed/src/pw_async_basic"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:46

### dir_pw_async_fuchsia

**Current value (from the default):** `"//third_party/pigweed/src/pw_async_fuchsia"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:47

### dir_pw_atomic

**Current value (from the default):** `"//third_party/pigweed/src/pw_atomic"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:48

### dir_pw_base64

**Current value (from the default):** `"//third_party/pigweed/src/pw_base64"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:49

### dir_pw_bloat

**Current value (from the default):** `"//third_party/pigweed/src/pw_bloat"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:50

### dir_pw_blob_store

**Current value (from the default):** `"//third_party/pigweed/src/pw_blob_store"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:51

### dir_pw_bluetooth

**Current value (from the default):** `"//third_party/pigweed/src/pw_bluetooth"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:52

### dir_pw_bluetooth_hci

**Current value (from the default):** `"//third_party/pigweed/src/pw_bluetooth_hci"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:53

### dir_pw_bluetooth_profiles

**Current value (from the default):** `"//third_party/pigweed/src/pw_bluetooth_profiles"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:55

### dir_pw_bluetooth_proxy

**Current value (from the default):** `"//third_party/pigweed/src/pw_bluetooth_proxy"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:56

### dir_pw_bluetooth_sapphire

**Current value (from the default):** `"//third_party/pigweed/src/pw_bluetooth_sapphire"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:58

### dir_pw_boot

**Current value (from the default):** `"//third_party/pigweed/src/pw_boot"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:59

### dir_pw_boot_cortex_m

**Current value (from the default):** `"//third_party/pigweed/src/pw_boot_cortex_m"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:60

### dir_pw_buf

**Current value (from the default):** `"//third_party/pigweed/src/pw_buf"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:61

### dir_pw_build

**Current value (from the default):** `"//third_party/pigweed/src/pw_build"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:62

### dir_pw_build_android

**Current value (from the default):** `"//third_party/pigweed/src/pw_build_android"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:63

### dir_pw_build_info

**Current value (from the default):** `"//third_party/pigweed/src/pw_build_info"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:64

### dir_pw_build_mcuxpresso

**Current value (from the default):** `"//third_party/pigweed/src/pw_build_mcuxpresso"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:65

### dir_pw_bytes

**Current value (from the default):** `"//third_party/pigweed/src/pw_bytes"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:66

### dir_pw_change

**Current value (from the default):** `"//third_party/pigweed/src/pw_change"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:67

### dir_pw_channel

**Current value (from the default):** `"//third_party/pigweed/src/pw_channel"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:68

### dir_pw_checksum

**Current value (from the default):** `"//third_party/pigweed/src/pw_checksum"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:69

### dir_pw_chre

**Current value (from the default):** `"//third_party/pigweed/src/pw_chre"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:70

### dir_pw_chrono

**Current value (from the default):** `"//third_party/pigweed/src/pw_chrono"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:71

### dir_pw_chrono_embos

**Current value (from the default):** `"//third_party/pigweed/src/pw_chrono_embos"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:72

### dir_pw_chrono_freertos

**Current value (from the default):** `"//third_party/pigweed/src/pw_chrono_freertos"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:73

### dir_pw_chrono_rp2040

**Current value (from the default):** `"//third_party/pigweed/src/pw_chrono_rp2040"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:74

### dir_pw_chrono_stl

**Current value (from the default):** `"//third_party/pigweed/src/pw_chrono_stl"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:75

### dir_pw_chrono_threadx

**Current value (from the default):** `"//third_party/pigweed/src/pw_chrono_threadx"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:76

### dir_pw_chrono_zephyr

**Current value (from the default):** `"//third_party/pigweed/src/pw_chrono_zephyr"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:77

### dir_pw_cli

**Current value (from the default):** `"//third_party/pigweed/src/pw_cli"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:78

### dir_pw_cli_analytics

**Current value (from the default):** `"//third_party/pigweed/src/pw_cli_analytics"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:79

### dir_pw_clock_tree

**Current value (from the default):** `"//third_party/pigweed/src/pw_clock_tree"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:80

### dir_pw_clock_tree_mcuxpresso

**Current value (from the default):** `"//third_party/pigweed/src/pw_clock_tree_mcuxpresso"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:82

### dir_pw_compilation_testing

**Current value (from the default):** `"//third_party/pigweed/src/pw_compilation_testing"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:84

### dir_pw_config_loader

**Current value (from the default):** `"//third_party/pigweed/src/pw_config_loader"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:85

### dir_pw_console

**Current value (from the default):** `"//third_party/pigweed/src/pw_console"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:86

### dir_pw_containers

**Current value (from the default):** `"//third_party/pigweed/src/pw_containers"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:87

### dir_pw_cpu_exception

**Current value (from the default):** `"//third_party/pigweed/src/pw_cpu_exception"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:88

### dir_pw_cpu_exception_cortex_m

**Current value (from the default):** `"//third_party/pigweed/src/pw_cpu_exception_cortex_m"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:90

### dir_pw_cpu_exception_risc_v

**Current value (from the default):** `"//third_party/pigweed/src/pw_cpu_exception_risc_v"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:92

### dir_pw_crypto

**Current value (from the default):** `"//third_party/pigweed/src/pw_crypto"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:93

### dir_pw_digital_io

**Current value (from the default):** `"//third_party/pigweed/src/pw_digital_io"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:94

### dir_pw_digital_io_linux

**Current value (from the default):** `"//third_party/pigweed/src/pw_digital_io_linux"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:95

### dir_pw_digital_io_mcuxpresso

**Current value (from the default):** `"//third_party/pigweed/src/pw_digital_io_mcuxpresso"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:97

### dir_pw_digital_io_rp2040

**Current value (from the default):** `"//third_party/pigweed/src/pw_digital_io_rp2040"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:98

### dir_pw_digital_io_zephyr

**Current value (from the default):** `"//third_party/pigweed/src/pw_digital_io_zephyr"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:99

### dir_pw_display

**Current value (from the default):** `"//third_party/pigweed/src/pw_display"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:100

### dir_pw_dma_mcuxpresso

**Current value (from the default):** `"//third_party/pigweed/src/pw_dma_mcuxpresso"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:101

### dir_pw_docgen

**Current value (from the default):** `"//third_party/pigweed/src/pw_docgen"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:102

### dir_pw_doctor

**Current value (from the default):** `"//third_party/pigweed/src/pw_doctor"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:103

### dir_pw_elf

**Current value (from the default):** `"//third_party/pigweed/src/pw_elf"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:104

### dir_pw_emu

**Current value (from the default):** `"//third_party/pigweed/src/pw_emu"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:105

### dir_pw_enum

**Current value (from the default):** `"//third_party/pigweed/src/pw_enum"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:106

### dir_pw_env_setup

**Current value (from the default):** `"//third_party/pigweed/src/pw_env_setup"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:107

### dir_pw_env_setup_zephyr

**Current value (from the default):** `"//third_party/pigweed/src/pw_env_setup_zephyr"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:108

### dir_pw_file

**Current value (from the default):** `"//third_party/pigweed/src/pw_file"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:109

### dir_pw_flatbuffers

**Current value (from the default):** `"//third_party/pigweed/src/pw_flatbuffers"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:110

### dir_pw_format

**Current value (from the default):** `"//third_party/pigweed/src/pw_format"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:111

### dir_pw_fortifier

**Current value (from the default):** `"//third_party/pigweed/src/pw_fortifier"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:112

### dir_pw_function

**Current value (from the default):** `"//third_party/pigweed/src/pw_function"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:113

### dir_pw_fuzzer

**Current value (from the default):** `"//third_party/pigweed/src/pw_fuzzer"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:114

### dir_pw_grpc

**Current value (from the default):** `"//third_party/pigweed/src/pw_grpc"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:115

### dir_pw_hdlc

**Current value (from the default):** `"//third_party/pigweed/src/pw_hdlc"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:116

### dir_pw_hex_dump

**Current value (from the default):** `"//third_party/pigweed/src/pw_hex_dump"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:117

### dir_pw_i2c

**Current value (from the default):** `"//third_party/pigweed/src/pw_i2c"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:118

### dir_pw_i2c_linux

**Current value (from the default):** `"//third_party/pigweed/src/pw_i2c_linux"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:119

### dir_pw_i2c_mcuxpresso

**Current value (from the default):** `"//third_party/pigweed/src/pw_i2c_mcuxpresso"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:120

### dir_pw_i2c_rp2040

**Current value (from the default):** `"//third_party/pigweed/src/pw_i2c_rp2040"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:121

### dir_pw_i2c_zephyr

**Current value (from the default):** `"//third_party/pigweed/src/pw_i2c_zephyr"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:122

### dir_pw_ide

**Current value (from the default):** `"//third_party/pigweed/src/pw_ide"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:123

### dir_pw_interrupt

**Current value (from the default):** `"//third_party/pigweed/src/pw_interrupt"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:124

### dir_pw_interrupt_cortex_m

**Current value (from the default):** `"//third_party/pigweed/src/pw_interrupt_cortex_m"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:126

### dir_pw_interrupt_freertos

**Current value (from the default):** `"//third_party/pigweed/src/pw_interrupt_freertos"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:128

### dir_pw_interrupt_zephyr

**Current value (from the default):** `"//third_party/pigweed/src/pw_interrupt_zephyr"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:129

### dir_pw_intrusive_ptr

**Current value (from the default):** `"//third_party/pigweed/src/pw_intrusive_ptr"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:130

### dir_pw_json

**Current value (from the default):** `"//third_party/pigweed/src/pw_json"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:131

### dir_pw_kernel

**Current value (from the default):** `"//third_party/pigweed/src/pw_kernel"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:132

### dir_pw_kvs

**Current value (from the default):** `"//third_party/pigweed/src/pw_kvs"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:133

### dir_pw_libc

**Current value (from the default):** `"//third_party/pigweed/src/pw_libc"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:134

### dir_pw_libcxx

**Current value (from the default):** `"//third_party/pigweed/src/pw_libcxx"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:135

### dir_pw_log

**Current value (from the default):** `"//third_party/pigweed/src/pw_log"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:136

### dir_pw_log_android

**Current value (from the default):** `"//third_party/pigweed/src/pw_log_android"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:137

### dir_pw_log_basic

**Current value (from the default):** `"//third_party/pigweed/src/pw_log_basic"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:138

### dir_pw_log_fuchsia

**Current value (from the default):** `"//third_party/pigweed/src/pw_log_fuchsia"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:139

### dir_pw_log_null

**Current value (from the default):** `"//third_party/pigweed/src/pw_log_null"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:140

### dir_pw_log_rpc

**Current value (from the default):** `"//third_party/pigweed/src/pw_log_rpc"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:141

### dir_pw_log_string

**Current value (from the default):** `"//third_party/pigweed/src/pw_log_string"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:142

### dir_pw_log_tokenized

**Current value (from the default):** `"//third_party/pigweed/src/pw_log_tokenized"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:143

### dir_pw_log_zephyr

**Current value (from the default):** `"//third_party/pigweed/src/pw_log_zephyr"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:144

### dir_pw_malloc

**Current value (from the default):** `"//third_party/pigweed/src/pw_malloc"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:145

### dir_pw_malloc_freelist

**Current value (from the default):** `"//third_party/pigweed/src/pw_malloc_freelist"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:146

### dir_pw_malloc_freertos

**Current value (from the default):** `"//third_party/pigweed/src/pw_malloc_freertos"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:147

### dir_pw_memory

**Current value (from the default):** `"//third_party/pigweed/src/pw_memory"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:148

### dir_pw_metric

**Current value (from the default):** `"//third_party/pigweed/src/pw_metric"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:149

### dir_pw_module

**Current value (from the default):** `"//third_party/pigweed/src/pw_module"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:150

### dir_pw_multibuf

**Current value (from the default):** `"//third_party/pigweed/src/pw_multibuf"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:151

### dir_pw_multisink

**Current value (from the default):** `"//third_party/pigweed/src/pw_multisink"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:152

### dir_pw_numeric

**Current value (from the default):** `"//third_party/pigweed/src/pw_numeric"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:153

### dir_pw_package

**Current value (from the default):** `"//third_party/pigweed/src/pw_package"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:154

### dir_pw_perf_test

**Current value (from the default):** `"//third_party/pigweed/src/pw_perf_test"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:155

### dir_pw_persistent_ram

**Current value (from the default):** `"//third_party/pigweed/src/pw_persistent_ram"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:156

### dir_pw_polyfill

**Current value (from the default):** `"//third_party/pigweed/src/pw_polyfill"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:157

### dir_pw_preprocessor

**Current value (from the default):** `"//third_party/pigweed/src/pw_preprocessor"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:158

### dir_pw_presubmit

**Current value (from the default):** `"//third_party/pigweed/src/pw_presubmit"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:159

### dir_pw_protobuf

**Current value (from the default):** `"//third_party/pigweed/src/pw_protobuf"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:160

### dir_pw_protobuf_compiler

**Current value (from the default):** `"//third_party/pigweed/src/pw_protobuf_compiler"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:161

### dir_pw_random

**Current value (from the default):** `"//third_party/pigweed/src/pw_random"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:162

### dir_pw_random_fuchsia

**Current value (from the default):** `"//third_party/pigweed/src/pw_random_fuchsia"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:163

### dir_pw_result

**Current value (from the default):** `"//third_party/pigweed/src/pw_result"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:164

### dir_pw_ring_buffer

**Current value (from the default):** `"//third_party/pigweed/src/pw_ring_buffer"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:165

### dir_pw_router

**Current value (from the default):** `"//third_party/pigweed/src/pw_router"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:166

### dir_pw_rpc

**Current value (from the default):** `"//third_party/pigweed/src/pw_rpc"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:167

### dir_pw_rpc_transport

**Current value (from the default):** `"//third_party/pigweed/src/pw_rpc_transport"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:168

### dir_pw_rust

**Current value (from the default):** `"//third_party/pigweed/src/pw_rust"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:169

### dir_pw_sensor

**Current value (from the default):** `"//third_party/pigweed/src/pw_sensor"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:170

### dir_pw_snapshot

**Current value (from the default):** `"//third_party/pigweed/src/pw_snapshot"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:171

### dir_pw_software_update

**Current value (from the default):** `"//third_party/pigweed/src/pw_software_update"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:172

### dir_pw_span

**Current value (from the default):** `"//third_party/pigweed/src/pw_span"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:173

### dir_pw_spi

**Current value (from the default):** `"//third_party/pigweed/src/pw_spi"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:174

### dir_pw_spi_linux

**Current value (from the default):** `"//third_party/pigweed/src/pw_spi_linux"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:175

### dir_pw_spi_mcuxpresso

**Current value (from the default):** `"//third_party/pigweed/src/pw_spi_mcuxpresso"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:176

### dir_pw_spi_rp2040

**Current value (from the default):** `"//third_party/pigweed/src/pw_spi_rp2040"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:177

### dir_pw_spi_zephyr

**Current value (from the default):** `"//third_party/pigweed/src/pw_spi_zephyr"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:178

### dir_pw_status

**Current value (from the default):** `"//third_party/pigweed/src/pw_status"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:179

### dir_pw_stm32cube_build

**Current value (from the default):** `"//third_party/pigweed/src/pw_stm32cube_build"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:180

### dir_pw_stream

**Current value (from the default):** `"//third_party/pigweed/src/pw_stream"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:181

### dir_pw_stream_uart_linux

**Current value (from the default):** `"//third_party/pigweed/src/pw_stream_uart_linux"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:182

### dir_pw_stream_uart_mcuxpresso

**Current value (from the default):** `"//third_party/pigweed/src/pw_stream_uart_mcuxpresso"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:184

### dir_pw_string

**Current value (from the default):** `"//third_party/pigweed/src/pw_string"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:185

### dir_pw_symbolizer

**Current value (from the default):** `"//third_party/pigweed/src/pw_symbolizer"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:186

### dir_pw_sync

**Current value (from the default):** `"//third_party/pigweed/src/pw_sync"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:187

### dir_pw_sync_baremetal

**Current value (from the default):** `"//third_party/pigweed/src/pw_sync_baremetal"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:188

### dir_pw_sync_embos

**Current value (from the default):** `"//third_party/pigweed/src/pw_sync_embos"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:189

### dir_pw_sync_freertos

**Current value (from the default):** `"//third_party/pigweed/src/pw_sync_freertos"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:190

### dir_pw_sync_stl

**Current value (from the default):** `"//third_party/pigweed/src/pw_sync_stl"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:191

### dir_pw_sync_threadx

**Current value (from the default):** `"//third_party/pigweed/src/pw_sync_threadx"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:192

### dir_pw_sync_zephyr

**Current value (from the default):** `"//third_party/pigweed/src/pw_sync_zephyr"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:193

### dir_pw_sys_io

**Current value (from the default):** `"//third_party/pigweed/src/pw_sys_io"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:194

### dir_pw_sys_io_ambiq_sdk

**Current value (from the default):** `"//third_party/pigweed/src/pw_sys_io_ambiq_sdk"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:195

### dir_pw_sys_io_arduino

**Current value (from the default):** `"//third_party/pigweed/src/pw_sys_io_arduino"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:196

### dir_pw_sys_io_baremetal_lm3s6965evb

**Current value (from the default):** `"//third_party/pigweed/src/pw_sys_io_baremetal_lm3s6965evb"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:198

### dir_pw_sys_io_baremetal_stm32f429

**Current value (from the default):** `"//third_party/pigweed/src/pw_sys_io_baremetal_stm32f429"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:200

### dir_pw_sys_io_emcraft_sf2

**Current value (from the default):** `"//third_party/pigweed/src/pw_sys_io_emcraft_sf2"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:202

### dir_pw_sys_io_mcuxpresso

**Current value (from the default):** `"//third_party/pigweed/src/pw_sys_io_mcuxpresso"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:203

### dir_pw_sys_io_rp2040

**Current value (from the default):** `"//third_party/pigweed/src/pw_sys_io_rp2040"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:204

### dir_pw_sys_io_stdio

**Current value (from the default):** `"//third_party/pigweed/src/pw_sys_io_stdio"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:205

### dir_pw_sys_io_stm32cube

**Current value (from the default):** `"//third_party/pigweed/src/pw_sys_io_stm32cube"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:206

### dir_pw_sys_io_zephyr

**Current value (from the default):** `"//third_party/pigweed/src/pw_sys_io_zephyr"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:207

### dir_pw_system

**Current value (from the default):** `"//third_party/pigweed/src/pw_system"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:208

### dir_pw_target_runner

**Current value (from the default):** `"//third_party/pigweed/src/pw_target_runner"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:209

### dir_pw_third_party

This is retained for backwards compatibility. Prefer using the pw_external_*
variables instead.

**Current value (from the default):** `"//third_party/pigweed/src/third_party"`

From //third_party/pigweed/src/modules.gni:27

### dir_pw_third_party_boringssl

If compiling backends with boringssl, this variable is set to the path to
the boringssl source code. When set, a pw_source_set for the boringssl
library is created at "$pw_external_boringssl".

**Current value (from the default):** `""`

From //third_party/pigweed/src/third_party/boringssl/boringssl.gni:19

### dir_pw_third_party_chre

If compiling backends with chre, this variable is set to the path to the
chre installation. When set, a pw_source_set for the chre library is
created at "$pw_external_chre".

**Current value for `target_cpu = "arm64"`:** `"//third_party/chre/src"`

From //.gn:143

**Overridden from the default:** `""`

From //third_party/pigweed/src/third_party/chre/chre.gni:19

**Current value for `target_cpu = "riscv64"`:** `"//third_party/chre/src"`

From //.gn:143

**Overridden from the default:** `""`

From //third_party/pigweed/src/third_party/chre/chre.gni:19

**Current value for `target_cpu = "x64"`:** `"//third_party/chre/src"`

From //.gn:143

**Overridden from the default:** `""`

From //third_party/pigweed/src/third_party/chre/chre.gni:19

### dir_pw_third_party_emboss

If compiling with Emboss, this variable is set to the path to the Emboss
source code.

**Current value for `target_cpu = "arm64"`:** `"//third_party/github.com/google/emboss/src"`

From //.gn:110

**Overridden from the default:** `""`

From //third_party/pigweed/src/third_party/emboss/emboss.gni:20

**Current value for `target_cpu = "riscv64"`:** `"//third_party/github.com/google/emboss/src"`

From //.gn:110

**Overridden from the default:** `""`

From //third_party/pigweed/src/third_party/emboss/emboss.gni:20

**Current value for `target_cpu = "x64"`:** `"//third_party/github.com/google/emboss/src"`

From //.gn:110

**Overridden from the default:** `""`

From //third_party/pigweed/src/third_party/emboss/emboss.gni:20

### dir_pw_third_party_fuchsia

Path to the Fuchsia sources to use in Pigweed's build. Defaults to Pigweed's
mirror of the few Fuchsia source files it uses.

**Current value for `target_cpu = "arm64"`:** `"//"`

From //.gn:104

**Overridden from the default:** `"//third_party/pigweed/src/third_party/fuchsia/repo"`

From //third_party/pigweed/src/third_party/fuchsia/fuchsia.gni:20

**Current value for `target_cpu = "riscv64"`:** `"//"`

From //.gn:104

**Overridden from the default:** `"//third_party/pigweed/src/third_party/fuchsia/repo"`

From //third_party/pigweed/src/third_party/fuchsia/fuchsia.gni:20

**Current value for `target_cpu = "x64"`:** `"//"`

From //.gn:104

**Overridden from the default:** `"//third_party/pigweed/src/third_party/fuchsia/repo"`

From //third_party/pigweed/src/third_party/fuchsia/fuchsia.gni:20

### dir_pw_third_party_googletest

If compiling tests with googletest, this variable is set to the path to the
googletest installation. When set, a pw_source_set for the googletest
library is created at "$pw_external_googletest". Incompatible
with pw_third_party_googletest_ALIAS definition.

**Current value (from the default):** `""`

From //third_party/pigweed/src/third_party/googletest/googletest.gni:20

### dir_pw_third_party_mbedtls

**Current value (from the default):** `""`

From //third_party/pigweed/src/third_party/mbedtls/mbedtls.gni:31

### dir_pw_third_party_nanopb

If compiling protos for nanopb, this variable is set to the path to the
nanopb installation. When set, a pw_source_set for the nanopb library is
created at "$pw_external_nanopb".

**Current value (from the default):** `""`

From //third_party/pigweed/src/third_party/nanopb/nanopb.gni:22

### dir_pw_third_party_protobuf

If compiling host tools that use libprotobuf, this variable is set to the
path to the protobuf installation. When set, a pw_source_set for the
protobuf library is created at "$pw_external_protobuf".

**Current value for `target_cpu = "arm64"`:** `"//third_party/protobuf/src"`

From //.gn:131

**Overridden from the default:** `""`

From //third_party/pigweed/src/third_party/protobuf/protobuf.gni:19

**Current value for `target_cpu = "riscv64"`:** `"//third_party/protobuf/src"`

From //.gn:131

**Overridden from the default:** `""`

From //third_party/pigweed/src/third_party/protobuf/protobuf.gni:19

**Current value for `target_cpu = "x64"`:** `"//third_party/protobuf/src"`

From //.gn:131

**Overridden from the default:** `""`

From //third_party/pigweed/src/third_party/protobuf/protobuf.gni:19

### dir_pw_thread

**Current value (from the default):** `"//third_party/pigweed/src/pw_thread"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:210

### dir_pw_thread_embos

**Current value (from the default):** `"//third_party/pigweed/src/pw_thread_embos"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:211

### dir_pw_thread_freertos

**Current value (from the default):** `"//third_party/pigweed/src/pw_thread_freertos"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:212

### dir_pw_thread_stl

**Current value (from the default):** `"//third_party/pigweed/src/pw_thread_stl"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:213

### dir_pw_thread_threadx

**Current value (from the default):** `"//third_party/pigweed/src/pw_thread_threadx"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:214

### dir_pw_thread_zephyr

**Current value (from the default):** `"//third_party/pigweed/src/pw_thread_zephyr"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:215

### dir_pw_time

**Current value (from the default):** `"//third_party/pigweed/src/pw_time"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:216

### dir_pw_tls_client

**Current value (from the default):** `"//third_party/pigweed/src/pw_tls_client"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:217

### dir_pw_tls_client_boringssl

**Current value (from the default):** `"//third_party/pigweed/src/pw_tls_client_boringssl"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:219

### dir_pw_tls_client_mbedtls

**Current value (from the default):** `"//third_party/pigweed/src/pw_tls_client_mbedtls"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:221

### dir_pw_tokenizer

**Current value (from the default):** `"//third_party/pigweed/src/pw_tokenizer"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:222

### dir_pw_toolchain

**Current value (from the default):** `"//third_party/pigweed/src/pw_toolchain"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:223

### dir_pw_trace

**Current value (from the default):** `"//third_party/pigweed/src/pw_trace"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:224

### dir_pw_trace_tokenized

**Current value (from the default):** `"//third_party/pigweed/src/pw_trace_tokenized"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:225

### dir_pw_transfer

**Current value (from the default):** `"//third_party/pigweed/src/pw_transfer"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:226

### dir_pw_uart

**Current value (from the default):** `"//third_party/pigweed/src/pw_uart"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:227

### dir_pw_uart_mcuxpresso

**Current value (from the default):** `"//third_party/pigweed/src/pw_uart_mcuxpresso"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:228

### dir_pw_unit_test

**Current value (from the default):** `"//third_party/pigweed/src/pw_unit_test"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:229

### dir_pw_uuid

**Current value (from the default):** `"//third_party/pigweed/src/pw_uuid"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:230

### dir_pw_varint

**Current value (from the default):** `"//third_party/pigweed/src/pw_varint"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:231

### dir_pw_watch

**Current value (from the default):** `"//third_party/pigweed/src/pw_watch"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:232

### dir_pw_web

**Current value (from the default):** `"//third_party/pigweed/src/pw_web"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:233

### dir_pw_work_queue

**Current value (from the default):** `"//third_party/pigweed/src/pw_work_queue"`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:234

### disable_boot_tests

Default value for `disabled` parameter for generated `boot_test()`.
TODO(https://fxbug.dev/320511796): Cleanup when no longer necessary.

**Current value (from the default):** `false`

From //build/testing/boot_tests/boot_test.gni:16

### disable_xattr_for_rbe

By default our RBE wrappers record the digest of remote action outputs
in an xattr to save tools from having to re-hashing these files.
In circumstances where xattr cannot be used (e.g. FUSE limitations),
set this to True to disable use of xattr.

**Current value (from the default):** `false`

From //build/toolchain/rbe.gni:37

### discoverable_package_labels

If you add package labels to this variable, the packages will be included in
the 'discoverable' package set, as defined by RFC-0212 "Package Sets":
https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs/0212_package_sets

They will be compiled, and published, but not added as dependencies of the
assembled images, and so will not be able to cause the inclusion of
entries to the legacy bundle.

As these cannot be part of the legacy AIB for a product, there is no
"legacy" version of this argument.

**Current value (from the default):** `[]`

From //BUILD.gn:47

### dont_profile_source_files

List of paths to source files to NOT instrument by `profile` variants.
These take precedence over `profile_source_files`.

**Current value (from the default):** `["//prebuilt/*"]`

From //build/config/profile/config.gni:18

### dwarf_version

Explicitly specify DWARF version used.

**Current value (from the default):** `5`

From //build/config/compiler.gni:102

### e2e_test_labels

Host-driven, "end-to-end" tests that run on a Fuchsia image (either real
hardware or emulated).

**Current value (from the default):** `[]`

From //BUILD.gn:72

### emboss_build_dir_to_root

An optional relative path from `root_build_dir` to `//`, which can be used
by `emboss_cc_library` and `emboss_rust_library` to rebase `embossc` source
files.

**Current value (from the default):** `""`

From //third_party/github.com/google/experimental-emboss-rust/build_defs.gni:38

### enable_assembly_heapdump

If true, forcefully enables Heapdump for the entire system (including
component_manager and all other monikers) during assembly of eng standard
builds.

**Current value (from the default):** `false`

From //src/lib/assembly/platform_configuration/BUILD.gn:13

### enable_bazel_remote_rbe

Configure bazel to build remotely with RBE where supported.
This can speed up builds via remote caching.
This option requires that bazel invocations have direct
external network access, and that users are authenticated to
access a remote execution service.
The Remote Execution API can be found at:
https://github.com/bazelbuild/remote-apis
For an overview of remote execution for Bazel, see https://bazel.build/remote/rbe

**Current value (from the default):** `false`

From //build/bazel/remote_services.gni:16

### enable_delayed_bazel_actions

**Current value (from the default):** `""`

From //BUILD.gn:146

### enable_frame_pointers

Controls whether the compiler emits full stack frames for function calls.
This reduces performance but increases the ability to generate good
stack traces, especially when we have bugs around unwind table generation.
It does not apply for host targets (see //build/config/BUILD.gn where it
is unset).

**Current value (from the default):** `true`

From //build/config/enable_frame_pointers.gni:11

### enable_grpc_ares

Compiles with ares.

**Current value (from the default):** `false`

From //third_party/grpc/BUILD.gn:15

### enable_jobserver

Set to true to have Ninja implement a GNU Make jobserver pool
to better coordinate parallel tasks, especially when sub-builds
are recursively invoked.

This allows launching all IDK sub-builds
at the same time without risking overloading the current machine.

IMPORTANT: This feature requires a version of Ninja that implements
the `--jobserver` option. See https://fxbug.dev/XXXXX for details.


**Current value (from the default):** `false`

From //build/config/jobserver.gni:16

### enable_lock_dep

Enable kernel lock dependency tracking.

**Current value (from the default):** `false`

From //zircon/kernel/params.gni:18

### enable_lock_dep_metadata_only

Enable kernel lock dependency metadata only (ignored if enable_lock_dep is
true).

**Current value (from the default):** `false`

From //zircon/kernel/params.gni:22

### enable_lock_dep_tests

Enable kernel lock dependency tracking tests.  By default this is
enabled when tracking is enabled, but can also be enabled independently
to assess whether the tests build and *fail correctly* when lockdep is
disabled.

**Current value (from the default):** `false`

From //zircon/kernel/params.gni:132

### enable_mdns_trace

Enables the tracing feature of mdns, which can be turned on using
"mdns-util verbose".

**Current value (from the default):** `false`

From //src/connectivity/network/mdns/service/BUILD.gn:14

### enable_netboot

The netboot zbi has been deprecated. This GN arg is now used to generate a
warning.

**Current value (from the default):** `false`

From //build/images/args.gni:114

### enable_netstack2_tracing

**Current value (from the default):** `false`

From //src/connectivity/network/BUILD.gn:9

### enable_perfetto_android_java_sdk

**Current value (from the default):** `false`

From //third_party/perfetto/gn/perfetto.gni:220

### enable_perfetto_benchmarks

**Current value (from the default):** `false`

From //third_party/perfetto/gn/perfetto.gni:230

### enable_perfetto_etm_importer

**Current value (from the default):** `false`

From //third_party/perfetto/gn/perfetto.gni:435

### enable_perfetto_fuzzers

**Current value (from the default):** `false`

From //third_party/perfetto/gn/perfetto.gni:233

### enable_perfetto_grpc

Enables gRPC in the Perfetto codebase. gRPC significantly increases build
times and the general footprint of Perfetto. As it only required for
BigTrace and even then only to build the final ready-to-ship binary, don't
enable this by default.

**Current value (from the default):** `false`

From //third_party/perfetto/gn/perfetto.gni:429

### enable_perfetto_heapprofd

**Current value (from the default):** `false`

From //third_party/perfetto/gn/perfetto.gni:181

### enable_perfetto_integration_tests

**Current value (from the default):** `false`

From //third_party/perfetto/gn/perfetto.gni:217

### enable_perfetto_ipc

**Current value (from the default):** `true`

From //third_party/perfetto/gn/perfetto.gni:174

### enable_perfetto_llvm_demangle

**Current value (from the default):** `false`

From //third_party/perfetto/gn/perfetto.gni:423

### enable_perfetto_llvm_symbolizer

Enables the use of the LLVM symbolizer in trace_processor.

**Current value (from the default):** `false`

From //third_party/perfetto/gn/perfetto.gni:439

### enable_perfetto_lockfree_taskrunner

This flag is used for the migration of UnixTaskRunner -> LockFreeTaskRunner.
It determines whether MaybeLockFreeTaskRunner is backed by UnixTaskRunner
or the newer LockFreeTaskRunner.
UnixTaskRunner is the battle-tested original TaskRunner implementaiton used
from 2017 -> 2025. LockFreeTaskRunner is the new improved TaskRunner which
is planning to sunset UnixTaskRunner.
Note that on Android platform (non-standalone) builds this flag is ignored
and the Android flag "use_lockfree_taskrunner" is used instead (see
perfetto_flags.aconfig)

**Current value (from the default):** `true`

From //third_party/perfetto/gn/perfetto.gni:315

### enable_perfetto_merged_protos_check

Check that the merged perfetto_trace.proto can be translated to a C++ lite
proto and compiled. This is disabled by default because it's expensive (it
can take a couple of minutes).

**Current value (from the default):** `false`

From //third_party/perfetto/gn/perfetto.gni:464

### enable_perfetto_pcre2

Enables PCRE2 support. Individual modules that should not pull libpcre2
in (e.g. libperfetto_client_experimental) can opt out by setting
-DPERFETTO_REGEX_FORCE_STD on their compile, which makes base::Regex
fall back to the std::regex backend.

**Current value (from the default):** `false`

From //third_party/perfetto/gn/perfetto.gni:412

### enable_perfetto_platform_services

**Current value (from the default):** `false`

From //third_party/perfetto/gn/perfetto.gni:165

### enable_perfetto_protovm_compiler

Enables the protovm_compiler tool.

**Current value (from the default):** `false`

From //third_party/perfetto/gn/perfetto.gni:450

### enable_perfetto_re2

Enables RE2 support.

**Current value (from the default):** `false`

From //third_party/perfetto/gn/perfetto.gni:415

### enable_perfetto_rt_mutex

**Current value (from the default):** `false`

From //third_party/perfetto/gn/perfetto.gni:294

### enable_perfetto_sock_inotify

**Current value (from the default):** `false`

From //third_party/perfetto/gn/perfetto.gni:303

### enable_perfetto_stderr_crash_dump

**Current value (from the default):** `false`

From //third_party/perfetto/gn/perfetto.gni:271

### enable_perfetto_system_consumer

**Current value (from the default):** `true`

From //third_party/perfetto/gn/perfetto.gni:322

### enable_perfetto_tools

**Current value (from the default):** `false`

From //third_party/perfetto/gn/perfetto.gni:211

### enable_perfetto_trace_processor

**Current value (from the default):** `false`

From //third_party/perfetto/gn/perfetto.gni:197

### enable_perfetto_trace_processor_httpd

**Current value (from the default):** `false`

From //third_party/perfetto/gn/perfetto.gni:382

### enable_perfetto_trace_processor_json

**Current value (from the default):** `false`

From //third_party/perfetto/gn/perfetto.gni:367

### enable_perfetto_trace_processor_linenoise

**Current value (from the default):** `false`

From //third_party/perfetto/gn/perfetto.gni:361

### enable_perfetto_trace_processor_mac_instruments

**Current value (from the default):** `false`

From //third_party/perfetto/gn/perfetto.gni:373

### enable_perfetto_trace_processor_percentile

**Current value (from the default):** `false`

From //third_party/perfetto/gn/perfetto.gni:356

### enable_perfetto_trace_processor_sqlite

**Current value (from the default):** `false`

From //third_party/perfetto/gn/perfetto.gni:351

### enable_perfetto_traceconv

**Current value (from the default):** `false`

From //third_party/perfetto/gn/perfetto.gni:454

### enable_perfetto_traced_perf

**Current value (from the default):** `false`

From //third_party/perfetto/gn/perfetto.gni:189

### enable_perfetto_traced_probes

**Current value (from the default):** `false`

From //third_party/perfetto/gn/perfetto.gni:339

### enable_perfetto_traced_relay

The relay service is enabled when platform services are enabled.
TODO(chinglinyu) check if we can enable on Windows.

**Current value (from the default):** `false`

From //third_party/perfetto/gn/perfetto.gni:343

### enable_perfetto_ui

**Current value (from the default):** `false`

From //third_party/perfetto/gn/perfetto.gni:458

### enable_perfetto_unittests

**Current value (from the default):** `false`

From //third_party/perfetto/gn/perfetto.gni:213

### enable_perfetto_version_gen

**Current value (from the default):** `false`

From //third_party/perfetto/gn/perfetto.gni:239

### enable_perfetto_watchdog

**Current value (from the default):** `false`

From //third_party/perfetto/gn/perfetto.gni:206

### enable_perfetto_winscope

**Current value (from the default):** `false`

From //third_party/perfetto/gn/perfetto.gni:444

### enable_perfetto_x64_cpu_opt

**Current value (from the default):** `false`

From //third_party/perfetto/gn/perfetto.gni:279

### enable_perfetto_zlib

**Current value (from the default):** `false`

From //third_party/perfetto/gn/perfetto.gni:390

### enable_perfetto_zstd

**Current value (from the default):** `false`

From //third_party/perfetto/gn/perfetto.gni:404

### enable_power_manager_debug

**Current value (from the default):** `false`

From //src/power/power-manager/BUILD.gn:130

### enable_rseq_backend_for_rcu

The RSEQ backend is not yet implemented.

**Current value (from the default):** `false`

From //src/lib/fuchsia-rcu/BUILD.gn:11

### enable_sestarnix_userspace_tests_on_linux

Enable SEStarnix userspace tests on Linux. On CI/CQ this requires internal
prebuilts only available in internal checkouts.
TODO(https://fxbug.dev/510724670): Remove this flag when the prebuilts are available publicly.

**Current value (from the default):** `false`

From //src/starnix/build/args.gni:14

### escher_test_for_glsl_spirv_mismatch

If true, this enables the |SpirvNotChangedTest| to check if the precompiled
shaders on disk are up to date and reflect the current shader source code
compiled with the latest shaderc tools/optimizations. People on the Scenic
team should build with this flag turned on to make sure that any shader
changes that were not run through the precompiler have their updated spirv
written to disk. Other teams and CQ do not need to worry about this flag.

**Current value (from the default):** `false`

From //src/ui/lib/escher/build_args.gni:19

### escher_use_runtime_glsl

Determines whether or not escher will build with the glslang and shaderc
libraries. When false, these libraries will not be included in the scenic/
escher binary and as a result shaders will not be able to be compiled at
runtime. Precompiled spirv code will be loaded into memory from disk
instead.

**Current value (from the default):** `false`

From //src/ui/lib/escher/build_args.gni:11

### exclude_testonly_syscalls

If true, excludes syscalls with the [testonly] attribute.

**Current value (from the default):** `false`

From //zircon/vdso/vdso.gni:9

### experimental_continuous_per_vmo_attribution_enabled

Add in-kernel support for querying a per-VMO populated bytes high-water
mark.

**Current value (from the default):** `false`

From //zircon/kernel/params.gni:112

### experimental_cxx_version

**NOTE:** This is for **experimentation only** and should not normally be
changed.  Set the version of the C++ standard to use when compiling. Must be
one of the values in `_available_cxx_versions`.
Note also that GN code should never use this variable directly, but always
instead use the `fuchsia_cxx_version` variable.

**Current value (from the default):** `false`

From //build/config/fuchsia_cxx_version.gni:35

### experimental_dso_enabled

If true, components that have an experimental dso version enabled
(//src/sys/runners/dso) will be included in place of the standard version.
This flag exists to support experimental dso builders, for more information
see https://fxbug.dev/403545512

**Current value (from the default):** `false`

From //build/assembly/tools/assembly/BUILD.gn:16

### experimental_elf_page_size

**TODO(https://fxbug.dev/42107211):** This is _extremely experimental_ and
**must not** be set for production (including SDK) builds.

This is the ELF ABI maximum page size.  It can always be increased (by
powers of two) while staying ABI-compatible, but never decreased; any
increases likely affect storage and memory sizes.  Binaries linked by
this GN build will be compatible with ELF loading on a system that at
runtime actually uses page sizes as big as this, as well as anything
smaller down to 4k.  No system that uses page sizes larger than this
will be able to load any ELF binaries produced by this build.

**Current value (from the default):** `4096`

From //build/config/fuchsia/BUILD.gn:35

### experimental_force_enable_new_wakeup_accounting

Force new scheduler wakeup accounting to be enabled, ignoring the boot
option value. This is used to force the feature on at the individual try bot
level, where gn arguments can be set but boot options cannot.

TODO(https://fxbug.dev/322207536): Stop resetting start and finish times
when unblocking once we solve races higher in the stack.

**Current value (from the default):** `false`

From //zircon/kernel/params.gni:120

### experimental_mem_enabled

If true, experimental memory optimization features like the
`kernel.oom.experimental_expand_memory_stall` kernel argument will be
enabled.

**Current value (from the default):** `false`

From //build/assembly/tools/assembly/BUILD.gn:21

### export_bazel_tests

Configures whether Bazel tests show up in tests.json so they're visible to
Fuchsia test runners, i.e. `fx test` and `botanist`.
LINT.IfChange

**Current value (from the default):** `true`

From //build/config/export_bazel_tests.gni:9

### exported_package_labels

If you add labels to this variable the `exported_fuchsia_package_archive()`
targets captured by these labels will be collected and exposed in the
'package_archives' build api module. Ordinary `fuchsia_package_archive()`
targets are not captured.

Note: This variable is only used for metadata collection -- any package
labels added here will still need to be included in the build graph
elsewhere.

It's usually advisable to use labels of well-defined, curated `group()`s of
packages instead of explicitly adding the labels of the
`exported_fuchsia_package_archive()` targets directly.

**Current value (from the default):** `[]`

From //BUILD.gn:61

### extra_bazel_assembly_targets

Extra GN targets to include when Bazel assembly is enabled. This list is
useful for including verification and other Bazel assembly specific targets.

**Current value (from the default):** `[]`

From //build/images/args.gni:75

### extra_bazel_build_action_labels

Set this in args.gn to list bazel_action() labels that are not
reachable from //:default, but that may be built locally
during development. This is required to ensure their @gn_targets_dir
directories are generated at generation time, and to perform queries
on the Bazel graph correctly.

**Current value (from the default):** `[]`

From //build/bazel/bazel_build_action_labels.gni:11

### extra_kernel_test_images

Each kernel_zbi_test() will stamp out a zbi_test() per entry in the provided
list of kernel images; this list unconditionally extends that selection.
This enables devs and builders to introduce more kinds of tests in the GN
graph without affecting gen times by default.

**Current value (from the default):** `[]`

From //build/testing/boot_tests/kernel_zbi_test.gni:25

### extra_package_labels

**Current value (from the default):** `[]`

From //third_party/cobalt/BUILD.gn:10

### extra_variants

Additional variant toolchain configs to support.
This is just added to [`known_variants`](#known_variants).

**Current value (from the default):** `[]`

From //build/config/BUILDCONFIG.gn:545

### fat_lto_objects

Whether to enable -ffat-lto-objects in LTO builds.
https://llvm.org/docs/FatLTO.html

**Current value (from the default):** `true`

From //build/config/lto/config.gni:14

### ffmpeg_profile

**Current value (from the default):** `"default"`

From //src/media/lib/ffmpeg/BUILD.gn:53

### ffx_build_dual_mode_plugins_as_subtools

Build any ffx plugins that can be built either as built-in or as separate
subtools as subtools.

Note that if you change this and don't `fx clean` you may wind up with stale
copies of either the main `ffx` binary (with all the plugins built in) or
the separately compiled ones, and that might produce confusing `ffx help`
or `ffx commands` output.

When all subtools that will be migrated to the SDK have been migrated,
this config flag will be set to true by default, deprecated, and eventually
removed: https://fxbug.dev/42068537

**Current value (from the default):** `false`

From //src/developer/ffx/config.gni:19

### fidl_api_coverage

If true, generates API coverage comments inside FIDL bindings.

**Current value (from the default):** `false`

From //build/fidl/toolchain.gni:11

### flatland_verbose_logging

If true, Flatland will log an excruciating amount of data.  For debugging.

**Current value (from the default):** `false`

From //src/ui/scenic/lib/utils/build_args.gni:7

### focaltech_driver_flavor

Toggles between the C++ and Rust driver implementations.

TODO(https://fxbug.dev/527192080): Remove the toggle after the Rust port is completed.
Valid values: "cpp", "rust"

**Current value (from the default):** `"cpp"`

From //src/ui/input/drivers/focaltech/BUILD.gn:16

### font_catalog_paths

**Current value (from the default):** `["//prebuilt/third_party/fonts/fuchsia.font_catalog.json"]`

From //src/fonts/build/font_args.gni:17

### font_pkg_entries

Merged contents of .font_pkgs.json files. Populated in fonts.gni.

**Current value (from the default):** `[]`

From //src/fonts/build/font_args.gni:32

### font_pkgs_paths

Locations of .font_pkgs.json files, which list the locations of font files
within the workspace, as well as safe names that are derived from the fonts'
file names and can be used to name Fuchsia packages.

**Current value (from the default):** `["//prebuilt/third_party/fonts/fuchsia.font_pkgs.json"]`

From //src/fonts/build/font_args.gni:22

### fonts_dir

Directory into which all fonts are checked out from CIPD

**Current value (from the default):** `"//prebuilt/third_party/fonts"`

From //src/fonts/build/font_args.gni:12

### fuchsia_product_assembly_config_label

The product assembly config used to configure the main Fuchsia image.
For GN products, this is required.
For Bazel products, this is optional.
For Bazel products, netboot will only be available when this is supplied.

**Current value (from the default):** `false`

From //build/product.gni:10

### fuchsia_sdk_root

Consumers of the Fuchsia SDK instantiate templates for various SDK parts at
a specific spot within their buildroots. The target name for the specific
part is then derived from the part name as specified in the meta.json
manifest. Different buildroot instantiate the SDK parts at different
locations and then set this variable. GN rules can then prefix this variable
name in SDK builds to the name of the SDK part. This flag is meaningless in
non-SDK buildroots.

**Current value (from the default):** `""`

From //build/fuchsia/sdk.gni:17

### fuchsia_sync_detect_lock_cycles

Whether to use tracing-mutex to detect cycles in the lock acquisition graph.
Only enable this on debug builds by default because it makes
balanced/release too slow for real use.
LINT.IfChange

**Current value (from the default):** `false`

From //src/lib/fuchsia-sync/build/args.gni:10

### futex_block_tracing_enabled

TODO(johngro): document

**Current value (from the default):** `false`

From //zircon/kernel/params.gni:60

### fvm_partition

**Current value (from the default):** `""`

From //build/images/args.gni:30

### fxfs_blob

Use Fxfs's blob implementation
Changes the flashing logic because the outputs changed.
Toggles a bunch of tests to use fxfs.

**Current value (from the default):** `true`

From //src/storage/fshost/generated_fshost_config.gni:17

### fxfs_partition

**Current value (from the default):** `""`

From //build/images/args.gni:31

### gcc_tool_dir

Directory where the GCC toolchain binaries ("gcc", "nm", etc.) are found.
This directory is expected to contain `aarch64-elf-*` and `x86_64-elf-*`
tools used to build for the Fuchsia targets.  This directory will not be
used for host tools.  If this is "", then a standard prebuilt is used.

**Current value (from the default):** `""`

From //build/toolchain/zircon/gcc.gni:10

### generate_cuckoo_tests

Defines the default value of `kernel_zbi_test()` template `generate_cuckoo`
parameters. When true, all instances of `kernel_zbi_tests()` will default to
generating a `cuckoo_zbi_test()` which is a full system image generated for
exfiltrating instrumentation data from a target system.

**Current value (from the default):** `false`

From //build/testing/boot_tests/kernel_zbi_test.gni:19

### generate_licenses_spdx_stubs

When true, generated_licenses_spdx template will generate stub SPDX files
with placeholder license. License gathering will be skipped. Since license
gathering is resource intensive, this is useful for non-production builds
that would run faster. The global configuration can be overwritten in
specific `generated_licenses_spdx` template invocation via the
`generate_stub` parameter.

**Current value (from the default):** `true`

From //build/licenses/generated_licenses_spdx.gni:14

### generate_plasa_artifacts

If set, causes the plasa artifacts to be generated.  Not all builds need to
use the plasa artifacts, so we set the default to skip the generation.

**Current value (from the default):** `false`

From //build/sdk/plasa/config.gni:8

### gigaboot_backends

Specifies the gn target that implements the required backends defined in
`gigaboot/cpp/backends.h`

**Current value (from the default):** `"//src/firmware/gigaboot/cpp:backends_nuc"`

From //src/firmware/gigaboot/cpp/backends.gni:8

### gigaboot_eng_permanent_attributes

Permanent attributes file for eng gigaboot

**Current value (from the default):** `"//third_party/android/platform/external/avb/test/data/atx_permanent_attributes.bin"`

From //src/firmware/gigaboot/cpp/backends.gni:11

### gigaboot_gbl_efi_app

**Current value (from the default):** `"//prebuilt/third_party/android/gbl/mainline/gbl_x86_64_dev.efi"`

From //src/firmware/gigaboot/cpp/backends.gni:34

### gigaboot_use_gbl

Boolean to indicate whether to use GBL for boot.

TODO(b/368647237): This is a temporary switch for enabling GBL based
installer first before we are ready to migrate gigaboot to use GBL.

**Current value (from the default):** `false`

From //src/firmware/gigaboot/cpp/backends.gni:40

### gigaboot_user_permanent_attributes

Permanent attributes file for prod-signed gigaboot. Setting this enables
target //src/firmware/gigaboot/cpp:user-esp

**Current value (from the default):** `""`

From //src/firmware/gigaboot/cpp/backends.gni:20

### gigaboot_userdebug_permanent_attributes

Permanent attributes file for userdebug gigaboot. Setting this enables
target
//src/firmware/gigaboot/cpp:userdebug-esp

**Current value (from the default):** `""`

From //src/firmware/gigaboot/cpp/backends.gni:16

### go_vet_enabled

  go_vet_enabled
    [bool] if false, go vet invocations are disabled for all builds.

**Current value (from the default):** `false`

From //build/go/go_build.gni:21

### gocache_dir

  gocache_dir
    Directory GOCACHE environment variable will be set to. This directory
    will have build and test results cached, and is safe to be written to
    concurrently. If overridden, this directory must be a full path.

**Current value (from the default):** `"/b/s/w/ir/x/w/fuchsia/out/not-default/.gocache"`

From //build/go/go_build.gni:17

### goodix_gt6853_driver_flavor

Toggles between the C++ and Rust driver implementations.

TODO(https://fxbug.dev/527194998): Remove the toggle after the Rust port is completed.
Valid values: "cpp", "rust"

**Current value (from the default):** `"cpp"`

From //src/ui/input/drivers/goodix-gt6853/BUILD.gn:20

### goodix_gt92xx_driver_flavor

Toggles between the C++ and Rust driver implementations.

TODO(https://fxbug.dev/527195603): Remove the toggle after the Rust port is completed.
Valid values: "cpp", "rust"

**Current value (from the default):** `"cpp"`

From //src/ui/input/drivers/goodix-gt92xx/BUILD.gn:16

### graphics_compute_generate_debug_shaders


Set to true in your args.gn file to generate pre-processed and
auto-formatted shaders under the "debug" sub-directory of HotSort
and Spinel target generation output directories.

These are never used, but can be reviewed manually to verify the
impact of configuration parameters, or when modifying a compute
shader.

Example results:

  out/default/
    gen/src/graphics/lib/compute/
       hotsort/targets/hs_amd_gcn3_u64/
          comp/
            hs_transpose.comp -> unpreprocessed shader
          debug/
            hs_transpose.glsl -> preprocessed shader


**Current value (from the default):** `true`

From //src/graphics/lib/compute/gn/glsl_shader_rules.gni:29

### graphics_compute_generate_spirv_debug_info


If you're using GPU-assisted validation then it's useful to
include debug info in combination with skipping the spirv-opt and
spirv-reduce pass.


**Current value (from the default):** `false`

From //src/graphics/lib/compute/gn/glsl_shader_rules.gni:47

### graphics_compute_skip_spirv_opt


At times we may want to compare the performance of unoptimized
vs. optimized shaders.  On desktop platforms, use of spirv-opt
doesn't appear to provide major performance improvements but it
significantly reduces the size of the SPIR-V modules.

Disabling the spirv-opt pass may also be useful in identifying and
attributing code generation bugs.


**Current value (from the default):** `false`

From //src/graphics/lib/compute/gn/glsl_shader_rules.gni:40

### grpc_use_static_linking

**Current value (from the default):** `false`

From //third_party/grpc/BUILD.gn:24

### hangcheck_timeout_ms

Set this to accommodate long running tests

**Current value (from the default):** `0`

From //src/graphics/drivers/msd-intel-gen/src/BUILD.gn:9

### has_board

This is a build that imports a board (vs. sdk).  If a board is set
(fx set <product>.<board>) this is true.

**Current value for `target_cpu = "arm64"`:** `true`

From //boards/arm64.gni:20

**Overridden from the default:** `false`

From //build/board.gni:8

**Current value for `target_cpu = "riscv64"`:** `true`

From //boards/riscv64.gni:20

**Overridden from the default:** `false`

From //build/board.gni:8

**Current value for `target_cpu = "x64"`:** `true`

From //boards/x64.gni:20

**Overridden from the default:** `false`

From //build/board.gni:8

### hermetic_test_package_labels

Fully hermetic tests (both by packaging and at runtime)

**Current value (from the default):** `[]`

From //BUILD.gn:68

### host_byteorder

**Current value (from the default):** `"undefined"`

From //build/config/host_byteorder.gni:7

### host_cpu

**Current value (from the default):** `"x64"`

### host_labels

If you add labels to this variable, these will be included in the 'host'
artifact set, which represents an additional set of host-only software that
is produced by the build.

These will be added to the build using the host toolchain.

**Current value for `target_cpu = "arm64"`:** `[]`

From //out/not-default/args.gn:14

**Overridden from the default:** `[]`

From //BUILD.gn:90

**Current value for `target_cpu = "riscv64"`:** `[]`

From //out/not-default/args.gn:14

**Overridden from the default:** `[]`

From //BUILD.gn:90

**Current value for `target_cpu = "x64"`:** `[]`

From //out/not-default/args.gn:14

**Overridden from the default:** `[]`

From //BUILD.gn:90

### host_os

**Current value (from the default):** `"linux"`

### host_test_labels

Host-only tests.  These cannot have any dependency on an assembled platform
image, or the compiled OS itself, not even for their host_test_data().

These will be added to the build using the host toolchain.

**Current value (from the default):** `[]`

From //BUILD.gn:78

### host_tools_base_path_override

Set this to the path of a directory containing prebuilt binaries for
the host tools generated by compiled_action(). This is useful when
performing sub-builds (e.g. when generating the IDK which requires
rebuilding SDK collections for different API levels and CPU architectures)
to avoid rebuilding the host tools every time.


**Current value (from the default):** `""`

From //build/host.gni:21

### host_tools_dir

This is the directory where host tools intended for manual use by
developers get installed.  It's something a developer might put
into their shell's $PATH.  Host tools that are just needed as part
of the build do not get copied here.  This directory is only for
things that are generally useful for testing or debugging or
whatnot outside of the GN build itself.  These are only installed
by an explicit install_host_tools() rule (see //build/host.gni).

**Current value (from the default):** `"//out/not-default/host-tools"`

From //build/host.gni:13

### hwasan_default_options

Default [HawrdwareAddressSanitizer](https://clang.llvm.org/docs/HardwareAssistedAddressSanitizerDesign.html)
options (before the `HWASAN_OPTIONS` environment variable is read at
runtime).  This can be set as a build argument to affect most "hwasan"
variants in $variants (which see), or overridden in $toolchain_args in
one of those variants.  This can be a list of strings or a single string.

Note that even if this is empty, programs in this build **cannot** define
their own `__hwasan_default_options` C function.  Instead, they can use a
sanitizer_extra_options() target in their `deps` and then any options
injected that way can override that option's setting in this list.

**Current value (from the default):** `["allocator_may_return_null=1"]`

From //build/config/sanitizers/sanitizer_default_options.gni:96

### i_can_haz_atlas_camera

If true, power on the Atlas camera at boot.
TODO(https://fxbug.dev/42162166): remove once we have a better way to manage ACPI device power.

**Current value (from the default):** `false`

From //src/devices/board/lib/acpi/BUILD.gn:10

### ice_detection

Enables a rustc wrapper that detects timeouts and ICEs
TODO(pineapple): enable by default when using rust_incremental after
b/345596983 is resolved

**Current value (from the default):** `false`

From //build/rust/build.gni:30

### icu_copy_icudata_to_root_build_dir

If set, the ":icudata" target will copy the ICU data to $root_build_dir.

**Current value (from the default):** `false`

From //build/icu.gni:27

### icu_disable_thin_archive

If true, compile icu into a standalone static library. Currently this is
only useful on Chrome OS.

**Current value (from the default):** `false`

From //build/icu.gni:19

### icu_fuchsia_extra_compile_flags

Fuchsia sometimes requires extra compilation flags for ICU to adapt it to
its current toolchain. Since it takes a while for ICU to roll through
Fuchsia, it can take a long time from an ICU commit to a fix rolling into
Fuchsia. This flag allows us to define the flag ahead of time in
//build/icu.gni, and remove the rollout issues.

**Current value (from the default):** `["-Wno-newline-eof", "-Wno-unnecessary-virtual-specifier"]`

From //build/icu.gni:38

### icu_fuchsia_extra_configs

Similar to above, except it allows adding an entire `config` target.

**Current value (from the default):** `[]`

From //build/icu.gni:47

### icu_fuchsia_override_data_dir

If set to nonempty, this is the label of the directory to be used to pull
the ICU data files content.  The setting has effect only when building
inside the Fuchsia source tree.

**Current value (from the default):** `""`

From //build/icu.gni:24

### icu_fuchsia_remove_configs

Similar to above, except it allows removing an entire `config` target, if
it exists.

**Current value (from the default):** `[]`

From //build/icu.gni:51

### icu_major_version_number

Contains the major version number of the ICU library, for dependencies that
need different configuration based on the library version. Currently this
is only useful in Fuchsia.

**Current value (from the default):** `"78"`

From //third_party/icu/default/version.gni:13

### icu_tzres_path

**Current value (from the default):** `"//prebuilt/third_party/tzres"`

From //src/lib/icu/tzdata/icu_tzres_source.gni:27

### icu_tzres_source

Which source location to use for ICU's time zone .res files:
"git" or "prebuilt".

If set to "git", then the tzres files will contain the same time zone
definitions as the ICU data monolith.

If set to "prebuilt", then the tzres files will be retrieved from CIPD
and may be newer than what's available in the ICU data monolith.

**Current value (from the default):** `"prebuilt"`

From //src/lib/icu/tzdata/icu_tzres_source.gni:16

### icu_use_data_file

Tells icu to load an external data file rather than rely on the icudata
being linked directly into the binary.

**Current value (from the default):** `true`

From //build/icu.gni:10

### icu_use_stub_data

If true, then this creates a stub data file. This should be disabled if
a custom data file will be used instead, in order to avoid conflicting
symbols.

**Current value (from the default):** `true`

From //build/icu.gni:15

### icu_use_target_out_dir

If set, the built libraries will live in their respective default output
directories, not the root_build_dir.

**Current value (from the default):** `true`

From //build/icu.gni:31

### idk_buildable_api_levels

The set of API levels for which this build will provide build-time
support in the IDK/SDK. The default set is all `supported` and
`in development` non-special API levels in //sdk/version_history.json.
Other valid values are a list containing a subset of the default set. If
empty, only targets for which the IDK contains artifacts built at "PLATFORM"
will be built.

This is useful for reducing the overall build time of any build that
includes the IDK/SDK in exchange for reduced coverage of API level support.
For example, `fx build //sdk:final_fuchsia_idk`.

To override the set of CPU architectures, see `idk_buildable_cpus`.

Do not use the `platform_version` member directly.
LINT.IfChange

**Current value (from the default):** `[27, 28, 29, 30, 31, 32, "NEXT"]`

From //build/config/fuchsia/platform_version.gni:44

### idk_buildable_cpus

The set of target CPU architectures for which the build will
provide build-time support in the IDK/SDK. The default set is
equivalent to `["arm64", "riscv64", "x64"]`. Other valid values are a list
containing a subset of that list that includes the current `target_cpu.

This is useful for reducing the overall build time of any build that
includes the IDK/SDK in exchange for reduced coverage of target CPU
architecture support. For example, `fx build //sdk:final_fuchsia_idk`.

To override the set of API levels, see `idk_buildable_api_levels`.
LINT.IfChange

**Current value (from the default):** `["arm64", "riscv64", "x64"]`

From //build/sdk/config.gni:72

### in_default_toolchain

Very unfortunately, GN never sets `current_toolchain` when executing
BUILDCONFIG.gn, so this variable is used to detect when this file
is executed in the context of the default toolchain.

It relies on any non-default toolchain to set it to false. It should
also never be set in `args.gn`.

LINT.IfChange(in_default_toolchain)

**Current value (from the default):** `true`

From //build/config/BUILDCONFIG.gn:57

### include_account_in_fvm

Include an account partition in the FVM image if set to true.

**Current value (from the default):** `false`

From //build/images/args.gni:44

### include_clippy

Turns rust targets into a group with both the normal target and clippy
target. This causes clippy targets to get included in the build. This gets
enabled by default with `fx set`, but is defaulted off in GN so it won't be
on in infra.

**Current value (from the default):** `false`

From //build/rust/rust_auxiliary_args.gni:17

### include_internal_fonts

Set to true to include internal fonts in the build.

**Current value (from the default):** `false`

From //src/fonts/build/font_args.gni:7

### include_rustdoc

Opt-in switch for .rustdoc subtargets. If `true`, respect per-target
`disable_rustdoc` setting. If `false`, do not define any rustdoc
subtargets.

**Current value (from the default):** `false`

From //build/rust/rust_auxiliary_args.gni:22

### include_zxdb_large_tests

Normally these tests are not built and run because they require large
amounts of optional data be downloaded. Set this to true to enable the build
for the zxdb_large_tests. See symbols/test_data/README.md for how to
download the data required for this test.

**Current value (from the default):** `false`

From //src/developer/debug/zxdb/BUILD.gn:13

### inet_config_enable_async_dns_sockets

Tells inet to support additionally support async dns sockets.

**Current value (from the default):** `true`

From //third_party/openweave-core/config.gni:17

### inet_want_endpoint_dns

Tells inet to include support for the corresponding protocol.

**Current value (from the default):** `true`

From //third_party/openweave-core/config.gni:10

### inet_want_endpoint_raw

**Current value (from the default):** `true`

From //third_party/openweave-core/config.gni:11

### inet_want_endpoint_tcp

**Current value (from the default):** `true`

From //third_party/openweave-core/config.gni:12

### inet_want_endpoint_tun

**Current value (from the default):** `true`

From //third_party/openweave-core/config.gni:14

### inet_want_endpoint_udp

**Current value (from the default):** `true`

From //third_party/openweave-core/config.gni:13

### integration_tests_verbose_logging

By default, log verbose font messages in tests.

**Current value (from the default):** `true`

From //src/fonts/tests/integration/BUILD.gn:20

### is_analysis

If set, the build will produce compilation analysis dumps, used for code
cross-referencing in code search.  The extra work done during analysis
is only needed for cross-referencing builds, so we're keeping the flag
and the analysis overhead turned off by default.

**Current value (from the default):** `false`

From //build/config/BUILDCONFIG.gn:30

### is_multi_product_build

**Current value (from the default):** `false`

From //build/images/args.gni:108

### is_perfetto_build_generator

All the tools/gen_* scripts set this to true. This is mainly used to locate
.gni files from //gn rather than //build.

**Current value (from the default):** `false`

From //third_party/perfetto/gn/perfetto.gni:94

### is_perfetto_embedder

This is for override via `gn args` (e.g. for tools/gen_xxx). Embedders
based on GN (e.g. v8) should NOT set this and instead directly sets
perfetto_build_with_embedder=true in their GN files.

**Current value (from the default):** `false`

From //third_party/perfetto/gn/perfetto.gni:99

### jdk_include_path

Path to a host JDK include directory that contains jni.h. When set,
the host-supported targets that build libperfetto_jni and its Java
proto codegen use this and its platform-specific subdirectory as
additional include paths. Empty (default) on Android / AOSP builds,
where jni.h comes from the NDK / jni_headers; populated by
tools/run_android_sdk_host_test from $JAVA_HOME.

**Current value (from the default):** `""`

From //third_party/perfetto/gn/perfetto.gni:228

### jtrace_enabled

Please refer to https://fuchsia.dev/fuchsia-src/development/debugging/jtrace
for a description of these configuration options.

Note that the special value "auto" is used only by the default definitions
of the entries (below).  It acts as a special value which automatically
chooses a default based on whether or not JTRACE is configured for
persistent tracing, while still allowing a user to explicitly override the
value regardless of whether persistent tracing is enabled or not.

**Current value (from the default):** `false`

From //zircon/kernel/params.gni:101

### jtrace_last_entry_storage

**Current value (from the default):** `0`

From //zircon/kernel/params.gni:102

### jtrace_target_buffer_size

**Current value (from the default):** `"auto"`

From //zircon/kernel/params.gni:103

### jtrace_use_large_entries

**Current value (from the default):** `"auto"`

From //zircon/kernel/params.gni:104

### jtrace_use_mono_timestamps

**Current value (from the default):** `false`

From //zircon/kernel/params.gni:105

### kernel_debug_print_level

Controls the verbosity of kernel dprintf messages. The higher the value,
the more dprintf messages emitted. Valid values are 0-2 (inclusive):
  0 - CRITCAL / ALWAYS
  1 - INFO
  2 - SPEW

**Current value (from the default):** `2`

From //zircon/kernel/params.gni:84

### kernel_extra_defines

Extra macro definitions for kernel code, e.g. "ENABLE_KERNEL_LL_DEBUG".

**Current value (from the default):** `[]`

From //zircon/kernel/params.gni:77

### kernel_memory_profiler

Enable stacktrace and statistics aggregation for each malloc and free.
`/boot/kernel/i/memory-profile/d/heap.bin` contains the binary data.
Use `ffx profile heapdump snapshot --kernel` to produce a symbolized
memory profile.

**Current value (from the default):** `false`

From //zircon/kernel/switch/BUILD.gn:29

### kernel_no_userabi

Build a kernel with no user-space support, for development only.

**Current value (from the default):** `false`

From //zircon/kernel/params.gni:108

### kernel_version_string

Version string embedded in the kernel for `zx_system_get_version_string`.
If set to the default "", a string is generated based on the
status of the fuchsia git repository.

**Current value (from the default):** `""`

From //zircon/kernel/BUILD.gn:19

### known_variants

List of variants that will form the basis for variant toolchains.
To make use of a variant, set [`select_variant`](#select_variant).

Normally this is not set as a build argument, but it serves to
document the available set of variants.
To add more, set [`extra_variants`](#extra_variants) instead.

Each element of the list is one variant, which is a scope defining:

  `configs` (optional)
      [list of labels] Each label names a config that will be
      automatically used by every target built in this variant.
      For each config `${label}`, there must also be a target
      `${label}_deps`, which each target built in this variant will
      automatically depend on.  The `variant()` template is the
      recommended way to define a config and its `_deps` target at
      the same time.

  `remove_common_configs` (optional)
  `remove_shared_configs` (optional)
      [list of labels] This list will be removed (with `-=`) from
      the `default_common_binary_configs` list (or the
      `default_shared_library_configs` list, respectively) after
      all other defaults (and this variant's configs) have been
      added.

  `deps` (optional)
      [list of labels] Added to the deps of every terminal target linked in
      this variant (as well as the automatic `${label}_deps` for
      each label in configs).

  `executable_deps`
      [list of labels] Added to the deps of every executable() target
      linked in this variant (as well as `deps`, above, and the automatic
      `${label}_deps` for each label in configs).

  `link_deps`
      [list of labels] Added to the deps of every linking target
      (i.e. terminal targets, plus shared_library() targets) linked in this
      variant (as well as `deps`, above, and the automatic `${label}_deps`
      for each label in configs).

  `source_deps`
      [list of labels] Added to the deps of every target that compiles
      `sources`.  This is added to source_set() and static_library()
      targets, and only this; the linking targets get both this and `deps`.

  `name` (required if configs is omitted)
      [string] Name of the variant as used in
      [`select_variant`](#select_variant) elements' `variant` fields.
      It's a good idea to make it something concise and meaningful when
      seen as e.g. part of a directory name under `$root_build_dir`.
      If name is omitted, configs must be nonempty and the simple names
      (not the full label, just the part after all `/`s and `:`s) of these
      configs will be used in toolchain names (each prefixed by a "-"),
      so the list of config names forming each variant must be unique
      among the lists in `known_variants + extra_variants`.

  `tags` (optional)
      [list of strings] A list of liberal strings describing properties
      of the toolchain instances created from this variant scope. See
      //build/toolchain/variant_tags.gni for the list of available
      values and their meaning.

  `toolchain_args` (optional)
      [scope] Each variable defined in this scope overrides a
      build argument in the toolchain context of this variant.

  `host_only` (optional)
  `target_only` (optional)
      [scope] This scope can contain any of the fields above.
      These values are used only for host or target, respectively.
      Any fields included here should not also be in the outer scope.


**Current value (from the default):**

```none
[{
  configs = ["//build/config/lto"]
  tags = ["lto"]
}, {
  configs = ["//build/config/lto:thinlto"]
  tags = ["lto"]
}, {
  name = "novariant"
}, {
  configs = ["//build/config/profile:coverage"]
  tags = ["coverage", "coverage_clang", "coverage_rust", "debugdata", "instrumented", "llvm-profdata", "needs-writable-globals"]
}, {
  configs = ["//build/config/profile:coverage-rust"]
  tags = ["coverage", "coverage_rust", "debugdata", "instrumented", "needs-writable-globals"]
}, {
  configs = ["//build/config/profile:coverage-cts"]
  tags = ["coverage", "coverage_clang", "debugdata", "instrumented", "llvm-profdata"]
}, {
  configs = ["//build/config/profile"]
  tags = ["debugdata", "instrumented", "llvm-profdata", "needs-writable-globals", "profile"]
}, {
  configs = ["//build/config/profile:profile-rust"]
  tags = ["debugdata", "instrumented", "needs-writable-globals", "profile"]
}, {
  configs = ["//build/config/profile:profile-use"]
  tags = ["instrumented"]
}, {
  configs = ["//build/config/profile:profile-use-rust"]
  tags = ["instrumented"]
}, {
  configs = ["//build/config/sanitizers:tsan"]
  tags = ["tsan", "instrumentation-runtime", "instrumented", "needs-compiler-abi", "needs-writable-globals", "uses-shadow", "kernel-excluded"]
}, {
  configs = ["//build/config/sanitizers:hwasan"]
  tags = ["hwasan", "instrumentation-runtime", "instrumented", "lsan", "needs-compiler-abi", "needs-writable-globals", "kernel-excluded", "replaces-allocator", "uses-shadow", "fuchsia-only"]
}, {
  configs = ["//build/config/sanitizers:hwasan", "//build/config/sanitizers:ubsan"]
  remove_common_configs = ["//build/config:no_rtti"]
  tags = ["hwasan", "instrumentation-runtime", "instrumented", "lsan", "needs-compiler-abi", "needs-writable-globals", "kernel-excluded", "replaces-allocator", "uses-shadow", "fuchsia-only", "ubsan"]
}, {
  configs = ["//build/config/sanitizers:ubsan"]
  remove_common_configs = ["//build/config:no_rtti"]
  tags = ["instrumented", "ubsan", "custom-runtime"]
}, {
  configs = ["//build/config/sanitizers:ubsan", "//build/config/sanitizers:sancov"]
  remove_common_configs = ["//build/config:no_rtti"]
  tags = ["instrumented", "instrumentation-runtime", "needs-compiler-abi", "needs-writable-globals", "kernel-excluded", "sancov", "ubsan"]
}, {
  configs = ["//build/config/sanitizers:asan"]
  host_only = {
  remove_shared_configs = ["//build/config:symbol_no_undefined"]
}
  remove_common_configs = ["//build/config:default_frame_pointers"]
  tags = ["asan", "instrumentation-runtime", "instrumented", "needs-compiler-abi", "needs-writable-globals", "lsan", "replaces-allocator", "uses-shadow", "kernel-excluded"]
  toolchain_args = { }
}, {
  configs = ["//build/config/sanitizers:asan", "//build/config/sanitizers:ubsan"]
  host_only = {
  remove_shared_configs = ["//build/config:symbol_no_undefined"]
}
  remove_common_configs = ["//build/config:default_frame_pointers", "//build/config:no_rtti"]
  tags = ["asan", "instrumentation-runtime", "instrumented", "needs-compiler-abi", "needs-writable-globals", "lsan", "replaces-allocator", "uses-shadow", "kernel-excluded", "ubsan"]
  toolchain_args = { }
}, {
  configs = ["//build/config/sanitizers:asan", "//build/config/sanitizers:sancov"]
  host_only = {
  remove_shared_configs = ["//build/config:symbol_no_undefined"]
}
  remove_common_configs = ["//build/config:default_frame_pointers"]
  tags = ["asan", "instrumentation-runtime", "instrumented", "needs-compiler-abi", "needs-writable-globals", "lsan", "replaces-allocator", "uses-shadow", "kernel-excluded", "sancov"]
  toolchain_args = { }
}, {
  configs = ["//build/config/sanitizers:asan", "//build/config:no-safe-stack"]
  host_only = {
  remove_shared_configs = ["//build/config:symbol_no_undefined"]
}
  name = "kasan"
  remove_common_configs = []
  tags = ["asan", "instrumentation-runtime", "instrumented", "needs-compiler-abi", "needs-writable-globals", "lsan", "replaces-allocator", "uses-shadow", "kernel-only"]
  toolchain_args = { }
}, {
  configs = ["//build/config/lto", "//build/config/sanitizers:cfi"]
  tags = ["lto", "custom-runtime", "cfi"]
}, {
  configs = ["//build/config/lto:thinlto", "//build/config/sanitizers:cfi"]
  tags = ["lto", "custom-runtime", "cfi"]
}, {
  configs = ["//build/config/sanitizers:asan", "//build/config:no-safe-stack", "//build/config/sanitizers:sancov"]
  host_only = {
  remove_shared_configs = ["//build/config:symbol_no_undefined"]
}
  name = "kasan-sancov"
  remove_common_configs = []
  tags = ["asan", "instrumentation-runtime", "instrumented", "needs-compiler-abi", "needs-writable-globals", "lsan", "replaces-allocator", "uses-shadow", "kernel-only", "sancov"]
  toolchain_args = { }
}, {
  configs = ["//build/config/sanitizers:asan", "//build/config/fuzzer", "//build/config/sanitizers:rust-asan", "//build/config:icf"]
  host_only = {
  remove_shared_configs = ["//build/config:symbol_no_undefined"]
}
  name = "asan-fuzzer"
  remove_common_configs = ["//build/config:default_frame_pointers", "//build/config:icf"]
  remove_shared_configs = ["//build/config:symbol_no_undefined"]
  tags = ["asan", "instrumentation-runtime", "instrumented", "needs-compiler-abi", "needs-writable-globals", "lsan", "replaces-allocator", "uses-shadow", "kernel-excluded", "fuzzer"]
  toolchain_args = {
  asan_default_options = "alloc_dealloc_mismatch=0:check_malloc_usable_size=0:detect_odr_violation=0:max_uar_stack_size_log=16:print_scariness=1:allocator_may_return_null=1:detect_leaks=0:detect_stack_use_after_return=1:malloc_context_size=128:print_summary=1:print_suppressions=0:strict_memcmp=0:symbolize=0"
}
}, {
  configs = ["//build/config/fuzzer", "//build/config/sanitizers:ubsan", "//build/config:icf"]
  name = "ubsan-fuzzer"
  remove_common_configs = ["//build/config:icf", "//build/config:no_rtti"]
  remove_shared_configs = ["//build/config:symbol_no_undefined"]
  tags = ["fuzzer", "instrumented", "instrumentation-runtime", "needs-compiler-abi", "ubsan"]
}, {
  name = "gcc"
  tags = ["gcc"]
}, {
  configs = ["//build/config/profile:heapdump"]
  tags = ["fuchsia-only", "heapdump", "instrumented", "instrumentation-runtime", "kernel-excluded", "needs-compiler-abi", "uses-shlibs"]
}]
```

From //build/config/BUILDCONFIG.gn:297

### link_rbe_check

Run one of the more expensive checks, intended for CI.
All of these require link_rbe_enable=true.

One of:

  * "none": No additional check.

  * "determinism":
      Check of determinism of linking by running locally twice
      and comparing outputs, failing if any differences are found.
      Even though this check doesn't involve RBE, it uses the same
      wrapper script, which knows what output files to expect and
      compare.

  * "consistency":
      Check consistency between local and remote link actions,
      by running both and comparing results.


**Current value (from the default):** `"none"`

From //build/toolchain/rbe.gni:286

### link_rbe_download_unstripped_outputs

**Current value (from the default):** `true`

From //build/toolchain/rbe.gni:302

### link_rbe_enable

Set to true to enable remote linking using RBE.
This covers actions that use `ar`, or use `clang` to drive
linkers like `lld`.

**Current value (from the default):** `false`

From //build/toolchain/rbe.gni:245

### link_rbe_exec_strategy

One of:

  * "remote": Execute action remotely on cache miss.
        The remote cache is always updated with this result.

  * "local": Lookup action in the remote cache, but execute action
        locally on cache miss.  The locally produced result is
        not uploaded to the remote cache.

  * "remote_local_fallback": Execute action remotely first.
        If that fails, run locally instead.  The locally produced
        results are not uploaded to the remote cache.

  * "racing": Race local vs. remote execution, take the first to finish.

  * "nocache": Force remote execution without using cached results.
        This can be useful for benchmarking cache-miss scenarios.

  (There are other rewrapper options that are not exposed.)

**Current value (from the default):** `"remote_local_fallback"`

From //build/toolchain/rbe.gni:266

### link_rbe_full_toolchain

reclient owns the logic for deciding what inputs are needed for
remote linking, but in some cases, it may fall behind
upstream toolchain development.
This option forces the *entire* toolchain directory to be included
as an input, which is generally guaranteed to work as it bears
no assumptions about how the toolchain works, but it comes at the
cost of performance overhead.
Use this primarily for debugging and as an emergency workaround.

**Current value (from the default):** `false`

From //build/toolchain/rbe.gni:296

### llvm_prefix

This directory contains the cipd packages for linux-x64, linux-arm64, and
mac-x64. Rather than using the prebuilts provided with the source tree, you
can download these individual packages from cipd and set this to the
directory containing those packages.

**Current value (from the default):** `"//prebuilt/third_party/llvm"`

From //src/lib/llvm/llvm_library.gni:10

### local_bench

Used to enable local benchmarking/fine-tuning when running benchmarks
in `fx shell`. Pass `--args=local_bench='true'` to `fx set` in order to
enable it.

**Current value (from the default):** `false`

From //src/developer/fuchsia-criterion/BUILD.gn:13

### lock_name_tracing_enabled

Enable lock names in durations emitted by the kernel:contention category.
This increases the per-lock size by a small amount to reference its tracing
label.

**Current value (from the default):** `false`

From //zircon/kernel/params.gni:27

### log_startup_sleep

**Current value (from the default):** `"30000"`

From //src/diagnostics/log_listener/BUILD.gn:14

### lsan_default_options

Default [LeakSanitizer](https://clang.llvm.org/docs/LeakSanitizer.html)
options (before the `LSAN_OPTIONS` environment variable is read at
runtime).  This can be set as a build argument to affect most "lsan"
variants in $variants (which see), or overridden in $toolchain_args in
one of those variants.  This can be a list of strings or a single string.

Note that even if this is empty, programs in this build **cannot** define
their own `__lsan_default_options` C function.  Instead, they can use a
sanitizer_extra_options() target in their `deps` and then any options
injected that way can override that option's setting in this list.

**Current value (from the default):** `[]`

From //build/config/sanitizers/sanitizer_default_options.gni:37

### magma_debug

**Current value (from the default):** `false`

From //src/graphics/magma/lib/magma/util/BUILD.gn:17

### magma_enable_tracing

Enable this to include fuchsia tracing capability

**Current value (from the default):** `true`

From //src/graphics/lib/magma/gnbuild/magma.gni:12

### magma_openvx_include

The path to OpenVX headers

**Current value (from the default):** `""`

From //src/graphics/lib/magma/gnbuild/magma.gni:15

### magma_openvx_package

The path to an OpenVX implementation

**Current value (from the default):** `""`

From //src/graphics/lib/magma/gnbuild/magma.gni:18

### main_pb_label

Label pointing to the main product bundle to work with if the default
product in a multi-product build is not desired.

**Current value (from the default):** `""`

From //build/images/args.gni:79

### max_build_concurrency

Upper bound on the build concurrency (-j) that `fx build` picks
automatically. Zero means no upper bound.

The automatic value is derived from the host CPU count, which assumes the
CPU is the scarcest resource. Environments where that does not hold can
lower it here; for example a FUSE-backed workspace exhausts file
descriptors long before it saturates a high-core host.

An explicit -j on the command line is always honored as-is.

**Current value (from the default):** `0`

From //build/config/BUILDCONFIG.gn:47

### max_log_disk_usage

Controls how many bytes of space on disk are used to persist device logs.
Should be a string value that only contains digits.

**Current value (from the default):** `"0"`

From //src/diagnostics/log_listener/BUILD.gn:13

### mbedtls_config_file

Configuration file for MbedTLS.

**Current value (from the default):** `"mbedtls-config.h"`

From //third_party/openthread/third_party/mbedtls/BUILD.gn:57

### mbedtls_deps

Extra dependencies for MbedTLS

**Current value (from the default):** `["../../src/core:libopenthread_core_config"]`

From //third_party/openthread/third_party/mbedtls/BUILD.gn:60

### min_crashlog_size

Controls minimum amount of space of persistent RAM to reserve for the
crashlog.  When other features (such as persistent debug logging) are
enabled, this value controls the minimum number of bytes which will
_always_ be reserved for the crashlog (subject to the total amount of
available persistent RAM), regardless of how much ram is requested by other
users of persistent RAM.  Must be a multiple of 128 bytes.

**Current value (from the default):** `2048`

From //zircon/kernel/lib/crashlog/params.gni:14

### mini_chromium_is_chromeos_ash

**Current value (from the default):** `false`

From //third_party/mini_chromium/src/build/platform.gni:32

### mini_chromium_is_chromeos_lacros

**Current value (from the default):** `false`

From //third_party/mini_chromium/src/build/platform.gni:31

### monolithic_binaries

**Current value (from the default):** `false`

From //third_party/perfetto/gn/perfetto.gni:250

### msd_arm_enable_all_cores

Enable all 8 cores, which is faster but emits more heat.

**Current value (from the default):** `true`

From //src/graphics/drivers/msd-arm-mali/src/BUILD.gn:9

### msd_arm_enable_cache_coherency

With this flag set the system tries to use cache coherent memory if the
GPU supports it.

**Current value (from the default):** `true`

From //src/graphics/drivers/msd-arm-mali/src/BUILD.gn:13

### msd_arm_enable_protected_debug_swap_mode

In protected mode, faults don't return as much information so they're much
harder to debug. To work around that, add a mode where protected atoms are
executed in non-protected mode and vice-versa.

NOTE: The memory security ranges should also be set (in TrustZone) to the
opposite of normal, so that non-protected mode accesses can only access
protected memory and vice versa. Also, growable memory faults won't work in
this mode, so larger portions of growable memory should precommitted (which
is not done by default).

**Current value (from the default):** `false`

From //src/graphics/drivers/msd-arm-mali/src/BUILD.gn:24

### msd_arm_mali_csf_firmware_label

Label of the firmware target for msd-arm-mali-csf.
If empty, no firmware is included in the driver package.

**Current value (from the default):** `""`

From //src/graphics/drivers/msd-arm-mali-csf/BUILD.gn:13

### netstack3_profile_rustc

Adds compilation flags to emit rustc self-profiling when building netstack3
targets. That helps us track down where time is spent and memory
consumption to play nice with RBE.

**Current value (from the default):** `false`

From //src/connectivity/network/netstack3/BUILD.gn:9

### ninja_implicit_inputs_root_labels

A set of target labels that will be used to generate list of
Ninja implicit inputs through GN metadata collection. These need
to be exposed to guarantee the incremental correctness of Ninja
subbuilds invoker from Bazel. For more details, read
//build/ninja_implicit_inputs/README.md

**Current value (from the default):** `["//:default"]`

From //build/ninja_implicit_inputs/config.gni:11

### ninja_upload_build_events

Configure ninja to stream build events and results to a service.
This is useful for sharing build results and invocation details
for reproducing and triaging issues.
This option uses direct network access and requires authentication.
The _infra variants are intended for use in build infrastructure.
Similar configuration for bazel lives in //build/bazel/remote_services.gni.
Note: uploading to Sponge is not yet supported.

Valid options:
  "": do not stream (default)
  "resultstore": uploads to ResultStore (for users)
  "resultstore_infra": uploads to ResultStore (for infra)

**Current value (from the default):** `""`

From //build/toolchain/resultstore.gni:18

### openthread_config_anycast_locator_enable

Enable anycast locator functionality

**Current value (from the default):** `false`

From //third_party/openthread/etc/gn/openthread.gni:82

### openthread_config_assert_enable

Enable assertions.

**Current value (from the default):** `true`

From //third_party/openthread/etc/gn/openthread.gni:79

### openthread_config_backbone_router_enable

Enable backbone router functionality

**Current value (from the default):** `false`

From //third_party/openthread/etc/gn/openthread.gni:85

### openthread_config_ble_tcat_enable

Enable BLE based commissioning functionality

**Current value (from the default):** `false`

From //third_party/openthread/etc/gn/openthread.gni:88

### openthread_config_border_agent_enable

Enable border agent support

**Current value (from the default):** `false`

From //third_party/openthread/etc/gn/openthread.gni:91

### openthread_config_border_agent_id_enable

Enable border agent ID

**Current value (from the default):** `false`

From //third_party/openthread/etc/gn/openthread.gni:94

### openthread_config_border_router_enable

Enable border router support

**Current value (from the default):** `false`

From //third_party/openthread/etc/gn/openthread.gni:97

### openthread_config_border_routing_enable

Enable border routing support

**Current value (from the default):** `false`

From //third_party/openthread/etc/gn/openthread.gni:100

### openthread_config_channel_manager_enable

Enable channel manager support

**Current value (from the default):** `false`

From //third_party/openthread/etc/gn/openthread.gni:103

### openthread_config_channel_monitor_enable

Enable channel monitor support

**Current value (from the default):** `false`

From //third_party/openthread/etc/gn/openthread.gni:106

### openthread_config_child_supervision_enable

Enable child supervision support

**Current value (from the default):** `false`

From //third_party/openthread/etc/gn/openthread.gni:109

### openthread_config_coap_api_enable

Enable coap api support

**Current value (from the default):** `false`

From //third_party/openthread/etc/gn/openthread.gni:112

### openthread_config_coap_observe_api_enable

Enable coap observe (RFC7641) api support

**Current value (from the default):** `false`

From //third_party/openthread/etc/gn/openthread.gni:118

### openthread_config_coap_secure_api_enable

Enable secure coap api support

**Current value (from the default):** `false`

From //third_party/openthread/etc/gn/openthread.gni:115

### openthread_config_coexistence_enable

Enable radio coexistence

**Current value (from the default):** `false`

From //third_party/openthread/etc/gn/openthread.gni:245

### openthread_config_commissioner_enable

Enable commissioner support

**Current value (from the default):** `false`

From //third_party/openthread/etc/gn/openthread.gni:121

### openthread_config_deps

Extra deps for OpenThread configuration.

**Current value (from the default):** `[]`

From //third_party/openthread/etc/gn/openthread.gni:38

### openthread_config_dhcp6_client_enable

Enable DHCP6 client support

**Current value (from the default):** `false`

From //third_party/openthread/etc/gn/openthread.gni:127

### openthread_config_dhcp6_server_enable

Enable DHCP6 server support

**Current value (from the default):** `false`

From //third_party/openthread/etc/gn/openthread.gni:130

### openthread_config_diag_enable

Enable diagnostic support

**Current value (from the default):** `false`

From //third_party/openthread/etc/gn/openthread.gni:133

### openthread_config_dns_client_enable

Enable DNS client support

**Current value (from the default):** `false`

From //third_party/openthread/etc/gn/openthread.gni:136

### openthread_config_dnssd_server_enable

Enable DNS-SD server support

**Current value (from the default):** `false`

From //third_party/openthread/etc/gn/openthread.gni:139

### openthread_config_ecdsa_enable

Enable ECDSA support

**Current value (from the default):** `false`

From //third_party/openthread/etc/gn/openthread.gni:142

### openthread_config_enable_builtin_mbedtls_management

**Current value (from the default):** `true`

From //third_party/openthread/etc/gn/openthread.gni:242

### openthread_config_file

OpenThread config header.

**Current value (from the default):** `"<openthread-config-fuchsia.h>"`

From //third_party/openthread/etc/gn/openthread.gni:35

### openthread_config_full_logs

Enable full logs

**Current value (from the default):** `false`

From //third_party/openthread/etc/gn/openthread.gni:229

### openthread_config_heap_external_enable

Enable external heap support

**Current value (from the default):** `true`

From //third_party/openthread/etc/gn/openthread.gni:148

### openthread_config_ip6_fragmentation_enable

Enable ipv6 fragmentation support

**Current value (from the default):** `false`

From //third_party/openthread/etc/gn/openthread.gni:151

### openthread_config_ip6_slaac_enable

Enable support for adding of auto-configured SLAAC addresses by OpenThread

**Current value (from the default):** `false`

From //third_party/openthread/etc/gn/openthread.gni:205

### openthread_config_jam_detection_enable

Enable jam detection support

**Current value (from the default):** `false`

From //third_party/openthread/etc/gn/openthread.gni:154

### openthread_config_joiner_enable

Enable joiner support

**Current value (from the default):** `false`

From //third_party/openthread/etc/gn/openthread.gni:157

### openthread_config_legacy_enable

Enable legacy network support

**Current value (from the default):** `false`

From //third_party/openthread/etc/gn/openthread.gni:160

### openthread_config_link_metrics_initiator_enable

Enable link metrics initiator

**Current value (from the default):** `false`

From //third_party/openthread/etc/gn/openthread.gni:163

### openthread_config_link_metrics_subject_enable

Enable link metrics subject

**Current value (from the default):** `false`

From //third_party/openthread/etc/gn/openthread.gni:166

### openthread_config_link_raw_enable

Enable link raw service

**Current value (from the default):** `false`

From //third_party/openthread/etc/gn/openthread.gni:169

### openthread_config_log_level_dynamic_enable

Enable dynamic log level control

**Current value (from the default):** `false`

From //third_party/openthread/etc/gn/openthread.gni:172

### openthread_config_log_output

Log output: none, debug_uart, app, platform

**Current value (from the default):** `""`

From //third_party/openthread/etc/gn/openthread.gni:76

### openthread_config_mac_csl_receiver_enable

Enable csl receiver

**Current value (from the default):** `false`

From //third_party/openthread/etc/gn/openthread.gni:124

### openthread_config_mac_filter_enable

Enable mac filter support

**Current value (from the default):** `false`

From //third_party/openthread/etc/gn/openthread.gni:175

### openthread_config_message_use_heap

Enable use built-in heap for message buffers

**Current value (from the default):** `false`

From //third_party/openthread/etc/gn/openthread.gni:178

### openthread_config_mle_long_routes_enable

Enable MLE long routes extension (experimental, breaks Thread conformance]

**Current value (from the default):** `false`

From //third_party/openthread/etc/gn/openthread.gni:181

### openthread_config_mlr_enable

Enable Multicast Listener Registration feature for Thread 1.2

**Current value (from the default):** `false`

From //third_party/openthread/etc/gn/openthread.gni:145

### openthread_config_multiple_instance_enable

Enable multiple instances

**Current value (from the default):** `false`

From //third_party/openthread/etc/gn/openthread.gni:187

### openthread_config_ncp_hdlc_enable

Enable NCP HDLC support

**Current value (from the default):** `false`

From //third_party/openthread/etc/gn/openthread.gni:238

### openthread_config_ncp_spi_enable

Enable NCP SPI support

**Current value (from the default):** `false`

From //third_party/openthread/etc/gn/openthread.gni:235

### openthread_config_otns_enable

Enable OTNS support

**Current value (from the default):** `false`

From //third_party/openthread/etc/gn/openthread.gni:232

### openthread_config_ping_sender

Enable ping sender support

**Current value (from the default):** `false`

From //third_party/openthread/etc/gn/openthread.gni:220

### openthread_config_platform_netif_enable

Enable platform netif support

**Current value (from the default):** `false`

From //third_party/openthread/etc/gn/openthread.gni:190

### openthread_config_platform_udp_enable

Enable platform UDP support

**Current value (from the default):** `false`

From //third_party/openthread/etc/gn/openthread.gni:193

### openthread_config_reference_device_enable

Enable Thread Test Harness reference device support

**Current value (from the default):** `false`

From //third_party/openthread/etc/gn/openthread.gni:196

### openthread_config_sntp_client_enable

Enable SNTP Client support

**Current value (from the default):** `false`

From //third_party/openthread/etc/gn/openthread.gni:208

### openthread_config_srp_client_enable

Enable SRP Client support

**Current value (from the default):** `false`

From //third_party/openthread/etc/gn/openthread.gni:211

### openthread_config_srp_server_enable

Enable SRP Server support

**Current value (from the default):** `false`

From //third_party/openthread/etc/gn/openthread.gni:214

### openthread_config_tcp_enable

Enable support for TCP

**Current value (from the default):** `true`

From //third_party/openthread/etc/gn/openthread.gni:217

### openthread_config_thread_version

Thread version

**Current value (from the default):** `"1.4"`

From //third_party/openthread/etc/gn/openthread.gni:73

### openthread_config_time_sync_enable

Enable the time synchronization service feature

**Current value (from the default):** `false`

From //third_party/openthread/etc/gn/openthread.gni:223

### openthread_config_tmf_netdata_service_enable

Enable support for injecting Service entries into the Thread Network Data

**Current value (from the default):** `false`

From //third_party/openthread/etc/gn/openthread.gni:199

### openthread_config_tmf_netdiag_client_enable

Enable TMF network diagnostics client

**Current value (from the default):** `false`

From //third_party/openthread/etc/gn/openthread.gni:184

### openthread_config_udp_forward_enable

Enable UDP forward support

**Current value (from the default):** `false`

From //third_party/openthread/etc/gn/openthread.gni:226

### openthread_core_config_deps

Extra deps for OpenThread core configuration.

**Current value (from the default):** `[]`

From //third_party/openthread/etc/gn/openthread.gni:50

### openthread_core_config_platform_check_file

OpenThread platform-specific config check header

**Current value (from the default):** `""`

From //third_party/openthread/etc/gn/openthread.gni:47

### openthread_enable_core_config_args

Configure OpenThread via GN arguments.

**Current value (from the default):** `true`

From //third_party/openthread/etc/gn/openthread.gni:67

### openthread_external_mbedtls

Use external mbedtls. If blank, internal mbedtls will be used.

**Current value (from the default):** `""`

From //third_party/openthread/etc/gn/openthread.gni:56

### openthread_external_platform

Use external platform.

**Current value (from the default):** `""`

From //third_party/openthread/etc/gn/openthread.gni:53

### openthread_package_name

Package name for OpenThread.

**Current value (from the default):** `"OPENTHREAD"`

From //third_party/openthread/etc/gn/openthread.gni:59

### openthread_package_version

Package version for OpenThread.

**Current value (from the default):** `"1.0.0"`

From //third_party/openthread/etc/gn/openthread.gni:62

### openthread_project_core_config_file

OpenThread project-specific core config header

**Current value (from the default):** `""`

From //third_party/openthread/etc/gn/openthread.gni:44

### openthread_project_include_dirs

Include directories for project specific configs.

**Current value (from the default):** `[]`

From //third_party/openthread/etc/gn/openthread.gni:41

### openthread_settings_ram

Enable volatile-only storage of settings

**Current value (from the default):** `false`

From //third_party/openthread/etc/gn/openthread.gni:202

### optimize

Global override for the "foo_optimize" args.

**Current value (from the default):** `false`

From //build/config/compiler.gni:10

### output_breakpad_syms

Sets if we should output breakpad symbols for Fuchsia binaries.

**Current value (from the default):** `false`

From //build/config/BUILDCONFIG.gn:33

### output_gsym

Controls whether we should output GSYM files for Fuchsia binaries.

**Current value (from the default):** `false`

From //build/config/BUILDCONFIG.gn:36

### package_flavor_selections

Used to configure the set of package flavors desired.

Usage:

 package_flavor_selections = [
   {
     name = "snazzy"
     flavor = "with_hooks"
   },
   {
     name = "some_other_package"
     flavor = "some_other_flavor"
   },
 ]

The above specifies that the package "snazzy" should use the
"with_hooks" flavor, and that "some_other_package" should use
the "some_other_flavor" flavor instead of their default flavor
all other packages using this template would use their default
package flavors.

**Current value (from the default):** `[]`

From //build/packages/prebuilt_package_with_flavors.gni:30

### perfetto_amalgamated_sdk

Set by the amalgamated SDK build. Defaults the optional zlib/re2 features
to off; the consumer opts in via perfetto_sdk_config.h (see build_config.h).

**Current value (from the default):** `false`

From //third_party/perfetto/gn/perfetto.gni:504

### perfetto_build_with_android

The Android blueprint file generator set this to true (as well as
is_perfetto_build_generator). This is just about being built in the
Android tree (AOSP and internal) and is NOT related with the target OS.
In standalone Android builds and Chromium Android builds, this is false.

**Current value (from the default):** `false`

From //third_party/perfetto/gn/perfetto.gni:90

### perfetto_enable_git_rev_version_header

**Current value (from the default):** `false`

From //third_party/perfetto/gn/perfetto.gni:328

### perfetto_force_dcheck

Whether DCHECKs should be enabled or not. Values: "on" | "off" | "".
By default ("") DCHECKs are enabled only:
- If DCHECK_ALWAYS_ON is defined (which is mainly a Chromium-ism).
- On debug builds (i.e. if NDEBUG is NOT defined) but only in Chromium,
  Android and standalone builds.
- On all other builds (e.g., SDK) it's off regardless of NDEBUG (unless
  DCHECK_ALWAYS_ON is defined).
See base/logging.h for the implementation of all this.

**Current value (from the default):** `""`

From //third_party/perfetto/gn/perfetto.gni:265

### perfetto_force_dlog

Whether DLOG should be enabled on debug builds (""), all builds ("on"), or
none ("off"). We disable it by default for embedders to avoid spamming their
console.

**Current value (from the default):** `""`

From //third_party/perfetto/gn/perfetto.gni:255

### perfetto_http_additional_cors_origins

Comma-separated list of additional CORS origins to allow in trace_processor
and websocket_bridge.

**Current value (from the default):** `""`

From //third_party/perfetto/gn/perfetto.gni:103

### perfetto_thread_safety_annotations

**Current value (from the default):** `false`

From //third_party/perfetto/gn/perfetto.gni:284

### perfetto_use_pkgconfig

Used by CrOS builds. Uses pkg-config to determine the appropriate flags
for including and linking system libraries.
  set `host_pkg_config` to the `BUILD_PKG_CONFIG` and
  set `pkg_config` to the target `PKG_CONFIG`.
Note: that if this is enabled `perfetto_use_system_protobuf` should be also.

**Current value (from the default):** `false`

From //third_party/perfetto/gn/perfetto.gni:474

### perfetto_use_system_expat

Used by NixOS system builds. Uses the system version of expat
from pkg-config instead of the hermetic one.

**Current value (from the default):** `false`

From //third_party/perfetto/gn/perfetto.gni:478

### perfetto_use_system_linenoise

Used by NixOS system builds. Uses the system version of linenoise
from pkg-config instead of the hermetic one.

**Current value (from the default):** `false`

From //third_party/perfetto/gn/perfetto.gni:482

### perfetto_use_system_protobuf

Used by CrOS system builds. Uses the system version of protobuf
from /usr/include instead of the hermetic one.

**Current value (from the default):** `false`

From //third_party/perfetto/gn/perfetto.gni:486

### perfetto_use_system_re2

Used by NixOS system builds. Uses the system version of re2
from pkg-config instead of the hermetic one.

**Current value (from the default):** `false`

From //third_party/perfetto/gn/perfetto.gni:490

### perfetto_use_system_sqlite

Used by CrOS system builds. Uses the system version of sqlite
from /usr/include instead of the hermetic one.

**Current value (from the default):** `false`

From //third_party/perfetto/gn/perfetto.gni:494

### perfetto_use_system_zlib

**Current value (from the default):** `false`

From //third_party/perfetto/gn/perfetto.gni:496

### perfetto_use_system_zstd

Used by NixOS system builds. Uses the system version of zstd
from pkg-config instead of the hermetic one.

**Current value (from the default):** `false`

From //third_party/perfetto/gn/perfetto.gni:500

### perfetto_verbose_logs_enabled

**Current value (from the default):** `true`

From //third_party/perfetto/gn/perfetto.gni:347

### persistent_ram_allocation_granularity

Controls the granularity of allocation of the global pool of persistent RAM.
All features which wish to use persistent RAM to preserve data across reboot
must operate in allocations which are a multiple of this value.  The value
should be a power of two, and typically should be a multiple of the
cacheline size of the target architecture.

**Current value (from the default):** `128`

From //zircon/kernel/params.gni:91

### pgo_profile_path

Profile data path that is used by PGO.

**Current value (from the default):** `""`

From //build/config/profile/config.gni:47

### prebuilt_dart_sdk

Directory containing prebuilt Dart SDK.
This must have in its `bin/` subdirectory `gen_snapshot.OS-CPU` binaries.

**Current value (from the default):** `"//prebuilt/third_party/dart/linux-x64"`

From //build/dart/dart.gni:8

### prebuilt_fastboot

**Current value (from the default):** `"//prebuilt/third_party/fastboot/fastboot"`

From //build/images/tools/fastboot.gni:6

### prebuilt_go_dir

  prebuilt_go_dir
    [string] points to the directory containing the prebuilt host go
    binary. By default, this points to the //prebuilts directory.

**Current value (from the default):** `"//prebuilt/third_party/go/linux-x64"`

From //build/go/go_build.gni:26

### product_assembly_overrides

This GN arg enables developer overrides for the given assembly targets

This is a list of scopes that take two fields:
 - assembly: (GN label pattern) the GN label(s) to apply the overrides to
 - overrides (GN label) the label of a set of developer overrides

Example:

 product_assembly_overrides = [
   {
     assembly = "//products/core/*"
     overrides = "//local:my_assembly_overrides"
   }
 ]

**Current value for `target_cpu = "arm64"`:** `[]`

From //out/not-default/args.gn:17

**Overridden from the default:** `[]`

From //build/assembly/developer_overrides.gni:516

**Current value for `target_cpu = "riscv64"`:** `[]`

From //out/not-default/args.gn:17

**Overridden from the default:** `[]`

From //build/assembly/developer_overrides.gni:516

**Current value for `target_cpu = "x64"`:** `[]`

From //out/not-default/args.gn:17

**Overridden from the default:** `[]`

From //build/assembly/developer_overrides.gni:516

### product_assembly_overrides_contents

This GN arg allows the overrides template to be specified in-line within
args.gn. It is incompatible with the above
'product_assembly_overrides_label' argument.

To use this, treat it like an 'assembly_developer_overrides()' template, and
the corresponding template will be instantiated at
`//build/assembly/overrides:inlined`, and set as the overrides for the
"main" product assembly as if the following were set:

  product_assembly_overrides_label = "//build/assembly/overrides:inlined"


**Current value (from the default):** `false`

From //build/assembly/developer_overrides.gni:535

### product_assembly_overrides_label

This GN arg provides a short-hand mechanism for setting the developer
overrides used by the "main" product assembly for a product. If this is set,
and there isn't a "main" product assembly defined, then a GN error will be
generated.

**Current value (from the default):** `false`

From //build/assembly/developer_overrides.gni:522

### product_bundle_labels

Labels for product bundles to assemble in addition to the main product
bundle.

**Current value (from the default):** `[]`

From //BUILD.gn:137

### profile_source_files

List of paths to source files to be instrumented by `profile` variants.

**Current value (from the default):** `["//*"]`

From //build/config/profile/config.gni:10

### profile_source_files_list_files

List of paths to files in Clang's `-fprofile-list` format describing files
and functions to be instrumented by `profile` variants.

**Current value (from the default):** `[]`

From //build/config/profile/config.gni:44

### proprietary_codecs

**Current value (from the default):** `false`

From //build/config/features.gni:9

### pw_assert_BACKEND

Backend for the pw_assert module's CHECK facade.

**Current value for `target_cpu = "arm64"`:** `"//third_party/pigweed/backends/pw_assert"`

From //.gn:76

**Overridden from the default:** `""`

From //third_party/pigweed/src/pw_assert/backend.gni:19

**Current value for `target_cpu = "riscv64"`:** `"//third_party/pigweed/backends/pw_assert"`

From //.gn:76

**Overridden from the default:** `""`

From //third_party/pigweed/src/pw_assert/backend.gni:19

**Current value for `target_cpu = "x64"`:** `"//third_party/pigweed/backends/pw_assert"`

From //.gn:76

**Overridden from the default:** `""`

From //third_party/pigweed/src/pw_assert/backend.gni:19

### pw_assert_CONFIG

The build target that overrides the default configuration options for this
module. This should point to a source set that provides defines through a
public config (which may -include a file or add defines directly).

**Current value (from the default):** `"//third_party/pigweed/src/pw_build:empty"`

From //third_party/pigweed/src/pw_assert/BUILD.gn:26

### pw_assert_LITE_BACKEND

Backend for the pw_assert module's ASSERT facade.

Warning: This naming is transitional. Modifying this build argument WILL
    result in future breakages. (b/235149326)

**Current value (from the default):** `"//third_party/pigweed/src/pw_assert:assert_compatibility_backend"`

From //third_party/pigweed/src/pw_assert/backend.gni:25

### pw_async2_DISPATCHER_FOR_TEST_BACKEND

**Current value (from the default):** `"//third_party/pigweed/src/pw_async2:basic_dispatcher_for_test"`

From //third_party/pigweed/src/pw_async2/backend.gni:29

### pw_bloat_BLOATY_CONFIG

Path to the Bloaty configuration file that defines the memory layout and
capacities for the target binaries.

**Current value (from the default):** `""`

From //third_party/pigweed/src/pw_bloat/bloat.gni:23

### pw_bloat_SHOW_SIZE_REPORTS

Controls whether to display size reports in the build output.

**Current value (from the default):** `false`

From //third_party/pigweed/src/pw_bloat/bloat.gni:40

### pw_bloat_TOOLCHAINS

List of toolchains to use in pw_toolchain_size_diff templates.

Each entry is a scope containing the following variables:

  name: Human-readable toolchain name.
  target: GN target that defines the toolchain.
  linker_script: Optional path to a linker script file to build for the
    toolchain's target.
  bloaty_config: Optional Bloaty confirugation file defining the memory
    layout of the binaries as specified in the linker script.

If this list is empty, pw_toolchain_size_diff targets become no-ops.

**Current value (from the default):** `[]`

From //third_party/pigweed/src/pw_bloat/bloat.gni:37

### pw_bluetooth_CONFIG

The build target that overrides the default configuration options for this
module. This should point to a source set that provides defines through a
public config (which may -include a file or add defines directly).

**Current value (from the default):** `"//third_party/pigweed/src/pw_build:empty"`

From //third_party/pigweed/src/pw_bluetooth/BUILD.gn:30

### pw_build_COLORIZE_OUTPUT

Controls whether compilers and other tools are told to use colorized output.

**Current value (from the default):** `true`

From //third_party/pigweed/src/pw_build/defaults.gni:61

### pw_build_DEFAULT_MODULE_CONFIG

The default implementation for all Pigweed module configurations.

This variable makes it possible to configure multiple Pigweed modules from
a single GN target. Module configurations can still be overridden
individually by setting a module's config backend directly (e.g.
pw_some_module_CONFIG = "//my_config").

Modules are configured through compilation options. The configuration
implementation might set individual compiler macros or forcibly include a
config header with multiple options using the -include flag.

**Current value (from the default):** `"//third_party/pigweed/src/pw_build:empty"`

From //third_party/pigweed/src/pw_build/module_config.gni:28

### pw_build_DEFAULT_VISIBILITY

Controls the default visibility of C/C++ libraries and executables
(pw_source_set, pw_static_library, pw_shared_library pw_executable). This
can be "*" or a list of paths.

This is useful for limiting usage of Pigweed modules via an explicit
allowlist. For the GN build to work, pw_build_DEFAULT_VISIBILITY must always
at least include the Pigweed repository ("$dir_pigweed/*").

Explicitly setting a target's visibility overrides this default.

**Current value (from the default):** `["*"]`

From //third_party/pigweed/src/pw_build/defaults.gni:58

### pw_build_EXECUTABLE_TARGET_TYPE

The name of the GN target type used to build Pigweed executables.

If this is a custom template, the .gni file containing the template must
be imported at the top of the target configuration file to make it globally
available.

**Current value (from the default):** `"executable"`

From //third_party/pigweed/src/pw_build/gn_internal/build_target.gni:39

### pw_build_EXECUTABLE_TARGET_TYPE_FILE

The path to the .gni file that defines pw_build_EXECUTABLE_TARGET_TYPE.

If pw_build_EXECUTABLE_TARGET_TYPE is not the default of `executable`, this
.gni file is imported to provide the template definition.

**Current value (from the default):** `""`

From //third_party/pigweed/src/pw_build/gn_internal/build_target.gni:45

### pw_build_LINK_DEPS

Additional build targets to add as dependencies for pw_executable,
pw_static_library, and pw_shared_library targets. The
$dir_pw_build:link_deps target pulls in these libraries.

pw_build_LINK_DEPS can be used to break circular dependencies for low-level
libraries such as pw_assert.

**Current value for `target_cpu = "arm64"`:** `["//third_party/pigweed/src/pw_assert:impl", "//third_party/pigweed/src/pw_log:impl"]`

From //.gn:98

**Overridden from the default:** `[]`

From //third_party/pigweed/src/pw_build/gn_internal/build_target.gni:26

**Current value for `target_cpu = "riscv64"`:** `["//third_party/pigweed/src/pw_assert:impl", "//third_party/pigweed/src/pw_log:impl"]`

From //.gn:98

**Overridden from the default:** `[]`

From //third_party/pigweed/src/pw_build/gn_internal/build_target.gni:26

**Current value for `target_cpu = "x64"`:** `["//third_party/pigweed/src/pw_assert:impl", "//third_party/pigweed/src/pw_log:impl"]`

From //.gn:98

**Overridden from the default:** `[]`

From //third_party/pigweed/src/pw_build/gn_internal/build_target.gni:26

### pw_build_PIP_CONSTRAINTS

**Current value (from the default):** `["//third_party/pigweed/src/pw_env_setup/py/pw_env_setup/virtualenv_setup/constraint.list"]`

From //third_party/pigweed/src/pw_build/python.gni:27

### pw_build_PIP_REQUIREMENTS

Default pip requirements file for all Pigweed based projects.

**Current value (from the default):** `[]`

From //third_party/pigweed/src/pw_build/python.gni:30

### pw_build_PYLINT_OUTPUT_FORMAT

Output format for pylint. Options include "text" and "colorized".

**Current value (from the default):** `"colorized"`

From //third_party/pigweed/src/pw_build/python.gni:47

### pw_build_PYTHON_BUILD_VENV

Default gn build virtualenv target.

**Current value for `target_cpu = "arm64"`:** `"//third_party/pigweed:fuchsia_pigweed_python_venv"`

From //.gn:126

**Overridden from the default:** `"//third_party/pigweed/src/pw_env_setup:pigweed_build_venv"`

From //third_party/pigweed/src/pw_build/python_gn_args.gni:23

**Current value for `target_cpu = "riscv64"`:** `"//third_party/pigweed:fuchsia_pigweed_python_venv"`

From //.gn:126

**Overridden from the default:** `"//third_party/pigweed/src/pw_env_setup:pigweed_build_venv"`

From //third_party/pigweed/src/pw_build/python_gn_args.gni:23

**Current value for `target_cpu = "x64"`:** `"//third_party/pigweed:fuchsia_pigweed_python_venv"`

From //.gn:126

**Overridden from the default:** `"//third_party/pigweed/src/pw_env_setup:pigweed_build_venv"`

From //third_party/pigweed/src/pw_build/python_gn_args.gni:23

### pw_build_PYTHON_PIP_DEFAULT_OPTIONS

General options passed to pip commands
https://pip.pypa.io/en/stable/cli/pip/#general-options

**Current value (from the default):** `["--disable-pip-version-check"]`

From //third_party/pigweed/src/pw_build/python_gn_args.gni:51

### pw_build_PYTHON_PIP_DOWNLOAD_ALL_PLATFORMS

DOCSTAG: [default-pip-gn-args]
Set pw_python_venv.vendor_wheel targets to download Python packages for all
platform combinations. This takes a significant amount of time.

**Current value (from the default):** `false`

From //third_party/pigweed/src/pw_build/python_gn_args.gni:28

### pw_build_PYTHON_PIP_INSTALL_DISABLE_CACHE

Adds '--no-cache-dir' forcing pip to ignore any previously cached Python
packages. On most systems this is located in ~/.cache/pip/

**Current value (from the default):** `false`

From //third_party/pigweed/src/pw_build/python_gn_args.gni:41

### pw_build_PYTHON_PIP_INSTALL_FIND_LINKS

List of paths to folders containing Python wheels (*.whl) or source tar
files (*.tar.gz). Pip will check each of these directories when looking for
potential install candidates. Each path will be passed to all 'pip install'
commands as '--find-links PATH'.

**Current value (from the default):** `[]`

From //third_party/pigweed/src/pw_build/python_gn_args.gni:47

### pw_build_PYTHON_PIP_INSTALL_OFFLINE

Adds --no-index forcing pip to not reach out to the internet (pypi.org) to
download packages. Using this option requires setting
pw_build_PYTHON_PIP_INSTALL_FIND_LINKS as well.

**Current value (from the default):** `false`

From //third_party/pigweed/src/pw_build/python_gn_args.gni:37

### pw_build_PYTHON_PIP_INSTALL_REQUIRE_HASHES

Adds '--require-hashes'. This option enforces hash checking on Python
package files.

**Current value (from the default):** `false`

From //third_party/pigweed/src/pw_build/python_gn_args.gni:32

### pw_build_PYTHON_STATIC_ANALYSIS_TOOLS

DOCSTAG: [python-static-analysis-tools] Default set of Python static alaysis
tools to run for pw_python_package targets.

**Current value (from the default):** `["pylint", "mypy"]`

From //third_party/pigweed/src/pw_build/python.gni:34

### pw_build_PYTHON_TEST_COVERAGE

If true, GN will run each Python test using the coverage command. A separate
coverage data file for each test will be saved. To generate reports from
this information run: pw presubmit --step gn_python_test_coverage

**Current value (from the default):** `false`

From //third_party/pigweed/src/pw_build/python.gni:44

### pw_build_PYTHON_TOOLCHAIN

Python tasks, such as running tests and Pylint, are done in a single GN
toolchain to avoid unnecessary duplication in the build.

**Current value (from the default):** `"//third_party/pigweed/src/pw_build/python_toolchain:python"`

From //third_party/pigweed/src/pw_build/python_gn_args.gni:20

### pw_build_TEST_TRANSITIVE_PYTHON_DEPS

Whether or not to lint/test transitive deps of pw_python_package targets.

For example: if lib_a depends on lib_b, lib_a.tests will run after first
running lib_b.tests if pw_build_TEST_TRANSITIVE_PYTHON_DEPS is true.

If pw_build_TEST_TRANSITIVE_PYTHON_DEPS is false, tests for a
pw_python_package will run if you directly build the target (e.g.
lib_b.tests) OR if the pw_python_package is placed in a pw_python_group AND
you build the group.tests target.

This applies to mypy, pylint, ruff and tests.

While this defaults to true for compatibility reasons, it's strongly
recommended to turn this off so you're not linting and testing all of your
external dependencies.

**Current value (from the default):** `true`

From //third_party/pigweed/src/pw_build/python.gni:64

### pw_build_TEST_TRANSITIVE_PYTHON_DEPS_FILTER_EXCLUDES

If you wish to enable transitive dependencies on a subset of your targets,
use this to filter out unwanted transitive dependencies.
For example: `[ "$dir_pigweed/*" ]`

**Current value (from the default):** `[]`

From //third_party/pigweed/src/pw_build/python.gni:69

### pw_build_TOOLCHAIN_LINK_DEPS

pw_build_TOOLCHAIN_LINK_DEPS is used by pw_toolchain module to set default
libary dependencies. Generally, this is not intended to be user-facing, but
if something is introduced here that you need to remove, you can do so by
overriding this variable in your own toolchain.

**Current value (from the default):** `[]`

From //third_party/pigweed/src/pw_build/gn_internal/build_target.gni:32

### pw_chre_CONFIG

The configuration for building CHRE.

**Current value for `target_cpu = "arm64"`:** `"//third_party/chre:chre_config"`

From //.gn:144

**Overridden from the default:** `"//third_party/chre:default_chre_config"`

From //third_party/pigweed/src/third_party/chre/chre.gni:22

**Current value for `target_cpu = "riscv64"`:** `"//third_party/chre:chre_config"`

From //.gn:144

**Overridden from the default:** `"//third_party/chre:default_chre_config"`

From //third_party/pigweed/src/third_party/chre/chre.gni:22

**Current value for `target_cpu = "x64"`:** `"//third_party/chre:chre_config"`

From //.gn:144

**Overridden from the default:** `"//third_party/chre:default_chre_config"`

From //third_party/pigweed/src/third_party/chre/chre.gni:22

### pw_chre_PLATFORM_BACKEND

CHRE's platform backend implementation. The default is the Pigweed backend.

**Current value for `target_cpu = "arm64"`:** `"//third_party/pigweed/src/pw_chre:chre_backend"`

From //.gn:145

**Overridden from the default:** `"//pw_chre:chre_backend"`

From //third_party/pigweed/src/third_party/chre/chre.gni:28

**Current value for `target_cpu = "riscv64"`:** `"//third_party/pigweed/src/pw_chre:chre_backend"`

From //.gn:145

**Overridden from the default:** `"//pw_chre:chre_backend"`

From //third_party/pigweed/src/third_party/chre/chre.gni:28

**Current value for `target_cpu = "x64"`:** `"//third_party/pigweed/src/pw_chre:chre_backend"`

From //.gn:145

**Overridden from the default:** `"//pw_chre:chre_backend"`

From //third_party/pigweed/src/third_party/chre/chre.gni:28

### pw_chre_PLATFORM_BACKEND_HEADERS

CHRE's platform backend headers. The default is the Pigweed backend.

**Current value for `target_cpu = "arm64"`:** `"//third_party/pigweed/src/pw_chre:chre_backend_headers"`

From //.gn:147

**Overridden from the default:** `"//pw_chre:chre_backend_headers"`

From //third_party/pigweed/src/third_party/chre/chre.gni:25

**Current value for `target_cpu = "riscv64"`:** `"//third_party/pigweed/src/pw_chre:chre_backend_headers"`

From //.gn:147

**Overridden from the default:** `"//pw_chre:chre_backend_headers"`

From //third_party/pigweed/src/third_party/chre/chre.gni:25

**Current value for `target_cpu = "x64"`:** `"//third_party/pigweed/src/pw_chre:chre_backend_headers"`

From //.gn:147

**Overridden from the default:** `"//pw_chre:chre_backend_headers"`

From //third_party/pigweed/src/third_party/chre/chre.gni:25

### pw_chrono_SYSTEM_CLOCK_BACKEND

Backend for the pw_chrono module's system_clock.

**Current value for `target_cpu = "arm64"`:** `"//third_party/pigweed/src/pw_chrono_stl:system_clock"`

From //.gn:78

**Overridden from the default:** `""`

From //third_party/pigweed/src/pw_chrono/backend.gni:17

**Current value for `target_cpu = "riscv64"`:** `"//third_party/pigweed/src/pw_chrono_stl:system_clock"`

From //.gn:78

**Overridden from the default:** `""`

From //third_party/pigweed/src/pw_chrono/backend.gni:17

**Current value for `target_cpu = "x64"`:** `"//third_party/pigweed/src/pw_chrono_stl:system_clock"`

From //.gn:78

**Overridden from the default:** `""`

From //third_party/pigweed/src/pw_chrono/backend.gni:17

### pw_chrono_SYSTEM_TIMER_BACKEND

Backend for the pw_chrono module's system_timer.

**Current value for `target_cpu = "arm64"`:** `"//third_party/pigweed/src/pw_chrono_stl:system_timer"`

From //.gn:80

**Overridden from the default:** `""`

From //third_party/pigweed/src/pw_chrono/backend.gni:20

**Current value for `target_cpu = "riscv64"`:** `"//third_party/pigweed/src/pw_chrono_stl:system_timer"`

From //.gn:80

**Overridden from the default:** `""`

From //third_party/pigweed/src/pw_chrono/backend.gni:20

**Current value for `target_cpu = "x64"`:** `"//third_party/pigweed/src/pw_chrono_stl:system_timer"`

From //.gn:80

**Overridden from the default:** `""`

From //third_party/pigweed/src/pw_chrono/backend.gni:20

### pw_command_launcher

Prefix for compilation commands (e.g. the path to a Goma or CCache compiler
launcher). Example for ccache:
  gn gen out --args='pw_command_launcher="ccache"'

**Current value (from the default):** `""`

From //third_party/pigweed/src/pw_toolchain/toolchain_args.gni:24

### pw_compilation_testing_NEGATIVE_COMPILATION_ENABLED

Enables or disables negative compilation tests for the current toolchain.
Disabled by default since negative compilation tests increase gn gen time
significantly.

**Current value (from the default):** `false`

From //third_party/pigweed/src/pw_compilation_testing/negative_compilation_test.gni:24

### pw_containers_CONFIG

The build target that overrides the default configuration options for this
module. This should point to a source set that provides defines through a
public config (which may -include a file or add defines directly).

**Current value (from the default):** `"//third_party/pigweed/src/pw_build:empty"`

From //third_party/pigweed/src/pw_containers/BUILD.gn:31

### pw_emboss_build_dir_to_root

An optional relative path from `root_build_dir` to `//`, which can be used
by `emboss_cc_library` to rebase `embossc` source files.

Normally we don't need to define this manually ourselves and can rely on
builtin GN functions to `rebase_path()` against `root_build_dir`, but that
approach is unreliable for [build environments where `//out` is a symlink
pointing to a directory outside of `//`][1], since [GN always provides
`root_build_dir` as an absolute symlink-derefed path][2].

When this is the case, we shouldn't use `root_build_dir` directly and must
instead construct a "lexically local" version of `root_build_dir` which is a
subpath of `//`.

Here's an example demonstrating this:

Using a project filesystem structure like this:
    /home/alice/foo_project (as `//`)
      src/foo.emb
      out/a_build_dir -> /storage/out/a_build_dir
This GN expression:
    rebase_path("src/foo.emb", "$root_build_dir")
Is equivalent to:
    rebase_path("/home/alice/foo_project/src/foo.emb", "/storage/out/a_build_dir")
Which evaluates to:
    "../../../home/alice/foo_project/src/foo.emb"

This differs from the result if the out directory isn't a symlink.
In that case, the same GN expression:
    rebase_path("src/foo.emb", "$root_build_dir")
Would be equivalent to:
    rebase_path("/home/alice/foo_project/src/foo.emb", "/home/alice/foo_project/out/a_build_dir")
Which evaluates to:
    "../../src/foo.emb"

While both of these examples resolve to the same file, `embossc` attempts to
place generated header files as siblings to the source .emb files, making it
sensitive to the lexical representation of the path. `emboss_cc_library`
also cancels out the leading `../` segments invoking `embossc` with an
`--output-path` of nested r"(fake/)*" directories to ensure these generated
files are contained within out directory.

In the first example, the output path is:
  "gen/fake/fake/fake/../../../home/alice/foo_project/src/foo.h"
Which simplifies to:
  "gen/home/alice/foo_project/src/foo.h"
This breaks `#include <foo.h>` directives in source files.

The second example places header files in the correct path:
  "gen/fake/fake/../../src/foo.h"
Which simplifies to:
  "gen/src/foo.h"

By using `pw_emboss_build_dir_to_root` as the logical relative path from
`root_build_dir` to `//`, it's possible to lexically construct a
`symlink_safe_build_dir` which behaves like `root_build_dir` without
resolving symlinks.

[1]: https://cs.opensource.google/fuchsia/fuchsia/+/main:scripts/cog/cartfs_out_directory.py
[2]: https://gn.googlesource.com/gn/+/e7f3202128bdb2429872fdb138626a010b2bff7f/src/gn/setup.cc#759

**Current value (from the default):** `""`

From //third_party/pigweed/src/third_party/emboss/build_defs.gni:75

### pw_env_setup_CIPD_BAZEL

**Current value (from the default):** `"../../prebuilt/third_party/bazel/linux-x64/bin"`

From //build_overrides/pigweed_environment.gni:24

### pw_env_setup_CIPD_DEFAULT

**Current value (from the default):** `"//prebuilt/third_party"`

From //build_overrides/pigweed_environment.gni:18

### pw_env_setup_CIPD_PIGWEED

**Current value (from the default):** `"//prebuilt/third_party"`

From //build_overrides/pigweed_environment.gni:19

### pw_env_setup_CIPD_PYTHON

**Current value (from the default):** `"../../prebuilt/third_party/python/linux-x64/bin"`

From //build_overrides/pigweed_environment.gni:21

### pw_external_abseil_cpp

**Current value (from the default):** `"//third_party/pigweed/src/third_party/abseil-cpp"`

From //third_party/pigweed/src/modules.gni:39

### pw_external_ambiq

**Current value (from the default):** `"//third_party/pigweed/src/third_party/ambiq"`

From //third_party/pigweed/src/modules.gni:40

### pw_external_apollo4

**Current value (from the default):** `"//third_party/pigweed/src/third_party/apollo4"`

From //third_party/pigweed/src/modules.gni:41

### pw_external_arduino

**Current value (from the default):** `"//third_party/pigweed/src/third_party/arduino"`

From //third_party/pigweed/src/modules.gni:42

### pw_external_boringssl

**Current value (from the default):** `"//third_party/pigweed/src/third_party/boringssl"`

From //third_party/pigweed/src/modules.gni:44

### pw_external_chre

**Current value (from the default):** `"//third_party/pigweed/src/third_party/chre"`

From //third_party/pigweed/src/modules.gni:45

### pw_external_chromium_verifier

**Current value (from the default):** `"//third_party/pigweed/src/third_party/chromium_verifier"`

From //third_party/pigweed/src/modules.gni:47

### pw_external_embos

**Current value (from the default):** `"//third_party/pigweed/src/third_party/embos"`

From //third_party/pigweed/src/modules.gni:48

### pw_external_emboss

**Current value (from the default):** `"//third_party/pigweed/src/third_party/emboss"`

From //third_party/pigweed/src/modules.gni:49

### pw_external_freertos

**Current value (from the default):** `"//third_party/pigweed/src/third_party/freertos"`

From //third_party/pigweed/src/modules.gni:51

### pw_external_fuchsia

**Current value (from the default):** `"//third_party/pigweed/src/third_party/fuchsia"`

From //third_party/pigweed/src/modules.gni:52

### pw_external_googletest

**Current value (from the default):** `"//third_party/pigweed/src/third_party/googletest"`

From //third_party/pigweed/src/modules.gni:54

### pw_external_llvm_builtins

**Current value (from the default):** `"//third_party/pigweed/src/third_party/llvm_builtins"`

From //third_party/pigweed/src/modules.gni:56

### pw_external_llvm_libc

**Current value (from the default):** `"//third_party/pigweed/src/third_party/llvm_libc"`

From //third_party/pigweed/src/modules.gni:58

### pw_external_llvm_libcxx

**Current value (from the default):** `"//third_party/pigweed/src/third_party/llvm_libcxx"`

From //third_party/pigweed/src/modules.gni:60

### pw_external_mbedtls

**Current value (from the default):** `"//third_party/pigweed/src/third_party/mbedtls"`

From //third_party/pigweed/src/modules.gni:61

### pw_external_mcuxpresso

**Current value (from the default):** `"//third_party/pigweed/src/third_party/mcuxpresso"`

From //third_party/pigweed/src/modules.gni:63

### pw_external_nanopb

**Current value (from the default):** `"//third_party/pigweed/src/third_party/nanopb"`

From //third_party/pigweed/src/modules.gni:64

### pw_external_perfetto

**Current value (from the default):** `"//third_party/pigweed/src/third_party/perfetto"`

From //third_party/pigweed/src/modules.gni:66

### pw_external_pico_sdk

**Current value (from the default):** `"//third_party/pigweed/src/third_party/pico_sdk"`

From //third_party/pigweed/src/modules.gni:68

### pw_external_protobuf

**Current value (from the default):** `"//third_party/pigweed/src/third_party/protobuf"`

From //third_party/pigweed/src/modules.gni:70

### pw_external_repo

**Current value (from the default):** `"//third_party/pigweed/src/third_party/repo"`

From //third_party/pigweed/src/modules.gni:71

### pw_external_smartfusion_mss

**Current value (from the default):** `"//third_party/pigweed/src/third_party/smartfusion_mss"`

From //third_party/pigweed/src/modules.gni:73

### pw_external_stm32cube

**Current value (from the default):** `"//third_party/pigweed/src/third_party/stm32cube"`

From //third_party/pigweed/src/modules.gni:75

### pw_external_threadx

**Current value (from the default):** `"//third_party/pigweed/src/third_party/threadx"`

From //third_party/pigweed/src/modules.gni:76

### pw_external_tinyusb

**Current value (from the default):** `"//third_party/pigweed/src/third_party/tinyusb"`

From //third_party/pigweed/src/modules.gni:77

### pw_log_BACKEND

Backend for the pw_log module.

**Current value for `target_cpu = "arm64"`:** `"//third_party/pigweed/backends/pw_log"`

From //.gn:81

**Overridden from the default:** `""`

From //third_party/pigweed/src/pw_log/backend.gni:17

**Current value for `target_cpu = "riscv64"`:** `"//third_party/pigweed/backends/pw_log"`

From //.gn:81

**Overridden from the default:** `""`

From //third_party/pigweed/src/pw_log/backend.gni:17

**Current value for `target_cpu = "x64"`:** `"//third_party/pigweed/backends/pw_log"`

From //.gn:81

**Overridden from the default:** `""`

From //third_party/pigweed/src/pw_log/backend.gni:17

### pw_log_CONFIG

The build target that overrides the default configuration options for this
module. This should point to a source set that provides defines through a
public config (which may -include a file or add defines directly).

**Current value (from the default):** `"//third_party/pigweed/src/pw_build:empty"`

From //third_party/pigweed/src/pw_log/BUILD.gn:28

### pw_log_GLOG_ADAPTER_CONFIG

The build target that overrides the default configuration options for the
glog adapter portion of this module.

**Current value (from the default):** `"//third_party/pigweed/src/pw_build:empty"`

From //third_party/pigweed/src/pw_log/BUILD.gn:32

### pw_module_tests

A list with all Pigweed module test groups. DO NOT SET THIS BUILD ARGUMENT!

**Current value (from the default):** `["//third_party/pigweed/src/docker:tests", "//third_party/pigweed/src/pw_alignment:tests", "//third_party/pigweed/src/pw_allocator:tests", "//third_party/pigweed/src/pw_allocator_zephyr:tests", "//third_party/pigweed/src/pw_analog:tests", "//third_party/pigweed/src/pw_android_toolchain:tests", "//third_party/pigweed/src/pw_arduino_build:tests", "//third_party/pigweed/src/pw_assert:tests", "//third_party/pigweed/src/pw_assert_basic:tests", "//third_party/pigweed/src/pw_assert_fuchsia:tests", "//third_party/pigweed/src/pw_assert_log:tests", "//third_party/pigweed/src/pw_assert_tokenized:tests", "//third_party/pigweed/src/pw_assert_trap:tests", "//third_party/pigweed/src/pw_assert_zephyr:tests", "//third_party/pigweed/src/pw_async:tests", "//third_party/pigweed/src/pw_async2:tests", "//third_party/pigweed/src/pw_async_basic:tests", "//third_party/pigweed/src/pw_async_fuchsia:tests", "//third_party/pigweed/src/pw_atomic:tests", "//third_party/pigweed/src/pw_base64:tests", "//third_party/pigweed/src/pw_bloat:tests", "//third_party/pigweed/src/pw_blob_store:tests", "//third_party/pigweed/src/pw_bluetooth:tests", "//third_party/pigweed/src/pw_bluetooth_hci:tests", "//third_party/pigweed/src/pw_bluetooth_profiles:tests", "//third_party/pigweed/src/pw_bluetooth_proxy:tests", "//third_party/pigweed/src/pw_bluetooth_sapphire:tests", "//third_party/pigweed/src/pw_boot:tests", "//third_party/pigweed/src/pw_boot_cortex_m:tests", "//third_party/pigweed/src/pw_buf:tests", "//third_party/pigweed/src/pw_build:tests", "//third_party/pigweed/src/pw_build_android:tests", "//third_party/pigweed/src/pw_build_info:tests", "//third_party/pigweed/src/pw_build_mcuxpresso:tests", "//third_party/pigweed/src/pw_bytes:tests", "//third_party/pigweed/src/pw_change:tests", "//third_party/pigweed/src/pw_channel:tests", "//third_party/pigweed/src/pw_checksum:tests", "//third_party/pigweed/src/pw_chre:tests", "//third_party/pigweed/src/pw_chrono:tests", "//third_party/pigweed/src/pw_chrono_embos:tests", "//third_party/pigweed/src/pw_chrono_freertos:tests", "//third_party/pigweed/src/pw_chrono_rp2040:tests", "//third_party/pigweed/src/pw_chrono_stl:tests", "//third_party/pigweed/src/pw_chrono_threadx:tests", "//third_party/pigweed/src/pw_chrono_zephyr:tests", "//third_party/pigweed/src/pw_cli:tests", "//third_party/pigweed/src/pw_cli_analytics:tests", "//third_party/pigweed/src/pw_clock_tree:tests", "//third_party/pigweed/src/pw_clock_tree_mcuxpresso:tests", "//third_party/pigweed/src/pw_compilation_testing:tests", "//third_party/pigweed/src/pw_config_loader:tests", "//third_party/pigweed/src/pw_console:tests", "//third_party/pigweed/src/pw_containers:tests", "//third_party/pigweed/src/pw_cpu_exception:tests", "//third_party/pigweed/src/pw_cpu_exception_cortex_m:tests", "//third_party/pigweed/src/pw_cpu_exception_risc_v:tests", "//third_party/pigweed/src/pw_crypto:tests", "//third_party/pigweed/src/pw_digital_io:tests", "//third_party/pigweed/src/pw_digital_io_linux:tests", "//third_party/pigweed/src/pw_digital_io_mcuxpresso:tests", "//third_party/pigweed/src/pw_digital_io_rp2040:tests", "//third_party/pigweed/src/pw_digital_io_zephyr:tests", "//third_party/pigweed/src/pw_display:tests", "//third_party/pigweed/src/pw_dma_mcuxpresso:tests", "//third_party/pigweed/src/pw_docgen:tests", "//third_party/pigweed/src/pw_doctor:tests", "//third_party/pigweed/src/pw_elf:tests", "//third_party/pigweed/src/pw_emu:tests", "//third_party/pigweed/src/pw_enum:tests", "//third_party/pigweed/src/pw_env_setup:tests", "//third_party/pigweed/src/pw_env_setup_zephyr:tests", "//third_party/pigweed/src/pw_file:tests", "//third_party/pigweed/src/pw_flatbuffers:tests", "//third_party/pigweed/src/pw_format:tests", "//third_party/pigweed/src/pw_fortifier:tests", "//third_party/pigweed/src/pw_function:tests", "//third_party/pigweed/src/pw_fuzzer:tests", "//third_party/pigweed/src/pw_grpc:tests", "//third_party/pigweed/src/pw_hdlc:tests", "//third_party/pigweed/src/pw_hex_dump:tests", "//third_party/pigweed/src/pw_i2c:tests", "//third_party/pigweed/src/pw_i2c_linux:tests", "//third_party/pigweed/src/pw_i2c_mcuxpresso:tests", "//third_party/pigweed/src/pw_i2c_rp2040:tests", "//third_party/pigweed/src/pw_i2c_zephyr:tests", "//third_party/pigweed/src/pw_ide:tests", "//third_party/pigweed/src/pw_interrupt:tests", "//third_party/pigweed/src/pw_interrupt_cortex_m:tests", "//third_party/pigweed/src/pw_interrupt_freertos:tests", "//third_party/pigweed/src/pw_interrupt_zephyr:tests", "//third_party/pigweed/src/pw_intrusive_ptr:tests", "//third_party/pigweed/src/pw_json:tests", "//third_party/pigweed/src/pw_kernel:tests", "//third_party/pigweed/src/pw_kvs:tests", "//third_party/pigweed/src/pw_libc:tests", "//third_party/pigweed/src/pw_libcxx:tests", "//third_party/pigweed/src/pw_log:tests", "//third_party/pigweed/src/pw_log_android:tests", "//third_party/pigweed/src/pw_log_basic:tests", "//third_party/pigweed/src/pw_log_fuchsia:tests", "//third_party/pigweed/src/pw_log_null:tests", "//third_party/pigweed/src/pw_log_rpc:tests", "//third_party/pigweed/src/pw_log_string:tests", "//third_party/pigweed/src/pw_log_tokenized:tests", "//third_party/pigweed/src/pw_log_zephyr:tests", "//third_party/pigweed/src/pw_malloc:tests", "//third_party/pigweed/src/pw_malloc_freelist:tests", "//third_party/pigweed/src/pw_malloc_freertos:tests", "//third_party/pigweed/src/pw_memory:tests", "//third_party/pigweed/src/pw_metric:tests", "//third_party/pigweed/src/pw_module:tests", "//third_party/pigweed/src/pw_multibuf:tests", "//third_party/pigweed/src/pw_multisink:tests", "//third_party/pigweed/src/pw_numeric:tests", "//third_party/pigweed/src/pw_package:tests", "//third_party/pigweed/src/pw_perf_test:tests", "//third_party/pigweed/src/pw_persistent_ram:tests", "//third_party/pigweed/src/pw_polyfill:tests", "//third_party/pigweed/src/pw_preprocessor:tests", "//third_party/pigweed/src/pw_presubmit:tests", "//third_party/pigweed/src/pw_protobuf:tests", "//third_party/pigweed/src/pw_protobuf_compiler:tests", "//third_party/pigweed/src/pw_random:tests", "//third_party/pigweed/src/pw_random_fuchsia:tests", "//third_party/pigweed/src/pw_result:tests", "//third_party/pigweed/src/pw_ring_buffer:tests", "//third_party/pigweed/src/pw_router:tests", "//third_party/pigweed/src/pw_rpc:tests", "//third_party/pigweed/src/pw_rpc_transport:tests", "//third_party/pigweed/src/pw_rust:tests", "//third_party/pigweed/src/pw_sensor:tests", "//third_party/pigweed/src/pw_snapshot:tests", "//third_party/pigweed/src/pw_software_update:tests", "//third_party/pigweed/src/pw_span:tests", "//third_party/pigweed/src/pw_spi:tests", "//third_party/pigweed/src/pw_spi_linux:tests", "//third_party/pigweed/src/pw_spi_mcuxpresso:tests", "//third_party/pigweed/src/pw_spi_rp2040:tests", "//third_party/pigweed/src/pw_spi_zephyr:tests", "//third_party/pigweed/src/pw_status:tests", "//third_party/pigweed/src/pw_stm32cube_build:tests", "//third_party/pigweed/src/pw_stream:tests", "//third_party/pigweed/src/pw_stream_uart_linux:tests", "//third_party/pigweed/src/pw_stream_uart_mcuxpresso:tests", "//third_party/pigweed/src/pw_string:tests", "//third_party/pigweed/src/pw_symbolizer:tests", "//third_party/pigweed/src/pw_sync:tests", "//third_party/pigweed/src/pw_sync_baremetal:tests", "//third_party/pigweed/src/pw_sync_embos:tests", "//third_party/pigweed/src/pw_sync_freertos:tests", "//third_party/pigweed/src/pw_sync_stl:tests", "//third_party/pigweed/src/pw_sync_threadx:tests", "//third_party/pigweed/src/pw_sync_zephyr:tests", "//third_party/pigweed/src/pw_sys_io:tests", "//third_party/pigweed/src/pw_sys_io_ambiq_sdk:tests", "//third_party/pigweed/src/pw_sys_io_arduino:tests", "//third_party/pigweed/src/pw_sys_io_baremetal_lm3s6965evb:tests", "//third_party/pigweed/src/pw_sys_io_baremetal_stm32f429:tests", "//third_party/pigweed/src/pw_sys_io_emcraft_sf2:tests", "//third_party/pigweed/src/pw_sys_io_mcuxpresso:tests", "//third_party/pigweed/src/pw_sys_io_rp2040:tests", "//third_party/pigweed/src/pw_sys_io_stdio:tests", "//third_party/pigweed/src/pw_sys_io_stm32cube:tests", "//third_party/pigweed/src/pw_sys_io_zephyr:tests", "//third_party/pigweed/src/pw_system:tests", "//third_party/pigweed/src/pw_target_runner:tests", "//third_party/pigweed/src/pw_thread:tests", "//third_party/pigweed/src/pw_thread_embos:tests", "//third_party/pigweed/src/pw_thread_freertos:tests", "//third_party/pigweed/src/pw_thread_stl:tests", "//third_party/pigweed/src/pw_thread_threadx:tests", "//third_party/pigweed/src/pw_thread_zephyr:tests", "//third_party/pigweed/src/pw_time:tests", "//third_party/pigweed/src/pw_tls_client:tests", "//third_party/pigweed/src/pw_tls_client_boringssl:tests", "//third_party/pigweed/src/pw_tls_client_mbedtls:tests", "//third_party/pigweed/src/pw_tokenizer:tests", "//third_party/pigweed/src/pw_toolchain:tests", "//third_party/pigweed/src/pw_trace:tests", "//third_party/pigweed/src/pw_trace_tokenized:tests", "//third_party/pigweed/src/pw_transfer:tests", "//third_party/pigweed/src/pw_uart:tests", "//third_party/pigweed/src/pw_uart_mcuxpresso:tests", "//third_party/pigweed/src/pw_unit_test:tests", "//third_party/pigweed/src/pw_uuid:tests", "//third_party/pigweed/src/pw_varint:tests", "//third_party/pigweed/src/pw_watch:tests", "//third_party/pigweed/src/pw_web:tests", "//third_party/pigweed/src/pw_work_queue:tests"]`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:435

### pw_modules

A list with paths to all Pigweed module. DO NOT SET THIS BUILD ARGUMENT!

**Current value (from the default):** `["//third_party/pigweed/src/docker", "//third_party/pigweed/src/pw_alignment", "//third_party/pigweed/src/pw_allocator", "//third_party/pigweed/src/pw_allocator_zephyr", "//third_party/pigweed/src/pw_analog", "//third_party/pigweed/src/pw_android_toolchain", "//third_party/pigweed/src/pw_arduino_build", "//third_party/pigweed/src/pw_assert", "//third_party/pigweed/src/pw_assert_basic", "//third_party/pigweed/src/pw_assert_fuchsia", "//third_party/pigweed/src/pw_assert_log", "//third_party/pigweed/src/pw_assert_tokenized", "//third_party/pigweed/src/pw_assert_trap", "//third_party/pigweed/src/pw_assert_zephyr", "//third_party/pigweed/src/pw_async", "//third_party/pigweed/src/pw_async2", "//third_party/pigweed/src/pw_async_basic", "//third_party/pigweed/src/pw_async_fuchsia", "//third_party/pigweed/src/pw_atomic", "//third_party/pigweed/src/pw_base64", "//third_party/pigweed/src/pw_bloat", "//third_party/pigweed/src/pw_blob_store", "//third_party/pigweed/src/pw_bluetooth", "//third_party/pigweed/src/pw_bluetooth_hci", "//third_party/pigweed/src/pw_bluetooth_profiles", "//third_party/pigweed/src/pw_bluetooth_proxy", "//third_party/pigweed/src/pw_bluetooth_sapphire", "//third_party/pigweed/src/pw_boot", "//third_party/pigweed/src/pw_boot_cortex_m", "//third_party/pigweed/src/pw_buf", "//third_party/pigweed/src/pw_build", "//third_party/pigweed/src/pw_build_android", "//third_party/pigweed/src/pw_build_info", "//third_party/pigweed/src/pw_build_mcuxpresso", "//third_party/pigweed/src/pw_bytes", "//third_party/pigweed/src/pw_change", "//third_party/pigweed/src/pw_channel", "//third_party/pigweed/src/pw_checksum", "//third_party/pigweed/src/pw_chre", "//third_party/pigweed/src/pw_chrono", "//third_party/pigweed/src/pw_chrono_embos", "//third_party/pigweed/src/pw_chrono_freertos", "//third_party/pigweed/src/pw_chrono_rp2040", "//third_party/pigweed/src/pw_chrono_stl", "//third_party/pigweed/src/pw_chrono_threadx", "//third_party/pigweed/src/pw_chrono_zephyr", "//third_party/pigweed/src/pw_cli", "//third_party/pigweed/src/pw_cli_analytics", "//third_party/pigweed/src/pw_clock_tree", "//third_party/pigweed/src/pw_clock_tree_mcuxpresso", "//third_party/pigweed/src/pw_compilation_testing", "//third_party/pigweed/src/pw_config_loader", "//third_party/pigweed/src/pw_console", "//third_party/pigweed/src/pw_containers", "//third_party/pigweed/src/pw_cpu_exception", "//third_party/pigweed/src/pw_cpu_exception_cortex_m", "//third_party/pigweed/src/pw_cpu_exception_risc_v", "//third_party/pigweed/src/pw_crypto", "//third_party/pigweed/src/pw_digital_io", "//third_party/pigweed/src/pw_digital_io_linux", "//third_party/pigweed/src/pw_digital_io_mcuxpresso", "//third_party/pigweed/src/pw_digital_io_rp2040", "//third_party/pigweed/src/pw_digital_io_zephyr", "//third_party/pigweed/src/pw_display", "//third_party/pigweed/src/pw_dma_mcuxpresso", "//third_party/pigweed/src/pw_docgen", "//third_party/pigweed/src/pw_doctor", "//third_party/pigweed/src/pw_elf", "//third_party/pigweed/src/pw_emu", "//third_party/pigweed/src/pw_enum", "//third_party/pigweed/src/pw_env_setup", "//third_party/pigweed/src/pw_env_setup_zephyr", "//third_party/pigweed/src/pw_file", "//third_party/pigweed/src/pw_flatbuffers", "//third_party/pigweed/src/pw_format", "//third_party/pigweed/src/pw_fortifier", "//third_party/pigweed/src/pw_function", "//third_party/pigweed/src/pw_fuzzer", "//third_party/pigweed/src/pw_grpc", "//third_party/pigweed/src/pw_hdlc", "//third_party/pigweed/src/pw_hex_dump", "//third_party/pigweed/src/pw_i2c", "//third_party/pigweed/src/pw_i2c_linux", "//third_party/pigweed/src/pw_i2c_mcuxpresso", "//third_party/pigweed/src/pw_i2c_rp2040", "//third_party/pigweed/src/pw_i2c_zephyr", "//third_party/pigweed/src/pw_ide", "//third_party/pigweed/src/pw_interrupt", "//third_party/pigweed/src/pw_interrupt_cortex_m", "//third_party/pigweed/src/pw_interrupt_freertos", "//third_party/pigweed/src/pw_interrupt_zephyr", "//third_party/pigweed/src/pw_intrusive_ptr", "//third_party/pigweed/src/pw_json", "//third_party/pigweed/src/pw_kernel", "//third_party/pigweed/src/pw_kvs", "//third_party/pigweed/src/pw_libc", "//third_party/pigweed/src/pw_libcxx", "//third_party/pigweed/src/pw_log", "//third_party/pigweed/src/pw_log_android", "//third_party/pigweed/src/pw_log_basic", "//third_party/pigweed/src/pw_log_fuchsia", "//third_party/pigweed/src/pw_log_null", "//third_party/pigweed/src/pw_log_rpc", "//third_party/pigweed/src/pw_log_string", "//third_party/pigweed/src/pw_log_tokenized", "//third_party/pigweed/src/pw_log_zephyr", "//third_party/pigweed/src/pw_malloc", "//third_party/pigweed/src/pw_malloc_freelist", "//third_party/pigweed/src/pw_malloc_freertos", "//third_party/pigweed/src/pw_memory", "//third_party/pigweed/src/pw_metric", "//third_party/pigweed/src/pw_module", "//third_party/pigweed/src/pw_multibuf", "//third_party/pigweed/src/pw_multisink", "//third_party/pigweed/src/pw_numeric", "//third_party/pigweed/src/pw_package", "//third_party/pigweed/src/pw_perf_test", "//third_party/pigweed/src/pw_persistent_ram", "//third_party/pigweed/src/pw_polyfill", "//third_party/pigweed/src/pw_preprocessor", "//third_party/pigweed/src/pw_presubmit", "//third_party/pigweed/src/pw_protobuf", "//third_party/pigweed/src/pw_protobuf_compiler", "//third_party/pigweed/src/pw_random", "//third_party/pigweed/src/pw_random_fuchsia", "//third_party/pigweed/src/pw_result", "//third_party/pigweed/src/pw_ring_buffer", "//third_party/pigweed/src/pw_router", "//third_party/pigweed/src/pw_rpc", "//third_party/pigweed/src/pw_rpc_transport", "//third_party/pigweed/src/pw_rust", "//third_party/pigweed/src/pw_sensor", "//third_party/pigweed/src/pw_snapshot", "//third_party/pigweed/src/pw_software_update", "//third_party/pigweed/src/pw_span", "//third_party/pigweed/src/pw_spi", "//third_party/pigweed/src/pw_spi_linux", "//third_party/pigweed/src/pw_spi_mcuxpresso", "//third_party/pigweed/src/pw_spi_rp2040", "//third_party/pigweed/src/pw_spi_zephyr", "//third_party/pigweed/src/pw_status", "//third_party/pigweed/src/pw_stm32cube_build", "//third_party/pigweed/src/pw_stream", "//third_party/pigweed/src/pw_stream_uart_linux", "//third_party/pigweed/src/pw_stream_uart_mcuxpresso", "//third_party/pigweed/src/pw_string", "//third_party/pigweed/src/pw_symbolizer", "//third_party/pigweed/src/pw_sync", "//third_party/pigweed/src/pw_sync_baremetal", "//third_party/pigweed/src/pw_sync_embos", "//third_party/pigweed/src/pw_sync_freertos", "//third_party/pigweed/src/pw_sync_stl", "//third_party/pigweed/src/pw_sync_threadx", "//third_party/pigweed/src/pw_sync_zephyr", "//third_party/pigweed/src/pw_sys_io", "//third_party/pigweed/src/pw_sys_io_ambiq_sdk", "//third_party/pigweed/src/pw_sys_io_arduino", "//third_party/pigweed/src/pw_sys_io_baremetal_lm3s6965evb", "//third_party/pigweed/src/pw_sys_io_baremetal_stm32f429", "//third_party/pigweed/src/pw_sys_io_emcraft_sf2", "//third_party/pigweed/src/pw_sys_io_mcuxpresso", "//third_party/pigweed/src/pw_sys_io_rp2040", "//third_party/pigweed/src/pw_sys_io_stdio", "//third_party/pigweed/src/pw_sys_io_stm32cube", "//third_party/pigweed/src/pw_sys_io_zephyr", "//third_party/pigweed/src/pw_system", "//third_party/pigweed/src/pw_target_runner", "//third_party/pigweed/src/pw_thread", "//third_party/pigweed/src/pw_thread_embos", "//third_party/pigweed/src/pw_thread_freertos", "//third_party/pigweed/src/pw_thread_stl", "//third_party/pigweed/src/pw_thread_threadx", "//third_party/pigweed/src/pw_thread_zephyr", "//third_party/pigweed/src/pw_time", "//third_party/pigweed/src/pw_tls_client", "//third_party/pigweed/src/pw_tls_client_boringssl", "//third_party/pigweed/src/pw_tls_client_mbedtls", "//third_party/pigweed/src/pw_tokenizer", "//third_party/pigweed/src/pw_toolchain", "//third_party/pigweed/src/pw_trace", "//third_party/pigweed/src/pw_trace_tokenized", "//third_party/pigweed/src/pw_transfer", "//third_party/pigweed/src/pw_uart", "//third_party/pigweed/src/pw_uart_mcuxpresso", "//third_party/pigweed/src/pw_unit_test", "//third_party/pigweed/src/pw_uuid", "//third_party/pigweed/src/pw_varint", "//third_party/pigweed/src/pw_watch", "//third_party/pigweed/src/pw_web", "//third_party/pigweed/src/pw_work_queue"]`

From //third_party/pigweed/src/pw_build/generated_pigweed_modules_lists.gni:241

### pw_preprocessor_CONFIG

The build target that overrides the default configuration options for this
module. This should point to a source set that provides defines through a
public config (which may -include a file or add defines directly).

**Current value (from the default):** `"//third_party/pigweed/src/pw_build:empty"`

From //third_party/pigweed/src/pw_preprocessor/BUILD.gn:25

### pw_protobuf_compiler_GENERATE_LEGACY_ENUM_SNAKE_CASE_NAMES

pwpb previously generated field enum names in SNAKE_CASE rather than
kConstantCase. Set this variable to temporarily enable legacy SNAKE_CASE
support while you migrate your codebase to kConstantCase.
b/266298474

**Current value (from the default):** `false`

From //third_party/pigweed/src/pw_protobuf_compiler/toolchain.gni:28

### pw_protobuf_compiler_GENERATE_PROTOS_ARGS

Args for generate_protos.py

**Current value (from the default):** `""`

From //third_party/pigweed/src/pw_protobuf_compiler/toolchain.gni:43

### pw_protobuf_compiler_GENERATE_PYTHON_TYPE_HINTS

Generate .pyi files for Python type hinting. This requires 'protoc-gen-mypy'
to be available on the PATH.

**Current value for `target_cpu = "arm64"`:** `false`

From //.gn:140

**Overridden from the default:** `true`

From //third_party/pigweed/src/pw_protobuf_compiler/toolchain.gni:40

**Current value for `target_cpu = "riscv64"`:** `false`

From //.gn:140

**Overridden from the default:** `true`

From //third_party/pigweed/src/pw_protobuf_compiler/toolchain.gni:40

**Current value for `target_cpu = "x64"`:** `false`

From //.gn:140

**Overridden from the default:** `true`

From //third_party/pigweed/src/pw_protobuf_compiler/toolchain.gni:40

### pw_protobuf_compiler_NO_GENERIC_OPTIONS_FILES

If true, requires the use of the `.pwpb_options` extensions for pw_protobuf
options files. If false, allows the generic `.options` to be used as well.

**Current value (from the default):** `false`

From //third_party/pigweed/src/pw_protobuf_compiler/toolchain.gni:32

### pw_protobuf_compiler_NO_ONEOF_CALLBACKS

If true, disables using callback interfaces for oneof fields, and keep the
legacy "oneof as struct member" interface.

**Current value (from the default):** `false`

From //third_party/pigweed/src/pw_protobuf_compiler/toolchain.gni:36

### pw_protobuf_compiler_PROTOC_BINARY

To override the protobuf compiler used set this to the path, relative to the
root_build_dir, to the protoc binary.

**Current value for `target_cpu = "arm64"`:** `"host_x64/protoc"`

From //.gn:130

**Overridden from the default:** `""`

From //third_party/pigweed/src/pw_protobuf_compiler/proto.gni:59

**Current value for `target_cpu = "riscv64"`:** `"host_x64/protoc"`

From //.gn:130

**Overridden from the default:** `""`

From //third_party/pigweed/src/pw_protobuf_compiler/proto.gni:59

**Current value for `target_cpu = "x64"`:** `"host_x64/protoc"`

From //.gn:130

**Overridden from the default:** `""`

From //third_party/pigweed/src/pw_protobuf_compiler/proto.gni:59

### pw_protobuf_compiler_PROTOC_PYTHON_DEPS

Optional Python package dependencies to include when running protoc.

**Current value for `target_cpu = "arm64"`:** `["//third_party/pigweed/backends/pw_protobuf_compiler:protoc_python_package"]`

From //.gn:134

**Overridden from the default:** `[]`

From //third_party/pigweed/src/pw_protobuf_compiler/proto.gni:42

**Current value for `target_cpu = "riscv64"`:** `["//third_party/pigweed/backends/pw_protobuf_compiler:protoc_python_package"]`

From //.gn:134

**Overridden from the default:** `[]`

From //third_party/pigweed/src/pw_protobuf_compiler/proto.gni:42

**Current value for `target_cpu = "x64"`:** `["//third_party/pigweed/backends/pw_protobuf_compiler:protoc_python_package"]`

From //.gn:134

**Overridden from the default:** `[]`

From //third_party/pigweed/src/pw_protobuf_compiler/proto.gni:42

### pw_protobuf_compiler_PROTOC_TARGET

To override the protobuf compiler used set this to the GN target that builds
the protobuf compiler.

**Current value for `target_cpu = "arm64"`:** `"//third_party/protobuf:protoc"`

From //.gn:129

**Overridden from the default:** `""`

From //third_party/pigweed/src/pw_protobuf_compiler/proto.gni:39

**Current value for `target_cpu = "riscv64"`:** `"//third_party/protobuf:protoc"`

From //.gn:129

**Overridden from the default:** `""`

From //third_party/pigweed/src/pw_protobuf_compiler/proto.gni:39

**Current value for `target_cpu = "x64"`:** `"//third_party/protobuf:protoc"`

From //.gn:129

**Overridden from the default:** `""`

From //third_party/pigweed/src/pw_protobuf_compiler/proto.gni:39

### pw_protobuf_compiler_TOOLCHAIN

**Current value (from the default):** `"//third_party/pigweed/src/pw_protobuf_compiler/toolchain:protocol_buffer"`

From //third_party/pigweed/src/pw_protobuf_compiler/toolchain.gni:22

### pw_rbe_arm_gcc_config

**Current value (from the default):** `""`

From //third_party/pigweed/src/pw_toolchain/rbe.gni:30

### pw_rbe_clang_config

**Current value (from the default):** `""`

From //third_party/pigweed/src/pw_toolchain/rbe.gni:29

### pw_rust_ENABLE_EXPERIMENTAL_BUILD

Enables compiling Pigweed's Rust libraries.

WARNING: This is experimental and *not* guaranteed to work.

**Current value (from the default):** `false`

From //third_party/pigweed/src/pw_rust/rust.gni:19

### pw_rust_USE_STD

**Current value (from the default):** `true`

From //third_party/pigweed/src/pw_rust/rust.gni:21

### pw_span_CONFIG

The build target that overrides the default configuration options for this
module. This should point to a source set that provides defines through a
public config (which may -include a file or add defines directly).

Most modules depend on pw_build_DEFAULT_MODULE_CONFIG as the default config,
but since this module's config options require interaction with the build
system, this defaults to an internal config to properly support
pw_span_ENABLE_ASSERTS.

**Current value (from the default):** `"//third_party/pigweed/src/pw_span:span_asserts"`

From //third_party/pigweed/src/pw_span/BUILD.gn:37

### pw_span_ENABLE_ASSERTS

Whether or not to enable bounds-checking asserts in pw::span. Enabling this
may significantly increase binary size, and can introduce dependency cycles
if your pw_assert backend's headers depends directly or indirectly on
pw_span. It's recommended to enable this for debug builds if possible.

**Current value (from the default):** `false`

From //third_party/pigweed/src/pw_span/BUILD.gn:27

### pw_status_CONFIG

The build target that overrides the default configuration options for this
module. This should point to a source set that provides defines through a
public config (which may -include a file or add defines directly).

**Current value (from the default):** `"//third_party/pigweed/src/pw_build:empty"`

From //third_party/pigweed/src/pw_status/BUILD.gn:25

### pw_string_CONFIG

The build target that overrides the default configuration options for this
module. This should point to a source set that provides defines through a
public config (which may -include a file or add defines directly).

**Current value (from the default):** `"//third_party/pigweed/src/pw_build:empty"`

From //third_party/pigweed/src/pw_string/BUILD.gn:26

### pw_sync_BINARY_SEMAPHORE_BACKEND

Backend for the pw_sync module's binary semaphore.

**Current value for `target_cpu = "arm64"`:** `"//third_party/pigweed/src/pw_sync_stl:binary_semaphore_backend"`

From //.gn:83

**Overridden from the default:** `""`

From //third_party/pigweed/src/pw_sync/backend.gni:17

**Current value for `target_cpu = "riscv64"`:** `"//third_party/pigweed/src/pw_sync_stl:binary_semaphore_backend"`

From //.gn:83

**Overridden from the default:** `""`

From //third_party/pigweed/src/pw_sync/backend.gni:17

**Current value for `target_cpu = "x64"`:** `"//third_party/pigweed/src/pw_sync_stl:binary_semaphore_backend"`

From //.gn:83

**Overridden from the default:** `""`

From //third_party/pigweed/src/pw_sync/backend.gni:17

### pw_sync_CONDITION_VARIABLE_BACKEND

Backend for the pw_sync module's condition variable.

**Current value (from the default):** `""`

From //third_party/pigweed/src/pw_sync/backend.gni:20

### pw_sync_COUNTING_SEMAPHORE_BACKEND

Backend for the pw_sync module's counting semaphore.

**Current value (from the default):** `""`

From //third_party/pigweed/src/pw_sync/backend.gni:23

### pw_sync_INTERRUPT_SPIN_LOCK_BACKEND

Backend for the pw_sync module's interrupt spin lock.

**Current value for `target_cpu = "arm64"`:** `"//third_party/pigweed/src/pw_sync_stl:interrupt_spin_lock"`

From //.gn:85

**Overridden from the default:** `""`

From //third_party/pigweed/src/pw_sync/backend.gni:35

**Current value for `target_cpu = "riscv64"`:** `"//third_party/pigweed/src/pw_sync_stl:interrupt_spin_lock"`

From //.gn:85

**Overridden from the default:** `""`

From //third_party/pigweed/src/pw_sync/backend.gni:35

**Current value for `target_cpu = "x64"`:** `"//third_party/pigweed/src/pw_sync_stl:interrupt_spin_lock"`

From //.gn:85

**Overridden from the default:** `""`

From //third_party/pigweed/src/pw_sync/backend.gni:35

### pw_sync_MUTEX_BACKEND

Backend for the pw_sync module's mutex.

**Current value for `target_cpu = "arm64"`:** `"//third_party/pigweed/src/pw_sync_stl:mutex_backend"`

From //.gn:86

**Overridden from the default:** `""`

From //third_party/pigweed/src/pw_sync/backend.gni:26

**Current value for `target_cpu = "riscv64"`:** `"//third_party/pigweed/src/pw_sync_stl:mutex_backend"`

From //.gn:86

**Overridden from the default:** `""`

From //third_party/pigweed/src/pw_sync/backend.gni:26

**Current value for `target_cpu = "x64"`:** `"//third_party/pigweed/src/pw_sync_stl:mutex_backend"`

From //.gn:86

**Overridden from the default:** `""`

From //third_party/pigweed/src/pw_sync/backend.gni:26

### pw_sync_OVERRIDE_SYSTEM_CLOCK_BACKEND_CHECK

Whether the GN asserts should be silenced in ensuring that a compatible
backend for pw_chrono_SYSTEM_CLOCK_BACKEND is chosen.
Set to true to disable the asserts.

**Current value (from the default):** `false`

From //third_party/pigweed/src/pw_sync/backend.gni:46

### pw_sync_RECURSIVE_MUTEX_BACKEND

Backend for the pw_sync module's recursive mutex.

**Current value (from the default):** `""`

From //third_party/pigweed/src/pw_sync/backend.gni:32

### pw_sync_THREAD_NOTIFICATION_BACKEND

Backend for the pw_sync module's thread notification.

**Current value for `target_cpu = "arm64"`:** `"//third_party/pigweed/src/pw_sync:binary_semaphore_thread_notification_backend"`

From //.gn:87

**Overridden from the default:** `""`

From //third_party/pigweed/src/pw_sync/backend.gni:38

**Current value for `target_cpu = "riscv64"`:** `"//third_party/pigweed/src/pw_sync:binary_semaphore_thread_notification_backend"`

From //.gn:87

**Overridden from the default:** `""`

From //third_party/pigweed/src/pw_sync/backend.gni:38

**Current value for `target_cpu = "x64"`:** `"//third_party/pigweed/src/pw_sync:binary_semaphore_thread_notification_backend"`

From //.gn:87

**Overridden from the default:** `""`

From //third_party/pigweed/src/pw_sync/backend.gni:38

### pw_sync_TIMED_MUTEX_BACKEND

Backend for the pw_sync module's timed mutex.

**Current value (from the default):** `""`

From //third_party/pigweed/src/pw_sync/backend.gni:29

### pw_sync_TIMED_THREAD_NOTIFICATION_BACKEND

Backend for the pw_sync module's timed thread notification.

**Current value for `target_cpu = "arm64"`:** `"//third_party/pigweed/src/pw_sync:binary_semaphore_timed_thread_notification_backend"`

From //.gn:88

**Overridden from the default:** `""`

From //third_party/pigweed/src/pw_sync/backend.gni:41

**Current value for `target_cpu = "riscv64"`:** `"//third_party/pigweed/src/pw_sync:binary_semaphore_timed_thread_notification_backend"`

From //.gn:88

**Overridden from the default:** `""`

From //third_party/pigweed/src/pw_sync/backend.gni:41

**Current value for `target_cpu = "x64"`:** `"//third_party/pigweed/src/pw_sync:binary_semaphore_timed_thread_notification_backend"`

From //.gn:88

**Overridden from the default:** `""`

From //third_party/pigweed/src/pw_sync/backend.gni:41

### pw_third_party_boringssl_ALIAS

Create a "$pw_external_boringssl" target that aliases an existing
target. This can be used to fix a diamond dependency conflict if a
downstream project uses its own boringssl target and cannot be changed to
use Pigweed's boringssl exclusively.

**Current value for `target_cpu = "arm64"`:** `"//third_party/boringssl"`

From //.gn:107

**Overridden from the default:** `""`

From //third_party/pigweed/src/third_party/boringssl/boringssl.gni:25

**Current value for `target_cpu = "riscv64"`:** `"//third_party/boringssl"`

From //.gn:107

**Overridden from the default:** `""`

From //third_party/pigweed/src/third_party/boringssl/boringssl.gni:25

**Current value for `target_cpu = "x64"`:** `"//third_party/boringssl"`

From //.gn:107

**Overridden from the default:** `""`

From //third_party/pigweed/src/third_party/boringssl/boringssl.gni:25

### pw_third_party_emboss_CONFIG

**Current value (from the default):** `"//third_party/pigweed/src/third_party/emboss:default_overrides"`

From //third_party/pigweed/src/third_party/emboss/emboss.gni:24

### pw_third_party_emboss_USE_GEN_SOURCES

Use the pre-generated list of sources provided by the Emboss project. This
flag is intended to be temporarily used until downstream users have had
enough time to update their Emboss to a newer version.
TODO(b/449017056): Remove this flag when downstream users have migrated.

**Current value for `target_cpu = "arm64"`:** `"true"`

From //.gn:111

**Overridden from the default:** `"false"`

From //third_party/pigweed/src/third_party/emboss/emboss.gni:30

**Current value for `target_cpu = "riscv64"`:** `"true"`

From //.gn:111

**Overridden from the default:** `"false"`

From //third_party/pigweed/src/third_party/emboss/emboss.gni:30

**Current value for `target_cpu = "x64"`:** `"true"`

From //.gn:111

**Overridden from the default:** `"false"`

From //third_party/pigweed/src/third_party/emboss/emboss.gni:30

### pw_third_party_googletest_ALIAS

If compiling with a custom googletest target, set this variable to point
to its label. Incompatible with dir_pw_third_party_googletest definition.

**Current value for `target_cpu = "arm64"`:** `"//third_party/googletest:gmock_no_testonly"`

From //.gn:156

**Overridden from the default:** `""`

From //third_party/pigweed/src/third_party/googletest/googletest.gni:24

**Current value for `target_cpu = "riscv64"`:** `"//third_party/googletest:gmock_no_testonly"`

From //.gn:156

**Overridden from the default:** `""`

From //third_party/pigweed/src/third_party/googletest/googletest.gni:24

**Current value for `target_cpu = "x64"`:** `"//third_party/googletest:gmock_no_testonly"`

From //.gn:156

**Overridden from the default:** `""`

From //third_party/pigweed/src/third_party/googletest/googletest.gni:24

### pw_third_party_mbedtls_CONFIG_HEADER

**Current value (from the default):** `"//third_party/pigweed/src/third_party/mbedtls/configs/config_pigweed.h"`

From //third_party/pigweed/src/third_party/mbedtls/mbedtls.gni:33

### pw_third_party_nanopb_AGGRESSIVE_NANOPB_PB2_REGEN

Deprecated, does nothing.

**Current value (from the default):** `true`

From //third_party/pigweed/src/third_party/nanopb/nanopb.gni:30

### pw_third_party_nanopb_CONFIG

The build target that overrides the default configuration options for this
module. This should point to a source set that provides defines through a
public config (which may -include a file or add defines directly).

**Current value (from the default):** `"//third_party/pigweed/src/pw_build:empty"`

From //third_party/pigweed/src/third_party/nanopb/nanopb.gni:27

### pw_thread_ID_BACKEND

Backend for the pw_thread module's pw::Thread::id.

**Current value (from the default):** `""`

From //third_party/pigweed/src/pw_thread/backend.gni:19

### pw_thread_OVERRIDE_SYSTEM_CLOCK_BACKEND_CHECK

Whether the GN asserts should be silenced in ensuring that a compatible
backend for pw_chrono_SYSTEM_CLOCK_BACKEND is chosen.
Set to true to disable the asserts.

**Current value (from the default):** `false`

From //third_party/pigweed/src/pw_thread/backend.gni:42

### pw_thread_SLEEP_BACKEND

Backend for the pw_thread module's pw::thread::sleep_{for,until}.

**Current value for `target_cpu = "arm64"`:** `"//third_party/pigweed/src/pw_thread_stl:sleep"`

From //.gn:90

**Overridden from the default:** `""`

From //third_party/pigweed/src/pw_thread/backend.gni:22

**Current value for `target_cpu = "riscv64"`:** `"//third_party/pigweed/src/pw_thread_stl:sleep"`

From //.gn:90

**Overridden from the default:** `""`

From //third_party/pigweed/src/pw_thread/backend.gni:22

**Current value for `target_cpu = "x64"`:** `"//third_party/pigweed/src/pw_thread_stl:sleep"`

From //.gn:90

**Overridden from the default:** `""`

From //third_party/pigweed/src/pw_thread/backend.gni:22

### pw_thread_TEST_THREAD_CONTEXT_BACKEND

Backend for the pw_thread module's pw::thread::test_thread_context.

**Current value (from the default):** `""`

From //third_party/pigweed/src/pw_thread/backend.gni:37

### pw_thread_THREAD_BACKEND

Backend for the pw_thread module's pw::Thread to create threads.

**Current value (from the default):** `""`

From //third_party/pigweed/src/pw_thread/backend.gni:25

### pw_thread_THREAD_CREATION_BACKEND

**Current value (from the default):** `"//third_party/pigweed/src/pw_thread:generic_thread_creation_unsupported"`

From //third_party/pigweed/src/pw_thread/backend.gni:31

### pw_thread_THREAD_ITERATION_BACKEND

Backend for the pw_thread module's pw::thread::thread_iteration.

**Current value (from the default):** `""`

From //third_party/pigweed/src/pw_thread/backend.gni:45

### pw_thread_YIELD_BACKEND

Backend for the pw_thread module's pw::thread::yield.

**Current value (from the default):** `""`

From //third_party/pigweed/src/pw_thread/backend.gni:34

### pw_toolchain_CLANG_PREFIX

This flag allows you to specify a prefix to use for clang, clang++,
and llvm-ar binaries to use when compiling with a clang-based toolchain.
This is useful for debugging toolchain-related issues by building with an
externally-provided toolchain.

Pigweed toolchains should NOT override this variable so projects or users
can control it via `.gn` or by setting it as a regular gn argument (e.g.
`gn gen --args='pw_toolchain_CLANG_PREFIX=/path/to/my-llvm-'`).

Examples:
  pw_toolchain_CLANG_PREFIX = ""
  command: "clang" (from PATH)

  pw_toolchain_CLANG_PREFIX = "my-"
  command: "my-clang" (from PATH)

  pw_toolchain_CLANG_PREFIX = "/bin/my-"
  command: "/bin/my-clang" (absolute path)

  pw_toolchain_CLANG_PREFIX = "//environment/clang_next/"
  command: "../environment/clang_next/clang" (relative path)

GN templates should use `pw_toolchain_clang_tools.*` to get the intended
command string rather than relying directly on pw_toolchain_CLANG_PREFIX.

If the prefix begins with "//", it will be rebased to be relative to the
root build directory.

**Current value (from the default):** `"//prebuilt/third_party/bin/"`

From //third_party/pigweed/src/pw_toolchain/clang_tools.gni:58

### pw_toolchain_COVERAGE_ENABLED

Indicates if this toolchain supports generating coverage reports from
pw_test targets.

For example, the static analysis toolchains that run `clang-tidy` instead
of the test binary itself cannot generate coverage reports.

This is typically set by individual toolchains and not by GN args.

**Current value (from the default):** `false`

From //third_party/pigweed/src/pw_toolchain/host_clang/toolchains.gni:32

### pw_toolchain_PROFILE_SOURCE_FILES

List of source files to selectively collect coverage.

**Current value (from the default):** `[]`

From //third_party/pigweed/src/pw_toolchain/host_clang/toolchains.gni:35

### pw_toolchain_RBE_DEBUG

**Current value (from the default):** `false`

From //third_party/pigweed/src/pw_toolchain/rbe.gni:26

### pw_toolchain_RUST_PREFIX

This flag allows you to specify a prefix for rustc.

This follows the same rules as pw_toolchain_CLANG_PREFIX, see above for
more information.

If the prefix begins with "//", it will be rebased to be relative to the
root build directory.

**Current value (from the default):** `""`

From //third_party/pigweed/src/pw_toolchain/clang_tools.gni:67

### pw_toolchain_SANITIZERS

Sets the sanitizer to pass to clang. Valid values are "address", "memory",
"thread", "undefined", "undefined_heuristic".

**Current value (from the default):** `[]`

From //third_party/pigweed/src/pw_toolchain/host_clang/toolchains.gni:23

### pw_toolchain_SCOPE

Scope defining the current toolchain. Contains all of the arguments required
by the generate_toolchain template. This should NOT be manually modified.

**Current value (from the default):** `{ }`

From //third_party/pigweed/src/pw_toolchain/toolchain_args.gni:18

### pw_toolchain_STATIC_ANALYSIS_SKIP_INCLUDE_PATHS

Disable clang-tidy for specific include paths. In the clang-tidy command,
include paths that end with one of these, or match as a regular expression,
are switched from -I to -isystem, which causes clang-tidy to ignore them.
Unfortunately, clang-tidy provides no other way to filter header files.

For example, the following ignores header files in "repo/include":

  pw_toolchain_STATIC_ANALYSIS_SKIP_INCLUDE_PATHS = ["repo/include"]

While the following ignores all third-party header files:

  pw_toolchain_STATIC_ANALYSIS_SKIP_INCLUDE_PATHS = [".*/third_party/.*"]


**Current value (from the default):** `[]`

From //third_party/pigweed/src/pw_toolchain/static_analysis_toolchain.gni:48

### pw_toolchain_STATIC_ANALYSIS_SKIP_SOURCES_RES

Regular expressions matching the paths of the source files to be excluded
from the analysis. clang-tidy provides no alternative option.

For example, the following disables clang-tidy on all source files in the
third_party directory:

  pw_toolchain_STATIC_ANALYSIS_SKIP_SOURCES_RES = ["third_party/.*"]


**Current value (from the default):** `[]`

From //third_party/pigweed/src/pw_toolchain/static_analysis_toolchain.gni:33

### pw_toolchain_USE_RBE

**Current value (from the default):** `false`

From //third_party/pigweed/src/pw_toolchain/rbe.gni:20

### pw_unit_test_AUTOMATIC_RUNNER

Path to a test runner to automatically run unit tests after they are built.

If set, a ``pw_test`` target's ``<target_name>.run`` action will invoke the
test runner specified by this argument, passing the path to the unit test to
run. If this is unset, the ``pw_test`` target's ``<target_name>.run`` step
will do nothing.

Targets that don't support parallelized execution of tests (e.g. a on-device
test runner that must flash a device and run the test in serial) should
set pw_unit_test_POOL_DEPTH to 1.

Type: string (name of an executable on the PATH, or path to an executable)
Usage: toolchain-controlled only

**Current value (from the default):** `""`

From //third_party/pigweed/src/pw_unit_test/test.gni:53

### pw_unit_test_AUTOMATIC_RUNNER_ARGS

Optional list of arguments to forward to the automatic runner.

Type: list of strings (args to pass to pw_unit_test_AUTOMATIC_RUNNER)
Usage: toolchain-controlled only

**Current value (from the default):** `[]`

From //third_party/pigweed/src/pw_unit_test/test.gni:59

### pw_unit_test_AUTOMATIC_RUNNER_TIMEOUT

Optional timeout to apply when running tests via the automatic runner.
Timeout is in seconds. Defaults to empty which means no timeout.

**Current value (from the default):** `""`

From //third_party/pigweed/src/pw_unit_test/test.gni:63

### pw_unit_test_BACKEND

The unit test framework implementation. Defaults to
pw_unit_test:light, which implements a subset of GoogleTest safe to run on
device. Set to //pw_unit_test:googletest when using GoogleTest.

Type: string (GN path to a source set)
Usage: toolchain-controlled only

**Current value for `target_cpu = "arm64"`:** `"//third_party/pigweed/src/pw_unit_test:googletest"`

From //.gn:151

**Overridden from the default:** `"//third_party/pigweed/src/pw_unit_test:light"`

From //third_party/pigweed/src/pw_unit_test/test.gni:31

**Current value for `target_cpu = "riscv64"`:** `"//third_party/pigweed/src/pw_unit_test:googletest"`

From //.gn:151

**Overridden from the default:** `"//third_party/pigweed/src/pw_unit_test:light"`

From //third_party/pigweed/src/pw_unit_test/test.gni:31

**Current value for `target_cpu = "x64"`:** `"//third_party/pigweed/src/pw_unit_test:googletest"`

From //.gn:151

**Overridden from the default:** `"//third_party/pigweed/src/pw_unit_test:light"`

From //third_party/pigweed/src/pw_unit_test/test.gni:31

### pw_unit_test_EXECUTABLE_TARGET_TYPE

The name of the GN target type used to build pw_unit_test executables.

Type: string (name of a GN template)
Usage: toolchain-controlled only

**Current value (from the default):** `"pw_executable"`

From //third_party/pigweed/src/pw_unit_test/test.gni:92

### pw_unit_test_EXECUTABLE_TARGET_TYPE_FILE

The path to the .gni file that defines pw_unit_test_EXECUTABLE_TARGET_TYPE.

If pw_unit_test_EXECUTABLE_TARGET_TYPE is not the default of
`pw_executable`, this .gni file is imported to provide the template
definition.

Type: string (path to a .gni file)
Usage: toolchain-controlled only

**Current value (from the default):** `""`

From //third_party/pigweed/src/pw_unit_test/test.gni:102

### pw_unit_test_MAIN

Implementation of a main function for ``pw_test`` unit test binaries. Must
be set to an appropriate target for the pw_unit_test backend.

Type: string (GN path to a source set)
Usage: toolchain-controlled only

**Current value for `target_cpu = "arm64"`:** `"//src/lib/fxl/test:gtest_main"`

From //.gn:149

**Overridden from the default:** `"//third_party/pigweed/src/pw_unit_test:simple_printing_main"`

From //third_party/pigweed/src/pw_unit_test/test.gni:38

**Current value for `target_cpu = "riscv64"`:** `"//src/lib/fxl/test:gtest_main"`

From //.gn:149

**Overridden from the default:** `"//third_party/pigweed/src/pw_unit_test:simple_printing_main"`

From //third_party/pigweed/src/pw_unit_test/test.gni:38

**Current value for `target_cpu = "x64"`:** `"//src/lib/fxl/test:gtest_main"`

From //.gn:149

**Overridden from the default:** `"//third_party/pigweed/src/pw_unit_test:simple_printing_main"`

From //third_party/pigweed/src/pw_unit_test/test.gni:38

### pw_unit_test_POOL_DEPTH

The maximum number of unit tests that may be run concurrently for the
current toolchain. Setting this to 0 disables usage of a pool, allowing
unlimited parallelization.

Note: A single target with two toolchain configurations (e.g. release/debug)
      will use two separate test runner pools by default. Set
      pw_unit_test_POOL_TOOLCHAIN to the same toolchain for both targets to
      merge the pools and force serialization.

Type: integer
Usage: toolchain-controlled only

**Current value (from the default):** `0`

From //third_party/pigweed/src/pw_unit_test/test.gni:76

### pw_unit_test_POOL_TOOLCHAIN

The toolchain to use when referring to the pw_unit_test runner pool. When
this is disabled, the current toolchain is used. This means that every
toolchain will use its own pool definition. If two toolchains should share
the same pool, this argument should be by one of the toolchains to the GN
path of the other toolchain.

Type: string (GN path to a toolchain)
Usage: toolchain-controlled only

**Current value (from the default):** `""`

From //third_party/pigweed/src/pw_unit_test/test.gni:86

### pw_unit_test_TESTONLY

If true, the pw_unit_test target, pw_test targets, and pw_test_group targets
will define `testonly = true`.  This is false by default for backwards
compatibility.

**Current value for `target_cpu = "arm64"`:** `true`

From //.gn:152

**Overridden from the default:** `false`

From //third_party/pigweed/src/pw_unit_test/test.gni:107

**Current value for `target_cpu = "riscv64"`:** `true`

From //.gn:152

**Overridden from the default:** `false`

From //third_party/pigweed/src/pw_unit_test/test.gni:107

**Current value for `target_cpu = "x64"`:** `true`

From //.gn:152

**Overridden from the default:** `false`

From //third_party/pigweed/src/pw_unit_test/test.gni:107

### qemu_boot_format

Boot format to use with QEMU. This chooses the boot format to use with
QEMU, determining which boot shim implementation is used as QEMU "kernel".
Valid alternatives vary by machine, but include "linuxboot".

**Current value (from the default):** `"linuxboot"`

From //zircon/kernel/phys/qemu.gni:165

### rbe_extra_reproxy_configs

Additional reproxy configuration files.
These are effectively concatenated with the main `reproxy_config_file`
in order of appearance.  Settings in later files in this list take
precedence over those earlier in the list.

**Current value (from the default):** `[]`

From //build/toolchain/rbe.gni:31

### rbe_mode

The overall mode for RBE to be operating in.  The valid values are:
 * 'off' => RBE is fully disabled. This is suitable for offline building
            using only local resources.
 * 'legacy_default' => The standard RBE configuration used if not otherwise
                       specified. This contains a mix of enabled/disabled
                       remote services.
 * 'remote_full' => Run as many actions remotely as possible, including
                 cache-misses, which reduces use of local resources.
 * 'racing' => Race remote against local execution, for some action types.
 * 'cloudtop' => An RBE configuration that's optimized for running on a
                 cloudtop. Suitable for high-bandwidth connections to
                 remote services and downloading remote outputs.
 * 'workstation' => An RBE configuration that's optimized for running on a
                 large workstation. Suitable for machines with a large
                 number of fast cores and a high bandwidth connection to
                 remote services.
 * 'infra' => The RBE configuration recommended for CI/CQ bots.
              Also uses high-bandwidth.
 * 'remote_cache_only' => Use RBE only as a remote-cache: on cache-miss,
                          execute locally instead of remotely.
 * 'low_bandwidth_remote' => An RBE configuration for low network bandwidth.
                             Saves bandwidth by avoiding downloading some
                             intermediate results.
 * 'nocache' => Force all cache-misses, and re-execute remotely.

**Current value (from the default):** `"off"`

From //build/toolchain/rbe_modes.gni:44

### rbe_settings_overrides

Overridden settings for the RBE mode.  This is a set of override values for
variables whose default values are set by the chosen RBE mode (above).

**Current value (from the default):** `{ }`

From //build/toolchain/rbe_modes.gni:48

### recovery_board_configuration_label

Possibly use a different configuration for recovery than for the main
product.  By default, use the same board.

This is a separate declare_args() block so that it can default to the
provided value for 'board_configuration_label'

**Current value (from the default):** `"//boards/arm64"`

From //build/board.gni:42

### redirect_guest_serial_logs

If true, redirects guest serial output to the system log.

**Current value (from the default):** `false`

From //src/virtualization/bin/args.gni:11

### remove_default_configs

**Current value for `target_cpu = "arm64"`:** `["//third_party/pigweed/src/pw_build:reduced_size"]`

From //.gn:165

**Overridden from the default:** `[]`

From //third_party/pigweed/src/pw_build/gn_internal/defaults.gni:36

**Current value for `target_cpu = "riscv64"`:** `["//third_party/pigweed/src/pw_build:reduced_size"]`

From //.gn:165

**Overridden from the default:** `[]`

From //third_party/pigweed/src/pw_build/gn_internal/defaults.gni:36

**Current value for `target_cpu = "x64"`:** `["//third_party/pigweed/src/pw_build:reduced_size"]`

From //.gn:165

**Overridden from the default:** `[]`

From //third_party/pigweed/src/pw_build/gn_internal/defaults.gni:36

### remove_default_public_deps

**Current value (from the default):** `[]`

From //third_party/pigweed/src/pw_build/gn_internal/defaults.gni:37

### repository_publish_blob_copy_mode

Controls which mode to use when copying blobs into the repository.
Supported modes are:

* `copy`: copy the blob if the blob does not already exist in the
  repository. This will use copy-on-write to efficiently copy the blob on
  file systems that support it.

* `copy-overwrite`: always copy the blob, overwriting any blob that
  exists in the blob repository. This will use copy-on-write to efficiently
  copy the blob on file systems that support it.

* `hard-link`: hard link the blob into the repository, or copy if we cannot
  create a hard link between the blob and the blob repository. Note that it
  is possible to modify the blob through the hard link, which would result
  in the blob not matching the blob's merkle.

**Current value (from the default):** `"hard-link"`

From //src/sys/pkg/bin/package-tool/package-tool.gni:140

### restat_cc

Set to true to make C++ compiles preserve timestamps of unchanged outputs.
re-client provides this feature out-of-the-box with
--preserve_unchanged_output_mtime, so it makes sense to default to true
when using `cxx_rbe_enable`.  When not using re-client, you can still
get write-if-change behavior through the `restat_wrapper` script,
but at the cost of the wrapper overhead (tradeoff vs. action pruning).

**Current value (from the default):** `false`

From //build/toolchain/restat.gni:28

### restat_rust

Set to true to make Rust compiles preserve timestamps of unchanged outputs.

**Current value (from the default):** `true`

From //build/toolchain/restat.gni:20

### riscv64_enable_vector

Whether to enable the use of RISC-V vector instructions.

**Current value (from the default):** `true`

From //build/config/riscv64/riscv64.gni:10

### rust_cap_lints

Sets the maximum lint level. "deny" will make all warnings into errors,
"warn" preserves them as warnings, and "allow" will ignore warnings.
LINT.IfChange

**Current value (from the default):** `"deny"`

From //build/rust/config.gni:64

### rust_debug_assertions

Enable debug assertions, e.g. for overflow checking.

**Current value (from the default):** `false`

From //build/rust/config.gni:26

### rust_incremental

Enable incremental rust compilation. Takes a path to the directory to use
as the cache.

**Current value (from the default):** `""`

From //build/rust/build.gni:8

### rust_lto

Sets the default LTO type for rustc builds.

**Current value (from the default):** `""`

From //build/rust/config.gni:59

### rust_one_rlib_per_dir

To avoid build nondeterminism due to extern search paths resolving
to more than one path during a build, this option places every rlib
into its own exclusive directory. This requires
`rustc_use_response_file = true` due to the command-line bloat this causes.

**Current value (from the default):** `true`

From //build/rust/build.gni:22

### rust_parallel_frontend_threads

Enable the rust parallel front-end with N threads

**Current value (from the default):** `false`

From //build/config/rust/BUILD.gn:29

### rust_rbe_check

Run one of the more expensive checks, intended for CI.
All of these require rust_rbe_enable=true.

One of:

  * "none": No additional check.

  * "determinism":
      Check of determinism of rustc targets by running locally twice
      and comparing outputs, failing if any differences are found.
      Even though this check doesn't involve RBE, it uses the same
      wrapper script, which knows what output files to expect and
      compare.

      Build outputs that depend on time are discouraged because they
      impact caching.
      If your result depends on the current time, this check will
      definitely fail.  If it depends on only the date, there is still
      a nonzero chance of failure, if the rerun falls on the next day.

  * "consistency":
      Check consistency between local and remote rust compiles,
      by running both and comparing results.


**Current value (from the default):** `"none"`

From //build/toolchain/rbe.gni:144

### rust_rbe_download_rlibs

TODO(b/42084033): Controls whether or not to download (intermediate)
rlibs from remote Rust build actions.

**Current value (from the default):** `true`

From //build/toolchain/rbe.gni:156

### rust_rbe_download_unstripped_binaries

**Current value (from the default):** `true`

From //build/toolchain/rbe.gni:152

### rust_rbe_enable

Set to true to enable distributed compilation of Rust using RBE.

**Current value (from the default):** `false`

From //build/toolchain/rbe.gni:97

### rust_rbe_exec_strategy

One of:

  * "remote": Execute action remotely on cache miss.
        The remote cache is always updated with this result.

  * "local": Lookup action in the remote cache, but execute action
        locally on cache miss.  The locally produced result is
        not uploaded to the remote cache.

  * "remote_local_fallback": Execute action remotely first.
        If that fails, run locally instead.  The locally produced
        results are not uploaded to the remote cache.

  * "racing": Race local vs. remote execution, take the first to finish.

  * "nocache": Force remote execution without using cached results.
        This can be useful for benchmarking cache-miss scenarios.

  (There are other rewrapper options that are not exposed.)

**Current value (from the default):** `"remote"`

From //build/toolchain/rbe.gni:118

### rust_v0_symbol_mangling

Controls whether the rust compiler uses v0 symbol mangling scheme
(see https://github.com/rust-lang/rfcs/blob/HEAD/text/2603-rust-symbol-name-mangling-v0.md).

**Current value (from the default):** `true`

From //build/config/rust/BUILD.gn:26

### rust_virtio_net

If true, uses the new Rust virtio-net device instead of the legacy C++
device.

**Current value (from the default):** `true`

From //src/virtualization/bin/args.gni:8

### rustc_prefix

Sets a custom base directory for `rustc` and `cargo`.
This can be used to test custom Rust toolchains.
LINT.IfChange

**Current value (from the default):** `"//prebuilt/third_party/rust/linux-x64"`

From //build/rust/config.gni:21

### rustc_timeout

A timeout to catch rustc hangs, expressed in seconds. A value of zero
means no timeout

**Current value (from the default):** `0`

From //build/rust/build.gni:34

### rustc_use_response_files

Place lengthy rustdeps and externs (GN) into ninja response files.
Response files are needed to get around command line length limitations.
rustc support for response files (as needed in our GN build) was
added with revision 'git_revision:dfe53afaebd817f334d8ef9dc75a5cd2562cf6e6'.

**Current value (from the default):** `true`

From //build/rust/build.gni:16

### rustc_version_description

Human-readable identifier for the toolchain version.

TODO(tmandry): Make this the same repo/revision info from `rustc --version`.
e.g., clang_version_description = read_file("$_rustc_lib_dir/VERSION")

**Current value (from the default):** `""`

From //build/rust/config.gni:56

### rustc_version_string

This is a string identifying the particular toolchain version in use.  Its
only purpose is to be unique enough that it changes when switching to a new
toolchain, so that recompilations with the new compiler can be triggered.

When using the prebuilt, this is ignored and the CIPD instance ID of the
prebuilt is used.

**Current value (from the default):** `"Oo_9vtP3i6NZ5emkZ3pdJ8jhPZxIIiQ794sWpXrLIl8C"`

From //build/rust/config.gni:50

### rustdoc_extern_html_root_url_base

**Current value (from the default):** `"https://fuchsia-docs.firebaseapp.com/rust"`

From //build/rust/config.gni:87

### scenic_enable_vulkan_validation

Include the vulkan validation layers in scenic.

**Current value (from the default):** `false`

From //src/ui/scenic/lib/utils/build_args.gni:10

### scheduler_extra_invariant_validation

Enables extra (expensive) validation of scheduler invariants to assist in
debugging changes to the scheduler's behavior.

**Current value (from the default):** `false`

From //zircon/kernel/params.gni:47

### scheduler_lock_spin_tracing_compressed

Enables compressed records when tracing lock-spin events.  The events will
be more difficult to interpret in a trace visualizer, but will take less
space and provide the same information to scripts which parse lock trace
data.

**Current value (from the default):** `false`

From //zircon/kernel/params.gni:57

### scheduler_lock_spin_tracing_enabled

Enables scheduler lock-spinning trace events for trace-based scheduler
performance analysis.

**Current value (from the default):** `false`

From //zircon/kernel/params.gni:51

### scheduler_queue_tracing_enabled

Enables scheduler queue tracing for trace-based scheduler performance
analysis.

**Current value (from the default):** `false`

From //zircon/kernel/params.gni:43

### scheduler_tracing_level

The level of detail for scheduler traces when enabled. Values greater than
zero add increasing details at the cost of increased trace buffer use.

0 = Default (standard) kernel:sched tracing.
1 = Adds duration traces for key scheduler operations.
2 = Adds bandwidth counters for utilization and processing rates.
3 = Adds run queue events.
4 = Adds flow events from wakeup to running state.
5 = Adds counters for misc stats and estimates.
6 = Adds detailed internal events.

**Current value (from the default):** `0`

From //zircon/kernel/params.gni:39

### scudo_default_options

Default [Scudo](https://llvm.org/docs/ScudoHardenedAllocator.html) options
(before the `SCUDO_OPTIONS` environment variable is read at runtime).
Scudo is the memory allocator in Fuchsia's C library, so this affects all
Fuchsia programs.  This can be a list of strings or a single string.

This operates similarly to [`asan_default_options`](#asan_default_options)
and its cousins for other sanitizers, but is slightly different.  If this
variable is empty, then no `__scudo_default_options` function is injected
into programs at all.  Individual targets can use dependencies on
sanitizer_extra_options() targets to cause options to be injected, and that
will be compatible with any build-wide settings of `scudo_default_options`.
Programs **can** define their own `__scudo_default_options` functions, but
doing so will break all builds with this variable is set to nonempty, so
any program in the build that needs such a setting (which should be only in
tests) can use the sanitizer_extra_options() mechanism instead.

**Current value (from the default):** `[]`

From //build/config/sanitizers/sanitizer_default_options.gni:84

### sdk_archive_labels

Extra idk_archive() labels to be uploaded to the artifacts store. This is an
extension mechanism for IDK bits outside of the main repository.

**Current value (from the default):** `[]`

From //BUILD.gn:100

### sdk_cross_compile_host_tools

Whether to cross-compile SDK tools for all supported host toolchains,
rather than just the current host toolchains.
For example, if this is true then for instance if building on linux x64 then
you'll also build SDK host tools for linux arm64.

**Current value (from the default):** `false`

From //sdk/config.gni:16

### sdk_id

Identifier for the Core SDK.
LINT.IfChange

**Current value (from the default):** `"33.99991231.0.1"`

From //sdk/config.gni:8

### sdk_inside_sub_build

Whether currently building a sub-build (vs. the main build targeting
"PLATFORM" and the primary target CPU architecture).
Prefer using other mechanisms when possible.
Can be true for any API level, including "PLATFORM", and CPU architecture.

**Current value (from the default):** `false`

From //build/sdk/config.gni:25

### sdk_max_simultaneous_sub_builds

When enable_jobserver is not set, this provides an upper bound on the
maximum number of subbuilds that may be running at the same time.
A larger number means these good things:
- Better parallelization of the inherently single-threaded parts of GN and
  ninja.
- Better parallelization in the face of "stragglers" in the build -
  situations where each subbuild is executing a small number of actions.

But also these bad things:
- More memory usage, potentially leading to swapping and slowdowns.
- More CPU contention when the build process is actually CPU-bound.
- Potentially forcing a lower value of `sdk_sub_build_parallelism`, since
  the total load is proportional to `sdk_max_simultaneous_sub_builds *
  sdk_sub_build_parallelism`.

5 was chosen mostly because it's the number of fingers on each of my hands.

**Current value (from the default):** `5`

From //build/sdk/config.gni:43

### sdk_sub_build_max_load_average

When enable_jobserver is not set, value of `-l` to pass to ninja during a
subbuild. If the system load average on the system goes beyond this value,
ninja will throttle itself. If left blank, the subbuild script will make a
guess.

**Current value (from the default):** `""`

From //build/sdk/config.gni:56

### sdk_sub_build_parallelism

When enable_jobserver is not set, value of `-j` to pass to ninja during a
subbuild. Note that up to `sdk_max_simultaneous_sub_builds` subbuilds may be
happening in parallel, so the number of concurrent actions may go as high as
this number times the number of concurrent subbuilds. If left blank, the
subbuild script will make a guess.

**Current value (from the default):** `""`

From //build/sdk/config.gni:50

### sdk_sub_build_verbose

Set to `true` to enable verbose logging during IDK subbuilds.

**Current value (from the default):** `false`

From //build/sdk/config.gni:59

### select_variant

List of "selectors" to request variant builds of certain targets.
Each selector specifies matching criteria and a chosen variant.
The first selector in the list to match a given target determines
which variant is used for that target.

Each selector is either a string or a scope.  A shortcut selector is
a string; it gets expanded to a full selector.  A full selector is a
scope, described below.

A string selector can match a name in
[`select_variant_shortcuts`](#select_variant_shortcuts).  If it's not a
specific shortcut listed there, then it can be the name of any variant
described in [`known_variants`](#known_variants).
A `selector` that's a simple variant name selects for every binary
built in the target toolchain: `{ host=false variant=selector }`.

If a string selector contains a slash, then it's `"shortcut/filename"`
and selects only the binary in the target toolchain whose `output_name`
matches `"filename"`, i.e. it adds `output_name=["filename"]` to each
selector scope that the shortcut's name alone would yield.

The scope that forms a full selector defines some of these:

    variant (required)
        [string or `false`] The variant that applies if this selector
        matches.  This can be `false` to choose no variant, or a string
        that names the variant.  See
        [`known_variants`](#known_variants).

The rest below are matching criteria.  All are optional.
The selector matches if and only if all of its criteria match.
If none of these is defined, then the selector always matches.

The first selector in the list to match wins and then the rest of
the list is ignored.  To construct more complex rules, use a blocklist
selector with `variant=false` before a catch-all default variant, or
a list of specific variants before a catch-all false variant.

Each "[strings]" criterion is a list of strings, and the criterion
is satisfied if any of the strings matches against the candidate string.

    host
        [boolean] If true, the selector matches in the host toolchain.
        If false, the selector matches in non-host toolchains.

    kernel
        [boolean] If true, the selector matches in is_kernel toolchains.
        If false, the selector matches in non-kernel toolchains.

    testonly
        [boolean] If true, the selector matches targets with testonly=true.
        If false, the selector matches in targets without testonly=true.

    target_type
        [strings]: `"executable"`, `"loadable_module"`, or `"fuchsia_driver"`

    output_name
        [strings]: target's `output_name` (default: its `target name`)

    label
        [strings]: target's full label with `:` (without toolchain suffix)

    name
        [strings]: target's simple name (label after last `/` or `:`)

    dir
        [strings]: target's label directory (`//dir` for `//dir:name`).

**Current value (from the default):** `[]`

From //build/config/BUILDCONFIG.gn:777

### select_variant_canonical

*This should never be set as a build argument.*
It exists only to be set in `toolchain_args`.
See //build/toolchain/clang_toolchain.gni for details.

**Current value (from the default):** `[]`

From //build/config/BUILDCONFIG.gn:782

### select_variant_shortcuts

List of short names for commonly-used variant selectors.  Normally this
is not set as a build argument, but it serves to document the available
set of short-cut names for variant selectors.  Each element of this list
is a scope where `.name` is the short name and `.select_variant` is a
a list that can be spliced into [`select_variant`](#select_variant).

**Current value (from the default):**

```none
[{
  name = "host_asan"
  select_variant = [{
  host = true
  variant = "asan"
}]
}, {
  name = "host_asan-ubsan"
  select_variant = [{
  host = true
  variant = "asan-ubsan"
}]
}, {
  name = "host_coverage"
  select_variant = [{
  host = true
  variant = "coverage"
}]
}, {
  name = "host_coverage-rust"
  select_variant = [{
  host = true
  variant = "coverage-rust"
}]
}, {
  name = "host_profile"
  select_variant = [{
  host = true
  variant = "profile"
}]
}, {
  name = "host_profile-rust"
  select_variant = [{
  host = true
  variant = "profile-rust"
}]
}, {
  name = "host_tsan"
  select_variant = [{
  host = true
  variant = "tsan"
}]
}, {
  name = "kernel_lto-cfi"
  select_variant = [{
  kernel = true
  variant = "lto-cfi"
}]
}, {
  name = "kernel_thinlto-cfi"
  select_variant = [{
  kernel = true
  variant = "thinlto-cfi"
}]
}, {
  name = "kubsan"
  select_variant = [{
  _zircon_cpu = "arm64"
  dir = ["//zircon/kernel/bin", "//zircon/kernel/arch/arm64/phys", "//zircon/kernel/arch/arm64/phys/boot-shim", "//zircon/kernel/arch/arm64/phys/efi", "//zircon/kernel/phys", "//zircon/kernel/phys/boot-shim", "//zircon/kernel/phys/efi", "//zircon/kernel/phys/test"]
  variant = "ubsan"
}]
}]
```

From //build/config/BUILDCONFIG.gn:552

### skip_buildtools_check

Skip buildtools dependency checks (needed for ChromeOS).

**Current value (from the default):** `false`

From //third_party/perfetto/gn/perfetto.gni:467

### smp_max_cpus

**Current value (from the default):** `16`

From //zircon/kernel/params.gni:14

### spinel_platform_header

Platform portability header for spinel.

**Current value (from the default):** `"\"spinel_platform.h\""`

From //third_party/openthread/src/lib/spinel/BUILD.gn:32

### stack_clash_protection

Whether to compile with stack-clash-protection enabled
https://clang.llvm.org/docs/ClangCommandLineReference.html#cmdoption-clang-fstack-clash-protection
https://blog.llvm.org/posts/2021-01-05-stack-clash-protection/

**Current value (from the default):** `true`

From //build/config/clang/stack_clash_protection.gni:9

### starnix_enable_console_tool

The console tool is intended only for developers. Disabled by default to
prevent automated use.

**Current value (from the default):** `false`

From //src/developer/ffx/tools/starnix/BUILD.gn:13

### starnix_enable_rust_backtrace

Whether to default the RUST_BACKTRACE environment variable to 1 for the
Starnix kernel. This is useful for identifying the stacks that acquired a
lock in a deadlock cycle.

**Current value (from the default):** `false`

From //src/starnix/build/args.gni:29

### starnix_enable_trace_and_debug_logs_in_release

Compiles-in trace and debug logging in release builds. By default, these
logs are compiled-out for performance reasons.

This option does not affect usage of the `fuchsia_trace` crate, which is
independent of Rust's tracing library.

For more information, see
https://fuchsia-review.googlesource.com/c/fuchsia/+/929995.

**Current value (from the default):** `false`

From //src/starnix/build/args.gni:24

### starnix_force_build_host_tests

Whether or not to force the build of x64 ELF executables for the syscall
tests, colliqually known as "host tests." The default preference is to build
and run tests against Machina instead.

**Current value (from the default):** `false`

From //src/starnix/build/args.gni:9

### starnix_host_linux_toolchain

We want to use a newer sysroot than is available by default in
$host_toolchain.

**Current value (from the default):** `"//build/toolchain:linux_x64"`

From //src/starnix/build/args.gni:33

### starnix_kgsl_debug

This flag enables verbose logging and debug checks.

**Current value (from the default):** `false`

From //src/starnix/modules/kgsl/BUILD.gn:15

### starnix_kgsl_enable

This flag affects the return value of a signal ioctl used by certain
container binaries to direct them to use kgsl-based or magma-based drivers.
It does not affect the availability of the kgsl device within the container.

**Current value (from the default):** `false`

From //src/starnix/modules/kgsl/BUILD.gn:12

### starnix_sync_detect_lock_dep_cycles

Whether to use lockdep to detect cycles in the lock acquisition graph.
Enabled by default in balanced and debug modes.

**Current value (from the default):** `false`

From //src/starnix/lib/starnix_sync/build/args.gni:8

### sysmem_contiguous_guard_page_count

**Current value (from the default):** `-1`

From //src/sysmem/server/BUILD.gn:25

### sysmem_contiguous_guard_pages_fatal

**Current value (from the default):** `false`

From //src/sysmem/server/BUILD.gn:23

### sysmem_contiguous_guard_pages_internal

**Current value (from the default):** `false`

From //src/sysmem/server/BUILD.gn:24

### sysmem_contiguous_guard_pages_unused

**Current value (from the default):** `false`

From //src/sysmem/server/BUILD.gn:26

### sysmem_contiguous_guard_pages_unused_cycle_seconds

**Current value (from the default):** `600`

From //src/sysmem/server/BUILD.gn:28

### sysmem_contiguous_guard_pages_unused_fraction_denominator

**Current value (from the default):** `128`

From //src/sysmem/server/BUILD.gn:27

### sysmem_contiguous_memory_size

**Current value (from the default):** `-1`

From //src/sysmem/server/BUILD.gn:19

### sysmem_contiguous_memory_size_percent

**Current value (from the default):** `5`

From //src/sysmem/server/BUILD.gn:20

### sysmem_protected_memory_size

**Current value (from the default):** `0`

From //src/sysmem/server/BUILD.gn:21

### sysmem_protected_memory_size_percent

**Current value (from the default):** `-1`

From //src/sysmem/server/BUILD.gn:22

### sysmem_protected_ranges_disable_dynamic

**Current value (from the default):** `false`

From //src/sysmem/server/BUILD.gn:29

### target_cpu

**Current value for `target_cpu = "arm64"`:** `"arm64"`

From //out/not-default/args.gn:10

**Overridden from the default:** `""`

**Current value for `target_cpu = "riscv64"`:** `"riscv64"`

From //out/not-default/args.gn:10

**Overridden from the default:** `""`

**Current value for `target_cpu = "x64"`:** `"x64"`

From //out/not-default/args.gn:10

**Overridden from the default:** `""`

### target_labels

Targets to add to the build graph in the default (Fuchsia) toolchain.

**Current value for `target_cpu = "arm64"`:** `["//bundles/docs_roller"]`

From //out/not-default/args.gn:15

**Overridden from the default:** `[]`

From //BUILD.gn:35

**Current value for `target_cpu = "riscv64"`:** `["//bundles/docs_roller"]`

From //out/not-default/args.gn:15

**Overridden from the default:** `[]`

From //BUILD.gn:35

**Current value for `target_cpu = "x64"`:** `["//bundles/docs_roller"]`

From //out/not-default/args.gn:15

**Overridden from the default:** `[]`

From //BUILD.gn:35

### target_os

**Current value (from the default):** `""`

### target_persistent_debuglog_size

Controls (in bytes) the target size of the persistent debug log, in bytes.
Setting this to zero disables all persistent debug log functionality.  Note
that while the system will make an attempt to secure this many bytes for the
persistent debug log, it may not be able to due to limited persistent RAM
resources.  Must be a multiple of 128 bytes.

**Current value (from the default):** `0`

From //zircon/kernel/lib/persistent-debuglog/params.gni:13

### target_sysroot

The absolute path of the sysroot that is used with the target toolchain.

**Current value (from the default):** `""`

From //build/config/sysroot.gni:7

### terminal_bold_font_path

**Current value (from the default):** `"//prebuilt/third_party/fonts/robotomono/RobotoMono-Bold.ttf"`

From //src/ui/bin/terminal/terminal_args.gni:12

### terminal_bold_italic_font_path

**Current value (from the default):** `"//prebuilt/third_party/fonts/robotomono/RobotoMono-BoldItalic.ttf"`

From //src/ui/bin/terminal/terminal_args.gni:20

### terminal_fallback_font_paths

Paths to files to use for fallback fonts

**Current value (from the default):** `[]`

From //src/ui/bin/terminal/terminal_args.gni:23

### terminal_font_path

**Current value (from the default):** `"//prebuilt/third_party/fonts/robotomono/RobotoMono-Regular.ttf"`

From //src/ui/bin/terminal/terminal_args.gni:8

### terminal_italic_font_path

**Current value (from the default):** `"//prebuilt/third_party/fonts/robotomono/RobotoMono-Italic.ttf"`

From //src/ui/bin/terminal/terminal_args.gni:16

### test_durations_file

A file containing historical test duration data for this build
configuration, used used by testsharder to evenly split tests across
shards. It should be set for any builds where testsharder will be run
afterwards.

**Current value (from the default):** `""`

From //BUILD.gn:96

### test_package_labels

**Current value (from the default):** `[]`

From //BUILD.gn:145

### thinlto_cache_dir

ThinLTO cache directory path.

**Current value (from the default):** `"thinlto-cache"`

From //build/config/lto/config.gni:10

### thinlto_jobs

Number of parallel ThinLTO jobs.

**Current value (from the default):** `8`

From //build/config/lto/config.gni:7

### time_trace

Whether to export time traces when building with clang.
https://releases.llvm.org/9.0.0/tools/clang/docs/ReleaseNotes.html#new-compiler-flags

**Current value (from the default):** `false`

From //build/config/clang/time_trace.gni:8

### toolchain_variant

*This should never be set as a build argument.* It exists only to be set in
`toolchain_args`. See
//docs/development/build/gn_concepts/internals/toolchains/build_arguments.md#toolchain_variant
for details and documentation for each field.

**Current value (from the default):**

```none
{
  base = "//build/toolchain/fuchsia:arm64"
}
```

From //build/config/BUILDCONFIG.gn:964

### truncate_build_info_commit_date

Truncate the date in the build_info to midnight UTC, and replace the commit
hash with one that's synthesized from that date.
This is not meant to be used outside this directory. It is only in this .gni
file so that //build/bazel:gn_build_variables_for_bazel can access it.

**Current value (from the default):** `false`

From //build/info/info.gni:23

### tsan_default_options

Default [ThreadSanitizer](https://clang.llvm.org/docs/ThreadSanitizer.html)
options (before the `TSAN_OPTIONS` environment variable is read at runtime).
This can be set as a build argument to affect most "tsan" variants in
$variants (which see), or overridden in $toolchain_args in one of those
variants. This can be a list of strings or a single string.

Note that even if this is empty, programs in this build **cannot** define
their own `__tsan_default_options` C function.  Instead, they can use a
sanitizer_extra_options() target in their `deps` and then any options
injected that way can override that option's setting in this list.

TODO(https://fxbug.dev/42171381): `ignore_noninstrumented_modules=1` can be reevaluated
when/if we have an instrumented libstd for Rust.

**Current value (from the default):** `["ignore_noninstrumented_modules=1"]`

From //build/config/sanitizers/sanitizer_default_options.gni:67

### ubsan_default_options

Default [UndefinedBehaviorSanitizer](https://clang.llvm.org/docs/UndefinedBehaviorSanitizer.html)
options (before the `UBSAN_OPTIONS` environment variable is read at
runtime).  This can be set as a build argument to affect most "ubsan"
variants in $variants (which see), or overridden in $toolchain_args in
one of those variants.  This can be a list of strings or a single string.

Note that even if this is empty, programs in this build **cannot** define
their own `__ubsan_default_options` C function.  Instead, they can use a
sanitizer_extra_options() target in their `deps` and then any options
injected that way can override that option's setting in this list.

**Current value (from the default):** `["print_stacktrace=1", "halt_on_error=1"]`

From //build/config/sanitizers/sanitizer_default_options.gni:49

### universe_package_labels

If you add package labels to this variable, the packages will be included
in the 'universe' package set, which represents all software that is
produced that is to be published to a package repository or to the SDK by
the build.

**Current value (from the default):** `[]`

From //BUILD.gn:32

### update_goldens

Set to true for the golden_file template to implicitly write updated goldens
instead of failing the action or test.
LINT.IfChange

**Current value (from the default):** `false`

From //build/testing/config.gni:9

### use_blink

**Current value (from the default):** `false`

From //build/config/features.gni:13

### use_ccache

Set to true to enable compiling with ccache

**Current value (from the default):** `false`

From //build/toolchain/ccache.gni:9

### use_dbus

**Current value (from the default):** `false`

From //build/config/features.gni:11

### use_direct_for_carnelian_examples

Include a config in the example packages to attempt to use view mode
direct.

**Current value (from the default):** `false`

From //src/lib/ui/carnelian/BUILD.gn:29

### use_gio

**Current value (from the default):** `false`

From //build/config/features.gni:12

### use_llvm_libc_string_functions

**NOTE: Experimental** Use the llvm-libc implementations of string
functions.

**Current value (from the default):** `false`

From //sdk/lib/c/libc.gni:21

### use_null_vulkan_on_host

TODO(liyl): Currently non-x64 platforms don't have Vulkan support,
so we always use the null Vulkan implementation instead.

Global arguments for whether we use a "null" Vulkan implementation on
host vulkan_executables and vulkan_tests, so that any attempt to create a
VkInstances or VkDevice will fail.

This argument will affect all vulkan_{executable/test} build targets.


**Current value (from the default):** `true`

From //src/lib/vulkan/build/config.gni:29

### use_prebuilt_buildidtool

Use the prebuilt buildidtool binary rather than one built locally.
**NOTE:** Setting this to `false` uses the `toolchain_deps` mechanism in
GN, which can slow down Ninja significantly.  Also, to circular deps the
$host_toolchain has no `toolchain_deps` and so doesn't ensure the
buildidtool is built before it's needed.  This may make builds unreliable,
but it should be possible to iterate on incremental builds and get the new
tool in place eventually.  This should only be used during active
development of buildidtool itself.

Note, this never applies to Go builds because of the circularity of using
buildidtool in the build of buildidtool.

**Current value (from the default):** `true`

From //build/toolchain/buildidtool.gni:17

### use_prebuilt_ffmpeg

Use a prebuilt ffmpeg binary rather than building it locally.  See
//src/media/lib/ffmpeg/README.md for details.  This is ignored when
building in variant builds for which there is no prebuilt.  In that case,
ffmpeg is always built from source so as to be built with the selected
variant's config.  When this is false (either explicitly or in a variant
build) then //third_party/ffmpeg must be in the source tree, which
requires:

```
jiri import -name third_party/ffmpeg -revision HEAD third_party/ffmpeg http://fuchsia.googlesource.com/integration
```
TODO(https://fxbug.dev/42068172): This isn't currently working. Use the method below.

Or, if already importing a different manifest from there, resulting in
errors from jiri update, it can work to just git clone (but jiri update
won't manage third_party/ffmpeg in this case):

```
mkdir build/secondary/third_party/ffmpeg
git clone https://fuchsia.googlesource.com/third_party/ffmpeg build/secondary/third_party/ffmpeg
mkdir third_party/yasm
git clone https://fuchsia.googlesource.com/third_party/yasm third_party/yasm
mkdir third_party/ffmpeg/src
git clone https://chromium.googlesource.com/chromium/third_party/ffmpeg third_party/ffmpeg/src
```

**Current value (from the default):** `true`

From //src/media/lib/ffmpeg/BUILD.gn:33

### use_spinel_for_carnelian_examples

Include a config in the example packages to attempt to use Spinel

**Current value (from the default):** `false`

From //src/lib/ui/carnelian/BUILD.gn:25

### use_swiftshader_vulkan_icd_on_host


Global arguments for whether we use the SwiftShader Vulkan ICD on host
vulkan_executables and vulkan_tests.

This argument will affect all vulkan_{executable/test} build targets and
it only works when use_null_vulkan_on_host is set to false.


**Current value (from the default):** `true`

From //src/lib/vulkan/build/config.gni:38

### use_test_pilot

When enabled, changes tests.json entries for fuchsia_test_package tests so
that botanist will use test-pilot to execute them. The tests.json changes
include adding a "path" entry for the test and setting "os" to "linux".

**Current value (from the default):** `false`

From //build/components/fuchsia_test_package.gni:16

### use_udev

**Current value (from the default):** `false`

From //build/config/features.gni:10

### use_vbmeta

If true, then a vbmeta image will be generated for provided ZBI
and the paving script will pave vbmeta images to the target device.

**Current value for `target_cpu = "arm64"`:** `true`

From //boards/arm64.gni:33

**Overridden from the default:** `false`

From //build/images/vbmeta.gni:14

**Current value (from the default):** `false`

From //build/images/vbmeta.gni:14

### use_vim3_dml

If true, use the newer vim3-dml driver instead of the vim3-devicetree
driver.

**Current value (from the default):** `false`

From //boards/vim3/args.gni:9

### using_fuchsia_sdk

Only set in buildroots where targets configure themselves for use with the
Fuchsia SDK

**Current value (from the default):** `false`

From //build/fuchsia/sdk.gni:8

### vbmeta_a_partition

**Current value (from the default):** `""`

From //build/images/args.gni:27

### vbmeta_b_partition

**Current value (from the default):** `""`

From //build/images/args.gni:28

### vbmeta_r_partition

**Current value (from the default):** `""`

From //build/images/args.gni:29

### verify_depfile

Controls whether the build runs the depfile verifier

**Current value (from the default):** `true`

From //build/rust/build.gni:25

### vfs_rust_uses_log

Set this to true to enable some additional logs in the vfs crate and have it
depend on the log crate. This should not be enabled in general for non-host
builds because it causes the vfs crate, which is built as a dylib, to be the
source of the global logger, which can cause problems for things that
dynamically link rust libraries (like drivers) and cause link errors at
worst, or incorrect log attribution at best.

**Current value (from the default):** `false`

From //src/storage/lib/vfs/rust/BUILD.gn:17

### vim3_mcu_fan_default_level

The default fan level used by the VIM3 MCU driver.

Valid values are between 0 (completely off) and 3 (full power).

Do not depend on this setting in checked-in code. This setting is intended
to facilitate at-desk development, and will be replaced by a more robust
configuration mechanism.

**Current value (from the default):** `1`

From //src/devices/mcu/drivers/vim3-mcu/BUILD.gn:18

### virtio_gpu_display_flavor

Toggles between the C++ and Rust driver implementations.

TODO(https://fxbug.dev/504722357): Remove the toggle after the Rust port is completed.
Valid values: "cpp", "rust"

**Current value (from the default):** `"cpp"`

From //src/graphics/display/drivers/virtio-gpu-display/BUILD.gn:14

### virtmagma_debug

Enable verbose logging in virtmagma-related code

**Current value (from the default):** `false`

From //src/graphics/lib/magma/include/virtio/virtmagma_debug.gni:7

### virtual_alloc_host_size_shift

Set the page size shift of the host. This is used when running the allocator
in a host environment where page size constants may not exist. If this does
not much the actual host page size then a run time error will occur.

**Current value (from the default):** `12`

From //zircon/kernel/lib/virtual_alloc/BUILD.gn:13

### vm_tracing_level

The level of detail for traces emitted by the VM system. Values greater than
zero add increasing details at the cost of increased trace buffer use.

0 = Default kernel:* tracing.
1 = Adds flow events for asynchronous page requests.
2 = Adds duration events related to accessed faults and page faults.
3 = Adds duration events for PMM allocations and frees.

**Current value (from the default):** `0`

From //zircon/kernel/params.gni:74

### vulkan_host_runtime_dir


|vulkan_host_runtime_dir| is the path to Vulkan runtime libraries, which
contains prebuilt Vulkan loader, Vulkan layers, SwiftShader Vulkan ICD,
and descriptor files required to load the libraries.


**Current value (from the default):** `"//prebuilt/third_party/vulkan_runtime/linux-x64"`

From //src/lib/vulkan/build/config.gni:17

### vulkan_sdk

**Current value (from the default):** `""`

From //src/graphics/examples/vkproto/common/common.gni:48

### wait_queue_depth_tracing_enabled

Enables tracing of wait queue depths.  Used for post-processing analysis of
how deep wait queues tend to be under various loads, as well as how
frequently the change depth.

**Current value (from the default):** `false`

From //zircon/kernel/params.gni:65

### warn_on_sdk_changes

Whether to only warn when an SDK has been modified.
If false, any unacknowledged SDK change will cause a build failure.
LINT.IfChange

**Current value (from the default):** `false`

From //build/sdk/config.gni:17

### weave_build_legacy_wdm

Tells openweave to support legacy WDM mode.

**Current value (from the default):** `false`

From //third_party/openweave-core/config.gni:29

### weave_build_warm

Tells openweave to build WARM libraries.

**Current value (from the default):** `true`

From //third_party/openweave-core/config.gni:26

### weave_system_config_use_sockets

Tells openweave components to use bsd-like sockets.

**Current value (from the default):** `true`

From //third_party/openweave-core/config.gni:7

### weave_with_nlfaultinjection

Tells openweave components to support fault injection.

**Current value (from the default):** `false`

From //third_party/openweave-core/config.gni:20

### weave_with_verhoeff

Tells openweave to support Verhoeff checksum.

**Current value (from the default):** `true`

From //third_party/openweave-core/config.gni:23

### wlancfg_config_type

Selects the wlan configuration type to use. Choices:
  "client" - client mode
  "ap" - access point mode
  "" (empty string) - no configuration

**Current value (from the default):** `"client"`

From //src/connectivity/wlan/wlancfg/BUILD.gn:17

### zedboot_product_assembly_config_label

The product assembly config used to configure the Zedboot image.

**Current value (from the default):** `false`

From //build/product.gni:13

### zircon_a_partition

Arguments to `fx flash` script (along with any `firmware_prebuilts` which
specify a partition).

If (exactly one of) `fvm_partition` or `fxfs_partition` is provided, the
flash script will flash the full OS, recovery + Zircon + FVM (or Fxfs) + SSH
keys. In this case, the bootloader must also support `fastboot oem
add-staged-bootloader-file ssh.authorized_keys`.

Otherwise, the script will flash the recovery image to all slots, which
doesn't require the FVM or SSH keys.

**Current value (from the default):** `""`

From //build/images/args.gni:24

### zircon_asserts

Used internally to enable ZX_ASSERT_XXX macros provided by
<zircon/assert.h>. This should not be set in args.gn.

To override the default value, GN targets should use the config label
//build/config/fuchsia:enable_zircon_asserts, while Bazel target should use
the feature name "enable_zircon_asserts".


**Current value (from the default):** `false`

From //build/config/fuchsia/zircon_asserts.gni:13

### zircon_b_partition

**Current value (from the default):** `""`

From //build/images/args.gni:25

### zircon_r_partition

**Current value (from the default):** `""`

From //build/images/args.gni:26

### zircon_toolchain

*This should never be set as a build argument.*
It exists only to be set in `toolchain_args`.
For Zircon toolchains, this will be a scope whose schema
is documented in //build/toolchain/zircon/zircon_toolchain.gni.
For all other toolchains, this will be false.

This allows testing for a Zircon-specific toolchain with:

  if (zircon_toolchain != false) {
    // code path for Zircon-specific toolchains
  } else {
    // code path for non-Zircon ones.
  }

**Current value (from the default):** `false`

From //build/config/BUILDCONFIG.gn:981

### zircon_tracelog

Where to emit a tracelog from Zircon's GN run. No trace will be produced if
given the empty string. Path can be source-absolute or system-absolute.

**Current value (from the default):** `""`

From //build/zircon/build_args.gni:8

### zx_assert_level

Controls which asserts are enabled.

`ZX_ASSERT` is always enabled.

* 0 disables standard C `assert()` and `ZX_DEBUG_ASSERT`.
* 1 disables `ZX_DEBUG_ASSERT`. Standard C `assert()` remains enabled.
* 2 enables all asserts.

**Current value (from the default):** `2`

From //build/config/zircon/levels.gni:13

## `target_cpu = "arm64"`

### arm_use_neon

Whether to use the neon FPU instruction set or not.
TODO(https://fxbug.dev/42168336): move this to boards.

**Current value (from the default):** `true`

From //build/config/arm.gni:9

## `target_cpu = "x64"`

### build_libvulkan_goldfish

**Current value (from the default):** `"//third_party/android/device/generic/goldfish-opengl:libvulkan_goldfish"`

From //src/graphics/lib/goldfish-vulkan/gnbuild/BUILD.gn:12

