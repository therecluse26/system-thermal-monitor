#!/usr/bin/env bash
# Writes the release notes in Markdown: the commits since the previous release, grouped by
# Conventional Commits type when the subjects use one, then how to install each package.
# Usage: changelog.sh <previous tag, or "" for the first release> <new tag>
# Needs GITHUB_REPOSITORY (owner/repo) for the links; GitHub Actions sets it.
set -euo pipefail

previous=$1
tag=$2
version=${tag#v}
url="${GITHUB_SERVER_URL:-https://github.com}/${GITHUB_REPOSITORY:?set GITHUB_REPOSITORY to owner/repo}"

breaking=() features=() fixes=() other=()
subject_re='^([a-z]+)(\(([^)]*)\))?(!)?: (.+)$'

while IFS=$'\t' read -r sha subject; do
  type="" scope="" bang="" text=$subject
  if [[ $subject =~ $subject_re ]]; then
    type=${BASH_REMATCH[1]} scope=${BASH_REMATCH[3]} bang=${BASH_REMATCH[4]} text=${BASH_REMATCH[5]}
  fi
  line="- ${scope:+**$scope:** }$text ([${sha:0:7}]($url/commit/$sha))"
  body=$(git log -1 --format=%b "$sha")
  if [[ -n $bang ]] || grep -qE '^BREAKING[ -]CHANGE:' <<<"$body"; then
    breaking+=("$line")
  elif [[ $type == feat ]]; then
    features+=("$line")
  elif [[ $type == fix ]]; then
    fixes+=("$line")
  else
    other+=("$line")
  fi
done < <(git log --no-merges --format=$'%H\t%s' ${previous:+"$previous..HEAD"})

section() {
  local title=$1
  shift
  (($#)) || return 0
  printf '### %s\n\n' "$title"
  printf '%s\n' "$@"
  printf '\n'
}

echo "## What's changed"
echo
if ((${#breaking[@]} + ${#features[@]} + ${#fixes[@]} == 0)); then
  printf '%s\n' "${other[@]}"
  echo
else
  section "Breaking changes" "${breaking[@]}"
  section "New features" "${features[@]}"
  section "Fixes" "${fixes[@]}"
  section "Other changes" "${other[@]}"
fi

if [[ -n $previous ]]; then
  echo "**All changes:** $url/compare/$previous...$tag"
else
  echo "**All changes:** $url/commits/$tag"
fi

cat <<EOF

## Install

| Package | For | Command |
|---|---|---|
| \`.deb\` | Debian, Ubuntu, Linux Mint, Pop!_OS | \`sudo apt install ./thermal-monitor_${version}_amd64.deb\` |
| \`.rpm\` | Fedora, RHEL, openSUSE | \`sudo dnf install ./thermal-monitor-${version}-1.x86_64.rpm\` |
| AppImage | Any distribution | \`chmod +x thermal-monitor_${version}_amd64.AppImage\`, then run it |

On NixOS, run the AppImage with \`appimage-run\`.
To check a download: \`sha256sum --check --ignore-missing SHA256SUMS\`.
EOF
