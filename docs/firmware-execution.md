# Firmware execution

The Rust processor executes USB audio control handlers from the official `atmel.image` and `nxp.image` M4 firmware.
Firmware instructions generate clock range, frequency, selector, and validity responses.
The host engine compares each response with an independent simulated device before streaming.
Audio packets still come from the simulated sample clock.
The `simulate` and `execute-firmware` commands remain isolated from physical USB devices.
The separate [native audio commands](linux-audio.md) use the installed Linux driver.

## Run the verified images

Use the two archives identified in the [firmware guide](firmware.md).
Create a private local corpus and run every firmware integration test:

```sh
mise x -- just firmware-corpus /tmp/motu-mseries-universal-mac.zip /tmp/motu-mseries-universal-windows.zip
mise x -- just firmware-test
```

The exporter checks both archive digests and every complete firmware image before writing either M4 image.
It requires a fresh directory, uses private Unix permissions, and excludes the M6 image.
Git ignores `.firmware-corpus`.
The normal test command reports three official-image tests as ignored.
`firmware-test` includes those tests and fails if an image is missing or modified.
The dedicated tests cover both image families, all six rates, every stream mode, repeated sessions, cancellation, and restoration.
Executable tests also compare captured samples and received playback packets.

Run an offline capture with firmware controls:

```sh
mise x -- cargo run --locked --package motu-m4 -- simulate capture --firmware .firmware-corpus/atmel.image --rate 192000 --frames 19 --output take.raw
```

The command reports `transport=firmware-control+simulated-audio` and the number of control instructions executed.
The same `--firmware` option works with `simulate play` and `simulate duplex`.
It conflicts with `--profile` and `--scenario`, which select the independent synthetic device.
The firmware adapter reads its initial clock from the emulated code before creating the comparison device.
Both inspected images initialize that clock to 48000 Hz.

## Processor and memory

The adapter uses [Armagnac 0.3.0](https://github.com/Ledger-Donjon/armagnac/tree/715dc09e74f219423f2f3baf9c818208e52473a1) with its ARMv7E-M integer instruction decoder.
The [vendor note](../vendor/README.md) records the source archive and compression feature change.
The adapter models single-processor `SEV` behavior because the selected library leaves that instruction unfinished.
It intercepts executed `WFI` and `WFE` effects and returns a wait stop instead of entering an unbounded event loop.
Conditional instructions keep their original conditions, and `WFE` consumes an existing event before continuing.

Memory layouts admit at most 1 MiB and reject arithmetic overflow and misalignment.
Images must fit the declared RAM, with aligned stacks and reset-vector targets inside the loaded payload.
Each execution admits 1–10000000 instructions and retains the last 64 instruction addresses.
Function calls validate Thumb entry points and use a return sentinel outside mapped memory.
Only a completed function return permits another isolated call on the same memory state.
Memory faults keep their address, transfer width, value when writing, and processor error kind.
Unsupported processor operations produce an explicit stop, including caught library panics.
No stop condition establishes successful firmware initialization.

Inspect bounded reset execution with explicit RAM assumptions:

```sh
mise x -- cargo run --locked --package motu-m4 -- execute-firmware --input .firmware-corpus/nxp.image --ram-base 0x20000000 --ram-bytes 0x30000 --instructions 100000
```

The command reports the stop, executed instruction count, registers, and recent instruction addresses.
`--entry` and four `--args` values select an isolated function call.
`--read` and `--read-bytes` inspect mapped RAM after execution.
The diagnostic runner accepts unrecognized image contents after structural validation.
The audio adapters require the exact complete-image SHA-256 digest before using any fixed address.

## Control transfers

For `nxp.image` 2.2.9+641, the model reserves 192 KiB of RAM starting at `0x20000000`.
The control adapter calls the `IN` handler at Thumb address `0x2000bf49` and the `OUT` handler at `0x2000c0f5`.
It reserves a 16 KiB memory window for USB at `0x40100000`, with setup storage at `0x40100080` and endpoint-zero `IN` descriptors.
An armed descriptor identifies the buffer and byte count to collect.
The adapter follows the firmware's progress state across 64-byte `IN` packets and both `OUT` handler phases.
The [official device header](https://github.com/nxp-mcuxpresso/mcux-sdk/blob/main/devices/LPC54628/LPC54628.h) identifies the matching USB device controller address `0x40094000`.
Matching addresses support the controller model without identifying the exact M4 board part.

For `atmel.image` 1.2.6+943, the model reserves 384 KiB of RAM starting at `0x20400000`.
The control adapter calls the endpoint interrupt routine at Thumb address `0x20401c09`.
It models endpoint-zero status, interrupt masks, write-one flag operations, and a 64-byte endpoint buffer window at `0xa0100000`.
Setup, `IN` readiness, and `OUT` data events drive the original interrupt routine.
The firmware generates response bytes and updates its clock state.
Register addresses follow the [`SAM E70/S70/V70/V71` data sheet](https://ww1.microchip.com/downloads/en/DeviceDoc/SAM-E70S70V70V71-Family-Data-Sheet-DS60001527F.pdf), `USBHS` register summary.
The separate backup-register model provides eight aligned word registers at `0x400e1890` for bounded reset inspection.
Unsupported register offsets and access widths fail rather than returning synthetic readiness values.

The full 74-byte range response splits into 64-byte and 10-byte packets in both images.
Integration tests compare every returned byte with the extracted table.
They exercise request lengths around the packet boundary and accept short replies beyond the table length.
The firmware adapters run data stages at declared entry points.
They don't model bus arbitration, interrupt priorities, packet retries, or physical clock settling.

## Verification boundaries

Routine tests use original instruction fixtures for arithmetic, calls, memory, conditional waits, and event handling.
The processor library is a third-party correctness boundary.
Protocol expectations and checked assembly examples provide independent expected results for the exercised paths.
They don't establish correctness for every Arm instruction.
`firmware-mutation` targets the processor adapter, peripheral models, and firmware-backed transport.
Its selected API tests exercise real image code.
`firmware-test` also covers the complete CLI workflows.
The mutation command requires `firmware-test` as its unmodified baseline and then runs the library tests for every mutant.
Run mutation testing in the bounded container described in the [development guide](README.md).

Bounded reset execution of `nxp.image` reaches 379852 instructions before an unfinished memory protection unit register operation stops the selected processor library.
The Atmel image reaches 32769 instructions with the backup-register model before an unmapped watchdog register stops execution.
Those observations identify concrete missing boot peripherals.
The isolated USB handlers execute without completing reset or configuring converters.
Flash programming, sample conversion, direct memory access, floating-point instructions, analog behavior, and complete board initialization remain outside the model.
