// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package expectation

import (
	"fmt"
	"os"
	"strings"

	"go.fuchsia.dev/fuchsia/src/connectivity/network/testing/conformance/expectation/outcome"
	"go.fuchsia.dev/fuchsia/src/connectivity/network/testing/conformance/expectation/platform"
	"go.fuchsia.dev/fuchsia/src/connectivity/network/testing/conformance/parseoutput"
)

type SuiteIdentifier struct {
	Platform  string
	SuiteName string
}

// TODO(https://fxbug.dev/42173824): Test expectations are intended to potentially be moved into a
// config file (perhaps JSON) rather than being embedded in Go in this way.
var expectations map[SuiteIdentifier]map[AnvlCaseNumber]outcome.Outcome = func() map[SuiteIdentifier]map[AnvlCaseNumber]outcome.Outcome {
	m := make(map[SuiteIdentifier]map[AnvlCaseNumber]outcome.Outcome)

	addAllExpectations := func(suite string, plt platform.Platform, expects map[AnvlCaseNumber]outcome.Outcome) {
		m[SuiteIdentifier{
			Platform:  plt.String(),
			SuiteName: strings.ToUpper(suite),
		}] = expects
	}

	// keep-sorted start
	addAllExpectations("arp", platform.NS2, arpExpectations)
	addAllExpectations("dhcp-client", platform.NS2, dhcpClientExpectations)
	addAllExpectations("dhcp-server", platform.NS2, dhcpServerExpectations)
	addAllExpectations("dhcpv6-client", platform.NS2, dhcpv6ClientExpectations)
	addAllExpectations("icmp", platform.NS2, icmpExpectations)
	addAllExpectations("icmp-router", platform.NS2, icmpRouterExpectations)
	addAllExpectations("icmpv6", platform.NS2, icmpv6Expectations)
	addAllExpectations("icmpv6-router", platform.NS2, icmpv6RouterExpectations)
	addAllExpectations("igmp", platform.NS2, igmpExpectations)
	addAllExpectations("ip", platform.NS2, ipExpectations)
	addAllExpectations("ip-router", platform.NS2, ipRouterExpectations)
	addAllExpectations("ipv6", platform.NS2, ipv6Expectations)
	addAllExpectations("ipv6-autoconfig", platform.NS2, ipv6AutoconfigExpectations)
	addAllExpectations("ipv6-mld", platform.NS2, ipv6MldExpectations)
	addAllExpectations("ipv6-ndp", platform.NS2, ipv6ndpExpectations)
	addAllExpectations("ipv6-pmtu", platform.NS2, ipv6PmtuExpectations)
	addAllExpectations("ipv6-router", platform.NS2, ipv6RouterExpectations)
	addAllExpectations("tcp-advanced", platform.NS2, tcpAdvancedExpectations)
	addAllExpectations("tcp-advanced-v6", platform.NS2, tcpAdvancedV6Expectations)
	addAllExpectations("tcp-core", platform.NS2, tcpCoreExpectations)
	addAllExpectations("tcp-core-v6", platform.NS2, tcpcorev6Expectations)
	addAllExpectations("tcp-highperf", platform.NS2, tcpHighperfExpectations)
	addAllExpectations("tcp-highperf-v6", platform.NS2, tcpHighperfV6Expectations)
	addAllExpectations("udp", platform.NS2, udpExpectations)
	addAllExpectations("udp-v6", platform.NS2, udpV6Expectations)
	// keep-sorted end

	// keep-sorted start
	addAllExpectations("arp", platform.NS3, arpExpectationsNS3)
	addAllExpectations("dhcp-client", platform.NS3, dhcpClientExpectationsNS3)
	addAllExpectations("dhcp-server", platform.NS3, dhcpServerExpectationsNS3)
	addAllExpectations("dhcpv6-client", platform.NS3, dhcpv6ClientExpectationsNS3)
	addAllExpectations("dhcpv6-client-pd", platform.NS3, dhcpv6ClientPDExpectationsNS3)
	addAllExpectations("icmp", platform.NS3, icmpExpectationsNS3)
	addAllExpectations("icmp-router", platform.NS3, icmpRouterExpectationsNS3)
	addAllExpectations("icmpv6", platform.NS3, icmpv6ExpectationsNS3)
	addAllExpectations("icmpv6-router", platform.NS3, icmpv6RouterExpectationsNS3)
	addAllExpectations("igmp", platform.NS3, igmpExpectationsNS3)
	addAllExpectations("igmpv3", platform.NS3, igmpv3ExpectationsNS3)
	addAllExpectations("ip", platform.NS3, ipExpectationsNS3)
	addAllExpectations("ip-router", platform.NS3, ipRouterExpectationsNS3)
	addAllExpectations("ipv6", platform.NS3, ipv6ExpectationsNS3)
	addAllExpectations("ipv6-autoconfig", platform.NS3, ipv6AutoconfigExpectationsNS3)
	addAllExpectations("ipv6-mld", platform.NS3, ipv6MldExpectationsNS3)
	addAllExpectations("ipv6-mldv2", platform.NS3, ipv6Mldv2ExpectationsNS3)
	addAllExpectations("ipv6-ndp", platform.NS3, ipv6ndpExpectationsNS3)
	addAllExpectations("ipv6-pmtu", platform.NS3, ipv6PmtuExpectationsNS3)
	addAllExpectations("ipv6-router", platform.NS3, ipv6RouterExpectationsNS3)
	addAllExpectations("tcp-advanced", platform.NS3, tcpAdvancedExpectationsNS3)
	addAllExpectations("tcp-advanced-v6", platform.NS3, tcpAdvancedV6ExpectationsNS3)
	addAllExpectations("tcp-core", platform.NS3, tcpCoreExpectationsNS3)
	addAllExpectations("tcp-core-v6", platform.NS3, tcpcorev6ExpectationsNS3)
	addAllExpectations("tcp-highperf", platform.NS3, tcpHighperfExpectationsNS3)
	addAllExpectations("tcp-highperf-v6", platform.NS3, tcpHighperfV6ExpectationsNS3)
	addAllExpectations("udp", platform.NS3, udpExpectationsNS3)
	addAllExpectations("udp-v6", platform.NS3, udpV6ExpectationsNS3)
	// keep-sorted end

	return m
}()

