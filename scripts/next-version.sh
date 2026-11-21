#!/usr/bin/env bash
# Computes the next semantic version from Conventional Commits since the last
# release tag (vX.Y.Z).
#
#   feat:                         -> minor
#   fix: / perf:                  -> patch
#   type!: or BREAKING CHANGE:    -> major (minor while the version is 0.x)
#   anything else (chore, docs…)  -> no release
#   a `Release-As: X.Y.Z` line     -> exactly X.Y.Z, if it is newer (used
#                                    to leave 0.x for 1.0.0)
#
# Prints `version=X.Y.Z`, `tag=vX.Y.Z`, `bump=<level>` and `previous=<tag>`.
# When nothing is releasable, version/tag/bump are empty. If GITHUB_OUTPUT is
# set, the same lines are appended there for GitHub Actions.
#
# Usage: scripts/next-version.sh [REF]   (REF defaults to HEAD)
set -euo pipefail

ref="${1:-HEAD}"
previous="$(git describe --tags --abbrev=0 --match 'v[0-9]*.[0-9]*.[0-9]*' "$ref" 2>/dev/null || true)"

if [[ -n "$previous" ]]; then
  range="$previous..$ref"
  base="${previous#v}"
else
  range="$ref"
  base="0.0.0"
fi

IFS=. read -r major minor patch <<<"${base%%[-+]*}"

level=0 # 0 none, 1 patch, 2 minor, 3 major
type_re='^([a-zA-Z]+)(\([^)]*\))?(!)?: .+'
while IFS= read -r -d $'\x1e' commit; do
  commit="${commit#$'\n'}" # git puts a newline between records
  subject="${commit%%$'\n'*}"
  body="${commit#*$'\n'}"
  if [[ "$subject" =~ $type_re ]]; then
    type="${BASH_REMATCH[1],,}"
    bang="${BASH_REMATCH[3]}"
  else
    continue
  fi
  if [[ -n "$bang" ]] || grep -qE '^BREAKING[ -]CHANGE: ' <<<"$body"; then
    level=3
    break
  fi
  case "$type" in
    feat) ((level < 2)) && level=2 ;;
    fix | perf) ((level < 1)) && level=1 ;;
  esac
done < <(git log --no-merges --format='%s%n%b%x1e' "$range")

# Pre-1.0: breaking changes bump minor, as SemVer allows for 0.x.
if ((level == 3 && major == 0)); then
  level=2
fi

case "$level" in
  3) version="$((major + 1)).0.0"; bump="major" ;;
  2) version="$major.$((minor + 1)).0"; bump="minor" ;;
  1) version="$major.$minor.$((patch + 1))"; bump="patch" ;;
  *) version=""; bump="" ;;
esac

# An explicit `Release-As: X.Y.Z` in any commit since the last release wins
# when it is newer than that release (the highest such version counts).
release_as="$(git log --no-merges --format='%b' "$range" |
  sed -nE 's/^Release-As: v?([0-9]+\.[0-9]+\.[0-9]+)[[:space:]]*$/\1/p' | sort -V | tail -n 1)"
if [[ -n "$release_as" ]]; then
  IFS=. read -r ra_major ra_minor ra_patch <<<"$release_as"
  if ((ra_major > major || (ra_major == major && (ra_minor > minor || (ra_minor == minor && ra_patch > patch))))); then
    version="$release_as"
    if ((ra_major > major)); then bump="major"; elif ((ra_minor > minor)); then bump="minor"; else bump="patch"; fi
  else
    echo "Ignoring Release-As: $release_as, which is not newer than $base." >&2
  fi
fi

out="version=$version
tag=${version:+v$version}
bump=$bump
previous=$previous"
echo "$out"
if [[ -n "${GITHUB_OUTPUT:-}" ]]; then
  echo "$out" >>"$GITHUB_OUTPUT"
fi
