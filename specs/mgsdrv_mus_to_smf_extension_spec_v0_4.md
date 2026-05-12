# MGSDRV / MGSC111 MUS → SMF 変換器 config 拡張仕様 v0.4

## 1. 目的

本仕様は、既存の `mgs2smf` 変換器に以下の拡張を追加するための仕様である。

1. MUS本文から任意のSMF/MIDIイベントを発行する変換器専用コマンドを追加する。
2. `@N` / `@eN` / `@rN` の解釈を track family ごとに分岐し、PSGでは envelope 選択、SCC/OPLLでは tone / envelope map として扱う。
3. SCC/OPLLの音色変更をYAML設定で Program Change / Bank Select / CC / macro にマップできるようにする。
4. OPLL/PSG等の `yR,D` レジスタ書き込みをIRに保持し、初期状態では無視＋診断、必要時のみYAML ruleでSMFイベントへ変換する。
5. config skeleton を自動生成し、曲で実際に使われた `@`、`@e`、`@r`、`y` だけを編集対象にできるようにする。
6. `#opll_mode 0` の通常OPLLトラックを、明示設定時のみ OPLL pseudo drum としてGM drum noteへ置換できるようにする。

本仕様は、既存の v0.1.0 仕様に対する **後方互換のconfig layer拡張** とする。通常のMGSDRV用 `.MUS` としての可読性・互換性を維持し、変換器専用指示はMGSCから見ればコメントになる形式を採用する。

---

## 2. 非目標

以下は本仕様の対象外である。

- SCC波形 `@s` の音響再現。
- OPLLカスタム音色 `@v` の音響再現。
- PSGノイズ周波数、トーン/ノイズモード、ハードエンベロープの完全な音響再現。
- OPLLレジスタ書き込み `yR,D` の完全移植。
- MIDI音源固有SysExの標準対応。
- SMFに存在しないMGSDRV固有動作の完全再現。

ただし、上記は **diagnostics**、**SMF text meta event**、またはユーザー定義macroとして残せるようにする。

---

## 3. 用語

| 用語 | 意味 |
|---|---|
| source track | MGSDRV/MGSC111の `1..9,a..h,r/f` トラック |
| family | `psg`, `psg_noise`, `scc`, `opll`, `rhythm` の分類 |
| AtCommand | MUS本文中の `@N`, `@eN`, `@rN` を抽象化したIRイベント |
| PsgEnvelopeSelect | PSGで `@N` / `@eN` / `@rN` が現在のエンベロープを選択するIRイベント |
| ToneChange | SCC/OPLLで `@N` が音色状態を変更するIRイベント |
| EnvelopeApply | SCC/OPLLで `@eN` / `@rN` が現在のエンベロープを変更するIRイベント |
| RegisterWrite | `yR,D` レジスタ書き込みを保持するIRイベント |
| ManualSmf | `;@smf ...` で発行される任意SMF/MIDIイベント |
| smfmap | 本仕様で導入するYAML設定ファイル |
| source_family | 物理的な入力元。`psg`, `psg_noise`, `scc`, `opll`, `rhythm` |
| render_role | SMF出力上の役割。`melody` または `drum` |
| OPLL pseudo drum | `#opll_mode 0` の通常OPLL/FMトラックを打楽器素材として使う上級者データの変換モード |

---

## 4. 互換性方針

### 4.1 MUS互換

変換器専用コマンドはコメント形式にする。

```mml
9 ;@smf pc 80
9 c4 d4 e4
```

MGSCは `;` 以降をコメントとして無視する。変換器だけが、コメント本文の先頭が `@smf` の場合に directive として解釈する。

### 4.2 既存変換器との互換

設定ファイルを指定しない場合、既存v0.1.0と同等の変換を行う。ただし、PSGの `@N` については、本仕様では以下を正しい既定とする。

```text
PSG の @N / @eN / @rN は Program Change ではなく envelope selection として扱う。
```

音色のProgram Change出力は、SCC/OPLLに対してのみ `tone_map` により行う。

---

## 5. 変換器専用SMF directive

### 5.1 基本構文

```ebnf
SmfDirectiveLine := TrackPrefix? MmlBeforeComment? ';' WS* '@smf' WS+ SmfDirective
SmfDirective     := Target? Command
Target           := 'track=' TrackId WS+ | 'channel=' Int WS+ | 'conductor' WS+
Command          := PcCommand | BankCommand | CcCommand | PbCommand
                  | RpnCommand | NrpnCommand | MarkerCommand | TextCommand
                  | MacroCommand | ResetCommand
```

