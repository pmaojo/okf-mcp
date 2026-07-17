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

Un JWK puede publicar una llave RSA (módulo `n` y exponente `e`) o una llave EC (curva `crv` y coordenadas `x`, `y`) — el campo `kty` indica cuál es. **Esto no es un detalle académico:** Supabase Auth firma sus tokens con **ES256 (EC, curva P-256)** por defecto, no con RSA. Una implementación que solo entienda `n`/`e` falla al deserializar el JWKS real con `missing field n` — no es un caso hipotético, es lo primero que rompe si copias un tutorial que asume RSA sin comprobarlo contra tu proveedor real:

```rust
fn decoding_key_from_jwk(jwk: &Jwk) -> Result<DecodingKey, AuthError> {
    match jwk.kty.as_str() {
        "RSA" => DecodingKey::from_rsa_components(n, e)...,
        "EC" => DecodingKey::from_ec_components(x, y)...,  // Supabase usa esta rama
        other => Err(AuthError(format!("unsupported key type: {other}"))),
    }
}
```

Una vez obtenida la llave de decodificación correspondiente al identificador `kid` (Key ID) que viaja en el encabezado del JWT, validamos los claims. Dado que Supabase Auth firma **todos** sus JWTs con la audiencia fija `aud: "authenticated"` (el rol de base de datos de Postgres), la validación de audiencia estándar de JWT fallaría si la comparamos directamente con nuestro `client_id`. Por ello, desactivamos la comprobación de `aud` en la validación estándar y comparamos manualmente el claim `client_id` extraído:

```rust
let mut validation = Validation::new(header.alg);
validation.validate_aud = false; // Desactivado: aud es siempre "authenticated"

let token_data = decode::<Claims>(token, &decoding_key, &validation)?;
let client_id = token_data.claims.client_id
    .or(token_data.claims.azp)
    .unwrap_or_else(|| "unknown".to_string());

if let Some(expected_client_id) = expected_client_id {
    if client_id != expected_client_id {
        return Err(AuthError("token issued for a different OAuth client".to_string()));
    }
}
```

Si el token es válido y pertenece al cliente esperado, conformamos el `Principal` del actor de forma dinámica para esta petición.

## 3.5. Conceptos de Rust en este capítulo

Este capítulo implementa criptografía y caché asíncrono avanzado empleando mecanismos de sincronización concurrentes:

* **Bloqueos de Lectura/Escritura con `RwLock`:** A diferencia de un `Mutex` (que solo permite el acceso a un hilo a la vez, ya sea para leer o escribir), un `RwLock` (Read-Write Lock) permite que **múltiples hilos lean los datos de forma simultánea** (`read().await`) sin bloquearse entre sí. El bloqueo completo de exclusión mutua solo ocurre cuando un hilo necesita actualizar el caché (`write().await`), lo cual es ideal para las llaves JWKS que cambian muy raramente (ej. una vez al día) pero se leen en cada petición de usuario.
* **Caché estático seguro con `OnceLock`:** Declarar variables globales o estáticas en Rust requiere garantías estrictas de seguridad de hilos (thread-safety). Combinamos `OnceLock` (para inicializar el caché global de llaves exactamente una vez) con `Arc<RwLock<Option<...>>>`. Esta "cebolla" de tipos garantiza que múltiples hilos del servidor web asíncrono puedan acceder a las llaves criptográficas de forma segura y sin carreras de datos.
* **Conversión de Errores con `jsonwebtoken` y `?`:** Las funciones de la librería externa `jsonwebtoken` devuelven errores específicos de su propio ecosistema. Para integrarlos limpiamente con nuestro sistema de control de errores, usamos mapeos de error (ej. `.map_err(...)`) o implementamos conversiones de tipo (`impl From<jsonwebtoken::errors::Error> for AuthError`) que permiten que el operador `?` los convierta de forma automática y transparente en el tipo de error propio del servidor.

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

Nótese que `vercel.json` necesita un `rewrite` explícito para `/.well-known/oauth-protected-resource` → `/api/mcp`, igual que para `/mcp`: Vercel solo invoca la función Rust para las rutas que le indiques. Y una trampa real que costó depurar: ese `vercel.json` tiene que vivir en el **Root Directory** configurado del proyecto (`crates/vercel-entry`, ver capítulo 11), no en la raíz del repositorio — uno puesto en la raíz del repo se ignora sin ningún aviso, y el síntoma es exactamente que `/mcp` y `/.well-known/...` responden 404 de la propia plataforma Vercel (no de nuestro código) mientras `/api/mcp` funciona perfectamente, porque ese sí sale del enrutado automático por convención de carpetas.

