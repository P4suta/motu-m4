default: check

check: format lint test workflow security

format:
    mise x -- cargo fmt --all -- --check

lint:
    mise x -- cargo clippy --locked --workspace --all-targets -- -D warnings

test:
    mise x -- cargo test --locked --workspace

workflow:
    mise x -- actionlint .github/workflows/check.yml
    mise x -- zizmor --offline --persona pedantic --strict-collection --no-config .github/workflows/check.yml

security:
    mise x -- cargo audit --file Cargo.lock --deny warnings
    mise x -- gitleaks dir . --no-banner --redact

proof:
    mise x -- cargo kani --package motu-m4-core --jobs 1 --output-format terse
    mise x -- cargo kani --package motu-m4-firmware --jobs 1 --output-format terse

mutation:
    mise x -- cargo mutants --workspace --test-workspace true --jobs 1 --no-shuffle --colors never --cargo-arg=--locked

probe:
    mise x -- cargo run --locked --package motu-m4 -- probe

audio-test:
    mise x -- cargo test --locked --package motu-m4-audio
    mise x -- cargo test --locked --package motu-m4 --test native_cli

firmware mac windows:
    mise x -- cargo xtask firmware --mac-archive {{ quote(mac) }} --windows-archive {{ quote(windows) }}

firmware-corpus mac windows:
    mise x -- cargo xtask firmware --mac-archive {{ quote(mac) }} --windows-archive {{ quote(windows) }} --export-corpus .firmware-corpus

firmware-test:
    mise x -- cargo test --locked --workspace -- --include-ignored

firmware-mutation: firmware-test
    mise x -- cargo mutants --workspace --test-package motu-m4 --test-package motu-m4-firmware --jobs 1 --no-shuffle --colors never --cargo-arg=--locked --cargo-arg=--lib --gitignore false --file 'crates/motu-m4-firmware/src/**' --file 'crates/motu-m4/src/firmware_device.rs' -- -- --include-ignored
