# Capítulo 13 — OAuth 2.1: El servidor MCP como Resource Server

Crate: [`crates/vercel-entry`](../crates/vercel-entry)

## 1. El problema

Cuando desplegamos un servidor MCP a la red, cualquier cliente HTTP con acceso al endpoint puede intentar realizar modificaciones o leer el grafo de conocimiento. Para un sistema en producción, es obligatorio securizar el acceso. Pero no queremos que el cliente envíe contraseñas crudas o claves de base de datos directamente.

Siguiendo el estándar de la industria, el servidor MCP debe actuar como un **Resource Server** bajo el protocolo **OAuth 2.1**. El cliente (el agente de IA) debe presentar un token de acceso (JWT) firmado por un **Authorization Server** de confianza (como Supabase Auth o un proveedor OIDC compatible). El servidor MCP debe validar criptográficamente la firma del token contra las llaves públicas publicadas en un endpoint JWKS (JSON Web Key Set) antes de permitir que cualquier herramienta sea invocada.

## 2. El invariante

> **El servidor MCP nunca ejecuta una herramienta ni realiza consultas a la base de datos para peticiones que requieran autenticación sin antes verificar criptográficamente la firma, vigencia y audiencia del token JWT.**

Y una segunda promesa de diseño:

> **La validación del token debe ser sin estado, y las llaves públicas de firma deben cachearse de forma local y thread-safe para no penalizar la latencia del enrutamiento de peticiones.**

## 3. La implementación mínima

El módulo de autorización ([auth.rs](../crates/vercel-entry/src/auth.rs)) implementa la verificación criptográfica. Primero se extrae el token del encabezado `Authorization: Bearer <token>` de la petición HTTP.

Una llave RSA se compone de dos elementos matemáticos: el módulo (`n`) y el exponente (`e`). `jsonwebtoken` nos permite fabricar una llave de decodificación a partir de estos parámetros obtenidos del JWKS:

```rust
let decoding_key = DecodingKey::from_rsa_components(&jwk.n, &jwk.e)
    .map_err(|e| AuthError(format!("invalid key: {e}")))?;
```

Una vez obtenida la llave de decodificación correspondiente al identificador `kid` (Key ID) que viaja en el encabezado del JWT, validamos los claims:

```rust
let mut validation = Validation::new(header.alg);
if let Some(aud) = expected_audience {
    validation.set_audience(&[aud]);
} else {
    validation.validate_aud = false;
}

let token_data = decode::<Claims>(token, &decoding_key, &validation)?;
```

Si el token es válido, se extrae el sujeto (`sub`) y el ID del cliente (`client_id`) para conformar el `Principal` del actor de forma dinámica para esta petición.

## 4. Una versión deliberadamente rota

Imagina que para simplificar la inicialización del servidor, mantienes una única instancia global de `McpServer` protegida por un `Mutex` y sobreescribes el actor antes de ejecutar el enrutamiento:

```rust
// ❌ NO HACER: Modificar un actor global compartido entre peticiones concurrentes
let mut server = shared_server().lock().unwrap();
server.handler.set_actor(actor); // Condición de carrera concurrente
let resp = route(&http_req, &budget, &origins, &mut server);
```

## 5. Por qué falla

En un servidor HTTP real, múltiples hilos procesan diferentes peticiones de forma paralela. Si compartes una única instancia mutable de `McpServer` (y por ende de `MemoryTools`):
1. **Carreras de datos:** El hilo A puede validar el token del Usuario A y fijar el actor. Inmediatamente después, antes de que el hilo A termine su ejecución, el hilo B valida el token del Usuario B y sobreescribe el actor. La escritura del hilo A se guardará en el historial a nombre del Usuario B.
2. **Cuello de botella:** Para evitar la carrera de datos, te verías obligado a bloquear el `Mutex` durante todo el tiempo que dure la petición (incluyendo los accesos a base de datos de la herramienta). Esto serializa las peticiones de todos los usuarios en un solo hilo de ejecución, destruyendo el rendimiento del servidor.

