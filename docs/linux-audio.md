# Daily Linux audio

The `audio` commands record, play, and duplex through the installed Linux audio driver.
Desktop configuration provides two stereo outputs, two mono inputs, and two stereo inputs for ordinary audio applications.
The kernel's `snd-usb-audio` driver handles USB transfers.
The Rust app handles sample conversion, streaming files, channel selection, and recovery reporting.
Normal audio commands don't need downloaded firmware images.

## Build and discover

On Ubuntu 24.04, install the native libraries and desktop audio tools:

```sh
sudo apt-get install libasound2-dev pipewire pipewire-bin pipewire-pulse wireplumber pulseaudio-utils dbus-daemon
mise x -- cargo build --locked --release --package motu-m4
./target/release/motu-m4 audio devices
./target/release/motu-m4 audio doctor
```

Run desktop commands inside the user session that owns PipeWire and WirePlumber.
`audio devices` reads kernel metadata without starting audio.
It verifies the product string and USB identifiers before selecting a card.
`audio doctor` also checks the PipeWire connection and four-channel source and sink.
Select `--card M4` when the system reports more than one M4.
`audio devices --json` supplies structured discovery results.

## Desktop applications

Expose all four hardware channels, then install routing:

```sh
./target/release/motu-m4 desktop profile
./target/release/motu-m4 desktop install --rate 192000 --restart
./target/release/motu-m4 desktop status
./target/release/motu-m4 desktop default main
./target/release/motu-m4 desktop default input-1
```

`desktop profile` selects the advertised `pro-audio` profile by its actual index.
`desktop install` writes one private file under the user's `pipewire/pipewire.conf.d` directory.
The `--restart` option restarts user audio services and interrupts active audio sessions.
Without `--restart`, the endpoints load after the next audio-server restart or login.
The command refuses an existing file with different contents.

