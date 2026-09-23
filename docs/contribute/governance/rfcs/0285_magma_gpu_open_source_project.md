<!-- Generated with `fx rfc` -->
<!-- mdformat off(templates not supported) -->
{% set rfcid = "RFC-0285" %}
{% include "docs/contribute/governance/rfcs/_common/_rfc_header.md" %}
# {{ rfc.name }}: {{ rfc.title }}
{# Fuchsia RFCs use templates to display various fields from _rfcs.yaml. View the #}
{# fully rendered RFCs at https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs #}
<!-- SET the `rfcid` VAR ABOVE. DO NOT EDIT ANYTHING ELSE ABOVE THIS LINE. -->

<!-- mdformat on -->

## Summary

Building on [RFC #198][rfc-198], this RFC proposes the creation of the Magma-GPU
exploratory committee. The goal of the committee would be to advance Magma as a
multi-OS GPU standard that bridges the high level[^1] and low level parts[^2] of
the GPU driver stack.

## Motivation

For background and definitions, readers are encouraged to review the appendices to
understand [GPU complexity](#appendix-i) and industry
trends
[towards open-source](#appendix-ii).

Magma is [Fuchsia's GPU driver model][magma-driver-model]. Magma acknowledges
the significant scope for differentiation in UMD and system driver designs, but
aims for more standardization in the cross-driver interface itself. This is useful
outside Fuchsia - the Android GPU virtualization effort proposed
[upstreaming Magma to Mesa][magma-mesa-upstream] to solve virtio fragmentation
issues (a design known as _magmavirt_).

While the upstream merge request remains a work in progress, the discussion
around it revealed gaps in Mesa's support for microkernel-like systems in
general.

| OS           | Mesa Status                           | Open-Source System driver                           |
| ------------ | ------------------------------------- | --------------------------------------------------- |
| **Fuchsia**  | [Mesa forks][mesa-fuchsia]            | [Fuchsia Git][fuchsia-intel-msd-git]                |
| **HaikuOS**  | Software rendering supported in-tree  | A NVK port [in progress][nvidia-haiku]              |
| **QNX**      | Maintains [Mesa forks][qnx-mesa]      | Closed system drivers                               |
| **Redox OS** | [Mesa forks for LLVMpipe][redox-mesa] | [Work started on Intel GPU driver][redox-intel-gpu] |

Each microkernel project creating its own interface and system drivers is
inefficient for both maintainers and microkernel developers.

Hardware vendors are unlikely to support system drivers for Fuchsia, QNX, Redox,
and Haiku. Most current microkernel system GPU drivers are written in C/C++,
while Linux DRM is already [transitioning to Rust][rust-drm].

To solve these issues, this RFC proposes to advance Magma as an industry-wide,
open-source microkernel GPU standard.

## Stakeholders

_Facilitator:_

- <drewry@google.com>

_Reviewers:_

- <cstout@google.com>
- <msandy@google.com>
- <dgilhooley@google.com>

## Goals

- Creation of the Magma GPU exploratory committee
- Regular quarterly meetings of the Magma GPU exploratory committee
- Landing Magma in at least one Mesa Vulkan driver
- Aligning on a set of common interfaces
- Potentially sharing system driver parts amongst Magma users

### Non-goals

- One-size-fits-all for everything
- Unifying Magma-in-Mesa workstreams with closed-source workstreams

## Design

The **Magma-GPU exploratory committee** will be created to foster collaboration
amongst Fuchsia Graphics, Android GPU virtualization and non-Fuchsia microkernel
developers.

Given the limited resources of microkernel projects, the committee will meet
quarterly via video chat for one hour and target a Mesa-level standard.

We will follow a [consensus-driven approach][consensus-decision-making], similar
to that of a [Khronos exploratory group](#appendix-iii).

Much of the work of GPU drivers can be shared, if architected correctly.

| Component                       | Location      | Primary Function                                                                 | Estimated Reuse |
| ------------------------------- | ------------- | -------------------------------------------------------------------------------- | --------------- |
| **User-Space Client libraries** | Mesa          | API translation, shader compilation to IR/machine code, state tracking.          | 90% - 95%       |
| **Hardware Core Logic**         | System Driver | Command buffer formatting, device register definitions, execution pipelines.     | 70% - 90%       |
| **Power Management**            | System Driver | Clock gating, voltage scaling, thermal limits, suspend/resume handling.          | 50% - 70%       |
| **Memory Management**           | System Driver | Allocating, pinning, and mapping physical memory for Direct Memory Access (DMA). | 0%              |
| **Hardware Discovery & IRQs**   | System Driver | Binding to the PCI bus, handling hardware interrupts.                            | 0%              |
| **Total Stack Average**         | Overall       | Fully-functional GPU driver.                                                     | 70% - 80%       |

The job of the committee is to translate the possibilities into actual code.

### Magma GPU API and protocol design

A clean protocol implies a clean implementation. Fuchsia's current Magma
[library][magma-now-lib] and [protocol][magma-now-protocol] are both vendor and
OS neutral and will serve as the basis for the committee to build upon. Both the
library and protocol will be versioned so consumers will have reliability
guarantees.

### Innovation

There are many areas where microkernel designs can innovate compared to the
status quo. These areas include -- but are not limited to -- GPU resets,
userspace command submission, and virtualization.

Innovating, while maintaining software compatibility for existing applications,
is an interesting technical challenge.

### Design of cross-platform Magma System Drivers (MSD)

Current implementations assume one system driver per GPU vendor. For example,
the [Haiku RadeonGfx][haiku-radeongfx] is different from the
[Haiku Nvidia driver][nvidia-haiku]. The
[Fuchsia Intel MSD][fuchsia-intel-msd-git] is a different binary from the
[Fuchsia ARM MSD][fuchsia-arm-msd-git], but both leverage the
[sys_driver][magma-now-sys-driver] library for common code.

To avoid the [mid-layer mistake](https://lwn.net/Articles/336262/) while
maximizing code reuse, a high-level design could be a set of composable Rust
crates that a specific OS can leverage to build their own MSD.

<img src="resources/0285_magma_gpu_open_source_project/msd_design.svg" width="50%" alt="Possible MSD design">

The committee will decide on the exact details.

### Source code location

Microkernel drivers are scattered throughout Fuchsia Git, RedoxOS GitLab, and
GitHub. For an industry-wide collaboration, we need to meet the following
requirements:

- each Magma GPU component (the client library, protocol, and system driver)
  needs to be easy to modify for interested developers
- common code cannot depend on OS-specific functionality

We proposed a new [GitLab][freedesktop-gitlab] location on freedesktop last
year, but ultimately settled on the [magma-gpu project][magma-gpu-github] GitHub
organization. There are a few options going forward:

1. It may be desirable that the system drivers live in Mesa itself. For example,
   in a structure like:

   - `src/magma-gpu/lib` (protocol definitions, client lib)
   - `src/magma-gpu/bin` (MSD server binaries)
   - `src/magma-gpu/ffi` (FFI bindings to C Mesa drivers)

   Vendors are already on Mesa; we will not need another location for them to
   submit code. There could be drawbacks to updating the UMD + system driver at
   the same time.

1. Fallback to the already-created GitHub organization.

### Copyright

The Magma GPU project adheres to the highest ethical and legal standards. Mesa
is already MIT-based and the Magma client library will be MIT-based. The common
code for the Magma System Drivers (MSD) binary will be MIT-licensed.

It _may_ be possible to tactically reference GPL-2.0 licensed Linux DRM code in
MSD code, if the virality of the GPL-2.0 code can be contained. However, that
requires further discussion among the exploratory committee and constituent
projects (and another possible Fuchsia RFC). This RFC leaves the issue
unresolved.

### Staffing and time commitment

We expect high-priority internal efforts to be the focus of the Fuchsia team.
Additional staffing is not requested nor required. This is an early-stage
ecosystem initiative.

## Appendix I: GPU drivers are complex {:#appendix-i}

GPU + NPU driver stacks share a similar topology:

<img src="resources/0285_magma_gpu_open_source_project/anatomy_of_an_accelerator.svg" width="50%" alt="Accelerator design">

The **usermode driver (UMD)** contains

- a compiler that outputs GPU assembly from a domain-specific language (SPIRV,
  CUDA C++, or HLSL)
- an implementation of a standardized API (Vulkan, CUDA, OpenGL, OpenCL)

The UMD submits commands to the **system driver**, which securely mediates
access to the hardware.

Typical UMDs and system drivers range from 100 to 500 kLOC each. The AMD Linux
kernel driver is [6 million lines][amd-6m-lines], with 1.5M lines of logic.

Beneath the system driver, modern GPU drivers rely on firmware blobs (also up to
1M+ LoC). These blobs handle power management, scheduling, and thermal
throttling.

Both Apple's [M1 GPU][m1-gpu-tales] and Nvidia's
[GSP co-processor][nvidia-riscv-cores] run embedded RTOSes, while ARM's new
[Mali CSF design][mali-gpu-csf] features advanced firmware. Professor Timothy
Roscoe argues these co-processors and blobs require a rethink of
[operating system design][roscoe-os-design].

## Appendix II: Open-source GPU drivers are an irresistible trend {:#appendix-ii}

A complex interplay among hobbyists, OS developers, and hardware vendors has led
to open-source becoming the best way to develop GPU drivers. This is most
acutely visible in the Linux ecosystem.

### Mesa: the side-project that started it all (1993 → 1997)

[Mesa][mesa-repo] is where dozens of open-source GPU UMDs are located. Mesa was
first [started in 1993][mesa-history] by Silicon Graphics developer Brian Paul.

As a side-project, Brian created a software implementation of OpenGL 1.0 and
released it to [the internet][dri-intro]. It quickly gained popularity.

In 1997, the HW-accelerated Mesa Glide driver was added, but it used a
closed-source system driver.

### Linux system drivers created by consultancies (1999 → 2004)

Precision Insight, a consulting firm contracted by Red Hat and Intel, led
development of the Linux [Direct Rendering Manager (DRM)][drm-low-level] kernel
module, developed for the 3Dlabs' GMX2000 GPU and Intel's i810 driver. This
serves as the basis for the infrastructure
[still in use today][linux-gpu-intro].

VALinux added the [Radeon DRM driver][radeon-drm-driver] shortly thereafter.

Tungsten graphics, a spinoff of Precision Insights, added the
[i915 Intel kernel module][i915-intel-module].

### Open-source graphics becomes a thing (2004 → 2010)

Intel's Open-Source Technology Center was formed in the mid-2000s, and they
became the biggest drivers of structural changes to
[Linux DRM and Mesa][intel-otc-graphics].

AMD started contributing to the Radeon driver. A French grad student
reverse-engineered the Nvidia driver and called it [nouveau][nouveau-lwn].

### Line in the sand (2010)

With the dawn of Android, several mobile vendors (Qualcomm, Imagination, ARM)
tried to upstream their Linux system drivers without open-sourcing their UMDs.
Linux kernel GPU maintainer Dave Airlie
[drew a line in the sand][airlie-line-in-sand] in 2010, requiring open-source
UMDs before the system driver could be merged into the Linux kernel. This was
codified [into the requirements][drm-uapi-requirements]:

> The short summary is that any addition of DRM uAPI requires corresponding
> open-sourced userspace patches, and those patches must be reviewed and ready
> for merging into a suitable and canonical upstream project.

### Linux desktop and hobbyists lead the way (2011 → 2021)

Mobile vendors did not open-source their UMDs for various reasons:

- GPU driver stacks were considered a value-added component
- legal issues surrounding closed-source UMDs
- time investment required to open-source them

Regardless, open-source GPU drivers were already a thing for Linux desktops.
Valve started investing in [open-source gaming][gabe-newell-open-source], and
ChromeOS' [Freon stack][chromeos-freon] benefited from open-source. Hobbyists
reverse engineered [freedreno][freedreno-phoronix] and
[panfrost][panfrost-phoronix].

ChromeOS shipped [Freedreno Chromebooks][freedreno-chromebooks] with minimal
Qualcomm assistance.

### Mobile vendors, Nvidia, and Android invest in open-source (2021 → present)

The quality and maintenance benefits of open-source GPU drivers became hard to
ignore. [ARM][arm-tyr-rust-driver] and [Imagination][imagination-open-source]
both started investing in Mesa and Linux DRM.

Qualcomm hired the [freedreno maintainer][qualcomm-hires-maintainer], and Nvidia
has started investing in [Linux DRM][nova-core-drm] (but not Mesa).

Freedreno is widely used for [Android gaming][adreno-740-turnip-guide] and the
[Steam Frame][steam-frame-turnip] will leverage it.

Android intends to update [Mesa drivers consistently][android-mesa-updates] for
the first time ever.

## Appendix III: Khronos {:#appendix-iii}

The [Khronos Group][khronos-group] is an industry body that creates open
standards for 3D graphics, machine learning, and AR/VR. Many industry standards
(Vulkan, OpenGL, OpenXR, OpenCL, glTF) have been created via the Khronos
process.

Before a standard is born, it progresses through a well-known pipeline:

- Stage I: An easy-to-form and essentially zero-cost **exploratory group**
- Stage II: Approval from Khronos board of directors to form a **working group**
- Stage III: Draft a **formal specification**
- Stage IV: Ratify the specification and write a **conformance suite**

Further amendments follow a formal
[voting process][khronos-working-group-guidelines].

[^1]: Vulkan, OpenGL, OpenCL, etc.

[^2]: Register access, memory management, power, etc.

[adreno-740-turnip-guide]: https://pocket-gaming.org/2026/02/13/level-up-your-android-emulation-the-adreno-740-turnip-driver-guide/
[airlie-line-in-sand]: https://lwn.net/Articles/394702/
[amd-6m-lines]: https://www.phoronix.com/news/AMD-Six-Million-Lines-Linux-7.0
[android-mesa-updates]: https://gitlab.freedesktop.org/mesa/mesa/-/merge_requests/41518
[arm-tyr-rust-driver]: https://www.collabora.com/news-and-blog/news-and-events/introducing-tyr-a-new-rust-drm-driver.html
[chromeos-freon]: https://tech.slashdot.org/story/15/03/09/0220205/google-introduces-freon-a-replacement-for-x11-on-chrome-os
[consensus-decision-making]: https://en.wikipedia.org/wiki/Consensus_decision-making
[dri-intro]: https://www.plunk.org/opengl/dri_intro.pdf
[drm-low-level]: https://web.archive.org/web/20240521055530/https://dri.sourceforge.net/doc/drm_low_level.html
[drm-uapi-requirements]: https://dri.freedesktop.org/docs/drm/gpu/drm-uapi.html#open-source-userspace-requirements
[freedesktop-gitlab]: https://gitlab.freedesktop.org/freedesktop/freedesktop/-/work_items/2459#note_3044121
[freedreno-chromebooks]: https://www.phoronix.com/news/Chromebooks-Mesa-QLCM
[freedreno-phoronix]: https://www.phoronix.com/news/MTI0Mjg
[fuchsia-arm-msd-git]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/graphics/drivers/msd-arm-mali/
[fuchsia-intel-msd-git]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/graphics/drivers/msd-intel-gen/
[gabe-newell-open-source]: https://www.pcgamer.com/gabe-newell-linux-and-open-source-are-the-future-of-gaming/
[haiku-radeongfx]: https://github.com/X547/RadeonGfx
[i915-intel-module]: https://lkml.iu.edu/hypermail/linux/kernel/0408.1/2238.html
[imagination-open-source]: https://blog.imaginationtech.com/imagination-and-our-commitment-to-open-source
[intel-otc-graphics]: https://www.phoronix.com/news/NjkwNw
[khronos-group]: https://www.khronos.org/
[khronos-working-group-guidelines]: https://www.khronos.org/files/khronos-group-operational-guidelines.pdf
[linux-gpu-intro]: https://docs.kernel.org/gpu/introduction.html
[m1-gpu-tales]: https://asahilinux.org/2022/11/tales-of-the-m1-gpu/
[magma-driver-model]: /docs/development/graphics/magma/README.md
[magma-gpu-github]: https://github.com/magma-gpu
[magma-mesa-upstream]: https://gitlab.freedesktop.org/mesa/mesa/-/merge_requests/33190
[magma-now-lib]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/lib/magma_client/include/lib/magma/magma.h
[magma-now-protocol]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/fidl/fuchsia.gpu.magma/magma.fidl
[magma-now-sys-driver]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/graphics/magma/lib/magma_service/sys_driver/
[mali-gpu-csf]: https://developer.arm.com/community/arm-community-blogs/b/mobile-graphics-and-gaming-blog/posts/mali-g710-developer-overview
[mesa-fuchsia]: https://fuchsia.googlesource.com/third_party/mesa/
[mesa-history]: https://docs.mesa.org/history.html
[mesa-repo]: https://gitlab.freedesktop.org/mesa/mesa
[nouveau-lwn]: https://lwn.net/Articles/269558/
[nova-core-drm]: https://www.phoronix.com/news/NOVA-Core-Co-Maintainer
[nvidia-haiku]: https://github.com/X547/nvidia-haiku
[nvidia-riscv-cores]: https://riscv.org/blog/how-nvidia-shipped-one-billion-risc-v-cores-in-2024/
[panfrost-phoronix]: https://www.phoronix.com/news/Panfrost-Mali-October-2018
[qnx-mesa]: https://lists.freedesktop.org/archives/mesa-dev/2026-March/226603.html
[qualcomm-hires-maintainer]: https://www.linkedin.com/posts/avinash-seetharamaiah-842a115_excited-to-share-that-rob-clark-will-be-joining-activity-7330988041773158400-_p0D
[radeon-drm-driver]: https://sources.debian.org/src/linux/6.12.86-1/drivers/gpu/drm/radeon/radeon_drv.c
[redox-intel-gpu]: https://www.phoronix.com/news/Redox-OS-Own-Intel-GPU-Driver
[redox-mesa]: https://gitlab.redox-os.org/redox-os/mesa
[rfc-198]: /docs/contribute/governance/rfcs/0198_magma_api_design.md
[roscoe-os-design]: https://www.youtube.com/watch?v=36myc8wQhLo
[rust-drm]: https://kernel-recipes.org/en/2025/schedule/a-rusty-odyssey-a-timeline-of-rust-in-the-drm-subsystem/
[steam-frame-turnip]: https://www.phoronix.com/news/Steam-Frame-Turnip-Vulkan
