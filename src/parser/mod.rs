pub mod loader;

use std::collections::{BTreeMap, BTreeSet};

use crate::diagnostics::Diagnostics;
use crate::ir::{
    ControlKind, ConversionOptions, ECommand, EnvelopeE, EnvelopeR, EnvelopeRef, IrEvent,
    LoopMarkerKind, Rational, RhythmMap, SongIr, TempoEvent, TrackIr, TrackKind, apply_dots,
    denominator_to_steps, velocity_from_source,
};

pub fn parse_source(text: &str, options: &ConversionOptions) -> Result<SongIr, String> {
    let mut song = SongIr::new(options.ppq);
    let mut macros: BTreeMap<u32, String> = BTreeMap::new();
    let mut track_chunks: Vec<(String, usize, String, i32, TrackKind)> = Vec::new();
    let mut macro_offset = 0i32;
    let mut opll_mode = 0i32;
    let mut play_track_filter: Option<BTreeSet<String>> = None;

    for (line, statement) in collect_statements(text) {
        let trimmed = statement.trim();
        if trimmed.is_empty() {
            continue;
        }
        let lower = trimmed.to_ascii_lowercase();
        if lower.starts_with("#end") {
            break;
        }
        if lower.starts_with('#') {
            parse_directive(
                trimmed,
                line,
                &mut macro_offset,
                &mut opll_mode,
                &mut play_track_filter,
                &mut song,
            );
            continue;
        }
        if lower.starts_with("@e") {
            parse_e_definition(trimmed, line, &mut song)?;
            continue;
        }
        if lower.starts_with("@r") {
            parse_r_definition(trimmed, line, &mut song)?;
            continue;
        }
        if lower.starts_with("@m") {
            parse_m_definition(trimmed, line, &mut song)?;
            continue;
        }
        if lower.starts_with("@s") {
            record_ignored_tone_definition(trimmed, line, "@s", &mut song.diagnostics);
            continue;
        }
        if lower.starts_with("@v") {
            record_ignored_tone_definition(trimmed, line, "@v", &mut song.diagnostics);
            continue;
        }
        if lower.starts_with("@#") {
            record_ignored_tone_definition(trimmed, line, "@#", &mut song.diagnostics);
            continue;
        }
        if lower.starts_with('*') {
            parse_macro_definition(
                trimmed,
                line,
                macro_offset,
                &mut macros,
                &mut song.diagnostics,
            );
            continue;
        }
        if let Some((selectors, body)) = parse_track_line(trimmed) {
            for selector in selectors {
                let kind = track_kind_for_selector(&selector, opll_mode, options.rhythm_map);
                track_chunks.push((selector, line, body.to_string(), macro_offset, kind));
            }
            continue;
        }

        song.diagnostics.add_parse_warning(
            Some(line),
            None,
            format!("unrecognized top-level statement: {trimmed}"),
        );
    }

    let mut order = Vec::new();
    let mut by_track: BTreeMap<String, Vec<(usize, String, i32, TrackKind)>> = BTreeMap::new();
    let mut kind_by_track: BTreeMap<String, TrackKind> = BTreeMap::new();
    let mut seen = BTreeSet::new();
    for (track, line, body, macro_offset, kind) in track_chunks {
        if let Some(filter) = &play_track_filter {
            if !filter.contains(&track) {
                continue;
            }
        }
        if seen.insert(track.clone()) {
            order.push(track.clone());
        }
        kind_by_track.entry(track.clone()).or_insert(kind);
        by_track
            .entry(track)
            .or_default()
            .push((line, body, macro_offset, kind));
    }

    for track_id in order {
        let kind = kind_by_track
            .get(&track_id)
            .copied()
            .unwrap_or(TrackKind::Melodic);
        let mut track = TrackIr::new_with_kind(track_id.clone(), kind);
        let mut state = MmlState::new(options.octave_base);
        if let Some(chunks) = by_track.get(&track_id) {
            for (line, body, macro_offset, kind) in chunks {
                let mut ctx = MmlContext {
                    macros: &macros,
                    macro_offset: *macro_offset,
                    track_kind: *kind,
                    loop_count: options.loop_count,
                    tempo_events: &mut song.tempo_events,
                    diagnostics: &mut song.diagnostics,
                    track_id: &track_id,
                    line: *line,
                };
                parse_mml(body, &mut state, &mut track, &mut ctx, 0)?;
            }
        }
        track.length_steps = state.cursor;
        song.tracks.push(track);
    }

    if song.tempo_events.is_empty() {
        song.tempo_events.push(TempoEvent {
            at_steps: Rational::ZERO,
            bpm: 120,
            source: "default".to_string(),
            line: None,
        });
    }

    Ok(song)
}

fn collect_statements(text: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut start_line = 1;
    let mut brace_depth = 0i32;

    for (idx, line) in text.lines().enumerate() {
        let line_no = idx + 1;
        if current.is_empty() {
            start_line = line_no;
        } else {
            current.push('\n');
        }
        current.push_str(line);
        brace_depth += brace_delta(line);
        if brace_depth <= 0 {
            out.push((start_line, current.clone()));
            current.clear();
            brace_depth = 0;
        }
    }
    if !current.trim().is_empty() {
        out.push((start_line, current));
    }
    out
}

fn brace_delta(line: &str) -> i32 {
    let mut delta = 0;
    let mut in_quote = false;
    for ch in line.chars() {
        match ch {
            '"' => in_quote = !in_quote,
            '{' if !in_quote => delta += 1,
            '}' if !in_quote => delta -= 1,
            _ => {}
        }
    }
    delta
}

fn parse_directive(
    line: &str,
    line_no: usize,
    macro_offset: &mut i32,
    opll_mode: &mut i32,
    play_track_filter: &mut Option<BTreeSet<String>>,
    song: &mut SongIr,
) {
    let mut parts = line.splitn(2, char::is_whitespace);
    let name = parts.next().unwrap_or_default().to_ascii_lowercase();
    let rest = parts.next().unwrap_or_default().trim();
    match name.as_str() {
        "#tempo" => {
            if let Some(bpm) = first_u32(rest) {
                song.tempo_events.push(TempoEvent {
                    at_steps: Rational::ZERO,
                    bpm,
                    source: "#tempo".to_string(),
                    line: Some(line_no),
                });
            } else {
                song.diagnostics
                    .add_parse_warning(Some(line_no), None, "#tempo without BPM");
            }
        }
        "#title" => {
            song.title = Some(unquote(rest));
        }
        "#macro_offset" => {
            if let Some(value) = first_i32(rest) {
                *macro_offset = value;
            } else {
                song.diagnostics.add_parse_warning(
                    Some(line_no),
                    None,
                    "#macro_offset without value",
                );
            }
        }
        "#opll_mode" => {
            if let Some(value) = first_i32(rest) {
                *opll_mode = value;
            } else {
                song.diagnostics
                    .add_parse_warning(Some(line_no), None, "#opll_mode without value");
            }
        }
        "#play_track" => {
            let tracks = parse_play_track_selectors(rest);
            if tracks.is_empty() {
                song.diagnostics.add_parse_warning(
                    Some(line_no),
                    None,
                    "#play_track without track selectors",
                );
            } else {
                *play_track_filter = Some(tracks);
            }
        }
        "#end" => {}
        _ => song
            .diagnostics
            .add_unsupported(Some(line_no), None, name, "ignored directive"),
    }
}

fn parse_e_definition(line: &str, line_no: usize, song: &mut SongIr) -> Result<(), String> {
    let (id, content) = parse_at_definition(line, "@e")
        .ok_or_else(|| format!("invalid @e definition at line {line_no}"))?;
    let mut parser = ListParser::new(&content);
    let mode = parser.next_i32().unwrap_or(0);
    let noise = parser.next_i32().unwrap_or(0);
    let commands = parse_e_commands(parser.rest(), line_no, &mut song.diagnostics);
    song.envelopes.e.insert(
        id,
        EnvelopeE {
            id,
            mode,
            noise,
            commands,
        },
    );
    Ok(())
}

fn parse_r_definition(line: &str, line_no: usize, song: &mut SongIr) -> Result<(), String> {
    let (id, content) = parse_at_definition(line, "@r")
        .ok_or_else(|| format!("invalid @r definition at line {line_no}"))?;
    let mut parser = ListParser::new(&content);
    let mode = parser.next_i32().unwrap_or(0);
    let noise = parser.next_i32().unwrap_or(0);
    let values = [
        parser.next_i32().unwrap_or(0),
        parser.next_i32().unwrap_or(0),
        parser.next_i32().unwrap_or(0),
        parser.next_i32().unwrap_or(0),
        parser.next_i32().unwrap_or(0),
        parser.next_i32().unwrap_or(0),
    ];
    song.envelopes.r.insert(
        id,
        EnvelopeR {
            id,
            mode,
            noise,
            al: values[0].clamp(0, 255) as u8,
            ar: values[1].clamp(0, 255) as u8,
            dr: values[2].clamp(0, 255) as u8,
            sl: values[3].clamp(0, 255) as u8,
            sr: values[4].clamp(0, 255) as u8,
            rr: values[5].clamp(0, 255) as u8,
        },
    );
    Ok(())
}

