use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use serde_yaml::Value;

use crate::ir::{
    ChannelOverflowPolicy, ECommand, EnvelopeKind, EnvelopeMode, EnvelopeRef, IrEvent,
    RegisterContext, SmfRequest, SongIr, SourceFamily, TrackKind,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Numbering {
    ZeroBased,
    OneBased,
}

impl Numbering {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "zero" | "zero_based" => Some(Self::ZeroBased),
            "one" | "one_based" => Some(Self::OneBased),
            _ => None,
        }
    }

    pub fn normalize(self, value: u8) -> u8 {
        match self {
            Self::ZeroBased => value,
            Self::OneBased => value.saturating_sub(1),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegisterMapPolicy {
    IgnoreAndReport,
    IgnoreSilently,
    TextMeta,
    ApplyRules,
    Error,
}

impl RegisterMapPolicy {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "ignore_and_report" | "report" => Some(Self::IgnoreAndReport),
            "ignore_silently" | "off" => Some(Self::IgnoreSilently),
            "text_meta" => Some(Self::TextMeta),
            "apply_rules" | "on" => Some(Self::ApplyRules),
            "error" => Some(Self::Error),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PsgNoiseDefaultAction {
    EnvelopeCc,
    DrumMap,
    Ignore,
}

impl PsgNoiseDefaultAction {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "envelope_cc" => Some(Self::EnvelopeCc),
            "drum_map" => Some(Self::DrumMap),
            "ignore" => Some(Self::Ignore),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SmfMapConfig {
    pub smf_ppq: Option<u16>,
    pub smf_type: u16,
    pub program_numbering: Numbering,
    pub channel_numbering: Numbering,
    pub channel_overflow: Option<ChannelOverflowPolicy>,
    pub rhythm_channel: u8,
    pub manual_smf_enabled: bool,
    pub tone_map_enabled: bool,
    pub psg_envelope_map_enabled: bool,
    pub psg_expression_cc: u8,
    pub psg_noise_enabled: bool,
    pub psg_noise_default_action: PsgNoiseDefaultAction,
    pub psg_noise_drum_map_enabled: bool,
    pub psg_noise_drum_channel: u8,
    pub psg_noise_drums: BTreeMap<u8, PsgNoiseDrumEntry>,
    pub envelope_map_enabled: bool,
    pub envelope_mode: Option<EnvelopeMode>,
    pub envelope_expression_cc: u8,
    pub envelope_volume_cc: u8,
    pub envelope_scc_enabled: bool,
    pub envelope_opll_enabled: bool,
    pub register_map_policy: RegisterMapPolicy,
    pub register_rules: Vec<RegisterRule>,
    pub macro_max_depth: usize,
    pub tracks: BTreeMap<String, TrackMapConfig>,
    pub tone_map: BTreeMap<SourceFamily, BTreeMap<u8, ToneMapEntry>>,
    pub macros: BTreeMap<String, Vec<SmfRequest>>,
    pub opll_respect_at_hash_rom_assign: bool,
}

impl Default for SmfMapConfig {
    fn default() -> Self {
        let mut tone_map: BTreeMap<SourceFamily, BTreeMap<u8, ToneMapEntry>> = BTreeMap::new();
        tone_map.insert(SourceFamily::Scc, BTreeMap::new());
        tone_map.insert(SourceFamily::Opll, default_opll_tones());

        Self {
            smf_ppq: None,
            smf_type: 1,
            program_numbering: Numbering::ZeroBased,
            channel_numbering: Numbering::ZeroBased,
            channel_overflow: None,
            rhythm_channel: 9,
            manual_smf_enabled: true,
            tone_map_enabled: true,
            psg_envelope_map_enabled: true,
            psg_expression_cc: 11,
            psg_noise_enabled: true,
            psg_noise_default_action: PsgNoiseDefaultAction::EnvelopeCc,
            psg_noise_drum_map_enabled: false,
            psg_noise_drum_channel: 9,
            psg_noise_drums: BTreeMap::new(),
            envelope_map_enabled: true,
            envelope_mode: None,
            envelope_expression_cc: 11,
            envelope_volume_cc: 7,
            envelope_scc_enabled: true,
            envelope_opll_enabled: true,
            register_map_policy: RegisterMapPolicy::IgnoreAndReport,
            register_rules: Vec::new(),
            macro_max_depth: 8,
            tracks: BTreeMap::new(),
            tone_map,
            macros: default_macros(),
            opll_respect_at_hash_rom_assign: true,
        }
    }
}

impl SmfMapConfig {
    pub fn merge_file(&mut self, path: &Path) -> Result<(), String> {
        let text = fs::read_to_string(path)
            .map_err(|e| format!("failed to read smfmap config {}: {e}", path.display()))?;
        self.merge_yaml_str(&text)
            .map_err(|e| format!("failed to parse smfmap config {}: {e}", path.display()))
    }

    pub fn merge_yaml_str(&mut self, text: &str) -> Result<(), String> {
        let root: Value = serde_yaml::from_str(text).map_err(|e| e.to_string())?;
        self.merge_yaml(&root);
        Ok(())
    }

    pub fn family_for_track(
        &self,
        track_id: &str,
        opll_mode: i32,
        kind: TrackKind,
    ) -> SourceFamily {
        if let Some(family) = self.tracks.get(track_id).and_then(|track| track.family) {
            return family;
        }
        default_family_for_track(track_id, opll_mode, kind)
    }

    pub fn midi_channel_for_track(&self, track_id: &str) -> Option<u8> {
        self.tracks
            .get(track_id)
            .and_then(|track| track.midi_channel)
            .map(|channel| self.channel_numbering.normalize(channel).min(15))
    }

    pub fn midi_port_for_track(&self, track_id: &str) -> Option<u8> {
        self.tracks.get(track_id).and_then(|track| track.midi_port)
    }

    pub fn tone_events(&self, family: SourceFamily, tone: u8) -> Option<&[SmfRequest]> {
        if !self.tone_map_enabled {
            return None;
        }
        match family {
            SourceFamily::Scc | SourceFamily::Opll => self
                .tone_map
                .get(&family)
                .and_then(|tones| tones.get(&tone))
                .map(|entry| entry.events.as_slice()),
            SourceFamily::Psg | SourceFamily::PsgNoise | SourceFamily::Rhythm => None,
        }
    }

    pub fn envelope_mode_for_family(
        &self,
        family: SourceFamily,
        cli_mode: EnvelopeMode,
    ) -> EnvelopeMode {
        match family {
            SourceFamily::Psg | SourceFamily::PsgNoise => {
                if self.psg_envelope_map_enabled {
                    cli_mode
                } else {
                    EnvelopeMode::Off
                }
            }
            SourceFamily::Scc => {
                if self.envelope_map_enabled && self.envelope_scc_enabled {
                    self.envelope_mode.unwrap_or(cli_mode)
                } else {
                    EnvelopeMode::Off
                }
            }
            SourceFamily::Opll => {
                if self.envelope_map_enabled && self.envelope_opll_enabled {
                    self.envelope_mode.unwrap_or(cli_mode)
                } else {
                    EnvelopeMode::Off
                }
            }
            SourceFamily::Rhythm => EnvelopeMode::Off,
        }
    }

    pub fn envelope_controller_for_family(&self, family: SourceFamily, mode: EnvelopeMode) -> u8 {
        match mode {
            EnvelopeMode::Cc7 => self.envelope_volume_cc,
            EnvelopeMode::Cc11 => match family {
                SourceFamily::Psg | SourceFamily::PsgNoise => self.psg_expression_cc,
                _ => self.envelope_expression_cc,
            },
            EnvelopeMode::NoteVelocity | EnvelopeMode::SplitVelocity | EnvelopeMode::Off => 11,
        }
    }

    pub fn psg_noise_drum_for_envelope(
        &self,
        reference: Option<EnvelopeRef>,
    ) -> Option<&PsgNoiseDrumEntry> {
        if !self.psg_noise_enabled
            || !self.psg_noise_drum_map_enabled
            || self.psg_noise_default_action != PsgNoiseDefaultAction::DrumMap
        {
            return None;
        }
        let id = match reference? {
            EnvelopeRef::E(id) | EnvelopeRef::R(id) => id,
        };
        self.psg_noise_drums.get(&id)
    }

    pub fn register_rule_events(
        &self,
        family: SourceFamily,
        source_track: &str,
        register: u8,
        data: u8,
        context: RegisterContext,
    ) -> Vec<(String, Vec<SmfRequest>)> {
        self.register_rules
            .iter()
            .filter(|rule| {
                rule.enabled
                    && rule
                        .matcher
                        .matches(family, source_track, register, data, context)
            })
            .map(|rule| (rule.name.clone(), rule.emit.clone()))
            .collect()
    }

    fn merge_yaml(&mut self, root: &Value) {
        self.merge_smf(root.get_key("smf"));
        self.merge_manual_smf(root.get_key("manual_smf"));
        self.merge_tracks(root.get_key("tracks"));
        self.merge_psg_envelope_map(root.get_key("psg_envelope_map"));
        self.merge_psg_noise_map(root.get_key("psg_noise_map"));
        self.merge_envelope_map(root.get_key("envelope_map"));
        self.merge_tone_map(root.get_key("tone_map"));
        self.merge_register_map(root.get_key("opll_register_map"));
        self.merge_macros(root.get_key("smf_macros"));
    }

    fn merge_smf(&mut self, value: Option<&Value>) {
        let Some(value) = value else {
            return;
        };
        if let Some(ppq) = value.get_key("ppq").and_then(yaml_u16) {
            self.smf_ppq = Some(ppq);
        }
        if let Some(smf_type) = value.get_key("type").and_then(yaml_u16) {
            self.smf_type = smf_type.min(1);
        }
        if let Some(numbering) = value
            .get_key("program_numbering")
            .and_then(yaml_string)
            .and_then(Numbering::parse)
        {
            self.program_numbering = numbering;
        }
        if let Some(numbering) = value
            .get_key("channel_numbering")
            .and_then(yaml_string)
            .and_then(Numbering::parse)
        {
            self.channel_numbering = numbering;
        }
        if let Some(channel_allocation) = value.get_key("channel_allocation") {
            if let Some(policy) = channel_allocation
                .get_key("overflow_policy")
                .or_else(|| channel_allocation.get_key("policy"))
                .and_then(yaml_string)
                .and_then(|policy| ChannelOverflowPolicy::parse(policy).ok())
            {
                self.channel_overflow = Some(policy);
            }
            if let Some(channel) = channel_allocation
                .get_key("rhythm_channel_zero_based")
                .and_then(yaml_u8)
            {
                self.rhythm_channel = channel.min(15);
            }
        }
    }

    fn merge_manual_smf(&mut self, value: Option<&Value>) {
        if let Some(enabled) = value
            .and_then(|value| value.get_key("enabled"))
            .and_then(yaml_bool)
        {
            self.manual_smf_enabled = enabled;
        }
    }

    fn merge_tracks(&mut self, value: Option<&Value>) {
        let Some(Value::Mapping(tracks)) = value else {
            return;
        };
        for (track_id, item) in tracks {
            let Some(track_id) = yaml_key_string(track_id) else {
                continue;
            };
            let Value::Mapping(fields) = item else {
                continue;
            };
            let entry = self.tracks.entry(track_id).or_default();
            if let Some(family) = fields
                .get_str("family")
                .and_then(yaml_string)
                .and_then(SourceFamily::parse)
            {
                entry.family = Some(family);
            }
            if let Some(channel) = fields.get_str("midi_channel").and_then(yaml_u8) {
                entry.midi_channel = Some(channel);
            }
            if let Some(port) = fields.get_str("midi_port").and_then(yaml_u8) {
                entry.midi_port = Some(port);
            }
        }
    }

    fn merge_psg_envelope_map(&mut self, value: Option<&Value>) {
        let Some(value) = value else {
            return;
        };
        if let Some(enabled) = value.get_key("enabled").and_then(yaml_bool) {
            self.psg_envelope_map_enabled = enabled;
        }
        if let Some(cc) = value
            .get_path(&["default", "expression_cc"])
            .and_then(yaml_u8)
        {
            self.psg_expression_cc = cc.min(127);
        }
    }

    fn merge_psg_noise_map(&mut self, value: Option<&Value>) {
        let Some(value) = value else {
            return;
        };
        if let Some(enabled) = value.get_key("enabled").and_then(yaml_bool) {
            self.psg_noise_enabled = enabled;
        }
        if let Some(action) = value
            .get_key("default_action")
            .and_then(yaml_string)
            .and_then(PsgNoiseDefaultAction::parse)
        {
            self.psg_noise_default_action = action;
        }
        if let Some(enabled) = value.get_path(&["drum_map", "enabled"]).and_then(yaml_bool) {
            self.psg_noise_drum_map_enabled = enabled;
        }
        if let Some(channel) = value.get_path(&["drum_map", "channel"]).and_then(yaml_u8) {
            self.psg_noise_drum_channel = self.channel_numbering.normalize(channel).min(15);
        }
        let Some(Value::Mapping(envelopes)) = value.get_path(&["drum_map", "envelopes"]) else {
            return;
        };
        for (id, item) in envelopes {
            let Some(id) = yaml_key_string(id).and_then(|id| id.parse::<u8>().ok()) else {
                continue;
            };
            let Value::Mapping(fields) = item else {
                continue;
            };
            let note = fields
                .get_str("note")
                .and_then(yaml_u8)
                .unwrap_or(35)
                .min(127);
            let velocity = fields
                .get_str("velocity")
                .and_then(yaml_u8)
                .map(|v| v.clamp(1, 127));
            let velocity_from_envelope_peak = fields
                .get_str("velocity_from_envelope_peak")
                .and_then(yaml_bool)
                .unwrap_or(true);
            let name = fields
                .get_str("name")
                .and_then(yaml_string)
                .map(str::to_string);
            self.psg_noise_drums.insert(
                id,
                PsgNoiseDrumEntry {
                    name,
                    note,
                    velocity,
                    velocity_from_envelope_peak,
                },
            );
        }
    }

    fn merge_envelope_map(&mut self, value: Option<&Value>) {
        let Some(value) = value else {
            return;
        };
        if let Some(enabled) = value.get_key("enabled").and_then(yaml_bool) {
            self.envelope_map_enabled = enabled;
        }
        if let Some(mode) = value
            .get_key("default_mode")
            .and_then(yaml_string)
            .and_then(|mode| EnvelopeMode::parse(mode).ok())
        {
            self.envelope_mode = Some(mode);
        }
        if let Some(cc) = value.get_path(&["midi_cc", "expression"]).and_then(yaml_u8) {
            self.envelope_expression_cc = cc.min(127);
        }
        if let Some(cc) = value.get_path(&["midi_cc", "volume"]).and_then(yaml_u8) {
            self.envelope_volume_cc = cc.min(127);
        }
        if let Some(enabled) = value
            .get_path(&["families", "scc", "enabled"])
            .and_then(yaml_bool)
        {
            self.envelope_scc_enabled = enabled;
        }
        if let Some(enabled) = value
            .get_path(&["families", "opll", "enabled"])
            .and_then(yaml_bool)
        {
            self.envelope_opll_enabled = enabled;
        }
    }

    fn merge_tone_map(&mut self, value: Option<&Value>) {
        let Some(Value::Mapping(root)) = value else {
            return;
        };
        for (family_key, family) in [("scc", SourceFamily::Scc), ("opll", SourceFamily::Opll)] {
            let Some(Value::Mapping(family_map)) = root.get_str(family_key) else {
                continue;
            };
            if let Some(enabled) = family_map.get_str("enabled").and_then(yaml_bool) {
                if !enabled {
                    self.tone_map.remove(&family);
                    continue;
                }
            }
            if family == SourceFamily::Opll {
                if let Some(respect) = family_map
                    .get_str("respect_at_hash_rom_assign")
                    .and_then(yaml_bool)
                {
                    self.opll_respect_at_hash_rom_assign = respect;
                }
            }
            let Some(Value::Mapping(tones)) = family_map.get_str("tones") else {
                continue;
            };
            for (tone_id, tone_value) in tones {
                let Some(tone_number) = yaml_key_string(tone_id).and_then(|id| id.parse().ok())
                else {
                    continue;
                };
                let mut entry = self
                    .tone_map
                    .get(&family)
                    .and_then(|family_tones| family_tones.get(&tone_number))
                    .cloned()
                    .unwrap_or_else(|| ToneMapEntry {
                        name: None,
                        events: Vec::new(),
                    });
                if let Value::Mapping(fields) = tone_value {
                    if let Some(name) = fields.get_str("name").and_then(yaml_string) {
                        entry.name = Some(name.to_string());
                    }
                    if let Some(events) = fields
                        .get_str("events")
                        .and_then(|value| parse_events(value, self))
                    {
                        entry.events = events;
                    } else {
                        let direct = parse_direct_events(fields, self);
                        if !direct.is_empty() {
                            entry.events = direct;
                        }
                    }
                }
                self.tone_map
                    .entry(family)
                    .or_default()
                    .insert(tone_number, entry);
            }
        }
    }

    fn merge_register_map(&mut self, value: Option<&Value>) {
        let Some(value) = value else {
            return;
        };
        if matches!(value.get_key("enabled").and_then(yaml_bool), Some(false)) {
            self.register_map_policy = RegisterMapPolicy::IgnoreSilently;
            return;
        }
        if let Some(policy) = value
            .get_key("default_policy")
            .and_then(yaml_string)
            .and_then(RegisterMapPolicy::parse)
        {
            self.register_map_policy = policy;
        }
        let Some(Value::Sequence(rules)) = value.get_key("rules") else {
            return;
        };
        self.register_rules.clear();
        for item in rules {
            let Value::Mapping(fields) = item else {
                continue;
            };
            let name = fields
                .get_str("name")
                .and_then(yaml_string)
                .unwrap_or("unnamed_register_rule")
                .to_string();
            let enabled = fields
                .get_str("enabled")
                .and_then(yaml_bool)
                .unwrap_or(false);
            let matcher = fields
                .get_str("match")
                .and_then(RegisterRuleMatcher::parse)
                .unwrap_or_default();
            let emit = fields
                .get_str("emit")
                .and_then(|value| parse_events(value, self))
                .unwrap_or_default();
            self.register_rules.push(RegisterRule {
                name,
                enabled,
                matcher,
                emit,
            });
        }
    }

    fn merge_macros(&mut self, value: Option<&Value>) {
        let Some(Value::Mapping(macros)) = value else {
            return;
        };
        for (name, item) in macros {
            let Some(name) = yaml_key_string(name) else {
                continue;
            };
            if name == "max_depth" {
                if let Some(max_depth) = yaml_usize(item) {
                    self.macro_max_depth = max_depth.max(1);
                }
                continue;
            }
            let events = match item {
                Value::Mapping(fields) => fields
                    .get_str("events")
                    .and_then(|value| parse_events(value, self)),
                Value::Sequence(_) => parse_events(item, self),
                _ => None,
            };
            if let Some(events) = events {
                self.macros.insert(name, events);
            }
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct TrackMapConfig {
    pub family: Option<SourceFamily>,
    pub midi_channel: Option<u8>,
    pub midi_port: Option<u8>,
}

#[derive(Clone, Debug)]
pub struct ToneMapEntry {
    pub name: Option<String>,
    pub events: Vec<SmfRequest>,
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct PsgNoiseDrumEntry {
    pub name: Option<String>,
    pub note: u8,
    pub velocity: Option<u8>,
    pub velocity_from_envelope_peak: bool,
}

#[derive(Clone, Debug)]
pub struct RegisterRule {
    pub name: String,
    pub enabled: bool,
    pub matcher: RegisterRuleMatcher,
    pub emit: Vec<SmfRequest>,
}

#[derive(Clone, Debug)]
pub struct RegisterRuleMatcher {
    family: StringMatcher,
    source_track: StringMatcher,
    register: U8Matcher,
    data: U8Matcher,
    context: StringMatcher,
}

impl Default for RegisterRuleMatcher {
    fn default() -> Self {
        Self {
            family: StringMatcher::Any,
            source_track: StringMatcher::Any,
            register: U8Matcher::Any,
            data: U8Matcher::Any,
            context: StringMatcher::Any,
        }
    }
}

impl RegisterRuleMatcher {
    fn parse(value: &Value) -> Option<Self> {
        let Value::Mapping(fields) = value else {
            return None;
        };
        Some(Self {
            family: fields
                .get_str("family")
                .map(StringMatcher::parse)
                .unwrap_or(StringMatcher::Any),
            source_track: fields
                .get_str("source_track")
                .map(StringMatcher::parse)
                .unwrap_or(StringMatcher::Any),
            register: fields
                .get_str("register")
                .map(U8Matcher::parse)
                .unwrap_or(U8Matcher::Any),
            data: fields
                .get_str("data")
                .map(U8Matcher::parse)
                .unwrap_or(U8Matcher::Any),
            context: fields
                .get_str("context")
                .map(StringMatcher::parse)
                .unwrap_or(StringMatcher::Any),
        })
    }

    fn matches(
        &self,
        family: SourceFamily,
        source_track: &str,
        register: u8,
        data: u8,
        context: RegisterContext,
    ) -> bool {
        self.family.matches(family.as_str())
            && self.source_track.matches(source_track)
            && self.register.matches(register)
            && self.data.matches(data)
            && self.context.matches(context.as_str())
    }
}

#[derive(Clone, Debug)]
enum StringMatcher {
    Any,
    One(String),
    Many(BTreeSet<String>),
}

impl StringMatcher {
    fn parse(value: &Value) -> Self {
        match value {
            Value::String(value) if value == "*" => Self::Any,
            Value::String(value) => Self::One(value.clone()),
            Value::Sequence(items) => {
                let values: BTreeSet<String> = items
                    .iter()
                    .filter_map(yaml_string)
                    .map(str::to_string)
                    .collect();
                if values.is_empty() {
                    Self::Any
                } else {
                    Self::Many(values)
                }
            }
            _ => Self::Any,
        }
    }

    fn matches(&self, value: &str) -> bool {
        match self {
            Self::Any => true,
            Self::One(expected) => expected == value,
            Self::Many(values) => values.contains(value),
        }
    }
}

#[derive(Clone, Debug)]
enum U8Matcher {
    Any,
    One(u8),
    Many(BTreeSet<u8>),
    Range { min: u8, max: u8 },
}

impl U8Matcher {
    fn parse(value: &Value) -> Self {
        match value {
            Value::String(value) if value == "*" => Self::Any,
            Value::String(value) => parse_range(value)
                .unwrap_or_else(|| parse_u8_scalar(value).map(Self::One).unwrap_or(Self::Any)),
            Value::Number(_) => yaml_u8(value).map(Self::One).unwrap_or(Self::Any),
            Value::Sequence(items) => {
                let values: BTreeSet<u8> = items.iter().filter_map(yaml_u8).collect();
                if values.is_empty() {
                    Self::Any
                } else {
                    Self::Many(values)
                }
            }
            _ => Self::Any,
        }
    }

    fn matches(&self, value: u8) -> bool {
        match self {
            Self::Any => true,
            Self::One(expected) => *expected == value,
            Self::Many(values) => values.contains(&value),
            Self::Range { min, max } => (*min..=*max).contains(&value),
        }
    }
}

pub fn emit_config_skeleton(song: &SongIr) -> String {
    let mut psg_envelopes = BTreeSet::new();
    let mut scc_tones = BTreeSet::new();
    let mut opll_tones = BTreeSet::new();
    let mut scc_opll_envelopes = BTreeSet::new();
    let mut register_writes = BTreeSet::new();

    for track in &song.tracks {
        for event in &track.events {
            match event {
                IrEvent::AtCommand {
                    family,
                    number,
                    spelling,
                    ..
                } => match family {
                    SourceFamily::Psg | SourceFamily::PsgNoise => {
                        psg_envelopes.insert(*number);
                    }
                    SourceFamily::Scc if *spelling == crate::ir::AtSpelling::At => {
                        scc_tones.insert(*number);
                    }
                    SourceFamily::Opll if *spelling == crate::ir::AtSpelling::At => {
                        opll_tones.insert(*number);
                    }
                    SourceFamily::Scc | SourceFamily::Opll => {
                        scc_opll_envelopes.insert((*family, *number, *spelling));
                    }
                    SourceFamily::Rhythm => {}
                },
                IrEvent::RegisterWrite {
                    family,
                    source_track,
                    register,
                    data,
                    context,
                    ..
                } => {
                    register_writes.insert((
                        *family,
                        source_track.clone(),
                        *register,
                        *data,
                        *context,
                    ));
                }
                _ => {}
            }
        }
    }
    for env in song.envelopes.e.values() {
        for command in &env.commands {
            if let ECommand::RegisterWrite { register, data } = command {
                register_writes.insert((
                    SourceFamily::Opll,
                    "*".to_string(),
                    (*register).clamp(0, 255) as u8,
                    (*data).clamp(0, 255) as u8,
                    RegisterContext::EnvelopeData,
                ));
            }
        }
    }

    let mut out = String::new();
    out.push_str("version: 0.3\n");
    out.push_str("profile_name: observed-skeleton\n\n");
    out.push_str("psg_envelope_map:\n  enabled: true\n  envelopes:\n");
    if psg_envelopes.is_empty() {
        out.push_str("    {}\n");
    } else {
        for id in psg_envelopes {
            out.push_str(&format!("    \"{id}\":\n"));
            out.push_str(&format!("      name: \"PSG envelope @{id}\"\n"));
        }
    }

    out.push_str("\ntone_map:\n  scc:\n    enabled: true\n    tones:\n");
    if scc_tones.is_empty() {
        out.push_str("      {}\n");
    } else {
        for id in scc_tones {
            out.push_str(&format!("      \"{id}\":\n"));
            out.push_str(&format!("        name: \"SCC @{id}\"\n"));
            out.push_str("        events: []\n");
        }
    }
    out.push_str("  opll:\n    enabled: true\n    tones:\n");
    if opll_tones.is_empty() {
        out.push_str("      {}\n");
    } else {
        for id in opll_tones {
            out.push_str(&format!("      \"{id}\":\n"));
            out.push_str(&format!("        name: \"OPLL @{id}\"\n"));
            out.push_str("        events: []\n");
        }
    }

    out.push_str("\nenvelope_map:\n  enabled: true\n  families:\n");
    out.push_str("    scc: { enabled: true }\n");
    out.push_str("    opll: { enabled: true }\n");
    if !scc_opll_envelopes.is_empty() {
        out.push_str("  observed:\n");
        for (family, id, spelling) in scc_opll_envelopes {
            let kind = match spelling {
                crate::ir::AtSpelling::At => EnvelopeKind::At,
                crate::ir::AtSpelling::AtE => EnvelopeKind::E,
                crate::ir::AtSpelling::AtR => EnvelopeKind::R,
            };
            out.push_str(&format!(
                "    - {{ family: {}, spelling: \"{}\", envelope: {}, kind: {:?} }}\n",
                family.as_str(),
                spelling.as_str(),
                id,
                kind
            ));
        }
    }

    out.push_str("\nopll_register_map:\n  enabled: true\n  default_policy: ignore_and_report\n");
    if register_writes.is_empty() {
        out.push_str("  rules: []\n");
    } else {
        out.push_str("  rules:\n");
        for (family, track, register, data, context) in register_writes {
            out.push_str("    - name: observed_y");
            out.push_str(&register.to_string());
            out.push('_');
            out.push_str(&data.to_string());
            out.push('\n');
            out.push_str("      enabled: false\n");
            out.push_str(&format!(
                "      match: {{ family: {}, source_track: \"{}\", register: {}, data: {}, context: {} }}\n",
                family.as_str(),
                track,
                register,
                data,
                context.as_str()
            ));
            out.push_str("      emit: []\n");
        }
    }
    out
}

fn default_family_for_track(track_id: &str, opll_mode: i32, kind: TrackKind) -> SourceFamily {
    if kind == TrackKind::Rhythm || (opll_mode == 1 && matches!(track_id, "f" | "r")) {
        return SourceFamily::Rhythm;
    }
    match track_id {
        "1" | "2" => SourceFamily::Psg,
        "3" => SourceFamily::PsgNoise,
        "4" | "5" | "6" | "7" | "8" => SourceFamily::Scc,
        "9" | "a" | "b" | "c" | "d" | "e" | "f" | "g" | "h" => SourceFamily::Opll,
        _ => SourceFamily::Psg,
    }
}

fn default_opll_tones() -> BTreeMap<u8, ToneMapEntry> {
    let programs = [
        (0, "violin", 40),
        (1, "guitar", 24),
        (2, "piano", 0),
        (3, "flute", 73),
        (4, "clarinet", 71),
        (5, "oboe", 68),
        (6, "trumpet", 56),
        (7, "organ", 16),
        (8, "horn", 60),
        (9, "synthesizer", 80),
        (10, "harpsichord", 6),
        (11, "vibraphone", 11),
        (12, "synth_bass", 38),
        (13, "wood_bass", 32),
        (14, "electric_bass", 33),
    ];
    let mut out = BTreeMap::new();
    for (id, name, program) in programs {
        out.insert(
            id,
            ToneMapEntry {
                name: Some(name.to_string()),
                events: vec![SmfRequest::Pc { program }],
            },
        );
    }
    out.insert(
        15,
        ToneMapEntry {
            name: Some("custom_user".to_string()),
            events: Vec::new(),
        },
    );
    out.insert(
        16,
        ToneMapEntry {
            name: Some("custom_user".to_string()),
            events: Vec::new(),
        },
    );
    out
}

fn default_macros() -> BTreeMap<String, Vec<SmfRequest>> {
    let mut out = BTreeMap::new();
    out.insert(
        "reset_channel".to_string(),
        vec![
            SmfRequest::Cc {
                controller: 121,
                value: 0,
            },
            SmfRequest::Cc {
                controller: 7,
                value: 100,
            },
            SmfRequest::Cc {
                controller: 10,
                value: 64,
            },
            SmfRequest::Cc {
                controller: 11,
                value: 127,
            },
        ],
    );
    out
}

fn parse_events(value: &Value, config: &SmfMapConfig) -> Option<Vec<SmfRequest>> {
    let Value::Sequence(items) = value else {
        return None;
    };
    let mut out = Vec::new();
    for item in items {
        if let Value::Mapping(map) = item {
            out.extend(parse_direct_events(map, config));
        }
    }
    Some(out)
}

fn parse_direct_events(map: &serde_yaml::Mapping, config: &SmfMapConfig) -> Vec<SmfRequest> {
    let mut out = Vec::new();
    for (key, value) in map {
        let Some(key) = yaml_key_string(key) else {
            continue;
        };
        if let Some(event) = parse_event(&key, value, config) {
            out.push(event);
        }
    }
    out
}

fn parse_event(key: &str, value: &Value, config: &SmfMapConfig) -> Option<SmfRequest> {
    match key {
        "pc" => Some(SmfRequest::Pc {
            program: config
                .program_numbering
                .normalize(event_field_u8(value, "program").or_else(|| yaml_u8(value))?),
        }),
        "bank" => Some(SmfRequest::Bank {
            msb: event_field_u8(value, "msb")?,
            lsb: event_field_u8(value, "lsb")?,
        }),
        "cc" => Some(SmfRequest::Cc {
            controller: event_field_u8(value, "controller")?,
            value: event_field_u8(value, "value")?,
        }),
        "pb" => Some(SmfRequest::PitchBend {
            value: event_field_i32(value, "value").or_else(|| yaml_i32(value))?,
        }),
        "rpn" => Some(SmfRequest::Rpn {
            msb: event_field_u8(value, "msb")?,
            lsb: event_field_u8(value, "lsb")?,
            value: event_field_u16(value, "value")?,
        }),
        "nrpn" => Some(SmfRequest::Nrpn {
            msb: event_field_u8(value, "msb")?,
            lsb: event_field_u8(value, "lsb")?,
            value: event_field_u16(value, "value")?,
        }),
        "marker" => Some(SmfRequest::Marker {
            text: event_field_string(value, "value")
                .or_else(|| yaml_string(value))
                .unwrap_or_default()
                .to_string(),
        }),
        "text" => Some(SmfRequest::Text {
            text: event_field_string(value, "value")
                .or_else(|| yaml_string(value))
                .unwrap_or_default()
                .to_string(),
        }),
        "macro" => Some(SmfRequest::Macro {
            name: event_field_string(value, "name")
                .or_else(|| yaml_string(value))
                .unwrap_or_default()
                .to_string(),
        }),
        _ => None,
    }
}

fn event_field_u8(value: &Value, key: &str) -> Option<u8> {
    value.get_key(key).and_then(yaml_u8)
}

fn event_field_u16(value: &Value, key: &str) -> Option<u16> {
    value.get_key(key).and_then(yaml_u16)
}

fn event_field_i32(value: &Value, key: &str) -> Option<i32> {
    value.get_key(key).and_then(yaml_i32)
}

fn event_field_string<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get_key(key).and_then(yaml_string)
}

fn parse_range(value: &str) -> Option<U8Matcher> {
    let (lhs, rhs) = value.split_once("..").or_else(|| value.split_once('-'))?;
    let min = parse_u8_scalar(lhs.trim())?;
    let max = parse_u8_scalar(rhs.trim())?;
    Some(U8Matcher::Range {
        min: min.min(max),
        max: min.max(max),
    })
}

fn parse_u8_scalar(value: &str) -> Option<u8> {
    parse_i32_scalar(value).and_then(|value| u8::try_from(value).ok())
}

fn yaml_key_string(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

trait YamlExt {
    fn get_key(&self, key: &str) -> Option<&Value>;
    fn get_path(&self, path: &[&str]) -> Option<&Value>;
}

impl YamlExt for Value {
    fn get_key(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Mapping(map) => map.get_str(key),
            _ => None,
        }
    }

    fn get_path(&self, path: &[&str]) -> Option<&Value> {
        let mut current = self;
        for key in path {
            current = current.get_key(key)?;
        }
        Some(current)
    }
}

trait MappingExt {
    fn get_str(&self, key: &str) -> Option<&Value>;
}

impl MappingExt for serde_yaml::Mapping {
    fn get_str(&self, key: &str) -> Option<&Value> {
        self.get(Value::String(key.to_string()))
    }
}

fn yaml_string(value: &Value) -> Option<&str> {
    match value {
        Value::String(value) => Some(value.as_str()),
        _ => None,
    }
}

fn yaml_bool(value: &Value) -> Option<bool> {
    match value {
        Value::Bool(value) => Some(*value),
        Value::String(value) => match value.as_str() {
            "true" | "on" | "yes" => Some(true),
            "false" | "off" | "no" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

fn yaml_usize(value: &Value) -> Option<usize> {
    yaml_i32(value).and_then(|value| usize::try_from(value).ok())
}

fn yaml_u8(value: &Value) -> Option<u8> {
    yaml_i32(value).and_then(|value| u8::try_from(value).ok())
}

fn yaml_u16(value: &Value) -> Option<u16> {
    yaml_i32(value).and_then(|value| u16::try_from(value).ok())
}

fn yaml_i32(value: &Value) -> Option<i32> {
    match value {
        Value::Number(number) => number.as_i64().and_then(|value| i32::try_from(value).ok()),
        Value::String(value) => parse_i32_scalar(value.trim()),
        _ => None,
    }
}

fn parse_i32_scalar(raw: &str) -> Option<i32> {
    let (sign, raw) = if let Some(rest) = raw.strip_prefix('-') {
        (-1, rest)
    } else if let Some(rest) = raw.strip_prefix('+') {
        (1, rest)
    } else {
        (1, raw)
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
