pub mod loader;

use std::collections::{BTreeMap, BTreeSet};

use crate::diagnostics::Diagnostics;
use crate::ir::{
    AtContext, AtSpelling, ControlKind, ConversionOptions, ECommand, EnvelopeE, EnvelopeKind,
    EnvelopeR, EnvelopeRef, IrEvent, LoopMarkerKind, MacroOrigin, NoteMeta, PitchGlideMeta,
    Rational, RegisterContext, RhythmMap, SmfRequest, SongIr, SourceFamily, SourceSpan, TempoEvent,
    TrackIr, TrackKind, apply_dots, denominator_to_steps, velocity_from_source,
};
use crate::smfmap::Numbering;

pub fn parse_source(text: &str, options: &ConversionOptions) -> Result<SongIr, String> {
    let mut song = SongIr::new(options.ppq);
    let mut macros: BTreeMap<u32, String> = BTreeMap::new();
    let mut track_chunks: Vec<(
        String,
        usize,
        String,
        MacroOffsetState,
        TrackKind,
        SourceFamily,
    )> = Vec::new();
    let mut top_level_manual_by_track: BTreeMap<String, Vec<IrEvent>> = BTreeMap::new();
    let mut macro_offsets = MacroOffsetState::default();
    let mut opll_mode = 0i32;
    let mut play_track_filter: Option<BTreeSet<String>> = None;
    let mut effective_loop_count = options.loop_count;

    for (line, statement) in collect_statements(text) {
        let raw_trimmed = statement.trim();
        if raw_trimmed.is_empty() {
            continue;
        }
        if let Some(comment_body) = leading_comment_body(raw_trimmed) {
            if effective_loop_count.is_none() {
                effective_loop_count = parse_msxplay_loop_count(comment_body);
            }
            if let Some(directive) = smf_comment_body(comment_body) {
                match parse_smf_directive(
                    directive,
                    None,
                    Rational::ZERO,
                    line,
                    options.smfmap.program_numbering,
                    options.smfmap.channel_numbering,
                ) {
                    Ok(IrEvent::ManualSmf {
                        source_track: Some(track_id),
                        target_channel,
                        source_step,
                        request,
                        source_span,
                    }) => {
                        top_level_manual_by_track
                            .entry(track_id.clone())
                            .or_default()
                            .push(IrEvent::ManualSmf {
                                source_track: Some(track_id),
                                target_channel,
                                source_step,
                                request,
                                source_span,
                            });
                    }
                    Ok(event) => song.conductor_events.push(event),
                    Err(message) => song
                        .diagnostics
                        .add_parse_warning(Some(line), None, message),
                }
            }
            continue;
        }

        if let Some((selectors, body)) = parse_track_line(raw_trimmed) {
            for selector in selectors {
                let kind = track_kind_for_selector(&selector, opll_mode, options.rhythm_map);
                let family = options.smfmap.family_for_track(&selector, opll_mode, kind);
                track_chunks.push((
                    selector,
                    line,
                    body.to_string(),
                    macro_offsets.clone(),
                    kind,
                    family,
                ));
            }
            continue;
        }

        let uncommented = strip_normal_comment(raw_trimmed);
        let trimmed = uncommented.trim();
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
                &mut macro_offsets,
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
            parse_at_hash_assignment(trimmed, line, &mut song);
            record_ignored_tone_definition(trimmed, line, "@#", &mut song.diagnostics);
            continue;
        }
        if lower.starts_with('*') {
            parse_macro_definition(
                trimmed,
                line,
                macro_offsets.numeric,
                &mut macros,
                &mut song.diagnostics,
            );
            continue;
        }
        song.diagnostics.add_parse_warning(
            Some(line),
            None,
            format!("unrecognized top-level statement: {trimmed}"),
        );
    }

    let mut order = Vec::new();
    let mut by_track: BTreeMap<
        String,
        Vec<(usize, String, MacroOffsetState, TrackKind, SourceFamily)>,
    > = BTreeMap::new();
    let mut kind_by_track: BTreeMap<String, TrackKind> = BTreeMap::new();
    let mut seen = BTreeSet::new();
    for (track, line, body, macro_offset, kind, family) in track_chunks {
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
            .push((line, body, macro_offset, kind, family));
    }
    for track in top_level_manual_by_track.keys() {
        if let Some(filter) = &play_track_filter {
            if !filter.contains(track) {
                continue;
            }
        }
        if seen.insert(track.clone()) {
            order.push(track.clone());
        }
        let kind = track_kind_for_selector(track, opll_mode, options.rhythm_map);
        kind_by_track.entry(track.clone()).or_insert(kind);
    }

    for track_id in order {
        let kind = kind_by_track
            .get(&track_id)
            .copied()
            .unwrap_or(TrackKind::Melodic);
        let mut track = TrackIr::new_with_kind(track_id.clone(), kind);
        if let Some(events) = top_level_manual_by_track.get(&track_id) {
            track.events.extend(events.clone());
        }
        let mut state = MmlState::new(options.octave_base);
        if let Some(chunks) = by_track.get(&track_id) {
            for segment in group_mml_segments(chunks) {
                let mut ctx = MmlContext {
                    macros: &macros,
                    macro_offsets: segment.macro_offsets,
                    macro_stack: Vec::new(),
                    track_kind: segment.kind,
                    family: segment.family,
                    loop_count: effective_loop_count,
                    tempo_events: &mut song.tempo_events,
                    diagnostics: &mut song.diagnostics,
                    track_id: &track_id,
                    line: segment.line,
                    program_numbering: options.smfmap.program_numbering,
                    channel_numbering: options.smfmap.channel_numbering,
                    pitch_glide_enabled: options.smfmap.pitch_glide.enabled,
                };
                parse_mml(&segment.body, &mut state, &mut track, &mut ctx, 0)?;
            }
        }
        track.length_steps = state.cursor;
        song.tracks.push(track);
    }
    extend_infinite_loops_to_common_horizon(&mut song.tracks, effective_loop_count);

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
    let mut escaped = false;
    for ch in line.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' if in_quote => escaped = true,
            '"' => in_quote = !in_quote,
            ';' if !in_quote => break,
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
    macro_offsets: &mut MacroOffsetState,
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
            if rest.trim_start().starts_with('{') {
                parse_macro_offset_map(rest, line_no, macro_offsets, &mut song.diagnostics);
            } else if let Some(value) = first_i32(rest) {
                macro_offsets.numeric = value;
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

fn parse_macro_offset_map(
    input: &str,
    line_no: usize,
    macro_offsets: &mut MacroOffsetState,
    diagnostics: &mut Diagnostics,
) {
    let Some(content) = extract_braced(input) else {
        diagnostics.add_parse_warning(
            Some(line_no),
            None,
            "#macro_offset map without braced content",
        );
        return;
    };
    let chars: Vec<char> = content.chars().collect();
    let mut pos = 0;
    let mut found = false;

    while pos < chars.len() {
        skip_ws_and_commas(&chars, &mut pos);
        if pos >= chars.len() {
            break;
        }

        let key = chars[pos].to_ascii_lowercase();
        if !key.is_ascii_alphabetic() {
            diagnostics.add_parse_warning(
                Some(line_no),
                None,
                format!("ignored #macro_offset map character: {}", chars[pos]),
            );
            pos += 1;
            continue;
        }
        pos += 1;
        skip_ws(&chars, &mut pos);

        if chars.get(pos) != Some(&'=') {
            diagnostics.add_parse_warning(
                Some(line_no),
                None,
                format!("#macro_offset map entry '{key}' without '='"),
            );
            skip_macro_offset_map_value(&chars, &mut pos);
            continue;
        }
        pos += 1;

        if let Some(value) = parse_i32_chars(&chars, &mut pos) {
            macro_offsets.named.insert(key, value);
            found = true;
        } else {
            diagnostics.add_parse_warning(
                Some(line_no),
                None,
                format!("#macro_offset map entry '{key}' without value"),
            );
        }
    }

    if !found {
        diagnostics.add_parse_warning(Some(line_no), None, "#macro_offset map without entries");
    }
}

fn skip_ws_and_commas(chars: &[char], pos: &mut usize) {
    while *pos < chars.len() && (chars[*pos].is_whitespace() || chars[*pos] == ',') {
        *pos += 1;
    }
}

fn skip_macro_offset_map_value(chars: &[char], pos: &mut usize) {
    while *pos < chars.len() && chars[*pos] != ',' && !chars[*pos].is_whitespace() {
        *pos += 1;
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

fn parse_at_hash_assignment(line: &str, line_no: usize, song: &mut SongIr) {
    let chars: Vec<char> = line.chars().collect();
    let mut pos = 2;
    let Some(logical) = parse_u32_chars(&chars, &mut pos) else {
        song.diagnostics.add_parse_warning(
            Some(line_no),
            None,
            "invalid @# assignment without tone number",
        );
        return;
    };
    skip_ws(&chars, &mut pos);
    if chars.get(pos) == Some(&'=') {
        pos += 1;
    }
    let Some(rom) = parse_u32_chars(&chars, &mut pos) else {
        song.diagnostics.add_parse_warning(
            Some(line_no),
            None,
            "invalid @# assignment without ROM tone",
        );
        return;
    };
    song.opll_tone_assignments
        .insert(logical.min(255) as u8, rom.min(255) as u8);
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

fn leading_comment_body(input: &str) -> Option<&str> {
    input.trim_start().strip_prefix(';')
}

fn smf_comment_body(comment_body: &str) -> Option<&str> {
    let trimmed = comment_body.trim_start();
    let rest = trimmed.strip_prefix("@smf")?;
    if rest.is_empty() || rest.starts_with(char::is_whitespace) {
        Some(rest.trim_start())
    } else {
        None
    }
}

fn parse_msxplay_loop_count(comment_body: &str) -> Option<u32> {
    let trimmed = comment_body.trim_start();
    let content = trimmed.strip_prefix('[')?.split_once(']')?.0;
    if !content.contains('=') {
        return None;
    }
    let mut saw_msxplay_key = false;
    for token in content.split_whitespace() {
        let Some((key, value)) = token.split_once('=') else {
            continue;
        };
        if matches!(
            key.to_ascii_lowercase().as_str(),
            "gain" | "name" | "duration" | "fade" | "cpu" | "lpf"
        ) {
            saw_msxplay_key = true;
        }
        if key.eq_ignore_ascii_case("loop") {
            return value.parse::<u32>().ok().filter(|count| *count > 0);
        }
    }
    saw_msxplay_key.then_some(2)
}

fn strip_normal_comment(input: &str) -> String {
    let mut in_quote = false;
    let mut escaped = false;
    for (idx, ch) in input.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' if in_quote => escaped = true,
            '"' => in_quote = !in_quote,
            ';' if !in_quote => return input[..idx].to_string(),
            _ => {}
        }
    }
    input.to_string()
}

fn parse_smf_directive(
    input: &str,
    default_track: Option<&str>,
    source_step: Rational,
    line: usize,
    program_numbering: Numbering,
    channel_numbering: Numbering,
) -> Result<IrEvent, String> {
    let mut lexer = SmfLexer::new(input);
    let mut source_track = default_track.map(|track| track.to_ascii_lowercase());
    let mut target_channel = None;
    let mut explicit_target = false;
    let mut token = lexer
        .next_token()
        .ok_or_else(|| ";@smf without command".to_string())?;

    loop {
        if token.eq_ignore_ascii_case("conductor") {
            source_track = None;
            explicit_target = true;
            token = lexer
                .next_token()
                .ok_or_else(|| ";@smf conductor without command".to_string())?;
            continue;
        }
        if let Some(track) = token.strip_prefix("track=") {
            if track.is_empty() {
                return Err(";@smf track= requires a source track id".to_string());
            }
            source_track = Some(track.to_ascii_lowercase());
            explicit_target = true;
            token = lexer
                .next_token()
                .ok_or_else(|| ";@smf track= without command".to_string())?;
            continue;
        }
        if token.starts_with("channel=") {
            let channel = token.trim_start_matches("channel=");
            if channel.is_empty() {
                return Err(";@smf channel= requires a MIDI channel".to_string());
            }
            let channel = parse_smf_number(channel)
                .ok_or_else(|| format!("invalid channel target: {channel}"))?
                .clamp(0, 255) as u8;
            target_channel = Some(channel_numbering.normalize(channel).min(15));
            explicit_target = true;
            token = lexer
                .next_token()
                .ok_or_else(|| ";@smf channel= without command".to_string())?;
            continue;
        }
        break;
    }

    if default_track.is_none() && !explicit_target {
        return Err("top-level ;@smf requires track=<id> or conductor".to_string());
    }

    let command = token.to_ascii_lowercase();
    let request = match command.as_str() {
        "pc" => SmfRequest::Pc {
            program: program_numbering.normalize(parse_smf_u8(&mut lexer, "pc program")?),
        },
        "bank" => SmfRequest::Bank {
            msb: parse_smf_u8(&mut lexer, "bank msb")?,
            lsb: parse_smf_u8(&mut lexer, "bank lsb")?,
        },
        "cc" => SmfRequest::Cc {
            controller: parse_smf_u8(&mut lexer, "cc controller")?,
            value: parse_smf_u8(&mut lexer, "cc value")?,
        },
        "pb" => {
            let value = parse_smf_i32(&mut lexer, "pb value")?;
            if !(-8192..=16383).contains(&value) {
                return Err(format!("pb value out of range: {value}"));
            }
            SmfRequest::PitchBend { value }
        }
        "rpn" => SmfRequest::Rpn {
            msb: parse_smf_u8(&mut lexer, "rpn msb")?,
            lsb: parse_smf_u8(&mut lexer, "rpn lsb")?,
            value: parse_smf_u16_range(&mut lexer, "rpn value", 0, 16383)?,
        },
        "nrpn" => SmfRequest::Nrpn {
            msb: parse_smf_u8(&mut lexer, "nrpn msb")?,
            lsb: parse_smf_u8(&mut lexer, "nrpn lsb")?,
            value: parse_smf_u16_range(&mut lexer, "nrpn value", 0, 16383)?,
        },
        "marker" => SmfRequest::Marker {
            text: lexer
                .next_token()
                .ok_or_else(|| "marker requires text".to_string())?,
        },
        "text" => SmfRequest::Text {
            text: lexer
                .next_token()
                .ok_or_else(|| "text requires text".to_string())?,
        },
        "macro" => SmfRequest::Macro {
            name: lexer
                .next_token()
                .ok_or_else(|| "macro requires name".to_string())?,
        },
        "reset" => SmfRequest::Reset {
            name: lexer.next_token(),
        },
        _ => return Err(format!("unknown ;@smf command: {command}")),
    };

    Ok(IrEvent::ManualSmf {
        source_track,
        target_channel,
        source_step,
        request,
        source_span: SourceSpan {
            line: Some(line),
            track: default_track.map(|track| track.to_string()),
        },
    })
}

struct SmfLexer {
    chars: Vec<char>,
    pos: usize,
}

impl SmfLexer {
    fn new(input: &str) -> Self {
        Self {
            chars: input.chars().collect(),
            pos: 0,
        }
    }

    fn next_token(&mut self) -> Option<String> {
        skip_ws(&self.chars, &mut self.pos);
        if self.pos >= self.chars.len() {
            return None;
        }
        if self.chars[self.pos] == '"' {
            return Some(self.read_quoted());
        }
        let start = self.pos;
        while self.pos < self.chars.len() && !self.chars[self.pos].is_whitespace() {
            self.pos += 1;
        }
        Some(self.chars[start..self.pos].iter().collect())
    }

    fn read_quoted(&mut self) -> String {
        self.pos += 1;
        let mut out = String::new();
        while self.pos < self.chars.len() {
            let ch = self.chars[self.pos];
            self.pos += 1;
            match ch {
                '"' => break,
                '\\' => {
                    if self.pos >= self.chars.len() {
                        out.push('\\');
                        break;
                    }
                    let escaped = self.chars[self.pos];
                    self.pos += 1;
                    match escaped {
                        '"' => out.push('"'),
                        '\\' => out.push('\\'),
                        'n' => out.push('\n'),
                        't' => out.push('\t'),
                        other => {
                            out.push('\\');
                            out.push(other);
                        }
                    }
                }
                other => out.push(other),
            }
        }
        out
    }
}

fn parse_smf_u8(lexer: &mut SmfLexer, name: &str) -> Result<u8, String> {
    let value = parse_smf_i32(lexer, name)?;
    if !(0..=127).contains(&value) {
        return Err(format!("{name} out of range 0..127: {value}"));
    }
    Ok(value as u8)
}

fn parse_smf_u16_range(
    lexer: &mut SmfLexer,
    name: &str,
    min: u16,
    max: u16,
) -> Result<u16, String> {
    let value = parse_smf_i32(lexer, name)?;
    if !(min as i32..=max as i32).contains(&value) {
        return Err(format!("{name} out of range {min}..{max}: {value}"));
    }
    Ok(value as u16)
}

fn parse_smf_i32(lexer: &mut SmfLexer, name: &str) -> Result<i32, String> {
    let token = lexer
        .next_token()
        .ok_or_else(|| format!("{name} requires a value"))?;
    parse_smf_number(&token).ok_or_else(|| format!("invalid {name}: {token}"))
}

fn parse_smf_number(token: &str) -> Option<i32> {
    let (sign, raw) = if let Some(rest) = token.strip_prefix('-') {
        (-1, rest)
    } else if let Some(rest) = token.strip_prefix('+') {
        (1, rest)
    } else {
        (1, token)
    };
    let value = if let Some(hex) = raw.strip_prefix("0x") {
        i32::from_str_radix(hex, 16).ok()?
    } else if let Some(hex) = raw.strip_prefix('$') {
        i32::from_str_radix(hex, 16).ok()?
    } else if let Some(binary) = raw.strip_prefix('%') {
        i32::from_str_radix(binary, 2).ok()?
    } else {
        raw.parse::<i32>().ok()?
    };
    Some(value * sign)
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

#[derive(Clone, Debug, Default)]
struct MacroOffsetState {
    numeric: i32,
    named: BTreeMap<char, i32>,
}

#[derive(Clone, Debug)]
struct MmlSegment {
    line: usize,
    body: String,
    macro_offsets: MacroOffsetState,
    kind: TrackKind,
    family: SourceFamily,
}

fn group_mml_segments(
    chunks: &[(usize, String, MacroOffsetState, TrackKind, SourceFamily)],
) -> Vec<MmlSegment> {
    let mut segments = Vec::new();
    let mut current: Option<MmlSegment> = None;
    let mut loop_depth = 0i32;

    for (line, body, macro_offsets, kind, family) in chunks {
        let segment = current.get_or_insert_with(|| MmlSegment {
            line: *line,
            body: String::new(),
            macro_offsets: macro_offsets.clone(),
            kind: *kind,
            family: *family,
        });
        if !segment.body.is_empty() {
            segment.body.push('\n');
        }
        segment.body.push_str(body);

        loop_depth += mml_loop_bracket_delta(body);
        if loop_depth <= 0 {
            loop_depth = 0;
            if let Some(segment) = current.take() {
                segments.push(segment);
            }
        }
    }

    if let Some(segment) = current {
        segments.push(segment);
    }

    segments
}

fn mml_loop_bracket_delta(input: &str) -> i32 {
    let mut delta = 0;
    let mut in_quote = false;
    let mut escaped = false;

    for ch in input.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' if in_quote => escaped = true,
            '"' => in_quote = !in_quote,
            ';' if !in_quote => break,
            '[' if !in_quote => delta += 1,
            ']' if !in_quote => delta -= 1,
            _ => {}
        }
    }

    delta
}

fn extend_infinite_loops_to_common_horizon(tracks: &mut [TrackIr], loop_count: Option<u32>) {
    let regions: Vec<Option<(Rational, Rational)>> =
        tracks.iter().map(infinite_loop_region).collect();
    let Some(horizon) = tracks
        .iter()
        .map(|track| track.length_steps)
        .max_by(|left, right| rational_cmp(*left, *right))
    else {
        return;
    };

    for (track, region) in tracks.iter_mut().zip(regions) {
        let Some((start, end)) = region else {
            continue;
        };
        if !rational_lt(end, horizon) {
            continue;
        }
        extend_track_loop_to_horizon(track, start, end, horizon, loop_count);
    }
}

fn infinite_loop_region(track: &TrackIr) -> Option<(Rational, Rational)> {
    let mut start = None;
    for event in &track.events {
        match event {
            IrEvent::LoopMarker {
                at_steps,
                kind: LoopMarkerKind::Start,
            } => start = Some(*at_steps),
            IrEvent::LoopMarker {
                at_steps,
                kind: LoopMarkerKind::End,
            } => {
                let Some(loop_start) = start else {
                    continue;
                };
                if rational_lt(loop_start, *at_steps) {
                    return Some((loop_start, *at_steps));
                }
            }
            _ => {}
        }
    }
    None
}

fn extend_track_loop_to_horizon(
    track: &mut TrackIr,
    start: Rational,
    end: Rational,
    horizon: Rational,
    loop_count: Option<u32>,
) {
    let expanded_len = rational_sub(end, start);
    let loop_len = loop_count
        .filter(|count| *count > 0)
        .map(|count| expanded_len.div_i64(count as i64))
        .unwrap_or(expanded_len);
    if !rational_lt(Rational::ZERO, loop_len) {
        return;
    }

    let unit_end = start.add(loop_len);
    let original_events = track.events.clone();
    let mut shift = expanded_len;
    while rational_lt(start.add(shift), horizon) {
        for event in &original_events {
            let Some(step) = event_step(event) else {
                continue;
            };
            if rational_lt(step, start) || !rational_lt(step, unit_end) {
                continue;
            }
            if let Some(shifted) = shift_loop_event(event, shift, horizon) {
                track.events.push(shifted);
            }
        }
        shift = shift.add(loop_len);
    }
    if rational_lt(track.length_steps, horizon) {
        track.length_steps = horizon;
    }
}

fn event_step(event: &IrEvent) -> Option<Rational> {
    match event {
        IrEvent::Note { start_steps, .. } | IrEvent::Rest { start_steps, .. } => Some(*start_steps),
        IrEvent::Tempo { at_steps, .. } | IrEvent::Control { at_steps, .. } => Some(*at_steps),
        IrEvent::LoopMarker { .. } => None,
        IrEvent::AtCommand { source_step, .. }
        | IrEvent::PsgEnvelopeSelect { source_step, .. }
        | IrEvent::ToneChange { source_step, .. }
        | IrEvent::EnvelopeApply { source_step, .. }
        | IrEvent::RegisterWrite { source_step, .. }
        | IrEvent::ManualSmf { source_step, .. } => Some(*source_step),
    }
}

fn shift_loop_event(event: &IrEvent, shift: Rational, horizon: Rational) -> Option<IrEvent> {
    let mut shifted = event.clone();
    match &mut shifted {
        IrEvent::Note {
            start_steps,
            duration_steps,
            ..
        }
        | IrEvent::Rest {
            start_steps,
            duration_steps,
        } => {
            *start_steps = start_steps.add(shift);
            if !rational_lt(*start_steps, horizon) {
                return None;
            }
            let end = start_steps.add(*duration_steps);
            if rational_lt(horizon, end) {
                *duration_steps = rational_sub(horizon, *start_steps);
                if !rational_lt(Rational::ZERO, *duration_steps) {
                    return None;
                }
            }
        }
        IrEvent::Tempo { at_steps, .. } | IrEvent::Control { at_steps, .. } => {
            *at_steps = at_steps.add(shift);
            if !rational_lt(*at_steps, horizon) {
                return None;
            }
        }
        IrEvent::LoopMarker { .. } => return None,
        IrEvent::AtCommand { source_step, .. }
        | IrEvent::PsgEnvelopeSelect { source_step, .. }
        | IrEvent::ToneChange { source_step, .. }
        | IrEvent::EnvelopeApply { source_step, .. }
        | IrEvent::RegisterWrite { source_step, .. }
        | IrEvent::ManualSmf { source_step, .. } => {
            *source_step = source_step.add(shift);
            if !rational_lt(*source_step, horizon) {
                return None;
            }
        }
    }
    Some(shifted)
}

fn rational_sub(left: Rational, right: Rational) -> Rational {
    Rational::new(
        left.num * right.den - right.num * left.den,
        left.den * right.den,
    )
}

fn rational_cmp(left: Rational, right: Rational) -> std::cmp::Ordering {
    let lhs = left.num as i128 * right.den as i128;
    let rhs = right.num as i128 * left.den as i128;
    lhs.cmp(&rhs)
}

fn rational_lt(left: Rational, right: Rational) -> bool {
    rational_cmp(left, right).is_lt()
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
    macro_offsets: MacroOffsetState,
    macro_stack: Vec<MacroFrame>,
    track_kind: TrackKind,
    family: SourceFamily,
    loop_count: Option<u32>,
    tempo_events: &'a mut Vec<TempoEvent>,
    diagnostics: &'a mut Diagnostics,
    track_id: &'a str,
    line: usize,
    program_numbering: Numbering,
    channel_numbering: Numbering,
    pitch_glide_enabled: bool,
}

#[derive(Clone, Debug)]
struct MacroCall {
    origin: MacroOrigin,
    effective_id: u32,
}

#[derive(Clone, Debug)]
struct MacroFrame {
    origin: MacroOrigin,
    note_seen: bool,
}

impl MmlContext<'_> {
    fn note_macro_metadata(&mut self) -> (Vec<MacroOrigin>, bool) {
        let macro_call_start = self.macro_stack.iter().any(|frame| !frame.note_seen);
        let origins = self
            .macro_stack
            .iter()
            .map(|frame| frame.origin.clone())
            .collect();
        for frame in &mut self.macro_stack {
            frame.note_seen = true;
        }
        (origins, macro_call_start)
    }
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
            'y' => parse_register_write(&chars, &mut pos, state, track, ctx),
            ';' => {
                if !parse_comment(&chars, &mut pos, state, track, ctx) {
                    break;
                }
            }
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
                let Some(macro_call) = parse_macro_reference(&chars, &mut pos, ctx)? else {
                    continue;
                };
                if let Some(body) = ctx.macros.get(&macro_call.effective_id).cloned() {
                    ctx.macro_stack.push(MacroFrame {
                        origin: macro_call.origin,
                        note_seen: false,
                    });
                    let result = parse_mml(&body, state, track, ctx, depth + 1);
                    ctx.macro_stack.pop();
                    result?;
                } else {
                    return Err(format!(
                        "undefined macro {} (effective *{}) at line {}",
                        macro_call.origin.token, macro_call.effective_id, ctx.line
                    ));
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

fn parse_macro_reference(
    chars: &[char],
    pos: &mut usize,
    ctx: &mut MmlContext<'_>,
) -> Result<Option<MacroCall>, String> {
    if let Some(prefix) = chars
        .get(*pos)
        .copied()
        .filter(|ch| ch.is_ascii_alphabetic())
    {
        *pos += 1;
        let prefix = prefix.to_ascii_lowercase();
        let Some(id) = parse_u32_chars(chars, pos) else {
            ctx.diagnostics.add_parse_warning(
                Some(ctx.line),
                Some(ctx.track_id.to_string()),
                format!("macro reference *{prefix} without number"),
            );
            return Ok(None);
        };
        let Some(base) = ctx.macro_offsets.named.get(&prefix).copied() else {
            ctx.diagnostics.add_parse_warning(
                Some(ctx.line),
                Some(ctx.track_id.to_string()),
                format!("undefined #macro_offset prefix: {prefix}"),
            );
            return Ok(None);
        };
        let Some(effective_id) = apply_macro_offset(id, base) else {
            return Err(format!(
                "macro id out of range after #macro_offset: *{prefix}{id} + {base} at line {}",
                ctx.line
            ));
        };
        return Ok(Some(MacroCall {
            origin: MacroOrigin {
                token: format!("*{prefix}{id}"),
                symbol: Some(prefix),
                number: id,
                resolved_number: effective_id,
            },
            effective_id,
        }));
    }

    if let Some(id) = parse_u32_chars(chars, pos) {
        let Some(effective_id) = apply_macro_offset(id, ctx.macro_offsets.numeric) else {
            return Err(format!(
                "macro id out of range after #macro_offset: *{id} + {} at line {}",
                ctx.macro_offsets.numeric, ctx.line
            ));
        };
        return Ok(Some(MacroCall {
            origin: MacroOrigin {
                token: format!("*{id}"),
                symbol: None,
                number: id,
                resolved_number: effective_id,
            },
            effective_id,
        }));
    }

    ctx.diagnostics.add_parse_warning(
        Some(ctx.line),
        Some(ctx.track_id.to_string()),
        "macro reference without number",
    );
    Ok(None)
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

    let source_octave = state.octave;
    let parsed_pitch_glide = parse_pitch_glide_suffix(chars, pos, state, ctx);
    let mut length = parsed_pitch_glide
        .as_ref()
        .map(|glide| glide.length)
        .or_else(|| parse_length(chars, pos, Some(state.default_length), ctx))
        .unwrap_or(state.default_length);
    while consume_tie_extension(chars, pos, &mut length, state.default_length, ctx) {}
    let slur_to_next = consume_slur(chars, pos);

    let raw_note = state.octave_base + (source_octave - 4) * 12 + semitone;
    if !(0..=127).contains(&raw_note) {
        ctx.diagnostics.add_parse_warning(
            Some(ctx.line),
            Some(ctx.track_id.to_string()),
            format!("MIDI note out of range and skipped: {raw_note}"),
        );
        state.cursor = state.cursor.add(length);
        return;
    }

    if state.slur_from_previous
        && parsed_pitch_glide.is_none()
        && extend_last_note_if_same(track, raw_note as u8, length)
    {
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

    let slur_from_previous = state.slur_from_previous;
    let (macro_origin, macro_call_start) = ctx.note_macro_metadata();
    track.events.push(IrEvent::Note {
        start_steps: state.cursor,
        duration_steps: duration,
        note: raw_note as u8,
        velocity: velocity_from_source(state.volume),
        tone: state.tone,
        envelope: state.envelope,
        meta: NoteMeta::from_source_note(
            note_char,
            source_octave,
            raw_note as u8,
            parsed_pitch_glide.and_then(|glide| glide.meta),
            macro_origin,
            macro_call_start,
            slur_from_previous,
            slur_to_next,
        ),
    });
    state.cursor = state.cursor.add(length);
    state.slur_from_previous = slur_to_next;
}

#[derive(Clone, Debug)]
struct ParsedPitchGlide {
    length: Rational,
    meta: Option<PitchGlideMeta>,
}

fn parse_pitch_glide_suffix(
    chars: &[char],
    pos: &mut usize,
    state: &mut MmlState,
    ctx: &mut MmlContext<'_>,
) -> Option<ParsedPitchGlide> {
    let before_ws = *pos;
    skip_ws(chars, pos);
    if chars.get(*pos) != Some(&'_') {
        *pos = before_ws;
        return None;
    }

    let command_start = *pos;
    *pos += 1;
    skip_ws(chars, pos);
    while let Some(ch) = chars.get(*pos) {
        match ch {
            '<' => {
                state.octave -= 1;
                *pos += 1;
                skip_ws(chars, pos);
            }
            '>' => {
                state.octave += 1;
                *pos += 1;
                skip_ws(chars, pos);
            }
            _ => break,
        }
    }

    let Some(target_note) = chars.get(*pos).copied().map(|ch| ch.to_ascii_lowercase()) else {
        record_pitch_glide_unsupported(chars, command_start, *pos, ctx);
        return Some(ParsedPitchGlide {
            length: state.default_length,
            meta: None,
        });
    };
    if !matches!(target_note, 'a'..='g') {
        record_pitch_glide_unsupported(chars, command_start, *pos, ctx);
        return Some(ParsedPitchGlide {
            length: state.default_length,
            meta: None,
        });
    }

    let mut target_semitone = note_semitone(target_note).unwrap_or(0);
    *pos += 1;
    while let Some(ch) = chars.get(*pos) {
        match ch {
            '+' | '#' => {
                target_semitone += 1;
                *pos += 1;
            }
            '-' => {
                target_semitone -= 1;
                *pos += 1;
            }
            _ => break,
        }
    }
    let length =
        parse_length(chars, pos, Some(state.default_length), ctx).unwrap_or(state.default_length);
    let command: String = chars[command_start..*pos].iter().collect();
    let raw_target = state.octave_base + (state.octave - 4) * 12 + target_semitone;
    let meta = if (0..=127).contains(&raw_target) {
        Some(PitchGlideMeta {
            target_midi_note: raw_target as u8,
            source: command,
        })
    } else {
        ctx.diagnostics.add_parse_warning(
            Some(ctx.line),
            Some(ctx.track_id.to_string()),
            format!("pitch glide target MIDI note out of range and ignored: {raw_target}"),
        );
        None
    };
    if !ctx.pitch_glide_enabled {
        record_pitch_glide_unsupported(chars, command_start, *pos, ctx);
    }
    Some(ParsedPitchGlide { length, meta })
}

fn record_pitch_glide_unsupported(
    chars: &[char],
    start: usize,
    end: usize,
    ctx: &mut MmlContext<'_>,
) {
    let command: String = chars[start..end].iter().collect();
    let policy = unsupported_mml_policy(&command);
    ctx.diagnostics.add_unsupported(
        Some(ctx.line),
        Some(ctx.track_id.to_string()),
        command,
        policy,
    );
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
        meta: NoteMeta::default(),
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
    let extra = parse_length(chars, pos, Some(default_length), ctx).unwrap_or(default_length);
    *length = length.add(extra);
    true
}

fn consume_slur(chars: &[char], pos: &mut usize) -> bool {
    let mut consumed = false;
    loop {
        skip_ws(chars, pos);
        if chars.get(*pos) != Some(&'&') {
            break;
        }
        *pos += 1;
        consumed = true;
    }
    consumed
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
            let number = id.min(255) as u8;
            let spelling = if next == 'e' {
                AtSpelling::AtE
            } else {
                AtSpelling::AtR
            };
            let kind = if next == 'e' {
                EnvelopeKind::E
            } else {
                EnvelopeKind::R
            };
            let reference = if next == 'e' {
                EnvelopeRef::E(id.min(255) as u8)
            } else {
                EnvelopeRef::R(id.min(255) as u8)
            };
            state.envelope = Some(reference);
            track.events.push(IrEvent::AtCommand {
                source_track: ctx.track_id.to_string(),
                source_step: state.cursor,
                family: ctx.family,
                number,
                spelling,
                context: AtContext::NormalMml,
                source_span: source_span(ctx),
            });
            match ctx.family {
                SourceFamily::Psg | SourceFamily::PsgNoise => {
                    track.events.push(IrEvent::PsgEnvelopeSelect {
                        source_track: ctx.track_id.to_string(),
                        source_step: state.cursor,
                        envelope_number: number,
                        kind,
                    });
                }
                SourceFamily::Scc | SourceFamily::Opll => {
                    track.events.push(IrEvent::EnvelopeApply {
                        source_track: ctx.track_id.to_string(),
                        source_step: state.cursor,
                        family: ctx.family,
                        envelope_number: number,
                        kind,
                    });
                }
                SourceFamily::Rhythm => {}
            }
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
        let number = id.min(255) as u8;
        track.events.push(IrEvent::AtCommand {
            source_track: ctx.track_id.to_string(),
            source_step: state.cursor,
            family: ctx.family,
            number,
            spelling: AtSpelling::At,
            context: AtContext::NormalMml,
            source_span: source_span(ctx),
        });
        match ctx.family {
            SourceFamily::Psg | SourceFamily::PsgNoise => {
                state.envelope = Some(EnvelopeRef::E(number));
                track.events.push(IrEvent::PsgEnvelopeSelect {
                    source_track: ctx.track_id.to_string(),
                    source_step: state.cursor,
                    envelope_number: number,
                    kind: EnvelopeKind::At,
                });
            }
            SourceFamily::Scc | SourceFamily::Opll => {
                state.tone = Some(number);
                track.events.push(IrEvent::ToneChange {
                    source_track: ctx.track_id.to_string(),
                    source_step: state.cursor,
                    family: ctx.family,
                    tone_number: number,
                });
            }
            SourceFamily::Rhythm => {}
        }
        return;
    }

    record_unsupported_at_command(chars, pos, ctx);
}

fn parse_register_write(
    chars: &[char],
    pos: &mut usize,
    state: &mut MmlState,
    track: &mut TrackIr,
    ctx: &mut MmlContext<'_>,
) {
    *pos += 1;
    let register = parse_i32_chars(chars, pos);
    skip_ws(chars, pos);
    if chars.get(*pos) == Some(&',') {
        *pos += 1;
    }
    let data = parse_i32_chars(chars, pos);
    match (register, data) {
        (Some(register), Some(data)) => {
            let register = register.clamp(0, 255) as u8;
            let data = data.clamp(0, 255) as u8;
            track.events.push(IrEvent::RegisterWrite {
                source_track: ctx.track_id.to_string(),
                source_step: state.cursor,
                family: ctx.family,
                register,
                data,
                context: RegisterContext::NormalMml,
                source_span: source_span(ctx),
            });
        }
        _ => ctx.diagnostics.add_parse_warning(
            Some(ctx.line),
            Some(ctx.track_id.to_string()),
            "y register write without register/data",
        ),
    }
}

fn parse_comment(
    chars: &[char],
    pos: &mut usize,
    state: &MmlState,
    track: &mut TrackIr,
    ctx: &mut MmlContext<'_>,
) -> bool {
    let comment_start = *pos + 1;
    let mut end = comment_start;
    while end < chars.len() && chars[end] != '\n' {
        end += 1;
    }
    let body: String = chars[comment_start..end].iter().collect();
    if let Some(directive) = smf_comment_body(&body) {
        match parse_smf_directive(
            directive,
            Some(ctx.track_id),
            state.cursor,
            ctx.line,
            ctx.program_numbering,
            ctx.channel_numbering,
        ) {
            Ok(event) => track.events.push(event),
            Err(message) => ctx.diagnostics.add_parse_warning(
                Some(ctx.line),
                Some(ctx.track_id.to_string()),
                message,
            ),
        }
    }
    *pos = if end < chars.len() { end + 1 } else { end };
    end < chars.len()
}

fn source_span(ctx: &MmlContext<'_>) -> SourceSpan {
    SourceSpan {
        line: Some(ctx.line),
        track: Some(ctx.track_id.to_string()),
    }
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
            parse_mml(&after, state, track, ctx, depth + 1)?;
        }
        parse_mml(&before, state, track, ctx, depth + 1)?;
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
    fn multiline_infinite_loop_generates_markers() {
        let events = parse_and_render("1 [0\n1 c4\n1 ]\n");
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
        let note_ons = events
            .iter()
            .filter(|event| matches!(event.kind, RenderedEventKind::NoteOn { note: 60, .. }))
            .count();
        assert_eq!(note_ons, 1);
    }

    #[test]
    fn earlier_infinite_loop_extends_to_common_default_horizon() {
        let events = parse_and_render("1 r1 [0 c4]\n2 r1r1 [0 d4]\n");
        let c_starts: Vec<u64> = events
            .iter()
            .filter_map(|event| match event.kind {
                RenderedEventKind::NoteOn { note: 60, .. } => Some(event.abs_tick),
                _ => None,
            })
            .collect();
        let d_starts: Vec<u64> = events
            .iter()
            .filter_map(|event| match event.kind {
                RenderedEventKind::NoteOn { note: 62, .. } => Some(event.abs_tick),
                _ => None,
            })
            .collect();
        assert_eq!(c_starts, vec![14400, 18000, 21600, 25200, 28800]);
        assert_eq!(d_starts, vec![28800]);
    }

    #[test]
    fn explicit_loop_count_extends_to_common_horizon() {
        let mut opts = ConversionOptions::default();
        opts.loop_count = Some(2);
        let mut song = parse_source("1 r1 [0 c4]\n2 r1r1 [0 d4]\n", &opts).unwrap();
        let events = collect_midi_events(&mut song, &opts).unwrap();
        let c_starts: Vec<u64> = events
            .iter()
            .filter_map(|event| match event.kind {
                RenderedEventKind::NoteOn { note: 60, .. } => Some(event.abs_tick),
                _ => None,
            })
            .collect();
        assert_eq!(c_starts, vec![14400, 18000, 21600, 25200, 28800, 32400]);
    }

    #[test]
    fn msxplay_header_defaults_to_two_global_loops() {
        let events = parse_and_render(";[gain=1.0]\n1 r1 [0 c4]\n2 r1r1 [0 d4]\n");
        let c_starts: Vec<u64> = events
            .iter()
            .filter_map(|event| match event.kind {
                RenderedEventKind::NoteOn { note: 60, .. } => Some(event.abs_tick),
                _ => None,
            })
            .collect();
        assert_eq!(c_starts, vec![14400, 18000, 21600, 25200, 28800, 32400]);
    }

    #[test]
    fn multiline_finite_loop_repeats_body() {
        let events = parse_and_render("1 [2\n1 c4\n1 ]\n");
        let starts: Vec<u64> = events
            .iter()
            .filter_map(|event| match event.kind {
                RenderedEventKind::NoteOn { note: 60, .. } => Some(event.abs_tick),
                _ => None,
            })
            .collect();
        assert_eq!(starts, vec![0, 3600]);
    }

    #[test]
    fn loop_alternative_exits_on_final_iteration() {
        let events = parse_and_render("1 [3 c|d] e\n");
        let starts: Vec<(u64, u8)> = events
            .iter()
            .filter_map(|event| match event.kind {
                RenderedEventKind::NoteOn { note, .. } => Some((event.abs_tick, note)),
                _ => None,
            })
            .collect();
        assert_eq!(
            starts,
            vec![
                (0, 60),
                (3600, 62),
                (7200, 60),
                (10800, 62),
                (14400, 60),
                (18000, 64),
            ]
        );
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
                "m1", "s2", "n3", "/4", "\\-5", "p6", "h7", "ho", "hf", "hi", "_c4", "so", "sf",
                "ko", "kf", "$", "@p1", "@l2", "@\\3", "@f4", "@o5"
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
        assert!(song.tracks[0].events.iter().any(|event| {
            matches!(
                event,
                IrEvent::RegisterWrite {
                    register: 7,
                    data: 9,
                    ..
                }
            )
        }));
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
    fn at_number_is_tone_designation_on_scc_notes() {
        let opts = ConversionOptions::default();
        let song = parse_source("4 @10 c d @2 e\n", &opts).unwrap();
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
    fn psg_at_number_selects_envelope_and_never_program_change() {
        let opts = ConversionOptions::default();
        let mut song = parse_source("@e1 = {1,0,f}\n1 @1 c4\n", &opts).unwrap();
        let events = collect_midi_events(&mut song, &opts).unwrap();
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
        assert!(
            !events
                .iter()
                .any(|event| { matches!(event.kind, RenderedEventKind::ProgramChange { .. }) })
        );
    }

    #[test]
    fn opll_at_number_renders_default_tone_map_program() {
        let opts = ConversionOptions::default();
        let mut song = parse_source("9 @10 c\n", &opts).unwrap();
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
    fn manual_smf_pc_is_before_next_note() {
        let opts = ConversionOptions::default();
        let mut song = parse_source("9 ;@smf pc 80\n9 c4\n", &opts).unwrap();
        let events = collect_midi_events(&mut song, &opts).unwrap();
        let pc_index = events
            .iter()
            .position(|event| {
                event.abs_tick == 0
                    && matches!(
                        event.kind,
                        RenderedEventKind::ProgramChange { program: 80, .. }
                    )
            })
            .unwrap();
        let note_index = events
            .iter()
            .position(|event| {
                event.abs_tick == 0 && matches!(event.kind, RenderedEventKind::NoteOn { .. })
            })
            .unwrap();
        assert!(pc_index < note_index);
    }

    #[test]
    fn manual_smf_channel_target_uses_explicit_channel() {
        let mut opts = ConversionOptions::default();
        opts.smfmap.channel_numbering = crate::smfmap::Numbering::OneBased;
        let mut song = parse_source(";@smf channel=3 pc 80\n1 c4\n", &opts).unwrap();
        let events = collect_midi_events(&mut song, &opts).unwrap();
        assert!(events.iter().any(|event| {
            event.track_index == 0
                && matches!(
                    event.kind,
                    RenderedEventKind::ProgramChange {
                        channel: 2,
                        program: 80
                    }
                )
        }));
    }

    #[test]
    fn manual_smf_macro_expands_from_yaml() {
        let mut opts = ConversionOptions::default();
        opts.smfmap
            .merge_yaml_str(
                r#"
smf_macros:
  bright:
    events:
      - pc: { program: 80 }
      - cc: { controller: 74, value: 105 }
"#,
            )
            .unwrap();
        let mut song = parse_source("9 ;@smf macro bright\n9 c4\n", &opts).unwrap();
        let events = collect_midi_events(&mut song, &opts).unwrap();
        assert!(events.iter().any(|event| {
            matches!(
                event.kind,
                RenderedEventKind::ProgramChange { program: 80, .. }
            )
        }));
        assert!(events.iter().any(|event| {
            matches!(
                event.kind,
                RenderedEventKind::ControlChange {
                    controller: 74,
                    value: 105,
                    ..
                }
            )
        }));
    }

    #[test]
    fn one_based_program_numbering_normalizes_yaml_and_manual_pc() {
        let mut opts = ConversionOptions::default();
        opts.smfmap
            .merge_yaml_str(
                r#"
smf:
  program_numbering: one_based
tone_map:
  scc:
    tones:
      "1":
        events:
          - pc: { program: 82 }
"#,
            )
            .unwrap();
        let mut song = parse_source("4 @1 ;@smf pc 81 c4\n", &opts).unwrap();
        let events = collect_midi_events(&mut song, &opts).unwrap();
        assert!(events.iter().any(|event| {
            matches!(
                event.kind,
                RenderedEventKind::ProgramChange { program: 81, .. }
            )
        }));
        assert!(events.iter().any(|event| {
            matches!(
                event.kind,
                RenderedEventKind::ProgramChange { program: 80, .. }
            )
        }));
    }

    #[test]
    fn scc_tone_map_from_yaml_emits_program_change() {
        let mut opts = ConversionOptions::default();
        opts.smfmap
            .merge_yaml_str(
                r#"
tone_map:
  scc:
    tones:
      "1":
        events:
          - pc: { program: 81 }
"#,
            )
            .unwrap();
        let mut song = parse_source("4 @1 c4\n", &opts).unwrap();
        let events = collect_midi_events(&mut song, &opts).unwrap();
        assert!(events.iter().any(|event| {
            matches!(
                event.kind,
                RenderedEventKind::ProgramChange { program: 81, .. }
            )
        }));
    }

    #[test]
    fn opll_at_hash_assignment_redirects_logical_tone_to_rom_tone() {
        let opts = ConversionOptions::default();
        let mut song = parse_source("@#2 = 14\n9 @2 c4\n", &opts).unwrap();
        let events = collect_midi_events(&mut song, &opts).unwrap();
        assert_eq!(song.opll_tone_assignments.get(&2).copied(), Some(14));
        assert!(events.iter().any(|event| {
            matches!(
                event.kind,
                RenderedEventKind::ProgramChange { program: 33, .. }
            )
        }));
    }

    #[test]
    fn psg_envelope_map_custom_cc_is_used() {
        let mut opts = ConversionOptions::default();
        opts.smfmap
            .merge_yaml_str(
                r#"
psg_envelope_map:
  default:
    expression_cc: 12
"#,
            )
            .unwrap();
        let mut song = parse_source("@e1 = {1,0,f}\n1 @1 c4\n", &opts).unwrap();
        let events = collect_midi_events(&mut song, &opts).unwrap();
        assert!(events.iter().any(|event| {
            matches!(
                event.kind,
                RenderedEventKind::ControlChange {
                    controller: 12,
                    value: 127,
                    ..
                }
            )
        }));
        assert!(!events.iter().any(|event| {
            matches!(
                event.kind,
                RenderedEventKind::ControlChange { controller: 11, .. }
            )
        }));
    }

    #[test]
    fn scc_envelope_map_can_use_volume_cc() {
        let mut opts = ConversionOptions::default();
        opts.smfmap
            .merge_yaml_str(
                r#"
envelope_map:
  default_mode: cc7
  midi_cc:
    volume: 7
"#,
            )
            .unwrap();
        let mut song = parse_source("@e1 = {1,0,f}\n4 @e1 c4\n", &opts).unwrap();
        let events = collect_midi_events(&mut song, &opts).unwrap();
        assert!(events.iter().any(|event| {
            matches!(
                event.kind,
                RenderedEventKind::ControlChange {
                    controller: 7,
                    value: 127,
                    ..
                }
            )
        }));
    }

    #[test]
    fn psg_noise_drum_map_replaces_melody_note() {
        let mut opts = ConversionOptions::default();
        opts.smfmap
            .merge_yaml_str(
                r#"
psg_noise_map:
  default_action: drum_map
  drum_map:
    enabled: true
    channel: 9
    envelopes:
      "1":
        name: snare
        note: 38
        velocity: 100
"#,
            )
            .unwrap();
        let mut song = parse_source("@e1 = {1,0,f}\n3 @1 c4\n", &opts).unwrap();
        let events = collect_midi_events(&mut song, &opts).unwrap();
        assert!(events.iter().any(|event| {
            matches!(
                event.kind,
                RenderedEventKind::NoteOn {
                    channel: 9,
                    note: 38,
                    velocity: 100
                }
            )
        }));
        assert!(
            !events
                .iter()
                .any(|event| { matches!(event.kind, RenderedEventKind::NoteOn { note: 60, .. }) })
        );
    }

    #[test]
    fn psg_noise_drum_map_reserves_drum_channel_from_melodic_allocation() {
        let mut opts = ConversionOptions::default();
        opts.channel_overflow = crate::ir::ChannelOverflowPolicy::MultiPort;
        opts.smfmap
            .merge_yaml_str(
                r#"
psg_noise_map:
  default_action: drum_map
  drum_map:
    enabled: true
    channel: 9
    envelopes:
      "1": { name: snare, note: 38, velocity: 100 }
"#,
            )
            .unwrap();

        let mut song = parse_source("123456789abcdefgh @1 c4\n", &opts).unwrap();
        let events = collect_midi_events(&mut song, &opts).unwrap();

        assert!(
            song.diagnostics
                .channel_allocations
                .iter()
                .all(|allocation| allocation.midi_channel != 9)
        );
        assert!(
            song.diagnostics
                .channel_allocations
                .iter()
                .any(|allocation| allocation.midi_port == 1)
        );
        assert!(events.iter().any(|event| {
            matches!(
                event.kind,
                RenderedEventKind::NoteOn {
                    channel: 9,
                    note: 38,
                    velocity: 100
                }
            )
        }));
    }

    #[test]
    fn opll_pseudo_drum_map_replaces_macro_notes_and_suppresses_tone_map() {
        let mut opts = ConversionOptions::default();
        opts.smfmap
            .merge_yaml_str(
                r#"
tracks:
  "h":
    source_family: opll
    render_role: drum
    drum_map: test_opll_pseudo

opll_pseudo_drum_map:
  enabled: true
  maps:
    test_opll_pseudo:
      output:
        midi_channel: 9
        mode: replace
        unmatched: warn_and_drop
        suppress_tone_map: true
      rules:
        - name: kick
          match: { macro_symbol: "b" }
          drum: { note: 36, velocity: source_volume }

tone_map:
  opll:
    tones:
      "15":
        events:
          - pc: { program: 0 }
"#,
            )
            .unwrap();

        let mut song = parse_source(
            "*1 = { @15 v15 c4 }\n#macro_offset { b = 0 }\nh *b01\n",
            &opts,
        )
        .unwrap();
        let events = collect_midi_events(&mut song, &opts).unwrap();
        let allocation = song.diagnostics.channel_allocations.first().unwrap();
        assert_eq!(allocation.source_track, "h");
        assert_eq!(allocation.midi_channel, 9);

        assert!(events.iter().any(|event| {
            matches!(
                event.kind,
                RenderedEventKind::NoteOn {
                    channel: 9,
                    note: 36,
                    velocity: 127
                }
            )
        }));
        assert!(!events.iter().any(|event| {
            matches!(
                event.kind,
                RenderedEventKind::ProgramChange { program: 0, .. }
            )
        }));
        assert!(
            !events
                .iter()
                .any(|event| { matches!(event.kind, RenderedEventKind::NoteOn { note: 60, .. }) })
        );
    }

    #[test]
    fn opll_pseudo_drum_hit_grouping_suppresses_macro_continuations() {
        let mut opts = ConversionOptions::default();
        opts.smfmap
            .merge_yaml_str(
                r#"
tracks:
  "h":
    source_family: opll
    render_role: drum
    drum_map: grouped

opll_pseudo_drum_map:
  enabled: true
  maps:
    grouped:
      output:
        midi_channel: 9
        mode: replace
      hit_grouping:
        enabled: true
        suppress_continuations:
          - macro_continuation
      rules:
        - match: { macro_symbol: "b" }
          drum: { note: 36, velocity: source_volume }
"#,
            )
            .unwrap();

        let mut song =
            parse_source("*1 = { c16 d16 }\n#macro_offset { b = 0 }\nh *b01\n", &opts).unwrap();
        let events = collect_midi_events(&mut song, &opts).unwrap();
        let kick_count = events
            .iter()
            .filter(|event| {
                matches!(
                    event.kind,
                    RenderedEventKind::NoteOn {
                        channel: 9,
                        note: 36,
                        ..
                    }
                )
            })
            .count();
        assert_eq!(kick_count, 1);
    }

    #[test]
    fn family_rhythm_shorthand_distinguishes_native_and_pseudo_opll_drums() {
        let mut opts = ConversionOptions::default();
        opts.smfmap
            .merge_yaml_str(
                r#"
tracks:
  "f": { family: rhythm }
  "h": { family: rhythm }

opll_pseudo_drum_map:
  enabled: true
"#,
            )
            .unwrap();

        assert_eq!(
            opts.smfmap.family_for_track("f", 1, TrackKind::Rhythm),
            SourceFamily::Rhythm
        );
        assert_eq!(
            opts.smfmap.family_for_track("h", 0, TrackKind::Melodic),
            SourceFamily::Opll
        );
        assert!(
            opts.smfmap
                .opll_pseudo_drum_map_for_track("f", TrackKind::Rhythm)
                .is_none()
        );
        assert!(
            opts.smfmap
                .opll_pseudo_drum_map_for_track("h", TrackKind::Melodic)
                .is_some()
        );
    }

    #[test]
    fn track_config_can_set_midi_port_and_channel() {
        let mut opts = ConversionOptions::default();
        opts.smfmap
            .merge_yaml_str(
                r#"
tracks:
  "1":
    midi_channel: 5
    midi_port: 2
"#,
            )
            .unwrap();
        let mut song = parse_source("1 c4\n", &opts).unwrap();
        let events = collect_midi_events(&mut song, &opts).unwrap();
        let allocation = song.diagnostics.channel_allocations.first().unwrap();
        assert_eq!(allocation.midi_channel, 5);
        assert_eq!(allocation.midi_port, 2);
        assert!(events.iter().any(|event| {
            event.track_index == 1 && matches!(event.kind, RenderedEventKind::MidiPort { port: 2 })
        }));
    }

    #[test]
    fn track_config_events_emit_initial_program_change() {
        let mut opts = ConversionOptions::default();
        opts.smfmap
            .merge_yaml_str(
                r#"
tracks:
  "3":
    midi_channel: 4
    events:
      - pc: { program: 38 }
"#,
            )
            .unwrap();
        let mut song = parse_source("3 c4\n", &opts).unwrap();
        let events = collect_midi_events(&mut song, &opts).unwrap();
        let pc_index = events
            .iter()
            .position(|event| matches!(event.kind, RenderedEventKind::ProgramChange { .. }))
            .unwrap();
        let note_index = events
            .iter()
            .position(|event| matches!(event.kind, RenderedEventKind::NoteOn { .. }))
            .unwrap();
        let pc = &events[pc_index];
        assert_eq!(pc.abs_tick, 0);
        assert!(pc_index < note_index);
        assert!(matches!(
            pc.kind,
            RenderedEventKind::ProgramChange {
                channel: 4,
                program: 38
            }
        ));
    }

    #[test]
    fn opll_register_write_is_reported_and_emits_no_smf_event() {
        let opts = ConversionOptions::default();
        let mut song = parse_source("9 y14,32 c4\n", &opts).unwrap();
        let events = collect_midi_events(&mut song, &opts).unwrap();
        assert!(song.diagnostics.register_write_usage.iter().any(|item| {
            item.track == "9" && item.family == "opll" && item.register == 14 && item.data == 32
        }));
        assert!(
            song.diagnostics
                .unsupported_commands
                .iter()
                .any(|item| item.command == "y14,32")
        );
        assert!(!events.iter().any(|event| {
            matches!(
                event.kind,
                RenderedEventKind::ProgramChange { .. }
                    | RenderedEventKind::ControlChange { .. }
                    | RenderedEventKind::PitchBend { .. }
            )
        }));
    }

    #[test]
    fn register_map_apply_rules_emits_configured_event() {
        let mut opts = ConversionOptions::default();
        opts.smfmap
            .merge_yaml_str(
                r#"
opll_register_map:
  default_policy: apply_rules
  rules:
    - name: cutoff
      enabled: true
      match: { family: opll, source_track: "*", register: 14, data: 32, context: "*" }
      emit:
        - cc: { controller: 74, value: 100 }
"#,
            )
            .unwrap();
        let mut song = parse_source("9 y14,32 c4\n", &opts).unwrap();
        let events = collect_midi_events(&mut song, &opts).unwrap();
        assert!(events.iter().any(|event| {
            matches!(
                event.kind,
                RenderedEventKind::ControlChange {
                    controller: 74,
                    value: 100,
                    ..
                }
            )
        }));
        assert!(
            !song
                .diagnostics
                .unsupported_commands
                .iter()
                .any(|item| item.policy == "apply_rules: no matching register rule")
        );
    }

    #[test]
    fn envelope_register_write_routes_to_register_map() {
        let mut opts = ConversionOptions::default();
        opts.smfmap
            .merge_yaml_str(
                r#"
opll_register_map:
  default_policy: apply_rules
  rules:
    - name: env_y
      enabled: true
      match: { family: opll, source_track: "*", register: 7, data: 9, context: EnvelopeData }
      emit:
        - cc: { controller: 74, value: 99 }
"#,
            )
            .unwrap();
        let mut song = parse_source("@e0 = {1,0,y7,9,f}\n9 @e0 c4\n", &opts).unwrap();
        let events = collect_midi_events(&mut song, &opts).unwrap();
        assert!(events.iter().any(|event| {
            matches!(
                event.kind,
                RenderedEventKind::ControlChange {
                    controller: 74,
                    value: 99,
                    ..
                }
            )
        }));
    }

    #[test]
    fn envelope_tone_change_routes_to_tone_map() {
        let opts = ConversionOptions::default();
        let mut song = parse_source("@e0 = {1,0,@14,f}\n9 @e0 c4\n", &opts).unwrap();
        let events = collect_midi_events(&mut song, &opts).unwrap();
        assert!(events.iter().any(|event| {
            matches!(
                event.kind,
                RenderedEventKind::ProgramChange { program: 33, .. }
            )
        }));
    }

    #[test]
    fn smf_type_zero_writes_single_track_header() {
        let mut opts = ConversionOptions::default();
        opts.smfmap
            .merge_yaml_str(
                r#"
smf:
  type: 0
"#,
            )
            .unwrap();
        let mut song = parse_source("12 c4\n", &opts).unwrap();
        let bytes = crate::render::render_smf(&mut song, &opts).unwrap();
        assert_eq!(&bytes[8..10], &[0x00, 0x00]);
        assert_eq!(&bytes[10..12], &[0x00, 0x01]);
    }

    #[test]
    fn config_skeleton_includes_observed_usage() {
        let opts = ConversionOptions::default();
        let song = parse_source(
            "@e1 = {1,0,f}\n@e2 = {,,@4f}\n1 @1 c4\n4 @2 c4\n4 @e2 d4\n9 @3 y14,32 c4\n",
            &opts,
        )
        .unwrap();
        let skeleton = crate::smfmap::emit_config_skeleton(&song);
        assert!(skeleton.contains("version: 0.4.1"));
        assert!(skeleton.contains("pitch_glide:\n  enabled: false"));
        assert!(skeleton.contains("name: \"PSG envelope @1\""));
        assert!(skeleton.contains("name: \"SCC @2\"\n        events: []"));
        assert!(skeleton.contains("name: \"SCC @4\"\n        events: []"));
        assert!(skeleton.contains("name: \"OPLL @3\"\n        events: []"));
        assert!(skeleton.contains("register: 14"));
        assert!(skeleton.contains("data: 32"));
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
    fn macro_offset_map_resolves_named_references() {
        let events = parse_and_render(
            "#macro_offset { b = 00, q = 110 }\n*04 = { c%48 }\n*120 = { d%48 }\n1 *b04 *q10 e%48\n",
        );
        let starts: Vec<(u64, u8)> = events
            .iter()
            .filter_map(|event| match event.kind {
                RenderedEventKind::NoteOn { note, .. } => Some((event.abs_tick, note)),
                _ => None,
            })
            .collect();
        assert_eq!(starts, vec![(0, 60), (3600, 62), (7200, 64)]);
    }

    #[test]
    fn pitch_glide_uses_target_length_for_timing() {
        let events = parse_and_render("1 l16 o3 b_<b%6 r8^%18 c%48\n");
        let starts: Vec<(u64, u8)> = events
            .iter()
            .filter_map(|event| match event.kind {
                RenderedEventKind::NoteOn { note, .. } => Some((event.abs_tick, note)),
                _ => None,
            })
            .collect();
        assert_eq!(starts, vec![(0, 59), (3600, 36)]);
    }

    #[test]
    fn pitch_glide_keeps_source_note_octave() {
        let events = parse_and_render("1 o4 c_<c%48\n");
        let starts: Vec<(u64, u8)> = events
            .iter()
            .filter_map(|event| match event.kind {
                RenderedEventKind::NoteOn { note, .. } => Some((event.abs_tick, note)),
                _ => None,
            })
            .collect();
        assert_eq!(starts, vec![(0, 60)]);
    }

    #[test]
    fn pitch_glide_opt_in_renders_pitch_bend() {
        let mut opts = ConversionOptions::default();
        opts.smfmap
            .merge_yaml_str(
                r#"
pitch_glide:
  enabled: true
  curve: { event_rate_hz: 4 }
"#,
            )
            .unwrap();
        let mut song = parse_source("1 t120 o4 c_<c%48\n", &opts).unwrap();
        let events = collect_midi_events(&mut song, &opts).unwrap();

        assert!(
            !song
                .diagnostics
                .unsupported_commands
                .iter()
                .any(|item| item.command.starts_with('_'))
        );
        let cc_at_start: Vec<(u8, u8)> = events
            .iter()
            .filter_map(|event| match event.kind {
                RenderedEventKind::ControlChange {
                    channel: 0,
                    controller,
                    value,
                } if event.abs_tick == 0 => Some((controller, value)),
                _ => None,
            })
            .collect();
        assert_eq!(
            cc_at_start,
            vec![(101, 0), (100, 0), (6, 24), (38, 0), (101, 127), (100, 127)]
        );
        let pitch_bends: Vec<(u64, u16)> = events
            .iter()
            .filter_map(|event| match event.kind {
                RenderedEventKind::PitchBend { channel: 0, value } => Some((event.abs_tick, value)),
                _ => None,
            })
            .collect();
        assert!(pitch_bends.contains(&(0, 8192)));
        assert!(
            pitch_bends
                .iter()
                .any(|(tick, value)| *tick < 3600 && *value == 4096)
        );
        assert!(pitch_bends.contains(&(3600, 8192)));
    }

    #[test]
    fn pitch_glide_shared_channel_policy_can_error() {
        let mut opts = ConversionOptions::default();
        opts.smfmap
            .merge_yaml_str(
                r#"
tracks:
  "1": { midi_channel: 0 }
  "2": { midi_channel: 0 }
pitch_glide:
  enabled: true
  shared_channel_policy: error
"#,
            )
            .unwrap();
        let mut song = parse_source("12 o4 c_<c%48\n", &opts).unwrap();
        let err = collect_midi_events(&mut song, &opts).unwrap_err();
        assert!(err.contains("pitch_glide rejected on shared MIDI channel"));
    }

    #[test]
    fn pitch_glide_slur_group_holds_note_on() {
        let mut opts = ConversionOptions::default();
        opts.smfmap
            .merge_yaml_str(
                r#"
pitch_glide:
  enabled: true
  curve: { event_rate_hz: 4 }
"#,
            )
            .unwrap();
        let mut song = parse_source("1 t120 o4 c_d%48&d_e%48\n", &opts).unwrap();
        let events = collect_midi_events(&mut song, &opts).unwrap();
        let note_ons: Vec<(u64, u8)> = events
            .iter()
            .filter_map(|event| match event.kind {
                RenderedEventKind::NoteOn { note, .. } => Some((event.abs_tick, note)),
                _ => None,
            })
            .collect();
        let note_offs: Vec<(u64, u8)> = events
            .iter()
            .filter_map(|event| match event.kind {
                RenderedEventKind::NoteOff { note, .. } => Some((event.abs_tick, note)),
                _ => None,
            })
            .collect();
        assert_eq!(note_ons, vec![(0, 60)]);
        assert_eq!(note_offs, vec![(7200, 60)]);
        assert!(events.iter().any(|event| {
            matches!(
                event.kind,
                RenderedEventKind::PitchBend {
                    channel: 0,
                    value: 9557
                }
            )
        }));
    }

    #[test]
    fn tie_extension_leaves_following_octave_note_target() {
        let events = parse_and_render("1 l6 o4 c^<b12 g\n");
        let starts: Vec<(u64, u8)> = events
            .iter()
            .filter_map(|event| match event.kind {
                RenderedEventKind::NoteOn { note, .. } => Some((event.abs_tick, note)),
                _ => None,
            })
            .collect();
        assert_eq!(starts, vec![(0, 60), (4800, 59), (6000, 55)]);
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
    fn consecutive_slurs_are_consumed_as_one_continuation() {
        let events = parse_and_render("1 q4 c&&c d\n");
        let note_ons: Vec<_> = events
            .iter()
            .filter(|event| matches!(event.kind, RenderedEventKind::NoteOn { .. }))
            .collect();
        assert_eq!(note_ons.len(), 2);
        let c_off = events
            .iter()
            .find(|event| matches!(event.kind, RenderedEventKind::NoteOff { note: 60, .. }))
            .unwrap();
        let d_on = events
            .iter()
            .find(|event| matches!(event.kind, RenderedEventKind::NoteOn { note: 62, .. }))
            .unwrap();
        assert_eq!(c_off.abs_tick, 7200);
        assert_eq!(d_on.abs_tick, 7200);
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