## 9. Frontera de producción: variables de entorno reales

Para este despliegue concreto (Supabase como Authorization Server), las variables de entorno de Vercel son:

| Variable | Valor | Para qué |
|---|---|---|
| `JWKS_URL` | `https://<proyecto>.supabase.co/auth/v1/.well-known/jwks.json` | Llaves públicas para verificar la firma del JWT (`auth.rs`) |
| `OAUTH_ISSUER` | `https://<proyecto>.supabase.co/auth/v1` | Se anuncia en `/.well-known/oauth-protected-resource` como `authorization_servers` |
| `JWT_AUDIENCE` | el `client_id` esperado de la OAuth App (**opcional**) | Restringe qué tokens acepta el servidor (comparando contra el claim `client_id`) |
| `SUPABASE_ANON_KEY` | la anon key pública de tu proyecto de Supabase | Requerida por la página de consentimiento para comunicarse con GoTrue |

`JWT_AUDIENCE` es opcional. Validar qué aplicación pidió el token es un requisito de la especificación de autorización de MCP (audiencia esperada). En Supabase Auth, todos los tokens llevan la audiencia estándar fija `aud: "authenticated"`, por lo que no podemos usar el validador automático de audiencia de JWT. En su lugar, desactivamos ese validador y, si `JWT_AUDIENCE` está configurada, comprobamos manualmente que el claim `client_id` (o en su defecto `azp`) coincida exactamente. 

Sin embargo, si habilitamos el **Registro Dinámico de Clientes (DCR)** en el servidor OAuth de Supabase, el cliente de Claude se registrará automáticamente en cada sesión, obteniendo un `client_id` dinámico recién generado. Si tuviéramos `JWT_AUDIENCE` fijada a un único valor estático en producción, el flujo DCR fallaría inmediatamente. Por ello, si no configuras `JWT_AUDIENCE`, el servidor MCP acepta cualquier token correctamente firmado por nuestro emisor (Supabase Auth). En este escenario, la barrera de seguridad real y el límite de confianza residen en la pantalla de inicio de sesión de Supabase Auth más la aprobación explícita del usuario en `/oauth/consent`, y no en la validación estática de un identificador de cliente.

Puedes confirmar que tu proyecto de Supabase emite JWTs firmados de forma asimétrica (necesario para JWKS — un secreto compartido HS256 no se puede publicar como llave pública) consultando:

```bash
curl https://<proyecto>.supabase.co/auth/v1/.well-known/jwks.json
curl https://<proyecto>.supabase.co/auth/v1/.well-known/openid-configuration
```

Si `jwks_uri` devuelve una lista de llaves (`"alg":"ES256"` o `"RS256"`), el proyecto ya emite tokens con firma asimétrica y esta pieza funciona sin cambios adicionales en Supabase.

### Registrar el cliente: manual, o automático con DCR

Claude soporta tres formas de registrarse como cliente OAuth contra tu Authorization Server (documentado por Anthropic en su guía de autenticación de conectores):

1. **`oauth_dcr`** — Dynamic Client Registration (RFC 7591): Claude se auto-registra llamando a un `registration_endpoint` que el AS anuncia en su discovery.
2. **`oauth_cimd`** — Client ID Metadata Documents: Claude usa una URL HTTPS como `client_id`; el AS la resuelve para leer el `redirect_uri` de Claude. No requiere llamada de registro, pero exige que el AS soporte client-ids en formato URL (un draft de OAuth todavía poco extendido).
3. **`oauth_anthropic_creds`** — Anthropic gestiona las credenciales directamente (requiere contactar `mcp-review@anthropic.com`).

**Opción manual (sin tocar nada más en Supabase):** en **Settings → Connectors → Add custom connector → Advanced settings**, introduces directamente el `Client ID` (y opcionalmente `Client Secret`) de una OAuth App que registres tú mismo en el dashboard de Supabase (Authentication → OAuth Apps), usando el `redirect_uri` que Claude muestre en esa misma pantalla (`https://claude.ai/api/mcp/auth_callback` para Claude.ai web/desktop/mobile/Cowork). Ese `client_id` es el valor que le darías a `JWT_AUDIENCE` si quisieras restringir el servidor a ese único cliente.

