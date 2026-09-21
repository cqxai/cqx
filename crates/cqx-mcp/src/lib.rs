//! A read-only MCP server, so an agent can ask cqx about the code.
//!
//! The score is computed locally in about two seconds for a 130k-line
//! repository. A hosted call would spend 255ms of latency before any work
//! started. The whole point is that the agent talks to the tool that is
//! already there.
//!
//! Four of the five tools are read-only. The fifth, `propose_rule`, writes
//! exactly one file — `cqx.json` — and only ever in the direction that makes
//! a rule stricter. **An agent may tighten a rule; only a person may loosen
//! one.** Without that asymmetry, "make the score go up" has two solutions and
//! the faster one is to lower the bar.
//!
//! Nothing here runs anything, and nothing here reads a file outside the tree
//! it was pointed at.

mod tools;

/// What can go wrong answering an agent.
///
/// Not `Result<_, String>`, and cqx itself is the reason: `result-string-density`
/// found this crate the moment it was written, which is the loop this whole
/// product is about — the tool said so before a person had to.
///
/// It is also better. `explain` failing because a rule does not exist and
/// `explain` failing because the folder cannot be read are different answers
/// to an agent, and as prose they were distinguishable only by reading the
/// sentence. Now they can be matched on, which is what will let a later
/// version answer "did you mean exit-in-library?" without parsing its own
/// error message.
#[derive(Debug)]
pub enum Trouble {
    /// The path given is not something we can read as a directory.
    Unreadable { path: PathBuf, why: String },
    /// The analysis itself refused. Carries what it said, because it is the
    /// only thing that knows why.
    Analysis(String),
    /// The call left out something it has to give.
    Missing(&'static str),
    /// It named a rule this tree does not have.
    NoSuchRule(String),
    /// It asked to change a rule in a direction it may not change it in.
    ///
    /// Its own variant rather than a sentence, because the answer has
    /// structure worth giving an agent: which objections, whether any of them
    /// is an actual loosening, and whether the proposal would have changed
    /// anything at all. A caller that cannot tell "this would lower the
    /// standard" from "this is already the value" is a caller that will retry
    /// the second one forever.
    NotATightening {
        rule: String,
        field: String,
        objections: Vec<String>,
        /// At least one objection is a loosening rather than merely unclear.
        /// The two deserve different sentences: "you are lowering the bar" is
        /// an accusation, and "this cannot be shown to raise it" is not.
        loosening: bool,
        nothing: bool,
    },
}

impl std::fmt::Display for Trouble {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Trouble::Unreadable { path, why } => write!(f, "could not read {}: {why}", path.display()),
            Trouble::Analysis(why) => f.write_str(why),
            Trouble::Missing(what) => write!(f, "{what}"),
            Trouble::NoSuchRule(name) => write!(
                f,
                "there is no rule named '{name}' in this tree. Call rules to see the ones in force."
            ),
            Trouble::NotATightening { rule, field, objections, loosening, nothing } => {
                if *nothing {
                    return write!(f, "{rule}.{field} is already that. Nothing to change.");
                }
                writeln!(
                    f,
                    "{}",
                    if *loosening {
                        "Refused: this would lower the standard, and only a person may do that."
                    } else {
                        "Refused: this cannot be shown to raise the standard, so only a person may make it."
                    }
                )?;
                for objection in objections {
                    writeln!(f, "  {rule}.{objection}")?;
                }
                write!(
                    f,
                    "You may make {rule} stricter. If this change is the right one, say so to the person you are working with and let them run: cqx config set {rule}.{field} <value>"
                )
            }
        }
    }
}

impl std::error::Error for Trouble {}

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

/// One conversation with one client.
///
/// Public because stdio is not the only way to reach this. The desktop
/// application serves the same tools over HTTP on a port, so that an agent
/// talks to the process that already has the tree warm instead of starting a
/// fresh one per question — and it must be the *same* dispatch, not a second
/// implementation that answers `tools/list` slightly differently.
///
/// It also holds the last tree scored, keyed on the canonical root: two calls
/// about the same path must not walk it twice. An agent that asks `score` and
/// then `explain` is the common case, not a different repository.
#[derive(Default)]
pub struct Session {
    cached: Option<(PathBuf, cqx_scan::Scanned)>,
}

/// The name this crate has used internally since it was written.
type Server = Session;

impl Session {
    pub fn new() -> Self {
        Self::default()
    }

