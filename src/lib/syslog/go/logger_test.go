// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// NOTE: We need to use Archivist so we can verify logs have been written, but if we run tests in
// parallel, there will only be one archivist component and the tests can interfere with each
// other. We could fix this by building a realm within each test, but it isn't worth the effort.

package syslog_test

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"os"
	"strings"
	"testing"
	"time"
	"unicode/utf8"

	"fidl/fuchsia/diagnostics"
	"fidl/fuchsia/diagnostics/types"
	"fidl/fuchsia/logger"

	"go.fuchsia.dev/fuchsia/src/lib/component"
	syslog "go.fuchsia.dev/fuchsia/src/lib/syslog/go"
)

const format = "integer: %d"

var pid = uint64(os.Getpid())

func TestLogSimple(t *testing.T) {
	actual := bytes.Buffer{}
	var options syslog.LogInitOptions
	options.MinSeverityForFileAndLineInfo = syslog.ErrorLevel
	options.Writer = &actual
	log, err := syslog.NewLogger(options)
	if err != nil {
		t.Fatal(err)
	}
	if err := log.Infof(format, 10); err != nil {
		t.Fatal(err)
	}
	expected := "INFO: integer: 10\n"
	got := string(actual.Bytes())
	if !strings.HasSuffix(got, expected) {
		t.Errorf("%q should have ended in %q", got, expected)
	}
	if !strings.Contains(got, fmt.Sprintf("[%d]", pid)) {
		t.Errorf("%q should contains %d", got, pid)
	}
}

func setup(t *testing.T, tags ...string) (*syslog.Logger, string) {
	ctx := component.NewContextFromStartupInfo()
	req, logSink, err := logger.NewLogSinkWithCtxInterfaceRequest()
	if err != nil {
		t.Fatal(err)
	}
	ctx.ConnectToEnvService(req)

	uniqueTag := fmt.Sprintf("%d", os.Getpid())
	allTags := append([]string{uniqueTag}, tags...)

	options := syslog.LogInitOptions{
		LogLevel: syslog.InfoLevel,
	}
	options.LogSink = logSink
	options.MinSeverityForFileAndLineInfo = syslog.ErrorLevel
	options.Tags = allTags
	log, err := syslog.NewLogger(options)
	if err != nil {
		t.Fatal(err)
	}

	return log, uniqueTag
}

func connectToArchive(t *testing.T) *diagnostics.ArchiveAccessorWithCtxInterface {
	req, archive, err := diagnostics.NewArchiveAccessorWithCtxInterfaceRequest()
	if err != nil {
		t.Fatal(err)
	}
	ctx := component.NewContextFromStartupInfo()
	ctx.ConnectToEnvService(req)
	return archive
}

