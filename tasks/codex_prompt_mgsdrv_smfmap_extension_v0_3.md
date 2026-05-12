# Codex Prompt: implement MGSDRV MUS → SMF config extension v0.3

Implement the v0.3 configuration extension for the existing `mgs2smf` converter.

Use these files as source of truth:

- `mgsdrv_mus_to_smf_extension_spec_v0_3.md`
- `mgsdrv_smfmap_config_schema_v0_3.yaml`
- `mgsdrv_smfmap_default_config_v0_3.yaml`

## Main goals

1. Add `.smfmap.yaml` config loading.
2. Add `;@smf` manual SMF directives.
3. Make `@N` / `@eN` / `@rN` family-aware:
   - PSG / PSG noise: route to PSG envelope selection.
   - SCC / OPLL: `@N` routes to tone_map; `@eN` / `@rN` routes to envelope_map.
4. Ensure PSG `@N` is never converted to Program Change by tone_map.
5. Add SCC/OPLL tone_map events: `pc`, `bank`, `cc`, `pb`, `rpn`, `nrpn`, `marker`, `text`, `macro`.
6. Parse `yR,D` as `RegisterWrite` IR. Default behavior is ignore + report. Add rule-based emission later if time permits.
7. Add `--emit-config-skeleton` to generate a YAML skeleton from actually observed `@`, `@e`, `@r`, and `y` usage.

## Required parser behavior

- Do not discard comments before checking for `;@smf`.
- A normal comment remains ignored.
- A comment whose trimmed body starts with `@smf` must be parsed as `ManualSmf`.
- `;@smf` inside a track line targets the current source track by default.
- Top-level `;@smf` requires `track=<id>` or `conductor` unless an explicit default is configured.

## Required IR additions

Add or equivalent:

```rust
enum IrEvent {
    AtCommand {
        source_track: TrackId,
        source_step: SourceStep,
        family: SourceFamily,
        number: u8,
        spelling: AtSpelling,
        context: AtContext,
        source_span: SourceSpan,
    },
    PsgEnvelopeSelect { source_track: TrackId, source_step: SourceStep, envelope_number: u8, kind: EnvelopeKind },
    ToneChange { source_track: TrackId, source_step: SourceStep, family: SourceFamily, tone_number: u8 },
    EnvelopeApply { source_track: TrackId, source_step: SourceStep, family: SourceFamily, envelope_number: u8, kind: EnvelopeKind },
    RegisterWrite { source_track: TrackId, source_step: SourceStep, family: SourceFamily, register: u8, data: u8, context: RegisterContext, source_span: SourceSpan },
    ManualSmf { source_track: Option<TrackId>, source_step: SourceStep, request: SmfRequest, source_span: SourceSpan },
}
```

## Required tests

1. `;@smf pc 80` produces Program Change before the next note.
2. `;@smf macro bright` expands events from YAML.
3. PSG track `1 @1 c4` with `@e1` defined produces CC#11 envelope and no Program Change.
4. SCC track `4 @1 c4` with `tone_map.scc.tones.1.pc=81` produces Program Change 81.
5. OPLL `9 y14,32 c4` is reported in diagnostics and emits no SMF event by default.
6. `--emit-config-skeleton` includes observed PSG envelope numbers, SCC/OPLL tones, and register writes.

## Default policies

- Manual SMF: enabled.
- PSG envelope map: enabled.
- PSG noise drum map: disabled.
- Tone map: enabled only for SCC/OPLL.
- OPLL register map: `ignore_and_report`.
- Event order: NoteOff, Bank, Program, RPN/NRPN, CC, PitchBend, InitialExpression, Meta, NoteOn, EnvelopeCurve.
