//! Test de integración: conversación MCP completa contra el servidor,
//! exactamente como la tendría un cliente real (Claude, Cursor, etc.),
//! mensaje JSON a mensaje JSON.

use json_mini::Value;
use mcp_core::McpServer;
use memory_tools::MemoryTools;
use memory_model::{Budget, Principal};
use memory_store::InMemoryStore;

fn server() -> McpServer<MemoryTools<InMemoryStore>> {
    let tools = MemoryTools::new(InMemoryStore::new(), Principal::local_dev(), Budget::default());
    McpServer::new("okf-memory", "test", tools)
}

/// Envía y parsea la respuesta; falla si no la hay.
fn send(server: &mut McpServer<MemoryTools<InMemoryStore>>, msg: &str) -> Value {
    let raw = server.handle_message(msg).expect("se esperaba respuesta");
    json_mini::parse(&raw).expect("respuesta JSON válida")
}

/// Extrae el payload textual de un CallToolResult y lo parsea.
fn tool_payload(response: &Value) -> (Value, bool) {
    let result = response.get("result").expect("result presente");
    let is_error = result.get("isError").and_then(|v| v.as_bool()).unwrap();
    let text = result.get("content").unwrap().as_array().unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();
    (json_mini::parse(text).expect("payload JSON"), is_error)
}

fn call_tool(server: &mut McpServer<MemoryTools<InMemoryStore>>, id: u32, name: &str, args: &str) -> (Value, bool) {
    let msg = format!(
        r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call","params":{{"name":"{name}","arguments":{args}}}}}"#
    );
    let resp = send(server, &msg);
    tool_payload(&resp)
}

#[test]
fn conversacion_completa() {
    let mut srv = server();

    // 1. initialize + initialized
    let resp = send(&mut srv, r#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"0"}}}"#);
    assert!(resp.get("result").is_some());
    assert!(srv.handle_message(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).is_none());

    // 2. tools/list expone las cuatro herramientas
    let resp = send(&mut srv, r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#);
    let tools = resp.get("result").unwrap().get("tools").unwrap().as_array().unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t.get("name").unwrap().as_str().unwrap()).collect();
    assert_eq!(names.len(), 4);
    for expected in ["memory_search", "memory_resolve", "memory_commit", "memory_history"] {
        assert!(names.contains(&expected), "falta {expected}");
    }

    // 3. Crear dos documentos enlazados
    let alice = r#"{"concept_id":"people/alice","reason":"alta inicial","markdown":"---\ntype: person\ntitle: Alice\ntags:\n  - rust\n---\nTrabaja en [[projects/okf-mcp]].\n"}"#;
    let (payload, is_error) = call_tool(&mut srv, 2, "memory_commit", alice);
    assert!(!is_error, "commit falló: {payload:?}");
    assert_eq!(payload.get("created"), Some(&Value::Bool(true)));
    let alice_hash = payload.get("hash").unwrap().as_str().unwrap().to_string();

    let project = r#"{"concept_id":"projects/okf-mcp","reason":"alta inicial","markdown":"---\ntype: project\ntitle: OKF MCP\n---\nServidor de memoria en Rust.\n"}"#;
    let (_, is_error) = call_tool(&mut srv, 3, "memory_commit", project);
    assert!(!is_error);

    // 4. Buscar
    let (payload, is_error) = call_tool(&mut srv, 4, "memory_search", r#"{"query":"rust"}"#);
    assert!(!is_error);
    assert_eq!(payload.get("count"), Some(&Value::Number(2.0)));

    // 5. Resolver con vecindario
    let (payload, is_error) = call_tool(&mut srv, 5, "memory_resolve", r#"{"concept_id":"people/alice"}"#);
    assert!(!is_error);
    let doc = payload.get("document").unwrap();
    assert!(doc.get("markdown").unwrap().as_str().unwrap().contains("[[projects/okf-mcp]]"));
    let vecinos = payload.get("neighborhood").unwrap().as_array().unwrap();
    assert_eq!(vecinos.len(), 1);
    assert_eq!(vecinos[0].get("concept_id").unwrap().as_str(), Some("projects/okf-mcp"));

    // 6. Conflicto CAS: escribir con un hash obsoleto
    let update_ok = format!(
        r#"{{"concept_id":"people/alice","expected_hash":"{alice_hash}","reason":"actualizo rol","markdown":"---\ntype: person\ntitle: Alice\n---\nAhora es staff.\n"}}"#
    );
    let (_, is_error) = call_tool(&mut srv, 6, "memory_commit", &update_ok);
    assert!(!is_error);

    // El mismo expected_hash otra vez: ya no es la cabeza → conflicto
    let update_stale = format!(
        r#"{{"concept_id":"people/alice","expected_hash":"{alice_hash}","reason":"pisotón","markdown":"---\ntype: person\ntitle: Alice\n---\nEscritura perdida.\n"}}"#
    );
    let (payload, is_error) = call_tool(&mut srv, 7, "memory_commit", &update_stale);
    assert!(is_error, "una base obsoleta debe ser conflicto");
    assert_eq!(payload.get("kind").unwrap().as_str(), Some("revision_conflict"));
    assert!(payload.get("current_hash").unwrap().as_str().is_some());

    // 7. La historia registra las dos revisiones reales, no el pisotón
    let (payload, is_error) = call_tool(&mut srv, 8, "memory_history", r#"{"concept_id":"people/alice"}"#);
    assert!(!is_error);
    assert_eq!(payload.get("revisions").unwrap().as_array().unwrap().len(), 2);
}

#[test]
fn entradas_hostiles() {
    let mut srv = server();

    // concept_id con traversal: InvalidArguments se convierte en
    // error de protocolo -32602, así que inspeccionamos la respuesta
    // cruda (no hay CallToolResult que parsear).
    let resp = srv.handle_message(
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"memory_resolve","arguments":{"concept_id":"../../etc/passwd"}}}"#,
    ).unwrap();
    assert!(resp.contains("-32602"), "traversal debe rechazarse: {resp}");

    // markdown sin frontmatter → fallo de dominio legible
    let (payload, is_error) = call_tool(
        &mut srv, 3, "memory_commit",
        r#"{"concept_id":"n","reason":"x","markdown":"sin frontmatter"}"#,
    );
    assert!(is_error);
    assert_eq!(payload.get("kind").unwrap().as_str(), Some("invalid_okf_document"));

    // concepto inexistente
    let (payload, is_error) = call_tool(&mut srv, 4, "memory_resolve", r#"{"concept_id":"no/existe"}"#);
    assert!(is_error);
    assert_eq!(payload.get("kind").unwrap().as_str(), Some("not_found"));
}
