// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package idletest

import (
	"fmt"
	"testing"
	"time"
)

func TestIdle4Mins(t *testing.T) {
	mpower, err := NewPowerMeasurement("Idle4Mins")
	if err != nil {
		t.Fatalf("NewPowerMeasurement: %s", err)
	}
	defer func() {
		if err := mpower.Stop(); err != nil {
			t.Fatalf("mpower.Stop: %s", err)
		}
	}()
	for i := 0; i < 4; i++ {
		fmt.Printf("1 minute sleep (%d/4)...\n", i+1)
		time.Sleep(time.Minute)
	}
	fmt.Println("Wake up!")
}
