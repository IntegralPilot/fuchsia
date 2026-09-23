// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// TODO(https://fxbug.dev/42165807): Fix null safety and remove this language version.
// @dart=2.9

import 'package:sl4f/sl4f.dart' as sl4f;
import 'package:test/test.dart';

const _timeout = Duration(seconds: 60);

void main() {
  sl4f.Sl4f sl4fDriver;

  setUp(() async {
    sl4fDriver = sl4f.Sl4f.fromEnvironment();
    await sl4fDriver.startServer();
  });

  tearDown(() async {
    await sl4fDriver.stopServer();
    sl4fDriver.close();
  });

  group(sl4f.Sl4f, () {
    test('Component search component not running', () async {
      final result = await sl4f.Component(sl4fDriver).search('does_not_exist');
      expect(result, false);
    });

    test('Component search a running component', () async {
      final result = await sl4f.Component(sl4fDriver).search('core/sl4f');
      expect(result, true);
    });

    test('Component List running components', () async {
      final result = await sl4f.Component(sl4fDriver).list();
      expect(result.isNotEmpty, true);
    });

    test('tests launcher with error', () async {
      expect(
          sl4f.Component(sl4fDriver)
              .launch('fuchsia-pkg://fuchsia.com/fake#meta/fake.cm'),
          throwsException);
    });

    test('test launch component and wait for stop', () async {
      expect(
          await sl4f.Component(sl4fDriver).launch(
              'fuchsia-pkg://fuchsia.com/sl4f-testing#meta/self-stop-component.cm'),
          'Success');
      final alive =
          await sl4f.Component(sl4fDriver).search('self-stop-component.cm');
      expect(alive, false);
    });

    test('test launch component and keep component alive', () async {
      // destroy the child component at the begin and the end to ensure it is clean.
      await sl4fDriver.ssh.run('component destroy "daemon-component"');
      expect(
          await sl4f.Component(sl4fDriver).launchAndDetach(
              'fuchsia-pkg://fuchsia.com/sl4f-testing#meta/daemon-component.cm'),
          'Success');
      var alive =
          await sl4f.Component(sl4fDriver).search('daemon-component.cm');
      expect(alive, true);
      await sl4fDriver.ssh.run('component destroy "daemon-component"');
      alive = await sl4f.Component(sl4fDriver).search('daemon-component.cm');
      expect(alive, false);
    });
  }, timeout: Timeout(_timeout));
}