fn parse_m_definition(line: &str, line_no: usize, song: &mut SongIr) -> Result<(), String> {
    let (id, content) = parse_at_definition(line, "@m")
        .ok_or_else(|| format!("invalid @m definition at line {line_no}"))?;
    song.control_texts.insert(id, unquote(content.trim()));
    Ok(())
}

fn parse_at_definition(line: &str, prefix: &str) -> Option<(u8, String)> {
    let lower = line.to_ascii_lowercase();
    if !lower.starts_with(prefix) {
        return None;
    }
    let chars: Vec<char> = line.chars().collect();
    let mut pos = prefix.len();
    let id = parse_u32_chars(&chars, &mut pos)? as u8;
    skip_ws(&chars, &mut pos);
    if chars.get(pos) == Some(&'=') {
        pos += 1;
    }
    skip_ws(&chars, &mut pos);
    if chars.get(pos) != Some(&'{') {
        return None;
    }
    let start = pos + 1;
    let mut end = chars.len();
    for i in (start..chars.len()).rev() {
        if chars[i] == '}' {
            end = i;
            break;
        }
    }
    Some((id, chars[start..end].iter().collect()))
}

fn parse_e_commands(input: &str, line_no: usize, diagnostics: &mut Diagnostics) -> Vec<ECommand> {
    let chars: Vec<char> = input.chars().collect();
    let mut pos = 0;
    let mut commands = Vec::new();
    while pos < chars.len() {
        skip_ws_and_separators(&chars, &mut pos);
        if pos >= chars.len() {
            break;
        }
        let ch = chars[pos];
        match ch {
            '[' => {
                commands.push(ECommand::LoopStart);
                pos += 1;
            }
            ']' => {
                commands.push(ECommand::LoopEnd);
                pos += 1;
            }
            '0'..='9' | 'a'..='f' | 'A'..='F' => {
                let level = ch.to_digit(16).unwrap_or(0).min(15) as u8;
                pos += 1;
                if chars.get(pos) == Some(&':') {
                    pos += 1;
                    let count = parse_u32_chars(&chars, &mut pos).unwrap_or(1).max(1);
                    commands.push(ECommand::Hold { level, count });
                } else if chars.get(pos) == Some(&'=') {
                    pos += 1;
                    let count = parse_u32_chars(&chars, &mut pos).unwrap_or(1).max(1);
                    commands.push(ECommand::Ramp {
                        target: level,
                        count,
                    });
                } else {
                    commands.push(ECommand::Level(level));
                }
            }
            '@' => {
                pos += 1;
                if let Some(tone) = parse_u32_chars(&chars, &mut pos) {
                    commands.push(ECommand::ToneChange(tone.min(255) as u8));
                } else {
                    diagnostics.add_parse_warning(
                        Some(line_no),
                        None,
                        "@e tone change command without tone number",
                    );
                }
            }
            'n' => {
                pos += 1;
                if let Some(value) = parse_u32_chars(&chars, &mut pos) {
                    commands.push(ECommand::Noise(value));
                    diagnostics.add_unsupported(
                        Some(line_no),
                        None,
                        format!("@e:n{value}"),
                        "preserved but not rendered",
                    );
                } else {
                    diagnostics.add_parse_warning(
                        Some(line_no),
                        None,
                        "@e noise command without value",
                    );
                }
            }
            '/' | '*' => {
                let command = ch;
                pos += 1;
                if let Some(value) = parse_i32_chars(&chars, &mut pos) {
                    commands.push(ECommand::ModeChange { command, value });
                    diagnostics.add_unsupported(
                        Some(line_no),
                        None,
                        format!("@e:{command}{value}"),
                        "preserved but not rendered",
                    );
                } else {
                    diagnostics.add_parse_warning(
                        Some(line_no),
                        None,
                        format!("@e mode command {command} without value"),
                    );
                }
            }
            'y' => {
                pos += 1;
                let register = parse_i32_chars(&chars, &mut pos);
                skip_ws(&chars, &mut pos);
                if chars.get(pos) == Some(&',') {
                    pos += 1;
                }
                let data = parse_i32_chars(&chars, &mut pos);
                match (register, data) {
                    (Some(register), Some(data)) => {
                        commands.push(ECommand::RegisterWrite { register, data });
                        diagnostics.add_unsupported(
                            Some(line_no),
                            None,
                            format!("@e:y{register},{data}"),
                            "preserved but not rendered",
                        );
                    }
                    _ => diagnostics.add_parse_warning(
                        Some(line_no),
                        None,
                        "@e register write command without register/data",
                    ),
                }
            }
            '\\' => {
                pos += 1;
                if let Some(value) = parse_i32_chars(&chars, &mut pos) {
                    commands.push(ECommand::FrequencyOffset(value));
                    diagnostics.add_unsupported(
                        Some(line_no),
                        None,
                        format!("@e:\\{value}"),
                        "preserved but not rendered",
                    );
                } else {
                    diagnostics.add_parse_warning(
                        Some(line_no),
                        None,
                        "@e frequency offset command without value",
                    );
                }
            }
            _ => {
                diagnostics.add_parse_warning(
                    Some(line_no),
                    None,
                    format!("ignored character in @e definition: {ch}"),
                );
                pos += 1;
            }
        }
    }
    commands
}

fn parse_macro_definition(
    line: &str,
    line_no: usize,
    macro_offset: i32,
    macros: &mut BTreeMap<u32, String>,
    diagnostics: &mut Diagnostics,
) {
    let chars: Vec<char> = line.chars().collect();
    let mut pos = 1;
    if let Some(id) = parse_u32_chars(&chars, &mut pos) {
        if let Some(content) = extract_braced(line) {
            if let Some(effective_id) = apply_macro_offset(id, macro_offset) {
                macros.insert(effective_id, content);
            } else {
                diagnostics.add_parse_warning(
                    Some(line_no),
                    None,
                    format!("macro id out of range after #macro_offset: *{id} + {macro_offset}"),
                );
            }
            return;
        }
    }
    diagnostics.add_parse_warning(Some(line_no), None, "invalid macro definition");
}

fn record_ignored_tone_definition(
    statement: &str,
    line: usize,
    command: &str,
    diagnostics: &mut Diagnostics,
) {
    diagnostics.add_unsupported(
        Some(line),
        None,
        command,
        if command == "@#" {
            "ignored tone assignment"
        } else {
            "ignored tone definition"
        },
    );
    diagnostics.add_ignored_tone_definition(
        Some(line),
        command,
        compact_statement_for_meta(statement),
    );
}

fn compact_statement_for_meta(statement: &str) -> String {
    statement.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn apply_macro_offset(id: u32, macro_offset: i32) -> Option<u32> {
    let effective = id as i64 + macro_offset as i64;
    if (0..=255).contains(&effective) {
        Some(effective as u32)
    } else {
        None
    }
}

fn extract_braced(line: &str) -> Option<String> {
    let start = line.find('{')?;
    let end = line.rfind('}')?;
    if end <= start {
        return None;
    }
    Some(line[start + 1..end].to_string())
}

fn parse_track_line(line: &str) -> Option<(Vec<String>, &str)> {
    let mut split_at = None;
    for (idx, ch) in line.char_indices() {
        if ch.is_whitespace() {
            split_at = Some(idx);
            break;
        }
        if !is_track_selector_char(ch) {
            return None;
        }
    }
    let split_at = split_at?;
    if split_at == 0 {
        return None;
    }
    let selectors = line[..split_at]
        .chars()
        .map(|ch| ch.to_ascii_lowercase().to_string())
        .collect();
    Some((selectors, line[split_at..].trim_start()))
}

fn is_track_selector_char(ch: char) -> bool {
    matches!(ch, '1'..='9' | 'a'..='h' | 'A'..='H' | 'r' | 'R')
}

fn parse_play_track_selectors(input: &str) -> BTreeSet<String> {
    input
        .chars()
        .filter(|ch| is_track_selector_char(*ch))
        .map(|ch| ch.to_ascii_lowercase().to_string())
        .collect()
}

fn track_kind_for_selector(selector: &str, opll_mode: i32, rhythm_map: RhythmMap) -> TrackKind {
    if rhythm_map == RhythmMap::Gm && opll_mode == 1 && matches!(selector, "f" | "r") {
        TrackKind::Rhythm
    } else {
        TrackKind::Melodic
    }
}

#[derive(Clone, Debug)]
struct MmlState {
    cursor: Rational,
    octave: i32,
    default_length: Rational,
    volume: i32,
    rhythm_volumes: BTreeMap<char, i32>,
    q: i32,
    tone: Option<u8>,
    envelope: Option<EnvelopeRef>,
    octave_base: i32,
    slur_from_previous: bool,
    warned_q0: bool,
}

impl MmlState {
    fn new(octave_base: i32) -> Self {
        Self {
            cursor: Rational::ZERO,
            octave: 4,
            default_length: denominator_to_steps(4).unwrap(),
            volume: 0,
            rhythm_volumes: BTreeMap::new(),
            q: 8,
            tone: None,
            envelope: None,
            octave_base,
            slur_from_previous: false,
            warned_q0: false,
        }
    }
}

struct MmlContext<'a> {
    macros: &'a BTreeMap<u32, String>,
    macro_offset: i32,
    track_kind: TrackKind,
    loop_count: Option<u32>,
    tempo_events: &'a mut Vec<TempoEvent>,
    diagnostics: &'a mut Diagnostics,
    track_id: &'a str,
    line: usize,
}

