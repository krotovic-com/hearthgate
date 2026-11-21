#!/bin/sh
# Builds the website: the landing page from site/ at the root and the mdBook
# documentation from docs/ under /docs/, into target/site.
#   scripts/build-site.sh
#   python3 -m http.server -d target/site 8000   # preview at http://localhost:8000
set -eu
cd "$(dirname "$0")/.."

command -v mdbook >/dev/null || { echo "build-site: mdbook is not installed (cargo install mdbook --version 0.4.52)" >&2; exit 1; }

rm -rf target/site
mdbook build docs
mkdir -p target/site
cp -R site/. target/site/
cp -R target/book target/site/docs
# mdBook copies everything under docs/, including its own config and theme sources.
rm -f target/site/docs/book.toml target/site/docs/theme/index.hbs
for f in favicon.svg favicon-32.png app-icon-512.png social-preview.png; do
  cp "docs/brand/$f" target/site/assets/
done
echo "build-site: wrote target/site"
