# Release Notes

## 0.1.0 - 2026-05-13

Initial release. Includes `.smfmap.yaml` schema/support through v0.4.1.

### Highlights

- Added `.smfmap.yaml` v0.4.1 support for opt-in MGSC111 `_target` pitch glide rendering as MIDI Pitch Bend.
- Added OPLL pseudo drum mapping for `#opll_mode 0` tracks that are physically OPLL/FM but should render as GM drums.
- Added v0.4 family-aware `@N` / `@eN` / `@rN` handling: PSG selects envelopes, while SCC/OPLL use `tone_map` and envelope rendering.
- Added PSG noise drum-map output, SCC/OPLL envelope-map output, and `yR,D` register-write diagnostics/mapping hooks.
- Added `--emit-config-skeleton` output for observed tones, envelopes, register writes, and the disabled-by-default `pitch_glide` section.
- Added GitHub Actions CI and tagged-release binary packaging.

### Compatibility Notes

- `pitch_glide.enabled` defaults to `false`. Pitch Bend is MIDI channel state, so glide tracks should normally be assigned to independent MIDI channels before enabling it.
- OPLL pseudo drum conversion is explicit config only. `#opll_mode 0` tracks are not auto-detected as drums.
- On OPLL pseudo drum tracks, `@N` and `@eN` are still kept as source state for rule matching, but Program Change output is suppressed by default when the map asks for it.
- Exact SCC wave morphing from tone changes inside `@eN` is not implemented. Current output approximates this through ordinary SMF events and pitch glide.

### Upgrade Notes

- Regenerate skeleton files with `--emit-config-skeleton` when adopting v0.4.1; older configs remain usable, but they will not contain `pitch_glide`.
- PSG melody tracks that need a specific GM instrument should use `tracks.<id>.events` for an initial `pc`, because PSG `@N` is an envelope selector.
- For songs that use `#opll_mode 0` OPLL tracks as percussion, configure `source_family: opll`, `render_role: drum`, `drum_map`, and a matching `opll_pseudo_drum_map`.

### Verification

- `cargo clippy -- -D warnings`
- `cargo test`
- `bash scripts/validate_compat.sh`

### Sample Policy

- The tracked `examples/placeholder.*` files are synthetic placeholders authored for this repository. They are intended to demonstrate config syntax only and are not a reference conversion of an existing song.
