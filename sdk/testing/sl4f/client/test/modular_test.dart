// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// TODO(https://fxbug.dev/42165807): Fix null safety and remove this language version.
// @dart=2.9

import 'dart:convert';
import 'dart:io';

import 'package:mockito/mockito.dart';
import 'package:sl4f/sl4f.dart';
import 'package:test/test.dart';

class MockSsh extends Mock implements Ssh {}

void main(List<String> args) {
  HttpServer fakeServer;
  Sl4f sl4f;
  MockSsh ssh;

  setUp(() async {
    ssh = MockSsh();
    fakeServer = await HttpServer.bind('127.0.0.1', 18080);
    sl4f = Sl4f('127.0.0.1', ssh, 18080);
  });

  tearDown(() async {
    await fakeServer.close();
  });

  test('call RestartSession facade with no params', () {
    void handler(HttpRequest req) async {
      expect(req.contentLength, greaterThan(0));
      final body = jsonDecode(await utf8.decoder.bind(req).join());
      expect(body['method'], 'modular_facade.RestartSession');
      expect(body['params'], null);
      req.response.write(
          jsonEncode({'id': body['id'], 'result': 'Success', 'error': null}));
      await req.response.close();
    }

    fakeServer.listen(handler);

    expect(Modular(sl4f).restartSession(), completion(equals('Success')));
  });

  test('call startBasemgr facade with params', () {
    const sessionUrl =
        'fuchsia-pkg://fuchsia.com/fake_session#meta/fake_session.cm';
    void handler(HttpRequest req) async {
      expect(req.contentLength, greaterThan(0));
      final body = jsonDecode(await utf8.decoder.bind(req).join());
      expect(body['method'], 'modular_facade.StartBasemgr');
      expect(body['params'], isNotNull);
      expect(
          body['params'],
          containsPair(
              'config',
              allOf(containsPair('basemgr', contains('base_shell')),
                  containsPair('sessionmgr', contains('session_agents')))));
      expect(body['params'], containsPair('session_url', sessionUrl));
      req.response.write(
          jsonEncode({'id': body['id'], 'result': 'Success', 'error': null}));
      await req.response.close();
    }

    fakeServer.listen(handler);

    expect(Modular(sl4f).startBasemgr('''{
      "basemgr": {
        "base_shell": {
          "url": "foo",
          "args": ["--bar"]
        }
      },
      "sessionmgr": {
        "session_agents": ["baz"]
      }
    }''', sessionUrl), completion(equals('Success')));
  });

  test('call startBasemgr facade with no params', () {
    void handler(HttpRequest req) async {
      expect(req.contentLength, greaterThan(0));
      final body = jsonDecode(await utf8.decoder.bind(req).join());
      expect(body['method'], 'modular_facade.StartBasemgr');
      expect(body['params'], isNotNull);
      expect(body['params'], isEmpty);
      req.response.write(
          jsonEncode({'id': body['id'], 'result': 'Success', 'error': null}));
      await req.response.close();
    }

    fakeServer.listen(handler);

    expect(Modular(sl4f).startBasemgr(), completion(equals('Success')));
  });

  test('call boot with no config', () async {
    const sessionUrl =
        'fuchsia-pkg://fuchsia.com/fake_session#meta/fake_session.cm';
    bool called = false;
    void handler(HttpRequest req) async {
      expect(req.contentLength, greaterThan(0));
      final body = jsonDecode(await utf8.decoder.bind(req).join());
      if (body['method'] == 'modular_facade.IsBasemgrRunning') {
        req.response.write(jsonEncode({
          'id': body['id'],
          'result': called,
          'error': null,
        }));
      } else {
        expect(body['method'], 'modular_facade.StartBasemgr');
        expect(body['params'], {'session_url': sessionUrl});
        called = true;
        req.response.write(
            jsonEncode({'id': body['id'], 'result': 'Success', 'error': null}));
      }
      await req.response.close();
    }

    fakeServer.listen(handler);

    await Modular(sl4f).boot(sessionUrl: sessionUrl);
    expect(called, isTrue, reason: 'StartBasemgr facade not called');
  });

  test('call boot with sessionUrl', () async {
    const sessionUrl =
        'fuchsia-pkg://fuchsia.com/fake_session#meta/fake_session.cm';
    bool called = false;
    void handler(HttpRequest req) async {
      expect(req.contentLength, greaterThan(0));
      final body = jsonDecode(await utf8.decoder.bind(req).join());
      if (body['method'] == 'modular_facade.IsBasemgrRunning') {
        req.response.write(jsonEncode({
          'id': body['id'],
          'result': called,
          'error': null,
        }));
      } else {
        expect(body['method'], 'modular_facade.StartBasemgr');
        expect(body['params'], isNotNull);
        expect(body['params'], containsPair('session_url', sessionUrl));
        called = true;
        req.response.write(
            jsonEncode({'id': body['id'], 'result': 'Success', 'error': null}));
      }
      await req.response.close();
    }

    fakeServer.listen(handler);

    await Modular(sl4f).boot(sessionUrl: sessionUrl);
    expect(called, isTrue, reason: 'StartBasemgr facade not called');
  });

  test('call boot with custom config', () async {
    const sessionUrl =
        'fuchsia-pkg://fuchsia.com/fake_session#meta/fake_session.cm';
    bool called = false;
    void handler(HttpRequest req) async {
      expect(req.contentLength, greaterThan(0));
      final body = jsonDecode(await utf8.decoder.bind(req).join());
      if (body['method'] == 'modular_facade.IsBasemgrRunning') {
        req.response.write(jsonEncode({
          'id': body['id'],
          'result': called,
          'error': null,
        }));
      } else {
        expect(body['method'], 'modular_facade.StartBasemgr');
        expect(body['params'], isNotNull);
        expect(
            body['params'],
            containsPair(
                'config',
                allOf(containsPair('basemgr', contains('base_shell')),
                    containsPair('sessionmgr', contains('session_agents')))));
        called = true;
        req.response.write(
            jsonEncode({'id': body['id'], 'result': 'Success', 'error': null}));
      }
      await req.response.close();
    }

    fakeServer.listen(handler);

    await Modular(sl4f).boot(config: '''{
      "basemgr": {
        "base_shell": {
          "url": "foo",
          "args": ["--bar"]
        }
      },
      "sessionmgr": {
        "session_agents": ["baz"]
      }
    }''', sessionUrl: sessionUrl);

    expect(called, isTrue, reason: 'StartBasemgr facade not called');
  });

  test('call boot with custom config and sessionUrl', () async {
    const sessionUrl =
        'fuchsia-pkg://fuchsia.com/fake_session#meta/fake_session.cm';
    bool called = false;
    void handler(HttpRequest req) async {
      expect(req.contentLength, greaterThan(0));
      final body = jsonDecode(await utf8.decoder.bind(req).join());
      if (body['method'] == 'modular_facade.IsBasemgrRunning') {
        req.response.write(jsonEncode({
          'id': body['id'],
          'result': called,
          'error': null,
        }));
      } else {
        expect(body['method'], 'modular_facade.StartBasemgr');
        expect(body['params'], isNotNull);
        expect(
            body['params'],
            containsPair(
                'config',
                allOf(containsPair('basemgr', contains('base_shell')),
                    containsPair('sessionmgr', contains('session_agents')))));
        expect(body['params'], containsPair('session_url', sessionUrl));
        called = true;
        req.response.write(
            jsonEncode({'id': body['id'], 'result': 'Success', 'error': null}));
      }
      await req.response.close();
    }

    fakeServer.listen(handler);

    await Modular(sl4f).boot(config: '''{
      "basemgr": {
        "base_shell": {
          "url": "foo",
          "args": ["--bar"]
        }
      },
      "sessionmgr": {
        "session_agents": ["baz"]
      }
    }''', sessionUrl: sessionUrl);

    expect(called, isTrue, reason: 'StartBasemgr facade not called');
  });

  test('isRunning: no', () {
    void handler(HttpRequest req) async {
      expect(req.contentLength, greaterThan(0));
      final body = jsonDecode(await utf8.decoder.bind(req).join());
      expect(body['method'], 'modular_facade.IsBasemgrRunning');
      req.response.write(jsonEncode({
        'id': body['id'],
        'result': false,
        'error': null,
      }));
      await req.response.close();
    }

    fakeServer.listen(handler);

    expect(Modular(sl4f).isRunning, completion(equals(false)));
  });

  test('isRunning: yes', () {
    void handler(HttpRequest req) async {
      expect(req.contentLength, greaterThan(0));
      final body = jsonDecode(await utf8.decoder.bind(req).join());
      expect(body['method'], 'modular_facade.IsBasemgrRunning');
      req.response.write(jsonEncode({
        'id': body['id'],
        'result': true,
        'error': null,
      }));
      await req.response.close();
    }

    fakeServer.listen(handler);

    expect(Modular(sl4f).isRunning, completion(equals(true)));
  });

  test('call KillBasemgr facade with no params', () {
    void handler(HttpRequest req) async {
      expect(req.contentLength, greaterThan(0));
      final body = jsonDecode(await utf8.decoder.bind(req).join());
      expect(body['method'], 'modular_facade.KillBasemgr');
      expect(body['params'], null);
      req.response.write(
          jsonEncode({'id': body['id'], 'result': 'Success', 'error': null}));
      await req.response.close();
    }

    fakeServer.listen(handler);

    expect(Modular(sl4f).killBasemgr(), completion(equals('Success')));
  });

  test('shutdown kills modular when it owns it', () async {
    const sessionUrl =
        'fuchsia-pkg://fuchsia.com/fake_session#meta/fake_session.cm';
    bool killed = true;
    void handler(HttpRequest req) async {
      expect(req.contentLength, greaterThan(0));
      final body = jsonDecode(await utf8.decoder.bind(req).join());
      if (body['method'] == 'modular_facade.IsBasemgrRunning') {
        req.response.write(jsonEncode({
          'id': body['id'],
          'result': !killed,
          'error': null,
        }));
      } else if (body['method'] == 'modular_facade.StartBasemgr') {
        expect(
          body['params'],
          isNotNull,
        );
        expect(body['params'], {'session_url': sessionUrl});
        killed = false;
        req.response.write(
            jsonEncode({'id': body['id'], 'result': 'Success', 'error': null}));
      } else {
        expect(body['method'], 'modular_facade.KillBasemgr');
        expect(body['params'], anyOf(isNull, isEmpty));
        killed = true;
        req.response.write(
            jsonEncode({'id': body['id'], 'result': 'Success', 'error': null}));
      }
      await req.response.close();
    }

    fakeServer.listen(handler);

    final modular = Modular(sl4f);
    await modular.boot(sessionUrl: sessionUrl);

    expect(modular.controlsTheSession, isTrue,
        reason: 'controlsTheSession after boot');

    await modular.shutdown();

    expect(modular.controlsTheSession, isFalse,
        reason: 'still controlsTheSession after shutdown');
    expect(killed, isTrue, reason: 'did not call KillBasemgr');
  });
}
