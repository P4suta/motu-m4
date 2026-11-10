# Firmware inspection

The offline tools inspect the Universal Firmware Updater `2026.2` from the [official M4 download page](https://motu.com/en-us/download/product/410/).
The macOS and Windows archives contain three identical firmware images.
The Atmel image contains version `1.2.6+943` and 353356 bytes.
The `nxp.image` file contains version `2.2.9+641` and 168596 bytes.
The M6 image contains version `1.2.3+2026` and 270060 bytes.
The updater release number differs from each embedded firmware version.

## Repeat the analysis

Download the public archives without running the updater.
The commands use the HTTP client `xh` and write into `/tmp`.

```sh
xh --ignore-stdin --follow --output /tmp/motu-mseries-universal-mac.zip GET https://motu.com/en-us/download-center/download/3013
xh --ignore-stdin --follow --output /tmp/motu-mseries-universal-windows.zip GET https://motu.com/en-us/download-center/download/3012
mise x -- just firmware /tmp/motu-mseries-universal-mac.zip /tmp/motu-mseries-universal-windows.zip
```

The command checks each archive's exact size and SHA-256 digest before reading ZIP entries.
It bounds decompression to each expected entry size, checks each image's digest, and finds one identical full image in the Windows executable.
It then compares the USB descriptor templates from `atmel.image` and `nxp.image` with the committed fixtures.
Changed downloads fail verification rather than updating the recorded digests.
The digests identify the bytes inspected on November 10, 2026.
They aren't publisher signatures.
The tool reads archive entries into memory without writing archive paths or applying permissions.
The development helper requires Rust 1.88 because its ZIP library includes current extraction security fixes.
The USB protocol core supports Rust 1.85.
The audio app and firmware executor require Rust 1.88 for the current cryptographic dependency.

Use `cargo xtask firmware --help` for the explicit fixture export option.
Export requires a fresh destination directory and writes only functional USB protocol data.
The repository excludes firmware instructions, vendor executables, device serial numbers, and audio content.

Run `motu-m4 inspect-firmware --input PATH` on an image extracted with a ZIP reader.
The read-only inspector validates the observed 64-byte container header, payload length, printable version, and zero padding.
It prints the initial stack and reset-handler words and locates USB descriptor candidates with file offsets.
It limits input to 1 MiB and rejects truncated containers and trailing data.
A valid header doesn't establish authenticity or executable correctness.

## Descriptor observations

Each image contains one 18-byte USB device descriptor and one 387-byte configuration template.
The Atmel template places capture on endpoint `0x83`.
The template from `nxp.image` uses `0x82`.
Both use playback endpoint `0x02`, clock source 2, 400-byte playback packets, and 405-byte capture packets.
The device templates report USB identifiers `07fd:000b`.
Their `bcdDevice` values are `0x0206` for Atmel, `0x0209` for `nxp.image`, and `0x0203` for M6.
The M6 image also contains a four-channel template, so a static template alone can't identify the final device layout.
Runtime descriptor changes and product selection remain separate questions.
The simulator labels the extracted layouts `atmel-template` and `nxp-template` rather than treating image filenames as hardware identities.
The `recorded` profile uses the cached descriptor fixture from the attached M4.
The `original-synthetic` profile changes that fixture's product identifier to `0x0008` to test the older Linux timing policy.
The synthetic profile doesn't claim to reproduce an observed original-generation descriptor set.

## Driver behavior

[Linux commit `7475ea8b58616b33c04afb976cf0fbaad9dd63fb`](https://github.com/torvalds/linux/blob/7475ea8b58616b33c04afb976cf0fbaad9dd63fb/sound/usb/quirks.c) applies a four-second boot delay to product `0x0008`.
Product `0x000b` selects `QUIRK_FLAG_CTL_MSG_DELAY_1M`, which waits 1000–2000 microseconds after class control transfers.
The [control wrapper](https://github.com/torvalds/linux/blob/7475ea8b58616b33c04afb976cf0fbaad9dd63fb/sound/usb/helper.c) applies the delay even when a transfer returns an error.
The Rust engine waits at least 1000 microseconds through success, failure, and clock restoration.
It uses elapsed transport time to wait out the original-generation boot interval.
The simulator enforces literal timing rules without consulting the driver's delay builders.
Virtual time makes the four-second initialization deterministic and instantaneous in tests.

The [Linux clock implementation](https://github.com/torvalds/linux/blob/7475ea8b58616b33c04afb976cf0fbaad9dd63fb/sound/usb/clock.c) skips a redundant sample-rate write when the requested rate already matches.
The Rust engine also skips that write and its restoration while preserving clock validation and stream cleanup.
The Linux quirk table returns after its first matching entry.
The exact `0x000b` delay entry doesn't inherit the later vendor-wide rate-validation flag.

## Analysis limits

The macOS updater contains unstripped symbols for discovery, firmware-version reads, block transfers, status, verification, and reboot.
Static inspection of `Updater::GetFrimwareVersion()` shows setup bytes `a1 02 00 00 00 00 40 00` and a version string beginning at response offset 4.
The spelling of that function name follows the vendor binary.
The Windows driver `4.6.0.705+cc30ed084` lists audio identifiers `07fd:0008` and `07fd:000b`, and update identifiers `07fd:0009` and `07fd:000d` in its installation files.
Those observations don't establish a complete firmware-update protocol or permit a physical update.
The [firmware execution adapter](firmware-execution.md) runs audio control handlers from `atmel.image` and `nxp.image` in a Rust processor emulator.
It models their control-transfer memory and peripheral boundaries and connects their responses to the host engine.
Complete firmware initialization, converters, flash programming, and analog circuits remain outside the model.

## Clock range fixtures

Each firmware image contains the same 74-byte clock range table.
The table starts at file offset 82868 in the Atmel image, 67580 in `nxp.image`, and 157920 in the M6 image.
It encodes six discrete frequencies: 44100, 48000, 88200, 96000, 176400, and 192000 Hz.
The byte layout matches the USB Audio clock frequency range parameter block.
Static byte inspection establishes the table's content.
Execution tests also exercise the request handlers in `atmel.image` and `nxp.image` that return it.
The archive verifier checks all six frequency records and exports the bytes as a shared fixture.
The simulated device serves that fixture for its range response without calling the driver's decoder or constructing a frequency table.
The driver then validates the firmware-derived response during every simulated session.
Tests cover truncated range data, altered records, all supported rates, and refusal before streaming.
