---
date: 2026-11-10
---

# Use native Linux audio for desktop applications

## Status

Proposed.

## Context

Daily applications need real audio transfers and usable channel routing.
Protocol simulation doesn't connect desktop applications.

## Decision

Use the kernel's USB audio driver through the [Advanced Linux Sound Architecture stream interface](https://www.alsa-project.org/alsa-doc/alsa-lib/pcm.html).
Keep file conversion and transfer recovery behind typed ports with bounded waits.
Expose stereo output pairs and microphone inputs through [PipeWire loopbacks](https://docs.pipewire.org/page_module_loopback.html).
Discover actual node names and channel positions before generating removable user configuration.
Close targeted streams after device removal because WirePlumber 0.4 lacks fallback suppression.
Preserve simulator and firmware tests as independent protocol checks.

## Alternatives

A userspace USB driver duplicates kernel scheduling and disconnect recovery while competing with desktop audio ownership.
Raw command wrappers alone leave sample conversion and partial-transfer handling outside the tested Rust implementation.

## Consequences

Native audio depends on `libasound` and the existing kernel driver.
Desktop routing needs PipeWire and WirePlumber.
Reconnect the device and restart routing after removal.
Physical latency and analog behavior remain empirical validation requirements.
Reconsider direct USB transfers when a measured driver limitation blocks an intended workflow.

## Confirmation

Require fault-injection tests, real library transfers through the software `null` device, isolated routing tests, and documented physical validation.
