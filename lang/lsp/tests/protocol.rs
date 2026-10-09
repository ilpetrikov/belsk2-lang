#![allow(clippy::indexing_slicing)] // indexing a JSON value never panics
//! Talks to the server over an in-memory connection, like an editor would.

use lsp_server::{Connection, Message, Notification, Request, RequestId};
use serde_json::{json, Value};

fn request(client: &Connection, id: i32, method: &str, params: Value) -> Value {
    let req = Request::new(RequestId::from(id), method.to_string(), params);
    let _ = client.sender.send(Message::Request(req));
    for msg in &client.receiver {
        if let Message::Response(r) = msg {
            if r.id == RequestId::from(id) {
                return r.result.unwrap_or(Value::Null);
            }
        }
    }
    Value::Null
}

fn notify(client: &Connection, method: &str, params: Value) {
    let _ = client.sender.send(Message::Notification(Notification::new(
        method.to_string(),
        params,
    )));
}

#[test]
fn editor_session() {
    let (server, client) = Connection::memory();
    let thread = std::thread::spawn(move || belsk2_lsp::serve(&server));

    let init = request(&client, 1, "initialize", json!({ "capabilities": {} }));
    assert_eq!(init["capabilities"]["hoverProvider"], json!(true));
    notify(&client, "initialized", json!({}));

    let dir = std::env::temp_dir().join("belsk2-lsp-tests");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("demo.belsk2");
    let _ = std::fs::write(&path, "");
    let uri = lsp_types::Url::from_file_path(&path)
        .map(|u| u.to_string())
        .unwrap_or_default();
    let text = "fn main() {\n    var n = len(\"héllo\");\n    prinb(n - \"a\");\n}\n";
    notify(
        &client,
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": uri, "languageId": "belsk2", "version": 1, "text": text } }),
    );
    let diags = client.receiver.iter().find_map(|m| match m {
        Message::Notification(n) if n.method == "textDocument/publishDiagnostics" => Some(n.params),
        _ => None,
    });
    let diags = diags.unwrap_or(Value::Null);
    assert_eq!(
        diags["diagnostics"][0]["range"]["start"],
        json!({ "line": 2, "character": 12 }),
        "{diags}"
    );

    let hover = request(
        &client,
        2,
        "textDocument/hover",
        json!({ "textDocument": { "uri": uri }, "position": { "line": 2, "character": 10 } }),
    );
    assert!(
        hover["contents"]["value"]
            .as_str()
            .unwrap_or("")
            .contains("var n: int"),
        "{hover}"
    );

    let hints = request(
        &client,
        3,
        "textDocument/inlayHint",
        json!({ "textDocument": { "uri": uri }, "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 5, "character": 0 } } }),
    );
    assert_eq!(hints[0]["label"], json!(": int"), "{hints}");

    let _ = request(&client, 4, "shutdown", Value::Null);
    notify(&client, "exit", Value::Null);
    assert!(thread.join().is_ok_and(|r| r.is_ok()));
}
