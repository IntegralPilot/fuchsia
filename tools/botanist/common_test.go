// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package botanist

import (
	"context"
	"io"
	"strings"
	"sync"
	"testing"

	"go.fuchsia.dev/fuchsia/tools/lib/streams"
)

func TestLockedWriterKeepsWritesContiguous(t *testing.T) {
	const line = "With great power comes great responsibility "
	var sb strings.Builder
	lw := NewLockedWriter(&sb)
	var wg sync.WaitGroup
	// Write each word in its own Write call. We exploit the fact
	// that strings.Builder writes all the bytes in one go.
	for _, s := range strings.SplitAfter(line, " ") {
		wg.Add(1)
		go func(s string) {
			defer wg.Done()
			lw.Write([]byte(s))
		}(s)
	}
	wg.Wait()
	// Check that each word occurs contiguously in the underlying Writer.
	for _, w := range strings.SplitAfter(sb.String(), " ") {
		if !strings.Contains(line, w) {
			t.Errorf("Failed to find contiguous write, got = %q, want = %q", "", w)
		}
	}
}

func write(t *testing.T, w io.Writer, data []string) {
	for _, subdata := range data {
		n, err := w.Write([]byte(subdata))
		if err != nil {
			t.Errorf("failed to write data %q: %s", subdata, err)
		}
		if n != len(subdata) {
			t.Errorf("received %d bytes for writing, want %d", n, len(subdata))
		}
	}
}

func TestStdioWriters(t *testing.T) {
	var w strings.Builder
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	ctx = streams.ContextWithStdout(ctx, NewLockedWriter(&w))

	stdout1, _, flush1 := NewStdioWriters(ctx, "s1")
	stdout2, _, flush2 := NewStdioWriters(ctx, "s2")

	var wg sync.WaitGroup
	wg.Add(2)
	go func() {
		defer wg.Done()
		write(t, stdout1, []string{"h", "i", " ", "f", "r", "o", "m", " st", "dout1", "\n", "extra"})
	}()
	go func() {
		defer wg.Done()
		write(t, stdout2, []string{"h", "e", "l", "l", "o ", "says std", "out2\n bye", "\n", "extra2"})
	}()

	// Wait for the writers to write all their lines.
	wg.Wait()
	// Flush the rest of the data not ending in a newline.
	flush1()
	flush2()

	for _, expectedLines := range [][]string{
		{"hi from stdout1", "extra"},
		{"hello says stdout2", "bye", "extra2"},
	} {
		startIndex := 0
		for _, line := range expectedLines {
			i := strings.Index(w.String(), line)
			if i < startIndex {
				t.Errorf("line %q missing or out of order from: %q", line, w.String())
			} else {
				startIndex = i + len(line)
			}
		}
	}
}

func TestLineWriterFiltersGoldfishOobWarning(t *testing.T) {
	var w strings.Builder
	lw := NewLineWriter(&w, "ffx")

	lines := []string{
		"normal startup line 1\n",
		"emulator: Warning: A write to goldfish_address_space_area is out-of-bound and is ignored. Offset = 0x3fc4b3000, size = 1\n",
		"normal startup line 2\n",
	}
	write(t, lw, lines)

	output := w.String()
	if strings.Contains(output, "goldfish_address_space_area is out-of-bound") {
		t.Errorf("expected Goldfish OOB warning to be filtered, but got: %s", output)
	}
	if !strings.Contains(output, "ffx: normal startup line 1\n") || !strings.Contains(output, "ffx: normal startup line 2\n") {
		t.Errorf("expected normal lines in output, got: %s", output)
	}
}
