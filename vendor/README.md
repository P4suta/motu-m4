# Armagnac source

Ledger-Donjon distributes `armagnac` 0.3.0 under `MIT OR Apache-2.0`.
The [crates.io archive](https://crates.io/crates/armagnac/0.3.0) has SHA-256 `255ae500daac6abce07a5d8b18e6c3c6a8fd66ee5dab40e22206fe73e2f12a16`.
The vendored directory contains the original archive beside the extracted package.
The vendored `src` directory and package README preserve every byte from the archive.
The package manifest disables `object` default features and enables `read` and `std`.
Compressed object files aren't needed for raw firmware execution.
Disabling compression excludes `ruzstd` 0.6.0 and [RUSTSEC-2026-0344](https://rustsec.org/advisories/RUSTSEC-2026-0344).
The upstream package doesn't include separate license files.
The vendored distribution supplies the [Apache License text](armagnac/LICENSE-APACHE) under the upstream package's declared Apache-2.0 option.
The manifest keeps the upstream dual-license declaration.
