#!/usr/bin/env bash
# The resume eval: when an agent picks a board back up, how much of what it
# needs does each way of reading the board deliver, and at what size?
#
#   evals/resume/run.sh [ekko-binary]
#
# Reads the real boards on this machine -- the default board and every project
# -- and never writes to them.
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

targets=("default:$EKKO_HOME/storage/storage.json")
for dir in "$EKKO_HOME"/projects/*/; do
  [ -d "$dir" ] || continue
  targets+=("$(basename "$dir"):${dir}.ekko/storage/storage.json")
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

for target in "${targets[@]}"; do
  name="${target%%:*}"
  file="${target#*:}"
  [ -f "$file" ] || continue
  if [ "$name" = default ]; then scope=(); else scope=(--project "$name"); fi

  mapfile -t sets < <(truth "$file")
  doing="${sets[0]}" ready="${sets[1]}" why="${sets[2]}"

  echo
  echo "== $name: $(wc -w <<<"$doing") in progress, $(wc -w <<<"$ready") ready, $(wc -w <<<"$why") attached reasons"
  printf '   %-12s %9s %9s %8s %8s %8s\n' strategy bytes '~tokens' doing ready why

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
    # Coverage is read with colour codes stripped, sizes with them left in: an
    # id behind an escape sequence is still delivered, and the escape sequence
    # is still paid for. Without the strip, an environment that forces colour
    # scores every coloured view at zero.
    plain="$(printf '%s' "$out" | sed 's/\x1b\[[0-9;]*m//g')"
    # Two characters per token: the median measured from the usage the API
    # reported for real ekko outputs in agent sessions. An estimate, labelled.
    printf '   %-12s %9s %9s %8s %8s %8s\n' "$strategy" "$bytes" "$((bytes / 2))" \
      "$(covered "$plain" "$doing")" "$(covered "$plain" "$ready")" "$(covered "$plain" "$why")"
  done
done
