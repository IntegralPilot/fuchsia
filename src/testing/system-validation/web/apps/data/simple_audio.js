// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

let html = `<!doctype html>
<html>
<head>
  <title>Audio MP3</title>
</head>
<body style="height:100%">
	<audio id="audio-player" src="http://127.0.0.1:81/mp3" type="audio/mpeg" controls autoplay loop></audio>
</body>
</html>`;

document.write(html);