fn parse_mml(
    input: &str,
    state: &mut MmlState,
    track: &mut TrackIr,
    ctx: &mut MmlContext<'_>,
    depth: usize,
) -> Result<(), String> {
    if depth > 8 {
        return Err(format!(
            "macro/loop expansion exceeded depth at line {}",
            ctx.line
        ));
    }
    let chars: Vec<char> = input.chars().collect();
    let mut pos = 0;
    while pos < chars.len() {
        skip_ws(&chars, &mut pos);
        if pos >= chars.len() {
            break;
        }
        let ch = chars[pos].to_ascii_lowercase();
        match ch {
            _ if ctx.track_kind == TrackKind::Rhythm && rhythm_drum_note(ch).is_some() => {
                parse_rhythm_note(&chars, &mut pos, state, track, ctx)
            }
            'a'..='g' => parse_note(&chars, &mut pos, state, track, ctx),
            'r' => parse_rest(&chars, &mut pos, state, track, ctx),
            'o' => {
                pos += 1;
                if let Some(value) = parse_i32_chars(&chars, &mut pos) {
                    state.octave = value;
                } else {
                    ctx.diagnostics.add_parse_warning(
                        Some(ctx.line),
                        Some(ctx.track_id.to_string()),
                        "o command without octave",
                    );
                }
            }
            '<' => {
                state.octave -= 1;
                pos += 1;
            }
            '>' => {
                state.octave += 1;
                pos += 1;
            }
            'l' => {
                pos += 1;
                if let Some(length) = parse_length(&chars, &mut pos, None, ctx) {
                    state.default_length = length;
                } else {
                    ctx.diagnostics.add_parse_warning(
                        Some(ctx.line),
                        Some(ctx.track_id.to_string()),
                        "l command without length",
                    );
                }
            }
            't' => {
                pos += 1;
                if let Some(bpm) = parse_u32_chars(&chars, &mut pos) {
                    ctx.tempo_events.push(TempoEvent {
                        at_steps: state.cursor,
                        bpm,
                        source: format!("t:{}", ctx.track_id),
                        line: Some(ctx.line),
                    });
                    track.events.push(IrEvent::Tempo {
                        at_steps: state.cursor,
                        bpm,
                    });
                } else {
                    ctx.diagnostics.add_parse_warning(
                        Some(ctx.line),
                        Some(ctx.track_id.to_string()),
                        "t command without BPM",
                    );
                }
            }
            'v' => parse_volume(&chars, &mut pos, state, ctx),
            ')' => {
                pos += 1;
                let delta = parse_i32_chars(&chars, &mut pos).unwrap_or(1);
                state.volume = (state.volume + delta).clamp(0, 15);
            }
            '(' => {
                pos += 1;
                let delta = parse_i32_chars(&chars, &mut pos).unwrap_or(1);
                state.volume = (state.volume - delta).clamp(0, 15);
            }
            'q' => {
                pos += 1;
                if let Some(value) = parse_i32_chars(&chars, &mut pos) {
                    state.q = value.clamp(0, 8);
                } else {
                    ctx.diagnostics.add_parse_warning(
                        Some(ctx.line),
                        Some(ctx.track_id.to_string()),
                        "q command without value",
                    );
                }
            }
            '@' => parse_at_command(&chars, &mut pos, state, track, ctx),
            '[' => parse_loop(&chars, &mut pos, state, track, ctx, depth)?,
            ']' => {
                ctx.diagnostics.add_parse_warning(
                    Some(ctx.line),
                    Some(ctx.track_id.to_string()),
                    "unmatched loop end",
                );
                pos += 1;
            }
            '|' => {
                ctx.diagnostics.add_parse_warning(
                    Some(ctx.line),
                    Some(ctx.track_id.to_string()),
                    "loop alternative outside loop ignored",
                );
                pos += 1;
            }
            '*' => {
                pos += 1;
                if let Some(id) = parse_u32_chars(&chars, &mut pos) {
                    let Some(effective_id) = apply_macro_offset(id, ctx.macro_offset) else {
                        return Err(format!(
                            "macro id out of range after #macro_offset: *{id} + {} at line {}",
                            ctx.macro_offset, ctx.line
                        ));
                    };
                    if let Some(body) = ctx.macros.get(&effective_id) {
                        parse_mml(body, state, track, ctx, depth + 1)?;
                    } else {
                        return Err(format!(
                            "undefined macro *{id} (effective *{effective_id}) at line {}",
                            ctx.line
                        ));
                    }
                } else {
                    ctx.diagnostics.add_parse_warning(
                        Some(ctx.line),
                        Some(ctx.track_id.to_string()),
                        "macro reference without number",
                    );
                }
            }
            '!' => break,
            ',' => pos += 1,
            _ if is_unsupported_mml_command(ch) => {
                record_unsupported_mml_command(&chars, &mut pos, ctx);
            }
            _ => {
                ctx.diagnostics.add_parse_warning(
                    Some(ctx.line),
                    Some(ctx.track_id.to_string()),
                    format!("ignored MML character: {}", chars[pos]),
                );
                pos += 1;
            }
        }
    }
    Ok(())
}

fn parse_note(
    chars: &[char],
    pos: &mut usize,
    state: &mut MmlState,
    track: &mut TrackIr,
    ctx: &mut MmlContext<'_>,
) {
    let note_char = chars[*pos].to_ascii_lowercase();
    *pos += 1;
    let mut semitone = note_semitone(note_char).unwrap_or(0);
    while let Some(ch) = chars.get(*pos) {
        match ch {
            '+' | '#' => {
                semitone += 1;
                *pos += 1;
            }
            '-' => {
                semitone -= 1;
                *pos += 1;
            }
            _ => break,
        }
    }

    let mut length =
        parse_length(chars, pos, Some(state.default_length), ctx).unwrap_or(state.default_length);
    while consume_tie_extension(chars, pos, &mut length, state.default_length, ctx) {}
    let slur_to_next = consume_slur(chars, pos);

    let raw_note = state.octave_base + (state.octave - 4) * 12 + semitone;
    if !(0..=127).contains(&raw_note) {
        ctx.diagnostics.add_parse_warning(
            Some(ctx.line),
            Some(ctx.track_id.to_string()),
            format!("MIDI note out of range and skipped: {raw_note}"),
        );
        state.cursor = state.cursor.add(length);
        return;
    }

    if state.slur_from_previous && extend_last_note_if_same(track, raw_note as u8, length) {
        state.cursor = state.cursor.add(length);
        state.slur_from_previous = slur_to_next;
        return;
    }

    let duration = if slur_to_next {
        length
    } else {
        let mut duration = length;
        if state.q == 0 {
            if !state.warned_q0 {
                ctx.diagnostics.add_unsupported(
                    Some(ctx.line),
                    Some(ctx.track_id.to_string()),
                    "q0",
                    "treated as q8",
                );
                state.warned_q0 = true;
            }
        } else if state.q < 8 {
            duration = length.mul_i64(state.q as i64).div_i64(8);
        }
        duration
    };

    track.events.push(IrEvent::Note {
        start_steps: state.cursor,
        duration_steps: duration,
        note: raw_note as u8,
        velocity: velocity_from_source(state.volume),
        tone: state.tone,
        envelope: state.envelope,
    });
    state.cursor = state.cursor.add(length);
    state.slur_from_previous = slur_to_next;
}

