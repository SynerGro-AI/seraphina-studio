# Public OBS Rust Rewrite — Plan

Goal: Seraphina Studio, a desktop app for paint, composite, 3D,
animation, streaming and a materials lab, and the tested library
behind it.
**Work smarter, not harder.**

## Where things stand

* The library modules that exist are listed in README.md (the list is
  checked against `src/lib.rs` by a test). Each has unit tests.
* `bitstream` is a port of OBS `bitstream_reader` and keeps a C ABI.
* Recording and RTMP streaming run in the app through ffmpeg
  (`src/bin/ui/output.rs`), tested against a local ffmpeg RTMP listener
  and, on Linux CI, mediamtx. Their source is the canvas, the screen, or a
  webcam (`src/bin/ui/camera.rs`: V4L2 on Linux, Media Foundation on
  Windows; macOS AVFoundation is planned). Live output has an optional
  stream delay (`encoder_packet` + `output_delay_queue` between the
  encoder ffmpeg and a copy-only relay ffmpeg).
* `tools/obs_cpp_to_rust_parser.py` turns an OBS `.c` file into a Rust
  skeleton that documents the lessons in `tools/lessons.json`.

## Planned (not built)

The v1 cleanup removed modules that the app and examples never used
(the `every_library_module_is_reached_from_a_binary_or_example` test
keeps it that way). They come back only as real implementations, each
with a test. The tables in README.md ("Planned (not built)") hold the
target dates (America/Edmonton; targets, not promises):

* v0.1.0: the release being cut now (main, the AI reliability work, the
  v1 cleanup, camera, stream delay and Windows packaging). Pending its tag
  and the owner's approval; no date here.
* M4 part 2 (M4b), part of v0.2.0: Wed Oct 28, 2026 (target).
* First green CI run, run locally: Sun Nov 1, 2026 (target). The GitHub
  run waits for the billing lock to lift; it does not unlock by itself.
* v0.2.0 release: Fri Nov 6, 2026 (target, pending the owner's approval).
  It holds the physics validation fixes (B4, B5 and the rest), M4b with the
  recorder timing fix (R1) and the local learner (C4), shared-learning
  packs (C7) and topic backgrounds (E7).

Everything below comes after v0.2.0 and has no date yet:

1. Seraphina.AGI as a separate local process (Grok is today's
   provider): proposed.
2. Remove `dist/` and root clutter: M5.
3. Physics foundation: M6.
4. OBS sources, scenes and the rest of Stream (E6): M7+.
5. The rest of the Photoshop / After Effects / Maya tools listed as
   `[ ]` in TOOLS.md: M7+.

## Principles
- Foundation first
- Math is perfect (prove it with tests)
- Preserve the public C ABI where a module ports OBS C code
- Fix the bugs in the safe path
- Delete before you add: no placeholder modules
- Every README claim is backed by a test or a measurement

## How to contribute
Clone, run `cargo test --lib` and `cargo test --bin ui`,
open a PR.
