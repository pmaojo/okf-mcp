# Despliegue del outbox-worker en Render

Este documento explica cómo desplegar el daemon `outbox-worker` del proyecto okf-mcp en [Render](https://render.com).

## ¿Por qué Render?

Render ofrece una capa gratuita que incluye:

- **Web services y workers persistentes** (no serverless).
- **PostgreSQL gratis** (hasta un cierto límite).
- **Despliegue automático desde GitHub**.
- **Soporte nativo para Docker**.

Es una buena alternativa a Railway si el trial de Railway ha expirado.

## Archivos de configuración

El repositorio incluye:

- `Dockerfile`: imagen multi-etapa que compila el workspace y ejecuta `outbox-worker`.
- `render.yaml`: blueprint de Render para crear el worker y la base de datos.
- `.dockerignore`: evita copiar archivos innecesarios al contexto de build.

## Requisitos previos

- Cuenta en [Render](https://render.com).
- Repo `pmaojo/okf-mcp` conectado a Render.
- Token de GitHub (`GITHUB_TOKEN`) con permisos de escritura en el repo destino.
- (Opcional) Clave de API de Gemini (`GEMINI_API_KEY`) para embeddings.

## Opción A: Despliegue con Blueprint (`render.yaml`)

1. Ve al [dashboard de Render](https://dashboard.render.com).
2. Clic en **New** → **Blueprint**.
3. Selecciona el repo `pmaojo/okf-mcp`.
4. Render leerá `render.yaml` y creará:
   - Un **PostgreSQL** llamado `okf-postgres`.
   - Un **Background Worker** llamado `okf-outbox-worker`.
5. Configura las variables de entorno que Render no genera automáticamente:
   - `GITHUB_TOKEN`
   - `GITHUB_REPO`
   - `GEMINI_API_KEY` (opcional)
6. Render desplegará el worker automáticamente.

## Opción B: Despliegue manual

Si prefieres no usar el blueprint:

1. Ve al dashboard de Render → **New** → **Background Worker**.
2. Selecciona el repo y la rama.
3. En **Runtime**, elige **Docker**.
4. Deja el **Dockerfile Path** como `./Dockerfile`.
5. Añade las variables de entorno:
   - `POSTGRES_URL`
   - `GITHUB_TOKEN`
   - `GITHUB_REPO`
   - `GEMINI_API_KEY` (opcional)
6. Clic en **Create Background Worker**.

## Variables de entorno

| Variable | Obligatoria | Descripción |
|---|---|---|
| `POSTGRES_URL` | Sí | Conexión PostgreSQL/Supabase. |
| `GITHUB_TOKEN` | Sí | Token de GitHub con permisos de escritura. |
| `GITHUB_REPO` | Sí | Repo destino, formato `owner/repo`. |
| `GEMINI_API_KEY` | No | Para generar embeddings. |

> Si usas la PostgreSQL que Render crea automáticamente, `POSTGRES_URL` se inyecta desde el blueprint. Si usas Supabase, configúrala manualmente.

## Verificar el despliegue

1. Ve a la pestaña **Logs** del worker en Render.
2. Deberías ver:

```text
Iniciando worker de Outbox...
```

3. Realiza un `memory_commit` en el servidor MCP.
4. Vuelve a los logs: deberías ver el evento procesado y sincronizado con GitHub.

## Solución de problemas

| Síntoma | Causa probable | Solución |
|---|---|---|
| `POSTGRES_URL must be set` | Falta la variable | Configurar `POSTGRES_URL` en Render |
| `GitHub sync failed` | Token o repo incorrecto | Verificar `GITHUB_TOKEN` y `GITHUB_REPO` |
| El build es muy lento | Docker copia todo el contexto | Revisar `.dockerignore` |
| No se generan embeddings | Falta `GEMINI_API_KEY` | Añadir la clave o ignorar si no se usa búsqueda semántica |

## Alternativas

Si Render no te convence, también puedes desplegar el worker en:

- **Fly.io**: ver guía en construcción.
- **Railway**: ver `DEPLOY_RAILWAY.md`.
- **Vercel Cron**: usa el endpoint `/api/outbox` en lugar de un daemon persistente.