    /// One JSON-RPC message in, at most one line out.
    ///
    /// `None` means the message was a notification and must not be answered.
    /// A caller serving HTTP replies `202 Accepted` with no body in that case;
    /// a caller serving stdio writes nothing.
    pub fn handle(&mut self, line: &str) -> Option<String> {
        handle_line(self, line)
    }
}

impl Server {
    /// The report for this `path`, scanning only when the root is new.
    fn report_for(&mut self, args: &Value) -> Result<Value, Trouble> {
        Ok(self.scanned(root_from(args)?)?.report.clone())
    }

    /// Throw the cached scan away.
    ///
    /// Wanted when the rules change: the tree is identical and the score is
    /// not, and a cache keyed on the root knows nothing about which rules
    /// were applied to it.
    pub(crate) fn forget(&mut self) {
        self.cached = None;
    }

    fn scanned(&mut self, root: PathBuf) -> Result<&cqx_scan::Scanned, Trouble> {
        let root = root.canonicalize().map_err(|e| Trouble::Unreadable {
            path: root.clone(),
            why: e.to_string(),
        })?;
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
            let scanned = cqx_scan::tree(&root, rules.as_deref(), true).map_err(Trouble::Analysis)?;
            self.cached = Some((root, scanned));
        }
        Ok(&self
            .cached
            .as_ref()
            .expect("cache is filled on this path")
            .1)
    }
}