トラック行の中に書かれた場合、`track=` を省略できる。

```mml
9 ;@smf pc 80
9 ;@smf cc 74 96
9 ;@smf macro bright_lead
```

トップレベルに書く場合は `track=` または `conductor` を明示する。

```mml
;@smf conductor marker "loop preparation"
;@smf track=9 pc 80
```

### 5.2 時刻の意味

`@smf` directive は **現在のsource step位置** に、時間を進めずにイベントを挿入する。

```mml
9 c4 ;@smf cc 74 100
9 d4
```

この例では、`cc 74 100` は `c4` の発音終了後、`d4` の発音開始前の現在位置に入る。

同じ行で音符より前に置いた場合は、その行の現在位置に入る。

```mml
9 ;@smf pc 80 c4
```

この例では、Program Change は `c4` のNote Onより前に入る。

### 5.3 対応コマンド

| command | 引数 | 出力 | 備考 |
|---|---:|---|---|
| `pc` | `program` | Program Change | 既定は0-based program number |
| `bank` | `msb lsb` | CC#0 + CC#32 | Program Changeより前に並べる |
| `cc` | `controller value` | Control Change | 値は0..127 |
| `pb` | `value` | Pitch Bend | `-8192..8191` または `0..16383` 設定式 |
| `rpn` | `msb lsb value` | RPN sequence | Data Entry MSB/LSB使用 |
| `nrpn` | `msb lsb value` | NRPN sequence | Data Entry MSB/LSB使用 |
| `marker` | `"text"` | Marker meta | conductorまたはtrack meta |
| `text` | `"text"` | Text meta | 診断・注記 |
| `macro` | `name` | YAML macro展開 | 複数イベント可 |
| `reset` | なし/種類 | CC reset sequence | YAMLで定義 |

### 5.4 数値表記

以下を許可する。

```text
10       decimal
0x0a     hexadecimal
$0a      hexadecimal
%1010    binary, optional
```

### 5.5 quote

`marker` / `text` はダブルクォート文字列を使う。

```mml
;@smf conductor marker "A section"
;@smf track=9 text "OPLL y14 ignored"
```

escapeは最低限以下に対応する。

```text
\"  quote
\\  backslash
\n   newline
\t   tab
```

### 5.6 未対応directive

未知の `;@smf` command は diagnostics に出す。

```yaml
manual_smf:
  unknown_command_policy: error
```

既定は `error`。通常コメントと違い、`@smf` と明示されたものはタイプミスが曲の再現性に影響するためである。

---

## 6. AtCommand dispatch

### 6.1 基本方針

`@N`, `@eN`, `@rN` は parse段階ではすべて `AtCommand` として保持し、resolve段階で family に応じて変換する。

```text
parse:
  @ / @e / @r → AtCommand

resolve:
  family=psg       → PsgEnvelopeSelect
  family=psg_noise → PsgEnvelopeSelect または optional drum map
  family=scc       → @ は ToneChange、@e/@r は EnvelopeApply
  family=opll      → @ は ToneChange、@e/@r は EnvelopeApply
```

### 6.2 PSG

PSGでは、`@N`, `@eN`, `@rN` をエンベロープ選択として扱う。

```text
@N   → current_psg_envelope = N
@eN  → current_psg_envelope = N
@rN  → current_psg_envelope = N
```

選択したエンベロープは、次のNote Onごとに先頭から再生する。

```mml
@e1 = {1,0,f:2,8=4,[84]}
1 @1 c4 d4
```

この例では、`c4` と `d4` のそれぞれで envelope 1 が再スタートし、既定ではCC#11カーブとして出力される。

### 6.3 PSG noise track

`family: psg_noise` では、PSG envelope として扱うのを既定とし、必要な曲だけ drum map を有効化する。

```text
既定: @N → psg envelope → CC#11
任意: @N → GM drum note / velocity / CC macro
```

Track3などのノイズ運用では、`@e` だけでなく `@NN` による切替を確実に反映する。

### 6.4 SCC

SCCでは、`@N` は tone map に渡す。

```text
@N → ToneChange(family=scc, tone=N)
```

`@eN` / `@rN` は現在のSCC toneに適用するエンベロープとして扱う。

```text
@eN → EnvelopeApply(family=scc, envelope=N, kind=e)
@rN → EnvelopeApply(family=scc, envelope=N, kind=r)
```

SMFでは、`EnvelopeApply` は現在のMIDI channel上のCC#11カーブ選択として扱う。

### 6.5 OPLL/FM

OPLL/FMでは、`@N` は tone map に渡す。

