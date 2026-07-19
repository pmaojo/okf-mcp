# Despliegue del outbox-worker en Railway

Este documento explica cómo desplegar el daemon `outbox-worker` del proyecto okf-mcp en [Railway](https://railway.app).

## ¿Qué hace el outbox-worker?

Es un proceso persistente que:

1. Escucha la tabla `outbox` de PostgreSQL/Supabase.
2. Sincroniza los cambios (commits y deletes) hacia GitHub.
3. Genera embeddings con Gemini para búsqueda semántica.
4. Ejecuta un bucle de reconciliación periódico `GitHub -> Supabase` para mantener ambos alineados.

## Requisitos previos

- Tener una cuenta en [Railway](https://railway.app).
- Tener acceso a una base de datos PostgreSQL (puede ser la de Railway o Supabase).
- Tener un token de GitHub (`GITHUB_TOKEN`) con permisos para escribir en el repo destino.
- (Opcional) Una clave de API de Gemini (`GEMINI_API_KEY`) para embeddings.

## Archivos de configuración

El repositorio ya incluye:

- `Procfile`: define el proceso `worker` que ejecuta el binario compilado.
- `railway.toml`: configura el build y el comando de inicio para Nixpacks.

## Pasos de despliegue

### 1. Crear el proyecto en Railway

1. Ve al [dashboard de Railway](https://railway.app/dashboard).
2. Clic en **New Project** → **Deploy from GitHub repo**.
3. Selecciona `pmaojo/okf-mcp`.

### 2. Añadir la base de datos (si usas Railway Postgres)

1. Dentro del proyecto, clic en **New** → **Database** → **Add PostgreSQL**.
2. Railway creará automáticamente la variable `DATABASE_URL`.
3. Copia el valor de `DATABASE_URL` y créala como `POSTGRES_URL` en las variables del servicio (paso 3).

> Si usas Supabase, omite este paso y usa directamente tu `POSTGRES_URL` de Supabase.

### 3. Configurar variables de entorno

Ve al servicio del worker en Railway → pestaña **Variables** y añade:

```env
POSTGRES_URL=postgres://usuario:pass@host:5432/db
GITHUB_TOKEN=tu_github_token
GITHUB_REPO=owner/repo
GEMINI_API_KEY=tu_gemini_key   # opcional
```

> **Nota:** Si usas la PostgreSQL de Railway, asegúrate de que `POSTGRES_URL` apunte a la misma base que usa `vercel-entry`.

### 4. Verificar el comando de inicio

Railway usará la configuración de `railway.toml`:

```toml
[build]
builder = "nixpacks"
buildCommand = "cargo build --release -p outbox-worker"

[deploy]
startCommand = "./target/release/outbox-worker"
```

Si por algún motivo Nixpacks no detecta el binario correctamente, puedes alternar usando el `Procfile`:

```
worker: cargo run --release -p outbox-worker
```

### 5. Desplegar

1. Railway detectará automáticamente el repo Rust y compilará el workspace.
2. Una vez terminado el build, ejecutará el daemon.
3. Ve a la pestaña **Logs** para verificar que arranca correctamente.

### 6. Verificar que funciona

En los logs deberías ver algo como:

```text
Iniciando worker de Outbox...
Procesando lote de 0 eventos...
```

Cuando hagas un `memory_commit` o `memory_delete` en el servidor MCP, el worker procesará el evento y lo sincronizará con GitHub.

## Solución de problemas

| Síntoma | Causa probable | Solución |
|---|---|---|
| `POSTGRES_URL must be set` | Falta la variable de entorno | Añadir `POSTGRES_URL` en Railway |
| `GitHub sync failed` | `GITHUB_TOKEN` o `GITHUB_REPO` incorrectos | Verificar token y formato `owner/repo` |
| El build falla por workspace | Nixpacks no detecta el crate correcto | Usar `railway.toml` con `buildCommand` explícito |
| No se generan embeddings | Falta `GEMINI_API_KEY` | Añadir la clave o ignorar si no se usa búsqueda semántica |

## Alternativa: ejecutar localmente

Si prefieres probar el worker en local antes de desplegar:

```bash
POSTGRES_URL=... GITHUB_TOKEN=... GITHUB_REPO=... cargo run -p outbox-worker
```

Para ejecutar solo un lote y salir:

```bash
ONCE=1 POSTGRES_URL=... GITHUB_TOKEN=... GITHUB_REPO=... cargo run -p outbox-worker
```

## Más información

- [Railway Config as Code](https://docs.railway.com/config-as-code/reference)
- [Nixpacks Rust Provider](https://nixpacks.com/docs/providers/rust)
- Capítulo 14 del tutorial: `tutorial/14-outbox-sincronizacion.md`