La solución es **instanciar `McpServer` de forma efímera** para cada petición en [mcp.rs](../crates/vercel-entry/api/mcp.rs). Dado que `PgPool` clona de forma interna de manera barata, instanciar las estructuras toma nanosegundos y aísla por completo el contexto de cada petición.

## 6. Memoria y asignación

Para evitar descargar las llaves públicas de firma en cada llamada (lo que añadiría cientos de milisegundos de latencia y saturaría de peticiones al servidor de autorización), implementamos un cache local con expiración en un `RwLock` asíncrono global ([auth.rs](../crates/vercel-entry/src/auth.rs)):

```rust
struct JwksCache {
    keys: Vec<Jwk>,
    expires_at: Instant,
}
static JWKS_CACHE: OnceLock<Arc<RwLock<Option<JwksCache>>>> = OnceLock::new();
```

El bloqueo de lectura (`read().await`) permite que múltiples peticiones verifiquen tokens concurrentemente sin bloquearse entre sí. Solo cuando el caché expira o se requiere un `kid` desconocido, se adquiere un bloqueo de escritura (`write().await`) para realizar la petición HTTP al endpoint JWKS y actualizar el cache.

## 7. Tests

Para probar la lógica de autenticación en desarrollo y pruebas unitarias, el validador es desactivado de manera transparente si la variable de entorno `JWKS_URL` no está definida. En este escenario, el sistema asume que opera en modo abierto y devuelve un actor por defecto (`Principal::local_dev()`).

## 8. Descubrimiento OAuth: cómo sabe el cliente MCP a dónde autenticarse

Validar el JWT no basta: un cliente MCP genérico (Claude, u otro agente) no sabe de antemano contra qué Authorization Server debe autenticarse, ni tiene un token todavía en su primera petición. Dos piezas del protocolo resuelven esto, implementadas en [mcp.rs](../crates/vercel-entry/api/mcp.rs):

1. **`GET /.well-known/oauth-protected-resource`** (RFC 9728): un endpoint público, sin autenticación, que responde con el recurso protegido y la lista de Authorization Servers de confianza:

   ```json
   {"resource": "https://tu-dominio/mcp", "authorization_servers": ["https://<proyecto>.supabase.co/auth/v1"]}
   ```

2. **Cabecera `WWW-Authenticate` en la respuesta 401**: cuando `validate_jwt` falla (token ausente, expirado o inválido), la respuesta incluye:

   ```
   WWW-Authenticate: Bearer resource_metadata="https://tu-dominio/.well-known/oauth-protected-resource"
   ```

   Esta cabecera es la que le dice al cliente "aquí no puedes actuar sin token, y esta es la URL donde puedes averiguar cómo conseguir uno".

El cliente sigue la cadena: `401` → lee `WWW-Authenticate` → descarga `/.well-known/oauth-protected-resource` → obtiene el `issuer` del Authorization Server → descubre allí `authorization_endpoint` y `token_endpoint` vía `<issuer>/.well-known/openid-configuration` (Supabase Auth expone descubrimiento OIDC estándar en esa ruta) → inicia el flujo `authorization_code` + PKCE.

Nótese que `vercel.json` necesita un `rewrite` explícito para `/.well-known/oauth-protected-resource` → `/api/mcp`, igual que para `/mcp`: Vercel solo invoca la función Rust para las rutas que le indiques.

## 9. Frontera de producción: variables de entorno reales

Para este despliegue concreto (Supabase como Authorization Server), las variables de entorno de Vercel son:

| Variable | Valor | Para qué |
|---|---|---|
| `JWKS_URL` | `https://<proyecto>.supabase.co/auth/v1/.well-known/jwks.json` | Llaves públicas para verificar la firma del JWT (`auth.rs`) |
| `OAUTH_ISSUER` | `https://<proyecto>.supabase.co/auth/v1` | Se anuncia en `/.well-known/oauth-protected-resource` como `authorization_servers` |
| `JWT_AUDIENCE` | el `client_id` de la OAuth App registrada en Supabase (opcional pero recomendado) | Restringe qué tokens acepta el servidor (`aud` claim) |

Puedes confirmar que tu proyecto de Supabase emite JWTs firmados de forma asimétrica (necesario para JWKS — un secreto compartido HS256 no se puede publicar como llave pública) consultando:

```bash
curl https://<proyecto>.supabase.co/auth/v1/.well-known/jwks.json
curl https://<proyecto>.supabase.co/auth/v1/.well-known/openid-configuration
```

Si `jwks_uri` devuelve una lista de llaves (`"alg":"ES256"` o `"RS256"`), el proyecto ya emite tokens con firma asimétrica y esta pieza funciona sin cambios adicionales en Supabase.

### La pieza manual: Supabase Auth no soporta Dynamic Client Registration

El MCP estándar espera que el cliente (Claude) pueda auto-registrarse contra el Authorization Server (RFC 7591, `registration_endpoint`) sin intervención humana. **Supabase Auth, a fecha de este tutorial, no expone un `registration_endpoint` público** — su discovery OIDC no lo anuncia y el endpoint `/auth/v1/oauth/register` responde `404` sin credenciales de administrador.

Esto significa que, para que Claude complete el flujo, primero debes registrar manualmente una "OAuth App" en el dashboard de Supabase (Authentication → OAuth Apps / third-party auth), obteniendo un `client_id` (y `client_secret` si aplica) con el `redirect_uri` exacto que Claude te muestre al añadir el conector. Ese `client_id` es el valor que corresponde a `JWT_AUDIENCE` arriba.

3. El cliente MCP deberá incluir la cabecera `Authorization: Bearer <token_jwt>` en cada petición POST — esto ya funcionaba antes de esta sección; lo nuevo es cómo el cliente *consigue* ese token sin intervención manual del protocolo (salvo el registro de la OAuth App, que sí es manual).

## 10. Principios SOLID en juego

* **S (Responsabilidad Única):** La validación criptográfica y la lógica de JWKS reside exclusivamente en [auth.rs](../crates/vercel-entry/src/auth.rs). El enrutador `mcp.rs` solo invoca la función y reacciona ante el éxito o el error 401.
* **D (Inversión de Dependencias):** El core del protocolo (`mcp-core`) y el enrutador HTTP (`mcp-http`) siguen siendo 100% agnósticos de la autenticación. No conocen JWT ni JWKS; simplemente propagan un objeto `Principal` tipado.
* **O (Abierto-Cerrado):** Podemos cambiar el proveedor de identidad de Supabase a Auth0 simplemente actualizando la URL de JWKS en la configuración del entorno, sin tocar una sola línea de código en el repositorio.

## 11. Ejercicios

1. **Guiado.** ¿Qué problemas de seguridad surgen si un token JWT no especifica fecha de expiración (`exp`)? ¿Cómo reacciona `jsonwebtoken` por defecto?
2. **Medio.** Modifica `auth.rs` para permitir múltiples audiencias válidas (p. ej., si tu servidor MCP es compartido por una aplicación web y una extensión de navegador).
3. **Abierto.** Diseña una estrategia para rotar las llaves de firma del JWKS en caliente sin causar errores temporales en peticiones concurrentes de usuarios legítimos.
4. **Abierto.** La sección 9 documenta que Supabase Auth no soporta Dynamic Client Registration (RFC 7591). Investiga qué otros Authorization Servers (Auth0, WorkOS, Ory Hydra) sí lo soportan de forma nativa, y qué tendría que cambiar en `mcp.rs`/`auth.rs` si migraras el `issuer` a uno de ellos — ¿alguna línea de código lo requeriría, o solo variables de entorno?