fn parse_rhythm_note(
    chars: &[char],
    pos: &mut usize,
    state: &mut MmlState,
    track: &mut TrackIr,
    ctx: &mut MmlContext<'_>,
) {
    let instrument = chars[*pos].to_ascii_lowercase();
    let Some(note) = rhythm_drum_note(instrument) else {
        *pos += 1;
        return;
    };
    *pos += 1;

    let mut length =
        parse_length(chars, pos, Some(state.default_length), ctx).unwrap_or(state.default_length);
    while consume_tie_extension(chars, pos, &mut length, state.default_length, ctx) {}
    if consume_slur(chars, pos) {
        ctx.diagnostics.add_parse_warning(
            Some(ctx.line),
            Some(ctx.track_id.to_string()),
            "slur after rhythm note ignored",
        );
    }

    let mut duration = length;
    if state.q == 0 {
        if !state.warned_q0 {
            ctx.diagnostics.add_unsupported(
                Some(ctx.line),
                Some(ctx.track_id.to_string()),
                "q0",
                "treated as q8",
            );
            state.warned_q0 = true;
        }
    } else if state.q < 8 {
        duration = length.mul_i64(state.q as i64).div_i64(8);
    }

    let volume = state
        .rhythm_volumes
        .get(&instrument)
        .copied()
        .unwrap_or(state.volume);
    track.events.push(IrEvent::Note {
        start_steps: state.cursor,
        duration_steps: duration,
        note,
        velocity: velocity_from_source(volume),
        tone: state.tone,
        envelope: None,
    });
    state.cursor = state.cursor.add(length);
    state.slur_from_previous = false;
}

fn parse_rest(
    chars: &[char],
    pos: &mut usize,
    state: &mut MmlState,
    track: &mut TrackIr,
    ctx: &mut MmlContext<'_>,
) {
    *pos += 1;
    let mut length =
        parse_length(chars, pos, Some(state.default_length), ctx).unwrap_or(state.default_length);
    while consume_tie_extension(chars, pos, &mut length, state.default_length, ctx) {}
    if consume_slur(chars, pos) {
        ctx.diagnostics.add_parse_warning(
            Some(ctx.line),
            Some(ctx.track_id.to_string()),
            "slur after rest ignored",
        );
    }
    track.events.push(IrEvent::Rest {
        start_steps: state.cursor,
        duration_steps: length,
    });
    state.cursor = state.cursor.add(length);
    state.slur_from_previous = false;
}

fn consume_tie_extension(
    chars: &[char],
    pos: &mut usize,
    length: &mut Rational,
    default_length: Rational,
    ctx: &mut MmlContext<'_>,
) -> bool {
    skip_ws(chars, pos);
    if chars.get(*pos) != Some(&'^') {
        return false;
    }
    *pos += 1;
    skip_ws(chars, pos);
    if let Some(next) = chars.get(*pos).copied() {
        let next = next.to_ascii_lowercase();
        if matches!(next, 'a'..='g' | 'r') || rhythm_drum_note(next).is_some() {
            *pos += 1;
            while matches!(chars.get(*pos), Some(&'+') | Some(&'#') | Some(&'-')) {
                *pos += 1;
            }
        }
    }
    let extra = parse_length(chars, pos, Some(default_length), ctx).unwrap_or(default_length);
    *length = length.add(extra);
    true
}

fn consume_slur(chars: &[char], pos: &mut usize) -> bool {
    skip_ws(chars, pos);
    if chars.get(*pos) == Some(&'&') {
        *pos += 1;
        true
    } else {
        false
    }
}

fn extend_last_note_if_same(
    track: &mut TrackIr,
    note_to_extend: u8,
    extra_steps: Rational,
) -> bool {
    let Some(IrEvent::Note {
        duration_steps,
        note,
        ..
    }) = track.events.last_mut()
    else {
        return false;
    };
    if *note != note_to_extend {
        return false;
    }
    *duration_steps = duration_steps.add(extra_steps);
    true
}

fn parse_volume(chars: &[char], pos: &mut usize, state: &mut MmlState, ctx: &mut MmlContext<'_>) {
    *pos += 1;
    if ctx.track_kind == TrackKind::Rhythm {
        if let Some(instrument) = chars.get(*pos).copied().map(|ch| ch.to_ascii_lowercase()) {
            if rhythm_drum_note(instrument).is_some() {
                *pos += 1;
                if let Some(value) = parse_i32_chars(chars, pos) {
                    state.rhythm_volumes.insert(instrument, value.clamp(0, 15));
                } else {
                    ctx.diagnostics.add_parse_warning(
                        Some(ctx.line),
                        Some(ctx.track_id.to_string()),
                        "rhythm v command without value",
                    );
                }
                return;
            }
        }
    }
    match chars.get(*pos) {
        Some('+') => {
            *pos += 1;
            let delta = parse_i32_chars(chars, pos).unwrap_or(1);
            state.volume = (state.volume + delta).clamp(0, 15);
        }
        Some('-') => {
            *pos += 1;
            let delta = parse_i32_chars(chars, pos).unwrap_or(1);
            state.volume = (state.volume - delta).clamp(0, 15);
        }
        _ => {
            if let Some(value) = parse_i32_chars(chars, pos) {
                state.volume = value.clamp(0, 15);
            } else {
                ctx.diagnostics.add_parse_warning(
                    Some(ctx.line),
                    Some(ctx.track_id.to_string()),
                    "v command without value",
                );
            }
        }
    }
}

fn parse_at_command(
    chars: &[char],
    pos: &mut usize,
    state: &mut MmlState,
    track: &mut TrackIr,
    ctx: &mut MmlContext<'_>,
) {
    *pos += 1;
    if *pos >= chars.len() {
        ctx.diagnostics.add_parse_warning(
            Some(ctx.line),
            Some(ctx.track_id.to_string()),
            "@ command without body",
        );
        return;
    }
    let next = chars
        .get(*pos)
        .copied()
        .unwrap_or('\0')
        .to_ascii_lowercase();
    if next == 'e' || next == 'r' {
        *pos += 1;
        if let Some(id) = parse_u32_chars(chars, pos) {
            let reference = if next == 'e' {
                EnvelopeRef::E(id.min(255) as u8)
            } else {
                EnvelopeRef::R(id.min(255) as u8)
            };
            state.envelope = Some(reference);
            track.events.push(IrEvent::Control {
                at_steps: state.cursor,
                kind: ControlKind::Envelope(reference),
            });
        } else {
            ctx.diagnostics.add_parse_warning(
                Some(ctx.line),
                Some(ctx.track_id.to_string()),
                "@e/@r without envelope number",
            );
        }
        return;
    }

    if next == 'm' {
        *pos += 1;
        if let Some(id) = parse_u32_chars(chars, pos) {
            track.events.push(IrEvent::Control {
                at_steps: state.cursor,
                kind: ControlKind::Text(id.min(255) as u8),
            });
        } else {
            ctx.diagnostics.add_parse_warning(
                Some(ctx.line),
                Some(ctx.track_id.to_string()),
                "@m without text number",
            );
        }
        return;
    }

    if let Some(id) = parse_u32_chars(chars, pos) {
        state.tone = Some(id.min(255) as u8);
        track.events.push(IrEvent::Control {
            at_steps: state.cursor,
            kind: ControlKind::Tone(id.min(255) as u8),
        });
        return;
    }

    record_unsupported_at_command(chars, pos, ctx);
}

fn parse_loop(
    chars: &[char],
    pos: &mut usize,
    state: &mut MmlState,
    track: &mut TrackIr,
    ctx: &mut MmlContext<'_>,
    depth: usize,
) -> Result<(), String> {
    let loop_start = state.cursor;
    *pos += 1;
    skip_ws(chars, pos);
    let prefix_count = parse_u32_chars(chars, pos);
    let body_start = *pos;
    let mut scan = *pos;
    let mut level = 1;
    while scan < chars.len() {
        match chars[scan] {
            '[' => level += 1,
            ']' => {
                level -= 1;
                if level == 0 {
                    break;
                }
            }
            _ => {}
        }
        scan += 1;
    }
    if scan >= chars.len() {
        return Err(format!("unterminated loop at line {}", ctx.line));
    }
    let body: String = chars[body_start..scan].iter().collect();
    *pos = scan + 1;
    let suffix_count = parse_u32_chars(chars, pos);
    let repeat_count = prefix_count.or(suffix_count).unwrap_or(2);

    if repeat_count == 0 {
        track.events.push(IrEvent::LoopMarker {
            at_steps: loop_start,
            kind: LoopMarkerKind::Start,
        });
        let iterations = ctx.loop_count.unwrap_or(1);
        for _ in 0..iterations {
            parse_mml(&body, state, track, ctx, depth + 1)?;
        }
        track.events.push(IrEvent::LoopMarker {
            at_steps: state.cursor,
            kind: LoopMarkerKind::End,
        });
        ctx.diagnostics.add_loop_marker(
            ctx.track_id.to_string(),
            loop_start,
            state.cursor,
            "infinite",
        );
    } else if let Some((before, after)) = split_alternative(&body) {
        for _ in 0..repeat_count.saturating_sub(1) {
            parse_mml(&before, state, track, ctx, depth + 1)?;
        }
        parse_mml(&after, state, track, ctx, depth + 1)?;
    } else {
        for _ in 0..repeat_count {
            parse_mml(&body, state, track, ctx, depth + 1)?;
        }
    }
    Ok(())
}

