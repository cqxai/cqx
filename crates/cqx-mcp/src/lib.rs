//! A read-only MCP server, so an agent can ask cqx about the code.
//!
//! The score is computed locally in about two seconds for a 130k-line
//! repository. A hosted call would spend 255ms of latency before any work
//! started. The whole point is that the agent talks to the tool that is
//! already there.
//!
//! Nothing here writes a file, changes a rule, or runs anything. A later
//! change adds `propose_rule`; this one must not.

mod tools;

use std::io::{self, BufRead, Write};
use std::path::PathBuf;

use deka_cli_core::{CommandSpec, Context, Registry};
use serde_json::{json, Value};

pub const MCP_COMMAND: CommandSpec = CommandSpec {
    name: "mcp",
    owner: "cqx-mcp",
    category: "index",
    summary: "Answer an agent about this tree over MCP, without writing anything",
    aliases: &[],
    subcommands: &[],
    handler: cmd_mcp,
};

pub fn register(registry: &mut Registry) {
    registry.add_command(MCP_COMMAND);
}

fn cmd_mcp(_context: &Context) {
    // stdin closing is how the client says goodbye. A broken pipe on stdout
    // is the same thing seen from the other end. Neither is a failure of
    // the command — and writing the error to stdout would corrupt the
    // stream we were in the middle of.
    let _ = serve(io::stdin().lock(), io::stdout());
}

/// One JSON object per line on stdin, one per line on stdout. Nothing else
/// may ever be written to stdout: a stray line corrupts the stream and the
/// failure looks like the agent going mad rather than like a log.
fn serve<R: BufRead, W: Write>(reader: R, mut writer: W) -> io::Result<()> {
    let mut server = Server::default();
    for line in reader.lines() {
        let line = match line {
            Ok(line) => line,
            // The client went away mid-line. That is shutdown, not a bad
            // message: a malformed JSON line is handled below and does not
            // reach here.
            Err(_) => break,
        };
        if let Some(reply) = handle_line(&mut server, &line) {
            writeln!(writer, "{reply}")?;
            writer.flush()?;
        }
    }
    Ok(())
}

/// Last tree we scored. Keyed only on the canonical root: two calls about
/// the same path in one session must not walk the tree twice, and watching
/// mtimes can wait. An agent that asks `score` and then `explain` is the
/// common case, not a different repository.
#[derive(Default)]
struct Server {
    cached: Option<(PathBuf, cqx_scan::Scanned)>,
}

impl Server {
    /// The report for this `path`, scanning only when the root is new.
    fn report_for(&mut self, args: &Value) -> Result<Value, String> {
        Ok(self.scanned(root_from(args)?)?.report.clone())
    }

    fn scanned(&mut self, root: PathBuf) -> Result<&cqx_scan::Scanned, String> {
        let root = root
            .canonicalize()
            .map_err(|e| format!("could not read {}: {e}", root.display()))?;
        let hit = self.cached.as_ref().is_some_and(|(have, _)| *have == root);
        if !hit {
            // `tree` searches upward when this is `None`. Passing a path
            // that is not there would fail the read, so only the file that
            // actually exists is handed over.
            let rules = {
                let candidate = root.join("cqx.json");
                if candidate.is_file() {
                    Some(candidate)
                } else {
                    None
                }
            };
            // Quote so a finding arrives with the line of code. An agent
            // that has to open the file to see what it was asked about is
            // doing the work this tool exists to save.
            let scanned = cqx_scan::tree(&root, rules.as_deref(), true)?;
            self.cached = Some((root, scanned));
        }
        Ok(&self
            .cached
            .as_ref()
            .expect("cache is filled on this path")
            .1)
    }
}

fn root_from(args: &Value) -> Result<PathBuf, String> {
    match args.get("path").and_then(Value::as_str) {
        Some(path) if !path.is_empty() => Ok(PathBuf::from(path)),
        _ => std::env::current_dir()
            .map_err(|e| format!("could not read the current directory: {e}")),
    }
}

/// One JSON-RPC message in, at most one line out.
///
/// A notification has no `id` and must not be answered: a reply to one is a
/// protocol error. The loop that calls this never exits on a bad message;
/// it returns an error *response* and waits for the next line.
fn handle_line(server: &mut Server, line: &str) -> Option<String> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }

    let parsed: Value = match serde_json::from_str(line) {
        Ok(value) => value,
        Err(_) => return Some(error_reply(Value::Null, -32700, "Parse error")),
    };

    let Some(obj) = parsed.as_object() else {
        return Some(error_reply(Value::Null, -32700, "Parse error"));
    };

    // Missing `id` is a notification. `id: null` is a request, and JSON-RPC
    // says the response must echo it — which is how a parse error, where
    // we never saw an id, is also answered.
    let id = obj.get("id").cloned()?;

    let method = obj.get("method").and_then(Value::as_str).unwrap_or("");
    let params = obj.get("params").cloned().unwrap_or(json!({}));

    let result = match method {
        "initialize" => initialize_result(),
        "tools/list" => tools::list(),
        "tools/call" => tools::call(server, &params),
        _ => return Some(error_reply(id, -32601, "Method not found")),
    };
    Some(ok_reply(id, result))
}

fn initialize_result() -> Value {
    json!({
        "protocolVersion": "2025-06-18",
        "capabilities": { "tools": {} },
        "serverInfo": {
            "name": "cqx",
            "version": env!("CARGO_PKG_VERSION"),
        },
    })
}

fn ok_reply(id: Value, result: Value) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string()
}

