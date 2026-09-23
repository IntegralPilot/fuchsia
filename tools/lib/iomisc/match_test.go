// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package iomisc

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"io"
	"slices"
	"strings"
	"testing"
	"time"
)

func TestMatchingReader(t *testing.T) {
	t.Run("sequence appears in a single read", func(t *testing.T) {
		sequence := []byte("ABCDE")
		var buf bytes.Buffer
		m := NewMatchingReader(&buf, sequence)
		assertMatch(t, m, nil)

		buf.Write(sequence)
		p := make([]byte, 1024)
		if _, err := m.Read(p); err != nil && !errors.Is(err, io.EOF) {
			t.Fatalf("unexpected error: %v", err)
		}
		assertMatch(t, m, sequence)
	})

	t.Run("sequence appears across multiple reads", func(t *testing.T) {
		sequence := []byte("ABCDE")
		var buf bytes.Buffer
		m := NewMatchingReader(&buf, sequence)
		assertMatch(t, m, nil)

		buf.Write([]byte("ABC"))
		p := make([]byte, 1024)
		if _, err := m.Read(p); err != nil {
			t.Fatalf("unexpected error: %v", err)
		}
		assertMatch(t, m, nil)

		buf.Write([]byte("D"))
		p = make([]byte, 1024)
		if _, err := m.Read(p); err != nil {
			t.Fatalf("unexpected error: %v", err)
		}
		assertMatch(t, m, nil)

		buf.Write([]byte("EFGH"))
		p = make([]byte, 1024)
		if _, err := m.Read(p); err != nil && !errors.Is(err, io.EOF) {
			t.Fatalf("unexpected error: %v", err)
		}
		assertMatch(t, m, sequence)
	})

	t.Run("Read throws EOFs after match", func(t *testing.T) {
		sequence := []byte("ABCDE")
		var buf bytes.Buffer
		m := NewMatchingReader(&buf, sequence)
		assertMatch(t, m, nil)

		buf.Write([]byte("ABCDE"))
		p := make([]byte, 1024)
		if _, err := m.Read(p); err != nil && !errors.Is(err, io.EOF) {
			t.Fatalf("unexpected error: %v", err)
		}
		assertMatch(t, m, sequence)

		buf.Write([]byte("FGHIJK"))
		p = make([]byte, 1024)
		if _, err := m.Read(p); !errors.Is(err, io.EOF) {
			t.Fatalf("unexpected error: expected EOF; got %v", err)
		}
		assertMatch(t, m, sequence)
	})

	t.Run("multiple sequences", func(t *testing.T) {
		sequences := [][]byte{[]byte("ABCDE"), []byte("BCDEF")}
		var buf bytes.Buffer
		m := NewMatchingReader(&buf, sequences...)
		assertMatch(t, m, nil)

		buf.Write([]byte("BCDE"))
		p := make([]byte, 1024)
		if _, err := m.Read(p); err != nil {
			t.Fatalf("unexpected error: %v", err)
		}
		assertMatch(t, m, nil)

		buf.Write([]byte("FGHIJK"))
		p = make([]byte, 1024)
		if _, err := m.Read(p); err != nil && !errors.Is(err, io.EOF) {
			t.Fatalf("unexpected error: %v", err)
		}
		assertMatch(t, m, sequences[1])
	})

	t.Run("sequence appears mid-read", func(t *testing.T) {
		sequence := []byte("BC")
		var buf bytes.Buffer
		m := NewMatchingReader(&buf, sequence)
		assertMatch(t, m, nil)
		buf.Write([]byte("ABCD"))
		p := make([]byte, 1024)
		if _, err := m.Read(p); err != nil && !errors.Is(err, io.EOF) {
			t.Fatalf("unexpected error: %v", err)
		}
		assertMatch(t, m, sequence)
	})
}

func assertMatch(t *testing.T, m *MatchingReader, match []byte) {
	t.Helper()
	if bytes.Compare(match, m.Match()) != 0 {
		t.Fatalf("expected match of %q; not %q", match, m.Match())
	}
}