fn split_alternative(body: &str) -> Option<(String, String)> {
    let chars: Vec<char> = body.chars().collect();
    let mut level = 0;
    for (idx, ch) in chars.iter().enumerate() {
        match ch {
            '[' => level += 1,
            ']' => level -= 1,
            '|' if level == 0 => {
                return Some((
                    chars[..idx].iter().collect(),
                    chars[idx + 1..].iter().collect(),
                ));
            }
            _ => {}
        }
    }
    None
}

fn parse_length(
    chars: &[char],
    pos: &mut usize,
    default_length: Option<Rational>,
    ctx: &mut MmlContext<'_>,
) -> Option<Rational> {
    skip_ws(chars, pos);
    let base = if chars.get(*pos) == Some(&'%') {
        *pos += 1;
        let steps = parse_u32_chars(chars, pos)?;
        Rational::from_i64(steps as i64)
    } else if let Some(denominator) = parse_u32_chars(chars, pos) {
        match denominator_to_steps(denominator) {
            Some(value) => value,
            None => {
                ctx.diagnostics.add_parse_warning(
                    Some(ctx.line),
                    Some(ctx.track_id.to_string()),
                    "zero length denominator; using default length",
                );
                default_length?
            }
        }
    } else {
        default_length?
    };
    let mut dots = 0;
    while chars.get(*pos) == Some(&'.') {
        dots += 1;
        *pos += 1;
    }
    Some(apply_dots(base, dots))
}

fn note_semitone(ch: char) -> Option<i32> {
    match ch {
        'c' => Some(0),
        'd' => Some(2),
        'e' => Some(4),
        'f' => Some(5),
        'g' => Some(7),
        'a' => Some(9),
        'b' => Some(11),
        _ => None,
    }
}

fn rhythm_drum_note(ch: char) -> Option<u8> {
    match ch {
        'b' => Some(36),
        's' => Some(38),
        'm' => Some(45),
        'c' => Some(49),
        'h' => Some(42),
        _ => None,
    }
}

fn is_unsupported_mml_command(ch: char) -> bool {
    matches!(
        ch,
        'm' | 's' | 'n' | '/' | 'p' | 'h' | '\\' | '_' | 'y' | '$' | 'k'
    )
}

fn record_unsupported_mml_command(chars: &[char], pos: &mut usize, ctx: &mut MmlContext<'_>) {
    let command = read_unsupported_mml_command(chars, pos);
    let policy = unsupported_mml_policy(&command);
    ctx.diagnostics.add_unsupported(
        Some(ctx.line),
        Some(ctx.track_id.to_string()),
        command,
        policy,
    );
}

fn record_unsupported_at_command(chars: &[char], pos: &mut usize, ctx: &mut MmlContext<'_>) {
    let mut command = String::from("@");
    command.push_str(&read_unsupported_at_suffix(chars, pos));
    let policy = unsupported_mml_policy(&command);
    ctx.diagnostics.add_unsupported(
        Some(ctx.line),
        Some(ctx.track_id.to_string()),
        command,
        policy,
    );
}

fn read_unsupported_mml_command(chars: &[char], pos: &mut usize) -> String {
    if *pos >= chars.len() {
        return String::new();
    }
    let lower = chars[*pos].to_ascii_lowercase();
    match lower {
        '$' => read_single_char_token(chars, pos),
        's' => read_fixed_keyword_or_numeric(chars, pos, &["so", "sf"]),
        'h' => read_fixed_keyword_or_numeric(chars, pos, &["ho", "hf", "hi"]),
        'k' => read_fixed_keyword_or_generic(chars, pos, &["ko", "kf"]),
        'y' => read_register_write_token(chars, pos),
        '_' => read_pitch_glide_token(chars, pos),
        'm' | 'n' | '/' | 'p' | '\\' => read_numeric_command_token(chars, pos),
        _ => read_generic_command_token(chars, pos),
    }
}

fn read_unsupported_at_suffix(chars: &[char], pos: &mut usize) -> String {
    if *pos >= chars.len() {
        return String::new();
    }
    let lower = chars[*pos].to_ascii_lowercase();
    match lower {
        '\\' | 'p' | 'l' | 'f' | 'o' => read_numeric_command_token(chars, pos),
        _ => read_generic_command_token(chars, pos),
    }
}

fn read_fixed_keyword_or_numeric(chars: &[char], pos: &mut usize, keywords: &[&str]) -> String {
    if let Some(token) = read_fixed_keyword(chars, pos, keywords) {
        token
    } else {
        read_numeric_command_token(chars, pos)
    }
}

fn read_fixed_keyword_or_generic(chars: &[char], pos: &mut usize, keywords: &[&str]) -> String {
    if let Some(token) = read_fixed_keyword(chars, pos, keywords) {
        token
    } else {
        read_generic_command_token(chars, pos)
    }
}

fn read_fixed_keyword(chars: &[char], pos: &mut usize, keywords: &[&str]) -> Option<String> {
    for keyword in keywords {
        let keyword_chars: Vec<char> = keyword.chars().collect();
        if *pos + keyword_chars.len() > chars.len() {
            continue;
        }
        if keyword_chars
            .iter()
            .enumerate()
            .all(|(idx, expected)| chars[*pos + idx].to_ascii_lowercase() == *expected)
        {
            let start = *pos;
            *pos += keyword_chars.len();
            return Some(chars[start..*pos].iter().collect());
        }
    }
    None
}

fn read_single_char_token(chars: &[char], pos: &mut usize) -> String {
    let start = *pos;
    *pos += 1;
    chars[start..*pos].iter().collect()
}

fn read_numeric_command_token(chars: &[char], pos: &mut usize) -> String {
    let start = *pos;
    *pos += 1;
    let _ = parse_i32_chars(chars, pos);
    loop {
        let before_separator = *pos;
        skip_ws(chars, pos);
        if chars.get(*pos) != Some(&',') {
            *pos = before_separator;
            break;
        }
        *pos += 1;
        let before_value = *pos;
        if parse_i32_chars(chars, pos).is_none() {
            *pos = before_value;
            break;
        }
    }
    chars[start..*pos].iter().collect()
}

fn read_register_write_token(chars: &[char], pos: &mut usize) -> String {
    let start = *pos;
    *pos += 1;
    let _ = parse_i32_chars(chars, pos);
    let before_separator = *pos;
    skip_ws(chars, pos);
    if chars.get(*pos) == Some(&',') {
        *pos += 1;
        let before_data = *pos;
        if parse_i32_chars(chars, pos).is_none() {
            *pos = before_data;
        }
    } else {
        *pos = before_separator;
    }
    chars[start..*pos].iter().collect()
}

fn read_pitch_glide_token(chars: &[char], pos: &mut usize) -> String {
    let start = *pos;
    *pos += 1;
    skip_ws(chars, pos);
    let Some(next) = chars.get(*pos).copied().map(|ch| ch.to_ascii_lowercase()) else {
        return chars[start..*pos].iter().collect();
    };
    if matches!(next, 'a'..='g') {
        *pos += 1;
        while matches!(chars.get(*pos), Some(&'+') | Some(&'#') | Some(&'-')) {
            *pos += 1;
        }
        consume_optional_length_token(chars, pos);
    } else {
        let _ = parse_i32_chars(chars, pos);
    }
    chars[start..*pos].iter().collect()
}

fn consume_optional_length_token(chars: &[char], pos: &mut usize) {
    if chars.get(*pos) == Some(&'%') {
        *pos += 1;
        while chars
            .get(*pos)
            .copied()
            .is_some_and(|ch| ch.is_ascii_digit())
        {
            *pos += 1;
        }
    } else {
        while chars
            .get(*pos)
            .copied()
            .is_some_and(|ch| ch.is_ascii_digit())
        {
            *pos += 1;
        }
    }
    while chars.get(*pos) == Some(&'.') {
        *pos += 1;
    }
}

fn read_generic_command_token(chars: &[char], pos: &mut usize) -> String {
    if *pos >= chars.len() {
        return String::new();
    }
    let start = *pos;
    *pos += 1;
    while *pos < chars.len()
        && (chars[*pos].is_ascii_alphanumeric()
            || matches!(chars[*pos], '+' | '-' | '_' | '\\' | '/' | '#'))
    {
        *pos += 1;
    }
    chars[start..*pos].iter().collect()
}

