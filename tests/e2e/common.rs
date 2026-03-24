use assert_cmd::Command;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicU16, Ordering};
use std::time::Duration;
use tempfile::TempDir;
use tungstenite::http::Request;
use tungstenite::{connect, Message};

/// Locate the `zdb` binary in the workspace target directory.
/// `CARGO_BIN_EXE_zdb` is only set for same-package binaries, and the
/// `assert_cmd::cargo::cargo_bin` function is deprecated, so we resolve
/// the path ourselves.
pub(crate) fn zdb_bin() -> PathBuf {
    if let Ok(p) = std::env::var("CARGO_BIN_EXE_zdb") {
        return PathBuf::from(p);
    }
    let mut dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    dir.pop(); // tests/ -> workspace root
    let target = std::env::var("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| dir.join("target"));
    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    target
        .join(profile)
        .join(format!("zdb{}", std::env::consts::EXE_SUFFIX))
}

static SERVER_PORT_COUNTER: AtomicU16 = AtomicU16::new(19100);

/// RAII guard that starts a `zdb serve` process and kills it on drop.
pub struct ServerGuard {
    child: Child,
    pub port: u16,
    pub pg_port: u16,
    pub token: String,
}

impl ServerGuard {
    pub fn start(repo: &ZdbTestRepo) -> Self {
        let port = SERVER_PORT_COUNTER.fetch_add(1, Ordering::SeqCst);
        let pg_port = SERVER_PORT_COUNTER.fetch_add(1, Ordering::SeqCst);
        let log_dir = repo.path().join(".local/test-logs");

        let mut child = std::process::Command::new(zdb_bin())
            .arg("--repo")
            .arg(repo.path())
            .arg("--log-dir")
            .arg(&log_dir)
            .arg("serve")
            .arg("--port")
            .arg(port.to_string())
            .arg("--pg-port")
            .arg(pg_port.to_string())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to start server");

        let stderr = child.stderr.take().unwrap();
        let reader = BufReader::new(stderr);
        let mut token = String::new();
        let mut http_ready = false;
        let mut pg_ready = false;

        for line in reader.lines() {
            let line = line.unwrap();
            if line.contains("auth token:") {
                if let Some(path) = line.split("auth token: ").nth(1) {
                    token = std::fs::read_to_string(path.trim())
                        .unwrap_or_default()
                        .trim()
                        .to_string();
                }
            }
            if line.contains("pgwire listening on") {
                pg_ready = true;
            } else if line.contains("listening on") {
                http_ready = true;
            }
            if http_ready && pg_ready {
                break;
            }
        }

        // Small delay for the TCP listener to be fully ready
        std::thread::sleep(Duration::from_millis(50));
        Self {
            child,
            port,
            pg_port,
            token,
        }
    }

    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}/graphql", self.port)
    }

    pub fn rest_url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}/rest{path}", self.port)
    }

    pub fn rest_client(&self) -> reqwest::blocking::Client {
        reqwest::blocking::Client::new()
    }

    pub fn rest_get(&self, path: &str) -> reqwest::blocking::Response {
        self.rest_client()
            .get(self.rest_url(path))
            .header("Authorization", format!("Bearer {}", self.token))
            .timeout(Duration::from_secs(5))
            .send()
            .expect("request failed")
    }

    pub fn rest_post(&self, path: &str, body: serde_json::Value) -> reqwest::blocking::Response {
        self.rest_client()
            .post(self.rest_url(path))
            .header("Authorization", format!("Bearer {}", self.token))
            .json(&body)
            .timeout(Duration::from_secs(5))
            .send()
            .expect("request failed")
    }

    pub fn rest_put(&self, path: &str, body: serde_json::Value) -> reqwest::blocking::Response {
        self.rest_client()
            .put(self.rest_url(path))
            .header("Authorization", format!("Bearer {}", self.token))
            .json(&body)
            .timeout(Duration::from_secs(5))
            .send()
            .expect("request failed")
    }

    pub fn rest_delete(&self, path: &str) -> reqwest::blocking::Response {
        self.rest_client()
            .delete(self.rest_url(path))
            .header("Authorization", format!("Bearer {}", self.token))
            .timeout(Duration::from_secs(5))
            .send()
            .expect("request failed")
    }

    pub fn graphql(&self, query: &str) -> serde_json::Value {
        let body = serde_json::json!({ "query": query });
        reqwest::blocking::Client::new()
            .post(self.url())
            .header("Authorization", format!("Bearer {}", self.token))
            .json(&body)
            .timeout(Duration::from_secs(5))
            .send()
            .expect("request failed")
            .json()
            .expect("invalid json")
    }

    pub fn graphql_with_vars(
        &self,
        query: &str,
        variables: serde_json::Value,
    ) -> serde_json::Value {
        let body = serde_json::json!({ "query": query, "variables": variables });
        reqwest::blocking::Client::new()
            .post(self.url())
            .header("Authorization", format!("Bearer {}", self.token))
            .json(&body)
            .timeout(Duration::from_secs(5))
            .send()
            .expect("request failed")
            .json()
            .expect("invalid json")
    }

    /// Open a graphql-ws WebSocket connection with auth.
    pub fn ws_connect(
        &self,
    ) -> tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<std::net::TcpStream>> {
        let request = Request::builder()
            .uri(format!("ws://127.0.0.1:{}/ws", self.port))
            .header("Authorization", format!("Bearer {}", self.token))
            .header("Sec-WebSocket-Protocol", "graphql-transport-ws")
            .header("Host", format!("127.0.0.1:{}", self.port))
            .header("Connection", "Upgrade")
            .header("Upgrade", "websocket")
            .header("Sec-WebSocket-Version", "13")
            .header(
                "Sec-WebSocket-Key",
                tungstenite::handshake::client::generate_key(),
            )
            .body(())
            .unwrap();

        let (ws, _resp) = connect(request).expect("WebSocket connect failed");
        ws
    }

    /// Init graphql-ws protocol and subscribe, returns the connected socket.
    pub fn ws_subscribe(
        &self,
        subscription_query: &str,
    ) -> tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<std::net::TcpStream>> {
        let mut ws = self.ws_connect();

        // connection_init
        ws.send(Message::Text(
            serde_json::json!({"type": "connection_init", "payload": {}})
                .to_string()
                .into(),
        ))
        .unwrap();

        // Wait for connection_ack
        let ack = ws.read().expect("no ack");
        let ack: serde_json::Value = serde_json::from_str(ack.to_text().unwrap()).unwrap();
        assert_eq!(ack["type"], "connection_ack");

        // Subscribe
        ws.send(Message::Text(
            serde_json::json!({
                "type": "subscribe",
                "id": "1",
                "payload": { "query": subscription_query }
            })
            .to_string()
            .into(),
        ))
        .unwrap();

        // Set read timeout to avoid blocking forever
        if let tungstenite::stream::MaybeTlsStream::Plain(ref s) = ws.get_ref() {
            s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        }

        ws
    }

    /// Open a graphql-ws WebSocket connection WITHOUT an Authorization header.
    /// Browser clients can't set headers on WebSocket, so this simulates that.
    pub fn ws_connect_no_header(
        &self,
    ) -> tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<std::net::TcpStream>> {
        let request = Request::builder()
            .uri(format!("ws://127.0.0.1:{}/ws", self.port))
            .header("Sec-WebSocket-Protocol", "graphql-transport-ws")
            .header("Host", format!("127.0.0.1:{}", self.port))
            .header("Connection", "Upgrade")
            .header("Upgrade", "websocket")
            .header("Sec-WebSocket-Version", "13")
            .header(
                "Sec-WebSocket-Key",
                tungstenite::handshake::client::generate_key(),
            )
            .body(())
            .unwrap();

        let (ws, _resp) = connect(request).expect("WebSocket connect failed");
        ws
    }

    /// Connect without header, authenticate via connection_init payload, subscribe.
    pub fn ws_subscribe_with_payload_auth(
        &self,
        subscription_query: &str,
    ) -> tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<std::net::TcpStream>> {
        let mut ws = self.ws_connect_no_header();

        // connection_init with bearer token in payload
        ws.send(Message::Text(
            serde_json::json!({
                "type": "connection_init",
                "payload": { "Authorization": format!("Bearer {}", self.token) }
            })
            .to_string()
            .into(),
        ))
        .unwrap();

        // Wait for connection_ack
        let ack = ws.read().expect("no ack");
        let ack: serde_json::Value = serde_json::from_str(ack.to_text().unwrap()).unwrap();
        assert_eq!(ack["type"], "connection_ack");

        // Subscribe
        ws.send(Message::Text(
            serde_json::json!({
                "type": "subscribe",
                "id": "1",
                "payload": { "query": subscription_query }
            })
            .to_string()
            .into(),
        ))
        .unwrap();

        // Set read timeout to avoid blocking forever
        if let tungstenite::stream::MaybeTlsStream::Plain(ref s) = ws.get_ref() {
            s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        }

        ws
    }
}

