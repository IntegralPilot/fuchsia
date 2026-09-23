// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// @dart=2.12

import 'dart:convert';
import 'dart:io';

import 'package:sl4f/sl4f.dart';
import 'package:test/test.dart';

void main(List<String> args) {
  late HttpServer fakeServer;
  late Sl4f sl4f;

  setUp(() async {
    fakeServer = await HttpServer.bind('127.0.0.1', 18080);
    sl4f = Sl4f('127.0.0.1', null, 18080);
  });

  tearDown(() async {
    await fakeServer.close();
  });

  test('call getActiveSessionStatus with no active session', () {
    void handler(HttpRequest req) async {
      expect(req.contentLength, greaterThan(0));
      final body = jsonDecode(await utf8.decoder.bind(req).join());
      expect(body['method'], 'media_session_facade.WatchActiveSessionStatus');
      req.response
          .write(jsonEncode({'id': body['id'], 'result': null, 'error': null}));
      await req.response.close();
    }

    fakeServer.listen(handler);
    expect(
        MediaSession(sl4f).getActiveSessionStatus(), completion(equals(null)));
  });

  test('call getActiveSessionStatus with active session', () {
    void handler(HttpRequest req) async {
      expect(req.contentLength, greaterThan(0));
      final body = jsonDecode(await utf8.decoder.bind(req).join());
      expect(body['method'], 'media_session_facade.WatchActiveSessionStatus');
      req.response.write(jsonEncode({
        'id': body['id'],
        'result': {
          "content_type": "Video",
          "duration": 202000000000,
          "error": null,
          "is_live": false,
          "player_state": "Playing",
          "repeat_mode": "Off",
          "shuffle_on": false
        },
        'error': null
      }));
      await req.response.close();
    }

    fakeServer.listen(handler);
    expect(MediaSession(sl4f).getActiveSessionStatus(),
        completion(equals('Playing')));
  });

  test('call getActiveSessionStatus with abnormal result content', () {
    void handler(HttpRequest req) async {
      expect(req.contentLength, greaterThan(0));
      final body = jsonDecode(await utf8.decoder.bind(req).join());
      expect(body['method'], 'media_session_facade.WatchActiveSessionStatus');
      // Missing 'player_state' key-value pair
      req.response.write(jsonEncode({
        'id': body['id'],
        'result': {
          "content_type": "Video",
          "duration": 202000000000,
          "error": null,
          "is_live": false,
          "repeat_mode": "Off",
          "shuffle_on": false
        },
        'error': null
      }));
      await req.response.close();
    }

    fakeServer.listen(handler);
    expect(
        MediaSession(sl4f).getActiveSessionStatus(), completion(equals(null)));
  });
}
