// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package bazel2gn

import (
	"testing"

	"github.com/google/go-cmp/cmp"
)

func TestMustMergeMaps(t *testing.T) {
	tests := []struct {
		name string
		m1   map[string]string
		m2   map[string]string
		want map[string]string
	}{
		{
			name: "two empty maps",
			m1:   map[string]string{},
			m2:   map[string]string{},
			want: map[string]string{},
		},
		{
			name: "first map empty",
			m1:   map[string]string{},
			m2:   map[string]string{"a": "b"},
			want: map[string]string{"a": "b"},
		},
		{
			name: "second map empty",
			m1:   map[string]string{"a": "b"},
			m2:   map[string]string{},
			want: map[string]string{"a": "b"},
		},
		{
			name: "no overlapping keys",
			m1: map[string]string{
				"a": "b",
				"b": "c",
			},
			m2: map[string]string{
				"c": "d",
			},
			want: map[string]string{
				"a": "b",
				"b": "c",
				"c": "d",
			},
		},
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			got := mustMergeMaps(tc.m1, tc.m2)
			if diff := cmp.Diff(tc.want, got); diff != "" {
				t.Errorf("mustMergeMaps(%v, %v) got result diff (-want, +got):\n%s", tc.m1, tc.m2, diff)
			}
		})
	}
}

func TestMustMergeMapsPanic(t *testing.T) {
	tests := []struct {
		name string
		m1   map[string]string
		m2   map[string]string
	}{
		{
			name: "overlapping keys",
			m1:   map[string]string{"a": "b"},
			m2:   map[string]string{"a": "c"},
		},
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			defer func() {
				if r := recover(); r == nil {
					t.Error("mustMergeMaps didn't panic as expected")
				}
			}()
			mustMergeMaps(tc.m1, tc.m2)
		})
	}
}
