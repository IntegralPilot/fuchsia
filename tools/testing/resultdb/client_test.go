// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package resultdb

import (
	"flag"
	"os"
	"path/filepath"
	"sort"
	"testing"

	"github.com/google/go-cmp/cmp"
	resultpb "go.chromium.org/luci/resultdb/proto/v1"
	sinkpb "go.chromium.org/luci/resultdb/sink/proto/v1"
)

var testDataDir = flag.String("test_data_dir", "testdata", "Path to testdata/; only used in GN build")

func TestGetLUCICtx(t *testing.T) {
	old := os.Getenv("LUCI_CONTEXT")
	defer os.Setenv("LUCI_CONTEXT", old)
	os.Setenv("LUCI_CONTEXT", filepath.Join(*testDataDir, "lucictx.json"))
	client, err := NewClient()
	if err != nil {
		t.Errorf("Cannot parse LUCI_CONTEXT: %v", err)
	}
	if client.resultSink.ResultSinkAddr != "result.sink" {
		t.Errorf("Incorrect value parsed for result_sink address. Got %s", client.resultSink.ResultSinkAddr)
	}
	if client.resultSink.AuthToken != "token" {
		t.Errorf("Incorrect value parsed for result_sink auth_token field. Got %s", client.resultSink.AuthToken)
	}
}

func TestParse2Summary(t *testing.T) {
	t.Parallel()
	const chunkSize = 5
	var requests []*sinkpb.ReportTestResultsRequest
	expectRequests := 0
	for _, name := range []string{"summary.json", "summary2.json"} {
		summary, err := ParseSummary(filepath.Join(*testDataDir, name))
		if err != nil {
			t.Fatal(err)
		}
		testResults, _, skipped := SummaryToResultSink(summary, []*resultpb.StringPair{}, name)
		expectRequests += (len(testResults)-1)/chunkSize + 1
		requests = append(requests, createTestResultsRequests(testResults, chunkSize)...)
		for _, testResult := range testResults {
			if len(testResult.TestId) == 0 {
				t.Errorf("Empty testId is not allowed.")
			}
		}
		if len(skipped) != 0 {
			t.Errorf("Tests got skipped %v, expect no skip", skipped)
		}
	}
	if len(requests) != expectRequests {
		t.Errorf("Incorrect number of request chuncks, got: %d want %d", len(requests), expectRequests)
	}
}

func TestFailWithLongTestName(t *testing.T) {
	summary, err := ParseSummary(filepath.Join(*testDataDir, "summary_long_name.json"))
	if err != nil {
		t.Fatal(err)
	}
	_, _, testsSkipped := SummaryToResultSink(summary, []*resultpb.StringPair{}, "")

	skippedTestName := "fuchsia-pkg://fuchsia.com/netstack-integration-tests#meta/netstack-inspect-integration-test.cm/:fuchsia_fuchsia_fuchsia_fuchsia_fuchsia_fuchsia_fuchsia_fuchsia_fuchsia_fuchsia_inspect_dhcp_netdevice::_multiple_invalid_port_and_single_invalid_trans_proto_vec_packetattributes_ip_proto_packet_formats_ip_ipv4proto_proto_packet_formats_ip_ipproto_udp_port_invalid_port_packetattributes_ip_proto_packet_formats_ip_ipv4proto_proto_packet_formats_ip_ipproto_udp_port_invalid_port_packetattributes_ip_proto_packet_formats_ip_ipv4proto_proto_packet_formats_ip_ipproto_tcp_port_dhcp_client_port_"

	if len(testsSkipped) != 1 {
		t.Errorf("Incorrect number of skipped tests got: %d want %d", len(testsSkipped), 1)
	}
	if testsSkipped[0] != skippedTestName {
		t.Errorf("Incorrect skipped test, got: %s want %s", testsSkipped[0], skippedTestName)
	}
}

