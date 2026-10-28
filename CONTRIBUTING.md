# Contributing to obs-rust-bitstream

This repo is Seraphina Studio (an egui desktop app in `src/bin/ui/`)
and the tested Rust library behind it. It is public, free and open.

## How to start

1. Clone the repo.
2. Run `cargo test` and `cargo test --features ui --lib --bins`.
3. Pick an unchecked item from the "Planned (not built)" table in
   README.md or from TOOLS.md and open a pull request.

## What is already done

The library modules listed in README.md. A test checks that list
against `src/lib.rs`, and another checks that every module is reached
from the app or an example.

## Rules

- No unwraps in production code.
- Result only for errors.
- Every new public function has a unit test, and every fix has a test
  that fails without it.
- A test that needs ffmpeg, a server or a key fails loudly without it,
  or is `#[ignore = "reason"]`. It never passes silently.
- No placeholder modules: a module lands with its first real caller.
- One change per pull request; `cargo fmt`, `cargo clippy --all-targets
  --features ui -- -D warnings` and `cargo machete` stay clean.

## Questions

Open an issue. Or just start. The tests will tell you if it is right.
