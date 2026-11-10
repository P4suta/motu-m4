---
date: 2026-11-10
---

# Verify audio with a simulated device

## Status

Proposed.

The [native audio proposal](use-alsa-and-pipewire-for-daily-audio.md) adds a separate app adapter while preserving the simulator's offline contract.

## Context

Development must avoid physical hardware writes.
The M4 descriptor fixture excludes serial numbers and audio content.

## Decision

Build the Rust protocol core and a transport port before a hardware streaming adapter.
Validate descriptors and clocks using [USB Audio sections 4.9–5.2](https://www.usb.org/sites/default/files/Audio2_with_Errata_and_ECN_through_Sep_14_2026.pdf).
Keep the simulator's request checks and audio generator independent of production packet decisions.
Exercise capture, playback, duplex, cancellation, and recovery through executable end-to-end tests.
Keep physical discovery read-only and expose no physical write transport.
Follow the [command-line guidelines](https://clig.dev/#the-basics) for help and output.

## Consequences

Software tests need no USB permission.
Fault injection covers disconnection, clock drift, malformed feedback, and recovery errors.
A simulator establishes behavior within its declared protocol model.
Physical timing, analog behavior, and hardware recovery remain outside the simulated contract.
This decision supersedes [direct usbfs audio development](use-linux-usbfs-for-rust-audio.md).

## Alternatives

Physical tests risk interrupting the active device and can't cover every injected fault.

## Confirmation

Require protocol proofs and executable end-to-end tests before completion.
