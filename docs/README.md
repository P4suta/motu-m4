# Development

The app supports native Linux audio, desktop channel routing, and offline USB protocol verification.
The [Linux audio guide](linux-audio.md) covers real recording, playback, duplex, and desktop setup.
The supported sample rates are 44100, 48000, 88200, 96000, 176400, and 192000 Hz.
Samples use little-endian 32-bit slots with 24 valid bits.
Playback clears the unused low eight bits of each sample before transmission.
Native audio uses the installed Linux driver through `ALSA`.
Simulation and firmware execution remain isolated from physical devices.

## Commands

Run `mise x -- just check` for formatting, strict Clippy, unit tests, and executable end-to-end tests.
The same command checks workflow security, RustSec advisories, and secrets.
The `mise.toml` file pins the security tools.
RustSec checks use the database fetched at execution time.
Use `cargo audit --no-fetch` with a preserved database checkout to repeat an advisory result.
Run `mise x -- just probe` to read cached Linux sysfs descriptors without taking device ownership.
The [firmware guide](firmware.md) explains reproducible inspection of the official updater archives.
The [execution guide](firmware-execution.md) covers real firmware control handlers, modeled peripherals, and offline integration tests.
Run `simulate capture --help`, `simulate play --help`, or `simulate duplex --help` for command options.
The `--rate` option defaults to 48000 Hz.
The `--profile` option selects `recorded`, `atmel-template`, `nxp-template`, or `original-synthetic` descriptor and timing models.
The `simulate` commands use interleaved signed samples without a container header.
Each frame contains four little-endian 32-bit slots, with the low eight bits unused.
Simulated capture admits at most 480000 frames, and playback inputs must contain complete frames within the same limit.
Duplex captures exactly as many frames as its playback input contains.

The `steady` scenario follows an exact fractional sample clock.
Each packet advances one virtual 125-microsecond interval.
The `drift` and `negative-drift` scenarios add or remove one frame per 2048 intervals, with initial packet jitter.
The `intermittent` scenario inserts an empty packet every 17 intervals.
Fault scenarios include `disconnect`, `malformed`, `stalled`, and `timeout`.
Clock faults include `short-control`, `unsupported-rate`, `malformed-ranges`, `invalid-clock`, and `clock-mismatch`.
Scenarios stop after 100000 packet requests, and the stalled scenario returns a timeout after 32 empty packets.
Use `--cancel-after-packets N` to cancel deterministically after N completed input intervals, including empty packets.
Cancellation at zero takes no device ownership.
The `--received PATH` option saves simulated playback packets, including final zero padding.
Outputs refuse existing files and use private file permissions on Unix.
A failed multi-file output removes newly created files and reports removal failures.

## Session contract

The engine validates the descriptor topology and endpoint capacity before taking ownership.
It reads the original clock, validates the returned frequency ranges, and admits only restorable sample rates.
Changed clocks must read back exactly and pass their advertised validity check before streaming starts.
An unchanged clock still passes the advertised validity check but requires no setting or restoration write.
Capture packet lengths provide implicit playback feedback.
The engine retains capture data during duplex playback and zero-pads the last output packet.
Every exit attempts acquired cleanup steps in reverse order and preserves the primary error with recovery failures.
Restoration includes reading back the original clock.
The simulator validates control request bytes independently of the production request builders.
It also refuses premature control transfers and interface starts using independent virtual timing rules.

`run_stream` exposes capture, playback, and duplex through `StreamRequest` with a validated `SampleRate`.
Its cancellation callback receives the number of completed capture packets.
Transport implementations must return from each operation within a finite deadline.
Their elapsed clock measures time since the transport first observed the device, and waits must advance time by at least the requested duration.
The library reports typed operations, transport failures, protocol failures, and recovery failures.

## Protocol sources

The [USB Audio specification](https://www.usb.org/sites/default/files/Audio2_with_Errata_and_ECN_through_Sep_14_2026.pdf) defines the interface descriptors in section 4.9, endpoint descriptors in section 4.10, and sampling clock requests in section 5.2.5.1.
The [Audio Data Formats specification](https://www.usb.org/sites/default/files/Audio2.0_final.zip), section 2.3.1.7.1 of `Frmts20 final.pdf`, defines signed samples, left justification, and zero padding.
The [official Japanese distributor's M4 specifications](https://h-resolution.com/product/m4/) list all six sample rates.
The descriptor fixture records the attached M4's device and configuration descriptors from Linux sysfs on November 10, 2026.
Its USB identifiers are `07fd:000b`, with firmware descriptor `2.04`, playback endpoint `0x02`, and capture endpoint `0x83`.
The fixture contains string descriptor indices.
It excludes string contents and device serial numbers.
The parser admits one configuration, four channels, 32-bit slots, 24 valid bits, and 125-microsecond endpoint intervals.
It validates the fixture's USB and external terminal paths, unique entity identifiers, and shared clock selector.
Unsupported stream formats and endpoint layouts fail validation.
Clock range responses admit at most 32 ordered, nonoverlapping, positive frequency ranges.
Each range contains either one discrete frequency or positive-resolution steps.

## Verification

`just proof` uses `Kani 0.68.0` with `CBMC 6.11.0`.
`just mutation` uses `cargo-mutants 27.1.0`.

Run these commands in a bounded container on owner machines, following the machine's verification policy.
Project-owned Rust forbids unsafe code.
The core has no runtime dependencies or standard library.
Mutation testing covers production Rust files.
The configuration excludes `proofs.rs` because its harnesses run through Kani and aren't executable in ordinary tests.

Kani checks all scalar inputs for identity selection, packet bounds, frame limits, and descriptor-length progress.
It checks every recovery stage with an arbitrary previous clock value.
The timing proof checks both generation policies.
Firmware proofs cover payload-length arithmetic and arbitrary 60-byte version fields.
The descriptor span proof covers arbitrary offsets and lengths within a 32-byte payload.
Behavioral tests cover the complete container parser and descriptor candidate scanning.
Clock proofs cover all sample-rate inputs, range-count bounds, and range-offset arithmetic.
Range decoder proofs cover arbitrary 26-byte responses and exact membership in a concrete discrete-and-stepped response.
They don't establish membership correctness for every possible 32-bit range and resolution combination.
Parser proofs cover rejection of lengths below 128 bytes for every input size.
Decoder proofs cover arbitrary 127-byte fragments, single descriptors up to 32 bytes, and the recorded 393-byte fixture.
Those bounded parser proofs don't establish acceptance rules for every possible descriptor chain.
Behavioral tests cover descriptor rejection, independent audio samples, implicit feedback, cancellation, disconnection, and recovery failures.
The simulator checks clock requests and packet lengths without calling production packet validators.

The simulator's end-to-end tests run the executable and temporary files through the simulated transport.
They establish software behavior within the simulator's model.
Physical timing, analog behavior, device firmware quirks, and hardware recovery require a later validation stage.
The [simulation decision](adr/verify-usb-audio-with-a-simulated-device.md) records the architecture and its limits.
Native audio tests cover the system library and private audio-server sessions, as described in the Linux audio guide.
