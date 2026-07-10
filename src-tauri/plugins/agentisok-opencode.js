// AgentIsOK hook plugin for OpenCode
// Bridges OpenCode events to AgentIsOK desktop app via TCP.
//
// Install:
//   1. Copy this file to ~/.config/opencode/plugins/agentisok.js
//   2. Add to ~/.config/opencode/config.json:
//      { "plugin": ["file:///Users/YOU/.config/opencode/plugins/agentisok.js"] }
//
// AgentIsOK must be running (the Tauri app) for permission requests to work.
// Non-permission events are fire-and-forget — OpenCode keeps working even if
// AgentIsOK is not running.
import { connect } from "net";
import { appendFileSync } from "fs";

const DEBUG_LOG = "/tmp/agentisok-opencode.log";
function debug(msg) {
  try { appendFileSync(DEBUG_LOG, `[${new Date().toISOString()}] ${msg}\n`); } catch {}
}
debug("plugin loaded");

const TCP_HOST = "127.0.0.1";
const TCP_PORT = 45873;
const PERMISSION_TIMEOUT_MS = 3_600_000; // 1h — user may review approvals away from terminal

const HOOK_EVENTS = {
  "session.created": "SessionStart",
  "session.deleted": "Stop",
  "session.status": null, // handled specially for idle
  "message.part.updated": null, // handled specially for text/tool
  "permission.asked": "PermissionRequest",
  "permission.replied": "PostToolUse",
};

/**
 * Connect to AgentIsOK TCP server, send a JSON line, read one response line.
 */
function sendAndWait(payload, timeoutMs = PERMISSION_TIMEOUT_MS) {
  return new Promise((resolve) => {
    const sock = connect({ host: TCP_HOST, port: TCP_PORT }, () => {
      sock.write(JSON.stringify(payload) + "\n");
    });

    let buf = "";
    sock.on("data", (chunk) => {
      buf += chunk.toString();
      const newline = buf.indexOf("\n");
      if (newline >= 0) {
        const line = buf.slice(0, newline);
        sock.destroy();
        try { resolve(JSON.parse(line)); } catch { resolve(null); }
      }
    });
    sock.on("error", () => resolve(null));
    sock.on("end", () => {
      const line = buf.trim();
      try { resolve(JSON.parse(line)); } catch { resolve(null); }
    });
    sock.setTimeout(timeoutMs, () => { sock.destroy(); resolve(null); });
  });
}

/**
 * Fire-and-forget — don't wait for response.
 */
function sendFireAndForget(payload) {
  try {
    const sock = connect({ host: TCP_HOST, port: TCP_PORT }, () => {
      sock.write(JSON.stringify(payload) + "\n");
      sock.end();
    });
    sock.on("error", () => {});
    sock.setTimeout(2000, () => sock.destroy());
  } catch {}
}

function makeHookPayload(source, event, raw, extra = {}) {
  return {
    event: "hook-event",
    data: {
      source,
      event,
      raw: raw || "{}",
      payload: extra,
    },
  };
}

// ---- Event mapping ----

let msgRoles = new Map();
let cwdMap = new Map();
const MAX_ROLES = 200;

function trackRole(info) {
  if (info?.id && info?.sessionID) {
    msgRoles.set(info.id, { role: info.role, sid: info.sessionID });
    if (msgRoles.size > MAX_ROLES) msgRoles.delete(msgRoles.keys().next().value);
  }
}

function roleFor(msgId) {
  return msgRoles.get(msgId) || {};
}