impl Drop for ServerGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Read the next "next" message from WS, skipping pings.
pub fn read_next(
    ws: &mut tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<std::net::TcpStream>>,
) -> serde_json::Value {
    loop {
        let msg = ws.read().expect("ws read failed");
        if let Message::Text(text) = msg {
            let val: serde_json::Value = serde_json::from_str(&text).unwrap();
            if val["type"] == "next" {
                return val;
            }
            if val["type"] == "ping" {
                ws.send(Message::Text(
                    serde_json::json!({"type": "pong"}).to_string().into(),
                ))
                .ok();
                continue;
            }
            if val["type"] == "error" {
                panic!("subscription error: {val}");
            }
        }
    }
}

pub struct ZdbTestRepo {
    pub dir: TempDir,
}

impl ZdbTestRepo {
    /// Init a new zettelkasten in a temp dir
    pub fn init() -> Self {
        let dir = TempDir::new().unwrap();
        Self::zdb_at(dir.path())
            .arg("init")
            .arg(dir.path())
            .assert()
            .success();
        Self::disable_git_signing(dir.path());
        Self { dir }
    }

    pub fn zdb_at(path: &Path) -> Command {
        let mut cmd = Command::new(zdb_bin());
        cmd.arg("--repo").arg(path);
        cmd
    }