```text
@N → ToneChange(family=opll, tone=N)
```

`@eN` / `@rN` は現在のtoneに対するエンベロープとして扱う。

```text
@eN → EnvelopeApply(family=opll, envelope=N, kind=e)
@rN → EnvelopeApply(family=opll, envelope=N, kind=r)
```

### 6.6 `@e`定義内の`@N`

`@e`定義内に `@N` が存在する場合は、`AtCommand` の `context` を `EnvelopeData` とする。

既定方針は以下。

```yaml
envelope_side_effects:
  tone_change_inside_envelope:
    policy: emit_at_exact_tick_with_warning
    allow_program_change: true
    allow_cc: true
    allow_split_note: false
```

ただし、Program Changeが発音中に即時反映されるかはMIDI音源依存である。実務上は、`@e`内の`@N`はProgram ChangeよりCC変化にマップすることを推奨する。

---

## 7. YAML config: top-level

設定ファイルの標準拡張子は `.smfmap.yaml` とする。

```yaml
version: 0.4
profile_name: default

smf:
  type: 1
  ppq: 3600
  program_numbering: zero_based
  channel_numbering: zero_based

tracks: {}
manual_smf: {}
at_command_dispatch: {}
psg_envelope_map: {}
psg_noise_map: {}
opll_pseudo_drum_map: {}
tone_map: {}
envelope_map: {}
opll_register_map: {}
smf_macros: {}
diagnostics: {}
```

### 7.1 merge順序

設定は以下の順でmergeする。

```text
built-in defaults
  → global config
  → song config
  → CLI override
```

mapの同名keyは後勝ち。配列は既定では置換する。`merge_mode: append` が指定された配列のみ追加する。

---

## 8. tracks設定

```yaml
tracks:
  "1":
    source_family: psg
    midi_channel: 0
    at_command_policy: psg_envelope

  "2":
    source_family: psg
    midi_channel: 1
    at_command_policy: psg_envelope

  "3":
    source_family: psg_noise
    midi_channel: 2
    at_command_policy: psg_envelope_or_drum

  "4":
    source_family: scc
    midi_channel: 3
    at_command_policy: tone_map

  "5":
    source_family: scc
    midi_channel: 4
    at_command_policy: tone_map

  "9":
    source_family: opll
    midi_channel: 8
    at_command_policy: tone_map

  "h":
    source_family: opll
    render_role: drum
    drum_map: default_opll_pseudo
    midi_channel: 9
```

v0.4では、物理的な入力元を `source_family`、SMF出力上の役割を `render_role` として分ける。従来互換のため `family` も受け付けるが、OPLL疑似ドラムでは `source_family: opll` と `render_role: drum` を明示することを推奨する。

`family: rhythm` は短縮記法として残す。OPLL系トラックでは内部的に概ね `source_family: opll`、`render_role: drum`、`drum_map: default_opll_pseudo` として正規化できる。ただし、`#opll_mode 1` の正式 rhythm track は `source_family: rhythm` / native rhythm map として扱い、OPLL pseudo drumとは区別する。

### 8.1 family既定

`#opll_mode 0` の既定:

```yaml
family_defaults:
  psg: ["1", "2"]
  psg_noise: ["3"]
  scc: ["4", "5", "6", "7", "8"]
  opll: ["9", "a", "b", "c", "d", "e", "f", "g", "h"]
  rhythm: []
```

`#opll_mode 1` の既定:

```yaml
family_defaults:
  psg: ["1", "2"]
  psg_noise: ["3"]
  scc: ["4", "5", "6", "7", "8"]
  opll: ["9", "a", "b", "c", "d", "e"]
  rhythm: ["f", "r"]
```

`tracks`で明示された設定は、`#opll_mode`由来の既定を上書きする。

---

## 9. PSG envelope map

### 9.1 基本設定

```yaml
psg_envelope_map:
  enabled: true
  default:
    source: mus_definition
    emit_expression_cc: true
    expression_cc: 11
    trigger:
      on_at_command: select_only
      on_note_on: restart_envelope
    loop_policy:
      finite_note_duration: render_until_note_off
      infinite_envelope_loop: render_until_note_off
    mode_noise_policy:
      tone_only: ignore
      noise_only: report
      tone_and_noise: report
      emit_meta_text: true
  envelopes: {}
```

### 9.2 音量変換

`@e` の音量レベル `0..15` を MIDI CC value `0..127` へ変換する。

```text
cc_value = round(level / 15 * 127)
```

configで範囲変更できる。

```yaml
psg_envelope_map:
  default:
    volume_scale:
      mgsc_min: 0
      mgsc_max: 15
      midi_min: 0
      midi_max: 127
      curve: linear
```