Desktop routing defaults to 192000 Hz while an M4 endpoint is active.
The `--rate` option on `desktop install` and `desktop render` accepts any supported M4 sample rate.
The [active-node rate property](https://docs.pipewire.org/page_man_pipewire-props_7.html) sets the connected graph's rate, so PipeWire resamples streams with different sample rates.
Another active node forcing a different rate can override the requested graph rate.
Generated loopbacks set `resample.quality` to 14, the upper bound in the documentation.
Each app controls its own resampling settings.
Remove the owned routing file before installing a different rate.

Select the M4 endpoints in desktop sound settings, a browser, a call app, or an audio workstation.
Main routes stereo to outputs 1–2, and Line routes stereo to outputs 3–4.
Input 1 and Input 2 expose separate mono inputs.
Inputs 1–2 and Inputs 3–4 expose stereo pairs.
The `motu_m4.M4.main.monitor` source supplies desktop playback audio to compatible apps.
The endpoints close when the hardware target disappears, without falling back to another speaker or microphone.
After reconnecting the M4, run `desktop install --restart` again to restore the endpoints.
Reapply the desired defaults after a restart.

Set software volume or run `desktop mute`:

```sh
./target/release/motu-m4 desktop volume --endpoint main --percent 50
./target/release/motu-m4 desktop mute --endpoint input-1
./target/release/motu-m4 desktop mute --endpoint input-1 --unmute
```

The volume command accepts 0–100 percent.
Hardware gain and direct monitoring remain controlled by the M4's physical controls.
`desktop render` prints the configuration before installation.
Its `--snapshot PATH` option accepts an offline `pw-dump` snapshot for diagnosis.

Remove the generated routing while preserving unrelated configuration:

```sh
./target/release/motu-m4 desktop remove --card M4 --restart
```

Removal works with the M4 disconnected when `--card` names its saved card ID.
The command only removes a file carrying the app's ownership marker.
Use `--config-dir PATH` for an alternate configuration root during installation or removal.
If hardware node names or profiles change, remove the owned configuration and generate it again.

## Capture and playback

Record input 1 into a mono, 24-bit `WAV` file at 192000 Hz:

```sh
./target/release/motu-m4 audio capture --card M4 --rate 192000 --seconds 60 --inputs 1 --output voice.wav
```

The [M4 specifications](https://h-resolution.com/product/m4/) list sample rates up to 192 kHz.
Capture defaults to 192000 Hz, all four inputs, and 60 seconds.
An explicit `--rate` overrides the recording default.
`--frames N` selects an exact frame count instead of a duration.
`--inputs` accepts `1`, `2`, `3`, `4`, `1-2`, `3-4`, or `all`.
Files stream to disk rather than accumulating in memory.
The size check reserves header space below the 4 GiB `WAV` limit.
Existing files aren't overwritten, and Unix recording files use private permissions.
Ctrl+C finalizes the header and retains complete captured frames.
A disconnection also finalizes the captured prefix and reports failure.

Play a file or record while playing a backing track:

```sh
./target/release/motu-m4 audio play --card M4 --input voice.wav --outputs main
./target/release/motu-m4 audio duplex --card M4 --input backing.wav --output take.wav
```

Playback accepts mono, stereo, or four-channel `WAV` files at the six M4 sample rates.
Playback and duplex take their sample rate from the input file.
Native playback adds no sample-rate conversion.
Playback converts signed 16-bit, 24-bit, and 32-bit audio to the M4's 24 valid bits.
It also accepts 32-bit floating-point audio.
Nonfinite floating-point samples fail validation.
Mono playback duplicates the signal across the selected stereo output pair.
`--outputs line` selects outputs 3–4.
Four-channel files require `--outputs all`.
Duplex records as many frames as the backing track contains.

Direct hardware access can conflict with an audio server that already owns the card.
For shared desktop audio, select the desired desktop defaults and use an installed shared audio device through `--pcm default`.
The shared device can resample or remix channels according to system configuration.
Direct hardware access preserves the requested four-channel format and rejects a negotiated rate mismatch.

## Implementation and checks

The adapter uses the Advanced Linux Sound Architecture (ALSA) [interleaved transfer interface](https://www.alsa-project.org/alsa-doc/alsa-lib/pcm.html) with nonblocking device opens.
It requests 256-frame periods and a four-period buffer.
Playback fills the negotiated buffer before starting, following the [ALSA playback example](https://www.alsa-project.org/alsa-doc/alsa-lib/_2test_2pcm_8c-example.html).
Each transfer handles partial frames, bounded waits, interruptions, buffer overruns, and suspension.
Cleanup errors preserve the original failure.
Successful reports include transferred frame counts and recovery counts.
Recovered overruns can create discontinuities in a recording.

The routing generator follows [PipeWire loopback channel mapping](https://docs.pipewire.org/page_module_loopback.html).
It reads actual node names and channel positions through [`pw-dump`](https://docs.pipewire.org/page_man_pw-dump_1.html).
The [session-manager properties](https://docs.pipewire.org/page_man_pipewire-props_7.html) keep routes passive and prevent fallback to unrelated devices.
Disabling reconnection also protects [WirePlumber 0.4](https://github.com/PipeWire/wireplumber/blob/0.4.17/src/scripts/policy-node.lua), whose linking policy lacks fallback suppression.
The [native audio decision](adr/use-alsa-and-pipewire-for-daily-audio.md) records the driver boundary.

Run the audio checks with the normal project gate:

```sh
mise x -- just check
mise x -- just audio-test
```

Native integration tests use an explicit software-only `ALSA` configuration in isolated child processes.
The desktop test starts private PipeWire, WirePlumber, and PulseAudio compatibility servers.
Hardware discovery stays off during the desktop test.
The test checks channel links, endpoint visibility, default selection, and hardware-target removal.
It restarts the isolated servers and verifies that every route returns.
Native controls cover volume and the `mute` command.
Tests use PipeWire `1.0.5`, WirePlumber `0.4.17`, and Ubuntu `24.04`.
Those tests don't measure analog performance or establish Windows/macOS latency equivalence.

An attached M4 passed physical checks at 48000 and 192000 Hz on November 10, 2026.
Capture and duplex produced four-channel, 24-bit recordings lasting 0.1 seconds at both rates.
At 192000 Hz, capture, silent playback, and duplex each transferred 19200 frames without a reported recovery.
Both audio-device handles closed after the checks.
The short checks don't establish long-session stability or analog channel fidelity.
The [processing-rate decision](adr/select-processing-rate-for-active-m4-routes.md) records the defaults and their resource costs.
