# `.smfmap.yaml` 使い方ガイド

この文書は、`mgs2smf` の v0.3 `smfmap` 設定ファイルの使い方を説明します。

`smfmap` は曲ごとの変換設定です。MUS ファイルは MGSDRV/MGSC111 で読める形のままにして、MIDI 出力だけを補正します。たとえば Program Change、CC、手動 MIDI イベント、未対応レジスタ書き込みの診断などを曲ごとに指定できます。

## 最初の使い方

まず MUS ファイルからスケルトンを生成します。

```bash
mgs2smf song.mus --emit-config-skeleton song.smfmap.yaml
```

生成された YAML を編集し、次にその設定で変換します。

```bash
mgs2smf song.mus -o song.mid --config song.smfmap.yaml
```

スケルトン生成と SMF 出力を同時に行うこともできます。

```bash
mgs2smf song.mus -o song.mid --emit-config-skeleton observed.smfmap.yaml
```

## 設定ファイルの読み込み

変換器はまず組み込み既定値を使います。その後、設定ファイルを読み込んで上書きします。

1. `--config <path>` がある場合は、指定順に読み込みます。
2. `--config` がない場合は、`.smfmap.yaml`、入力ファイルと同じディレクトリの `.smfmap.yaml`、`song.smfmap.yaml` を探します。

後から読まれた設定が優先されます。

例:

```bash
mgs2smf song.mus -o song.mid \
  --config common.smfmap.yaml \
  --config song.smfmap.yaml
```

## おすすめの作業手順

1. `--emit-config-skeleton` で、実際に使われている番号だけを出力します。
2. 出力された tone、envelope、register write だけを編集します。
3. `--config` で変換します。
4. `--diagnostics` の JSON で未設定や無視された項目を確認します。

```bash
mgs2smf song.mus --emit-config-skeleton song.smfmap.yaml
mgs2smf song.mus -o song.mid --config song.smfmap.yaml --diagnostics song.json
```

スケルトンには、MUS 内で実際に観測されたものだけが入ります。

- PSG の `@N`、`@eN`、`@rN` envelope 番号
- SCC の `@N` tone 番号
- OPLL の `@N` tone 番号
- `yR,D` レジスタ書き込み

## SMF 設定

`smf` では、ファイル全体の MIDI 設定を指定できます。`--ppq`、`--channel-overflow`、`--program-numbering`、`--channel-numbering` などの CLI オプションがある場合は、CLI 側が優先されます。

```yaml
smf:
  type: 1
  ppq: 3600
  program_numbering: zero_based
  channel_numbering: zero_based
  channel_allocation:
    overflow_policy: error
    rhythm_channel_zero_based: 9
```

## トラック family

`@N`、`@eN`、`@rN` は、トラックの family によって意味が変わります。

```yaml
tracks:
  "1": { family: psg,       midi_channel: 0  }
  "2": { family: psg,       midi_channel: 1  }
  "3": { family: psg_noise, midi_channel: 2  }
  "4": { family: scc,       midi_channel: 3  }
  "9": { family: opll,      midi_channel: 8  }
```

既定の動作:

- `psg`: `@N`、`@eN`、`@rN` は PSG envelope 選択です。
- `psg_noise`: 既定では PSG と同じく envelope 選択です。ドラム変換は任意で、既定では無効です。
- `scc`: `@N` は tone map を参照します。`@eN` と `@rN` は envelope 選択です。
- `opll`: `@N` は tone map を参照します。`@eN` と `@rN` は envelope 選択です。
- `rhythm`: `@` コマンドは無視され、診断に出ます。

重要: PSG の `@N` は `tone_map` に流しません。そのため PSG の `@N` から Program Change は出ません。

## 手動 SMF directive

MUS ファイル内に `;@smf` コメントを書くと、MGSDRV/MGSC111 互換を保ったまま MIDI/メタイベントを追加できます。

```mml
9 ;@smf pc 80
9 ;@smf cc 74 100
9 c4 d4
```

トラック行の中に書いた `;@smf` は、既定で現在の source track を対象にします。

トップレベルに書く場合は、通常は `track=<id>` または `conductor` を指定します。