func TestParseExoneratedSummary(t *testing.T) {
	// 1. Load and map the custom summary containing exonerations
	summary, err := ParseSummary(filepath.Join(*testDataDir, "summary_exonerated.json"))
	if err != nil {
		t.Fatal(err)
	}

	// Assert total count of target items in summary returned by ParseSummary
	if len(summary.Tests) != 5 {
		t.Fatalf("Unexpected parsed tests count inside summary: got %d, want 5", len(summary.Tests))
	}

	results, exonerations, skipped := SummaryToResultSink(summary, []*resultpb.StringPair{}, "summary_exonerated.json")

	// Expect exactly 6 results:
	// - 1 from prefix/exonerated_target_only (exonerated target is mapped as FAILED)
	// - 3 from prefix/failed_target_with_exonerated_case (1 exonerated case (FAILED) + 1 passed case (PASSED) + 1 failed target details (FAILED))
	// - 1 from prefix/passed_target (passed target itself)
	// - 1 from prefix/skipped_target (skipped target itself)
	// - 0 from prefix/unloaded_target_exceeding_limit_... (skipped upload target due to name length exceeds 512 limit)
	if len(results) != 6 {
		t.Fatalf("Unexpected number of mapped results: got %d, want 6", len(results))
	}
	// Expect exactly 2 exonerations:
	// - 1 target exoneration from prefix/exonerated_target_only
	// - 1 case exoneration from prefix/failed_target_with_exonerated_case/SuiteX:CaseExonerated
	if len(exonerations) != 2 {
		t.Fatalf("Unexpected number of mapped exonerations: got %d, want 2", len(exonerations))
	}

	// Expect exactly 1 skipped target run exceeding the 512-byte limit
	if len(skipped) != 1 {
		t.Fatalf("Got %d skipped tests, want 1", len(skipped))
	}
	expectedSkippedName := "prefix/unloaded_target_exceeding_limit_fuchsia_fuchsia_fuchsia_fuchsia_fuchsia_fuchsia_fuchsia_fuchsia_fuchsia_fuchsia_fuchsia_fuchsia_fuchsia_fuchsia_fuchsia_fuchsia_fuchsia_fuchsia_fuchsia_fuchsia_fuchsia_fuchsia_fuchsia_fuchsia_fuchsia_fuchsia_fuchsia_fuchsia_fuchsia_fuchsia_fuchsia_fuchsia_fuchsia_fuchsia_fuchsia_fuchsia_fuchsia_fuchsia_fuchsia_fuchsia_fuchsia_fuchsia_fuchsia_fuchsia_fuchsia_fuchsia_fuchsia_fuchsia_fuchsia_fuchsia_fuchsia_fuchsia_fuchsia_fuchsia_fuchsia_fuchsia_fuchsia_fuchsia_fuchsia_fuchsia_fuchsia_fuchsia"
	if skipped[0] != expectedSkippedName {
		t.Errorf("Unexpected skipped target returned: got %q, want %q", skipped[0], expectedSkippedName)
	}

	// 2. Verify mapped payload details
	// results[0]: prefix/exonerated_target_only (FAILED)
	if results[0].TestId != "prefix/exonerated_target_only" || results[0].StatusV2 != resultpb.TestResult_FAILED {
		t.Errorf("Unexpected mapped result 0: %+v", results[0])
	}

	// results[1]: prefix/failed_target_with_exonerated_case/SuiteX:CaseExonerated (FAILED)
	if results[1].TestId != "prefix/failed_target_with_exonerated_case/SuiteX:CaseExonerated" || results[1].StatusV2 != resultpb.TestResult_FAILED {
		t.Errorf("Unexpected mapped result 1: %+v", results[1])
	}
	if results[1].FailureReason == nil || len(results[1].FailureReason.Errors) == 0 || results[1].FailureReason.Errors[0].Message != "Flaky run instance" {
		t.Errorf("Unexpected FailureReason for result 1: %+v", results[1].FailureReason)
	}

	// results[2]: prefix/failed_target_with_exonerated_case/SuiteX:CasePassed (PASSED)
	if results[2].TestId != "prefix/failed_target_with_exonerated_case/SuiteX:CasePassed" || results[2].StatusV2 != resultpb.TestResult_PASSED {
		t.Errorf("Unexpected mapped result 2: %+v", results[2])
	}

	// results[3]: prefix/failed_target_with_exonerated_case (FAILED)
	if results[3].TestId != "prefix/failed_target_with_exonerated_case" || results[3].StatusV2 != resultpb.TestResult_FAILED {
		t.Errorf("Unexpected mapped result 3: %+v", results[3])
	}
	expectedErrorMessage := "CaseExonerated: Flaky run instance"
	if results[3].FailureReason == nil || len(results[3].FailureReason.Errors) == 0 || results[3].FailureReason.Errors[0].Message != expectedErrorMessage {
		t.Errorf("Target failure reason was not populated with the failed cases. got err: %q", results[3].FailureReason.GetErrors()[0].GetMessage())
	}

	// results[4]: prefix/passed_target (PASSED)
	if results[4].TestId != "prefix/passed_target" || results[4].StatusV2 != resultpb.TestResult_PASSED {
		t.Errorf("Unexpected mapped result 4: %+v", results[4])
	}

	// results[5]: prefix/skipped_target (SKIPPED)
	if results[5].TestId != "prefix/skipped_target" || results[5].StatusV2 != resultpb.TestResult_SKIPPED {
		t.Errorf("Unexpected mapped result 5: %+v", results[5])
	}
	if results[5].SkippedReason == nil || results[5].SkippedReason.ReasonMessage != "skipped because unaffected" {
		t.Errorf("Skipped target mapped incorrect skipped reason, got: %+v", results[5].SkippedReason)
	}

	// Expect the exonerations mapped to:
	// - Case exoneration: prefix/failed_target_with_exonerated_case/SuiteX:CaseExonerated
	// - Target exoneration: prefix/exonerated_target_only
	exoneratedIDs := []string{exonerations[0].TestId, exonerations[1].TestId}
	sort.Strings(exoneratedIDs)
	expectedIDs := []string{
		"prefix/exonerated_target_only",
		"prefix/failed_target_with_exonerated_case/SuiteX:CaseExonerated",
	}
	if diff := cmp.Diff(exoneratedIDs, expectedIDs); diff != "" {
		t.Errorf("Exoneration IDs differ (-got +want):\n%s", diff)
	}
}
