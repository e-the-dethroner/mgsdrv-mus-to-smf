# MGSDRV / MGSC111 MUS → SMF 変換器 実装計画

## 1. 目的

MGSC111.TXT 仕様で書かれた MGSDRV 用 `.MUS` テキスト資産を、スタンダード MIDI ファイル（SMF）へ変換する。

主目的は以下の 3 点。

1. 既存 MUS 資産を DAW / MIDI プレイヤーで扱えるようにする。
2. 音高・発音タイミング・休符・テンポ・トラック分離をできるだけ正確に保持する。
3. `@e` / `@r` エンベロープを MIDI の Expression / Volume / Velocity 操作へ近似移植する。

音色そのもの、SCC 波形、OPLL カスタム音色、PSG ノイズ音色など、SMF に標準的な等価表現がないものは移植対象外とする。

---

## 2. 非目標

以下は初期実装では移植しない。

- `@s` SCC 波形定義の再現。
- `@v` OPLL カスタム音色定義の再現。
- `@#` による ROM 音色番号割り当ての完全再現。
- PSG ノイズ周波数・トーン/ノイズモードの音響再現。
- OPLL レジスタ直書き `y` の完全再現。
- MGSDRV 固有フェード、制御文字列、MIB 連携。
- SCC / OPLL / PSG の実機音色エミュレーション。

ただし、これらは診断ログまたは MIDI text meta event として残せるようにする。

---

## 3. 推奨実装言語

推奨は **Rust**。

理由:

- 大量 MUS 資産の一括変換に向く。
- パーサ・IR・SMF ライタを型安全に書ける。
- `midly` 等で SMF を高速に生成できる。
- CLI ツールとして配布しやすい。

候補クレート:

- `midly`: SMF 書き出し。
- `winnow` または `nom`: tokenizer / parser。
- `encoding_rs`: Shift-JIS / UTF-8 読み込み。
- `serde`, `serde_yaml`, `toml`: 設定ファイル。
- `clap`: CLI。
- `anyhow`, `thiserror`: エラー処理。

---

## 4. 全体アーキテクチャ

```text
.MUS text
  ↓
Source loader
  - UTF-8 / Shift-JIS 判定
  - CRLF / LF 正規化
  - コメント除去
  ↓
Top-level parser
  - #directive
  - macro definition
  - @e / @r / @s / @v tone definition
  - track line
  ↓
Macro expander
  - *n / macro_offset
  - recursion depth guard
  ↓
Track MML parser
  - notes / rests / lengths
  - octave / volume / q / @ / @e
  - loops / alternatives
  - rhythm track tokens
  ↓
Intermediate Representation, IR
  - absolute source step position
  - source track id
  - tempo map candidates
  - envelope definitions
  - note/rest/control events
  ↓
Semantic resolver
  - global tempo map
  - channel allocation
  - @e/@r attachment
  - loop handling
  - unsupported command diagnostics
  ↓
SMF renderer
  - Type 1
  - conductor track
  - per-source-track MIDI track
  - notes, CC, program change, markers
  ↓
.mid + .json diagnostic report
```

---

## 5. SMF 出力方針

### 5.1 ファイル形式

- 既定: SMF Type 1。
- Track 0: conductor track。
  - tempo meta events。
  - time signature, key signature は必要に応じて既定値。
  - title / loop markers。
- Track 1 以降: MGSDRV source track ごとの MIDI track。

### 5.2 PPQ

推奨 PPQ は **3600**。

理由:

- MGSC111 では 4分音符が `%48` steps。
- PPQ 3600 の場合、1 MGSC step = 3600 / 48 = 75 MIDI ticks で整数。
- `@e` の 1 count = 1/60 秒相当を MIDI tick に変換すると、BPM が整数なら `ticks_per_count = BPM` になり、丸め誤差が小さい。

小さいファイルを優先する場合は PPQ 480 / 960 も許可するが、`@e` CC カーブに丸め誤差が出やすくなる。

### 5.3 テンポ

- `#tempo n` と `t n` は MIDI Set Tempo meta event に変換する。
- MIDI tempo は `microseconds_per_quarter = 60_000_000 / n`。
- `t n` は MGSDRV 仕様上グローバル。トラック個別テンポとして扱わない。
- 複数トラックに同一 tick で同じテンポが出る場合は重複削除。
- 同一 tick で異なるテンポが出る場合は diagnostic error または warning。

### 5.4 タイミング

- source step → SMF tick:

```text
midi_tick = source_step * ppq / 48
```

