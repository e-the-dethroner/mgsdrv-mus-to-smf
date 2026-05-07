use crate::ir::{Rational, TempoEvent};

#[derive(Clone, Debug, Default)]
pub struct Diagnostics {
    pub input_file: Option<String>,
    pub output_file: Option<String>,
    pub ppq: u16,
    pub encoding: Option<String>,
    pub track_summaries: Vec<TrackSummary>,
    pub unsupported_commands: Vec<UnsupportedCommand>,
    pub ignored_tone_definitions: Vec<IgnoredToneDefinition>,
    pub parse_warnings: Vec<ParseWarning>,
    pub channel_allocations: Vec<ChannelAllocation>,
    pub loop_markers: Vec<LoopMarkerDiagnostic>,
    pub envelope_event_counts: Vec<EnvelopeEventCount>,
}

impl Diagnostics {
    pub fn add_parse_warning(
        &mut self,
        line: Option<usize>,
        track: Option<String>,
        message: impl Into<String>,
    ) {
        self.parse_warnings.push(ParseWarning {
            line,
            track,
            message: message.into(),
        });
    }

    pub fn add_unsupported(
        &mut self,
        line: Option<usize>,
        track: Option<String>,
        command: impl Into<String>,
        policy: impl Into<String>,
    ) {
        self.unsupported_commands.push(UnsupportedCommand {
            line,
            track,
            command: command.into(),
            policy: policy.into(),
        });
    }

    pub fn add_ignored_tone_definition(
        &mut self,
        line: Option<usize>,
        command: impl Into<String>,
        text: impl Into<String>,
    ) {
        self.ignored_tone_definitions.push(IgnoredToneDefinition {
            line,
            command: command.into(),
            text: text.into(),
        });
    }

    pub fn add_loop_marker(
        &mut self,
        track: String,
        start_step: Rational,
        end_step: Rational,
        kind: impl Into<String>,
    ) {
        self.loop_markers.push(LoopMarkerDiagnostic {
            track,
            start_step,
            end_step,
            kind: kind.into(),
        });
    }

    pub fn has_warnings(&self) -> bool {
        !self.parse_warnings.is_empty() || !self.unsupported_commands.is_empty()
    }

    pub fn to_json(&self, tempo_events: &[TempoEvent]) -> String {
        let mut out = String::new();
        out.push_str("{\n");
        push_field_string_opt(&mut out, "input", self.input_file.as_deref(), true);
        push_field_string_opt(&mut out, "output", self.output_file.as_deref(), true);
        push_field_number(&mut out, "ppq", self.ppq as u64, true);
        push_field_string_opt(&mut out, "encoding", self.encoding.as_deref(), true);

        out.push_str("  \"tracks\": [\n");
        for (i, track) in self.track_summaries.iter().enumerate() {
            if i > 0 {
                out.push_str(",\n");
            }
            out.push_str("    {\n");
            push_object_string(&mut out, "source_track", &track.source_track, true, 6);
            push_object_number(&mut out, "midi_track", track.midi_track as u64, true, 6);
            push_object_number(&mut out, "midi_channel", track.midi_channel as u64, true, 6);
            push_object_number(&mut out, "midi_port", track.midi_port as u64, true, 6);
            push_object_raw(&mut out, "source_steps", &track.source_steps, true, 6);
            push_object_number(&mut out, "note_count", track.note_count as u64, true, 6);
            push_object_number(&mut out, "cc_count", track.cc_count as u64, false, 6);
            out.push_str("\n    }");
        }
        out.push_str("\n  ],\n");

        out.push_str("  \"tempo_map\": [\n");
        for (i, tempo) in tempo_events.iter().enumerate() {
            if i > 0 {
                out.push_str(",\n");
            }
            out.push_str("    {\n");
            push_object_raw(
                &mut out,
                "source_step",
                &tempo.at_steps.to_source_step_json(),
                true,
                6,
            );
            push_object_number(&mut out, "bpm", tempo.bpm as u64, false, 6);
            out.push_str("\n    }");
        }
        out.push_str("\n  ],\n");

        out.push_str("  \"unsupported_commands\": [\n");
        for (i, item) in self.unsupported_commands.iter().enumerate() {
            if i > 0 {
                out.push_str(",\n");
            }
            out.push_str("    {\n");
            push_object_number_opt(&mut out, "line", item.line, true, 6);
            push_object_string_opt(&mut out, "track", item.track.as_deref(), true, 6);
            push_object_string(&mut out, "command", &item.command, true, 6);
            push_object_string(&mut out, "policy", &item.policy, false, 6);
            out.push_str("\n    }");
        }
        out.push_str("\n  ],\n");

        out.push_str("  \"ignored_tone_definitions\": [\n");
        for (i, item) in self.ignored_tone_definitions.iter().enumerate() {
            if i > 0 {
                out.push_str(",\n");
            }
            out.push_str("    {\n");
            push_object_number_opt(&mut out, "line", item.line, true, 6);
            push_object_string(&mut out, "command", &item.command, true, 6);
            push_object_string(&mut out, "text", &item.text, false, 6);
            out.push_str("\n    }");
        }
        out.push_str("\n  ],\n");

        out.push_str("  \"parse_warnings\": [\n");
        for (i, item) in self.parse_warnings.iter().enumerate() {
            if i > 0 {
                out.push_str(",\n");
            }
            out.push_str("    {\n");
            push_object_number_opt(&mut out, "line", item.line, true, 6);
            push_object_string_opt(&mut out, "track", item.track.as_deref(), true, 6);
            push_object_string(&mut out, "message", &item.message, false, 6);
            out.push_str("\n    }");
        }
        out.push_str("\n  ],\n");

        out.push_str("  \"channel_allocation\": [\n");
        for (i, item) in self.channel_allocations.iter().enumerate() {
            if i > 0 {
                out.push_str(",\n");
            }
            out.push_str("    {\n");
            push_object_string(&mut out, "source_track", &item.source_track, true, 6);
            push_object_number(&mut out, "midi_track", item.midi_track as u64, true, 6);
            push_object_number(&mut out, "midi_channel", item.midi_channel as u64, true, 6);
            push_object_number(&mut out, "midi_port", item.midi_port as u64, false, 6);
            out.push_str("\n    }");
        }
        out.push_str("\n  ],\n");

        out.push_str("  \"loop_markers\": [\n");
        for (i, item) in self.loop_markers.iter().enumerate() {
            if i > 0 {
                out.push_str(",\n");
            }
            out.push_str("    {\n");
            push_object_string(&mut out, "track", &item.track, true, 6);
            push_object_raw(
                &mut out,
                "start_step",
                &item.start_step.to_source_step_json(),
                true,
                6,
            );
            push_object_raw(
                &mut out,
                "end_step",
                &item.end_step.to_source_step_json(),
                true,
                6,
            );
            push_object_string(&mut out, "kind", &item.kind, false, 6);
            out.push_str("\n    }");
        }
        out.push_str("\n  ],\n");

        out.push_str("  \"envelope_event_counts\": [\n");
        for (i, item) in self.envelope_event_counts.iter().enumerate() {
            if i > 0 {
                out.push_str(",\n");
            }
            out.push_str("    {\n");
            push_object_string(&mut out, "track", &item.track, true, 6);
            push_object_string(&mut out, "envelope", &item.envelope, true, 6);
            push_object_number(&mut out, "event_count", item.event_count as u64, false, 6);
            out.push_str("\n    }");
        }
        out.push_str("\n  ]\n");

        out.push_str("}\n");
        out
    }
}

