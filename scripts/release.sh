#!/usr/bin/env bash
# Cut a release of ekko: the whole procedure as one command that stops at the
# first step that fails. The version and the release notes are the only
# inputs that take judgment.
#
#   scripts/release.sh [--check] [--trailer 'Key: value']... X.Y.Z NOTES.md
#
# Run it from a tree at origin/main: main's own tree, pulled, or a worktree
# there. NOTES.md is the text of the GitHub release, and its opening
# paragraph, rewrapped, is the body of the release commit. --trailer adds a
# trailer to that commit, a Co-Authored-By say, and can be repeated.
#
# The steps, in order:
#   bump       the version in Cargo.toml and Cargo.lock, one line in each
#   test       cargo test
#   clippy     cargo clippy, with warnings as errors
#   build      a release build, which has to report the new version
#   semantics  evals/agent/semantics.py, every row fixed
#   scale      evals/agent/scale.py, every prime within 6,000 characters
#   commit     'release: vX.Y.Z', holding the bump and nothing else
#   push       to main, as a fast-forward
#   ci         the CI run of that commit, watched until it ends
#   tag        vX.Y.Z, annotated and signed, pushed
#   github     the GitHub release, from NOTES.md
#   nixos      the system flake's ekko input moved to the release commit,
#              and the system built, not switched to
# The evals run against the build this run made, not whatever
# target/release/ekko or PATH last held. What is left is the rebuild, which
# needs sudo, and restarting every Claude Code session: a server started
# before the rebuild keeps running the old binary.
#
# --check stops after the evals and puts Cargo.toml and Cargo.lock back, so a
# tree can be shown to be releasable before anyone decides to release it.
#
# After a failure, run the same command again. A run that fails before the
# push leaves no commit behind, and starts over. Once the release commit is on
# origin/main, a run resumes at ci, and skips a tag or a GitHub release that
# already exists.
#
# The terminal gets a line per step. Each step's output goes to a log of its
# own under target/release-logs/, and a failure prints the end of that log.
#
# EKKO_RELEASE_NIXOS is the system flake's folder (default ~/NixOS; set it
# empty to skip that step), and EKKO_RELEASE_HOST its nixosConfigurations
# name (default the hostname).
set -euo pipefail

usage() {
  echo "usage: scripts/release.sh [--check] [--trailer 'Key: value']... X.Y.Z NOTES.md" >&2
  echo "       scripts/release.sh --help says what it does" >&2
  exit 2
}

die() {
  echo "release: $*" >&2
  exit 1
}

check=0
trailers=()
args=()
while [ $# -gt 0 ]; do
  case $1 in
    --check) check=1 ;;
    --trailer)
      [ $# -ge 2 ] || usage
      trailers+=(--trailer "$2")
      shift
      ;;
    --trailer=*) trailers+=(--trailer "${1#--trailer=}") ;;
    -h | --help)
      sed -n '/^#!/d; /^#/!q; s/^# \{0,1\}//p' "$0"
      exit 0
      ;;
    -*) usage ;;
    *) args+=("$1") ;;
  esac
  shift