- `%N` は N source steps。
- 通常音長 `4`, `8`, `16` などは whole note = 192 steps として解釈する。

```text
steps = 192 / length_denominator
```

- 付点は Fraction で計算する。
- 最終レンダリング時のみ整数 tick に丸める。
- 丸めはトラックごとに誤差拡散する。

---

## 6. 音符・休符・ゲート

### 6.1 音高

既定:

```text
o4 c = MIDI note 60
```

設定で `o4c = 48` などに変更可能にする。

### 6.2 note / rest

- `a`〜`g`, `+`, `-` を MIDI note number に変換。
- `r` は時刻だけ進める。
- `o`, `<`, `>` は source state を更新。
- `l` は既定音長を更新。

### 6.3 q

`q0..q8` は note duration 内の key-on 比率として扱う。

```text
on_steps  = total_steps * q / 8
off_steps = total_steps - on_steps
```

- 既定 `q8`。
- `q0` は FM のみ key-on 継続という MGSDRV 固有挙動だが、SMF では「次の明示 key-off まで伸ばす」または「unsupported warning」の選択式。
- 初期実装では `q0` を warning + `q8` 扱いにしてよい。

### 6.4 タイ `^` とスラー `&`

- `^length` は同一 note/rest の長さ加算。
- `&` は次音との legato / split-note merge として扱う。
- 同一ピッチで `&` が続く場合は 1 つの長い note に統合する。
- 異なるピッチで `&` が続く場合は、gap なしの note-off / note-on にする。必要なら overlap ticks を設定可能にする。

---

## 7. ループ

### 7.1 有限ループ

- `[n ... ]` と `[ ... ]n` は n 回展開する。
- 回数省略は 2 回。
- ネストは最大 16。
- `|` は最終ループ脱出として解釈する。

### 7.2 無限ループ `[0 ... ]`

SMF に標準ループ命令はないため、以下を既定とする。

- ループ body は 1 回だけ出力。
- loop start / loop end の marker meta event を出力。
- CLI `--loop-count N` が指定された場合は N 回展開して有限 SMF を出力。

推奨 marker 名:

```text
loopStart
loopEnd
```

DAW によって marker loop への対応は異なるため、音声化用途では `--loop-count` を使う。

---

## 8. 音量・ベロシティ・エンベロープ

### 8.1 基本 v

MGSDRV `v0..v15` は MIDI note-on velocity に変換する。

```text
velocity = round(v / 15 * 127)
```

`v+N`, `v-N`, `)N`, `(N` は現在 volume state を更新する。

### 8.2 @e の推奨変換

`@e` は MIDI Expression CC #11 へ変換する。

理由:

- MIDI velocity は note-on 時点の値であり、発音後に変化できない。
- `@e` は時間変化する音量包絡なので、CC #11 の方が近い。
- source track ごとに別 MIDI channel を割り当てることで、トラック単位の CC カーブが独立する。

変換:

```text
env_level_0_to_15 -> expression = round(env_level / 15 * 127)
```

有効音量は概念上以下。

```text
effective = note_velocity_from_v * expression_from_env / 127
```

### 8.3 @e の tokenizer

`@eN = { Mode, Noise, data... }`

対応する data command:

| source | 意味 | SMF 変換 |
|---|---|---|
| `0`〜`f` | 音量を設定し 1 count 待つ | CC #11 |
| `x:count` | x を count 保持 | CC #11 |
| `x=count` | 現在値から x へ count で線形変化 | 複数 CC #11 |
| `[` `]` | envelope loop | note duration まで展開 |
| `nN` | PSG noise frequency | ignore + diagnostic |
| `/N`, `*N` | PSG mode | ignore + diagnostic |
| `@N` | envelope 内 tone change | ignore + diagnostic |
| `yR,D` | register write | ignore + diagnostic |
| `\N` | frequency offset | optional pitch bend / default ignore |
| `.`, `,` | separator | ignore |

count は 1/60 秒単位として扱う。

### 8.4 @r の近似変換

`@rN = { Mode,Noise,AL,AR,DR,SL,SR,RR }` は 60Hz の離散 ADSR として CC #11 に変換する。

初期実装では以下の簡易モデルで十分。

1. key-on 時に `AL` を volume 0..255 としてセット。
2. 1/60 秒ごとに `AR` を加算し、255 到達後 decay へ。
3. `DR` で `SL` まで減算。
4. `SR` で note-off まで減算。
5. note-off 後の `RR` は optional。既定では note-off 後の CC は出さない。

`@e` と `@r` は同番号で後勝ちにする。

### 8.5 velocity 分割モード

