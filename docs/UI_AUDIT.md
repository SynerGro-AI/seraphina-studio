# Audit: SynerGro-AI/obs-rust-bitstream, now SynerGro-AI/seraphina-studio (UI focus)

Date: 2026-10-20 (America/Edmonton, MDT)
Clone: `/workspace/obs-rust-bitstream`. Base: `main` @ `b0046ec`. Work is on the local branch `ui-polish` and has **not been pushed**.

## 1. What the project is
This is a Rust library crate of about 644 files and about 61.6k lines. It contains many small "foundation modules": the bitstream `Reader` (wrap or no-wrap position counters), keyframes, live weights, joint hierarchy, a reconnect policy, a PSD reader, export and profiling code, and more. It also has several CLI bins. The only GUI was `src/bin/ui.rs`, a single 800-line egui window called "Tuxedo Edit". It also existed as an older stub on the `side-panels` branch. The two `copilot/*` branches are empty.

## 2. How to run
* **Windows (primary):** install Rust (MSVC toolchain), then run `cargo run --release --features ui --bin ui`. See the README section "Desktop app: Seraphina Studio".
* **Prebuilt Windows bundle:** the `windows-ui` artifact of a CI run (`.github/workflows/ci.yml` builds it with MSVC and stages it with `packaging/windows/stage.ps1`; `packaging/windows/zip-files.txt` lists its files: the exe, `README.txt`, `LICENSE`, `SOURCE.txt`, `assets/`, `scripts/` and the `out.*` samples). The cross-compiled zip this audit first used was built by hand outside the repo and is not available.
* **Linux:** `cargo run --release --features ui --bin ui`. Needs a GL driver and libxkbcommon-x11.
* **Cross-compile:** `rustup target add x86_64-pc-windows-gnu`, install `mingw-w64`, then run `cargo build --release --features ui --bin ui --target x86_64-pc-windows-gnu`.
* **Env:** `SERAPHINA_MODE=stream|paint|3d|animate|composite` and `SERAPHINA_LIGHT=1`.

## 3. Build status

| Check | Before (main) | After (ui-polish) |
|---|---|---|
| `cargo build` | OK, 57 warnings | OK, **0 warnings** |
| `cargo test` | **Fails to compile** (see R1) | **1717 passed**, 0 failed |
| `cargo test --features ui --bin ui` | n/a | **11 passed** |
| `cargo build --features ui --bin ui` | Builds, but the UI hangs on its first frame | OK, 0 warnings; runs |
| Windows cross-compile (gnu) | **Fails** (see R2) | OK, 0 warnings, PE32+ GUI-subsystem exe |
| Toolchain | System rustc 1.85 too old for the deps | rustup stable 1.99.1 |

Windows exe: `target/x86_64-pc-windows-gnu/release/ui.exe` (from the cross-compile command above) (about 19.3 MB). I did **not** execute it, because the box has no Windows and no Wine.

> **Update (v1 cleanup, 2026-10-24):** this is no longer true. The Windows
> build now runs under Wine on the box (lib and ui tests, plus a launch
> smoke test with a screenshot), and CI's `windows-ui` job runs the lib
> and ui tests on `windows-latest`. `src/windows_capture.rs` (R2) and the
> `windows` feature were later deleted as unused.

## 4. Repo-level findings

| ID | Severity | Location | Finding | Fix |
|---|---|---|---|---|
| R1 | High | Cargo.toml `[[bin]] ui`; src/export.rs:199; src/psd.rs tests | CI `cargo test` was broken: the ui bin compiled without the `ui` feature, E0382 use-after-move in export.rs, and the psd tests raced on shared temp filenames | `required-features=["ui"]`, `.clone()`, unique temp names |
| R2 | High | src/windows_capture.rs | `cfg(windows)` code used the optional `windows` crate without the feature, so every Windows build failed | Gated on `all(windows, feature="windows")`, plus a stub `Capture` impl |
| R3 | Low | 50+ lib files | 57 compiler warnings (mostly imports used only by tests) | `#[cfg(test)]` imports, removed dead ones |
| R4 | Low | `dist/obs-rust-ui` | Committed Linux prebuilt of an *older* UI revision that doesn't match the source. It still shows fake "Streaming: Live / REC 00:47:32" | README note; recommend deleting it from the repo |
| R5 | Low | .github/workflows/ci.yml | No UI tests and no Windows job | Added UI tests and a `windows-latest` job (**not yet run on GitHub**) |

## 5. Original UI findings (`src/bin/ui.rs` on main; line numbers refer to that file)

