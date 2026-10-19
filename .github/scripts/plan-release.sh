#!/usr/bin/env bash
# Chooses the next release from the commits since the last release tag (vX.Y.Z).
# Prints key=value lines for $GITHUB_OUTPUT: release, version, tag, previous.
#
# The bump follows Conventional Commits where the commit subjects use them:
#   "feat: ..." or "feat(scope): ..."                -> minor
#   "fix!: ..." (any type with !) or "BREAKING CHANGE:" -> major
#   anything else, including plain messages          -> patch
# The first release uses the version in src-tauri/tauri.conf.json.
# Later, that version wins when it is higher: raise it there to jump ahead.
set -euo pipefail

configured=$(jq -r .version src-tauri/tauri.conf.json)
previous=$(git describe --tags --abbrev=0 --match 'v[0-9]*.[0-9]*.[0-9]*' 2>/dev/null || true)

if [[ -z $previous ]]; then
  next=$configured
else
  if [[ -z $(git rev-list "$previous..HEAD") ]]; then
    echo "Nothing to release: no commits since $previous." >&2
    echo "release=false"
    exit 0
  fi
  IFS=. read -r major minor patch <<<"${previous#v}"
  subjects=$(git log --no-merges --format=%s "$previous..HEAD")
  bodies=$(git log --no-merges --format=%b "$previous..HEAD")
  if grep -qE '^[a-z]+(\([^)]*\))?!:' <<<"$subjects" || grep -qE '^BREAKING[ -]CHANGE:' <<<"$bodies"; then
    next="$((major + 1)).0.0"
  elif grep -qE '^feat(\([^)]*\))?:' <<<"$subjects"; then
    next="$major.$((minor + 1)).0"
  else
    next="$major.$minor.$((patch + 1))"
  fi
  next=$(printf '%s\n' "$next" "$configured" | sort -V | tail -n 1)
fi

tag="v$next"
if git rev-parse -q --verify "refs/tags/$tag" >/dev/null; then
  echo "Tag $tag exists, but this commit does not descend from it. Raise the version in src-tauri/tauri.conf.json." >&2
  exit 1
fi

echo "Next release: $tag (previous: ${previous:-none})" >&2
echo "release=true"
echo "version=$next"
echo "tag=$tag"
echo "previous=$previous"
