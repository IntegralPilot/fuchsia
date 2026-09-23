// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// @dart=2.12

import 'sl4f_client.dart';

/// Interact with the Components of the system.
class Component {
  final Sl4f _sl4f;

  Component(this._sl4f);

  /// List currently running components in the device.
  Future<List<String>> list() async {
    final List<dynamic> result = await _sl4f.request('component_facade.List');
    return result.cast<String>();
  }

  /// Returns true if the component [name] is currently running.
  ///
  /// [name] is the exact name of the component, like 'component.cm'.
  Future<bool> search(String name) async {
    final result =
        await _sl4f.request('component_facade.Search', {'name': name});
    return result == 'Success';
  }

  /// Launches a component given by [url], and wait for the component to stop.
  Future<dynamic> launch(String url, [List<String>? args]) async {
    if (args != null && args.isNotEmpty) {
      return await _sl4f.request('component_facade.Launch',
          {'url': url, 'arguments': args, 'wait_until_stop': true});
    }
    return await _sl4f.request(
        'component_facade.Launch', {'url': url, 'wait_until_stop': true});
  }

  /// Launches a component given by [url], and detach the component.
  Future<dynamic> launchAndDetach(String url, [List<String>? args]) async {
    if (args != null && args.isNotEmpty) {
      return await _sl4f.request('component_facade.Launch',
          {'url': url, 'arguments': args, 'wait_until_stop': false});
    }
    return await _sl4f.request(
        'component_facade.Launch', {'url': url, 'wait_until_stop': false});
  }
}