| Severity | Line(s) | Finding |
|---|---|---|
| **Critical** | 625 | `over_rgba.pixels().nth(i)` inside the per-pixel loop is O(n²) on a 1168×784 image. The first frame never renders (100% CPU, black window). |
| High | 237–242, 441–446 | Save / Save PSD overwrite the bundled `assets/mountain_spire.psd`, or write PSD bytes into a `.jpg` after Open Recent |
| High | 702 | The canvas interaction rect is the whole column, so strokes land offset from the cursor |
| High | 644–648 | The "Burn" tool writes gold pixels into the first image row, not at the click position |
| Medium | 782, 784 | Status bar hardcodes "Streaming: Live" and "REC: 00:47:32" |
| Medium | 113–114 | Streaming and recording default to `true` |
| Medium | 409–418, 777, 779 | Hardcoded canvas info and document name |
| Medium | 401–406, 635–636 | Opacity, Fill and Feather sliders do nothing (no recomposite) |
| Medium | 633 | "Normal" blend averages layers instead of compositing over |
| Medium | 664 | The "Gaussian" filter is actually grayscale |
| Medium | 537–539 | The "Retry" button increments `retry_max` instead of reconnecting |
| Medium | 438, 516 | Silently writes `assets/weights.txt` and `assets/stream_dump.bin` |
| Medium | — | Open fails silently, with no error feedback |
| Low | 106 | Default tool "Brush Tool" doesn't match the toolbar names |
| Low | 768 | Hotkey shown in the status bar is hardcoded to "B" |
| Low | 207–208 | Fake "Pro User / LICENSED" labels |
| Low | 583 | The Toon Boom layer list shows Maya objects |
| Low | 57, 60 | Unused fields |
| Low | 763 | f32 literal-fallback lint |
| Low | 205 | PSD path not resolved via `resolve_asset` |
| Low | README | Promises "Press ? to see every bound key", but no keys were bound |

## 6. What was built (branch `ui-polish`)
The old UI was replaced by **Seraphina Studio** (`src/bin/ui/`: main, theme, stream, paint, scene, animate, composite, timeline, assistant_panel). New tested library modules: `ui_canvas`, `stream_inspect` and `assistant`.

* Shell: top bar with a segmented mode switcher (Stream / Paint / 3D / Animate / Composite), collapsible and resizable side panels, inspector, timeline, status bar, toasts, and a shortcuts sheet (`?`). It has dark and light palettes, and uses consistent spacing, type scale and cards.
* Every finding in section 5 is resolved by the rewrite. The composite is linear and cached, Save goes to `*_edit.*` (never overwrites assets), the status bar reflects real state, and there are no fake license labels.

### Real vs stubbed

| Area | Status |
|---|---|
| Stream inspector | **Real.** Drives the repo's bitstream `Reader` with a Safe (usize) / Wrap (u8) toggle, a read log, hex grid, and a dimmed region past the u8 wrap point |
| Stream output / reconnect | **Simulated.** No encoder or network; the reconnect policy is shown with a simulated state |
| Paint | **Real.** Brush, eraser, layers, blend modes, opacity, filters, undo/redo, zoom/pan, save |
| 3D | **Real but basic.** Wireframe viewport of a small scene (primitives plus joints registered in the repo's `JointHierarchy`) with an outliner and transform inspector; no mesh import or real renderer |
| Animate / Composite | **Real.** Keyframe tracks with linear interpolation, a scrubbable timeline, layer transforms |
| Seraphina assistant | **Stub.** The `AssistantBackend` trait has an `OfflineBackend` that returns canned, clearly labelled responses; no model is connected |
| Online learning | **Real hook, toy learner.** The `OnlineLearner` trait's `WeightNudgeLearner` applies clamped ±step updates from 👍/👎 feedback; no model is trained |
| PSD | Reads metadata and layers only; pixel data is not decoded (it uses the bundled JPEG for pixels) |
| Docking | Panels can be toggled and resized only; no drag-and-drop docking |

## 7. Screenshots (real captures via Xvfb with software GL on Linux)
Taken on the audit box and not in the repo (so not available to readers): `01_stream_dark`, `02_paint_dark`, `03_3d_dark`, `04_animate_dark`, `05_composite_dark`, `06_assistant_chat`, `07_shortcuts`, and `08_{stream,paint,3d,animate,composite}_light` (all `.png`).

## 8. Remaining gaps and recommendations
1. Run the exe on real Windows (DPI scaling, GPU driver, file dialogs). It has only been cross-compiled.
2. Let the new CI Windows job run on GitHub.
3. Connect a real assistant backend behind the `AssistantBackend` trait, and a real stream encoder.
4. Decode PSD pixel data; add true docking (e.g. egui_dock).
5. Delete the stale `dist/obs-rust-ui` from the repo.
