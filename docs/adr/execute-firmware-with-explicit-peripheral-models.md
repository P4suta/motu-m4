---
date: 2026-11-10
---

# Execute firmware with explicit peripheral models

## Status

Proposed.

## Context

Static firmware inspection can't test the instructions that produce USB responses.
Development must leave the physical device untouched.

## Decision

Use the pure Rust [Armagnac processor](https://github.com/Ledger-Donjon/armagnac/tree/715dc09e74f219423f2f3baf9c818208e52473a1) behind a bounded execution adapter.
Vendor release 0.3.0 with object-file compression off to exclude vulnerable `ruzstd` 0.6.0.
Require Rust 1.88 for the executor's cryptographic dependency.
Validate firmware vectors, mapped memory, call arguments, and execution budgets before running instructions.
Model documented peripheral behavior and refuse unknown accesses with addresses and execution context.
Keep firmware bytes outside version control and use deterministic assembly fixtures for routine tests.
Connect established firmware entry points to protocol tests with independent expected responses.

## Consequences

Processor emulation introduces a third-party correctness boundary.
Successful isolated calls establish behavior for their declared initial state.
They don't establish complete board equivalence.
Reconsider the processor library when a required instruction or validated result disagrees with an independent oracle.

## Alternatives

A mature external emulator offers another instruction oracle but adds a native runtime and platform packaging.
The static-data simulator remains useful for fault injection but can't execute firmware behavior.

## Confirmation

Require instruction fixtures, access refusals, bounded execution, official-image runs, and affected host end-to-end tests.