fn unsupported_mml_policy(command: &str) -> &'static str {
    let lower = command.to_ascii_lowercase();
    if lower == "so" || lower == "sf" {
        "ignore_with_warning: FM sustain not rendered"
    } else if lower == "ko" || lower == "kf" {
        "ignore_with_warning: keyoff command ignored"
    } else if matches!(lower.as_str(), "ho" | "hf" | "hi") || lower.starts_with('h') {
        "ignore_with_warning: LFO not rendered"
    } else if lower.starts_with("@p") {
        "ignore_with_warning: LFO not rendered"
    } else if lower.starts_with("@l") {
        "ignore_with_warning: FM total level not rendered"
    } else if lower.starts_with("@\\") {
        "ignore_with_warning: fine detune not rendered"
    } else if lower.starts_with("@f") {
        "text_meta_or_ignore: fade command ignored"
    } else if lower.starts_with("@o") {
        "text_meta_or_ignore: output/control command ignored"
    } else if lower.starts_with('m') || lower.starts_with('s') {
        "ignore_with_warning: PSG hard envelope not rendered"
    } else if lower.starts_with('n') {
        "ignore_with_warning: PSG noise not rendered"
    } else if lower.starts_with('/') {
        "ignore_with_warning: PSG mode/forced keyoff not rendered"
    } else if lower.starts_with('\\') {
        "ignore_with_warning: detune not rendered"
    } else if lower.starts_with('p') {
        "ignore_with_warning: pitch down/LFO not rendered"
    } else if lower.starts_with('_') {
        "ignore_with_warning: pitch glide not rendered"
    } else if lower.starts_with('y') {
        "ignore_with_warning: register write ignored"
    } else if lower == "$" {
        "ignore: debug command"
    } else {
        "ignore_with_warning: ignored MML command"
    }
}

struct ListParser<'a> {
    chars: Vec<char>,
    pos: usize,
    source: &'a str,
}

impl<'a> ListParser<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            chars: source.chars().collect(),
            pos: 0,
            source,
        }
    }

    fn next_i32(&mut self) -> Option<i32> {
        skip_ws_and_separators(&self.chars, &mut self.pos);
        parse_i32_chars(&self.chars, &mut self.pos)
    }

    fn rest(&self) -> &str {
        let byte_pos = self
            .source
            .char_indices()
            .nth(self.pos)
            .map(|(idx, _)| idx)
            .unwrap_or(self.source.len());
        &self.source[byte_pos..]
    }
}

fn first_u32(input: &str) -> Option<u32> {
    let chars: Vec<char> = input.chars().collect();
    let mut pos = 0;
    while pos < chars.len() && !chars[pos].is_ascii_digit() {
        pos += 1;
    }
    parse_u32_chars(&chars, &mut pos)
}

fn first_i32(input: &str) -> Option<i32> {
    let chars: Vec<char> = input.chars().collect();
    let mut pos = 0;
    while pos < chars.len()
        && !chars[pos].is_ascii_digit()
        && chars[pos] != '-'
        && chars[pos] != '+'
    {
        pos += 1;
    }
    parse_i32_chars(&chars, &mut pos)
}

fn parse_i32_chars(chars: &[char], pos: &mut usize) -> Option<i32> {
    skip_ws(chars, pos);
    let mut sign = 1;
    if chars.get(*pos) == Some(&'-') {
        sign = -1;
        *pos += 1;
    } else if chars.get(*pos) == Some(&'+') {
        *pos += 1;
    }
    parse_u32_chars(chars, pos).map(|value| value as i32 * sign)
}

fn parse_u32_chars(chars: &[char], pos: &mut usize) -> Option<u32> {
    skip_ws(chars, pos);
    let start = *pos;
    let mut value = 0u32;
    while *pos < chars.len() && chars[*pos].is_ascii_digit() {
        value = value
            .saturating_mul(10)
            .saturating_add(chars[*pos].to_digit(10).unwrap_or(0));
        *pos += 1;
    }
    if *pos == start { None } else { Some(value) }
}

fn skip_ws(chars: &[char], pos: &mut usize) {
    while *pos < chars.len() && chars[*pos].is_whitespace() {
        *pos += 1;
    }
}

fn skip_ws_and_separators(chars: &[char], pos: &mut usize) {
    while *pos < chars.len()
        && (chars[*pos].is_whitespace() || chars[*pos] == ',' || chars[*pos] == '.')
    {
        *pos += 1;
    }
}

