// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package tefmocheck

import (
	"strings"
	"testing"

	"github.com/google/go-cmp/cmp"
)

func TestLoadChecksFromYAML(t *testing.T) {
	yamlData := `
failure_mode_checks:
  - kind: "string_in_log"
    string: "Exceeded safe temperature range"
    log_types:
      - syslogType
  - kind: "string_in_log"
    string: "botanist ERROR: SerialReadErrorMsg"
    log_types:
      - swarmingOutputType
  - kind: "string_in_log"
    string: "cas: failed to call UploadIfMissing"
    infra_failure: true
    log_types:
      - swarmingOutputType
    only_on_states:
      - BOT_DIED
    skip_passed_test: true
    ignore_flakes: true
    attribute_to_test: true
    add_tag: true
  - kind: "string_in_log"
    string: "DEVICE SUSPEND TIMED OUT"
    log_types:
      - serialLogType
      - syslogType
  - kind: "string_in_log"
    string: "critical to root job killed with"
    log_types:
      - swarmingOutputType
      - serialLogType
      - syslogType
    except_blocks:
        # LINT.IfChange(mobly_test_start)
      - start_string: "======== Mobly config content ========"
        # LINT.ThenChange(//src/testing/end_to_end/mobly_driver/mobly_driver/api/api_infra.py:mobly_test_start)
        # LINT.IfChange(mobly_test_end)
        end_string: "[=====MOBLY RESULTS=====]"
        # LINT.ThenChange(//src/testing/end_to_end/mobly_driver/mobly_driver/api/api_infra.py:mobly_test_end)
      - start_string: "=== RUN   TestKillCriticalProcess"
        end_string: "--- PASS: TestKillCriticalProcess"
      - start_string: "=== RUN   TestOOMHard"
        end_string: "--- PASS: TestOOMHard"
`

	checks, err := LoadChecksFromYAML([]byte(yamlData))
	if err != nil {
		t.Fatalf("LoadChecksFromYAML failed: %v", err)
	}

	exceptBlocks := []*logBlock{
		{
			startString: "======== Mobly config content ========",
			endString:   "[=====MOBLY RESULTS=====]",
		},
		{
			startString: "=== RUN   TestKillCriticalProcess",
			endString:   "--- PASS: TestKillCriticalProcess",
		},
		{
			startString: "=== RUN   TestOOMHard",
			endString:   "--- PASS: TestOOMHard",
		},
	}

	expected := []FailureModeCheck{
		&stringInLogCheck{
			String: "Exceeded safe temperature range",
			Type:   syslogType,
		},
		&stringInLogCheck{
			String: "botanist ERROR: SerialReadErrorMsg",
			Type:   swarmingOutputType,
		},
		&stringInLogCheck{
			String:          "cas: failed to call UploadIfMissing",
			Type:            swarmingOutputType,
			InfraFailure:    true,
			OnlyOnStates:    []string{"BOT_DIED"},
			SkipPassedTest:  true,
			IgnoreFlakes:    true,
			AttributeToTest: true,
			AddTag:          true,
		},
		&stringInLogCheck{
			String: "DEVICE SUSPEND TIMED OUT",
			Type:   serialLogType,
		},
		&stringInLogCheck{
			String: "DEVICE SUSPEND TIMED OUT",
			Type:   syslogType,
		},
		&stringInLogCheck{
			String:       "critical to root job killed with",
			Type:         swarmingOutputType,
			ExceptBlocks: exceptBlocks,
		},
		&stringInLogCheck{
			String:       "critical to root job killed with",
			Type:         serialLogType,
			ExceptBlocks: exceptBlocks,
		},
		&stringInLogCheck{
			String:       "critical to root job killed with",
			Type:         syslogType,
			ExceptBlocks: exceptBlocks,
		},
	}

	if diff := cmp.Diff(expected, checks, cmp.AllowUnexported(stringInLogCheck{}, logBlock{})); diff != "" {
		t.Errorf("LoadChecksFromYAML() returned diff (-want +got):\n%s", diff)
	}
}

func TestLoadChecksFromYAML_Errors(t *testing.T) {
	tests := []struct {
		name    string
		yaml    string
		wantErr string
	}{
		{
			name:    "string_in_log missing string",
			yaml:    "failure_mode_checks: [{kind: 'string_in_log', log_types: ['serialLogType']}]",
			wantErr: "field is required",
		},
		{
			name:    "string_in_log missing type",
			yaml:    "failure_mode_checks: [{kind: 'string_in_log', string: 'error'}]",
			wantErr: "field is required",
		},
		{
			name:    "unknown kind",
			yaml:    "failure_mode_checks: [{kind: 'invalid_kind'}]",
			wantErr: "unknown check kind",
		},
		{
			name:    "unknown log type",
			yaml:    "failure_mode_checks: [{kind: 'string_in_log', string: 'error', log_types: ['invalidLogType']}]",
			wantErr: "invalid log type",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			_, err := LoadChecksFromYAML([]byte(tt.yaml))
			if err == nil {
				t.Fatal("expected error, got nil")
			}
			if !strings.Contains(err.Error(), tt.wantErr) {
				t.Errorf("error %q does not contain %q", err.Error(), tt.wantErr)
			}
		})
	}
}

func TestLoadChecksFromYAMLEmpty(t *testing.T) {
	yamlData := "failure_mode_checks:\n"
	checks, err := LoadChecksFromYAML([]byte(yamlData))
	if err != nil {
		t.Fatalf("LoadChecksFromYAML failed on empty config: %v", err)
	}

	if len(checks) != 0 {
		t.Errorf("Expected 0 checks, got %d", len(checks))
	}
}