### 9.3 envelope override

曲ごとに特定のエンベロープを補正できる。

```yaml
psg_envelope_map:
  envelopes:
    "12":
      name: noise_hit
      override:
        expression_multiplier: 0.85
        velocity_bias: -8
        min_cc: 0
        max_cc: 110
```

### 9.4 undefined envelope

`@N` が参照する `@eN` / `@rN` 定義が存在しない場合の既定は warning + no envelope である。

```yaml
psg_envelope_map:
  undefined_policy: warn_and_no_envelope
```

候補:

```text
warn_and_no_envelope
warn_and_keep_previous
error
```

---

## 10. PSG noise map

PSG noise trackでは、初期実装は `psg_envelope_map` に流す。MIDI drumへの変換は任意機能とする。

```yaml
psg_noise_map:
  enabled: true
  default_action: envelope_cc
  report_mode_noise: true

  drum_map:
    enabled: false
    trigger: on_note_on
    channel: 9
    envelopes:
      "20":
        name: noise_kick
        note: 36
        velocity_from_envelope_peak: true
      "21":
        name: noise_snare
        note: 38
        velocity_from_envelope_peak: true
      "22":
        name: noise_hihat
        note: 42
        velocity_from_envelope_peak: true
```

`drum_map.enabled: true` の場合、該当 envelope number に対して melody noteではなくGM drum noteを出せる。ただし、既定では無効とする。

---

## 10.5 OPLL pseudo drum map

`#opll_mode 0` の `f/g/h` などは物理的には通常OPLL/FMトラックである。曲によってはこれらをキック、スネア、ハイハット、タム等の疑似ドラム素材として使うため、v0.4では明示設定時のみGM drum noteへ置換できる。

既定では無効とする。通常の `#opll_mode 0` 資産では `f/g/h` がメロディ、和音、効果音として使われる可能性があるため、自動判定は行わない。

```yaml
tracks:
  "f": { source_family: opll, render_role: drum, drum_map: default_opll_pseudo, midi_channel: 9 }
  "g": { source_family: opll, render_role: drum, drum_map: default_opll_pseudo, midi_channel: 9 }
  "h": { source_family: opll, render_role: drum, drum_map: default_opll_pseudo, midi_channel: 9 }

opll_pseudo_drum_map:
  enabled: true
  maps:
    default_opll_pseudo:
      output:
        midi_channel: 9
        mode: replace
        unmatched: warn_and_drop
        suppress_tone_map: true
        suppress_program_change: true
      hit_grouping:
        enabled: true
        suppress_continuations:
          - macro_continuation
          - slur_ampersand
      rules:
        - { name: kick,       match: { macro_symbol: "b" }, drum: { note: 36, velocity: source_volume } }
        - { name: snare,      match: { macro_symbol: "s" }, drum: { note: 38, velocity: source_volume } }
        - { name: closed_hat, match: { macro_symbol: "h" }, drum: { note: 42, velocity: source_volume } }
        - { name: open_hat,   match: { macro_symbol: "o" }, drum: { note: 46, velocity: source_volume } }
```

`@N`、`@eN`、`vN` は疑似ドラムでもOPLL/FMの状態として保持する。`@N` は Program Change 出力ではなく、`source_tone` / `effective_tone` のmatch材料になる。`@eN` は `envelope`、通常音符は `note_name` / `octave` / `source_midi_note` としてmatchできる。

match key:

```text
macro_symbol
track / source_track
envelope
source_tone
effective_tone / effective_rom_tone
note_name
octave
source_midi_note
```

`output.mode`:

```text
replace      元noteを消し、GM drum noteだけを出す。
add          元noteも残し、GM drum noteも追加する。
passthrough  変換せず元noteだけを出す。
```

`hit_grouping.enabled: true` では、同一マクロ内の2個目以降のmatching noteを継続音として抑制できる。休符または次のマクロ開始で新しいヒットとして扱う。`slur_ampersand` も継続音として抑制できる。`pitch_slide_underscore` は設定名として予約するが、v0.4実装では pitch-slide MML は引き続き未再現診断の対象とする。

---

## 11. SCC/OPLL tone map

### 11.1 基本設定

`tone_map` はSCC/OPLL専用とする。PSGは含めない。

```yaml
tone_map:
  defaults:
    on_unmapped: keep_previous_and_warn
    emit_before_next_note: true
    emit_text_meta: false
  scc:
    enabled: true
    tones: {}
  opll:
    enabled: true
    respect_at_hash_rom_assign: true
    tones: {}
```

