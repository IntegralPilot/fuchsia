# Test data for the unwinder

This is used for testing certain unwinding implementations that need a real binary to parse. At this
time, these tests are heavily reliant on the binary layout of the file being stable, so the
binary itself is checked into the tree. Follow the below instructions to update the binary when the
source file is changed.

## Building a new binary

  1. Add `//src/lib/unwinder/test_data:unwind_info_test_data(//build/toolchain:linux_arm-shared)` to
     the build graph. The easiest way to do this is to have the test binary in
     `//src/lib/unwinder:unwinder_tests_bin` dep on that target. The toolchain argument is important
     because the binary is specifically meant to be generated with the aarch32 toolchain and
     generate Arm32 bit unwind tables.
  2. The resulting shared object file will be at
     `//out/<dir>/linux_arm-shared/libunwind_info_test_data.so`. Copy that file to
     `//src/lib/unwinder/test_data/libunwind_info_test_data.targetso`, then remove the dep from step
     1.

This could be updated in the future to pull prebuilt binaries from CIPD if they get too big to keep
checked into the tree. See the zxdb documentation at
`//src/developer/debug/zxdb/symbols/test_data/README.md` for an example of how to do that.