```mml
;@smf track=9 pc 80
;@smf conductor marker "A section"
```

`channel=<n>` を使うと、MIDI チャンネルを直接指定できます。トップレベルの channel 指定 directive は conductor track に書き込まれます。`track=<id>` と組み合わせると、イベントを書き込む SMF トラックは source track 側にしつつ、送信チャンネルだけを変えられます。

```mml
;@smf channel=3 pc 80
;@smf track=9 channel=3 cc 74 100
```

対応コマンド:

```text
pc <program>
bank <msb> <lsb>
cc <controller> <value>
pb <value>
rpn <msb> <lsb> <value>
nrpn <msb> <lsb> <value>
marker "text"
text "text"
macro <name>
reset [name]
```

数値は 10 進数、`0x` 16 進数、`$` 16 進数、`%` 2 進数を使えます。

`smf.program_numbering` と `smf.channel_numbering` により、YAML と手動 directive の program/channel 番号を zero-based として読むか one-based として読むかを指定できます。

例:

```mml
9 ;@smf bank 0 32
9 ;@smf pc 0x50
9 ;@smf cc $4a 100
9 ;@smf pb -1200
9 ;@smf text "OPLL custom tone starts here"
```

## SMF macro

macro は、複数の SMF イベントに名前を付けたものです。チャンネル初期化や音色補正を何度も使う場合に便利です。

```yaml
smf_macros:
  bright_lead:
    events:
      - pc: { program: 80 }
      - cc: { controller: 7, value: 110 }
      - cc: { controller: 11, value: 127 }
      - cc: { controller: 74, value: 105 }
```

MUS 側から呼び出します。

```mml
9 ;@smf macro bright_lead
9 c4
```

`reset` は既定で `reset_channel` macro を展開します。

```mml
9 ;@smf reset
```

## SCC / OPLL tone map

`tone_map` は、SCC/OPLL の `@N` を MIDI イベントへ変換するための設定です。

```yaml
tone_map:
  scc:
    enabled: true
    tones:
      "1":
        name: scc_lead
        events:
          - pc: { program: 81 }
          - cc: { controller: 74, value: 96 }

  opll:
    enabled: true
    tones:
      "9":
        name: synth_lead
        events:
          - bank: { msb: 0, lsb: 0 }
          - pc: { program: 80 }
          - cc: { controller: 91, value: 24 }
```

MUS 側:

```mml
4 @1 c4
9 @9 c4
```

既存のスケルトンや手書き設定では、YAML の inline map 形式が使われることがあります。

```yaml
tone_map:
  scc:
    enabled: true
    tones:
      "8":  { name: "SCC @8",  events: [] }
      "16": { name: "SCC @16", events: [] }
      "24": { name: "SCC @24", events: [] }
```

これは、その曲で SCC の `@8`、`@16`、`@24` が使われているが、まだ MIDI イベントは割り当てていない、という意味です。`events: []` は未設定のプレースホルダです。

上の inline 形式は、次の block 形式と同じ意味です。

```yaml
tone_map:
  scc:
    enabled: true
    tones:
      "8":
        name: "SCC @8"
        events: []
```

`events` を編集するときは、下のような block 形式を推奨します。`events` を複数行にする場合、周囲の `{ ... }` は残さないでください。

```yaml
tone_map:
  scc:
    enabled: true
    tones:
      "8":
        name: "SCC @8 lead"
        events:
          - pc: { program: 81 }
          - cc: { controller: 74, value: 96 }
      "16":
        name: "SCC @16 bass"
        events:
          - pc: { program: 38 }
```

同じ tick のイベント順は v0.3 の方針に従います。NoteOff、Bank、Program、RPN/NRPN、CC、PitchBend、初期 expression、meta、NoteOn、envelope curve の順です。

`respect_at_hash_rom_assign` が true の場合、OPLL の `@#N = M` assignment を反映します。

```mml
@#2 = 14
9 @2 c4
```

```yaml
tone_map:
  opll:
    respect_at_hash_rom_assign: true
```

この例では、MUS の `@2` が `tone_map.opll.tones["14"]` を参照します。

## PSG envelope map