DAW / 音源が CC #11 を期待通り解釈しない場合に備え、次の代替モードを用意する。

```text
envelope_mode = "split_velocity"
```

このモードでは envelope の変化点ごとに note を短く分割し、各 note-on velocity を変化させる。

注意:

- レガートや持続音が崩れる。
- ノート数が増える。
- 初期実装の既定にはしない。

---

## 9. 音色の扱い

### 9.1 既定

音色定義は移植しない。

- `@s`: parse して捨てる。
- `@v`: parse して捨てる。
- `@#`: parse して捨てる。
- `@N`: tone state として保持するが、SMF Program Change は出さない。

### 9.2 任意の GM Program Change

設定で `tone_policy = "gm_program"` の場合のみ、`@0..@14` を GM 近似 program に変換する。

例:

| MGSDRV tone | 名前 | GM program 0-based 例 |
|---:|---|---:|
| 0 | Violin | 40 |
| 1 | Guitar | 24 |
| 2 | Piano | 0 |
| 3 | Flute / Clarinet 系 | 73 または 71 |
| 4 | Clarinet | 71 |
| 5 | Oboe | 68 |
| 6 | Trumpet | 56 |
| 7 | Organ | 16 |
| 8 | Horn | 60 |
| 9 | Synthesizer | 80 |
| 10 | Harpsichord | 6 |
| 11 | Vibraphone | 11 |
| 12 | Synth Bass | 38 |
| 13 | Wood Bass | 32 |
| 14 | Electric Bass | 33 |

これは音色再現ではなく、MIDI 再生時の便宜的な近似である。

---

## 10. Rhythm track

`#opll_mode 1` の rhythm track `f` / `r` は GM drum channel 10 に変換する。

| MGSDRV | 意味 | GM drum note |
|---|---|---:|
| `b` | bass drum | 36 |
| `s` | snare | 38 |
| `m` | tom | 45 |
| `c` | cymbal | 49 |
| `h` | hi-hat | 42 |

- `v<instrument>N` は該当 drum note の velocity state に変換。
- `ko` / `kf` は diagnostic のみでよい。
- 複数発音区切り `;` はコメント記号と衝突しやすいので、最初は通常 MML 行の `;` をコメントとして扱い、rhythm-inline separator には対応を段階 2 に回す。

---

## 11. MIDI channel allocation

### 11.1 既定

1 source track = 1 MIDI channel を原則にする。

理由:

- `@e` を CC #11 で出すには、トラックごとの独立 channel が必要。

### 11.2 channel 不足

MGSDRV は最大で MIDI の 16 channel を超える場合がある。対応ポリシー:

1. `--midi-port-mode multi_port`
   - SMF MIDI Port meta event を使い、論理 port を増やす。
   - 互換性は DAW 依存。
2. `--channel-overflow shared`
   - channel を共有し、共有 channel では CC envelope を抑制する。
3. `--channel-overflow error`
   - 変換を失敗させる。

既定は `error`。

---

## 12. Unsupported command policy

SMF に等価表現がないものは移植しない。ただし silent drop は避け、diagnostic に残す。

| command | policy |
|---|---|
| `#psg_tune` | ignore + warning |
| `#opll_tune` | ignore + warning |
| `m`, `s` PSG hard envelope | ignore + warning |
| `n` PSG noise frequency | ignore + warning |
| `/N` PSG mode / FM forced keyoff | FM keyoff は可能なら note off、PSG mode は ignore |
| `p`, `h`, `@p`, `ho`, `hf`, `hi` LFO | optional pitch bend、既定 ignore |
| `_note` pitch glide | optional pitch bend、既定 ignore |
| `\N`, `@\N` detune | optional pitch bend、既定 ignore |
| `yR,D` register write | ignore + warning |
| `@l` FM total level | optional CC #7、既定 warning |
| `so`, `sf` FM sustain | optional CC #64、既定 warning |
| `@m`, `@o`, `@f`, `$` | text/meta or ignore |
| `!` | track parse termination |

---

## 13. CLI 仕様案

```bash
mgs2smf input.mus -o output.mid
```

主要 options:

```bash
--config config.yaml
--ppq 3600
--smf-type 1
--loop-count 1
--loop-marker-name loopStart:loopEnd
--envelope-mode cc11        # cc11 | cc7 | velocity | split_velocity | off
--tone-policy ignore        # ignore | gm_program | text_meta
--rhythm-map gm             # gm | off
--octave-base o4c=60
--encoding auto             # auto | utf-8 | shift_jis
--diagnostics output.json
--strict                    # warning を error 扱い
--channel-overflow error    # error | shared | multi_port
```

