# Test build args file

This is a test file representing the
generated file in //docs which contains
the comments from all args declared in .gni files.

Most of the errors are about code that is not in code
blocks that looks like references

something_like = [ "file1", "file2" ]

or a multiline list = [ "file1",
"file2.txt" ]

**Current value (from the default):** `["detect_stack_use_after_return=1", "quarantine_size_mb=32"]`


This can be set in args.gn by vendor-specific build configurations.
Consider the following example from a fictitious
//vendor/acme/proprietary/BUILD.gn file:

     # Generate the firmware for our device.
     action("generate_firmware") {
       ...
     }

     # Ensure the generated firmware is visible to Bazel as a filegroup()
     # @legacy_ninja_build_outputs repository//:acme_firmware
     bazel_input_resource("acme_firmware") {
       deps = [ ":generate_firmware" ]
       sources = get_target_outputs(deps[0])
       outputs = [ "{{source_file_part}}" ]
       visibility = [ ":*" ]
     }

     # Build the installer with Bazel.
     bazel_action("build_installer") {
       command = "build"
       bazel_targets = "//vendor/acme/proprietary/installer"
       bazel_inputs = [ ":acme_firmware" ]
       copy_outputs = [
         {
           bazel = "vendor/acme/proprietary/installer/installer"
           ninja = "installer"
         }
       ]
     }