function mapOpenCodeEvent(ev) {
  const t = ev.type;
  const p = ev.properties || {};

  // session.created
  if (t === "session.created" && p.info) {
    const sid = p.info.id;
    if (p.info.directory) cwdMap.set(sid, p.info.directory);
    return makeHookPayload("opencode", "SessionStart", "", {
      session_id: sid,
      cwd: p.info.directory || "",
    });
  }

  // session.deleted
  if (t === "session.deleted" && p.info) {
    const sid = p.info.id;
    cwdMap.delete(sid);
    msgRoles.forEach((v, k) => { if (v.sid === sid) msgRoles.delete(k); });
    return makeHookPayload("opencode", "Stop", "", { session_id: sid });
  }

  // session.status idle → Stop
  if (t === "session.status" && p.sessionID) {
    if (p.status?.type === "idle") {
      const sid = p.sessionID;
      return makeHookPayload("opencode", "Stop", "", { session_id: sid });
    }
    return null;
  }

  // message.updated — track role
  if (t === "message.updated" && p.info) {
    trackRole(p.info);
    return null;
  }

  // message.part.updated — text
  if (t === "message.part.updated" && p.part) {
    const meta = roleFor(p.part.messageID);
    if (!meta.sid) return null;

    if (p.part.type === "text" && p.part.text) {
      if (meta.role === "user") {
        return makeHookPayload("opencode", "UserPromptSubmit", p.part.text, {
          session_id: meta.sid,
          cwd: cwdMap.get(meta.sid) || "",
          prompt: p.part.text,
        });
      }
    }

    // tool use
    if (p.part.type === "tool" && p.part.sessionID) {
      const st = p.part.state?.status;
      const tn = (p.part.tool || "unknown").replace(/_/g, " ");
      if (st === "running" || st === "pending") {
        return makeHookPayload("opencode", "PreToolUse", "", {
          session_id: p.part.sessionID,
          tool_name: tn,
          tool_input: typeof p.part.state?.input === "string"
            ? p.part.state.input.slice(0, 300)
            : JSON.stringify(p.part.state?.input || {}).slice(0, 300),
          cwd: cwdMap.get(p.part.sessionID) || "",
        });
      }
      if (st === "completed" || st === "error") {
        return makeHookPayload("opencode", "PostToolUse", "", {
          session_id: p.part.sessionID,
          tool_name: tn,
          cwd: cwdMap.get(p.part.sessionID) || "",
        });
      }
    }

    return null;
  }

  // permission.asked
  if (t === "permission.asked" && p.id && p.sessionID) {
    const patterns = p.patterns || [];
    const toolName = p.permission || "unknown";
    const cmd = patterns.join(" && ");
    const fp = toolName === "edit" || toolName === "write" ? patterns[0] || "" : "";

    return makeHookPayload("opencode", "PermissionRequest", "", {
      session_id: p.sessionID,
      permission_id: p.id,
      tool_name: toolName,
      command: cmd,
      file_path: fp,
      reason: `OpenCode wants ${toolName}: ${patterns[0] || "?"}`,
      cwd: cwdMap.get(p.sessionID) || "",
    });
  }

  return null;
}

// ---- Plugin entry ----

export default async ({ client, serverUrl }) => {
  const serverPort = serverUrl ? parseInt(serverUrl.port) || 4096 : 4096;
  const internalFetch = client?._client?.getConfig?.()?.fetch || null;

  return {
    event: async ({ event }) => {
      try {
        debug(`EVENT type=${event?.type} props=${JSON.stringify(event?.properties || {}).slice(0, 200)}`);
        const payload = mapOpenCodeEvent(event);
        if (!payload) return;
        debug(`MAPPED ${payload.data.event} source=${payload.data.source}`);

        // PermissionRequest — block and wait for AgentIsOK decision
        if (payload.data.event === "PermissionRequest" && internalFetch) {
          const requestId = payload.data.payload?.permission_id;
          if (!requestId) return;

          const response = await sendAndWait(payload);
          if (!response) return;

          const approved = response.approved === true;
          const reply = approved ? "once" : "reject";
          try {
            await internalFetch(
              new Request(`http://localhost:${serverPort}/permission/${requestId}/reply`, {
                method: "POST",
                headers: { "Content-Type": "application/json" },
                body: JSON.stringify({ reply }),
              })
            );
          } catch {}
          return;
        }

        // Regular events — fire and forget
        sendFireAndForget(payload);
      } catch {
        // Never block OpenCode if AgentIsOK is down
      }
    },
  };
};
