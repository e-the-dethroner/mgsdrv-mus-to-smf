use crate::diagnostics::Diagnostics;
use crate::ir::{ConversionOptions, IrEvent, NoteMeta, PitchGlideMeta, TrackIr};
use crate::smfmap::PitchGlideRangePolicy;

use super::{PRI_PITCH_BEND, PRI_RPN_NRPN, RenderedEvent, RenderedEventKind, tempo_at_tick};

#[derive(Clone, Copy)]
pub(super) struct PitchGlideGroupSegment<'a> {
    pub(super) event_index: usize,
    pub(super) start_tick: u64,
    pub(super) end_tick: u64,
    pub(super) note: u8,
    pub(super) meta: &'a NoteMeta,
}

pub(super) fn collect_pitch_glide_slur_group<'a>(
    events: &'a [IrEvent],
    start_index: usize,
    ppq: u16,
) -> Option<Vec<PitchGlideGroupSegment<'a>>> {
    let mut index = start_index;
    let mut group = Vec::new();
    let mut has_pitch_glide = false;

    while let Some(event) = events.get(index) {
        let IrEvent::Note {
            start_steps,
            duration_steps,
            note,
            meta,
            ..
        } = event
        else {
            break;
        };
        if index != start_index && !meta.slur_from_previous {
            break;
        }
        has_pitch_glide |= meta.pitch_glide.is_some();
        let start_tick = start_steps.to_midi_ticks(ppq);
        let end_tick = start_steps.add(*duration_steps).to_midi_ticks(ppq);
        group.push(PitchGlideGroupSegment {
            event_index: index,
            start_tick,
            end_tick,
            note: *note,
            meta,
        });
        if !meta.slur_to_next {
            break;
        }
        index += 1;
    }

    if group.len() > 1 && has_pitch_glide {
        Some(group)
    } else {
        None
    }
}

pub(super) fn emit_pitch_bend_range_setup(
    events: &mut Vec<RenderedEvent>,
    midi_track: usize,
    channel: u8,
    semitones: u8,
    cents: u8,
    emit_null: bool,
) {
    for (controller, value) in [
        (101, 0),
        (100, 0),
        (6, semitones.min(127)),
        (38, cents.min(127)),
    ] {
        events.push(RenderedEvent::channel(
            midi_track,
            0,
            PRI_RPN_NRPN,
            RenderedEventKind::ControlChange {
                channel,
                controller,
                value,
            },
        ));
    }
    if emit_null {
        for controller in [101, 100] {
            events.push(RenderedEvent::channel(
                midi_track,
                0,
                PRI_RPN_NRPN,
                RenderedEventKind::ControlChange {
                    channel,
                    controller,
                    value: 127,
                },
            ));
        }
    }
    events.push(RenderedEvent::channel(
        midi_track,
        0,
        PRI_PITCH_BEND,
        RenderedEventKind::PitchBend {
            channel,
            value: 8192,
        },
    ));
}

