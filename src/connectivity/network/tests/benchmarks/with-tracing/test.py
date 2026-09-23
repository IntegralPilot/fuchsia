# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import os
import pathlib

import fuchsia_base_test
from mobly import asserts, test_runner


# Runs a test where the purpose is not to produce performance metrics as
# output, but just to produce a trace for manual inspection.
#
# When the test is run on Infra, the trace is made available for download via
# the "task outputs" link. Currently this host-side wrapper code is needed for
# making the trace available that way, but in the future that could be done
# by using "ffx test" to run the test on Infra (see https://fxbug.dev/42076004).
class NetstackBenchmarksWithTracingTest(fuchsia_base_test.FuchsiaBaseTest):
    def test_loopback_socket_benchmarks_with_netstack3_tracing(self) -> None:
        self._run_test(
            "loopback-socket-benchmarks-with-tracing-pkg-netstack3",
            "loopback-socket-benchmarks-with-netstack3-tracing",
        )

    def test_tun_socket_benchmarks_with_netstack3_tracing(self) -> None:
        self._run_test(
            "tun-socket-benchmarks-tests-netstack3",
            "tun-socket-benchmarks-netstack3-with-tracing",
        )

    def _run_test(self, package_name: str, component_name: str) -> None:
        self.dut.ffx.run_test_component(
            f"fuchsia-pkg://fuchsia.com/{package_name}#meta/{component_name}.cm",
            ffx_test_args=[
                "--realm",
                "/core/testing/system-tests",
                "--output-directory",
                self.test_case_path,
            ],
            capture_output=False,
        )
        test_result_files = list(
            pathlib.Path(self.test_case_path).rglob("trace.fxt")
        )
        asserts.assert_equal(len(test_result_files), 1)
        dest_file = os.path.join(
            self.log_path,
            f"{package_name}-{component_name}-trace.fxt",
        )
        os.rename(test_result_files[0], dest_file)


if __name__ == "__main__":
    test_runner.main()
