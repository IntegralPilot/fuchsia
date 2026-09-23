#!/bin/sh
#
# Copyright 2018 The Fuchsia Authors
#
# Use of this source code is governed by a MIT-style
# license that can be found in the LICENSE file or at
# https://opensource.org/licenses/MIT

# This script is run within the chroot of the created Debian distribution.

export DEBIAN_FRONTEND=noninteractive DEBCONF_NONINTERACTIVE_SEEN=true
export LC_ALL=C LANGUAGE=C LANG=C

# Abort on error.
set -e

# Create default account.
username="root"
default_password="password"
home=/root
default_shell=/usr/bin/dash
echo "${username}:${default_password}" | chpasswd
echo "Default login/password is ${username}:${default_password}" > /etc/issue
chsh -s ${default_shell} ${username}

# Squelch MOTD.
touch ${home}/.hushlogin

cat > ${home}/.profile << EOF
# Make the prompt as simple as possible (useful for testing).
export PS1='$ '

# Dash has a builtin 'echo' that is not very capable. This function will be
# higher precedence than the builtin and will redirect to the binary version
# of echo.
echo() {
  env echo \$*
}
EOF

# Setup hostname.
echo "machina-guest" > /etc/hostname
echo "127.0.1.1    machina-guest" >> /etc/hosts

# Prevent certain known-bad modules from loading.
#
# https://fuchsia.dev/fuchsia-src/contribute/respectful_code note:
# "blacklist" is a keyword required by modprobe.
cat > /etc/modprobe.d/machina.conf << EOF
# Causes a panic on certain x86-64 CPUs: <https://fxbug.dev/42167352>.
blacklist intel_pmc_core
EOF

# To shrink the image size, we move all needed modules into the initrd and then
# remove modules from the root filesystem.
#
# All modules that are needed _must_ be listed here.
cat >> /etc/initramfs-tools/modules << EOF
rfkill
intel_rapl_msr
ghash_clmulni_intel
aesni_intel
vmw_vsock_virtio_transport
romfs
evdev
virtio_balloon
pcspkr
button
fuse
f2fs
configfs
virtio_rng
ip_tables
autofs4
ext4
crc32c_generic
virtio_input
virtio_gpu
virtio_net
virtio_console
virtio_blk
crct10dif_pclmul
crc32_pclmul
virtio_pci
crc32c_intel
overlay
uinput
input-core
loop
tun
tap
nls_utf8
nls_base
unicode
nls_ascii
dm_mod
EOF

# Explicitly disable resume from swap. This is to ensure we never try to to
# wait for a resume device before finishing booting since we don't use a
# swap partition anyways.
echo "RESUME=none" > /etc/initramfs-tools/conf.d/resume

# Reduce the number of modules in initramfs to make it smaller.
#
# By default `modules=most` is used here, which pulls in a large number of
# modules for things like USB, filesystems, networking, etc. We have
# enuemrated the list of modules we want to keep and this tells
# update-initramfs to include _only_ those modules.
echo "MODULES=list" > /etc/initramfs-tools/conf.d/modules

update-initramfs -u

# Enable automatic login for serial getty, most importantly on hvc0. This
# overrides the default configuration at
# /lib/systemd/system/serial-getty@.service. The first ExecStart line is to
# reset in the case where the default configuration is set up to append
# ExecStart lines. Note: We use `--skip-login --login-options "-f ${username}"`
# instead of `--autologin` to make agetty quieter.
mkdir -p /etc/systemd/system/serial-getty@.service.d
cat >> /etc/systemd/system/serial-getty@.service.d/override.conf << EOF
[Service]
ExecStart=
ExecStart=-/sbin/agetty --skip-login --login-options "-f ${username}" --noissue --noclear %I xterm
EOF

# Expose a simple telnet interface over vsock port 23.
#
# Note we're using socat to bind the pty to the socket so that we ensure we
# don't send any telnet control messages.
cat >> /etc/systemd/system/telnet.socket << EOF
[Unit]
Description=Telnet Server Activation Port

[Socket]
ListenStream=vsock::23
Accept=true

[Install]
WantedBy=sockets.target
EOF

cat >> /etc/systemd/system/telnet@.service << EOF
[Unit]
Description=Telnet Server
After=local-fs.target

[Service]
ExecStart=-/usr/bin/socat - EXEC:/bin/login,pty,stderr,setsid,sigint,sane,ctty
StandardInput=socket
StandardOutput=socket
EOF

# Expose the guest interaction daemon to allow automated tests to send files to
# and receive files from the guest and exec commands.
cat >> /etc/systemd/system/guest_interaction_daemon.service << EOF
[Unit]
Description=Guest Interaction Daemon

[Service]
Type=simple
ExecStart=-/guest_interaction/guest_interaction_daemon

[Install]
WantedBy=multi-user.target
EOF

systemctl enable telnet.socket
systemctl enable guest_interaction_daemon.service

# Disable some long-running startup services to allow the guest to boot more
# quickly.
#
# Long-running modules can be found with:
#
#   $ systemctl is-system-running --wait
#   $ systemd-analyze blame
systemctl disable NetworkManager-wait-online.service
systemctl disable isc-dhcp-server.service

# Mount the test utils and guest interaction daemon on start up.
cat >> /etc/fstab << EOF
/dev/vdb /test_utils auto ro 0 0
/dev/vdc /guest_interaction romfs ro 0 0
EOF

# Remove apt-list files (~75MiB).
rm -r /var/cache/apt
rm -r /var/lib/apt/lists/*
# Remove locale files (~44MiB)
rm -rf /usr/share/locale/*
# Remove docs (~22MiB)
rm -rf /usr/share/doc/*
# Remove manpages (~9MiB)
rm -rf /usr/share/man/*

# We have included all the modules we need into the initramfs, so we remove all
# modules modules from the root filesystem.
#
# This saves over 300MiB on the x64 build and cuts the disk image size
# approximately in half.
rm -rf /lib/modules/*

# Remove files cached by apt.
apt clean