pub(super) fn effective_pitch_glide_range(
    track: &TrackIr,
    options: &ConversionOptions,
) -> (u8, u8) {
    let config = &options.smfmap.pitch_glide;
    if config.range_policy != PitchGlideRangePolicy::AutoExpandTo48 {
        return (
            config.pitch_bend_range_semitones,
            config.pitch_bend_range_cents,
        );
    }

    let configured_cents =
        config.pitch_bend_range_semitones as i32 * 100 + config.pitch_bend_range_cents as i32;
    let mut max_delta_cents = track
        .events
        .iter()
        .filter_map(|event| match event {
            IrEvent::Note { note, meta, .. } => meta
                .pitch_glide
                .as_ref()
                .map(|glide| (glide.target_midi_note as i32 - *note as i32).abs() * 100),
            _ => None,
        })
        .max()
        .unwrap_or(0);
    for start_index in 0..track.events.len() {
        let Some(IrEvent::Note { note, meta, .. }) = track.events.get(start_index) else {
            continue;
        };
        if !meta.slur_to_next {
            continue;
        }
        let origin_note = *note as i32;
        let mut index = start_index;
        let mut has_pitch_glide = false;
        while let Some(IrEvent::Note { note, meta, .. }) = track.events.get(index) {
            if index != start_index && !meta.slur_from_previous {
                break;
            }
            has_pitch_glide |= meta.pitch_glide.is_some();
            if has_pitch_glide {
                max_delta_cents = max_delta_cents
                    .max((*note as i32 - origin_note).abs() * 100)
                    .max(
                        meta.pitch_glide
                            .as_ref()
                            .map(|glide| (glide.target_midi_note as i32 - origin_note).abs() * 100)
                            .unwrap_or(0),
                    );
            }
            if !meta.slur_to_next {
                break;
            }
            index += 1;
        }
    }
    if max_delta_cents <= configured_cents {
        return (
            config.pitch_bend_range_semitones,
            config.pitch_bend_range_cents,
        );
    }
    let semitones = ((max_delta_cents + 99) / 100).clamp(1, 48) as u8;
    (semitones, 0)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn render_pitch_glide_curve(
    events: &mut Vec<RenderedEvent>,
    midi_track: usize,
    start_tick: u64,
    end_tick: u64,
    channel: u8,
    source_note: u8,
    glide: &PitchGlideMeta,
    range_semitones: u8,
    range_cents: u8,
    tempo_points: &[(u64, u32)],
    ppq: u16,
    track_id: &str,
    options: &ConversionOptions,
    diagnostics: &mut Diagnostics,
) -> Result<(), String> {
    let total_delta_cents = (glide.target_midi_note as i32 - source_note as i32) * 100;
    render_pitch_bend_curve_segment(
        events,
        midi_track,
        start_tick,
        end_tick,
        channel,
        0,
        total_delta_cents,
        &glide.source,
        range_semitones,
        range_cents,
        tempo_points,
        ppq,
        track_id,
        options,
        diagnostics,
    )?;
    if options.smfmap.pitch_glide.reset.at_note_end {
        events.push(RenderedEvent::channel(
            midi_track,
            end_tick,
            PRI_PITCH_BEND,
            RenderedEventKind::PitchBend {
                channel,
                value: 8192,
            },
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn render_pitch_glide_group_curve(
    events: &mut Vec<RenderedEvent>,
    midi_track: usize,
    channel: u8,
    group: &[PitchGlideGroupSegment<'_>],
    range_semitones: u8,
    range_cents: u8,
    tempo_points: &[(u64, u32)],
    ppq: u16,
    track_id: &str,
    options: &ConversionOptions,
    diagnostics: &mut Diagnostics,
) -> Result<(), String> {
    let Some(first) = group.first() else {
        return Ok(());
    };
    let origin_note = first.note as i32;
    for segment in group {
        let start_cents = (segment.note as i32 - origin_note) * 100;
        let (target_note, source) = segment
            .meta
            .pitch_glide
            .as_ref()
            .map(|glide| (glide.target_midi_note, glide.source.as_str()))
            .unwrap_or((segment.note, "slur pitch segment"));
        let target_cents = (target_note as i32 - origin_note) * 100;
        render_pitch_bend_curve_segment(
            events,
            midi_track,
            segment.start_tick,
            segment.end_tick,
            channel,
            start_cents,
            target_cents,
            source,
            range_semitones,
            range_cents,
            tempo_points,
            ppq,
            track_id,
            options,
            diagnostics,
        )?;
    }
    if options.smfmap.pitch_glide.reset.at_note_end
        && let Some(last) = group.last()
    {
        events.push(RenderedEvent::channel(
            midi_track,
            last.end_tick,
            PRI_PITCH_BEND,
            RenderedEventKind::PitchBend {
                channel,
                value: 8192,
            },
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn render_pitch_bend_curve_segment(
    events: &mut Vec<RenderedEvent>,
    midi_track: usize,
    start_tick: u64,
    end_tick: u64,
    channel: u8,
    start_delta_cents: i32,
    target_delta_cents: i32,
    source: &str,
    range_semitones: u8,
    range_cents: u8,
    tempo_points: &[(u64, u32)],
    ppq: u16,
    track_id: &str,
    options: &ConversionOptions,
    diagnostics: &mut Diagnostics,
) -> Result<(), String> {
    let bend_range_cents = (range_semitones.max(1) as i32 * 100 + range_cents as i32).max(1);
    let max_delta_cents = start_delta_cents.abs().max(target_delta_cents.abs());
    if max_delta_cents > bend_range_cents {
        match options.smfmap.pitch_glide.range_policy {
            PitchGlideRangePolicy::ErrorIfExceeded => {
                return Err(format!(
                    "pitch_glide range exceeded on track {track_id}: {} requires {} cents, configured range is {} cents",
                    source, max_delta_cents, bend_range_cents
                ));
            }
            PitchGlideRangePolicy::Clamp | PitchGlideRangePolicy::AutoExpandTo48 => {
                diagnostics.add_unsupported(
                    None,
                    Some(track_id.to_string()),
                    source.to_string(),
                    "pitch_glide: out of range clamped to configured pitch bend range",
                );
            }
        }
    }

    let duration_ticks = end_tick.saturating_sub(start_tick).max(1);
    let bpm = tempo_at_tick(tempo_points, start_tick).max(1);
    let seconds = duration_ticks as f64 * 60.0 / (ppq as f64 * bpm as f64);
    let event_rate = options.smfmap.pitch_glide.curve.event_rate_hz.max(1) as f64;
    let steps = (seconds * event_rate).round().max(1.0) as u64;
    let last_curve_tick = end_tick.saturating_sub(1).max(start_tick);
    let curve_span = last_curve_tick.saturating_sub(start_tick);
    let min_delta_cents = options.smfmap.pitch_glide.curve.min_delta_cents as i32;
    let mut last_emitted_cents: Option<i32> = None;
    let mut last_event_key: Option<(u64, u16)> = None;

    let start_index = if options.smfmap.pitch_glide.curve.include_start_point {
        0
    } else {
        1
    };
    let end_index = if options.smfmap.pitch_glide.curve.include_end_point {
        steps
    } else {
        steps.saturating_sub(1)
    };

    if options.smfmap.pitch_glide.reset.before_next_note_on
        && !options.smfmap.pitch_glide.curve.include_start_point
        && start_delta_cents == 0
    {
        events.push(RenderedEvent::channel(
            midi_track,
            start_tick,
            PRI_PITCH_BEND,
            RenderedEventKind::PitchBend {
                channel,
                value: 8192,
            },
        ));
    }

    for index in start_index..=end_index {
        let cents = start_delta_cents
            + (((target_delta_cents - start_delta_cents) as i64 * index as i64) / steps as i64)
                as i32;
        let is_endpoint = index == 0 || index == steps;
        if !is_endpoint
            && let Some(last) = last_emitted_cents
            && (cents - last).abs() < min_delta_cents
        {
            continue;
        }
        let tick = start_tick + (curve_span * index / steps);
        let value = pitch_bend_value_from_cents(cents, bend_range_cents);
        if last_event_key == Some((tick, value)) {
            continue;
        }
        events.push(RenderedEvent::channel(
            midi_track,
            tick,
            PRI_PITCH_BEND,
            RenderedEventKind::PitchBend { channel, value },
        ));
        last_emitted_cents = Some(cents);
        last_event_key = Some((tick, value));
    }
    Ok(())
}

fn pitch_bend_value_from_cents(delta_cents: i32, range_cents: i32) -> u16 {
    let signed = ((delta_cents as f64 / range_cents as f64) * 8192.0).round() as i32;
    (8192 + signed.clamp(-8192, 8191)).clamp(0, 16383) as u16
}
