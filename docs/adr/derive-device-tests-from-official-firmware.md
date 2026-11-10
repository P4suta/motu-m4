---
date: 2026-11-10
---

# Derive device tests from official firmware

## Status

Proposed.

## Context

One recorded USB layout can't expose assumptions about other M4 firmware variants.
The [official updater](https://motu.com/en-us/download-center/download/3013) contains independently inspectable descriptors.
[Linux](https://github.com/torvalds/linux/blob/7475ea8b58616b33c04afb976cf0fbaad9dd63fb/sound/usb/quirks.c) also requires generation-specific delays.

## Decision

Use a Rust offline inspector and checksum-pinned archive verification.
Compare firmware bytes across macOS and Windows distributions.
Store functional USB descriptors as regression fixtures and keep vendor executables outside the repository.
Run the transport engine with both M4 layouts and an independently timed simulator.
Use virtual elapsed time for deterministic initialization and recovery tests.

## Consequences

Static inspection grounds descriptor assumptions but doesn't execute firmware instructions or emulate peripherals.
Archive verification needs downloaded vendor files.
Reconsider instruction-level emulation when peripheral models and an executable coverage target are available.

## Alternatives

A full microcontroller emulator could expose firmware state transitions, but an incomplete peripheral model would supply misleading results.
The recorded-device simulator alone misses firmware layout changes.

## Confirmation

Require malformed-input tests, cross-distribution byte comparison, both-layout streaming tests, timing refusal tests, and affected protocol proofs.
