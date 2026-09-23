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

	"go.fuchsia.dev/fuchsia/tools/lib/logger"
)

// MatchingReader is an io.Reader implementation that wraps another such
// implementation. It reads only up until one of the sequences has been read consecutively.
type MatchingReader struct {
	r        io.Reader
	toMatch  [][]byte
	progress []int
	matchIdx int
}

// NewMatchingReader returns a MatchingReader that matches any of toMatch.
func NewMatchingReader(reader io.Reader, toMatch ...[]byte) *MatchingReader {
	return &MatchingReader{
		r:        reader,
		toMatch:  toMatch,
		progress: make([]int, len(toMatch)),
		matchIdx: -1,
	}
}

// Match returns the first match among the bytes read, or nil if there
// has yet to be a match.
func (m *MatchingReader) Match() []byte {
	if m.matchIdx >= 0 {
		return m.toMatch[m.matchIdx]
	}
	return nil
}

// Read reads from the underlying reader and checks whether the pattern
// has been matched among the bytes read. Once a match has been found,
// subsequent reads will return an io.EOF.
func (m *MatchingReader) Read(p []byte) (int, error) {
	if m.matchIdx >= 0 {
		return 0, io.EOF
	}
	n, err := m.r.Read(p)
	p = p[:n]
	for i, tm := range m.toMatch {
		for j := 0; j < n; j++ {
			remainingToMatch := tm[m.progress[i]:]
			relevantP := p[j:min(len(remainingToMatch)+j, len(p))]
			if bytes.HasPrefix(remainingToMatch, relevantP) {
				m.progress[i] += len(relevantP)
				if m.progress[i] == len(tm) {
					m.matchIdx = i
				}
				break
			} else {
				m.progress[i] = 0
			}
		}
		if m.matchIdx >= 0 {
			return n, io.EOF
		}
	}
	return n, err
}

// ReadUntilMatch reads from a Reader until it encounters an occurrence of one
// of the byte slices specified in toMatch.
// Checks ctx for cancellation only between calls to m.Read(), so cancellation
// will not be noticed if m.Read() blocks.
// See https://github.com/golang/go/issues/20280 for discussion of similar issues.
func ReadUntilMatch(ctx context.Context, reader io.Reader, toMatch ...[]byte) ([]byte, error) {
	m := NewMatchingReader(reader, toMatch...)
	// buf size considerations: smaller => more responsive to ctx cancellation,
	// larger => less CPU overhead.
	buf := make([]byte, 1024)
	lastReadSize := 0
	var match []byte
	for ctx.Err() == nil {
		readErr := make(chan error, 1)
		go func() {
			var err error
			lastReadSize, err = m.Read(buf)
			if errors.Is(err, io.EOF) {
				match = m.Match()
				if match != nil {
					readErr <- nil
					return
				}
			}
			readErr <- err
		}()
		select {
		case <-ctx.Done():
			break
		case err := <-readErr:
			if err != nil {
				return nil, err
			} else if match != nil {
				return match, nil
			}
		}
	}

	// If we time out, it is helpful to see the last bytes processed.
	logger.Debugf(ctx, "ReadUntilMatch(%q): last %d bytes read before cancellation: %q", bytes.Join(toMatch, []byte(", ")), lastReadSize, buf[:lastReadSize])

	return nil, ctx.Err()
}

// ReadUntilMatchString has identical behavior to ReadUntilMatch, but accepts
// and returns strings instead of byte slices.
func ReadUntilMatchString(ctx context.Context, reader io.Reader, strings ...string) (string, error) {
	var toMatch [][]byte
	for _, s := range strings {
		toMatch = append(toMatch, []byte(s))
	}
	b, err := ReadUntilMatch(ctx, reader, toMatch...)
	return string(b), err
}

// Matcher checks if a needle is present in an incrementally gathered haystack.
type Matcher struct {
	// needle is the sequence to match.
	needle []byte
	// i is the length of the needle prefix matched so far.
	i int
}

// NewMatcher creates a Matcher that matches the given needle.
func NewMatcher(needle []byte) *Matcher {
	return &Matcher{
		needle: needle,
		i:      0,
	}
}

// String returns a string representation for Matcher.
//
// It is formatted as Matcher("matched so far"_"not yet matched"). The
// quotes are omitted if the matched or the unmatched portion is empty.
// This is primarily useful in tests.
func (m *Matcher) String() string {
	switch m.i {
	case 0:
		return fmt.Sprintf("Matcher(_%q)", m.needle)
	case len(m.needle):
		return fmt.Sprintf("Matcher(%q_)", m.needle)
	default:
		return fmt.Sprintf("Matcher(%q_%q)", m.needle[:m.i], m.needle[m.i:])
	}
}

// Match checks if the needle is present in the partial haystack concatenated with prior haystacks.
//
// Once the needle is found, it always returns true.
func (m *Matcher) Match(haystack []byte) bool {
	nl, hl := len(m.needle), len(haystack)
	// Exit early if needle is already found or for a trivial match ("" matches everything).
	if m.i == nl {
		return true
	}
	// hb is the beginning of the haystack suffix.
	for hb := 0; hb < hl; hb++ {
		// Match a substring of the needle that excludes the matched prefix and extends
		// up to the end of the shorter of the needle or the current haystack suffix.
		needle := m.needle[m.i:min(nl, m.i+hl-hb)]
		if bytes.HasPrefix(haystack[hb:], needle) {
			m.i += len(needle)
			return m.i == nl
		}
		// Find a non-trivial suffix of the matched needle that is a prefix of the needle.
		// On success, attempt another match at the same position in the haystack but with
		// the prefix of the needle found previously.
		for j := 1; j <= m.i; j++ {
			if bytes.HasPrefix(m.needle, m.needle[j:m.i]) {
				m.i -= j // shorten the matched needle
				hb -= 1  // retry the match
				break
			}
		}
	}
	return false
}
