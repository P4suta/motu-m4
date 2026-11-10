---
date: 2026-11-10
---

# Use Linux usbfs for Rust audio

## Status

Superseded by [Verify USB audio with a simulated device](verify-usb-audio-with-a-simulated-device.md).

## Context

The `MOTU M4` uses four-channel USB Audio with implicit feedback.
Other M Series devices share its USB identifiers.

## Decision

Use the [Linux USB userspace interface](https://docs.kernel.org/driver-api/usb/usb.html#life-cycle-of-user-mode-drivers) for direct transfers.
Separate a dependency-free protocol core from the Linux adapter.
Verify the product name and identifiers before takeover.
Require explicit takeover, preserve the previous clock, and restore driver ownership after streaming.
Start with four channels at 48000 Hz.

## Consequences

The Linux USB host controller remains a dependency.
The [nusb project](https://github.com/kevinmehall/nusb) lacks the required isochronous transport.
A kernel module requires more kernel-facing infrastructure.
Direct streaming requires USB write permission and interrupts existing audio.
Descriptor validation can't establish streaming behavior or hardware recovery.

## Alternatives

Calling the standard audio driver leaves USB streaming outside the Rust implementation.

## Confirmation

Physical streaming and restoration tests remain pending.
