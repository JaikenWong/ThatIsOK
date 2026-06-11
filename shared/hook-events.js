function normalizeHookEnvelope(eventData) {
  if (!eventData || typeof eventData !== 'object') {
    return {
      source: 'unknown',
      event: 'unknown',
      payload: null,
      raw: typeof eventData === 'string' ? eventData : ''
    };
  }

  return {
    source: eventData.source || 'unknown',
    event: eventData.event || 'unknown',
    payload: eventData.payload || null,
    raw: eventData.raw || JSON.stringify(eventData.payload || eventData)
  };
}

function isPermissionLikeEvent(eventData) {
  if (String(eventData.event).toLowerCase() === 'permissionrequest') {
    return true;
  }

  const raw = String(eventData.raw || '').toLowerCase();
  return raw.includes('permission') || raw.includes('approval');
}

function isPreToolUseEvent(eventData) {
  return String(eventData.event).toLowerCase() === 'pretooluse';
}

function formatSourceLabel(source) {
  if (source === 'claude') {
    return 'Claude';
  }
  if (source === 'codex') {
    return 'Codex';
  }
  if (source === 'gemini') {
    return 'Gemini';
  }
  if (source === 'minimax') {
    return 'MiniMax';
  }
  if (source === 'opencode') {
    return 'OpenCode';
  }
  return 'Agent';
}

function extractCommand(eventData) {
  const payload = eventData.payload || {};
  const direct = [payload.command, payload.cmd].find(Boolean);
  if (direct) {
    return String(direct);
  }

  if (payload.tool_input && typeof payload.tool_input === 'object') {
    if (payload.tool_input.command) {
      return String(payload.tool_input.command);
    }
    if (payload.tool_input.cmd) {
      return String(payload.tool_input.cmd);
    }
  }

  const rawCommand = extractCommandFromRaw(eventData.raw);
  if (rawCommand) {
    return rawCommand;
  }

  return null;
}

function extractToolName(eventData) {
  const payload = eventData.payload || {};
  return payload.tool_name || payload.toolName || payload.tool || payload.matcher || 'permission';
}

function extractSandbox(eventData) {
  const payload = eventData.payload || {};
  const direct = payload.sandbox_permissions || payload.sandbox || payload.permission_mode;
  if (direct) {
    return direct;
  }

  const raw = String(eventData.raw || '');
  const match = raw.match(/sandbox(?:[_\s-]?permissions?| mode)?\s*:\s*([^\r\n]+)/i);
  return match ? match[1].trim() : null;
}

function extractPrefixRule(eventData) {
  const payload = eventData.payload || {};
  const rule = payload.prefix_rule || payload.prefixRule;
  if (Array.isArray(rule)) {
    return rule.join(' ');
  }
  if (rule) {
    return String(rule);
  }

  const raw = String(eventData.raw || '');
  const match = raw.match(/prefix[_\s-]?rule\s*:\s*([^\r\n]+)/i);
  return match ? match[1].trim() : null;
}

function extractFilePath(eventData) {
  const payload = eventData.payload || {};
  return payload.file_path || payload.filePath || payload.path || null;
}

function buildInterventionModel(eventData) {
  const payload = eventData.payload || {};
  const command = extractCommand(eventData);
  const filePath = extractFilePath(eventData);
  const reason = extractReason(eventData);
  const fallback = [payload.matcher, reason, payload.message, payload.prompt].filter(Boolean)[0];
  const actionKind = getActionKind({ command, filePath, toolName: extractToolName(eventData), raw: eventData.raw });
  const toolName = extractToolName(eventData);
  const title = buildInterventionTitle({
    source: eventData.source,
    actionKind,
    filePath,
    toolName
  });

  return {
    source: eventData.source,
    event: eventData.event,
    title,
    detail: String(reason || fallback || command || eventData.raw || '--').slice(0, 240),
    command,
    filePath,
    reason,
    actionKind,
    toolName,
    sandbox: extractSandbox(eventData),
    prefixRule: extractPrefixRule(eventData),
    raw: eventData.raw,
    meta: payload
  };
}

function extractReason(eventData) {
  const payload = eventData.payload || {};
  const direct = [payload.reason, payload.message, payload.prompt].find(Boolean);
  if (direct) {
    return String(direct);
  }

  const raw = String(eventData.raw || '');
  const reasonMatch = raw.match(/reason\s*:\s*([\s\S]*?)(?:\n\s*\n|\n\s{2,}[A-Z][^:\n]*:|$)/i);
  if (reasonMatch) {
    return reasonMatch[1].replace(/\s+/g, ' ').trim();
  }

  return null;
}

function extractCommandFromRaw(rawValue) {
  const raw = String(rawValue || '');
  if (!raw) {
    return null;
  }

  const commandLineMatch = raw.match(/command\s*:\s*([^\r\n]+)/i);
  if (commandLineMatch) {
    return commandLineMatch[1].trim();
  }

  const blockMatch = raw.match(/following command\??\s*([\s\S]*?)(?:\n\s*\n|$)/i);
  if (!blockMatch) {
    return null;
  }

  const candidate = blockMatch[1]
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean)
    .filter((line) => !/^reason\s*:/i.test(line))
    .join(' ');

  return candidate || null;
}

function buildInterventionTitle({ source, actionKind, filePath, toolName }) {
  const sourceLabel = formatSourceLabel(source);

  if (filePath) {
    return `${sourceLabel} wants to edit ${basename(filePath)}`;
  }

  if (actionKind === 'run_command') {
    return `${sourceLabel} wants to run a command`;
  }

  if (actionKind === 'read_process') {
    return `${sourceLabel} wants to inspect local processes`;
  }

  if (actionKind === 'network') {
    return `${sourceLabel} wants network access`;
  }

  if (toolName && toolName !== 'permission') {
    return `${sourceLabel} needs approval for ${String(toolName).replace(/_/g, ' ')}`;
  }

  return `${sourceLabel} needs approval`;
}

function getActionKind({ command, filePath, toolName, raw }) {
  if (filePath) {
    return 'edit_file';
  }

  const text = String([command, toolName, raw].filter(Boolean).join(' ')).toLowerCase();
  if (!text) {
    return 'unknown';
  }

  if (/\b(get-process|ps|tasklist|wmic|pgrep|process status)\b/.test(text)) {
    return 'read_process';
  }

  if (/\b(curl|wget|invoke-webrequest|fetch|http|https|dns)\b/.test(text)) {
    return 'network';
  }

  if (command) {
    return 'run_command';
  }

  return 'unknown';
}

function basename(filePath) {
  if (!filePath) return '';
  const parts = String(filePath).replace(/\\/g, '/').split('/');
  return parts[parts.length - 1] || filePath;
}

function buildHookDecisionResponse({ approved, allowPersistent, requiresDecision }) {
  return {
    ok: true,
    requiresDecision: Boolean(requiresDecision),
    approved: Boolean(approved),
    allowPersistent: Boolean(allowPersistent)
  };
}

module.exports = {
  normalizeHookEnvelope,
  isPermissionLikeEvent,
  isPreToolUseEvent,
  buildInterventionModel,
  buildHookDecisionResponse
};
