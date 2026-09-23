// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package util

// FindRepeatingPeriod tries to find a repeating pattern in value.
// For example, for []uint64{1, 2, 3, 1, 2, 3, 1} it returns 3, true.
// It searches greedily and will not always succeed in finding a pattern.
func FindRepeatingPeriod(value []uint64) (int, bool) {
	if len(value) == 0 {
		return 0, false
	}

	period := -1
	for i, v := range value[1:] {
		if v == value[0] {
			period = i + 1
			break
		}
	}
	if period == -1 {
		return 0, false
	}

	for i, v := range value {
		if v != value[i%period] {
			return 0, false
		}
	}
	return period, true
}