### 11.2 tone event

各toneは複数イベントを持てる。

```yaml
tone_map:
  scc:
    tones:
      "0":
        name: scc_square_like
        events:
          - pc: { program: 80 }
          - cc: { controller: 74, value: 100 }

  opll:
    tones:
      "1":
        name: opll_guitar_to_gm_guitar
        events:
          - pc: { program: 24 }
          - cc: { controller: 7, value: 105 }
```

対応event type:

```text
pc
bank
cc
pb
rpn
nrpn
marker
text
macro
```

### 11.3 OPLL built-in tone skeleton

OPLLの組み込み音色は、便宜的なGM割当のskeletonを生成できる。これは音色再現ではなく、DAW編集用の初期値である。

```yaml
tone_map:
  opll:
    tones:
      "0":  { name: violin,        events: [ { pc: { program: 40 } } ] }
      "1":  { name: guitar,        events: [ { pc: { program: 24 } } ] }
      "2":  { name: piano,         events: [ { pc: { program: 0  } } ] }
      "3":  { name: flute,         events: [ { pc: { program: 73 } } ] }
      "4":  { name: clarinet,      events: [ { pc: { program: 71 } } ] }
      "5":  { name: oboe,          events: [ { pc: { program: 68 } } ] }
      "6":  { name: trumpet,       events: [ { pc: { program: 56 } } ] }
      "7":  { name: organ,         events: [ { pc: { program: 16 } } ] }
      "8":  { name: horn,          events: [ { pc: { program: 60 } } ] }
      "9":  { name: synthesizer,   events: [ { pc: { program: 80 } } ] }
      "10": { name: harpsichord,   events: [ { pc: { program: 6  } } ] }
      "11": { name: vibraphone,    events: [ { pc: { program: 11 } } ] }
      "12": { name: synth_bass,    events: [ { pc: { program: 38 } } ] }
      "13": { name: wood_bass,     events: [ { pc: { program: 32 } } ] }
      "14": { name: electric_bass, events: [ { pc: { program: 33 } } ] }
      "15": { name: custom_user,   events: [] }
      "16": { name: custom_user,   events: [] }
```

### 11.4 `@#` ROM tone assignment

`respect_at_hash_rom_assign: true` の場合、`@#N = M` を以下のように解釈する。

```text
MUS本文の @N → logical tone N
@#N = M が存在 → tone_map.opll.tones[M] を参照
@#N = M が存在しない → tone_map.opll.tones[N] を参照
```

初期実装では `@#` を無視してもよいが、diagnosticsには残す。

---

## 12. Envelope map for SCC/OPLL

PSG以外でも、`@eN` / `@rN` はCC#11カーブとして出力する。

```yaml
envelope_map:
  enabled: true
  default_mode: cc11
  midi_cc:
    expression: 11
    volume: 7
  trigger:
    on_apply_command: select_only
    on_note_on: restart_envelope
  families:
    scc:
      enabled: true
    opll:
      enabled: true
```

### 12.1 note-on前の初期CC

Note Onの前に、選択中エンベロープの初期CCを出す。

```yaml
envelope_map:
  emit_initial_cc_before_note_on: true
```

### 12.2 split velocity fallback

```yaml
envelope_map:
  fallback_modes:
    split_velocity:
      enabled: false
      min_segment_ticks: 30
```

既定では無効。CC#11非対応音源向けの代替として残す。

---

## 13. `yR,D` register map

### 13.1 parse方針

通常MML内と`@e`定義内の `yR,D` は、捨てずに `RegisterWrite` としてIRに残す。

```rust
RegisterWrite {
    source_track,
    source_step,
    family,
    register,
    data,
    context, // NormalMml | EnvelopeData
}
```

### 13.2 既定方針

```yaml
opll_register_map:
  enabled: true
  default_policy: ignore_and_report
  report:
    collect_usage: true
    group_by: [family, register, data, source_track, context]
  rules: []
```

初期状態では一切SMFイベントを出さない。ただし、使用実態をdiagnosticsとconfig skeletonに出す。

### 13.3 rule形式

```yaml
opll_register_map:
  rules:
    - name: opll_y2_to_expression
      enabled: false
      match:
        family: opll
        register: 2
        data: "*"
        context: "*"
      emit:
        - cc:
            controller: 11
            value: "expr: clamp(127 - round(data * 127 / 63), 0, 127)"
```

### 13.4 match

`match` は以下を指定できる。

