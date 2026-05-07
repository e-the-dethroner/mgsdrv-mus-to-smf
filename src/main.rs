mod diagnostics;
mod ir;
mod parser;
mod render;

use std::env;
use std::fs;
use std::path::PathBuf;

use ir::{
    ChannelOverflowPolicy, ConversionOptions, EncodingChoice, EnvelopeMode, RhythmMap, TonePolicy,
};

fn main() {
    if let Err(err) = run() {
        eprintln!("mgs2smf: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let cli = Cli::parse(env::args().skip(1).collect())?;
    if cli.help {
        print_help();
        return Ok(());
    }

    let loaded = parser::loader::load_path(&cli.input, cli.options.encoding)?;
    let mut song = parser::parse_source(&loaded.text, &cli.options)?;
    song.diagnostics.input_file = Some(cli.input.display().to_string());
    song.diagnostics.output_file = Some(cli.output.display().to_string());
    song.diagnostics.ppq = cli.options.ppq;
    song.diagnostics.encoding = Some(loaded.encoding_used);
    for warning in loaded.warnings {
        song.diagnostics.add_parse_warning(None, None, warning);
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

    fs::write(&cli.output, midi).map_err(|e| format!("failed to write output MIDI: {e}"))?;

    if let Some(path) = &cli.diagnostics {
        fs::write(path, song.diagnostics.to_json(&song.tempo_events))
            .map_err(|e| format!("failed to write diagnostics: {e}"))?;
    }

    Ok(())
}

#[derive(Debug)]
struct Cli {
    input: PathBuf,
    output: PathBuf,
    diagnostics: Option<PathBuf>,
    options: ConversionOptions,
    help: bool,
}

impl Cli {
    fn parse(args: Vec<String>) -> Result<Self, String> {
        if args.iter().any(|a| a == "-h" || a == "--help") {
            return Ok(Self {
                input: PathBuf::new(),
                output: PathBuf::new(),
                diagnostics: None,
                options: ConversionOptions::default(),
                help: true,
            });
        }

        let mut input: Option<PathBuf> = None;
        let mut output: Option<PathBuf> = None;
        let mut diagnostics: Option<PathBuf> = None;
        let mut options = ConversionOptions::default();

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
            output: output.ok_or("missing output path; use -o output.mid")?,
            diagnostics,
            options,
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
           --diagnostics <path>                    Write JSON diagnostics\n\
           --strict                                Treat warnings as errors\n\
           -h, --help                              Show this help"
    );
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