done
[ ${#args[@]} -eq 2 ] || usage

version=${args[0]#v}
tag=v$version
[[ $version =~ ^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]] ||
  die "'${args[0]}' is not a version: X.Y.Z"
notes=$(realpath -e -- "${args[1]}" 2>/dev/null) || die "no notes at ${args[1]}"

# The notes' opening paragraph becomes the commit's body, so they have to
# open with one.
lede=$(awk 'NF { started = 1 } started && !NF { done = 1 } started && !done' "$notes")
[ -n "$lede" ] || die "$notes is empty"
case $lede in
  '#'* | '```'* | '- '* | '* '*)
    die "$notes has to open with a paragraph, which becomes the release commit's body"
    ;;
esac

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)
cd "$root"
[ "$(git rev-parse --show-toplevel)" = "$root" ] || die "$root is not a git checkout"

for tool in gh nix jq flock fmt; do
  command -v "$tool" >/dev/null || die "$tool is not on PATH"
done
gh auth status --hostname github.com >/dev/null 2>&1 ||
  die "gh is not logged in to github.com: gh auth login"

repo=$(sed -n 's|^repository = "https://github.com/\([^"]*\)"$|\1|p' Cargo.toml)
[ -n "$repo" ] || die "Cargo.toml names no GitHub repository"

nixos_dir=${EKKO_RELEASE_NIXOS-$HOME/NixOS}

# The lock lives in the common git folder, which every worktree shares.
exec 9>"$(git rev-parse --git-common-dir)/ekko-release.lock"
flock -n 9 || die "another release is running from this repository"

git fetch --quiet origin main || die "git fetch origin main failed"

short() { git rev-parse --short "$1"; }

# The [package] version in Cargo.toml at a commit.
version_at() {
  git show "$1:Cargo.toml" |
    awk '/^\[/ { section = $0 }
      section == "[package]" && /^version = "/ && !seen { seen = 1; gsub(/^version = "|"$/, ""); print }'
}

fresh=0
pushed=0
start=
sha=
current=$(version_at origin/main)
if [ "$current" = "$version" ]; then
  # main is at this version already, so an earlier run pushed the release
  # commit, and this one resumes after it. awk reads to the end rather than
  # exit at the first match: under pipefail, git log killed by SIGPIPE would
  # fail the pipeline.
  sha=$(git log --format='%H%x09%s' origin/main |
    awk -F '\t' -v subject="release: $tag" '$2 == subject && !found { found = 1; print $1 }')
  [ -n "$sha" ] || die "main is at $version already, with no 'release: $tag' commit"
  [ "$check" = 0 ] || die "$tag is already released, as $(short "$sha")"
  echo "ekko $tag: its release commit $(short "$sha") is on origin/main, so this run resumes at ci"
elif [ "$(printf '%s\n' "$current" "$version" | sort -V | tail -n 1)" != "$version" ]; then
  die "$version does not come after $current, the version on main"
else
  fresh=1
  [ -z "$(git status --porcelain --untracked-files=no)" ] || die "the tree has changes: release a clean tree"
  start=$(git rev-parse HEAD)
  [ "$start" = "$(git rev-parse origin/main)" ] ||
    die "HEAD $(short HEAD) is not origin/main $(short origin/main): pull, or run from a tree at origin/main"
  if git rev-parse --quiet --verify "refs/tags/$tag" >/dev/null; then
    die "tag $tag already exists here, with no release commit on origin/main"
  fi
  [ -z "$(git ls-remote --tags origin "refs/tags/$tag")" ] ||
    die "tag $tag already exists on origin, with no release commit on main"
  if gh release view "$tag" --repo "$repo" >/dev/null 2>&1; then
    die "GitHub already has a release $tag"
  fi
  echo "ekko $tag, from $(short HEAD) at origin/main"
fi

logs=$root/target/release-logs/$tag-$(date +%Y%m%dT%H%M%S)
mkdir -p "$logs"
echo "logs in ${logs#"$root"/}"

# A run that stops before its push leaves the tree as it found it: the
# release commit undone, and the bump put back.
cleanup() {
  rm -f Cargo.toml.bump Cargo.lock.bump
  [ "$fresh" = 1 ] && [ "$pushed" = 0 ] || return 0
  if [ "$(git rev-parse HEAD)" != "$start" ]; then
    git reset --quiet --keep "$start"
    echo "the release commit is undone here"
  fi
  if [ -n "$(git status --porcelain --untracked-files=no -- Cargo.toml Cargo.lock)" ]; then
    git checkout --quiet -- Cargo.toml Cargo.lock
    echo "Cargo.toml and Cargo.lock are put back"
  fi
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

# step NAME WHAT COMMAND...: runs COMMAND with its output in NAME's log, and
# stops the release if it fails. COMMAND runs in a subshell, so set -e holds
# inside a function too, and writes a one-line summary to fd 3.
n=0
step() {
  local name=$1 what=$2 status began=$SECONDS summary
  shift 2
  n=$((n + 1))
  step_log=$(printf '%s/%02d-%s.log' "$logs" "$n" "$name")
  printf '%-10s %s: ' "$name" "$what"
  set +e
  (
    set -e
    "$@"
  ) 3>"$logs/summary" >"$step_log" 2>&1
  status=$?
  set -e
  summary=$(cat "$logs/summary")
  if [ "$status" -eq 0 ]; then
    echo "ok${summary:+, $summary} ($((SECONDS - began)) s)"
  else
    echo "FAILED ($((SECONDS - began)) s)"
    tail -n 40 "$step_log" | sed 's/^/    /'
    echo "    the whole log: ${step_log#"$root"/}"
    exit "$status"
  fi
}

dev() { nix develop --command "$@"; }

bump() {
  awk -v version="$version" '
    /^\[/ { section = $0 }
    section == "[package]" && /^version = "/ && !done { $0 = "version = \"" version "\""; done = 1 }
    { print }' Cargo.toml >Cargo.toml.bump
  awk -v version="$version" '
    /^\[\[package\]\]$/ { ekko = 0 }
    $0 == "name = \"ekko\"" { ekko = 1 }
    ekko && /^version = "/ { $0 = "version = \"" version "\""; ekko = 0 }
    { print }' Cargo.lock >Cargo.lock.bump
  cat Cargo.toml.bump >Cargo.toml
  cat Cargo.lock.bump >Cargo.lock
  rm Cargo.toml.bump Cargo.lock.bump
  git diff -- Cargo.toml Cargo.lock
  # One line in each file, or the edit caught a line it should not have.
  [ "$(git diff --numstat -- Cargo.toml Cargo.lock)" = "$(printf '1\t1\tCargo.lock\n1\t1\tCargo.toml')" ]
  echo "$current to $version" >&3
}

tests() {
  dev cargo test --locked
  awk '/^test result: ok\./ { passed += $4 } END { print passed " passed" }' "$step_log" >&3
}

clippy() {
  dev cargo clippy --locked --all-targets -- -D warnings
}

build() {
  dev cargo build --release --locked
  local target says
  target=$(dev cargo metadata --format-version 1 --no-deps --offline | jq -r .target_directory)
  bin=$target/release/ekko
  says=$("$bin" --version)
  [ "$says" = "$version" ] || {
    echo "$bin reports $says, not $version"
    return 1
  }
  echo "$bin" >"$logs/bin"
  echo "${bin#"$root"/}" >&3
}

semantics() {
  EKKO_BIN=$bin dev python3 evals/agent/semantics.py
  grep -o '[0-9]* of [0-9]* fixed' "$step_log" | tail -n 1 >&3
}

# scale.py reports sizes and leaves judging them to its reader, so the
# 6,000-character bound on a prime is checked here, against its results.
scale() {
  EKKO_BIN=$bin dev python3 evals/agent/scale.py
  local results
  results=$(sed -n 's/^results: //p' "$step_log" | tail -n 1)
  [ -f "$results" ] || {
    echo "scale.py named no results file"
    return 1
  }
  jq -e '[.rows[] | select((.prime_chars // 1e9) > 6000 or any(keys[]; endswith("_error")))] | length == 0' \
    "$results" >/dev/null || {
    echo "a prime over 6,000 characters, or a call that failed:"
    jq -c '.rows[] | {board, prime_chars} + with_entries(select(.key | endswith("_error")))' "$results"
    return 1
  }
  jq -r '"the largest prime \([.rows[].prime_chars] | max) characters"' "$results" >&3
}

commit() {
  local changed
  changed=$(git status --porcelain --untracked-files=no)
  [ "$changed" = "$(printf ' M Cargo.lock\n M Cargo.toml')" ] || {
    echo "the tree holds more than the bump:"
    echo "$changed"
    return 1
  }
  {
    printf 'release: %s\n\n' "$tag"
    printf '%s\n' "$lede" | fmt -w 72
  } >"$logs/message"
  git add -- Cargo.toml Cargo.lock
  git commit --quiet --file "$logs/message" "${trailers[@]}"
  git log -1 --format='%h %s' >&3
}

push() {
  git push origin "HEAD:refs/heads/main"
}

ci() {
  local run tries=0 conclusion
  until run=$(gh run list --repo "$repo" --commit "$sha" --workflow ci.yml --event push --limit 1 \
    --json databaseId --jq '.[0].databaseId // empty') && [ -n "$run" ]; do
    tries=$((tries + 1))
    [ "$tries" -le 36 ] || {
      echo "no CI run for $sha after three minutes"
      return 1
    }
    sleep 5
  done
  echo "https://github.com/$repo/actions/runs/$run"
  gh run watch "$run" --repo "$repo" --exit-status --compact --interval 20 || {
    gh run view "$run" --repo "$repo" --log-failed | tail -n 80
    return 1
  }
  conclusion=$(gh run view "$run" --repo "$repo" --json conclusion --jq .conclusion)
  [ "$conclusion" = success ] || {
    echo "run $run concluded $conclusion"
    return 1
  }
  echo "https://github.com/$repo/actions/runs/$run" >&3
}

tag() {
  local remote
  remote=$(git ls-remote origin "refs/tags/$tag^{}" | cut -f 1)
  if [ -n "$remote" ] && ! git rev-parse --quiet --verify "refs/tags/$tag" >/dev/null; then
    git fetch --quiet origin "refs/tags/$tag:refs/tags/$tag"
  fi
  if git rev-parse --quiet --verify "refs/tags/$tag" >/dev/null; then
    [ "$(git rev-parse "$tag^{commit}")" = "$sha" ] || {
      echo "tag $tag here is on $(git rev-parse "$tag^{commit}"), not on the release commit $sha"
      return 1
    }
  else
    git tag --annotate --sign --message "Ekko $tag" "$tag" "$sha"
  fi
  git verify-tag "$tag"
  if [ -z "$remote" ]; then
    git push origin "refs/tags/$tag"
  elif [ "$remote" != "$sha" ]; then
    echo "tag $tag on origin is on $remote, not on the release commit $sha"
    return 1
  fi
  echo "signed, on $(short "$sha")" >&3
}

github() {
  if ! gh release view "$tag" --repo "$repo" >/dev/null 2>&1; then
    gh release create "$tag" --repo "$repo" --verify-tag --title "Ekko $tag" --notes-file "$notes"
  fi
  gh release view "$tag" --repo "$repo" --json url --jq .url >&3
}

# The system flake follows main, so the update locks whatever main holds; if
# main has moved past the release commit, the old lock is put back.
nixos() {
  local lock=$nixos_dir/flake.lock token rev system
  token=$(gh auth token)
  cp -- "$lock" "$logs/flake.lock.before"
  (cd "$nixos_dir" && NIX_CONFIG="access-tokens = github.com=$token" nix flake update ekko)
  rev=$(jq -r .nodes.ekko.locked.rev "$lock")
  [ "$rev" = "$sha" ] || {
    cp -- "$logs/flake.lock.before" "$lock"
    echo "main has moved past the release: ekko would lock at $rev, not at $sha, so $lock is put back"
    return 1
  }
  system=$(cd "$nixos_dir" && NIX_CONFIG="access-tokens = github.com=$token" \
    nix build ".#nixosConfigurations.$host.config.system.build.toplevel" --no-link --print-out-paths)
  nix path-info --recursive "$system" |
    awk -v want="-ekko-$version" 'substr($0, length($0) - length(want) + 1) == want { print; found = 1 }
      END { exit !found }' || {
    echo "$system holds no ekko-$version"
    return 1
  }
  echo "$system" >&3
}

if [ "$fresh" = 1 ]; then
  step bump "the version in Cargo.toml and Cargo.lock" bump
  step test "cargo test" tests
  step clippy "cargo clippy" clippy
  step build "a release build" build
  bin=$(cat "$logs/bin")
  step semantics "evals/agent/semantics.py, against that build" semantics
  step scale "evals/agent/scale.py, against that build" scale
  if [ "$check" = 1 ]; then
    echo "$tag passes every check, and nothing is committed"
    exit 0
  fi
  step commit "the release commit" commit
  sha=$(git rev-parse HEAD)
  step push "to main" push
  pushed=1
fi
step ci "the CI run of $(short "$sha")" ci
step tag "$tag" tag
step github "the GitHub release" github
if [ -n "$nixos_dir" ] && [ -d "$nixos_dir" ]; then
  host=${EKKO_RELEASE_HOST:-$(hostname)}
  step nixos "$nixos_dir, the system built for $host" nixos
  left="rebuild the system from $nixos_dir, which needs sudo"
else
  echo "nixos      skipped: no system flake at '$nixos_dir'"
  left="install $tag"
fi
echo
echo "released $tag. Left: $left, then restart every Claude Code session."
