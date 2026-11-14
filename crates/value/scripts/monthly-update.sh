#!/usr/bin/env bash
# Monthly refresh of the value layer, run by stablecoin-value-update.timer on wtwang-wsl.
#
# Works in a dedicated clone (default ~/services/stablecoin-lab-data) so it never touches a
# development checkout. Fetches the sources, builds with the automatic window end, and when
# the pinned data changed, opens a pull request titled "data(value): YYYY-MM update" with a
# summary of added months and revised values. It never merges. The site reads
# crates/value/data/out/map_data.json from main, so a merge is the release.
set -euo pipefail

CLONE="${STABLECOIN_LAB_DATA_CLONE:-$HOME/services/stablecoin-lab-data}"
REPO_URL="https://github.com/egpivo/stablecoin-lab.git"
STAMP="$(date -u +%Y-%m)"
BRANCH="data/value-$STAMP"
BASE_REF="${BASE_REF:-origin/main}"   # override only to test before the value layer is on main
DRY_RUN="${DRY_RUN:-}"              # non-empty: build and print the PR body, push nothing
DATA="crates/value/data"

if [[ ! -d "$CLONE/.git" ]]; then
  git clone -q "$REPO_URL" "$CLONE"
fi
cd "$CLONE"
if [[ -n "$(git status --porcelain)" ]]; then
  echo "refusing: $CLONE has local changes" >&2
  exit 1
fi
git fetch -q origin
if git ls-remote --exit-code --heads origin "$BRANCH" >/dev/null; then
  echo "$BRANCH already exists on origin; nothing to do"
  exit 0
fi
git checkout -q -B "$BRANCH" "$BASE_REF"

prev="$(mktemp)"
trap 'rm -f "$prev" "$prev.body"' EXIT
cp "$DATA/out/map_data.json" "$prev"

cargo run -q --release -p stablecoin-value -- fetch
build_line="$(cargo run -q --release -p stablecoin-value -- build)"
echo "$build_line"

if git diff --quiet -- "$DATA/manifest" "$DATA/out/map_data.json"; then
  echo "no change in pinned data"
  git checkout -q main 2>/dev/null || true
  exit 0
fi

{
  echo "## Objective"
  echo
  echo "Monthly refresh of the value layer data ($STAMP). Merging publishes it: the site's Lab map reads \`$DATA/out/map_data.json\` from main."
  echo
  echo "## Changes"
  echo
  echo "- \`$DATA/manifest/raw.sha256\` pins the new raw files (BIS, World Bank, DefiLlama)."
  echo "- \`$DATA/out/map_data.json\` rebuilt. Build: \`$build_line\`"
  echo
  python3 crates/value/python/diff_summary.py "$prev" "$DATA/out/map_data.json"
  echo
  echo "## Example"
  echo
  echo "Opened by \`crates/value/scripts/monthly-update.sh\` (stablecoin-value-update.timer). Review the revised countries before merging."
} > "$prev.body"

if [[ -n "$DRY_RUN" ]]; then
  cat "$prev.body"
  git checkout -q -- "$DATA/manifest" "$DATA/out"   # leave the clone clean for the next run
  echo "DRY_RUN: no commit, push or PR"
  exit 0
fi

git add "$DATA/manifest/raw.sha256" "$DATA/out/map_data.json"
git -c user.name="${GIT_AUTHOR_NAME:-egpivo}" commit -q -m "data(value): $STAMP update"
git push -q -u origin "$BRANCH"
gh pr create --repo egpivo/stablecoin-lab --head "$BRANCH" --base main \
  --title "data(value): $STAMP update" --body-file "$prev.body"
