// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package bundler

import (
	"context"
	"fmt"
)

// A simple in-memory implementation of dataSink.
type memSink struct {
	contents map[string][]byte
	err      error
	dir      string
}

func NewMemSink(contents map[string][]byte, err error, dir string) *memSink {
	return &memSink{
		contents: contents,
		err:      err,
		dir:      dir,
	}
}

func (s *memSink) ReadFromGCS(ctx context.Context, object string) ([]byte, error) {
	if s.err != nil {
		return nil, s.err
	}
	if _, ok := s.contents[object]; !ok {
		return nil, fmt.Errorf("file not found")
	}
	return s.contents[object], nil
}

func (s *memSink) GetBucketName() string {
	return s.dir
}

func (s *memSink) DoesPathExist(ctx context.Context, prefix string) (bool, error) {
	if s.err != nil {
		return false, s.err
	}
	if _, ok := s.contents[prefix]; !ok {
		return false, nil
	}
	return true, nil
}
