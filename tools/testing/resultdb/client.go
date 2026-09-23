// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package resultdb

import (
	"encoding/json"
	"fmt"
	"net/http"
	"os"
	"strings"

	sinkpb "go.chromium.org/luci/resultdb/sink/proto/v1"
	"google.golang.org/protobuf/encoding/protojson"
	"google.golang.org/protobuf/proto"
)

// luciContext corresponds to the schema of the file identified by the
// LUCI_CONTEXT env var. See
// https://crsrc.org/i/go/src/go.chromium.org/luci/lucictx/sections.proto for
// the whole structure.
type luciContext struct {
	ResultDB   resultDB   `json:"resultdb"`
	ResultSink resultSink `json:"result_sink"`
}

// resultSink holds the result_sink information parsed from LUCI_CONTEXT.
type resultSink struct {
	AuthToken      string `json:"auth_token"`
	ResultSinkAddr string `json:"address"`
}

type Client struct {
	resultSink *resultSink
	httpClient *http.Client
	semaphore  chan struct{}
}

type resultDB struct {
	CurrentInvocation resultDBInvocation `json:"current_invocation"`
}

type resultDBInvocation struct {
	Name string `json:"name"`
}

func report[T proto.Message](c *Client, requests []T, endpoint string) error {
	for _, req := range requests {
		data := protojson.Format(req)
		err := c.sendData(data, endpoint)
		if err != nil {
			return err
		}
	}
	return nil
}

func (c *Client) ReportTestResults(requests []*sinkpb.ReportTestResultsRequest) error {
	return report(c, requests, "ReportTestResults")
}

func (c *Client) ReportTestExonerations(requests []*sinkpb.ReportTestExonerationsRequest) error {
	return report(c, requests, "ReportTestExonerations")
}

func (c *Client) ReportInvocationLevelArtifacts(outputRoot string, invocationArtifacts []string) error {
	invocationRequest := &sinkpb.ReportInvocationLevelArtifactsRequest{
		Artifacts: InvocationLevelArtifacts(outputRoot, invocationArtifacts),
	}

	testResult := protojson.Format(invocationRequest)
	return c.sendData(testResult, "ReportInvocationLevelArtifacts")
}

func (c *Client) sendData(data, endpoint string) error {
	<-c.semaphore
	defer func() { c.semaphore <- struct{}{} }()

	url := fmt.Sprintf("http://%s/prpc/luci.resultsink.v1.Sink/%s", c.resultSink.ResultSinkAddr, endpoint)
	req, err := http.NewRequest("POST", url, strings.NewReader(data))
	if err != nil {
		return err
	}
	// ResultSink HTTP authorization scheme is documented at
	// https://fuchsia.googlesource.com/third_party/luci-go/+/HEAD/resultdb/sink/proto/v1/sink.proto#29
	req.Header.Add("Authorization", fmt.Sprintf("ResultSink %s", c.resultSink.AuthToken))
	req.Header.Add("Accept", "application/json")
	req.Header.Add("Content-Type", "application/json")
	resp, err := c.httpClient.Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		return fmt.Errorf("ResultDB Http Request errored with status code %s (%d)", http.StatusText(resp.StatusCode), resp.StatusCode)
	}
	return nil
}

func NewClient() (*Client, error) {
	b, err := os.ReadFile(os.Getenv("LUCI_CONTEXT"))
	if err != nil {
		return nil, err
	}
	var ctx luciContext
	if err = json.Unmarshal(b, &ctx); err != nil {
		return nil, err
	}
	// We are clearly running inside a LUCI_CONTEXT luciexe environment but rdb
	// stream was not started.
	if ctx.ResultSink.AuthToken == "" || ctx.ResultSink.ResultSinkAddr == "" {
		return nil, fmt.Errorf("resultdb is enabled but not resultsink for invocation. Make sure swarming is run under \"rdb stream\"")
	}

	client := &Client{
		resultSink: &ctx.ResultSink,
		httpClient: &http.Client{},
		semaphore:  make(chan struct{}, 64),
	}

	for i := 0; i < cap(client.semaphore); i++ {
		client.semaphore <- struct{}{}
	}
	return client, nil
}