**Opción DCR (sin tocar nada en Claude):** Supabase Auth SÍ soporta `oauth_dcr`, pero apagado por defecto y con una trampa de nomenclatura real. Actívalo en **Authentication → OAuth Server → Allow dynamic client registration**; el efecto observable es que `registration_endpoint` aparece (o desaparece) en `<issuer>/.well-known/oauth-authorization-server`. La trampa: el path real que anuncia ese campo es `/auth/v1/oauth/clients/register` — **no** `/auth/v1/oauth/register`, que es el que uno tiende a probar primero (y que responde `404` tanto con DCR activo como apagado, porque simplemente no existe). Verificar el endpoint real siempre pasa por leer el discovery document, nunca por adivinar la convención:

```bash
curl https://<proyecto>.supabase.co/auth/v1/.well-known/oauth-authorization-server \
  | python3 -c 'import json,sys; print(json.load(sys.stdin)["registration_endpoint"])'
```

Con DCR activo, cada vez que Claude conecta se auto-registra como un cliente nuevo (`registration_type: dynamic` en `auth.oauth_clients`) — por eso la sección anterior insiste en que `JWT_AUDIENCE` debe quedar sin definir en este modo: fijarla a un cliente pre-registrado rechazaría sistemáticamente al cliente dinámico que Claude acaba de crear.

El cliente MCP deberá incluir la cabecera `Authorization: Bearer <token_jwt>` en cada petición POST — esto ya funcionaba antes de esta sección; lo nuevo es cómo el cliente *consigue* ese token sin que el protocolo de registro sea automático (el registro de la OAuth App en Supabase sí es manual, una vez).

### La sorpresa real: el Client ID manual no sigue el descubrimiento

Con todo lo anterior en su sitio — `/.well-known/oauth-protected-resource` respondiendo, `authorization_servers` apuntando a Supabase, el `client_id` metido a mano en Claude — el flujo seguía fallando: Claude generaba la URL de autorización contra **nuestro propio dominio** (`https://tu-dominio/authorize?...`), no contra el `authorization_endpoint` real de Supabase. Se comprobó a fondo que no era un fallo de descubrimiento: tanto `<issuer>/.well-known/openid-configuration` como `<issuer>/.well-known/oauth-authorization-server` (las dos convenciones de RFC 8414) responden con el `authorization_endpoint` correcto.

La conclusión, tras descartar todo lo demás: el modo "Client ID manual" de Claude para conectores MCP no delega en el `authorization_servers` anunciado — asume que el propio servidor MCP aloja el Authorization Server en su mismo dominio, con los paths estándar `/authorize` y `/token`. Es una limitación observada del cliente, no algo que la especificación MCP exija.

La solución no es pelearse con Claude, es dárselo: [mcp.rs](../crates/vercel-entry/api/mcp.rs) implementa `/authorize` y `/token` como un **proxy transparente** hacia los endpoints reales de Supabase.

```rust
async fn authorize_proxy_handler(uri: Uri) -> Response {
    let issuer = std::env::var("OAUTH_ISSUER")...;
    let query = uri.query().unwrap_or("");
    let location = format!("{issuer}/oauth/authorize?{query}");
    (StatusCode::FOUND, [("location", location)]).into_response()
}
```

`GET /authorize` reenvía la query string EXACTA (sin reparsear ni un solo parámetro — el `code_challenge` es un valor base64url sensible a cualquier re-encoding) como un 302 hacia `<issuer>/oauth/authorize`. `POST /token` hace lo mismo con el cuerpo de la petición hacia `<issuer>/oauth/token`, y devuelve la respuesta de Supabase sin tocarla.

Una corrección sobre la marcha, verificada contra el Supabase real: `token_endpoint_auth_methods_supported` anuncia `"none"` como opción (clientes públicos, solo PKCE), pero **la OAuth App concreta que registres puede estar configurada como confidencial** (`client_secret_basic`) — la primera prueba en vivo de este proxy falló exactamente así: `"client is registered for 'client_secret_basic' but 'none' was used"`. El proxy no genera ni conoce ningún secreto propio, pero si el cliente (Claude) manda uno vía la cabecera `Authorization` (Basic Auth), tiene que reenviarla igual que el resto — omitirla no es "no custodiar secretos", es simplemente perder una cabecera que el flujo necesita. `token_proxy_handler` reenvía `Authorization` si está presente, sin leerla ni transformarla.

Esto significa que, para Claude, `tu-dominio` SÍ es el Authorization Server — solo que cada endpoint es una línea que reenvía a Supabase. El cliente nunca necesita enterarse.

### El último reto: la pantalla de consentimiento personalizada

