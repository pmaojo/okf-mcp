#!/bin/sh
# Verifica las dos reglas del workspace (hito 1):
#   1. Ningún Cargo.toml declara dependencias externas (solo `path`).
#   2. Todo lib.rs/main.rs arranca con #![forbid(unsafe_code)].
# Úsalo en CI o antes de un commit: ./scripts/check-std-only.sh
set -eu
cd "$(dirname "$0")/.."

fallo=0

for toml in crates/*/Cargo.toml; do
    # Dentro de [dependencies]: toda línea "nombre = ..." debe llevar path =
    deps=$(awk '/^\[dependencies\]/{on=1;next} /^\[/{on=0} on && /^[a-zA-Z0-9_-]+ *=/' "$toml")
    externas=$(printf '%s\n' "$deps" | grep -v 'path *=' | grep -v '^$' || true)
    if [ -n "$externas" ]; then
        echo "DEPENDENCIA EXTERNA en $toml:"
        printf '%s\n' "$externas"
        fallo=1
    fi
done

for src in crates/*/src/lib.rs crates/*/src/main.rs; do
    [ -f "$src" ] || continue
    if ! grep -q '#!\[forbid(unsafe_code)\]' "$src"; then
        echo "FALTA #![forbid(unsafe_code)] en $src"
        fallo=1
    fi
done

if [ "$fallo" -eq 0 ]; then
    echo "OK: núcleo 100% std, sin unsafe."
fi
exit "$fallo"