fn unquote(input: &str) -> String {
    let trimmed = input.trim();
    if trimmed.len() >= 2 && trimmed.starts_with('"') && trimmed.ends_with('"') {
        trimmed[1..trimmed.len() - 1].to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::{RenderedEventKind, collect_midi_events};

    fn parse_and_render(input: &str) -> Vec<crate::render::RenderedEvent> {
        let opts = ConversionOptions::default();
        let mut song = parse_source(input, &opts).unwrap();
        collect_midi_events(&mut song, &opts).unwrap()
    }

    #[test]
    fn tempo_and_cdef_become_four_quarter_notes() {
        let events = parse_and_render("#tempo 120\n1 cdef\n");
        let starts: Vec<u64> = events
            .iter()
            .filter_map(|event| match event.kind {
                RenderedEventKind::NoteOn { note, .. } => Some((event.abs_tick, note)),
                _ => None,
            })
            .map(|(tick, _)| tick)
            .collect();
        assert_eq!(starts, vec![0, 3600, 7200, 10800]);

        let offs: Vec<u64> = events
            .iter()
            .filter_map(|event| match event.kind {
                RenderedEventKind::NoteOff { .. } => Some(event.abs_tick),
                _ => None,
            })
            .collect();
        assert_eq!(offs[0], 3600);
    }

    #[test]
    fn smf_auxiliary_meta_events_are_emitted() {
        let opts = ConversionOptions::default();
        let mut song = parse_source("#title \"aux meta\"\n1 c\n", &opts).unwrap();
        song.diagnostics.input_file = Some("tests/compat/simple_scale.mus".to_string());
        let events = collect_midi_events(&mut song, &opts).unwrap();

        assert!(events.iter().any(|event| {
            event.track_index == 0
                && event.abs_tick == 0
                && matches!(
                    event.kind,
                    RenderedEventKind::TimeSignature {
                        numerator: 4,
                        denominator_power: 2,
                        clocks_per_metronome: 24,
                        thirty_seconds_per_quarter: 8,
                    }
                )
        }));
        assert!(events.iter().any(|event| {
            event.track_index == 0
                && event.abs_tick == 0
                && matches!(
                    event.kind,
                    RenderedEventKind::KeySignature {
                        sharps_flats: 0,
                        minor: false,
                    }
                )
        }));
        assert!(events.iter().any(|event| {
            matches!(
                &event.kind,
                RenderedEventKind::Text(text) if text == "Generated by mgs2smf"
            )
        }));
        assert!(events.iter().any(|event| {
            matches!(
                &event.kind,
                RenderedEventKind::Text(text)
                    if text == "Source MUS: tests/compat/simple_scale.mus"
            )
        }));
    }

    #[test]
    fn percent_length_48_is_one_quarter() {
        let events = parse_and_render("1 c%48\n");
        let off = events
            .iter()
            .find(|event| matches!(event.kind, RenderedEventKind::NoteOff { .. }))
            .unwrap();
        assert_eq!(off.abs_tick, 3600);
    }

    #[test]
    fn rest_percent_48_creates_gap() {
        let events = parse_and_render("1 c%48 r%48 d%48\n");
        let starts: Vec<u64> = events
            .iter()
            .filter_map(|event| match event.kind {
                RenderedEventKind::NoteOn { .. } => Some(event.abs_tick),
                _ => None,
            })
            .collect();
        assert_eq!(starts, vec![0, 7200]);
    }

    #[test]
    fn e_envelope_generates_cc11() {
        let events = parse_and_render("@e0 = {1,0,f}\n1 @e0 c4\n");
        assert!(events.iter().any(|event| {
            event.abs_tick == 0
                && matches!(
                    event.kind,
                    RenderedEventKind::ControlChange {
                        controller: 11,
                        value: 127,
                        ..
                    }
                )
        }));
    }

    #[test]
    fn e_envelope_count_timing_follows_tempo_map() {
        let events = parse_and_render("#tempo 120\n@e0 = {1,0,f:31,8}\n1 @e0 c1\n2 r%48 t60\n");
        let cc11s: Vec<(u64, u8)> = events
            .iter()
            .filter_map(|event| match event.kind {
                RenderedEventKind::ControlChange {
                    controller: 11,
                    value,
                    ..
                } => Some((event.abs_tick, value)),
                _ => None,
            })
            .collect();
        assert!(cc11s.contains(&(0, 127)));
        assert!(cc11s.contains(&(3660, 68)));
        assert!(!cc11s.contains(&(3720, 68)));
    }

    #[test]
    fn r_envelope_adsr_generates_cc11_curve() {
        let events = parse_and_render("@r0 = {0,0,0,128,64,128,32,0}\n1 @r0 c4\n");
        let cc11s: Vec<(u64, u8)> = events
            .iter()
            .filter_map(|event| match event.kind {
                RenderedEventKind::ControlChange {
                    controller: 11,
                    value,
                    ..
                } => Some((event.abs_tick, value)),
                _ => None,
            })
            .collect();
        assert!(cc11s.contains(&(0, 0)));
        assert!(cc11s.contains(&(120, 64)));
        assert!(cc11s.contains(&(240, 127)));
        assert!(cc11s.contains(&(480, 95)));
    }

    #[test]
    fn split_velocity_splits_note_at_envelope_changes() {
        let mut opts = ConversionOptions::default();
        opts.envelope_mode = crate::ir::EnvelopeMode::SplitVelocity;
        let mut song = parse_source("@e0 = {1,0,f:1,8:1,4:1}\n1 v15 @e0 c4\n", &opts).unwrap();
        let events = collect_midi_events(&mut song, &opts).unwrap();

        let note_ons: Vec<(u64, u8)> = events
            .iter()
            .filter_map(|event| match event.kind {
                RenderedEventKind::NoteOn { velocity, .. } => Some((event.abs_tick, velocity)),
                _ => None,
            })
            .collect();
        let note_offs: Vec<u64> = events
            .iter()
            .filter_map(|event| match event.kind {
                RenderedEventKind::NoteOff { .. } => Some(event.abs_tick),
                _ => None,
            })
            .collect();

        assert_eq!(note_ons, vec![(0, 127), (120, 68), (240, 34)]);
        assert_eq!(note_offs, vec![120, 240, 3600]);
        assert!(!events.iter().any(|event| {
            matches!(
                event.kind,
                RenderedEventKind::ControlChange { controller: 11, .. }
            )
        }));
    }

    #[test]
    fn e_envelope_tone_change_is_preserved() {
        let opts = ConversionOptions::default();
        let song = parse_source("@e0 = {1,0,@10,f}\n1 @e0 c\n", &opts).unwrap();
        let commands = &song.envelopes.e.get(&0).unwrap().commands;
        assert!(commands.contains(&ECommand::ToneChange(10)));
        assert!(
            !song
                .diagnostics
                .unsupported_commands
                .iter()
                .any(|item| item.command == "@e:@10")
        );
    }

    #[test]
    fn e_envelope_non_smf_commands_are_preserved_with_warnings() {
        let opts = ConversionOptions::default();
        let song = parse_source("@e0 = {1,0,n3,/2,*1,y7,9,\\-2,f}\n1 @e0 c\n", &opts).unwrap();
        let commands = &song.envelopes.e.get(&0).unwrap().commands;
        assert!(commands.contains(&ECommand::Noise(3)));
        assert!(commands.contains(&ECommand::ModeChange {
            command: '/',
            value: 2
        }));
        assert!(commands.contains(&ECommand::ModeChange {
            command: '*',
            value: 1
        }));
        assert!(commands.contains(&ECommand::RegisterWrite {
            register: 7,
            data: 9
        }));
        assert!(commands.contains(&ECommand::FrequencyOffset(-2)));

        let unsupported: Vec<&str> = song
            .diagnostics
            .unsupported_commands
            .iter()
            .map(|item| item.command.as_str())
            .collect();
        assert!(unsupported.contains(&"@e:n3"));
        assert!(unsupported.contains(&"@e:/2"));
        assert!(unsupported.contains(&"@e:*1"));
        assert!(unsupported.contains(&"@e:y7,9"));
        assert!(unsupported.contains(&"@e:\\-2"));
    }

    #[test]
    fn infinite_loop_generates_markers() {
        let events = parse_and_render("1 [0 c]\n");
        assert!(events.iter().any(|event| {
            matches!(
                &event.kind,
                RenderedEventKind::Marker { text, .. } if text == "loopStart"
            )
        }));
        assert!(events.iter().any(|event| {
            matches!(
                &event.kind,
                RenderedEventKind::Marker { text, .. } if text == "loopEnd"
            )
        }));
    }

    #[test]
    fn custom_loop_marker_names_are_rendered() {
        let mut opts = ConversionOptions::default();
        opts.loop_marker_start = "A".to_string();
        opts.loop_marker_end = "B".to_string();
        let mut song = parse_source("1 [0 c]\n", &opts).unwrap();
        let events = collect_midi_events(&mut song, &opts).unwrap();
        assert!(events.iter().any(|event| {
            matches!(
                &event.kind,
                RenderedEventKind::Marker { text, .. } if text == "A"
            )
        }));
        assert!(events.iter().any(|event| {
            matches!(
                &event.kind,
                RenderedEventKind::Marker { text, .. } if text == "B"
            )
        }));
    }

    #[test]
    fn unsupported_tones_become_warnings() {
        let opts = ConversionOptions::default();
        let song = parse_source("@s0 = {0}\n@v0 = {0}\n@#2 = 3\n", &opts).unwrap();
        let commands: Vec<&str> = song
            .diagnostics
            .unsupported_commands
            .iter()
            .map(|item| item.command.as_str())
            .collect();
        assert_eq!(commands, vec!["@s", "@v", "@#"]);
        let ignored: Vec<&str> = song
            .diagnostics
            .ignored_tone_definitions
            .iter()
            .map(|item| item.command.as_str())
            .collect();
        assert_eq!(ignored, vec!["@s", "@v", "@#"]);
    }

    #[test]
    fn unsupported_mml_commands_are_diagnosed_by_policy() {
        let opts = ConversionOptions::default();
        let song = parse_source(
            r"1 m1 s2 n3 /4 \-5 p6 h7 ho hf hi _c4 y7,9 so sf ko kf $ @p1 @l2 @\3 @f4 @o5 c
",
            &opts,
        )
        .unwrap();
        let unsupported: Vec<(&str, &str)> = song
            .diagnostics
            .unsupported_commands
            .iter()
            .map(|item| (item.command.as_str(), item.policy.as_str()))
            .collect();
        let commands: Vec<&str> = unsupported.iter().map(|(command, _)| *command).collect();
        assert_eq!(
            commands,
            vec![
                "m1", "s2", "n3", "/4", "\\-5", "p6", "h7", "ho", "hf", "hi", "_c4", "y7,9", "so",
                "sf", "ko", "kf", "$", "@p1", "@l2", "@\\3", "@f4", "@o5"
            ]
        );

        for (command, policy_fragment) in [
            ("m1", "PSG hard envelope"),
            ("s2", "PSG hard envelope"),
            ("n3", "PSG noise"),
            ("/4", "PSG mode"),
            ("\\-5", "detune"),
            ("p6", "pitch down"),
            ("h7", "LFO"),
            ("ho", "LFO"),
            ("hf", "LFO"),
            ("hi", "LFO"),
            ("_c4", "pitch glide"),
            ("y7,9", "register write"),
            ("so", "FM sustain"),
            ("sf", "FM sustain"),
            ("ko", "keyoff"),
            ("kf", "keyoff"),
            ("$", "debug"),
            ("@p1", "LFO"),
            ("@l2", "FM total level"),
            ("@\\3", "fine detune"),
            ("@f4", "fade"),
            ("@o5", "output/control"),
        ] {
            assert!(
                unsupported.iter().any(|(diagnosed, policy)| {
                    *diagnosed == command && policy.contains(policy_fragment)
                }),
                "missing policy fragment {policy_fragment:?} for {command:?}"
            );
        }

        let note_count = song.tracks[0]
            .events
            .iter()
            .filter(|event| matches!(event, IrEvent::Note { .. }))
            .count();
        assert_eq!(note_count, 1);
        assert!(song.diagnostics.parse_warnings.is_empty());
    }

    #[test]
    fn tone_policy_text_meta_preserves_ignored_tone_definitions() {
        let mut opts = ConversionOptions::default();
        opts.tone_policy = crate::ir::TonePolicy::TextMeta;
        let mut song = parse_source("@s0 = { 0, 1 }\n@v1 = { 2 }\n@#2 = 3\n1 c\n", &opts).unwrap();
        let events = collect_midi_events(&mut song, &opts).unwrap();
        let text: Vec<&str> = events
            .iter()
            .filter_map(|event| match &event.kind {
                RenderedEventKind::Text(text) => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert!(text.iter().any(|item| item.contains("@s0 = { 0, 1 }")));
        assert!(text.iter().any(|item| item.contains("@v1 = { 2 }")));
        assert!(text.iter().any(|item| item.contains("@#2 = 3")));
    }

    #[test]
    fn at_number_is_tone_designation_on_notes() {
        let opts = ConversionOptions::default();
        let song = parse_source("1 @10 c d @2 e\n", &opts).unwrap();
        let tones: Vec<Option<u8>> = song.tracks[0]
            .events
            .iter()
            .filter_map(|event| match event {
                IrEvent::Note { tone, .. } => Some(*tone),
                _ => None,
            })
            .collect();
        assert_eq!(tones, vec![Some(10), Some(10), Some(2)]);
    }

    #[test]
    fn at_number_renders_gm_program_change_when_enabled() {
        let mut opts = ConversionOptions::default();
        opts.tone_policy = crate::ir::TonePolicy::GmProgram;
        let mut song = parse_source("1 @10 c\n", &opts).unwrap();
        let events = collect_midi_events(&mut song, &opts).unwrap();
        assert!(events.iter().any(|event| {
            event.abs_tick == 0
                && matches!(
                    event.kind,
                    RenderedEventKind::ProgramChange {
                        channel: 0,
                        program: 6
                    }
                )
        }));
    }

    #[test]
    fn at_number_renders_text_meta_when_enabled() {
        let mut opts = ConversionOptions::default();
        opts.tone_policy = crate::ir::TonePolicy::TextMeta;
        let mut song = parse_source("1 @10 c\n", &opts).unwrap();
        let events = collect_midi_events(&mut song, &opts).unwrap();
        assert!(events.iter().any(|event| {
            event.abs_tick == 0
                && matches!(
                    &event.kind,
                    RenderedEventKind::Text(text) if text == "MGS tone @10"
                )
        }));
    }

    #[test]
    fn control_text_definition_renders_text_meta() {
        let opts = ConversionOptions::default();
        let mut song = parse_source("@m2 = {\"hello\"}\n1 c @m2 d\n", &opts).unwrap();
        let events = collect_midi_events(&mut song, &opts).unwrap();
        assert_eq!(
            song.control_texts.get(&2).map(String::as_str),
            Some("hello")
        );
        assert!(events.iter().any(|event| {
            event.abs_tick == 3600
                && matches!(
                    &event.kind,
                    RenderedEventKind::Text(text) if text == "hello"
                )
        }));
    }

    #[test]
    fn undefined_control_text_warns() {
        let opts = ConversionOptions::default();
        let mut song = parse_source("1 @m3 c\n", &opts).unwrap();
        let _ = collect_midi_events(&mut song, &opts).unwrap();
        assert!(
            song.diagnostics
                .parse_warnings
                .iter()
                .any(|item| item.message == "undefined control text @m3")
        );
    }

    #[test]
    fn macro_offset_applies_to_definition_and_reference() {
        let events = parse_and_render("#macro_offset 10\n*0 = { c%48 }\n1 *0 d%48\n");
        let starts: Vec<(u64, u8)> = events
            .iter()
            .filter_map(|event| match event.kind {
                RenderedEventKind::NoteOn { note, .. } => Some((event.abs_tick, note)),
                _ => None,
            })
            .collect();
        assert_eq!(starts, vec![(0, 60), (3600, 62)]);
    }

    #[test]
    fn play_track_filters_source_tracks() {
        let opts = ConversionOptions::default();
        let mut song = parse_source("#play_track 2a\n12a c\n3 d\n", &opts).unwrap();
        let events = collect_midi_events(&mut song, &opts).unwrap();
        let source_tracks: Vec<&str> = song
            .tracks
            .iter()
            .map(|track| track.source_id.as_str())
            .collect();
        assert_eq!(source_tracks, vec!["2", "a"]);

        let note_channels: Vec<u8> = events
            .iter()
            .filter_map(|event| match event.kind {
                RenderedEventKind::NoteOn { channel, .. } => Some(channel),
                _ => None,
            })
            .collect();
        assert_eq!(note_channels, vec![0, 1]);
    }

    #[test]
    fn octave_base_option_changes_o4c() {
        let mut opts = ConversionOptions::default();
        opts.octave_base = 48;
        let mut song = parse_source("1 c\n", &opts).unwrap();
        let events = collect_midi_events(&mut song, &opts).unwrap();
        let note = events
            .iter()
            .find_map(|event| match event.kind {
                RenderedEventKind::NoteOn { note, .. } => Some(note),
                _ => None,
            })
            .unwrap();
        assert_eq!(note, 48);
    }

    #[test]
    fn same_pitch_slur_merges_note_duration() {
        let events = parse_and_render("1 q4 c&c\n");
        let note_ons: Vec<_> = events
            .iter()
            .filter(|event| matches!(event.kind, RenderedEventKind::NoteOn { .. }))
            .collect();
        assert_eq!(note_ons.len(), 1);
        let note_off = events
            .iter()
            .find(|event| matches!(event.kind, RenderedEventKind::NoteOff { .. }))
            .unwrap();
        assert_eq!(note_off.abs_tick, 7200);
    }

    #[test]
    fn different_pitch_slur_has_no_gap() {
        let events = parse_and_render("1 q4 c&d\n");
        let c_off = events
            .iter()
            .find(|event| matches!(event.kind, RenderedEventKind::NoteOff { note: 60, .. }))
            .unwrap();
        let d_on = events
            .iter()
            .find(|event| matches!(event.kind, RenderedEventKind::NoteOn { note: 62, .. }))
            .unwrap();
        assert_eq!(c_off.abs_tick, 3600);
        assert_eq!(d_on.abs_tick, 3600);
    }

    #[test]
    fn opll_mode_1_rhythm_track_uses_gm_drums_on_channel_10() {
        let events = parse_and_render("#opll_mode 1\nf v15 b s m c h\n");
        let notes: Vec<(u64, u8, u8, u8)> = events
            .iter()
            .filter_map(|event| match event.kind {
                RenderedEventKind::NoteOn {
                    channel,
                    note,
                    velocity,
                } => Some((event.abs_tick, channel, note, velocity)),
                _ => None,
            })
            .collect();
        assert_eq!(
            notes,
            vec![
                (0, 9, 36, 127),
                (3600, 9, 38, 127),
                (7200, 9, 45, 127),
                (10800, 9, 49, 127),
                (14400, 9, 42, 127),
            ]
        );
    }

    #[test]
    fn rhythm_instrument_volume_overrides_track_volume() {
        let events = parse_and_render("#opll_mode 1\nf v0 vb15 b vc8 c\n");
        let notes: Vec<(u8, u8)> = events
            .iter()
            .filter_map(|event| match event.kind {
                RenderedEventKind::NoteOn { note, velocity, .. } => Some((note, velocity)),
                _ => None,
            })
            .collect();
        assert_eq!(notes, vec![(36, 127), (49, 68)]);
    }

    #[test]
    fn rhythm_map_off_keeps_opll_f_track_melodic() {
        let mut opts = ConversionOptions::default();
        opts.rhythm_map = RhythmMap::Off;
        let mut song = parse_source("#opll_mode 1\nf v15 b\n", &opts).unwrap();
        let events = collect_midi_events(&mut song, &opts).unwrap();
        let note = events
            .iter()
            .find_map(|event| match event.kind {
                RenderedEventKind::NoteOn { channel, note, .. } => Some((channel, note)),
                _ => None,
            })
            .unwrap();
        assert_eq!(note, (0, 71));
    }

    #[test]
    fn default_channel_overflow_errors() {
        let opts = ConversionOptions::default();
        let mut song = parse_source("123456789abcdefgh c\n", &opts).unwrap();
        let err = collect_midi_events(&mut song, &opts).unwrap_err();
        assert!(err.contains("MIDI channel overflow"));
    }

    #[test]
    fn shared_channel_overflow_suppresses_envelope_cc_on_shared_channels() {
        let mut opts = ConversionOptions::default();
        opts.channel_overflow = crate::ir::ChannelOverflowPolicy::Shared;
        let mut song = parse_source("@e0 = {1,0,f}\n123456789abcdefgh @e0 c\n", &opts).unwrap();
        let events = collect_midi_events(&mut song, &opts).unwrap();
        let note_ons = events
            .iter()
            .filter(|event| matches!(event.kind, RenderedEventKind::NoteOn { .. }))
            .count();
        let cc11s = events
            .iter()
            .filter(|event| {
                matches!(
                    event.kind,
                    RenderedEventKind::ControlChange { controller: 11, .. }
                )
            })
            .count();
        assert_eq!(note_ons, 17);
        assert_eq!(cc11s, 15);
        assert!(
            song.diagnostics
                .unsupported_commands
                .iter()
                .any(|item| item.command == "channel-overflow shared")
        );
        assert!(
            song.diagnostics
                .unsupported_commands
                .iter()
                .any(|item| item.command == "envelope CC on shared MIDI channel")
        );
    }

    #[test]
    fn multi_port_channel_overflow_uses_midi_port_meta() {
        let mut opts = ConversionOptions::default();
        opts.channel_overflow = crate::ir::ChannelOverflowPolicy::MultiPort;
        let mut song = parse_source("@e0 = {1,0,f}\n123456789abcdefgh @e0 c\n", &opts).unwrap();
        let events = collect_midi_events(&mut song, &opts).unwrap();

        let cc11s = events
            .iter()
            .filter(|event| {
                matches!(
                    event.kind,
                    RenderedEventKind::ControlChange { controller: 11, .. }
                )
            })
            .count();
        assert_eq!(cc11s, 17);

        let last = song.diagnostics.channel_allocations.last().unwrap();
        assert_eq!(last.source_track, "h");
        assert_eq!(last.midi_channel, 0);
        assert_eq!(last.midi_port, 1);

        assert!(events.iter().any(|event| {
            event.track_index == 17 && matches!(event.kind, RenderedEventKind::MidiPort { port: 1 })
        }));
        assert!(song.diagnostics.unsupported_commands.is_empty());
        assert!(song.diagnostics.parse_warnings.is_empty());
    }
}
