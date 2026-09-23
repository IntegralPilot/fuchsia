// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// @dart=2.12

import 'sl4f_client.dart';

/// Interact with the media sessions of the system.
class MediaSession {
  final Sl4f _sl4f;

  MediaSession(this._sl4f);

  /// Get active media session player status.
  ///
  /// It will return null if there's no active session.
  /// Otherwise, it will return active player state as a string.
  /// The possible state values:
  ///   - Idle
  ///   - Playing
  ///   - Paused
  ///   - Buffering
  ///   - Error
  Future<String?> getActiveSessionStatus() async {
    final result = await _sl4f
        .request('media_session_facade.WatchActiveSessionStatus', {});
    if (result == null || !result.containsKey('player_state')) {
      return null;
    } else {
      return result['player_state'];
    }
  }
}
