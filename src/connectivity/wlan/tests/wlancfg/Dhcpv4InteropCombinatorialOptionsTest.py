#!/usr/bin/env python3
#
# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import random
from dataclasses import dataclass

import dhcp_testing
import fuchsia_wlan_base_test
from mobly import asserts, test_runner

OPT_NUM_DOMAIN_SEARCH = 119
OPT_NUM_DOMAIN_NAME = 15


@dataclass
class Test:
    name: str
    dhcp_options: dict[str, int | str]
    dhcp_parameters: dict[str, str]


class Dhcpv4InteropCombinatorialOptionsTest(
    fuchsia_wlan_base_test.FuchsiaWlanBaseTest
):
    """DhcpV4 tests which validate combinations of DHCP options."""

    async def setup_class(self) -> None:
        await super().setup_class()
        self.dhcp = dhcp_testing.DhcpHelper(
            dut=self.dut,
            openwrt_ap=self.openwrt_ap,
            access_point=self.access_point,
            log_path=self.log_path,
        )

    async def setup_test(self) -> None:
        await super().setup_test()
        await self.dut.wlan_policy.ensure_clean_state()

    async def teardown_test(self) -> None:
        await self.dut.wlan_policy.ensure_clean_state()
        if self.access_point:
            self.access_point.stop_all_aps()
        await super().teardown_test()

    async def pre_run(self) -> None:
        async def test_logic(t: Test) -> None:
            await self.dhcp.run_test_case_expect_dhcp_success(
                t.dhcp_parameters, t.dhcp_options
            )

        def name_func(t: Test) -> str:
            return f"test_{t.name}"

        self.generate_tests(
            test_logic=test_logic,
            name_func=name_func,
            arg_sets=[
                (t,)
                for t in [
                    Test(
                        name="domain_name_valid",
                        dhcp_options={
                            "domain-name": '"example.test"',
                            "dhcp-parameter-request-list": OPT_NUM_DOMAIN_NAME,
                        },
                        dhcp_parameters={},
                    ),
                    Test(
                        name="domain_name_invalid",
                        dhcp_options={
                            "domain-name": '"example.invalid"',
                            "dhcp-parameter-request-list": OPT_NUM_DOMAIN_NAME,
                        },
                        dhcp_parameters={},
                    ),
                    Test(
                        name="domain_search_valid",
                        dhcp_options={
                            "domain-name": '"example.test"',
                            "dhcp-parameter-request-list": OPT_NUM_DOMAIN_SEARCH,
                        },
                        dhcp_parameters={},
                    ),
                    Test(
                        name="domain_search_invalid",
                        dhcp_options={
                            "domain-name": '"example.invalid"',
                            "dhcp-parameter-request-list": OPT_NUM_DOMAIN_SEARCH,
                        },
                        dhcp_parameters={},
                    ),
                    Test(
                        name="max_sized_message",
                        dhcp_options=self._generate_max_sized_message_dhcp_options(),
                        dhcp_parameters={},
                    ),
                ]
            ],
        )

    def _generate_max_sized_message_dhcp_options(self) -> dict[str, int | str]:
        """Generates the DHCP options for max sized message test.

        The RFC limits DHCP payloads to 576 bytes unless the client signals it
        can handle larger payloads, which it does by sending DHCP option 57,
        "Maximum DHCP Message Size". Despite being able to accept larger
        payloads, clients typically don't advertise this. The test verifies that
        the client accepts a large message split across multiple ethernet
        frames. The test is created by sending many bytes of options through the
        domain-name-servers option, which is of unbounded length (though is
        compressed per RFC1035 section 4.1.4).

        Returns:
            A dict of DHCP options.
        """
        typical_ethernet_mtu = 1500

        long_dns_setting = ", ".join(
            f'"ns{num}.example"'
            for num in random.sample(range(100_000, 1_000_000), 250)
        )
        # RFC1035 compression means any shared suffix ('.example' in this case)
        # will be deduplicated. Calculate approximate length by removing that
        # suffix.
        long_dns_setting_len = len(
            long_dns_setting.replace(", ", "")
            .replace('"', "")
            .replace(".example", "")
            .encode("utf-8")
        )
        asserts.assert_true(
            long_dns_setting_len > typical_ethernet_mtu,
            "Expected to generate message greater than ethernet mtu",
        )

        return {
            "dhcp-max-message-size": long_dns_setting_len * 2,
            "domain-search": long_dns_setting,
            "dhcp-parameter-request-list": OPT_NUM_DOMAIN_SEARCH,
        }


if __name__ == "__main__":
    test_runner.main()
