use std::collections::BTreeMap;

use crate::diagnostics::Diagnostics;

pub const WHOLE_NOTE_STEPS: i64 = 192;
pub const QUARTER_NOTE_STEPS: i64 = 48;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EncodingChoice {
    Auto,
    Utf8,
    ShiftJis,
}

impl EncodingChoice {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "auto" => Ok(Self::Auto),
            "utf-8" | "utf8" => Ok(Self::Utf8),
            "shift_jis" | "shift-jis" | "sjis" => Ok(Self::ShiftJis),
            _ => Err(format!("invalid encoding: {value}")),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EnvelopeMode {
    Cc11,
    Cc7,
    NoteVelocity,
    SplitVelocity,
    Off,
}

impl EnvelopeMode {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "cc11" => Ok(Self::Cc11),
            "cc7" => Ok(Self::Cc7),
            "note_velocity" | "velocity" => Ok(Self::NoteVelocity),
            "split_velocity" => Ok(Self::SplitVelocity),
            "off" => Ok(Self::Off),
            _ => Err(format!("invalid envelope mode: {value}")),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TonePolicy {
    Ignore,
    GmProgram,
    TextMeta,
}

impl TonePolicy {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "ignore" => Ok(Self::Ignore),
            "gm_program" => Ok(Self::GmProgram),
            "text_meta" => Ok(Self::TextMeta),
            _ => Err(format!("invalid tone policy: {value}")),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RhythmMap {
    Gm,
    Off,
}

impl RhythmMap {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "gm" => Ok(Self::Gm),
            "off" => Ok(Self::Off),
            _ => Err(format!("invalid rhythm map: {value}")),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChannelOverflowPolicy {
    Error,
    Shared,
    MultiPort,
}

impl ChannelOverflowPolicy {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "error" => Ok(Self::Error),
            "shared" => Ok(Self::Shared),
            "multi_port" => Ok(Self::MultiPort),
            _ => Err(format!("invalid channel overflow policy: {value}")),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ConversionOptions {
    pub ppq: u16,
    pub loop_count: Option<u32>,
    pub loop_marker_start: String,
    pub loop_marker_end: String,
    pub envelope_mode: EnvelopeMode,
    pub tone_policy: TonePolicy,
    pub rhythm_map: RhythmMap,
    pub channel_overflow: ChannelOverflowPolicy,
    pub encoding: EncodingChoice,
    pub octave_base: i32,
    pub strict: bool,
}

impl Default for ConversionOptions {
    fn default() -> Self {
        Self {
            ppq: 3600,
            loop_count: None,
            loop_marker_start: "loopStart".to_string(),
            loop_marker_end: "loopEnd".to_string(),
            envelope_mode: EnvelopeMode::Cc11,
            tone_policy: TonePolicy::Ignore,
            rhythm_map: RhythmMap::Gm,
            channel_overflow: ChannelOverflowPolicy::Error,
            encoding: EncodingChoice::Auto,
            octave_base: 60,
            strict: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Rational {
    pub num: i64,
    pub den: i64,
}

impl Rational {
    pub const ZERO: Self = Self { num: 0, den: 1 };

    pub fn new(num: i64, den: i64) -> Self {
        assert!(den != 0, "rational denominator must not be zero");
        if num == 0 {
            return Self::ZERO;
        }
        let sign = if den < 0 { -1 } else { 1 };
        let num = num * sign;
        let den = den.abs();
        let gcd = gcd_i64(num.abs(), den);
        Self {
            num: num / gcd,
            den: den / gcd,
        }
    }

    pub fn from_i64(value: i64) -> Self {
        Self { num: value, den: 1 }
    }

    pub fn add(self, rhs: Self) -> Self {
        Self::new(self.num * rhs.den + rhs.num * self.den, self.den * rhs.den)
    }

    pub fn mul_i64(self, rhs: i64) -> Self {
        Self::new(self.num * rhs, self.den)
    }

    pub fn div_i64(self, rhs: i64) -> Self {
        Self::new(self.num, self.den * rhs)
    }

    pub fn to_source_step_json(self) -> String {
        if self.den == 1 {
            self.num.to_string()
        } else {
            format!("\"{}/{}\"", self.num, self.den)
        }
    }

    pub fn to_midi_ticks(self, ppq: u16) -> u64 {
        let num = self.num as i128 * ppq as i128;
        let den = self.den as i128 * QUARTER_NOTE_STEPS as i128;
        div_round_i128(num, den).max(0) as u64
    }
}

impl Default for Rational {
    fn default() -> Self {
        Self::ZERO
    }
}

fn gcd_i64(mut a: i64, mut b: i64) -> i64 {
    while b != 0 {
        let r = a % b;
        a = b;
        b = r;
    }
    a.max(1)
}

fn div_round_i128(num: i128, den: i128) -> i128 {
    if num >= 0 {
        (num + den / 2) / den
    } else {
        -((-num + den / 2) / den)
    }
}

#[derive(Clone, Debug)]
pub struct SongIr {
    pub title: Option<String>,
    pub ppq: u16,
    pub tempo_events: Vec<TempoEvent>,
    pub tracks: Vec<TrackIr>,
    pub envelopes: EnvelopeTable,
    pub control_texts: BTreeMap<u8, String>,
    pub diagnostics: Diagnostics,
}

impl SongIr {
    pub fn new(ppq: u16) -> Self {
        Self {
            title: None,
            ppq,
            tempo_events: Vec::new(),
            tracks: Vec::new(),
            envelopes: EnvelopeTable::default(),
            control_texts: BTreeMap::new(),
            diagnostics: Diagnostics::default(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TempoEvent {
    pub at_steps: Rational,
    pub bpm: u32,
    pub source: String,
    pub line: Option<usize>,
}

#[derive(Clone, Debug)]
pub struct TrackIr {
    pub source_id: String,
    pub kind: TrackKind,
    pub events: Vec<IrEvent>,
    pub length_steps: Rational,
}

impl TrackIr {
    pub fn new_with_kind(source_id: String, kind: TrackKind) -> Self {
        Self {
            source_id,
            kind,
            events: Vec::new(),
            length_steps: Rational::ZERO,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TrackKind {
    Melodic,
    Rhythm,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IrEvent {
    Note {
        start_steps: Rational,
        duration_steps: Rational,
        note: u8,
        velocity: u8,
        tone: Option<u8>,
        envelope: Option<EnvelopeRef>,
    },
    Rest {
        start_steps: Rational,
        duration_steps: Rational,
    },
    Tempo {
        at_steps: Rational,
        bpm: u32,
    },
    Control {
        at_steps: Rational,
        kind: ControlKind,
    },
    LoopMarker {
        at_steps: Rational,
        kind: LoopMarkerKind,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ControlKind {
    Tone(u8),
    Envelope(EnvelopeRef),
    Text(u8),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LoopMarkerKind {
    Start,
    End,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum EnvelopeRef {
    E(u8),
    R(u8),
}

#[derive(Clone, Debug, Default)]
pub struct EnvelopeTable {
    pub e: BTreeMap<u8, EnvelopeE>,
    pub r: BTreeMap<u8, EnvelopeR>,
}

impl EnvelopeTable {
    pub fn get(&self, reference: EnvelopeRef) -> Option<EnvelopeDefinition<'_>> {
        match reference {
            EnvelopeRef::E(id) => self.e.get(&id).map(EnvelopeDefinition::E),
            EnvelopeRef::R(id) => self.r.get(&id).map(EnvelopeDefinition::R),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum EnvelopeDefinition<'a> {
    E(&'a EnvelopeE),
    R(&'a EnvelopeR),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EnvelopeE {
    pub id: u8,
    pub mode: i32,
    pub noise: i32,
    pub commands: Vec<ECommand>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ECommand {
    Level(u8),
    Hold { level: u8, count: u32 },
    Ramp { target: u8, count: u32 },
    ToneChange(u8),
    Noise(u32),
    ModeChange { command: char, value: i32 },
    RegisterWrite { register: i32, data: i32 },
    FrequencyOffset(i32),
    LoopStart,
    LoopEnd,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EnvelopeR {
    pub id: u8,
    pub mode: i32,
    pub noise: i32,
    pub al: u8,
    pub ar: u8,
    pub dr: u8,
    pub sl: u8,
    pub sr: u8,
    pub rr: u8,
}

pub fn denominator_to_steps(denominator: u32) -> Option<Rational> {
    if denominator == 0 {
        None
    } else {
        Some(Rational::new(WHOLE_NOTE_STEPS, denominator as i64))
    }
}

pub fn apply_dots(mut base: Rational, dot_count: u32) -> Rational {
    let mut add = base;
    for _ in 0..dot_count {
        add = add.div_i64(2);
        base = base.add(add);
    }
    base
}

pub fn velocity_from_source(volume: i32) -> u8 {
    let clamped = volume.clamp(0, 15);
    let value = ((clamped * 127) + 7) / 15;
    value.max(1) as u8
}
