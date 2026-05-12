# `.smfmap.yaml` Guide

This document explains how to use the v0.3 `smfmap` configuration layer for `mgs2smf`.

`smfmap` is a per-song mapping file. It lets you keep the MUS file compatible with MGSDRV/MGSC111 while adding converter-only choices for MIDI output, such as Program Change mapping, CC events, manual MIDI events, and diagnostics for unsupported chip register writes.

## Quick Start

Generate a skeleton from the MUS file:

```bash
mgs2smf song.mus --emit-config-skeleton song.smfmap.yaml
```

Edit the generated YAML, then render with it:

```bash
mgs2smf song.mus -o song.mid --config song.smfmap.yaml
```

You can also generate a skeleton and render in the same run:

```bash
mgs2smf song.mus -o song.mid --emit-config-skeleton observed.smfmap.yaml
```

## Config Discovery

The converter loads built-in defaults first. Then it loads config files in this order:

1. Explicit `--config <path>` files, in the order given.
2. If no `--config` is given, discovered config files:
   `.smfmap.yaml`, an input-directory `.smfmap.yaml`, and `song.smfmap.yaml`.

Later settings override earlier settings.

Example:

```bash
mgs2smf song.mus -o song.mid \
  --config common.smfmap.yaml \
  --config song.smfmap.yaml
```

## Recommended Workflow

1. Run `--emit-config-skeleton`.
2. Edit only the observed tones, envelopes, and register writes.
3. Render with `--config`.
4. Inspect diagnostics JSON when something is unmapped or ignored.

```bash
mgs2smf song.mus --emit-config-skeleton song.smfmap.yaml
mgs2smf song.mus -o song.mid --config song.smfmap.yaml --diagnostics song.json
```

The skeleton includes only usage actually found in the MUS file:

- PSG envelope numbers from `@N`, `@eN`, and `@rN`.
- SCC tones from `@N`.
- OPLL tones from `@N`.
- Register writes from `yR,D`.

## SMF Settings

Use `smf` for file-level MIDI settings. CLI options such as `--ppq`, `--channel-overflow`, `--program-numbering`, and `--channel-numbering` override config values when both are present.

```yaml
smf:
  type: 1
  ppq: 3600
  program_numbering: zero_based
  channel_numbering: zero_based
  channel_allocation:
    overflow_policy: error
    rhythm_channel_zero_based: 9
```

## Track Families

`@N`, `@eN`, and `@rN` are interpreted by track family.

```yaml
tracks:
  "1": { family: psg,       midi_channel: 0  }
  "2": { family: psg,       midi_channel: 1  }
  "3": { family: psg_noise, midi_channel: 2  }
  "4": { family: scc,       midi_channel: 3  }
  "9": { family: opll,      midi_channel: 8  }
```

Default family behavior:

- `psg`: `@N`, `@eN`, and `@rN` select a PSG envelope.
- `psg_noise`: same as PSG by default. Drum mapping is optional and disabled by default.
- `scc`: `@N` selects a tone map entry. `@eN` and `@rN` select an envelope.
- `opll`: `@N` selects a tone map entry. `@eN` and `@rN` select an envelope.
- `rhythm`: `@` commands are ignored and reported.

Important: PSG `@N` is intentionally not routed to `tone_map`, so it will not produce Program Change events.

## Manual SMF Directives

Use `;@smf` comments in the MUS file to insert MIDI or meta events without breaking MGSDRV/MGSC compatibility.

```mml
9 ;@smf pc 80
9 ;@smf cc 74 100
9 c4 d4
```

When `;@smf` appears inside a track line, it targets the current source track by default.

Top-level directives normally specify `track=<id>` or `conductor`:

```mml
;@smf track=9 pc 80
;@smf conductor marker "A section"
```

Use `channel=<n>` to target a MIDI channel directly. A top-level channel-targeted directive is written to the conductor track. It can also be combined with `track=<id>` when you want the event written into a source track's SMF track but sent on a different MIDI channel.

```mml
;@smf channel=3 pc 80
;@smf track=9 channel=3 cc 74 100
```

Supported commands:

```text
pc <program>
bank <msb> <lsb>
cc <controller> <value>
pb <value>
rpn <msb> <lsb> <value>
nrpn <msb> <lsb> <value>
marker "text"
text "text"
macro <name>
reset [name]
```

Numbers may be decimal, `0x` hex, `$` hex, or `%` binary.

`smf.program_numbering` and `smf.channel_numbering` control whether YAML/manual program and channel numbers are treated as zero-based or one-based.

Examples:

```mml
9 ;@smf bank 0 32
9 ;@smf pc 0x50
9 ;@smf cc $4a 100
9 ;@smf pb -1200
9 ;@smf text "OPLL custom tone starts here"
```

## SMF Macros

Macros are named lists of SMF events. They are useful for repeated channel setup.

```yaml
smf_macros:
  bright_lead:
    events:
      - pc: { program: 80 }
      - cc: { controller: 7, value: 110 }
      - cc: { controller: 11, value: 127 }
      - cc: { controller: 74, value: 105 }
```

Use a macro from MUS:

```mml
9 ;@smf macro bright_lead
9 c4
```

`reset` expands `reset_channel` by default:

```mml
9 ;@smf reset
```

## SCC and OPLL Tone Map

Use `tone_map` to translate SCC/OPLL `@N` into MIDI events.

```yaml
tone_map:
  scc:
    enabled: true
    tones:
      "1":
        name: scc_lead
        events:
          - pc: { program: 81 }
          - cc: { controller: 74, value: 96 }

  opll:
    enabled: true
    tones:
      "9":
        name: synth_lead
        events:
          - bank: { msb: 0, lsb: 0 }
          - pc: { program: 80 }
          - cc: { controller: 91, value: 24 }
```

