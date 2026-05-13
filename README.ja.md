# mgsdrv-mus-to-smf

[English README](README.md)

MGSDRV/MGSC111 の `.MUS` MML テキストを Standard MIDI File Type 1 へ変換するツールです。

```bash
cargo run -- input.mus -o output.mid
```

生成されるバイナリ名は `mgs2smf` です。

ドキュメント:

- [`.smfmap.yaml` 使い方ガイド](docs/smfmap.ja.md)
- [`.smfmap.yaml` Guide](docs/smfmap.en.md)
- [Release Notes](CHANGELOG.md)
- [合成サンプル](examples/README.md)

## 使い方

```bash
mgs2smf input.mus -o output.mid \
  --ppq 3600 \
  --loop-count 2 \
  --loop-marker-name loopStart:loopEnd \
  --envelope-mode cc11 \
  --tone-policy ignore \
  --rhythm-map gm \
  --channel-overflow error \
  --encoding auto \
  --octave-base o4c=60 \
  --config song.smfmap.yaml \
  --emit-config-skeleton observed.smfmap.yaml \
  --diagnostics output.json
```

`.smfmap.yaml` を使うと、SCC/OPLL tone、PSG envelope、PSG noise drum、OPLL pseudo drum、pitch glide などを曲ごとに調整できます。まずは `--emit-config-skeleton` で曲中に観測された設定の下書きを出力し、それを編集する運用を想定しています。

## 主な対応内容

- UTF-8 / Shift-JIS 読み込み、CRLF/LF 正規化、`;` コメント処理。
- `#opll_mode`、`#tempo`、`#title`、`#macro_offset`、`#play_track`、`#end`。
- `@e` / `@r` envelope 定義、テンポ変更を考慮した 60 Hz CC 近似、SMF化できない envelope command の IR/diagnostics 保持。
- `@mN={...}` control text 定義と `@mN` MIDI text meta 出力。
- `@s` / `@v` tone 定義の診断付き保持、および OPLL `@#N = M` の tone-map indirection。
- source track `1..9`、`a..h`、`r`。複数 selector 行の展開。
- note、rest、octave、`o4c` 指定、default length、`%N`、付点、tie、slur、`t`、`v`、相対 volume、`q`、`@N`、`@eN`、`@rN`、loop、`!`。
- `[0 ...]` infinite loop の有限展開。MSXPlay 形式の header comment は、`loop=N` または `--loop-count N` がない場合 2 global loops として扱います。
- `#opll_mode 1` の rhythm track `f` / `r` を GM drum note へ変換。
- `.smfmap.yaml` 読み込み。`--config`、`.smfmap.yaml`、`*.smfmap.yaml` を利用できます。
- `;@smf` manual SMF directive。`pc`、`bank`、`cc`、`pb`、`rpn`、`nrpn`、`marker`、`text`、`macro`、`reset` に対応。
- v0.4 family-aware `@N` dispatch。PSG/PSG noise では envelope 選択、SCC/OPLL では `tone_map` / envelope として扱います。
- PSG noise drum map、SCC/OPLL envelope map、`yR,D` register write diagnostics / mapping hook。
- v0.4 OPLL pseudo drum map。`#opll_mode 0` の通常 OPLL/FM track を明示設定で GM drum 化できます。
- v0.4.1 `pitch_glide`。MGSC111 `_target` pitch glide を opt-in で MIDI Pitch Bend に変換できます。
- `--emit-config-skeleton` による observed YAML skeleton 生成。
- JSON diagnostics。warnings、unsupported commands、channel allocation、loop markers、track summaries、envelope event counts などを出力します。

## 互換性確認

互換性確認用の SMF は以下で生成できます。

```bash
bash scripts/validate_compat.sh
```

`tests/compat/*.mus` を `target/compat-smf/*.mid` と diagnostics JSON に変換します。DAW や MIDI player での確認に使ってください。

## リリース自動化

CI は pull request と `main` / `master` への push で実行されます。`cargo fmt --check`、`cargo clippy -- -D warnings`、`cargo test --locked`、互換性確認用 SMF の生成を確認します。

`v0.1.0` のようなタグを push すると、Linux、macOS Intel、Windows 向けの release binary をビルドし、GitHub Release 名 `0.1.0` として packaged archive を添付します。

## サンプルについて

`examples/placeholder.*` は、このリポジトリ用に作成した合成サンプルです。既存曲の変換例ではなく、`.smfmap.yaml` の構文確認とリリース梱包用のプレースホルダーです。
