use crate::ir::{ConversionOptions, EnvelopeRef, NoteMeta, SongIr};
use crate::smfmap::{
    DrumOutputMode, DrumUnmatchedPolicy, DrumVelocity, OpllPseudoDrumContext, OpllPseudoDrumMap,
};

use super::{PRI_NOTE_OFF, PRI_NOTE_ON, RenderedEvent, RenderedEventKind};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum OpllPseudoDrumRenderResult {
    Replaced,
    Added,
    Dropped,
    Passthrough,
}

#[derive(Default)]
pub(super) struct OpllPseudoDrumRenderState {
    macro_hit_active: bool,
}

impl OpllPseudoDrumRenderState {
    pub(super) fn reset(&mut self) {
        self.macro_hit_active = false;
    }

    fn should_suppress(&mut self, map: &OpllPseudoDrumMap, meta: &NoteMeta) -> bool {
        if !map.hit_grouping.enabled {
            return false;
        }
        if meta.macro_call_start {
            self.macro_hit_active = false;
        }
        if map.hit_grouping.suppress_slur_ampersand && meta.slur_from_previous {
            return true;
        }
        map.hit_grouping.suppress_macro_continuation
            && !meta.macro_origin.is_empty()
            && self.macro_hit_active
    }

    fn mark_emitted(&mut self, map: &OpllPseudoDrumMap, meta: &NoteMeta) {
        if map.hit_grouping.enabled && !meta.macro_origin.is_empty() {
            self.macro_hit_active = true;
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn render_opll_pseudo_drum_note(
    events: &mut Vec<RenderedEvent>,
    midi_track: usize,
    start_tick: u64,
    end_tick: u64,
    source_track: &str,
    source_tone: Option<u8>,
    envelope: Option<EnvelopeRef>,
    source_midi_note: u8,
    source_velocity: u8,
    meta: &NoteMeta,
    state: &mut OpllPseudoDrumRenderState,
    map: &OpllPseudoDrumMap,
    options: &ConversionOptions,
    song: &mut SongIr,
) -> OpllPseudoDrumRenderResult {
    if map.output.mode == DrumOutputMode::Passthrough {
        return OpllPseudoDrumRenderResult::Passthrough;
    }
    if state.should_suppress(map, meta) {
        return match map.output.mode {
            DrumOutputMode::Replace => OpllPseudoDrumRenderResult::Dropped,
            DrumOutputMode::Add | DrumOutputMode::Passthrough => {
                OpllPseudoDrumRenderResult::Passthrough
            }
        };
    }

    let effective_tone = source_tone.map(|tone| {
        if options.smfmap.opll_respect_at_hash_rom_assign {
            song.opll_tone_assignments
                .get(&tone)
                .copied()
                .unwrap_or(tone)
        } else {
            tone
        }
    });
    let envelope_number = envelope.map(|reference| match reference {
        EnvelopeRef::E(id) | EnvelopeRef::R(id) => id,
    });
    let macro_symbol = meta.macro_origin.last().and_then(|origin| origin.symbol);
    let context = OpllPseudoDrumContext {
        source_track,
        macro_symbol,
        envelope: envelope_number,
        source_tone,
        effective_tone,
        note_name: meta.note_name,
        octave: meta.source_octave,
        source_midi_note: meta.source_midi_note.or(Some(source_midi_note)),
    };

    if let Some(rule) = options.smfmap.opll_pseudo_drum_for_note(map, &context) {
        state.mark_emitted(map, meta);
        let channel = map
            .output
            .midi_channel
            .unwrap_or(options.smfmap.rhythm_channel)
            .min(15);
        let velocity = match rule.drum.velocity {
            DrumVelocity::SourceVolume | DrumVelocity::SourceOrTone => source_velocity,
            DrumVelocity::Fixed(value) => value,
        };
        events.push(RenderedEvent::channel(
            midi_track,
            start_tick,
            PRI_NOTE_ON,
            RenderedEventKind::NoteOn {
                channel,
                note: rule.drum.note,
                velocity,
            },
        ));
        events.push(RenderedEvent::channel(
            midi_track,
            end_tick,
            PRI_NOTE_OFF,
            RenderedEventKind::NoteOff {
                channel,
                note: rule.drum.note,
                velocity: 0,
            },
        ));
        return match map.output.mode {
            DrumOutputMode::Replace => OpllPseudoDrumRenderResult::Replaced,
            DrumOutputMode::Add => OpllPseudoDrumRenderResult::Added,
            DrumOutputMode::Passthrough => OpllPseudoDrumRenderResult::Passthrough,
        };
    }

    if matches!(
        map.output.unmatched,
        DrumUnmatchedPolicy::WarnAndDrop | DrumUnmatchedPolicy::WarnAndPassthrough
    ) {
        song.diagnostics.add_unsupported(
            None,
            Some(source_track.to_string()),
            "opll pseudo drum unmatched note",
            "opll_pseudo_drum_map unmatched rule",
        );
    }
    match map.output.unmatched {
        DrumUnmatchedPolicy::WarnAndDrop | DrumUnmatchedPolicy::Drop => {
            OpllPseudoDrumRenderResult::Dropped
        }
        DrumUnmatchedPolicy::Passthrough | DrumUnmatchedPolicy::WarnAndPassthrough => {
            OpllPseudoDrumRenderResult::Passthrough
        }
    }
}
