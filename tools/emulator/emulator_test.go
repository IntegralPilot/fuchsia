// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package emulator

import (
	"bufio"
	"bytes"
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"

	"go.fuchsia.dev/fuchsia/tools/lib/productbundle"
)

func TestCheckForLogMessage(t *testing.T) {
	logLines := []string{
		"Some message",
		"Another message",
		"First message we're looking for",
		"Another message",
		"Second message we're looking for",
	}
	testStr := strings.Join(logLines, "\n")
	// attach a newline, because the reader expects this
	testStr = fmt.Sprintf("%s\n", testStr)
	fakeReader := bufio.NewReader(strings.NewReader(testStr))
	i := Instance{}
	if err := i.checkForLogMessage(fakeReader, logLines[2]); err != nil {
		t.Fatal(err)
	}

	fakeReader = bufio.NewReader(strings.NewReader(testStr))

	i = Instance{stdout: fakeReader}
	if err := i.WaitForLogMessage(logLines[2]); err != nil {
		t.Fatal(err)
	}

	fakeReader = bufio.NewReader(strings.NewReader(testStr))

	i = Instance{stdout: fakeReader}
	if err := i.WaitForLogMessages([]string{logLines[2]}); err != nil {
		t.Fatal(err)
	}

	fakeReader = bufio.NewReader(strings.NewReader(testStr))

	i = Instance{stdout: fakeReader}
	if err := i.WaitForLogMessages([]string{logLines[2], logLines[4]}); err != nil {
		t.Fatal(err)
	}

	fakeReader = bufio.NewReader(strings.NewReader(testStr))

	i = Instance{stdout: fakeReader}
	if err := i.WaitForLogMessages([]string{logLines[4], logLines[2]}); err != nil {
		t.Fatal(err)
	}
}

func TestSetLogDestination(t *testing.T) {
	expectedLogOutput := `line 1
line 2
line 3
`

	// Route the expected log output through `cat` to provide a more accurate testing scenario:
	// Instance.Wait() should observe EOF on Instance.stdout exactly when Instance.cmd has
	// terminated.
	cmd := exec.Command("cat")
	cmd.Stdin = strings.NewReader(expectedLogOutput)
	catStdout, err := cmd.StdoutPipe()
	if err != nil {
		t.Fatal(err)
	}

	if err := cmd.Start(); err != nil {
		t.Fatal(err)
	}

	i := Instance{
		stdout: bufio.NewReader(catStdout),
		cmd:    cmd,
	}

	var buf bytes.Buffer
	i.SetLogDestination(&buf)

	if _, err := i.Wait(); err != nil {
		t.Fatal(err)
	}
	if buf.String() != expectedLogOutput {
		t.Fatalf("got buf.String() = %q, wanted %q", buf.String(), expectedLogOutput)
	}
}

func TestFindImageByName(t *testing.T) {
	tmpDir := t.TempDir()
	pb := productbundle.ProductBundle{
		SystemA: []productbundle.SystemImage{
			{Name: "zircon-a", Path: "zircon-a.zbi", Type: "zbi"},
		},
		SystemR: []productbundle.SystemImage{
			{Name: "zircon-r", Path: "zircon-r.zbi", Type: "zbi"},
		},
	}
	pbData, err := json.Marshal(pb)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(tmpDir, "product_bundle.json"), pbData, 0o644); err != nil {
		t.Fatal(err)
	}

	dist := &Distribution{
		pbPath: tmpDir,
	}

	// Test finding SystemA image
	img, err := dist.FindImageByName("zircon-a", "zbi")
	if err != nil {
		t.Fatalf("FindImageByName(zircon-a) failed: %v", err)
	}
	expectedPath := filepath.Join(tmpDir, "zircon-a.zbi")
	if img.Path != expectedPath {
		t.Errorf("expected path %q, got %q", expectedPath, img.Path)
	}

	// Test finding SystemR image (recovery)
	img, err = dist.FindImageByName("zircon-r", "zbi")
	if err != nil {
		t.Fatalf("FindImageByName(zircon-r) failed: %v", err)
	}
	expectedPath = filepath.Join(tmpDir, "zircon-r.zbi")
	if img.Path != expectedPath {
		t.Errorf("expected path %q, got %q", expectedPath, img.Path)
	}
}

func TestInstanceKill(t *testing.T) {
	inst := &Instance{}
	if err := inst.Kill(); err != nil {
		t.Errorf("Kill on empty instance failed: %v", err)
	}
}
