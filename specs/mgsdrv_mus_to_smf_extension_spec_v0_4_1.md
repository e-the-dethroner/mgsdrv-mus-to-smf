# MGSDRV MUS to SMF Extension Spec v0.4.1

v0.4.1 extends v0.4 with opt-in rendering of MGSC111 `_target` pitch glide as MIDI Pitch Bend.

## Compatibility

- `pitch_glide.enabled` defaults to `false`.
- When disabled, `_target` continues to affect timing and target octave state as before, but is reported as unsupported source behavior.
- When enabled, `_target` is rendered only on tracks whose MIDI channel is safe according to `shared_channel_policy`.
- Existing v0.4 OPLL pseudo drum behavior is unchanged.

## Config

```yaml
version: 0.4.1

pitch_glide:
  enabled: false
  output: pitch_bend
  pitch_bend_range_semitones: 24
  pitch_bend_range_cents: 0
  emit_rpn_pitch_bend_range: true
  emit_rpn_null_after_setting: true
  curve:
    shape: linear
    event_rate_hz: 60
    min_delta_cents: 4
    include_start_point: true
    include_end_point: true
  reset:
    at_note_end: true
    before_next_note_on: true
    at_track_end: true
  shared_channel_policy: warn_and_suppress
  range_policy: clamp
```

Allowed values:

- `output`: `pitch_bend`
- `shared_channel_policy`: `error`, `warn_and_suppress`, `allow_unsafe`
- `range_policy`: `clamp`, `error_if_exceeded`, `auto_expand_to_48`
- `curve.shape`: `linear`

## Rendering

For each note with `_target`, the renderer keeps the source note number as the MIDI Note On note and emits a Pitch Bend curve to the target note.

```text
delta_cents = (target_midi_note - source_midi_note) * 100
bend_signed = round(delta_cents / range_cents * 8192)
midi_pitch_bend_value = clamp(8192 + bend_signed, 0, 16383)
```

The final curve point is emitted before Note Off when possible. If pitch-glide notes are joined by `&`, the renderer holds the first Note On across the glide group and emits Pitch Bend segments for each slurred source segment. If `reset.at_note_end` is true, Pitch Bend center (`8192`) is emitted at the end of the note or glide group. If `reset.at_track_end` is true, Pitch Bend center is also emitted at track end.

When `emit_rpn_pitch_bend_range` is true, channels that actually render pitch glide receive:

```text
CC 101, 0
CC 100, 0
CC   6, <semitones>
CC  38, <cents>
CC 101, 127
CC 100, 127
Pitch Bend center
```

The RPN null pair is omitted only when `emit_rpn_null_after_setting` is false.

## Shared Channel Policy

Pitch Bend is MIDI channel state. If a source track with pitch glide shares the same MIDI port/channel with another source track:

- `error`: conversion fails.
- `warn_and_suppress`: pitch glide for that source track is suppressed and diagnostics report it.
- `allow_unsafe`: pitch glide is emitted even though it may bend other notes on the same channel.

## Recommended `lov.mus` SCC Setup

```yaml
version: 0.4.1

tracks:
  "6": { source_family: scc, midi_channel: 3, midi_port: 0 }
  "7": { source_family: scc, midi_channel: 4, midi_port: 0 }
  "8": { source_family: scc, midi_channel: 5, midi_port: 0 }

pitch_glide:
  enabled: true
  output: pitch_bend
  pitch_bend_range_semitones: 24
  emit_rpn_pitch_bend_range: true
  emit_rpn_null_after_setting: true
  shared_channel_policy: error
  range_policy: auto_expand_to_48
```

## Deferred

- Exact SCC wave morphing from tone changes inside `@eN`.
- CC/filter/brightness approximation for SCC envelope tone changes.
