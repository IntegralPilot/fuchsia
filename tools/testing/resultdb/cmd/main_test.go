// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package main

import (
	"strings"
	"testing"

	"github.com/google/go-cmp/cmp"
	resultpb "go.chromium.org/luci/resultdb/proto/v1"
	"google.golang.org/protobuf/testing/protocmp"
)

func TestStringPairConvert(t *testing.T) {
	tests := []struct {
		tag  string
		want *resultpb.StringPair
	}{
		{
			tag:  "swarming_id   :   foo_bar",
			want: &resultpb.StringPair{Key: "swarming_id", Value: "foo_bar"},
		},
		{
			tag:  "swarming-bot-id: abc-def",
			want: &resultpb.StringPair{Key: "swarming-bot-id", Value: "abc-def"},
		},
	}
	for _, tc := range tests {
		got, err := stringPairFromString(tc.tag)
		if err != nil {
			t.Errorf("stringPairFromString(%s) errored %v", tc.tag, err)
		}
		if diff := cmp.Diff(tc.want, got, protocmp.Transform()); diff != "" {
			t.Errorf("stringPairFromString diff (-want +got):\n%s", diff)
		}
	}
}

func TestConvertTags(t *testing.T) {
	tags := []string{"tag1:value1", "  tag2  :  value2  ", "tag3:"}
	out, err := convertTags(tags)
	if err != nil {
		t.Errorf("convertTags(%v) errored %v", tags, err)
	}
	if len(out) != len(tags) {
		t.Errorf("convertTags(%v) did not convert to the correct number of keypairs %v", tags, len(tags))
	}
	for _, pair := range out {
		if strings.Contains(pair.Key, " ") || strings.Contains(pair.Value, " ") {
			t.Errorf("convertTags(%v) = %v did not did not remove extra space", tags, pair)
		}
	}
}
