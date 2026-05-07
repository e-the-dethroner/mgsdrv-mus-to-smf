# Compatibility Smoke Corpus

Run:

```bash
bash scripts/validate_compat.sh
```

The script writes `.mid` and diagnostics `.json` files to `target/compat-smf`.

Manual DAW/player checks:

- The file opens as SMF Type 1 with separate source tracks.
- Track names, default 4/4 time signature, C major key signature, tempo events, and text meta are visible where the player exposes them.
- `envelope_tempo_change.mid` keeps CC #11 envelope timing stable across the tempo change.
- `loop_marker.mid` exposes loopStart/loopEnd markers and finite unroll notes.
- `rhythm_loop.mid` maps the rhythm track to GM drum channel 10.
- `multi_port_overflow.mid` exposes MIDI Port meta events for tracks past the first 15 melodic channels.
