//! The MCP tests' rig: `signal mcp` spawned on a copy of a project, spoken
//! to over its stdin and stdout as an MCP client does. Every line it writes
//! on stdout is read as one JSON-RPC message (AC2: nothing else may appear
//! there). The null driver only; nothing is played (ADR 0029 §3).

#![allow(dead_code)]

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};

/// A folder of its own for test `name`.
pub fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("mcp-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// The repository's root.
pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Copies the folder `from` to `to` (files and folders only).
pub fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let path = entry.unwrap().path();
        let target = to.join(path.file_name().unwrap());
        if path.is_dir() {
            copy_dir(&path, &target);
        } else {
            fs::copy(&path, &target).unwrap();
        }
    }
}

/// A copy of the repository's `projects/<name>` (tests never write the
/// committed projects).
pub fn copy_of(project: &str, name: &str) -> PathBuf {
    let dir = scratch(name).join("project");
    copy_dir(&root().join("projects").join(project), &dir);
    dir
}

/// A copy of the fixture `crates/signal-cli/tests/fixtures/<fixture>`.
pub fn fixture(fixture: &str, name: &str) -> PathBuf {
    let dir = scratch(name).join("project");
    copy_dir(
        &Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(fixture),
        &dir,
    );
    dir
}

/// A project folder holding `text` as its project.toml.
pub fn project(text: &str, name: &str) -> PathBuf {
    let dir = scratch(name).join("project");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("project.toml"), text).unwrap();
    dir
}

/// A running `signal mcp`.
pub struct Mcp {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
    next: u64,
    pub dir: PathBuf,
    pub out: PathBuf,
}

fn reader(stdout: ChildStdout) -> Receiver<String> {
    let (tx, rx) = channel();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    rx
}

impl Mcp {
    /// `signal mcp <dir> [extra…]`, its renders and sessions in a folder
    /// beside the project.
    pub fn start(dir: &Path, extra: &[&str]) -> Mcp {
        let base = dir.parent().unwrap().to_owned();
        let out = base.join("out");
        let mut child = Command::new(env!("CARGO_BIN_EXE_signal"))
            .arg("--setup")
            .arg(base.join("setup.toml"))
            .arg("mcp")
            .arg(dir)
            .arg("--out")
            .arg(&out)
            .args(extra)
            .env("SIGNAL_SETUP", base.join("setup.toml"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("signal mcp starts");
        let lines = reader(child.stdout.take().unwrap());
        Mcp {
            stdin: child.stdin.take(),
            child,
            lines,
            next: 0,
            dir: dir.to_owned(),
            out,
        }
    }

    /// `start`, then the legacy handshake (`initialize`, `initialized`).
    pub fn started(dir: &Path, extra: &[&str]) -> Mcp {
        let mut mcp = Mcp::start(dir, extra);
        let init = mcp.rpc(
            "initialize",
            json!({"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "test", "version": "1"}}),
        );
        assert_eq!(init["result"]["protocolVersion"], "2025-11-25", "{init}");
        mcp.notify("notifications/initialized", json!({}));
        mcp
    }

    pub fn write_line(&mut self, line: &str) {
        let stdin = self.stdin.as_mut().unwrap();
        writeln!(stdin, "{line}").unwrap();
        stdin.flush().unwrap();
    }

    /// The next message on stdout, which must be one JSON-RPC 2.0 object.
    pub fn line(&mut self) -> Value {
        let line = self
            .lines
            .recv_timeout(Duration::from_secs(120))
            .expect("a line on stdout within 120 s");
        let value: Value = serde_json::from_str(&line)
            .unwrap_or_else(|e| panic!("stdout carries only JSON-RPC frames, not {line:?}: {e}"));
        assert_eq!(value["jsonrpc"], "2.0", "{value}");
        value
    }

    pub fn notify(&mut self, method: &str, params: Value) {
        let line = json!({"jsonrpc": "2.0", "method": method, "params": params}).to_string();
        self.write_line(&line);
    }

    /// A request and its whole response.
    pub fn rpc(&mut self, method: &str, params: Value) -> Value {
        self.next += 1;
        let id = self.next;
        let line =
            json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string();
        self.write_line(&line);
        let reply = self.line();
        assert_eq!(reply["id"], id, "{reply}");
        reply
    }

    /// `tools/call`'s result.
    pub fn call(&mut self, name: &str, arguments: Value) -> Value {
        let reply = self.rpc("tools/call", json!({"name": name, "arguments": arguments}));
        assert!(reply.get("error").is_none(), "{name}: {reply}");
        let result = reply["result"].clone();
        assert!(result["content"][0]["type"] == "text", "{result}");
        result
    }

    /// A call that must succeed: its structured result.
    pub fn ok(&mut self, name: &str, arguments: Value) -> Value {
        let result = self.call(name, arguments);
        assert_eq!(result["isError"], false, "{name}: {result}");
        assert!(
            !result["content"][0]["text"].as_str().unwrap().is_empty(),
            "a summary"
        );
        result["structuredContent"].clone()
    }

    /// A call that must be refused: its message.
    pub fn refused(&mut self, name: &str, arguments: Value) -> String {
        let result = self.call(name, arguments);
        assert_eq!(result["isError"], true, "{name}: {result}");
        result["content"][0]["text"].as_str().unwrap().to_owned()
    }

    /// The structured detail of a refusal (the server's error).
    pub fn refusal(&mut self, name: &str, arguments: Value) -> Value {
        let result = self.call(name, arguments);
        assert_eq!(result["isError"], true, "{name}: {result}");
        result["structuredContent"]["error"].clone()
    }

    /// The project's file `file`.
    pub fn file(&self, file: &str) -> String {
        fs::read_to_string(self.dir.join(file)).unwrap()
    }

    /// Closes stdin (the MCP shutdown) and waits for the exit; nothing more
    /// may have been written on stdout.
    pub fn finish(mut self) -> ExitStatus {
        drop(self.stdin.take());
        let status = self.child.wait().unwrap();
        if let Ok(line) = self.lines.recv_timeout(Duration::from_millis(200)) {
            panic!("stdout after the last reply: {line:?}");
        }
        status
    }
}

impl Drop for Mcp {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A `base` no file has: a write naming it is a conflict.
pub fn stale() -> String {
    "0".repeat(64)
}