fn error_reply(id: Value, code: i64, message: &str) -> String {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message },
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn rpc(server: &mut Server, line: &str) -> Value {
        let reply = handle_line(server, line).expect("expected a reply");
        serde_json::from_str(&reply).expect("reply is json")
    }

    fn fixture(name: &str) -> String {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures")
            .join(name)
            .to_string_lossy()
            .into_owned()
    }

    fn call(server: &mut Server, id: u64, name: &str, arguments: Value) -> Value {
        rpc(
            server,
            &json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": "tools/call",
                "params": { "name": name, "arguments": arguments },
            })
            .to_string(),
        )
    }

    fn tool_text(reply: &Value) -> &str {
        reply["result"]["content"][0]["text"]
            .as_str()
            .expect("tool result text")
    }

    #[test]
    fn initialize_returns_protocol_version_and_server_name() {
        let reply = rpc(
            &mut Server::default(),
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#,
        );
        assert_eq!(reply["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(reply["result"]["serverInfo"]["name"], "cqx");
        assert_eq!(
            reply["result"]["serverInfo"]["version"],
            env!("CARGO_PKG_VERSION")
        );
        assert_eq!(reply["result"]["capabilities"], json!({ "tools": {} }));
    }

    #[test]
    fn notification_produces_no_reply() {
        assert!(handle_line(
            &mut Server::default(),
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        )
        .is_none());
    }

    #[test]
    fn tools_list_has_exactly_four_tools_each_with_a_schema() {
        let reply = rpc(
            &mut Server::default(),
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
        );
        let tools = reply["result"]["tools"].as_array().expect("tools array");
        assert_eq!(tools.len(), 4);
        let names: Vec<&str> = tools
            .iter()
            .map(|t| t["name"].as_str().expect("name"))
            .collect();
        assert_eq!(names, ["score", "findings", "rules", "explain"]);
        for tool in tools {
            let schema = tool.get("inputSchema").expect("inputSchema");
            assert_eq!(schema["type"], "object");
            assert!(schema.get("properties").is_some());
        }
    }

    #[test]
    fn unknown_method_is_not_found_and_the_loop_continues() {
        let mut server = Server::default();
        let err = rpc(&mut server, r#"{"jsonrpc":"2.0","id":1,"method":"nope"}"#);
        assert_eq!(err["error"]["code"], -32601);
        // A method that is not found is a reply, not a shutdown: the next
        // message still gets an answer. That is what "the loop continues"
        // has to mean when the tests cannot start a process.
        let ok = rpc(
            &mut server,
            r#"{"jsonrpc":"2.0","id":2,"method":"initialize"}"#,
        );
        assert_eq!(ok["result"]["serverInfo"]["name"], "cqx");
    }

    #[test]
    fn malformed_line_is_parse_error() {
        let mut server = Server::default();
        let err = rpc(&mut server, "this is not json");
        assert_eq!(err["error"]["code"], -32700);
        assert_eq!(err["id"], Value::Null);
        let ok = rpc(
            &mut server,
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#,
        );
        assert_eq!(ok["result"]["serverInfo"]["name"], "cqx");
    }

    #[test]
    fn score_on_fixtures_basic_returns_scores() {
        let reply = call(
            &mut Server::default(),
            1,
            "score",
            json!({ "path": fixture("basic") }),
        );
        assert!(reply.get("error").is_none(), "{reply}");
        assert_ne!(reply["result"].get("isError"), Some(&Value::Bool(true)));
        let body: Value = serde_json::from_str(tool_text(&reply)).expect("pretty json");
        let scores = body["scores"].as_object().expect("scores");
        assert!(!scores.is_empty());
        assert!(scores.values().all(Value::is_number));
        assert!(body["scan"]["files"].as_u64().unwrap() > 0);
        assert!(body["scan"].get("lines").is_some());
        assert!(body["scan"].get("ms").is_some());
        assert!(body["scan"].get("cqx").is_some());
    }

    #[test]
    fn explain_unknown_rule_is_a_tool_error_not_a_protocol_error() {
        let reply = call(
            &mut Server::default(),
            1,
            "explain",
            json!({
                "path": fixture("basic"),
                "rule": "no-such-rule",
            }),
        );
        assert!(
            reply.get("error").is_none(),
            "a missing rule is a tool failure, not a JSON-RPC error: {reply}"
        );
        assert_eq!(reply["result"]["isError"], true);
        let text = tool_text(&reply);
        assert!(
            text.contains("no-such-rule"),
            "the sentence should name the rule the agent asked about: {text}"
        );
        assert!(
            text.contains(' ') && !text.trim_start().starts_with('{'),
            "a person should be able to read this, not parse it: {text}"
        );
    }

    #[test]
    fn findings_file_filter_returns_only_that_file() {
        let reply = call(
            &mut Server::default(),
            1,
            "findings",
            json!({
                "path": fixture("hard"),
                "file": "crates/hard/src/h3_const.rs",
            }),
        );
        assert!(reply.get("error").is_none(), "{reply}");
        let body: Value = serde_json::from_str(tool_text(&reply)).expect("pretty json");
        let findings = body["findings"].as_array().expect("findings");
        assert!(
            !findings.is_empty(),
            "fixtures/hard plants findings in h3_const.rs"
        );
        for finding in findings {
            let file = finding["file"].as_str().expect("file");
            assert_eq!(file, "crates/hard/src/h3_const.rs", "{finding}");
            assert!(finding.get("rule").and_then(Value::as_str).is_some());
            assert!(finding.get("category").and_then(Value::as_str).is_some());
        }
    }
}
