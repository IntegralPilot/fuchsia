// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#include "src/developer/vsock-sshd-host/data_dir.h"

#include <lib/syslog/cpp/macros.h>
#include <lib/zx/vmo.h>
#include <zircon/errors.h>
#include <zircon/status.h>

#include <fbl/ref_ptr.h>

#include "src/lib/files/file.h"
#include "src/storage/lib/vfs/cpp/pseudo_dir.h"
#include "src/storage/lib/vfs/cpp/vmo_file.h"

extern "C" {
// clang-format off
#include "third_party/openssh-portable/authfile.h"
#include "third_party/openssh-portable/includes.h"
#include "third_party/openssh-portable/sshbuf.h"
#include "third_party/openssh-portable/ssherr.h"
#include "third_party/openssh-portable/sshkey.h"
// clang-format on
}

namespace {

fbl::RefPtr<fs::VmoFile> CreateVmoFile(const void* data, size_t len) {
  zx::vmo vmo;
  if (zx_status_t status = zx::vmo::create(len, 0, &vmo); status != ZX_OK) {
    FX_LOGS(FATAL) << "Failed to create vmo: " << zx_status_get_string(status);
  }
  if (zx_status_t status = vmo.write(data, 0, len); status != ZX_OK) {
    FX_LOGS(FATAL) << "Failed to write file contents to vmo: " << zx_status_get_string(status);
  }
  return fbl::MakeRefCounted<fs::VmoFile>(std::move(vmo), len);
}

fbl::RefPtr<fs::VmoFile> CopyAuthorizedKeys() {
  constexpr char kAuthorizeKeysPath[] = "/data/ssh/authorized_keys";

  std::vector<uint8_t> contents;
  FX_CHECK(files::ReadFileToVector(kAuthorizeKeysPath, &contents))
      << "Failed to read authorized_keys file";

  return CreateVmoFile(contents.data(), contents.size());
}

void sshkey_auto_free(struct sshkey** key) { sshkey_free(*key); }
void sshbuf_auto_free(struct sshbuf** key) { sshbuf_free(*key); }

fbl::RefPtr<fs::VmoFile> GenerateHostKeys() {
  constexpr char key_type[] = "ed25519";

  __attribute__((cleanup(sshkey_auto_free))) struct sshkey* private_key = nullptr;
  __attribute__((cleanup(sshkey_auto_free))) struct sshkey* public_key = nullptr;

  int type = sshkey_type_from_name(key_type);

  if (int r = sshkey_generate(type, 0, &private_key); r != 0) {
    FX_LOGS(FATAL) << "sshkey_generate failed: " << ssh_err(r);
  }

  if (int r = sshkey_from_private(private_key, &public_key); r != 0) {
    FX_LOGS(FATAL) << "sshkey_from_private failed: " << ssh_err(r);
  }

  __attribute__((cleanup(sshbuf_auto_free))) struct sshbuf* private_key_blob = sshbuf_new();
  if (private_key_blob == nullptr) {
    FX_LOGS(FATAL) << "sshbuf_new failed";
  }

  if (int r = sshkey_private_to_fileblob(private_key, private_key_blob, "", "", 1, nullptr, 0);
      r != 0) {
    FX_LOGS(FATAL) << "Serializing key failed: " << ssh_err(r);
  }

  return CreateVmoFile(sshbuf_ptr(private_key_blob), sshbuf_len(private_key_blob));
}
}  // namespace

fbl::RefPtr<fs::PseudoDir> BuildDataDir() {
  auto ssh_dir = fbl::MakeRefCounted<fs::PseudoDir>();
  ssh_dir->AddEntry("authorized_key", CopyAuthorizedKeys());
  ssh_dir->AddEntry("ssh_host_ed25519_key", GenerateHostKeys());
  auto data_dir = fbl::MakeRefCounted<fs::PseudoDir>();
  data_dir->AddEntry("ssh", ssh_dir);
  return data_dir;
}