Con el proxy transparente funcionando, Claude redirigirá la sesión del usuario a `https://tu-dominio/authorize`, que a su vez lo reenvía a la página de login de Supabase Auth. Sin embargo, hay un último obstáculo en el estándar de servidores OAuth personalizados: **la pantalla de consentimiento**.

Supabase Auth espera que, tras iniciar sesión, el usuario acepte explícitamente conceder permisos a la aplicación externa (en este caso, el cliente de Claude). Pero **Supabase no aloja esta pantalla por defecto**. En su lugar, exige que el desarrollador proporcione una "Consent URI" (o *Authorization Path* en la configuración del OAuth Server en Supabase) que apunte a una interfaz web creada por nosotros.

Para evitar tener que desplegar y mantener un frontend separado para un solo archivo HTML, [mcp.rs](../crates/vercel-entry/api/mcp.rs) aloja e implementa la pantalla de consentimiento directamente en la ruta **`GET /oauth/consent`**:

1. **Servicio del HTML estático:** Rust lee una plantilla HTML/JS empotrada en el binario (`CONSENT_PAGE_TEMPLATE`) y reemplaza dinámicamente marcadores como `__OAUTH_ISSUER_JSON__` y `__SUPABASE_ANON_KEY_JSON__` con las variables de entorno reales.
2. **Lógica de consentimiento en el cliente (JavaScript):**
   - El script de la página comprueba si hay una sesión activa de Supabase Auth en el navegador del usuario. Si no la hay, muestra un formulario de login clásico que hace una llamada directa a `POST /token?grant_type=password` de GoTrue (Supabase Auth).
   - Una vez autenticado, solicita los detalles de la autorización pendiente llamando a `GET /oauth/authorizations/{id}` utilizando el parámetro `authorization_id` que viaja en la URL. Esto permite renderizar en pantalla el nombre del cliente (ej. *"Claude.ai"*) y la lista de permisos (*scopes*) solicitados.
   - Cuando el usuario hace clic en *"Permitir"* o *"Denegar"*, el JS envía la decisión del usuario mediante una petición `POST /oauth/authorizations/{id}/consent` con `{ "action": "approve" | "deny" }`.
   - Si la acción fue aprobada, Supabase Auth responde con una `redirect_url` que contiene el código de autorización temporal. El script de la página redirige al usuario a esa URL, completando el flujo OAuth de regreso a Claude de forma transparente.

Esta solución combina la potencia de GoTrue (el motor OAuth 2.1 e identidad de Supabase) con un hosting estático ultra-eficiente en nuestra propia función de Vercel, manteniendo un esquema de despliegue de **un solo binario serverless**.

## 10. Principios SOLID en juego

* **S (Responsabilidad Única):** La validación criptográfica y la lógica de JWKS reside exclusivamente en [auth.rs](../crates/vercel-entry/src/auth.rs). El enrutador `mcp.rs` solo invoca la función y reacciona ante el éxito o el error 401.
* **D (Inversión de Dependencias):** El core del protocolo (`mcp-core`) y el enrutador HTTP (`mcp-http`) siguen siendo 100% agnósticos de la autenticación. No conocen JWT ni JWKS; simplemente propagan un objeto `Principal` tipado.
* **O (Abierto-Cerrado):** Podemos cambiar el proveedor de identidad de Supabase a Auth0 simplemente actualizando la URL de JWKS en la configuración del entorno, sin tocar una sola línea de código en el repositorio.

## 11. Ejercicios

1. **Guiado.** ¿Qué problemas de seguridad surgen si un token JWT no especifica fecha de expiración (`exp`)? ¿Cómo reacciona `jsonwebtoken` por defecto?
2. **Medio.** Modifica `auth.rs` para permitir múltiples audiencias válidas (p. ej., si tu servidor MCP es compartido por una aplicación web y una extensión de navegador).
3. **Abierto.** Diseña una estrategia para rotar las llaves de firma del JWKS en caliente sin causar errores temporales en peticiones concurrentes de usuarios legítimos.
4. **Abierto.** Con DCR activo, cualquier cliente MCP puede auto-registrarse contra tu Supabase Auth y llegar hasta la pantalla de consentimiento — el usuario sigue teniendo que aprobar, pero nada impide que un cliente malicioso pida scopes amplios con un nombre engañoso. Diseña una estrategia para que `/oauth/consent` muestre suficiente contexto (o registre lo suficiente) como para que esa aprobación sea informada. ¿Le pedirías a `execute_sql`/`auth.oauth_clients` una lista de clientes de confianza, en vez de aceptar cualquier `client_id` dinámico sin más?
