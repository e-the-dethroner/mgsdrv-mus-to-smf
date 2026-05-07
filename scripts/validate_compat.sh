#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
out_dir="${1:-"$repo_root/target/compat-smf"}"

mkdir -p "$out_dir"
cargo build --quiet --manifest-path "$repo_root/Cargo.toml"

for src in "$repo_root"/tests/compat/*.mus; do
  name="$(basename "$src" .mus)"
  mid="$out_dir/$name.mid"
  json="$out_dir/$name.json"
  args=(
    "$repo_root/target/debug/mgs2smf"
    "$src"
    -o "$mid"
    --diagnostics "$json"
    --ppq 3600
  )

  case "$name" in
    loop_marker)
      args+=(--loop-count 2)
      ;;
    multi_port_overflow)
      args+=(--channel-overflow multi_port)
      ;;
    tone_text_meta)
      args+=(--tone-policy text_meta)
      ;;
  esac

  "${args[@]}"
  test -s "$mid"
  test -s "$json"
  printf 'wrote %s and %s\n' "$mid" "$json"
done

printf 'compat SMF outputs written to %s\n' "$out_dir"