PSG / PSG noise トラックでは、`@N`、`@eN`、`@rN` は envelope 選択です。選択された envelope は各 Note On で先頭から再生され、既定では expression CC として出力されます。

```mml
@e1 = {1,0,f:2,8=4,[84]}
1 @1 c4 d4
```

この場合、PSG トラックに CC #11 の envelope curve が出ます。Program Change は出ません。

関連する設定:

```yaml
psg_envelope_map:
  enabled: true
  undefined_policy: warn_and_no_envelope
  default:
    emit_expression_cc: true
    expression_cc: 11
    trigger:
      on_at_command: select_only
      on_note_on: restart_envelope
```

## PSG noise drum map

PSG noise は既定では envelope CC として出力します。スキーマ上はドラム変換も用意されていますが、既定では無効です。

```yaml
psg_noise_map:
  enabled: true
  default_action: envelope_cc
  drum_map:
    enabled: false
```

特定の envelope 番号を melody note ではなく GM drum note にしたい場合は、drum map を有効にします。

```yaml
psg_noise_map:
  default_action: drum_map
  drum_map:
    enabled: true
    channel: 9
    envelopes:
      "21":
        name: noise_snare
        note: 38
        velocity: 100
```

## SCC / OPLL envelope map

SCC/OPLL では、`@eN` と `@rN` が以後の note に使う envelope を選びます。既定では CC #11 として出力しますが、`envelope_map` で CC #7、note velocity、split velocity、off を選べます。

```yaml
envelope_map:
  enabled: true
  default_mode: cc11
  midi_cc:
    expression: 11
    volume: 7
  families:
    scc: { enabled: true }
    opll: { enabled: true }
```

## レジスタ書き込み

MUS の `yR,D` は RegisterWrite IR として保持され、既定では diagnostics に報告されます。

```mml
9 y14,32 c4
```

既定設定:

```yaml
opll_register_map:
  enabled: true
  default_policy: ignore_and_report
```

既定では MIDI イベントは出ません。チップ固有の動作を見える形で残し、必要に応じて `;@smf`、text event、または register rule へ置き換えるためのものです。

一致した register write から MIDI を出す場合は、`default_policy: apply_rules` または `--register-map on` を使います。

```yaml
opll_register_map:
  default_policy: apply_rules
  rules:
    - name: y14_filter
      enabled: true
      match: { family: opll, source_track: "*", register: 14, data: 32, context: "*" }
      emit:
        - cc: { controller: 74, value: 100 }
```

`@e` 定義内の register write は `context: EnvelopeData` として rule に渡されます。

## diagnostics

設定がどう適用されたか確認するには diagnostics JSON を出します。

```bash
mgs2smf song.mus -o song.mid --config song.smfmap.yaml --diagnostics song.json
```

diagnostics には次の情報が入ります。

- 読み込んだ config ファイル
- 手動 SMF イベント
- tone 使用状況
- PSG envelope 使用状況
- register write 使用状況
- unmapped tone
- unsupported command と warning

## CLI override

v0.3 関連の主なオプション:

```text
--config <path>
--emit-config-skeleton <path>
--manual-smf <on|off>
--tone-map <on|off>
--psg-envelope-map <on|off>
--psg-noise-drum-map <on|off>
--register-map <off|report|on|error>
--program-numbering <zero|one>
--channel-numbering <zero|one>
```

`--register-map report` が既定の動作です。`on` は `apply_rules` を有効にし、一致する rule がない write だけを報告します。

## 最小例

`song.mus`:

```mml
@e1 = {1,0,f:2,8}
1 @1 c4
4 @1 c4
9 ;@smf macro bright_lead
9 @9 c4
9 y14,32 c4
```

`song.smfmap.yaml`:

```yaml
version: 0.3

tone_map:
  scc:
    tones:
      "1":
        events:
          - pc: { program: 81 }
  opll:
    tones:
      "9":
        events:
          - pc: { program: 80 }

smf_macros:
  bright_lead:
    events:
      - cc: { controller: 74, value: 105 }
      - cc: { controller: 91, value: 24 }
```

変換:

```bash
mgs2smf song.mus -o song.mid --config song.smfmap.yaml --diagnostics song.json
```