```yaml
match:
  family: opll           # psg | psg_noise | scc | opll | rhythm | *
  source_track: "9"      # track id or *
  register: 2            # number or *
  data: "*"              # number, range, list, or *
  context: NormalMml     # NormalMml | EnvelopeData | *
```

`data`は以下も許可する。

```yaml
data: [0, 1, 2]
data: { min: 0, max: 63 }
```

### 13.5 expression language

`expr:` は安全な式評価のみ許可する。

使用可能変数:

```text
data
register
source_step
midi_tick
track
family
tone
envelope
```

使用可能関数:

```text
clamp(x, min, max)
round(x)
floor(x)
ceil(x)
min(a, b)
max(a, b)
```

式からMIDI値を生成する場合は、最終的に許容範囲へclampする。

---

## 14. SMF macro

### 14.1 定義

```yaml
smf_macros:
  bright_lead:
    events:
      - pc: { program: 80 }
      - cc: { controller: 7, value: 110 }
      - cc: { controller: 11, value: 127 }
      - cc: { controller: 74, value: 105 }
      - cc: { controller: 91, value: 24 }

  reset_channel:
    events:
      - cc: { controller: 121, value: 0 }
      - cc: { controller: 7, value: 100 }
      - cc: { controller: 10, value: 64 }
      - cc: { controller: 11, value: 127 }
```

### 14.2 呼び出し

MUS本文:

```mml
9 ;@smf macro bright_lead
9 o5 v15 l8 cdefgab>c
```

YAML内:

```yaml
tone_map:
  scc:
    tones:
      "0":
        events:
          - macro: { name: bright_lead }
```

### 14.3 再帰制限

macroが別macroを呼ぶことは許可するが、最大深度を設定する。

```yaml
smf_macros:
  max_depth: 8
```

循環参照はerror。

---

## 15. イベント順序

同一tickに複数イベントが発生した場合、以下の順序で出力する。

```text
1. Note Off
2. Bank Select CC#0 / CC#32
3. Program Change
4. RPN / NRPN select
5. 通常CC
6. Pitch Bend
7. @e/@r由来の初期Expression CC
8. Text / Marker meta
9. Note On
10. 発音後のenvelope CCカーブ
```

設定で一部変更可能だが、Program Changeと初期ExpressionはNote Onより前に出すのを既定とする。

```yaml
smf:
  event_order:
    - note_off
    - bank_select
    - program_change
    - rpn_nrpn
    - cc
    - pitch_bend
    - initial_expression
    - meta
    - note_on
    - envelope_curve
```

---

## 16. IR変更

既存IRに以下を追加する。

```rust
enum IrEvent {
    Note { ... },
    Rest { ... },
    Tempo { ... },

    AtCommand {
        source_track: TrackId,
        source_step: SourceStep,
        family: SourceFamily,
        number: u8,
        spelling: AtSpelling, // At | AtE | AtR
        context: AtContext,   // NormalMml | EnvelopeData
        source_span: SourceSpan,
    },

    PsgEnvelopeSelect {
        source_track: TrackId,
        source_step: SourceStep,
        envelope_number: u8,
        kind: EnvelopeKind, // E | R | Unknown
    },

    ToneChange {
        source_track: TrackId,
        source_step: SourceStep,
        family: SourceFamily, // Scc | Opll
        tone_number: u8,
    },

    EnvelopeApply {
        source_track: TrackId,
        source_step: SourceStep,
        family: SourceFamily, // Scc | Opll
        envelope_number: u8,
        kind: EnvelopeKind, // E | R
    },

    RegisterWrite {
        source_track: TrackId,
        source_step: SourceStep,
        family: SourceFamily,
        register: u8,
        data: u8,
        context: RegisterContext, // NormalMml | EnvelopeData
        source_span: SourceSpan,
    },

    ManualSmf {
        source_track: Option<TrackId>,
        source_step: SourceStep,
        request: SmfRequest,
        source_span: SourceSpan,
    },
}
```

### 16.1 resolver処理

```text
for event in parsed_ir:
  if event is AtCommand:
    resolve by family and spelling
  if event is RegisterWrite:
    keep for register_map resolver
  if event is ManualSmf:
    resolve immediately using macros/config
```

---

## 17. Diagnostics

### 17.1 出力項目

```yaml
diagnostics:
  include:
    - input_file
    - output_file
    - config_files
    - track_summary
    - manual_smf_events
    - tone_usage
    - psg_envelope_usage
    - scc_opll_envelope_usage
    - register_write_usage
    - unmapped_tones
    - undefined_envelopes
    - unsupported_commands
    - warnings
```

### 17.2 register write usage例

