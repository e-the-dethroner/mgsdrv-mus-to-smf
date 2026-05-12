# mgsdrv-mus-to-smf

MGSDRV/MGSC111 `.MUS` MML text to Standard MIDI File Type 1 converter.

```bash
cargo run -- input.mus -o output.mid
```

The binary name is `mgs2smf`.

Documentation:

- [`.smfmap.yaml` Guide](docs/smfmap.en.md)
- [`.smfmap.yaml` 使い方ガイド](docs/smfmap.ja.md)

```bash
mgs2smf input.mus -o output.mid \
  --ppq 3600 \
  --loop-count 2 \
  --loop-marker-name loopStart:loopEnd \
  --envelope-mode cc11 \
  --tone-policy ignore \
  --rhythm-map gm \
  --channel-overflow error \
  --encoding auto \
  --octave-base o4c=60 \
  --config song.smfmap.yaml \
  --emit-config-skeleton observed.smfmap.yaml \
  --diagnostics output.json
```

Implemented MVP:

- UTF-8 / Shift-JIS loading via `encoding_rs`, auto fallback, CRLF/LF normalization, and `;` comment stripping.
- `#opll_mode`, `#tempo`, `#title`, `#macro_offset`, `#play_track`, `#end`.
- `@e` / `@r` envelope definitions, including tempo-map-aware 60 Hz CC timing and preservation of non-SMF `@e` commands in IR/diagnostics.
- `@mN={...}` control text definitions and `@mN` MML text meta output.
- Ignored `@s` / `@v` tone definitions with diagnostics, plus OPLL `@#N = M` assignment parsing for tone-map indirection.
- `--tone-policy text_meta` preserves ignored tone definitions as MIDI text meta events.
- Source track lines `1..9`, `a..h`, `r`, with duplicated content for multi-track selectors.
- Notes, rests, octave, configurable `o4c`, default length, `%N`, dotted lengths, ties, slur with same-pitch merge, `t`, `v`, relative volume, `q`, `@N` tone designation, `@eN`, `@rN`, loops, and `!`.
- `[0 ...]` infinite loops are finite-rendered to a common global loop horizon; MSXPlay-style header comments such as `;[gain=1.0 ...]` default to two global loops unless `loop=N` or `--loop-count N` overrides it.
- `@N` can render as GM Program Change with `--tone-policy gm_program` or as MIDI text meta with `--tone-policy text_meta`.
- Unsupported MML commands from the spec (`m`, `s`, `n`, `/`, `\`, `p`, `h`, `ho`, `hf`, `hi`, `y`, `so`, `sf`, `ko`, `kf`, `$`, `@p`, `@l`, `@\`, `@f`, `@o`) are ignored with classified diagnostics.
- `#opll_mode 1` rhythm tracks `f` / `r` with GM drum notes for `b`, `s`, `m`, `c`, `h` when `--rhythm-map gm` is active.
- `--loop-marker-name start:end` customizes infinite loop marker names.
- `--channel-overflow error|shared|multi_port`; `shared` reuses MIDI channels and suppresses CC envelopes on shared channels, while `multi_port` emits MIDI Port meta events.
- `.smfmap.yaml` loading via `--config` or discovered `.smfmap.yaml` / `*.smfmap.yaml` files.
- `;@smf` manual SMF directives for `pc`, `bank`, `cc`, `pb`, `rpn`, `nrpn`, `marker`, `text`, `macro`, and `reset`, with optional `track=`, `channel=`, and `conductor` targets.
- v0.4 family-aware `@N` dispatch: PSG/PSG noise select envelopes, while SCC/OPLL route `@N` through `tone_map`; PSG `@N` does not emit Program Change.
- PSG noise drum-map output, SCC/OPLL envelope-map output, and register-rule emission from `yR,D` when enabled by `.smfmap.yaml` / `--register-map on`.
- v0.4 OPLL pseudo drum maps can route explicit `render_role: drum` OPLL tracks through GM drum notes using macro/tone/envelope/note match rules.
- v0.4.1 `pitch_glide` can render MGSC111 `_target` pitch glide as opt-in MIDI Pitch Bend with RPN pitch bend range setup.
- `yR,D` register writes are preserved in IR and reported in diagnostics by default without emitting SMF events.
- `--emit-config-skeleton` writes a YAML skeleton for observed `@`, `@e`, `@r`, and `y` usage.
- SMF Type 0/1 output, with Type 1 using a conductor track plus one MIDI track per source track, tempo/time/key/text meta events, note velocity from `v`, and envelope approximation via CC #11 by default.
- JSON diagnostics for warnings, unsupported commands, channel allocation, loop markers, track summaries, and envelope event counts.

Compatibility smoke outputs can be generated with:

```bash
bash scripts/validate_compat.sh
```

The script converts `tests/compat/*.mus` into `target/compat-smf/*.mid` plus diagnostics JSON for DAW/player checks.
