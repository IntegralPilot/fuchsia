# Boot Tests

_Boot tests_ are a useful framework for exercising code in low-level
environments (e.g., early kernel or userspace boot) where mechanisms for
command/control or data exfiltration are hard to come by. In this framing, a
test is specified by a set of images to boot (e.g., via a product bundle),
execution amounts to booting the images, and the determination of success is
indicated by a magic 'success string' being printed to serial at some point
during or after the boot process, after the relevant logic has successfully
executed.

Templates that define boot tests wrap [`boot_test()`][boot_test.gni].

For reasons particular to automated testing, all boot tests are expected be
aggregated under the [//bundles/boot_tests/BUILD.gn][boot_test_bundle]. Once in
your dependency graph, boot tests can be executed locally with
`fx run-boot-test`.

[boot_test.gni]: /build/testing/boot_tests/boot_test.gni
[boot_test_bundle]: /bundles/boot_tests/BUILD.gn