```json
{
  "register_write_usage": [
    {
      "family": "opll",
      "track": "9",
      "register": 14,
      "data": 32,
      "context": "NormalMml",
      "count": 3,
      "first_line": 128,
      "policy": "ignored"
    }
  ]
}
```

### 17.3 tone usage例

```json
{
  "tone_usage": [
    {
      "family": "scc",
      "track": "4",
      "tone": 12,
      "count": 18,
      "mapped": false,
      "first_line": 42
    },
    {
      "family": "opll",
      "track": "9",
      "tone": 1,
      "count": 4,
      "mapped": true,
      "first_line": 12
    }
  ]
}
```

---

## 18. Config skeleton生成

### 18.1 CLI

```bash
mgs2smf song.mus --emit-config-skeleton song.smfmap.yaml
```

変換と同時に出す場合:

```bash
mgs2smf song.mus -o song.mid --config song.smfmap.yaml --diagnostics song.report.json
```

### 18.2 skeleton内容

skeletonには、曲で実際に使われたものだけを出す。SCC/OPLL の `@eN` が実際に使われている場合、その envelope 定義内の `@tone` も `tone_map` の観測toneとして出す。

```yaml
version: 0.4
generated_from: song.mus

tracks:
  "3": { source_family: psg_noise, midi_channel: 2, at_command_policy: psg_envelope_or_drum }
  "4": { source_family: scc, midi_channel: 3, at_command_policy: tone_map }
  "9": { source_family: opll, midi_channel: 8, at_command_policy: tone_map }
  # OPLL pseudo drums are not inferred automatically; enable only when verified.
  # "h": { source_family: opll, render_role: drum, drum_map: default_opll_pseudo, midi_channel: 9 }

psg_envelope_map:
  envelopes:
    "20":
      name: "PSG envelope @20"
      used_by_tracks: ["3"]
      events: []

psg_noise_map:
  drum_map:
    enabled: false
    envelopes:
      "20":
        name: "noise hit candidate"
        note: 38
        velocity_from_envelope_peak: true

tone_map:
  scc:
    tones:
      "12":
        name: "SCC @12"
        used_by_tracks: ["4"]
        events: []
  opll:
    tones:
      "1":
        name: "OPLL @1 guitar"
        used_by_tracks: ["9"]
        events:
          - pc: { program: 24 }

opll_register_map:
  observed:
    - family: opll
      register: 14
      data: 32
      count: 2
      tracks: ["9"]
      rule: null
```

---

## 19. CLI追加

```bash
--config <path>                       # smfmap yaml
--emit-config-skeleton <path>          # 使用tone/envelope/yを元にskeleton生成
--manual-smf <on|off>                  # ;@smfを読むか
--program-numbering <zero|one>         # config/program CLI表記
--channel-numbering <zero|one>         # config/channel CLI表記
--tone-map <on|off>                    # SCC/OPLL tone_mapを使うか
--psg-envelope-map <on|off>            # PSG @N/@eN/@rNをenvelopeとして使うか
--psg-noise-drum-map <on|off>          # PSG noise drum_mapを使うか
--register-map <off|report|on>         # yR,D の扱い
```

既定:

```text
manual-smf=on
program-numbering=zero
channel-numbering=zero
tone-map=on if config exists else off
psg-envelope-map=on
psg-noise-drum-map=off
register-map=report
```

---

## 20. 実装フェーズ

### Phase 1: manual SMF + config loader

- YAML config loader。
- `;@smf` directive parser。
- `pc`, `bank`, `cc`, `marker`, `text`, `macro`対応。
- macro展開。
- diagnosticsにmanual event countを出す。

### Phase 2: family-aware AtCommand resolver

- `AtCommand` IR導入。
- PSG `@N` / `@eN` / `@rN` → `PsgEnvelopeSelect`。
- SCC/OPLL `@N` → `ToneChange`。
- SCC/OPLL `@eN` / `@rN` → `EnvelopeApply`。
- PSGの`@N`がtone_mapに流れないことをテストで保証。

### Phase 3: tone_map + envelope_map

- SCC/OPLL `tone_map`。
- OPLL built-in skeleton。
- `@#` はparse + diagnostic。可能ならlogical tone解決。
- PSG envelope mapを`@N`にも適用。

### Phase 4: register map diagnostics

- `yR,D`をIRに残す。
- normal MML / envelope dataのcontext区別。
- register_write_usageをdiagnosticsとskeletonに出す。

### Phase 5: register map rule engine

- match/emit rule。
- `expr:` evaluator。
- ruleでCC/PC/text/metaを発行。

### Phase 6: PSG noise drum map

