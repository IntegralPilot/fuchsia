// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package ffx

import (
	"context"
	"encoding/json"
	"os"
	"path/filepath"
	"testing"

	"github.com/google/go-cmp/cmp"
)

// createScript returns the path to a bash script that output the given content.
func createScript(t *testing.T, contents string) string {
	name := filepath.Join(t.TempDir(), "ffxtool.sh")
	contents = `#!/bin/bash
	echo '` + contents + `'`
	if err := os.WriteFile(name, []byte(contents), 0o700); err != nil {
		t.Fatal(err)
	}
	return name
}

func TestTargetListEmpty(t *testing.T) {
	data, err := json.Marshal([]TargetEntry{})
	if err != nil {
		t.Fatalf("Failed to marshal: %s", err)
	}

	ffxtoolScript := createScript(t, string(data))

	runDir := NewRunDir(filepath.Join(t.TempDir(), "ffx-run-dir"))
	ffx, err := NewFFXTool(ffxtoolScript, runDir)
	if err != nil {
		t.Fatalf("Failed to create ffx tool: %s", err)
	}
	entries, err := ffx.TargetList(context.Background(), "", 0)
	if err != nil {
		t.Fatalf("Failed to run target list: %s", err)
	}
	if len(entries) != 0 {
		t.Fatalf("entries not empty: %v", entries)
	}
}

func TestTargetList(t *testing.T) {
	expected_entries := []TargetEntry{
		{NodeName: "1", Addresses: []TargetAddress{{Type: "Ip", IP: "127.0.0.1"}}, TargetState: "Product"},
		{NodeName: "2", Addresses: []TargetAddress{{Type: "Ip", IP: "127.0.0.2"}}, TargetState: "Product"},
		{NodeName: "fuchsia-5254-475e-82ef", Addresses: []TargetAddress{{Type: "Ip", IP: "fe80::9bf7:2e3:c4f8:9638%qemu"}}, TargetState: "Product"},
	}
	data, err := json.Marshal(expected_entries)
	if err != nil {
		t.Fatalf("Failed to marshal: %s", err)
	}

	ffxtoolScript := createScript(t, string(data))

	runDir := NewRunDir(filepath.Join(t.TempDir(), "ffx-run-dir"))
	ffx, err := NewFFXTool(ffxtoolScript, runDir)
	if err != nil {
		t.Fatalf("Failed to create ffx tool: %s", err)
	}
	entries, err := ffx.TargetList(context.Background(), "", 0)
	if err != nil {
		t.Fatalf("Failed to run target list: %s", err)
	}
	if diff := cmp.Diff(entries, expected_entries); diff != "" {
		t.Fatalf("unexpected entries, diff:\n%s", diff)
	}

	entries, err = ffx.TargetListForNode(context.Background(), "1")
	if err != nil {
		t.Fatalf("Failed to run target list: %s", err)
	}
	if diff := cmp.Diff(entries, expected_entries[:1]); diff != "" {
		t.Fatalf("unexpected entries, diff:\n%s", diff)
	}

	target, err := ffx.WaitForTarget(context.Background(), "127.0.0.1")
	if err != nil {
		t.Fatalf("Failed to run target list: %s", err)
	}
	if target.NodeName != "1" {
		t.Fatalf("unexpected device name, expected 1, got %s", target.NodeName)
	}
}

func TestTargetListStrict(t *testing.T) {
	expectedEntries := []TargetEntry{
		{NodeName: "1", Addresses: []TargetAddress{{Type: "Ip", IP: "127.0.0.1"}}, TargetState: "Product"},
		{NodeName: "2", Addresses: []TargetAddress{{Type: "Ip", IP: "127.0.0.2"}}, TargetState: "Product"},
	}
	data, err := json.Marshal(expectedEntries)
	if err != nil {
		t.Fatalf("Failed to marshal: %s", err)
	}

	ffxtoolScript := createScript(t, string(data))

	runDir := NewRunDir(filepath.Join(t.TempDir(), "ffx-run-dir"))

	// We must call newFfxStrict directly because NewFFXToolForVersion still defaults to daemon mode.
	ffxStrict, err := newFfxStrict(context.Background(), ffxtoolScript, runDir, "")
	if err != nil {
		t.Fatalf("Failed to create ffx strict: %s", err)
	}
	entries, err := ffxStrict.TargetList(context.Background(), "", 0)
	if err != nil {
		t.Fatalf("Failed to run target list: %s", err)
	}
	if diff := cmp.Diff(entries, expectedEntries); diff != "" {
		t.Fatalf("unexpected entries, diff:\n%s", diff)
	}
}

func TestCloseDaemon(t *testing.T) {
	ffxtoolScript := createScript(t, "")
	runDir := RunDir{path: t.TempDir()}
	ffx, err := newFfxDaemon(context.Background(), ffxtoolScript, runDir, "")
	if err != nil {
		t.Fatal(err)
	}
	if err := ffx.Close(context.Background()); err != nil {
		t.Fatalf("Close failed: %s", err)
	}
}

func TestCloseStrict(t *testing.T) {
	ffxtoolScript := createScript(t, "")
	runDir := RunDir{path: t.TempDir()}
	ffxStrict, err := newFfxStrict(context.Background(), ffxtoolScript, runDir, "")
	if err != nil {
		t.Fatal(err)
	}
	if err := ffxStrict.Close(context.Background()); err != nil {
		t.Fatalf("Close failed: %s", err)
	}
}

func TestMatchTarget(t *testing.T) {
	entry := TargetEntry{
		NodeName: "my-node",
		Serial:   "12345",
		Addresses: []TargetAddress{
			{Type: "Ip", IP: "192.168.1.5", SSHPort: 2222},
		},
	}

	tests := []struct {
		target string
		want   bool
	}{
		{"", true},
		{"my-node", true},
		{"12345", true},
		{"192.168.1.5", true},
		{"192.168.1.5:2222", true},
		{"other-node", false},
		{"192.168.1.6", false},
	}

	for _, tc := range tests {
		got := matchTarget(entry, tc.target)
		if got != tc.want {
			t.Errorf("matchTarget(entry, %q) = %v; want %v", tc.target, got, tc.want)
		}
	}
}
