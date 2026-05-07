# Codex 実装依頼プロンプト: MGSDRV/MGSC111 MUS → SMF 変換器

Rust で CLI ツール `mgs2smf` を実装してください。

参照ファイル:

- `mgsdrv_mus_to_smf_spec.yaml`
- `mgsdrv_mus_to_smf_plan.md`

## 実装ゴール

MGSC111.TXT 仕様の `.MUS` MML テキストを Standard MIDI File Type 1 に変換する。

最優先は以下です。

1. 発音タイミングを正確にする。
2. source track ごとに SMF track を分ける。
3. `#tempo` / `t` を MIDI tempo meta event にする。
4. `v` を note-on velocity にする。
5. `@e` / `@r` を MIDI CC #11 Expression に近似変換する。
6. SCC/OPLL/PSG の音色定義は再現しない。
7. SMF に等価表現がない command は warning と diagnostics に残して無視する。

## 推奨 crate

- `midly`
- `winnow` または `nom`
- `encoding_rs`
- `clap`
- `serde`, `serde_yaml`, `serde_json`
- `anyhow`, `thiserror`

## 必須 CLI

```bash
mgs2smf input.mus -o output.mid
```

必須 option:

```bash
--ppq 3600
--loop-count N
--envelope-mode cc11|cc7|note_velocity|split_velocity|off
--tone-policy ignore|gm_program|text_meta
--encoding auto|utf-8|shift_jis
--diagnostics output.json
--strict
```

## 実装順序

### Step 1: project scaffold

- Rust binary crate。
- `src/main.rs`, `src/parser`, `src/ir`, `src/render`, `src/diagnostics` に分割。
- Snapshot しやすい event list を持つ。

### Step 2: loader

- UTF-8 / Shift-JIS auto detect。
- CRLF / LF 正規化。
- `;` コメント除去。

### Step 3: top-level parser

対応:

- `#opll_mode`
- `#tempo`
- `#title`
- `#end`
- `@eN={...}`
- `@rN={...}`
- `@s`, `@v`, `@#` は parse して ignore warning。
- track line `1..h`。

### Step 4: MML parser MVP

対応:

- `o`, `<`, `>`
- `l`
- `t`
- `v`, `v+`, `v-`, `)`, `(`
- `q`
- notes `a..g`, `+`, `-`
- rest `r`
- length denominator, `%N`, dotted length
- tie `^`
- slur `&`
- finite/infinite loop markers
- `@N`, `@eN`, `@rN`
- `!` track termination

### Step 5: IR

IR は MIDI に直接依存させない。

最低限:

```rust
struct SongIr {
    title: Option<String>,
    ppq: u16,
    tempo_events: Vec<TempoEvent>,
    tracks: Vec<TrackIr>,
    envelopes: EnvelopeTable,
    diagnostics: Diagnostics,
}

struct TrackIr {
    source_id: String,
    events: Vec<IrEvent>,
    length_steps: Rational,
}

enum IrEvent {
    Note { start_steps, duration_steps, note, velocity, envelope_id },
    Rest { start_steps, duration_steps },
    Tempo { at_steps, bpm },
    Control { at_steps, kind },
    LoopMarker { at_steps, kind },
}
```

### Step 6: SMF renderer

- Type 1。
- Conductor track に tempo meta。
- 各 source track を別 SMF track へ。
- 1 source step = ppq / 48 MIDI ticks。
- default ppq = 3600。
- `v` は velocity。
- `@e` / `@r` は CC #11。
- CC は note-on より前に出す。

### Step 7: diagnostics

JSON を出す。

- unsupported command。
- ignored tone definitions。
- channel allocation。
- loop markers。
- parse warnings。
- envelope event count。

### Step 8: tests

必ず入れる test:

1. `#tempo 120 / cdef` が 4 note になり、各 note が 3600 ticks。
2. `%48` が 3600 ticks。
3. `r%48` が 3600 ticks の gap。
4. `@e` が CC #11 を生成する。
5. `[0 ...]` が loopStart / loopEnd marker を生成する。
6. unsupported `@s` / `@v` が warning になる。

## 注意点

- 音色再現を実装しないでください。
- `@e` を単純に note velocity に潰さないでください。既定は CC #11 です。
- float は最終段まで使わず、Fraction / rational / integer accumulator を使ってください。
- channel を共有すると CC #11 が混線するため、既定は 1 source track = 1 MIDI channel です。
- MIDI channel が不足した場合、既定は error にしてください。
- 無限ループは SMF 標準にないため、marker + optional finite unroll にしてください。