type AnvlCaseNumber struct {
	MajorNumber int
	MinorNumber int
}

func (n AnvlCaseNumber) String() string {
	return fmt.Sprintf("%d.%d", n.MajorNumber, n.MinorNumber)
}

// Returns whether a is smaller than b
func (a AnvlCaseNumber) Cmp(b AnvlCaseNumber) bool {
	return a.MajorNumber < b.MajorNumber ||
		(a.MajorNumber == b.MajorNumber && a.MinorNumber < b.MinorNumber)
}

var Pass = outcome.Pass
var Fail = outcome.Fail
var Inconclusive = outcome.Inconclusive
var NoResponse = outcome.NoResponse
var Flaky = outcome.Flaky
var Skip = outcome.Skip
var AnvlSkip = outcome.AnvlSkip

func GetExpectation(
	ident parseoutput.CaseIdentifier,
) (outcome.Outcome, bool) {
	suiteIdent := SuiteIdentifier{
		SuiteName: ident.SuiteName,
		Platform:  ident.Platform,
	}
	perSuiteExpectations, ok := expectations[suiteIdent]
	if ok {
		expectation, ok := perSuiteExpectations[AnvlCaseNumber{
			MajorNumber: ident.MajorNumber,
			MinorNumber: ident.MinorNumber,
		}]
		if ok {
			return expectation, ok
		}
	}
	if os.Getenv("ANVL_DEFAULT_EXPECTATION_PASS") != "" {
		return outcome.Pass, true
	}
	return 0, false
}

func GetPerSuiteExpectations(
	ident SuiteIdentifier,
) (map[AnvlCaseNumber]outcome.Outcome, bool) {
	expectations, ok := expectations[ident]
	return expectations, ok
}
