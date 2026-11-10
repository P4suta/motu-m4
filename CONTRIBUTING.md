# Contributing

## Build and verify

Follow the [Linux audio guide](docs/linux-audio.md) for native libraries and desktop tools.
The repository pins its tools in `mise.toml`.

```sh
mise install
mise x -- just check
```

The standard gate checks formatting, Clippy, tests, workflow configuration, dependency advisories, and secrets.
Audio tests use software devices and private audio-server sessions.
They don't require a connected M4 or proprietary firmware images.
The [development guide](docs/README.md) describes protocol proofs and the [firmware guide](docs/firmware.md) describes optional image checks.

## Propose a change

Open an [issue](https://github.com/P4suta/motu-m4/issues) describing the affected command and expected behavior.
Keep each change within one issue's scope.
Add a failing behavioral test before changing production behavior.
Keep device effects behind the existing transport and desktop ports.
Use synthetic audio and descriptor fixtures without device serial numbers.
Preserve the upstream source bytes identified in [vendor/README.md](vendor/README.md).
Use an English Conventional Commit title for each commit and pull request.
Explain the resulting behavior and actual validation in the pull request.

## Release handling

Prepare releases as drafts and attach the final artifacts before publishing.
Published release assets remain immutable.