func checkoutput(t *testing.T, uniqueTag, expectedMsg string, severity syslog.LogLevel, tags ...string) {
	archive := connectToArchive(t)
	iteratorReq, iterator, err := diagnostics.NewBatchIteratorWithCtxInterfaceRequest()
	if err != nil {
		t.Fatal(err)
	}
	var clientSelectorConfig diagnostics.ClientSelectorConfiguration
	clientSelectorConfig.SetSelectAll(true)
	var streamParams diagnostics.StreamParameters
	streamParams.SetDataType(diagnostics.DataTypeLogs)
	streamParams.SetStreamMode(diagnostics.StreamModeSnapshot)
	streamParams.SetFormat(diagnostics.FormatJson)
	streamParams.SetClientSelectorConfiguration(clientSelectorConfig)
	if err := archive.StreamDiagnostics(context.Background(), streamParams, iteratorReq); err != nil {
		t.Fatal(err)
	}
	for {
		result, err := iterator.GetNext(context.Background())
		if err != nil {
			t.Fatal(err)
		}
		if result.Which() != diagnostics.BatchIteratorGetNextResultResponse {
			t.Fatalf("unexpected result: %d", result.Which())
		}
		batch := result.Response.Batch
		for _, msg := range batch {
			if msg.Which() != diagnostics.FormattedContentJson {
				t.Fatalf("unexpected content format: %d", msg.Which())
			}
			buf := make([]byte, msg.Json.Size)
			if err := msg.Json.Vmo.Read(buf, 0); err != nil {
				t.Fatal(err)
			}
			var data []struct {
				Metadata struct {
					Severity string   `json:"severity"`
					Tags     []string `json:"tags"`
				} `json:"metadata"`
				Payload struct {
					Root struct {
						Message struct {
							Value string `json:"value"`
						} `json:"message"`
					} `json:"root"`
				} `json:"payload"`
			}
			if err := json.Unmarshal(buf, &data); err != nil {
				t.Fatal(err)
			}
			for _, d := range data {
				// Ignore the WaitForInterestChange messages
				if strings.Contains(d.Payload.Root.Message.Value, "WaitForInterestChange") {
					continue
				}
				for _, tag := range d.Metadata.Tags {
					if tag == uniqueTag {
						// Found the message, now check it.
						expectedSeverity := severity.String()
						if severity == syslog.WarningLevel {
							expectedSeverity = "WARN"
						}
						if d.Metadata.Severity != expectedSeverity {
							t.Errorf("severity error, got: %q, want: %q", d.Metadata.Severity, expectedSeverity)
						}
						if d.Payload.Root.Message.Value != expectedMsg {
							t.Errorf("msg error, got: %q, want: %q", d.Payload.Root.Message.Value, expectedMsg)
						}
						actualTags := d.Metadata.Tags
						allTags := append([]string{uniqueTag}, tags...)
						if len(actualTags) != len(allTags) {
							t.Fatalf("tags error, got: %q, want: %q", actualTags, allTags)
						}
						for i := range allTags {
							if actualTags[i] != allTags[i] {
								t.Errorf("tags error, got: %q, want: %q", actualTags[i], allTags[i])
							}
						}
						return
					}
				}
			}
		}
	}
}

func TestLog(t *testing.T) {
	log, uniqueTag := setup(t)
	defer func() {
		if err := log.Close(); err != nil {
			t.Error(err)
		}
	}()

	if err := log.Infof(format, 10); err != nil {
		t.Fatal(err)
	}

	checkoutput(t, uniqueTag, fmt.Sprintf(format, 10), syslog.InfoLevel)
}

func TestLogWithLocalTag(t *testing.T) {
	log, uniqueTag := setup(t)
	defer func() {
		if err := log.Close(); err != nil {
			t.Error(err)
		}
	}()
	if err := log.InfoTf("local_tag", format, 10); err != nil {
		t.Fatal(err)
	}
	expectedMsg := fmt.Sprintf(format, 10)
	checkoutput(t, uniqueTag, expectedMsg, syslog.InfoLevel, "local_tag")
}

func TestLogWithGlobalTags(t *testing.T) {
	log, uniqueTag := setup(t, "gtag1", "gtag2")
	defer func() {
		if err := log.Close(); err != nil {
			t.Error(err)
		}
	}()
	if err := log.InfoTf("local_tag", format, 10); err != nil {
		t.Fatal(err)
	}
	expectedMsg := fmt.Sprintf(format, 10)
	checkoutput(t, uniqueTag, expectedMsg, syslog.InfoLevel, "gtag1", "gtag2", "local_tag")
}

func TestLoggerSeverity(t *testing.T) {
	log, uniqueTag := setup(t)
	defer func() {
		if err := log.Close(); err != nil {
			t.Error(err)
		}
	}()
	log.SetSeverity(types.Severity(syslog.WarningLevel))
	if err := log.Infof(format, 10); err != nil {
		t.Fatal(err)
	}
	if err := log.Warnf(format, 10); err != nil {
		t.Fatal(err)
	}
	expectedMsg := fmt.Sprintf(format, 10)
	checkoutput(t, uniqueTag, expectedMsg, syslog.WarningLevel)
}

func TestLoggerVerbosity(t *testing.T) {
	log, uniqueTag := setup(t)
	defer func() {
		if err := log.Close(); err != nil {
			t.Error(err)
		}
	}()
	if err := log.VLogf(syslog.DebugVerbosity, format, 9); err != nil {
		t.Fatal(err)
	}
	log.SetVerbosity(syslog.DebugVerbosity)
	if err := log.VLogf(syslog.DebugVerbosity, format, 10); err != nil {
		t.Fatal(err)
	}
	expectedMsg := fmt.Sprintf(format, 10)
	checkoutput(t, uniqueTag, expectedMsg, syslog.InfoLevel)
}

