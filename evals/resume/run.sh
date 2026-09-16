#!/usr/bin/env bash
# The resume eval: when an agent picks a board back up, how much of what it
# needs does each way of reading the board deliver, and at what size?
#
#   evals/resume/run.sh [ekko-binary]
#
# Pass target/release/ekko to measure this checkout. The default is whatever
# `ekko` is on PATH -- the installed build, which may be an older revision.
#
# Reads the real boards on this machine and never writes to them, found where
# ekko keeps them since 7bf8191: the default board in $EKKO_HOME, read with
# --ekko-dir so that a project found from the current folder cannot stand in
# for it; every project `ekko init` registered in $EKKO_HOME/projects.json, in
# its own folder; and any legacy $EKKO_HOME/projects/<name>/ not adopted yet,
# which --project still reads.
#
# The ground truth is computed here, with jq, straight from storage.json and
# independently of ekko's own code. An eval that asked ekko what the right
# answer was could only ever agree with ekko.
#
# What an agent needs to resume, as three sets of ids:
#   doing  open tasks in progress
#   ready  open tasks with no open blocker, not in progress
#   why    notes attached to a task in either set -- the reasons
#
# An id counts as delivered when the output has a line that starts with it,
# the way the board prints items ("  93. ..."). Coverage of ids is necessary
# and not sufficient: it shows the facts arrived, not that an agent used them.
set -euo pipefail

EKKO="${1:-ekko}"
EKKO_HOME="${EKKO_HOME:-$HOME/.ekko}"

# Three parallel lists: what to call the board, how to point ekko at it, and
# the storage the ground truth is read from.
labels=("default board")
scopes=(ekko-dir)
files=("$EKKO_HOME/storage/storage.json")

if [ -f "$EKKO_HOME/projects.json" ]; then
  while IFS=$'\t' read -r name path; do
    labels+=("$name")
    scopes+=(project)
    files+=("$path/.ekko/storage/storage.json")
  done < <(jq -r '.projects[] | [.name, .path] | @tsv' "$EKKO_HOME/projects.json")
fi

for dir in "$EKKO_HOME"/projects/*/; do
  [ -d "$dir" ] || continue
  name="$(basename "$dir")"
  if printf '%s\n' "${labels[@]}" | grep -qxF -- "$name"; then continue; fi
  labels+=("$name")
  scopes+=(project)
  files+=("${dir}.ekko/storage/storage.json")
done

truth() {
  jq -r '
    [ .[] | select(.trashed == null and .stashed == null) ] as $v
    | (reduce $v[] as $i ({}; .[$i.uid // ("id" + ($i._id | tostring))] = $i)) as $by
    | def open($x): ($x._isTask and ($x.isComplete != true) and ($x.cancelled != true));
      [ $v[] | select(open(.) and .inProgress == true) ] as $doing
    | [ $v[] | select(open(.) and (.inProgress != true)
          and ([ (.blockedBy // [])[] | $by[.] | select(. != null and open(.)) ] | length == 0)) ] as $ready
    | ([ $doing[], $ready[] ] | map(.uid)) as $focus
    | [ $v[] | select((._isTask | not)
          and ((.attachedTo // .anchor) as $a | $a != null and ($focus | index($a) != null))) ] as $why
    | "\($doing | map(._id) | join(" "))", "\($ready | map(._id) | join(" "))", "\($why | map(._id) | join(" "))"
  ' "$1"
}

# "hits/total" for the ids in $2 against the output in $1.
covered() {
  local out="$1" hit=0 n=0 id
  for id in $2; do
    n=$((n + 1))
    if grep -qE "^[[:space:]]*${id}\.[[:space:]]" <<<"$out"; then hit=$((hit + 1)); fi
  done
  echo "$hit/$n"
}

for i in "${!labels[@]}"; do
  label="${labels[$i]}"
  file="${files[$i]}"
  [ -f "$file" ] || continue
  if [ "${scopes[$i]}" = ekko-dir ]; then scope=(--ekko-dir "$EKKO_HOME"); else scope=(--project "$label"); fi

  mapfile -t sets < <(truth "$file")
  doing="${sets[0]}" ready="${sets[1]}" why="${sets[2]}"

  echo
  echo "== $label: $(wc -w <<<"$doing") in progress, $(wc -w <<<"$ready") ready, $(wc -w <<<"$why") attached reasons"
  printf '   %-12s %9s %9s %9s %8s %8s %8s\n' strategy bytes chars '~tokens' doing ready why

  for strategy in board board-head60 list-ready prime; do
    case "$strategy" in
      board)        out="$("$EKKO" "${scope[@]}" 2>/dev/null || true)" ;;
      board-head60) out="$("$EKKO" "${scope[@]}" 2>/dev/null | head -60 || true)" ;;
      list-ready)   out="$("$EKKO" "${scope[@]}" --list ready 2>/dev/null || true)" ;;
      prime)        out="$("$EKKO" "${scope[@]}" --prime 2>/dev/null || true)" ;;
    esac
    if [ -z "$out" ]; then
      printf '   %-12s %9s\n' "$strategy" "n/a"
      continue
    fi
    bytes=$(printf '%s' "$out" | wc -c)
    # Characters as well as bytes: Claude Code cuts hook output at 10,000
    # characters, and a prime past that arrives as a short preview instead.
    chars=$(printf '%s' "$out" | LC_ALL=C.UTF-8 wc -m)
    # Coverage is read with colour codes stripped, sizes with them left in: an
    # id behind an escape sequence is still delivered, and the escape sequence
    # is still paid for. Without the strip, an environment that forces colour
    # scores every coloured view at zero.
    plain="$(printf '%s' "$out" | sed 's/\x1b\[[0-9;]*m//g')"
    # Two characters per token: the median measured from the usage the API
    # reported for real ekko outputs in agent sessions. An estimate, labelled.
    printf '   %-12s %9s %9s %9s %8s %8s %8s\n' "$strategy" "$bytes" "$chars" "$((bytes / 2))" \
      "$(covered "$plain" "$doing")" "$(covered "$plain" "$ready")" "$(covered "$plain" "$why")"
  done
done