---

## 14. Diagnostic report

`--diagnostics output.json` で以下を出す。

```json
{
  "input": "song.mus",
  "output": "song.mid",
  "ppq": 3600,
  "tracks": [
    {
      "source_track": "4",
      "midi_track": 2,
      "midi_channel": 3,
      "source_steps": 6144,
      "note_count": 280,
      "cc_count": 920,
      "warnings": []
    }
  ],
  "tempo_map": [
    { "source_step": 0, "bpm": 120 }
  ],
  "unsupported_commands": [
    { "line": 42, "track": "4", "command": "@s", "policy": "ignored" }
  ],
  "loop_markers": [
    { "start_step": 0, "end_step": 6144, "kind": "infinite" }
  ]
}
```

---

## 15. 実装フェーズ

### Phase 1: MVP

対象:

- Shift-JIS / UTF-8 読み込み。
- コメント除去。
- `#opll_mode`, `#tempo`, `#title`, `#end`。
- track line `1..h`。
- `o`, `<`, `>`, `l`, `t`, `v`, `v+`, `v-`, `)`, `(`。
- `a..g`, `+`, `-`, `r`, `%N`, 付点、`^`, `&`。
- finite loop / infinite loop marker。
- SMF Type 1 note output。
- `@e` 定義 parse。
- `@e` を CC #11 で出力。

この段階で、普通の旋律資産と `@e` 音量包絡の多くが変換できる。

### Phase 2: MGSC111 grammar completion

対象:

- macro definition / expansion。
- `#macro_offset`。
- `@r` ADSR 近似。
- rhythm track。
- `|` loop alternative。
- `@m` text meta。
- unsupported command diagnostics の網羅。

### Phase 3: Quality / compatibility

対象:

- `--loop-count` による finite render。
- channel overflow policy。
- GM Program Change option。
- tempo changes inside tune。
- `@e` loop の長時間展開最適化。
- SMF を DAW / player で検証。

### Phase 4: Optional musical features

対象:

- pitch bend for detune / glide / LFO。
- PSG noise percussion heuristic。
- per-note expression 対応 MIDI 2.0 / MPE 風 mode。
- reference MGSDRV 演奏との比較支援。

---

## 16. テスト方針

### 16.1 Unit tests

- length parser。
- `%N`, dotted length, tie `^`。
- loop expansion。
- macro expansion。
- `@e` tokenizer。
- source step → MIDI tick mapping。
- tempo map seconds ↔ ticks conversion。

### 16.2 Golden tests

小さい MUS 入力に対して、期待する MIDI event list を比較する。

例:

```mml
#opll_mode 0
#tempo 120
1 o4 v15 l4 cdef
```

期待:

- PPQ 3600。
- c,d,e,f が各 3600 ticks 間隔。
- note velocity 127。

### 16.3 Envelope tests

```mml
@e0 = {1,0,f:2,8=4,[84]}
1 @e0 o4 v15 c1
```

期待:

- note-on at tick 0。
- CC #11 initial 127。
- `f:2` は 2 counts 保持。
- `8=4` は 4 counts で下降。
- loop は note duration まで展開。

### 16.4 Regression corpus

ユーザーの MUS 資産から以下を選ぶ。

- ループなし短曲。
- `[0 ... ]` 無限ループ曲。
- `@e` 多用曲。
- `@r` 使用曲。
- rhythm track 使用曲。
- tempo change 使用曲。
- SCC / OPLL / PSG を混在する曲。

---

## 17. Codex への実装指示の要点

1. 音色再現を実装しようとしない。
2. まず timing と note event を正確にする。
3. `@e` は velocity ではなく CC #11 を既定にする。
4. source track ごとに MIDI channel を分ける。
5. unsupported command は無視しても必ず diagnostic に残す。
6. `Fraction` / rational を使い、早い段階で float にしない。
7. SMF 出力前に IR event list をテスト可能にする。
8. 無限ループは marker + optional unroll。
9. `--strict` で曖昧変換を error にできるようにする。
10. 大量ファイル変換を想定し、CLI と batch mode を用意する。

---

## 18. 参照した仕様上の前提

- MGSC111 では MUS テキストを MGSC で MGS へコンパイルする。
- MGSC111 の MML では 4分音符が `%48`、1音の step 指定は最大 256。
- `t` は全トラックへ影響するグローバルテンポ。
- `@e` は 0..f の音量値、hold、linear change、loop を持つ envelope 定義。
- SMF に存在しない音色定義は移植しない。

