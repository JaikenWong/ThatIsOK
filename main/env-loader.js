const fs = require('fs');
const path = require('path');
const os = require('os');

const API_ENV_KEYS = new Set([
  'MINIMAX_CN_API_KEY',
  'MINIMAX_API_KEY',
  'MINIMAX_API_TOKEN',
  'MINIMAX_BASE_URL',
  'MINIMAX_API_HOST',
  'DEEPSEEK_API_KEY',
  'DEEPSEEK_API_TOKEN',
  'DEEPSEEK_BASE_URL'
]);

function loadEnvFile(filePath = path.join(process.cwd(), '.env'), options = {}) {
  if (!fs.existsSync(filePath)) {
    return;
  }

  const content = fs.readFileSync(filePath, 'utf8');
  for (const line of content.split(/\r?\n/)) {
    const parsed = parseEnvLine(line, options.allowedKeys);
    if (!parsed) continue;
    setEnv(parsed.key, parsed.value, options);
  }
}

function loadEnvironment(app = null) {
  const candidates = getEnvFileCandidates(app);
  for (const filePath of candidates) {
    loadEnvFile(filePath);
  }

  for (const filePath of getShellProfileCandidates()) {
    loadEnvFile(filePath, { allowedKeys: API_ENV_KEYS });
  }
}

function getEnvFileCandidates(app = null) {
  const candidates = [
    process.env.THATISOK_ENV_FILE,
    path.join(process.cwd(), '.env'),
    path.join(os.homedir(), '.thatisok', '.env'),
    path.join(os.homedir(), '.config', 'thatisok', '.env')
  ];

  if (app && typeof app.getAppPath === 'function') {
    candidates.push(path.join(app.getAppPath(), '.env'));
  }
  if (app && typeof app.getPath === 'function') {
    candidates.push(path.join(app.getPath('userData'), '.env'));
  }
  if (process.resourcesPath) {
    candidates.push(path.join(process.resourcesPath, '.env'));
  }

  candidates.push(path.join(path.dirname(process.execPath), '.env'));

  return [...new Set(candidates.filter(Boolean))];
}

function getShellProfileCandidates() {
  return [
    '.zshenv',
    '.zprofile',
    '.zshrc',
    '.bash_profile',
    '.bashrc',
    '.profile'
  ].map((fileName) => path.join(os.homedir(), fileName));
}

function parseEnvLine(line, allowedKeys = null) {
  let trimmed = String(line || '').trim();
  if (!trimmed || trimmed.startsWith('#')) {
    return null;
  }

  if (trimmed.startsWith('export ')) {
    trimmed = trimmed.slice('export '.length).trim();
  }

  const eqIndex = trimmed.indexOf('=');
  if (eqIndex <= 0) {
    return null;
  }

  const key = trimmed.slice(0, eqIndex).trim();
  if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(key)) {
    return null;
  }
  if (allowedKeys && !allowedKeys.has(key)) {
    return null;
  }

  let value = trimmed.slice(eqIndex + 1).trim();
  value = stripInlineComment(value);
  if (
    (value.startsWith('"') && value.endsWith('"')) ||
    (value.startsWith("'") && value.endsWith("'"))
  ) {
    value = value.slice(1, -1);
  }

  return { key, value };
}

function stripInlineComment(value) {
  let quote = null;
  for (let index = 0; index < value.length; index += 1) {
    const char = value[index];
    if ((char === '"' || char === "'") && value[index - 1] !== '\\') {
      quote = quote === char ? null : quote || char;
    }
    if (char === '#' && !quote && /\s/.test(value[index - 1] || '')) {
      return value.slice(0, index).trim();
    }
  }
  return value;
}

function setEnv(key, value, options = {}) {
  if (!key || process.env[key] !== undefined && !options.override) {
    return;
  }
  process.env[key] = value;
}

module.exports = {
  loadEnvFile,
  loadEnvironment,
  getEnvFileCandidates,
  parseEnvLine
};
