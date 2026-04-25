use crate::common::{read_next, ServerGuard, DdbTestRepo};
use tungstenite::connect;
use tungstenite::http::Request;
use tungstenite::Message;

#[test]
fn subscribe_mutate_receive() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    let mut ws = server
        .ws_subscribe("subscription { doogatChanged { action doogatId doogat { id title } } }");

    // Mutate: create a doogat
    let result = server.graphql_with_vars(
        r#"mutation($input: CreateDoogatInput!) { createDoogat(input: $input) { id title } }"#,
        serde_json::json!({ "input": { "title": "Sub Test", "content": "body" } }),
    );
    assert!(result.get("errors").is_none(), "create failed: {result}");
    let created_id = result["data"]["createDoogat"]["id"].as_str().unwrap();

    // Read the subscription event
    let event = read_next(&mut ws);
    let data = &event["payload"]["data"]["doogatChanged"];
    assert_eq!(data["action"], "created");
    assert_eq!(data["doogatId"], created_id);
    assert_eq!(data["doogat"]["title"], "Sub Test");
}

#[test]
fn subscribe_delete_receive() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    // Create first
    let result = server.graphql_with_vars(
        r#"mutation($input: CreateDoogatInput!) { createDoogat(input: $input) { id } }"#,
        serde_json::json!({ "input": { "title": "To Delete" } }),
    );
    let id = result["data"]["createDoogat"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Subscribe to deletions
    let mut ws = server.ws_subscribe("subscription { doogatDeleted }");

    // Delete
    let result = server.graphql(&format!(r#"mutation {{ deleteDoogat(id: "{id}") }}"#));
    assert!(result.get("errors").is_none(), "delete failed: {result}");

    // Read event
    let event = read_next(&mut ws);
    let deleted_id = &event["payload"]["data"]["doogatDeleted"];
    assert_eq!(deleted_id.as_str().unwrap(), id);
}

#[test]
fn ws_auth_invalid_header_returns_401() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    let request = Request::builder()
        .uri(format!("ws://127.0.0.1:{}/ws", server.port))
        .header("Authorization", "Bearer wrong-token")
        .header("Sec-WebSocket-Protocol", "graphql-transport-ws")
        .header("Host", format!("127.0.0.1:{}", server.port))
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Sec-WebSocket-Version", "13")
        .header(
            "Sec-WebSocket-Key",
            tungstenite::handshake::client::generate_key(),
        )
        .body(())
        .unwrap();

    let result = connect(request);
    // Invalid header → 401 at upgrade time
    assert!(result.is_err(), "WS with bad header should fail");
}

#[test]
fn ws_payload_auth_subscribe_receive() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    let mut ws = server.ws_subscribe_with_payload_auth(
        "subscription { doogatChanged { action doogatId doogat { id title } } }",
    );

    // Mutate via GraphQL
    let result = server.graphql_with_vars(
        r#"mutation($input: CreateDoogatInput!) { createDoogat(input: $input) { id title } }"#,
        serde_json::json!({ "input": { "title": "Payload Auth", "content": "body" } }),
    );
    assert!(result.get("errors").is_none(), "create failed: {result}");
    let created_id = result["data"]["createDoogat"]["id"].as_str().unwrap();

    let event = read_next(&mut ws);
    let data = &event["payload"]["data"]["doogatChanged"];
    assert_eq!(data["action"], "created");
    assert_eq!(data["doogatId"], created_id);
    assert_eq!(data["doogat"]["title"], "Payload Auth");
}

#[test]
fn ws_payload_auth_invalid_token() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    let mut ws = server.ws_connect_no_header();

    // Send connection_init with wrong token
    ws.send(Message::Text(
        serde_json::json!({
            "type": "connection_init",
            "payload": { "Authorization": "Bearer wrong-token" }
        })
        .to_string()
        .into(),
    ))
    .unwrap();

    // Should get an error or connection close, not connection_ack
    let msg = ws.read();
    match msg {
        Ok(Message::Text(text)) => {
            let val: serde_json::Value = serde_json::from_str(&text).unwrap();
            assert_ne!(
                val["type"], "connection_ack",
                "should not ack invalid token"
            );
        }
        Ok(Message::Close(_)) => {} // acceptable: server closed connection
        Err(_) => {}                // acceptable: connection dropped
        other => panic!("unexpected message: {other:?}"),
    }
}