    /// Run zdb in this repo's dir
    pub fn zdb(&self) -> Command {
        Self::zdb_at(self.path())
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    fn disable_git_signing(path: &Path) {
        let status = std::process::Command::new("git")
            .current_dir(path)
            .args(["config", "commit.gpgsign", "false"])
            .status()
            .expect("failed to run git config");
        assert!(status.success(), "git config commit.gpgsign failed");
    }
}

/// Two-node setup: bare remote + two working repos
pub struct TwoNodeSetup {
    pub remote_dir: TempDir,
    pub node1: ZdbTestRepo,
    pub node2_dir: TempDir,
}

impl TwoNodeSetup {
    pub fn new() -> Self {
        // Bare remote
        let remote_dir = TempDir::new().unwrap();
        std::process::Command::new("git")
            .args(["init", "--bare"])
            .arg(remote_dir.path())
            .output()
            .unwrap();

        // Node 1
        let node1 = ZdbTestRepo::init();
        std::process::Command::new("git")
            .current_dir(node1.path())
            .args(["remote", "add", "origin"])
            .arg(remote_dir.path())
            .output()
            .unwrap();
        ZdbTestRepo::zdb_at(node1.path())
            .args(["register-node", "Laptop"])
            .assert()
            .success();

        // Node 2 dir (clone happens after node1 pushes)
        let node2_dir = TempDir::new().unwrap();

        Self {
            remote_dir,
            node1,
            node2_dir,
        }
    }