#[derive(Clone, Debug)]
pub struct TrackSummary {
    pub source_track: String,
    pub midi_track: usize,
    pub midi_channel: u8,
    pub midi_port: u8,
    pub source_steps: String,
    pub note_count: usize,
    pub cc_count: usize,
}

#[derive(Clone, Debug)]
pub struct UnsupportedCommand {
    pub line: Option<usize>,
    pub track: Option<String>,
    pub command: String,
    pub policy: String,
}

#[derive(Clone, Debug)]
pub struct IgnoredToneDefinition {
    pub line: Option<usize>,
    pub command: String,
    pub text: String,
}

#[derive(Clone, Debug)]
pub struct ParseWarning {
    pub line: Option<usize>,
    pub track: Option<String>,
    pub message: String,
}

#[derive(Clone, Debug)]
pub struct ChannelAllocation {
    pub source_track: String,
    pub midi_track: usize,
    pub midi_channel: u8,
    pub midi_port: u8,
}

#[derive(Clone, Debug)]
pub struct LoopMarkerDiagnostic {
    pub track: String,
    pub start_step: Rational,
    pub end_step: Rational,
    pub kind: String,
}

#[derive(Clone, Debug)]
pub struct EnvelopeEventCount {
    pub track: String,
    pub envelope: String,
    pub event_count: usize,
}

fn push_field_string_opt(out: &mut String, key: &str, value: Option<&str>, comma: bool) {
    out.push_str("  ");
    push_key(out, key);
    match value {
        Some(value) => {
            out.push('"');
            push_escaped(out, value);
            out.push('"');
        }
        None => out.push_str("null"),
    }
    if comma {
        out.push(',');
    }
    out.push('\n');
}

fn push_field_number(out: &mut String, key: &str, value: u64, comma: bool) {
    out.push_str("  ");
    push_key(out, key);
    out.push_str(&value.to_string());
    if comma {
        out.push(',');
    }
    out.push('\n');
}

fn push_object_string(out: &mut String, key: &str, value: &str, comma: bool, indent: usize) {
    push_indent(out, indent);
    push_key(out, key);
    out.push('"');
    push_escaped(out, value);
    out.push('"');
    if comma {
        out.push(',');
    }
    out.push('\n');
}

fn push_object_string_opt(
    out: &mut String,
    key: &str,
    value: Option<&str>,
    comma: bool,
    indent: usize,
) {
    push_indent(out, indent);
    push_key(out, key);
    match value {
        Some(value) => {
            out.push('"');
            push_escaped(out, value);
            out.push('"');
        }
        None => out.push_str("null"),
    }
    if comma {
        out.push(',');
    }
    out.push('\n');
}

fn push_object_number(out: &mut String, key: &str, value: u64, comma: bool, indent: usize) {
    push_indent(out, indent);
    push_key(out, key);
    out.push_str(&value.to_string());
    if comma {
        out.push(',');
    }
    out.push('\n');
}

fn push_object_number_opt(
    out: &mut String,
    key: &str,
    value: Option<usize>,
    comma: bool,
    indent: usize,
) {
    push_indent(out, indent);
    push_key(out, key);
    match value {
        Some(value) => out.push_str(&value.to_string()),
        None => out.push_str("null"),
    }
    if comma {
        out.push(',');
    }
    out.push('\n');
}

fn push_object_raw(out: &mut String, key: &str, value: &str, comma: bool, indent: usize) {
    push_indent(out, indent);
    push_key(out, key);
    out.push_str(value);
    if comma {
        out.push(',');
    }
    out.push('\n');
}

fn push_key(out: &mut String, key: &str) {
    out.push('"');
    out.push_str(key);
    out.push_str("\": ");
}

fn push_indent(out: &mut String, indent: usize) {
    for _ in 0..indent {
        out.push(' ');
    }
}

fn push_escaped(out: &mut String, value: &str) {
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
}
