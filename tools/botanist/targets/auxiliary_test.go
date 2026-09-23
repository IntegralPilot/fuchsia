// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package targets

import (
	"reflect"
	"testing"
)

func TestAuxiliary(t *testing.T) {
	tests := []struct {
		name    string
		config  AuxiliaryConfig
		want    any
		wantErr bool
	}{
		{
			name: "valid config",
			config: AuxiliaryConfig{
				"type":   "auxiliary",
				"device": "AccessPoint",
				"ip":     "192.168.42.11",
			},
			want: AuxiliaryConfig{
				"type": "AccessPoint",
				"ip":   "192.168.42.11",
			},
		}, {
			name: "missing device",
			config: AuxiliaryConfig{
				"type": "auxiliary",
				"ip":   "192.168.42.11",
			},
			wantErr: true,
		}, {
			name: "target is not a string",
			config: AuxiliaryConfig{
				"type":   "auxiliary",
				"device": 1,
				"ip":     "192.168.42.11",
			},
			wantErr: true,
		}, {
			name: "device is empty string",
			config: AuxiliaryConfig{
				"type":   "auxiliary",
				"device": "",
				"ip":     "192.168.42.11",
			},
			wantErr: true,
		},
	}

	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			a, err := NewAuxiliary(test.config)
			if test.wantErr {
				if err == nil {
					t.Fatalf("expected error from NewAuxiliary, got none")
				}
				return
			}
			if err != nil {
				t.Fatalf("unexpected error from NewAuxiliary: %s", err)
			}

			got, err := a.TestConfig(false)
			if err != nil {
				t.Fatalf("unexpected error from TestConfig: %s", err)
			}
			if !reflect.DeepEqual(got, test.want) {
				t.Fatalf("got %s, want TestConfig()=%s", got, test.want)
			}
		})
	}
}