func TestReadUntilMatch(t *testing.T) {
	t.Run("success", func(t *testing.T) {
		r, w := io.Pipe()
		defer w.Close()
		defer r.Close()

		go func() {
			w.Write([]byte("ABC"))
			w.Write([]byte("D"))
			w.Write([]byte("EFGH"))
		}()

		target := "ABCDE"
		match, err := ReadUntilMatchString(context.Background(), r, target)
		if err != nil {
			t.Fatalf("unexpected error: %v", err)
		}
		if match != target {
			t.Fatalf("expected match of %q; not %q", target, match)
		}
	})

	t.Run("read fails", func(t *testing.T) {
		r, w := io.Pipe()
		defer r.Close()

		go func() {
			w.Write([]byte("bar"))
			w.Close()
		}()

		_, err := ReadUntilMatchString(context.Background(), r, "foo")
		if !errors.Is(err, io.EOF) {
			t.Errorf("ReadUntilMatch() returned %v, want io.EOF", err)
		}
	})

	t.Run("cancellation", func(t *testing.T) {
		r, w := io.Pipe()
		defer r.Close()
		defer w.Close()

		ctx, cancel := context.WithDeadline(context.Background(), time.Now().Add(10*time.Millisecond))
		defer cancel()

		go func() {
			b := []byte("B")
			for {
				w.Write(b)
			}
		}()

		if _, err := ReadUntilMatchString(ctx, r, "A"); err == nil || !errors.Is(err, context.DeadlineExceeded) {
			t.Errorf("ReadUntilMatch() returned %v, want DeadlineExceeded ", err)
		}
	})
}

func TestMatcherWithDifferentReadSteps(t *testing.T) {
	tests := []struct {
		needle string
		in     []string
		want   []bool
	}{
		// "" matches empty haystack.
		{
			needle: "",
			in:     []string{""},
			want:   []bool{true},
		},
		// "" matches non-empty haystack.
		{
			needle: "",
			in:     []string{"AB"},
			want:   []bool{true},
		},
		// Unsplit haystack and needle equals the haystack.
		{
			needle: "ABC",
			in:     []string{"ABC"},
			want:   []bool{true},
		},
		// Unsplit haystack and needle is NOT in the haystack.
		{
			needle: "DE",
			in:     []string{"ABC"},
			want:   []bool{false},
		},
		// Unsplit haystack and needle is a prefix of the haystack.
		{
			needle: "AB",
			in:     []string{"ABCD"},
			want:   []bool{true},
		},
		// Unsplit haystack and needle is a suffix of the haystack.
		{
			needle: "CD",
			in:     []string{"ABCD"},
			want:   []bool{true},
		},
		// Unsplit haystack and needle is a substring of the haystack.
		{
			needle: "BC",
			in:     []string{"ABCD"},
			want:   []bool{true},
		},
		// Split haystack and needle equals the joined haystack.
		{
			needle: "ABCDE",
			in:     []string{"AB", "CDE"},
			want:   []bool{false, true},
		},
		// Split haystack and needle is NOT in the joined haystack.
		{
			needle: "ABCDE",
			in:     []string{"CAB", "CDF"},
			want:   []bool{false, false},
		},
		// Split haystack and needle is a prefix of the joined haystack.
		{
			needle: "ABC",
			in:     []string{"AB", "CDE"},
			want:   []bool{false, true},
		},
		// Split haystack and needle is a suffix of the joined haystack.
		{
			needle: "CDE",
			in:     []string{"ABC", "DE"},
			want:   []bool{false, true},
		},
		// Split haystack and needle is a suffix of the joined haystack.
		{
			needle: "AB",
			in:     []string{"A", "AB"},
			want:   []bool{false, true},
		},
		// Split haystack and needle is a substring of the joined haystack.
		{
			needle: "ABCDE",
			in:     []string{"CAB", "CD", "EFG"},
			want:   []bool{false, false, true},
		},
		// Split haystack and needle is a substring of the joined haystack.
		{
			needle: "DE",
			in:     []string{"AB", "CDEF", "G"},
			want:   []bool{false, true, true},
		},
		// Split haystack and needle is a substring of the joined haystack.
		{
			needle: "AB",
			in:     []string{"A", "CA", "B"},
			want:   []bool{false, false, true},
		},
		// Split haystack and needle is a substring of the joined haystack.
		{
			needle: "AAAB",
			in:     []string{"AA", "A", "AB"},
			want:   []bool{false, false, true},
		},
		// Split haystack and needle is a substring of the joined haystack.
		{
			needle: "BABC",
			in:     []string{"B", "AB", "ABC"},
			want:   []bool{false, false, true},
		},
		// Split haystack and needle is NOT a substring of the joined haystack.
		{
			needle: "ABC",
			in:     []string{"B", "AB", "BC"},
			want:   []bool{false, false, false},
		},
	}

	for _, test := range tests {
		t.Run(fmt.Sprintf("Match(%q)<-[%s]", test.needle, strings.Join(test.in, ",")), func(t *testing.T) {
			m := NewMatcher([]byte(test.needle))
			got := make([]bool, 0, len(test.in))
			for _, p := range test.in {
				got = append(got, m.Match([]byte(p)))
			}
			if !slices.Equal(got, test.want) {
				t.Errorf("%s, got matches %v, want matches %v", t.Name(), got, test.want)
			}
		})
	}
}
