#!/bin/sh
# Verifica las dos reglas del workspace (hito 1 + 2):
#   1. Ningún Cargo.toml declara dependencias externas (solo `path`),
#      SALVO los crates marcados explícitamente como adaptadores de
#      frontera (primera línea: "# ADAPTADOR: dependencias externas
#      permitidas") — hoy solo crates/vercel-entry.
#   2. Todo lib.rs/main.rs (y api/*.rs de los adaptadores) arranca
#      con #![forbid(unsafe_code)].
# Úsalo en CI o antes de un commit: ./scripts/check-std-only.sh
set -eu
cd "$(dirname "$0")/.."

fallo=0
marcador='# ADAPTADOR: dependencias externas permitidas'

for toml in crates/*/Cargo.toml; do
    if head -n1 "$toml" | grep -qF "$marcador"; then
        continue
    fi
    # Dentro de [dependencies]: toda línea "nombre = ..." debe llevar path =
    deps=$(awk '/^\[dependencies\]/{on=1;next} /^\[/{on=0} on && /^[a-zA-Z0-9_-]+ *=/' "$toml")
    externas=$(printf '%s\n' "$deps" | grep -v 'path *=' | grep -v '^$' || true)
    if [ -n "$externas" ]; then
        echo "DEPENDENCIA EXTERNA en $toml:"
        printf '%s\n' "$externas"
        fallo=1
    fi
done

for src in crates/*/src/lib.rs crates/*/src/main.rs crates/*/api/*.rs; do
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