func TestLoggerRegisterInterest(t *testing.T) {
	log, _ := setup(t)
	defer func() {
		if err := log.Close(); err != nil {
			t.Error(err)
		}
	}()

	req, logSettings, err := diagnostics.NewLogSettingsWithCtxInterfaceRequest()
	if err != nil {
		t.Fatal(err)
	}
	ctx := component.NewContextFromStartupInfo()
	ctx.ConnectToEnvService(req)
	var componentSelector diagnostics.ComponentSelector
	componentSelector.SetMonikerSegments([]diagnostics.StringSelector{
		diagnostics.StringSelectorWithStringPattern("**"),
	})
	var interest types.Interest
	interest.SetMinSeverity(types.SeverityDebug)
	var logInterest diagnostics.LogSettingsSetComponentInterestRequest
	logInterest.SetSelectors([]diagnostics.LogInterestSelector{
		{
			Selector: componentSelector,
			Interest: interest,
		},
	})
	if err := logSettings.SetComponentInterest(context.Background(), logInterest); err != nil {
		t.Fatal(err)
	}

	for {
		if log.GetSeverity() == types.SeverityDebug {
			break
		}
		time.Sleep(10 * time.Millisecond)
	}

	logSettings.Close()

	for {
		if log.GetSeverity() == types.SeverityInfo {
			break
		}
		time.Sleep(10 * time.Millisecond)
	}
}

func TestGlobalTagLimits(t *testing.T) {
	var options syslog.LogInitOptions
	options.Writer = os.Stdout
	var tags [logger.MaxTags + 1]string
	for i := 0; i < len(tags); i++ {
		tags[i] = "a"
	}
	options.Tags = tags[:]
	if _, err := syslog.NewLogger(options); err == nil || !strings.Contains(err.Error(), "too many tags") {
		t.Fatalf("unexpected error: %s", err)
	}
	options.Tags = tags[:logger.MaxTags]
	var tag [logger.MaxTagLenBytes + 1]byte
	for i := 0; i < len(tag); i++ {
		tag[i] = 65
	}
	options.Tags[1] = string(tag[:])
	if _, err := syslog.NewLogger(options); err == nil || !strings.Contains(err.Error(), "tag too long") {
		t.Fatalf("unexpected error: %s", err)
	}
}

func TestLocalTagLimits(t *testing.T) {
	log, uniqueTag := setup(t)
	defer func() {
		if err := log.Close(); err != nil {
			t.Error(err)
		}
	}()
	var tag [logger.MaxTagLenBytes + 1]byte
	for i := 0; i < len(tag); i++ {
		tag[i] = 65
	}
	if err := log.InfoTf(string(tag[:]), format, 10); err != nil {
		t.Fatal(err)
	}
	expectedMsg := fmt.Sprintf(format, 10)
	checkoutput(t, uniqueTag, expectedMsg, syslog.InfoLevel, string(tag[:logger.MaxTagLenBytes]))
}

func TestMessageLenLimit(t *testing.T) {
	log, uniqueTag := setup(t)
	defer func() {
		if err := log.Close(); err != nil {
			t.Error(err)
		}
	}()
	// Without tags, it's 64 bytes to the message argument plus 16 bytes for the message argument header. The tag
	// should be an extra 24 bytes (8 bytes header, 8 bytes string arg, 8 bytes value)
	msgLen := 4095*8 - 104

	const stripped = '𠜎'
	// Ensure only part of stripped fits.
	msg := strings.Repeat("x", msgLen-(utf8.RuneLen(stripped)-1)) + string(stripped)
	switch err := log.Infof(msg).(type) {
	case *syslog.ErrMsgTooLong:
		if err.Msg != string(stripped) {
			t.Fatalf("unexpected truncation: %s", err.Msg)
		}
	default:
		t.Fatalf("unexpected error: %#v", err)
	}

	const ellipsis = "..."
	expectedMsg := msg[:msgLen-len(ellipsis)] + ellipsis
	checkoutput(t, uniqueTag, expectedMsg, syslog.InfoLevel)
}
