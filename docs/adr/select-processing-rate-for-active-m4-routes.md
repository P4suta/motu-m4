---
date: 2026-11-10
---

# Select the processing rate for active M4 routes

## Status

Proposed.

## Context

The owner prioritizes audio quality and requests 192 kHz operation.
The M4 supports 192 kHz with 24 valid bits.

## Decision

Default recording to 192000 Hz and keep the input file's rate for native playback.
Accept an explicit desktop processing rate, defaulting to 192000 Hz.
Use [node.force-rate](https://docs.pipewire.org/page_man_pipewire-props_7.html) on active M4 endpoints and set loopback resampler quality to 14.
Preserve explicit rate overrides and existing configuration ownership checks.

## Alternatives

The existing 48000 Hz defaults use fewer resources.
A global forced clock also affects idle sessions.
Resampling every playback file would add conversion to native playback.

## Consequences

Compared with 48000 Hz, native sample traffic quadruples.
Active desktop routes can resample other applications sharing their graph.
A higher processing rate doesn't establish an audible improvement.
Reconsider the defaults if measured overruns prevent reliable use.

## Confirmation

Require default-rate executable tests, explicit-rate overrides, active-graph inspection, and short 192000 Hz hardware checks.