- `psg_noise_map.drum_map`。
- `@N` envelope peakからvelocity算出。
- GM drum channel出力。

### Phase 7: OPLL pseudo drum map

- `source_family` と `render_role` の分離。
- `#opll_mode 0` OPLLトラックを明示設定時のみGM drum noteへ置換。
- `#macro_offset` 付きマクロ参照を `macro_symbol` としてnote metadataに保持。
- `source_tone` / `effective_tone` / `envelope` / `note_name` 等のmatch。
- `hit_grouping` によるマクロ内継続note抑制。

---

## 21. Acceptance tests

### 21.1 `;@smf pc`

Input:

```mml
#opll_mode 0
#tempo 120
9 ;@smf pc 80
9 o4 v15 c4
```

Expected:

```yaml
track: "9"
events_before_first_note:
  - program_change: 80
first_note: { note: 60, velocity: 127 }
```

### 21.2 `;@smf macro`

Config:

```yaml
smf_macros:
  bright:
    events:
      - pc: { program: 80 }
      - cc: { controller: 74, value: 100 }
```

Input:

```mml
9 ;@smf macro bright
9 c4
```

Expected:

```yaml
events_before_first_note:
  - program_change: 80
  - cc: { controller: 74, value: 100 }
```

### 21.3 PSG `@N` goes to envelope, not tone

Input:

```mml
#opll_mode 0
#tempo 120
@e1 = {1,0,f:2,8=4}
1 @1 c4
```

Expected:

```yaml
source_track: "1"
family: psg
program_change_count: 0
has_cc11_envelope: true
psg_envelope_selected: 1
```

### 21.4 SCC `@N` goes to tone_map

Config:

```yaml
tone_map:
  scc:
    enabled: true
    tones:
      "1":
        events:
          - pc: { program: 81 }
```

Input:

```mml
4 @1 c4
```

Expected:

```yaml
source_track: "4"
family: scc
program_change: 81
```

### 21.5 OPLL `y` is reported by default

Input:

```mml
9 y14,32 c4
```

Expected:

```yaml
register_write_usage:
  - family: opll
    register: 14
    data: 32
    policy: ignored
smf_events_from_y: 0
```

### 21.6 OPLL `y` rule emits CC

Config:

```yaml
opll_register_map:
  rules:
    - name: y2_to_expr
      enabled: true
      match: { family: opll, register: 2, data: "*", context: "*" }
      emit:
        - cc: { controller: 11, value: "expr: clamp(127 - round(data * 127 / 63), 0, 127)" }
```

Input:

```mml
9 y2,0 c4
```

Expected:

```yaml
events_before_first_note:
  - cc: { controller: 11, value: 127 }
```

### 21.7 config skeleton includes used commands

Input:

```mml
@e20 = {1,0,f:2,8=4}
@e21 = {,,@13f}
3 @20 c8
4 @12 d8
4 @e21 f8
9 y14,32 e8
```

Expected skeleton contains:

```yaml
psg_envelope_map.envelopes."20"
tone_map.scc.tones."12"
tone_map.scc.tones."13"
opll_register_map.observed[register=14,data=32]
```

### 21.8 OPLL pseudo drum map replaces macro notes

Config:

```yaml
tracks:
  "h": { source_family: opll, render_role: drum, drum_map: grouped, midi_channel: 9 }
opll_pseudo_drum_map:
  enabled: true
  maps:
    grouped:
      output: { midi_channel: 9, mode: replace, suppress_tone_map: true }
      hit_grouping: { enabled: true, suppress_continuations: [macro_continuation] }
      rules:
        - { match: { macro_symbol: "b" }, drum: { note: 36, velocity: source_volume } }
```

Input:

```mml
*1 = { @15 c16 d16 }
#macro_offset { b = 0 }
h *b01
```

Expected:

```yaml
midi_channel: 9
note_on:
  - { note: 36 }
program_change_from_opll_at15: false
source_notes_c_d: false
```

---

## 22. Recommended default config

実務上の既定は以下。

```yaml
manual_smf.enabled: true
psg_envelope_map.enabled: true
psg_noise_map.drum_map.enabled: false
opll_pseudo_drum_map.enabled: false
tone_map.scc.enabled: true
tone_map.opll.enabled: true
opll_register_map.default_policy: ignore_and_report
envelope_map.default_mode: cc11
smf.channel_allocation.policy: one_source_track_per_channel
```

これにより、既存MUS資産のタイミング・ノート・エンベロープ移植を維持しつつ、音色やOPLLコマンドの移植作業をYAML編集に集約できる。
