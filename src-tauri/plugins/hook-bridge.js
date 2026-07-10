#!/usr/bin/env node
// AgentIsOK hook bridge (JS variant) for Claude Code / Codex on Windows.
//
// Mirrors the Rust binary's headless `--hook-source/--hook-event` path:
// reads hook JSON from stdin, connects to AgentIsOK's local TCP server
// (127.0.0.1:45873), sends one JSON line, and for PermissionRequest events
// prints the agent-facing decision JSON to stdout.
//
// Used when the OS whitelisting/EDR makes spawning the app exe on every
// hook event impractical. Node.exe (already whitelisted) spawns this
// tiny script instead, so the app exe only runs once as the GUI.
const { createConnection } = require("net");

const argv = process.argv.slice(2);
function getArg(name) {
  const i = argv.indexOf(name);
  return i >= 0 && i + 1 < argv.length ? argv[i + 1] : null;
}

const source = getArg("--hook-source") || "unknown";
const event = getArg("--hook-event") || "unknown";
const eventLower = event.toLowerCase();
const isPermission = eventLower === "permissionrequest" || eventLower === "permission_request";

let raw = "";
let started = false;

function begin() {
  if (started) return;
  started = true;
  let payload = null;
  try {
    payload = JSON.parse(raw);
  } catch {
    payload = null;
  }
  send({
    event: "hook-event",
    data: { source, event, raw, payload },
  });
}

function tryBegin() {
  if (started) return;
  try {
    JSON.parse(raw);
    begin();
  } catch {
    // wait for more stdin
  }
}

process.stdin.setEncoding("utf8");
process.stdin.on("data", (chunk) => {
  raw += chunk;
  tryBegin();
});
process.stdin.on("end", begin);
setTimeout(begin, 10_000); // safety net mirroring the Rust read_stdin_json timeout

function send(payload) {
  const timeoutMs = isPermission ? 1_800_000 : 5_000;
  const sock = createConnection({ host: "127.0.0.1", port: 45873 }, () => {
    sock.write(JSON.stringify(payload) + "\n");
  });

  let buf = "";
  let done = false;

  function finish(line) {
    if (done) return;
    done = true;
    try {
      sock.destroy();
    } catch {}
    if (line && isPermission) {
      writePermissionOutput(line);
    }
    process.exit(0);
  }

  sock.on("data", (chunk) => {
    buf += chunk.toString();
    const nl = buf.indexOf("\n");
    if (nl >= 0) finish(buf.slice(0, nl));
  });
  sock.on("end", () => finish(buf.trim()));
  sock.on("error", () => finish(null));
  sock.setTimeout(timeoutMs, () => finish(null));
}

function writePermissionOutput(line) {
  let res;
  try {
    res = JSON.parse(line);
  } catch {
    return;
  }
  if (res.requiresDecision !== true) return;
  if (res.isQuestion === true) {
    writeQuestionOutput(res);
    return;
  }
  const approved = res.approved === true;
  const decision = { behavior: approved ? "allow" : "deny" };
  if (!approved) {
    decision.message = "Denied from AgentIsOK";
    decision.interrupt = false;
  }
  const out = {
    continue: true,
    hookSpecificOutput: {
      hookEventName: "PermissionRequest",
      decision,
    },
  };
  if (source === "claude") out.suppressOutput = true;
  process.stdout.write(JSON.stringify(out));
}

function writeQuestionOutput(res) {
  const answer = (typeof res.answer === "string" ? res.answer : "").trim();
  if (source === "opencode") {
    process.stdout.write(JSON.stringify({ type: "answer", text: answer }));
    return;
  }
  const question =
    (typeof res.question === "string" ? res.question : "").trim() || "answer";
  const decision = { behavior: "allow" };
  const hookSpecificOutput = {
    hookEventName: "PermissionRequest",
    decision,
    answer,
  };
  if (answer) {
    hookSpecificOutput.updatedInput = { answers: { [question]: answer } };
  }
  const out = { continue: true, hookSpecificOutput };
  if (source === "claude") out.suppressOutput = true;
  process.stdout.write(JSON.stringify(out));
}
