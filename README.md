# mgsdrv-mus-to-smf

MGSDRV/MGSC111 `.MUS` MML text to Standard MIDI File Type 1 converter.

```bash
cargo run -- input.mus -o output.mid
```

The binary name is `mgs2smf`.

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
  --diagnostics output.json
```

Implemented MVP:

- UTF-8 / Shift-JIS loading via `encoding_rs`, auto fallback, CRLF/LF normalization, and `;` comment stripping.
- `#opll_mode`, `#tempo`, `#title`, `#macro_offset`, `#play_track`, `#end`.
- `@e` / `@r` envelope definitions, including tempo-map-aware 60 Hz CC timing and preservation of non-SMF `@e` commands in IR/diagnostics.
- `@mN={...}` control text definitions and `@mN` MML text meta output.
- Ignored `@s`, `@v`, `@#` tone definitions with diagnostics.
- `--tone-policy text_meta` preserves ignored tone definitions as MIDI text meta events.
- Source track lines `1..9`, `a..h`, `r`, with duplicated content for multi-track selectors.
- Notes, rests, octave, configurable `o4c`, default length, `%N`, dotted lengths, ties, slur with same-pitch merge, `t`, `v`, relative volume, `q`, `@N` tone designation, `@eN`, `@rN`, loops, and `!`.
- `@N` can render as GM Program Change with `--tone-policy gm_program` or as MIDI text meta with `--tone-policy text_meta`.
- Unsupported MML commands from the spec (`m`, `s`, `n`, `/`, `\`, `p`, `h`, `ho`, `hf`, `hi`, `_`, `y`, `so`, `sf`, `ko`, `kf`, `$`, `@p`, `@l`, `@\`, `@f`, `@o`) are ignored with classified diagnostics.
- `#opll_mode 1` rhythm tracks `f` / `r` with GM drum notes for `b`, `s`, `m`, `c`, `h` when `--rhythm-map gm` is active.
- `--loop-marker-name start:end` customizes infinite loop marker names.
- `--channel-overflow error|shared|multi_port`; `shared` reuses MIDI channels and suppresses CC envelopes on shared channels, while `multi_port` emits MIDI Port meta events.
- SMF Type 1 output with a conductor track, one MIDI track per source track, tempo/time/key/text meta events, note velocity from `v`, and envelope approximation via CC #11 by default.
- JSON diagnostics for warnings, unsupported commands, channel allocation, loop markers, track summaries, and envelope event counts.

Compatibility smoke outputs can be generated with:

```bash
bash scripts/validate_compat.sh
```

The script converts `tests/compat/*.mus` into `target/compat-smf/*.mid` plus diagnostics JSON for DAW/player checks.
