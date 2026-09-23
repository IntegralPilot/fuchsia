// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package main

import (
	"flag"
	"fmt"
	"os"
	"strings"

	resultpb "go.chromium.org/luci/resultdb/proto/v1"
	sinkpb "go.chromium.org/luci/resultdb/sink/proto/v1"

	"go.fuchsia.dev/fuchsia/tools/lib/flagmisc"
	"go.fuchsia.dev/fuchsia/tools/testing/resultdb"
)

var (
	summaries           flagmisc.StringsValue
	tags                flagmisc.StringsValue
	outputRoot          string
	invocationArtifacts flagmisc.StringsValue
)

func main() {
	if err := mainImpl(); err != nil {
		fmt.Fprintf(os.Stderr, "ResultDB upload errored: %v\n", err)
		os.Exit(1)
	}
}

func mainImpl() error {
	flag.Var(&summaries, "summary", "Repeated flag, file location to summary.json."+
		" To pass in multiple files do '--summary file1.json --summary file2.json'")
	flag.Var(&tags, "tag", "Repeated flag, add tag to all test case and test suites."+
		" Uses the format key:value. To pass in multiple tags do '--tag key1:val1 --tag key2:val2'")
	flag.StringVar(&outputRoot, "output", "",
		"Output root path to be joined with 'output_file' field in summary.json. If not set, current directory will be used.")
	flag.Var(&invocationArtifacts, "invocation-artifact", "Repeated flag, path of file to upload as an invocation-level artifact.")

	flag.Parse()

	var requests []*sinkpb.ReportTestResultsRequest
	var exonerationRequests []*sinkpb.ReportTestExonerationsRequest
	var allTestsSkipped []string

	tagPairs, err := convertTags(tags)
	if err != nil {
		return err
	}

	requests, exonerationRequests, allTestsSkipped, err = resultdb.ProcessSummaries(summaries, tagPairs, outputRoot)
	if err != nil {
		return err
	}

	client, err := resultdb.NewClient()
	if err != nil {
		return err
	}

	if err = client.ReportTestResults(requests); err != nil {
		return err
	}

	if err = client.ReportTestExonerations(exonerationRequests); err != nil {
		return err
	}

	if err = client.ReportInvocationLevelArtifacts(outputRoot, invocationArtifacts); err != nil {
		return err
	}

	if len(allTestsSkipped) > 0 {
		return fmt.Errorf("Some tests could not be uploaded due to testname exceeding byte limit %d.", resultdb.MaxTestIDLength)
	}
	return nil
}

func convertTags(tags []string) ([]*resultpb.StringPair, error) {
	t := []*resultpb.StringPair{}
	for _, tag := range tags {
		pair, err := stringPairFromString(tag)
		if err != nil {
			return nil, err
		}
		t = append(t, pair)
	}
	return t, nil
}

// stringPairFromString creates a pb.StringPair from the given key:val string.
func stringPairFromString(s string) (*resultpb.StringPair, error) {
	p := strings.SplitN(s, ":", 2)
	if len(p) != 2 {
		return nil, fmt.Errorf("cannot match tag content %s in the format key:value", s)
	}
	return &resultpb.StringPair{Key: strings.TrimSpace(p[0]), Value: strings.TrimSpace(p[1])}, nil
}
