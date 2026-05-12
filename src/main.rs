mod diagnostics;
mod ir;
mod parser;
mod render;
mod smfmap;

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use ir::{
    ChannelOverflowPolicy, ConversionOptions, EncodingChoice, EnvelopeMode, RhythmMap, TonePolicy,
};
use smfmap::{Numbering, PsgNoiseDefaultAction, RegisterMapPolicy};

fn main() {
    if let Err(err) = run() {
        eprintln!("mgs2smf: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut cli = Cli::parse(env::args().skip(1).collect())?;
    if cli.help {
        print_help();
        return Ok(());
    }

    let config_files = load_smfmap_configs(&mut cli)?;

    let loaded = parser::loader::load_path(&cli.input, cli.options.encoding)?;
    let mut song = parser::parse_source(&loaded.text, &cli.options)?;
    song.diagnostics.input_file = Some(cli.input.display().to_string());
    song.diagnostics.output_file = cli.output.as_ref().map(|path| path.display().to_string());
    song.diagnostics.config_files = config_files;
    song.diagnostics.ppq = cli.options.ppq;
    song.diagnostics.encoding = Some(loaded.encoding_used);
    for warning in loaded.warnings {
        song.diagnostics.add_parse_warning(None, None, warning);
    }

    if let Some(path) = &cli.emit_config_skeleton {
        fs::write(path, smfmap::emit_config_skeleton(&song))
            .map_err(|e| format!("failed to write config skeleton: {e}"))?;
        if cli.output.is_none() {
            if let Some(path) = &cli.diagnostics {
                fs::write(path, song.diagnostics.to_json(&song.tempo_events))
                    .map_err(|e| format!("failed to write diagnostics: {e}"))?;
            }
            return Ok(());
        }
    }

    if cli.options.strict && song.diagnostics.has_warnings() {
        if let Some(path) = &cli.diagnostics {
            fs::write(path, song.diagnostics.to_json(&song.tempo_events))
                .map_err(|e| format!("failed to write diagnostics: {e}"))?;
        }
        return Err("strict mode rejected warnings before rendering".to_string());
    }

    let midi = render::render_smf(&mut song, &cli.options)?;

    if cli.options.strict && song.diagnostics.has_warnings() {
        if let Some(path) = &cli.diagnostics {
            fs::write(path, song.diagnostics.to_json(&song.tempo_events))
                .map_err(|e| format!("failed to write diagnostics: {e}"))?;
        }
        return Err("strict mode rejected warnings during rendering".to_string());
    }

    let output = cli
        .output
        .as_ref()
        .ok_or("missing output path; use -o output.mid")?;
    fs::write(output, midi).map_err(|e| format!("failed to write output MIDI: {e}"))?;

    if let Some(path) = &cli.diagnostics {
        fs::write(path, song.diagnostics.to_json(&song.tempo_events))
            .map_err(|e| format!("failed to write diagnostics: {e}"))?;
    }

    Ok(())
}

#[derive(Debug)]
struct Cli {
    input: PathBuf,
    output: Option<PathBuf>,
    diagnostics: Option<PathBuf>,
    config_paths: Vec<PathBuf>,
    emit_config_skeleton: Option<PathBuf>,
    options: ConversionOptions,
    ppq_override: Option<u16>,
    channel_overflow_override: Option<ChannelOverflowPolicy>,
    program_numbering_override: Option<Numbering>,
    channel_numbering_override: Option<Numbering>,
    help: bool,
}

impl Cli {
    fn parse(args: Vec<String>) -> Result<Self, String> {
        if args.iter().any(|a| a == "-h" || a == "--help") {
            return Ok(Self {
                input: PathBuf::new(),
                output: None,
                diagnostics: None,
                config_paths: Vec::new(),
                emit_config_skeleton: None,
                options: ConversionOptions::default(),
                ppq_override: None,
                channel_overflow_override: None,
                program_numbering_override: None,
                channel_numbering_override: None,
                help: true,
            });
        }

        let mut input: Option<PathBuf> = None;
        let mut output: Option<PathBuf> = None;
        let mut diagnostics: Option<PathBuf> = None;
        let mut config_paths = Vec::new();
        let mut emit_config_skeleton: Option<PathBuf> = None;
        let mut options = ConversionOptions::default();
        let mut ppq_override = None;
        let mut channel_overflow_override = None;
        let mut program_numbering_override = None;
        let mut channel_numbering_override = None;

        let mut i = 0;
        while i < args.len() {
            match args[i].as_str() {
                "-o" | "--output" => {
                    i += 1;
                    output = Some(PathBuf::from(
                        args.get(i).ok_or("--output requires a path")?,
                    ));
                }
                "--ppq" => {
                    i += 1;
                    let value = args.get(i).ok_or("--ppq requires an integer")?;
                    options.ppq = value
                        .parse::<u16>()
                        .map_err(|_| format!("invalid --ppq value: {value}"))?;
                    ppq_override = Some(options.ppq);
                }
                "--loop-count" => {
                    i += 1;
                    let value = args.get(i).ok_or("--loop-count requires an integer")?;
                    options.loop_count = Some(
                        value
                            .parse::<u32>()
                            .map_err(|_| format!("invalid --loop-count value: {value}"))?,
                    );
                }
                "--loop-marker-name" => {
                    i += 1;
                    let value = args.get(i).ok_or("--loop-marker-name requires start:end")?;
                    let (start, end) = parse_loop_marker_name(value)?;
                    options.loop_marker_start = start;
                    options.loop_marker_end = end;
                }
                "--envelope-mode" => {
                    i += 1;
                    let value = args.get(i).ok_or("--envelope-mode requires a value")?;
                    options.envelope_mode = EnvelopeMode::parse(value)?;
                }
                "--tone-policy" => {
                    i += 1;
                    let value = args.get(i).ok_or("--tone-policy requires a value")?;
                    options.tone_policy = TonePolicy::parse(value)?;
                }
                "--rhythm-map" => {
                    i += 1;
                    let value = args.get(i).ok_or("--rhythm-map requires a value")?;
                    options.rhythm_map = RhythmMap::parse(value)?;
                }
                "--channel-overflow" => {
                    i += 1;
                    let value = args.get(i).ok_or("--channel-overflow requires a value")?;
                    options.channel_overflow = ChannelOverflowPolicy::parse(value)?;
                    channel_overflow_override = Some(options.channel_overflow);
                }
                "--encoding" => {
                    i += 1;
                    let value = args.get(i).ok_or("--encoding requires a value")?;
                    options.encoding = EncodingChoice::parse(value)?;
                }
                "--octave-base" => {
                    i += 1;
                    let value = args
                        .get(i)
                        .ok_or("--octave-base requires o4c=<midi_note>")?;
                    options.octave_base = parse_octave_base(value)?;
                }
                "--diagnostics" => {
                    i += 1;
                    diagnostics = Some(PathBuf::from(
                        args.get(i).ok_or("--diagnostics requires a path")?,
                    ));
                }
                "--config" => {
                    i += 1;
                    config_paths.push(PathBuf::from(
                        args.get(i).ok_or("--config requires a path")?,
                    ));
                }
                "--emit-config-skeleton" => {
                    i += 1;
                    emit_config_skeleton = Some(PathBuf::from(
                        args.get(i)
                            .ok_or("--emit-config-skeleton requires a path")?,
                    ));
                }
                "--manual-smf" => {
                    i += 1;
                    options.smfmap.manual_smf_enabled =
                        parse_on_off(args.get(i).ok_or("--manual-smf requires on|off")?)?;
                }
                "--tone-map" => {
                    i += 1;
                    options.smfmap.tone_map_enabled =
                        parse_on_off(args.get(i).ok_or("--tone-map requires on|off")?)?;
                }
                "--psg-envelope-map" => {
                    i += 1;
                    options.smfmap.psg_envelope_map_enabled =
                        parse_on_off(args.get(i).ok_or("--psg-envelope-map requires on|off")?)?;
                }
                "--psg-noise-drum-map" => {
                    i += 1;
                    let enabled =
                        parse_on_off(args.get(i).ok_or("--psg-noise-drum-map requires on|off")?)?;
                    options.smfmap.psg_noise_drum_map_enabled = enabled;
                    if enabled {
                        options.smfmap.psg_noise_default_action = PsgNoiseDefaultAction::DrumMap;
                    }
                }
                "--register-map" => {
                    i += 1;
                    let value = args.get(i).ok_or("--register-map requires off|report|on")?;
                    options.smfmap.register_map_policy = RegisterMapPolicy::parse(value)
                        .ok_or_else(|| format!("invalid --register-map value: {value}"))?;
                }
                "--program-numbering" | "--channel-numbering" => {
                    let option = args[i].clone();
                    i += 1;
                    let value = args.get(i).ok_or("numbering option requires zero|one")?;
                    let numbering = Numbering::parse(value)
                        .ok_or_else(|| format!("invalid numbering value: {value}"))?;
                    if option == "--program-numbering" {
                        options.smfmap.program_numbering = numbering;
                        program_numbering_override = Some(numbering);
                    } else {
                        options.smfmap.channel_numbering = numbering;
                        channel_numbering_override = Some(numbering);
                    }
                }
                "--strict" => {
                    options.strict = true;
                }
                other if other.starts_with('-') => {
                    return Err(format!("unknown option: {other}"));
                }
                path => {
                    if input.is_some() {
                        return Err(format!("unexpected positional argument: {path}"));
                    }
                    input = Some(PathBuf::from(path));
                }
            }
            i += 1;
        }

        Ok(Self {
            input: input.ok_or("missing input .mus path")?,
            output,
            diagnostics,
            config_paths,
            emit_config_skeleton,
            options,
            ppq_override,
            channel_overflow_override,
            program_numbering_override,
            channel_numbering_override,
            help: false,
        })
    }
}

fn print_help() {
    println!(
        "Usage: mgs2smf input.mus -o output.mid [options]\n\
         \n\
         Options:\n\
           --ppq <integer>                         Default: 3600\n\
           --loop-count <integer>                  Unroll [0 ...] infinite loops N times\n\
           --loop-marker-name <start:end>          Default: loopStart:loopEnd\n\
           --envelope-mode <cc11|cc7|note_velocity|split_velocity|off>\n\
           --tone-policy <ignore|gm_program|text_meta>\n\
           --rhythm-map <gm|off>                   Default: gm\n\
           --channel-overflow <error|shared|multi_port>\n\
           --encoding <auto|utf-8|shift_jis>\n\
           --octave-base o4c=<midi_note>           Default: o4c=60\n\
           --config <path>                         Load .smfmap.yaml config\n\
           --emit-config-skeleton <path>           Write observed smfmap skeleton\n\
           --manual-smf <on|off>\n\
           --tone-map <on|off>\n\
           --psg-envelope-map <on|off>\n\
           --psg-noise-drum-map <on|off>\n\
           --register-map <off|report|on|error>\n\
           --program-numbering <zero|one>\n\
           --channel-numbering <zero|one>\n\
           --diagnostics <path>                    Write JSON diagnostics\n\
           --strict                                Treat warnings as errors\n\
           -h, --help                              Show this help"
    );
}

fn load_smfmap_configs(cli: &mut Cli) -> Result<Vec<String>, String> {
    let mut loaded = Vec::new();
    for path in discover_smfmap_configs(&cli.input, &cli.config_paths) {
        cli.options.smfmap.merge_file(&path)?;
        loaded.push(path.display().to_string());
    }
    if cli.ppq_override.is_none() {
        if let Some(ppq) = cli.options.smfmap.smf_ppq {
            cli.options.ppq = ppq;
        }
    }
    if cli.channel_overflow_override.is_none() {
        if let Some(policy) = cli.options.smfmap.channel_overflow {
            cli.options.channel_overflow = policy;
        }
    }
    if let Some(numbering) = cli.program_numbering_override {
        cli.options.smfmap.program_numbering = numbering;
    }
    if let Some(numbering) = cli.channel_numbering_override {
        cli.options.smfmap.channel_numbering = numbering;
    }
    Ok(loaded)
}

fn discover_smfmap_configs(input: &Path, explicit: &[PathBuf]) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    let mut push_if_exists = |path: PathBuf| {
        if path.is_file() {
            let key = path.display().to_string();
            if seen.insert(key) {
                paths.push(path);
            }
        }
    };
    if explicit.is_empty() {
        push_if_exists(PathBuf::from(".smfmap.yaml"));
        if let Some(parent) = input.parent() {
            push_if_exists(parent.join(".smfmap.yaml"));
        }
        push_if_exists(input.with_extension("smfmap.yaml"));
    }
    for path in explicit {
        let key = path.display().to_string();
        if seen.insert(key) {
            paths.push(path.clone());
        }
    }
    paths
}

fn parse_on_off(value: &str) -> Result<bool, String> {
    match value {
        "on" | "true" | "yes" => Ok(true),
        "off" | "false" | "no" => Ok(false),
        _ => Err(format!("expected on|off, got {value}")),
    }
}

fn parse_octave_base(value: &str) -> Result<i32, String> {
    let raw = value.strip_prefix("o4c=").unwrap_or(value);
    let note = raw
        .parse::<i32>()
        .map_err(|_| format!("invalid --octave-base value: {value}"))?;
    if !(0..=127).contains(&note) {
        return Err(format!(
            "--octave-base must be in MIDI note range 0..127: {note}"
        ));
    }
    Ok(note)
}

fn parse_loop_marker_name(value: &str) -> Result<(String, String), String> {
    let Some((start, end)) = value.split_once(':') else {
        return Err("--loop-marker-name must be formatted as start:end".to_string());
    };
    if start.is_empty() || end.is_empty() {
        return Err("--loop-marker-name start and end must be non-empty".to_string());
    }
    Ok((start.to_string(), end.to_string()))
}