#[test]
fn ws_no_auth_no_payload_rejected() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    let mut ws = server.ws_connect_no_header();

    // Send connection_init with empty payload
    ws.send(Message::Text(
        serde_json::json!({ "type": "connection_init", "payload": {} })
            .to_string()
            .into(),
    ))
    .unwrap();

    // Should get an error or connection close, not connection_ack
    let msg = ws.read();
    match msg {
        Ok(Message::Text(text)) => {
            let val: serde_json::Value = serde_json::from_str(&text).unwrap();
            assert_ne!(
                val["type"], "connection_ack",
                "should not ack empty payload"
            );
        }
        Ok(Message::Close(_)) => {} // acceptable: server closed connection
        Err(_) => {}                // acceptable: connection dropped
        other => panic!("unexpected message: {other:?}"),
    }
}

#[test]
fn no_subscriber_mutations_work() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    // No WS subscribers — just do mutations and verify they succeed
    for i in 0..3 {
        let result = server.graphql_with_vars(
            r#"mutation($input: CreateDoogatInput!) { createDoogat(input: $input) { id } }"#,
            serde_json::json!({ "input": { "title": format!("NoSub {i}") } }),
        );
        assert!(
            result.get("errors").is_none(),
            "create {i} failed: {result}"
        );
    }

    // Verify all created
    let result = server.graphql(r#"{ doogats { id } }"#);
    let list = result["data"]["doogats"].as_array().unwrap();
    assert_eq!(list.len(), 3);
}

#[test]
fn subscribe_type_filter() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    // Create two types. PRD 00129 made typed createDoogat populate the type
    // table and enforce NOT NULL, so leave `name` nullable here — the test
    // only cares about the doogat type, not column constraints.
    for table in ["contact", "bookmark"] {
        let sql = format!("CREATE TABLE {table} (name TEXT)");
        let result = server.graphql_with_vars(
            r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
            serde_json::json!({ "sql": sql }),
        );
        assert!(
            result.get("errors").is_none(),
            "CREATE TABLE {table} failed: {result}"
        );
    }

    // Subscribe to contactChanged only
    let mut ws = server.ws_subscribe("subscription { contactChanged { action doogatId } }");

    // Create a bookmark — should NOT trigger contact subscription
    let result = server.graphql_with_vars(
        r#"mutation($input: CreateDoogatInput!) { createDoogat(input: $input) { id } }"#,
        serde_json::json!({ "input": { "title": "My Bookmark", "type": "bookmark" } }),
    );
    assert!(
        result.get("errors").is_none(),
        "create bookmark failed: {result}"
    );

    // Create a contact — SHOULD trigger
    let result = server.graphql_with_vars(
        r#"mutation($input: CreateDoogatInput!) { createDoogat(input: $input) { id } }"#,
        serde_json::json!({ "input": { "title": "Alice", "type": "contact" } }),
    );
    assert!(
        result.get("errors").is_none(),
        "create contact failed: {result}"
    );
    let contact_id = result["data"]["createDoogat"]["id"].as_str().unwrap();

    // The first event we get should be the contact, not the bookmark
    let event = read_next(&mut ws);
    let data = &event["payload"]["data"]["contactChanged"];
    assert_eq!(data["action"], "created");
    assert_eq!(data["doogatId"], contact_id);
}

#[test]
fn subscribe_disconnect_reconnect() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    // First connection + subscription
    {
        let mut ws = server.ws_subscribe("subscription { doogatChanged { action doogatId } }");
        let result = server.graphql_with_vars(
            r#"mutation($input: CreateDoogatInput!) { createDoogat(input: $input) { id } }"#,
            serde_json::json!({ "input": { "title": "First" } }),
        );
        assert!(result.get("errors").is_none());
        let event = read_next(&mut ws);
        assert_eq!(
            event["payload"]["data"]["doogatChanged"]["action"],
            "created"
        );
        // ws drops here — disconnect
    }

    // Second connection — should work without issues
    let mut ws = server.ws_subscribe("subscription { doogatChanged { action doogatId } }");
    let result = server.graphql_with_vars(
        r#"mutation($input: CreateDoogatInput!) { createDoogat(input: $input) { id } }"#,
        serde_json::json!({ "input": { "title": "Second" } }),
    );
    assert!(result.get("errors").is_none());
    let event = read_next(&mut ws);
    assert_eq!(
        event["payload"]["data"]["doogatChanged"]["action"],
        "created"
    );
}