fn root_from(args: &Value) -> Result<PathBuf, Trouble> {
    match args.get("path").and_then(Value::as_str) {
        Some(path) if !path.is_empty() => Ok(PathBuf::from(path)),
        _ => std::env::current_dir().map_err(|e| Trouble::Unreadable {
            path: PathBuf::from("."),
            why: e.to_string(),
        }),
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
    fn tools_list_has_five_tools_and_only_one_of_them_writes() {
        let reply = rpc(
            &mut Server::default(),
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
        );
        let tools = reply["result"]["tools"].as_array().expect("tools array");
        assert_eq!(tools.len(), 5);
        let names: Vec<&str> = tools
            .iter()
            .map(|t| t["name"].as_str().expect("name"))
            .collect();
        assert_eq!(
            names,
            ["score", "findings", "rules", "propose_rule", "explain"]
        );
        for tool in tools {
            let schema = tool.get("inputSchema").expect("inputSchema");
            assert_eq!(schema["type"], "object");
            assert!(schema.get("properties").is_some());
        }

        // The annotation a client shows a person when it asks whether to allow
        // a call. Exactly one tool here writes anything, and if that ever
        // stops being true it must stop being true on purpose.
        let writers: Vec<&str> = tools
            .iter()
            .filter(|t| t["annotations"]["readOnlyHint"] == json!(false))
            .map(|t| t["name"].as_str().expect("name"))
            .collect();
        assert_eq!(writers, ["propose_rule"]);
        for tool in tools {
            assert_ne!(
                tool["annotations"]["destructiveHint"],
                json!(true),
                "{} claims to be destructive",
                tool["name"]
            );
        }
    }

    /// A copy of a fixture, so a test that writes `cqx.json` writes it
    /// somewhere disposable rather than into the repository it was read from.
    fn sandbox(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cqx-mcp-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        copy(Path::new(&fixture("hard")), &dir);
        dir
    }

    fn copy(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).expect("mkdir");
        for entry in std::fs::read_dir(from).expect("readdir").flatten() {
            let target = to.join(entry.file_name());
            if entry.path().is_dir() {
                copy(&entry.path(), &target);
            } else {
                std::fs::copy(entry.path(), target).expect("copy");
            }
        }
    }

    fn propose(server: &mut Server, dir: &Path, field: &str, value: Value, because: Value) -> Value {
        let mut arguments = json!({
            "path": dir.display().to_string(),
            "rule": "env-controlled-spawn",
            "field": field,
            "value": value,
        });
        if !because.is_null() {
            arguments["because"] = because;
        }
        call(server, 9, "propose_rule", arguments)
    }

    /// The public door, used the way the desktop application uses it.
    ///
    /// Not a duplicate of the tests below: those call `handle_line` directly,
    /// and this one proves the exported type reaches the same dispatch. A
    /// second implementation of `tools/list` in the HTTP caller is the thing
    /// this type exists to prevent.
    #[test]
    fn the_public_session_answers_the_same_as_the_stdio_loop() {
        let mut session = Session::new();
        let reply = session
            .handle(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#)
            .expect("a reply");
        let parsed: Value = serde_json::from_str(&reply).expect("json");
        assert_eq!(parsed["result"]["tools"].as_array().expect("tools").len(), 5);

        // And a notification is still silence, which an HTTP caller turns
        // into 202 rather than into an empty body with a content type.
        assert!(session
            .handle(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
            .is_none());
    }

    #[test]
    fn a_tightening_is_written_with_its_reason() {
        let dir = sandbox("tighten");
        let mut server = Server::default();
        let reply = propose(&mut server, &dir, "weight", json!(45), json!("it keeps reaching production"));
        assert!(reply["result"]["isError"].is_null(), "{reply:#}");

        let written: Value = serde_json::from_str(tool_text(&reply)).expect("json");
        assert_eq!(written["changes"][0]["direction"], "tighter");

        // The file, and the reason beside the rule.
        let text = std::fs::read_to_string(dir.join("cqx.json")).expect("cqx.json");
        assert!(text.contains("\"weight\": 45"), "{text}");
        assert!(text.contains("it keeps reaching production"), "{text}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The whole point. An agent may not lower the bar.
    #[test]
    fn a_loosening_is_refused_and_nothing_is_written() {
        let dir = sandbox("loosen");
        let mut server = Server::default();
        let reply = propose(&mut server, &dir, "weight", json!(5), json!("it is noisy"));
        assert_eq!(reply["result"]["isError"], json!(true), "{reply:#}");

        let said = tool_text(&reply);
        assert!(said.contains("lower the standard"), "{said}");
        assert!(said.contains("cqx config set"), "it should say who can: {said}");
        assert!(!dir.join("cqx.json").exists(), "a refused proposal wrote a file");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A parameter is stricter and still refused, because the direction
    /// depends on what it measures. The refusal must not accuse the agent of
    /// lowering the bar — it did not, and telling it so would be wrong.
    #[test]
    fn an_unprovable_change_is_refused_in_different_words() {
        let dir = sandbox("unclear");
        let mut server = Server::default();
        let reply = call(
            &mut server,
            9,
            "propose_rule",
            json!({
                "path": dir.display().to_string(),
                "rule": "oversized-files",
                "field": "max_lines",
                "value": 400,
                "because": "we keep files small",
            }),
        );
        assert_eq!(reply["result"]["isError"], json!(true));
        let said = tool_text(&reply);
        assert!(said.contains("cannot be shown to raise"), "{said}");
        assert!(!said.contains("lower the standard"), "{said}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A no-op is its own answer. An agent told "this would lower the
    /// standard" about a change that changes nothing will try again forever.
    #[test]
    fn proposing_the_value_it_already_has_says_so() {
        let dir = sandbox("noop");
        let mut server = Server::default();
        propose(&mut server, &dir, "weight", json!(45), json!("first"));
        let again = propose(&mut server, &dir, "weight", json!(45), json!("second"));
        assert_eq!(again["result"]["isError"], json!(true));
        assert!(tool_text(&again).contains("already that"), "{}", tool_text(&again));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_proposal_without_a_reason_is_refused() {
        let dir = sandbox("why");
        let mut server = Server::default();
        let reply = propose(&mut server, &dir, "weight", json!(45), Value::Null);
        assert_eq!(reply["result"]["isError"], json!(true));
        assert!(tool_text(&reply).contains("because"), "{}", tool_text(&reply));
        assert!(!dir.join("cqx.json").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The cached scan was computed under the old rules. A score asked for
    /// straight after a rule change must not be the one from before it.
    #[test]
    fn changing_a_rule_throws_away_the_cached_scan() {
        let dir = sandbox("cache");
        let mut server = Server::default();
        let before: Value = serde_json::from_str(tool_text(&call(
            &mut server,
            1,
            "score",
            json!({ "path": dir.display().to_string() }),
        )))
        .expect("json");

        propose(&mut server, &dir, "weight", json!(45), json!("stricter"));

        let after: Value = serde_json::from_str(tool_text(&call(
            &mut server,
            2,
            "score",
            json!({ "path": dir.display().to_string() }),
        )))
        .expect("json");

        // env-controlled-spawn is a security rule and the fixture trips it, so
        // weighting it more heavily must move that score. An unchanged number
        // here means the cache answered.
        assert_ne!(
            before["scores"]["security"], after["scores"]["security"],
            "the score did not change after the rule did: {before:#} vs {after:#}"
        );
        let _ = std::fs::remove_dir_all(&dir);
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
