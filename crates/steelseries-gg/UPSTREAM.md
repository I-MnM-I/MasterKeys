# Upstream

Vendored from https://github.com/Ven0m0/steelseriesgg-rs (MIT, see LICENSE)
at commit `84a015293606db70f39dc1e857b264648db3e489`, on 2026-11-08.

## Local changes

- Package renamed `steelseries-gg-linux` → `steelseries-gg`; build profiles moved to the workspace root.
- Trimmed to Apex 3 (0x161A) only. Removed: audio/Sonar, desktop GUI, CLI, daemon, commands, diagnostics,
  pollrate, benchmarks, fuzzing, examples, tools, scripts, build.rs; all headsets, mice and other keyboards.
- Removed the whole multi-device layer (`devices/`, `device_state`, `validation`) and per-key RGB
  (`PerKeyEffect`, `PerKeyEffectEngine`, `PerKeyRgbController`). The Apex 3 is zone-lit; its I/O lives in
  `apex3` (protocol verified on hardware, see `docs/apex3-protocol.md` in the workspace root).
- `performance` reduced to `EffectComplexity` + `calculate_effect_complexity` (used by the RGB engine).
- Added `apex3` sync API (report builders + `Apex3Device`), protocol inlined from the hardware probe.
- `EffectEngine` computes the animation phase in f64 (wrapped to one cycle) so animations stay smooth after days of uptime; added `compute_at(elapsed)`.
- Kept docs: `docs/development/gamesense-api.md`, `gg-gamesense-engine.md`.
- `input::Remapper::new` accepts an empty binding set (MasterKeys runs it for pass-through around macros); the empty check moved to `input::linux::run`.
- `input::BindingSet` derives `PartialEq`/`Eq` (MasterKeys only re-sends bindings that changed).
- `gamesense`: `set_rgb_callback` replaced by `set_output(mpsc sender of GameSenseOutput)` (colours carry the game; heartbeats and removed games are reported); `router()` public and `serve(listener)` added (serves without writing coreProps.json); `rgb-zoned-device` handlers supported.