    /// Clone remote into node2 and register it
    pub fn clone_node2(&self) -> PathBuf {
        let node2_path = self.node2_dir.path().join("repo");
        std::process::Command::new("git")
            .args(["clone"])
            .arg(self.remote_dir.path())
            .arg(&node2_path)
            .output()
            .unwrap();
        ZdbTestRepo::disable_git_signing(&node2_path);
        ZdbTestRepo::zdb_at(&node2_path)
            .args(["register-node", "Desktop"])
            .assert()
            .success();
        node2_path
    }
}

/// N-node setup: bare remote + N registered working repos
pub struct MultiNodeSetup {
    pub remote_dir: TempDir,
    pub nodes: Vec<PathBuf>,
    _temps: Vec<TempDir>,
}

impl MultiNodeSetup {
    /// Create a bare remote and N nodes.
    /// Node 0 initializes the repo and pushes; nodes 1..N clone.
    pub fn new(n: usize) -> Self {
        assert!(n >= 2, "need at least 2 nodes");

        let remote_dir = TempDir::new().unwrap();
        std::process::Command::new("git")
            .args(["init", "--bare"])
            .arg(remote_dir.path())
            .output()
            .unwrap();

        let mut nodes = Vec::with_capacity(n);
        let mut temps = Vec::with_capacity(n);

        // Node 0: init + push
        let dir0 = TempDir::new().unwrap();
        let path0 = dir0.path().to_path_buf();
        ZdbTestRepo::zdb_at(&path0)
            .arg("init")
            .arg(&path0)
            .assert()
            .success();
        ZdbTestRepo::disable_git_signing(&path0);
        std::process::Command::new("git")
            .current_dir(&path0)
            .args(["remote", "add", "origin"])
            .arg(remote_dir.path())
            .output()
            .unwrap();
        ZdbTestRepo::zdb_at(&path0)
            .args(["register-node", "Node-0"])
            .assert()
            .success();
        Self::git_push(&path0);
        nodes.push(path0);
        temps.push(dir0);

        // Nodes 1..N: clone + register
        for i in 1..n {
            let dir = TempDir::new().unwrap();
            let path = dir.path().join("repo");
            std::process::Command::new("git")
                .args(["clone"])
                .arg(remote_dir.path())
                .arg(&path)
                .output()
                .unwrap();
            ZdbTestRepo::disable_git_signing(&path);
            ZdbTestRepo::zdb_at(&path)
                .args(["register-node", &format!("Node-{i}")])
                .assert()
                .success();
            nodes.push(path);
            temps.push(dir);
        }

        Self {
            remote_dir,
            nodes,
            _temps: temps,
        }
    }

    /// Push from a node to origin
    pub fn push(node: &Path) {
        Self::git_push(node);
    }

    /// Sync a node
    pub fn sync(node: &Path) {
        ZdbTestRepo::zdb_at(node).arg("sync").assert().success();
    }

    /// Create a zettel on a node, return its ID
    pub fn create(node: &Path, title: &str, body: &str) -> String {
        let out = ZdbTestRepo::zdb_at(node)
            .args(["create", "--title", title, "--body", body])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "create failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// Read a zettel, return stdout
    pub fn read(node: &Path, id: &str) -> String {
        let out = ZdbTestRepo::zdb_at(node)
            .args(["read", id])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "read failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).to_string()
    }

    /// Update a zettel
    pub fn update(node: &Path, id: &str, title: &str, body: &str) {
        ZdbTestRepo::zdb_at(node)
            .args(["update", id, "--title", title, "--body", body])
            .assert()
            .success();
    }

    /// Delete a zettel
    pub fn delete(node: &Path, id: &str) {
        ZdbTestRepo::zdb_at(node)
            .args(["delete", id])
            .assert()
            .success();
    }

    fn git_push(path: &Path) {
        std::process::Command::new("git")
            .current_dir(path)
            .args(["push", "-u", "origin", "master"])
            .output()
            .unwrap();
    }
}
