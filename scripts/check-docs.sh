#!/bin/sh
# La documentación es parte del build (capítulo 16 del tutorial):
#   1. `cargo doc` del núcleo sin UN SOLO warning: ningún item
#      público sin documentar (#![warn(missing_docs)]) y ningún
#      enlace intra-doc roto (rustdoc los verifica al compilar).
#   2. Los ejemplos de la documentación compilan y PASAN como tests
#      (`cargo test --doc`). Un ejemplo que miente rompe el build.
# Úsalo en CI o antes de un commit: ./scripts/check-docs.sh
set -eu
cd "$(dirname "$0")/.."

# Crates del núcleo (hito 1): solo std, documentación exigible al 100%.
# Los adaptadores de frontera se documentan igual, pero su puerta de
# calidad vive en sus propios tests de contrato.
nucleo="-p memory-model -p hash-core -p json-mini -p okf-core \
 -p graph-core -p ontology-core -p conflict-core -p store-core -p memory-store \
 -p memory-tools -p mcp-core -p ingest-core"

# shellcheck disable=SC2086  # $nucleo debe expandirse en argumentos
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps $nucleo
# shellcheck disable=SC2086
cargo test --doc $nucleo

echo "OK: documentación sin warnings y doctests en verde."
