# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

FROM alpine@sha256:c5b1261d6d3e43071626931fc004f70149baeba2c8ec672bd4f27761f8e1ad6b

RUN apk add vim bash less openssh sshpass && \
    rm -rf /var/cache/apk/*

RUN mkdir -p /root/.ssh \
    && chmod 0700 /root/.ssh

RUN ssh-keygen -A

COPY sshd_config /etc/ssh/sshd_config

RUN echo "root:root" | chpasswd

CMD ["bash"]