In the MUS file:

```mml
4 @1 c4
9 @9 c4
```

Existing skeletons or hand-written configs may use YAML inline maps:

```yaml
tone_map:
  scc:
    enabled: true
    tones:
      "8":  { name: "SCC @8",  events: [] }
      "16": { name: "SCC @16", events: [] }
      "24": { name: "SCC @24", events: [] }
```

This means the song used SCC `@8`, `@16`, and `@24`, but no MIDI events have been assigned yet. `events: []` is a placeholder.

The inline form above is equivalent to this block form:

```yaml
tone_map:
  scc:
    enabled: true
    tones:
      "8":
        name: "SCC @8"
        events: []
```

When editing `events`, prefer the block form below. Do not keep the surrounding `{ ... }` while moving `events` to multiple lines.

```yaml
tone_map:
  scc:
    enabled: true
    tones:
      "8":
        name: "SCC @8 lead"
        events:
          - pc: { program: 81 }
          - cc: { controller: 74, value: 96 }
      "16":
        name: "SCC @16 bass"
        events:
          - pc: { program: 38 }
```

Event order at the same tick follows the v0.3 policy: NoteOff, Bank, Program, RPN/NRPN, CC, PitchBend, initial expression, meta, NoteOn, envelope curve.

OPLL `@#N = M` assignments are honored when `respect_at_hash_rom_assign` is true:

```mml
@#2 = 14
9 @2 c4
```

```yaml
tone_map:
  opll:
    respect_at_hash_rom_assign: true
```

Here, MUS `@2` looks up `tone_map.opll.tones["14"]`.

## PSG Envelope Mapping

For PSG and PSG noise tracks, `@N`, `@eN`, and `@rN` select envelopes. The selected envelope restarts on each Note On and is rendered as expression CC by default.

```mml
@e1 = {1,0,f:2,8=4,[84]}
1 @1 c4 d4
```

This produces CC #11 envelope curves on the PSG track and does not produce Program Change.

Relevant config:

```yaml
psg_envelope_map:
  enabled: true
  undefined_policy: warn_and_no_envelope
  default:
    emit_expression_cc: true
    expression_cc: 11
    trigger:
      on_at_command: select_only
      on_note_on: restart_envelope
```

## PSG Noise Drum Map

PSG noise uses envelope CC output by default. Drum mapping exists in the schema for song-specific use, but it is disabled by default.

```yaml
psg_noise_map:
  enabled: true
  default_action: envelope_cc
  drum_map:
    enabled: false
```

Enable drum mapping when an envelope number should become a GM drum note instead of a melody note:

```yaml
psg_noise_map:
  default_action: drum_map
  drum_map:
    enabled: true
    channel: 9
    envelopes:
      "21":
        name: noise_snare
        note: 38
        velocity: 100
```

## SCC and OPLL Envelope Map

For SCC/OPLL, `@eN` and `@rN` select an envelope for subsequent notes. By default this is rendered as CC #11, but `envelope_map` can choose CC #7, note velocity, split velocity, or off.

```yaml
envelope_map:
  enabled: true
  default_mode: cc11
  midi_cc:
    expression: 11
    volume: 7
  families:
    scc: { enabled: true }
    opll: { enabled: true }
```

## Register Writes

MUS `yR,D` commands are preserved as register-write IR and reported in diagnostics by default.

```mml
9 y14,32 c4
```

Default behavior:

```yaml
opll_register_map:
  enabled: true
  default_policy: ignore_and_report
```

This emits no MIDI event by default. It is meant to make unsupported chip-specific behavior visible so you can decide whether to map it manually with `;@smf`, text events, or register rules.

To emit MIDI from matching register writes, set `default_policy: apply_rules` or use `--register-map on`:

```yaml
opll_register_map:
  default_policy: apply_rules
  rules:
    - name: y14_filter
      enabled: true
      match: { family: opll, source_track: "*", register: 14, data: 32, context: "*" }
      emit:
        - cc: { controller: 74, value: 100 }
```

Register writes inside `@e` definitions are routed with `context: EnvelopeData`.

## Diagnostics

Use diagnostics to audit how the config was applied:

```bash
mgs2smf song.mus -o song.mid --config song.smfmap.yaml --diagnostics song.json
```

Diagnostics include:

- Loaded config files.
- Manual SMF events.
- Tone usage.
- PSG envelope usage.
- Register write usage.
- Unmapped tones.
- Unsupported commands and warnings.

## CLI Overrides

Useful v0.3 options:

```text
--config <path>
--emit-config-skeleton <path>
--manual-smf <on|off>
--tone-map <on|off>
--psg-envelope-map <on|off>
--psg-noise-drum-map <on|off>
--register-map <off|report|on|error>
--program-numbering <zero|one>
--channel-numbering <zero|one>
```

`--register-map report` is the default behavior. `on` enables `apply_rules` and reports only writes with no matching rule.

## Minimal Example

`song.mus`:

```mml
@e1 = {1,0,f:2,8}
1 @1 c4
4 @1 c4
9 ;@smf macro bright_lead
9 @9 c4
9 y14,32 c4
```

`song.smfmap.yaml`:

```yaml
version: 0.3

tone_map:
  scc:
    tones:
      "1":
        events:
          - pc: { program: 81 }
  opll:
    tones:
      "9":
        events:
          - pc: { program: 80 }

smf_macros:
  bright_lead:
    events:
      - cc: { controller: 74, value: 105 }
      - cc: { controller: 91, value: 24 }
```

Render:

```bash
mgs2smf song.mus -o song.mid --config song.smfmap.yaml --diagnostics song.json
```
