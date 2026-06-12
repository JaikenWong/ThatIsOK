const fs = require('fs');
const path = require('path');
const os = require('os');

const CLAUDE_HOOK_EVENTS = [
    'SessionStart',
    'UserPromptSubmit',
    'PreToolUse',
    'PostToolUse',
    'Stop',
    'PermissionRequest'
];

const CODEX_HOOK_EVENTS = [
    'SessionStart',
    'UserPromptSubmit',
    'PreToolUse',
    'PostToolUse',
    'Stop',
    'PermissionRequest'
];

const GEMINI_HOOK_EVENTS = [
    'SessionStart',
    'UserPromptSubmit',
    'PreToolUse',
    'PostToolUse',
    'Stop'
];

const CLAUDE_VALID_HOOK_EVENTS = new Set([
    'PreToolUse',
    'PostToolUse',
    'PostToolUseFailure',
    'PostToolBatch',
    'Notification',
    'UserPromptSubmit',
    'UserPromptExpansion',
    'SessionStart',
    'SessionEnd',
    'Stop',
    'StopFailure',
    'SubagentStart',
    'SubagentStop',
    'PreCompact',
    'PostCompact',
    'PermissionRequest',
    'PermissionDenied',
    'Setup',
    'TeammateIdle',
    'TaskCreated',
    'TaskCompleted',
    'Elicitation',
    'ElicitationResult',
    'ConfigChange',
    'WorktreeCreate',
    'WorktreeRemove',
    'InstructionsLoaded',
    'CwdChanged',
    'FileChanged',
    'MessageDisplay'
]);

const MANAGED_KEY = 'ThatIsOk';

class ConfigInjector {
    static appPath = process.cwd();
    static bridgePath = null;

    static setAppPath(appPath) {
        if (appPath) {
            this.appPath = appPath;
        }
    }

    static setBridgePath(bridgePath) {
        if (bridgePath) {
            this.bridgePath = bridgePath;
        }
    }

    static injectClaude() {
        const configPath = path.join(os.homedir(), '.claude', 'settings.json');

        if (!fs.existsSync(configPath)) {
            console.log('Claude settings not found at:', configPath);
            return;
        }

        try {
            const config = JSON.parse(fs.readFileSync(configPath, 'utf8'));
            config.hooks = config.hooks || {};
            this.cleanClaudeLegacyConfig(config);

            for (const eventName of CLAUDE_HOOK_EVENTS) {
                const existing = Array.isArray(config.hooks[eventName]) ? config.hooks[eventName] : [];
                const managedEntry = {
                    matcher: '*',
                    hooks: [
                        {
                            type: 'command',
                            command: this.buildBridgeCommand('claude', eventName),
                            timeout: eventName === 'PermissionRequest' ? 86400 : 10
                        }
                    ],
                    _managedBy: MANAGED_KEY
                };

                const filtered = existing.filter(
                    (entry) => entry && entry._managedBy !== MANAGED_KEY && !this.isManagedClaudeHookValue(entry)
                );
                config.hooks[eventName] = [...filtered, managedEntry];
            }

            fs.writeFileSync(configPath, JSON.stringify(config, null, 2));
            console.log('Successfully injected ThatIsOk hooks into Claude settings.');
        } catch (err) {
            console.error('Failed to inject into Claude settings:', err);
        }
    }

    static injectCodex() {
        const hooksPath = path.join(os.homedir(), '.codex', 'hooks.json');

        try {
            const config = fs.existsSync(hooksPath)
                ? JSON.parse(fs.readFileSync(hooksPath, 'utf8'))
                : { hooks: {} };

            config.hooks = config.hooks || {};

            for (const eventName of CODEX_HOOK_EVENTS) {
                const existing = Array.isArray(config.hooks[eventName]) ? config.hooks[eventName] : [];
                const managedKey = 'ThatIsOk';
                const managedEntry = {
                    hooks: [
                        {
                            type: 'command',
                            command: this.buildBridgeCommand('codex', eventName),
                            timeout: eventName === 'PermissionRequest' ? 86400 : 10
                        }
                    ],
                    _managedBy: managedKey
                };

                const filtered = existing.filter(
                    (entry) => entry && entry._managedBy !== managedKey && !this.isManagedHookValue(entry)
                );
                config.hooks[eventName] = [...filtered, managedEntry];
            }

            fs.mkdirSync(path.dirname(hooksPath), { recursive: true });
            fs.writeFileSync(hooksPath, JSON.stringify(config, null, 2));
            console.log('Successfully injected ThatIsOk hooks into Codex hooks.');
        } catch (err) {
            console.error('Failed to inject into Codex hooks:', err);
        }
    }

    static injectGemini() {
        const configPath = path.join(os.homedir(), '.gemini', 'settings.json');

        if (!fs.existsSync(configPath)) {
            console.log('Gemini settings not found at:', configPath);
            return;
        }

        try {
            const config = JSON.parse(fs.readFileSync(configPath, 'utf8'));
            config.hooks = config.hooks || {};

            for (const eventName of GEMINI_HOOK_EVENTS) {
                const existing = Array.isArray(config.hooks[eventName]) ? config.hooks[eventName] : [];
                const managedEntry = {
                    hooks: [
                        {
                            type: 'command',
                            command: this.buildBridgeCommand('gemini', eventName),
                            timeout: 10
                        }
                    ],
                    _managedBy: MANAGED_KEY
                };

                const filtered = existing.filter(
                    (entry) => entry && entry._managedBy !== MANAGED_KEY && !this.isManagedHookValue(entry)
                );
                config.hooks[eventName] = [...filtered, managedEntry];
            }

            fs.writeFileSync(configPath, JSON.stringify(config, null, 2));
            console.log('Successfully injected ThatIsOk hooks into Gemini settings.');
        } catch (err) {
            console.error('Failed to inject into Gemini settings:', err);
        }
    }

    static uninjectClaude() {
        const configPath = path.join(os.homedir(), '.claude', 'settings.json');

        if (!fs.existsSync(configPath)) {
            return;
        }

        try {
            const config = JSON.parse(fs.readFileSync(configPath, 'utf8'));
            if (!config.hooks) {
                return;
            }

            for (const eventName of CLAUDE_HOOK_EVENTS) {
                if (Array.isArray(config.hooks[eventName])) {
                    config.hooks[eventName] = config.hooks[eventName].filter(
                        (entry) => entry && entry._managedBy !== 'ThatIsOk'
                    );
                    if (config.hooks[eventName].length === 0) {
                        delete config.hooks[eventName];
                    }
                }
            }

            fs.writeFileSync(configPath, JSON.stringify(config, null, 2));
            console.log('Removed ThatIsOk hooks from Claude settings.');
        } catch (err) {
            console.error('Failed to uninject Claude settings:', err);
        }
    }

    static cleanClaudeLegacyConfig(config) {
        if (config.hooks) {
            for (const eventName of Object.keys(config.hooks)) {
                if (!CLAUDE_VALID_HOOK_EVENTS.has(eventName) && this.isManagedClaudeHookValue(config.hooks[eventName])) {
                    delete config.hooks[eventName];
                }
            }
        }

        if (config.statusLine && this.isManagedCommand(config.statusLine.command)) {
            delete config.statusLine;
        }
    }

    static isManagedClaudeHookValue(value) {
        if (!value) {
            return false;
        }

        if (typeof value === 'string') {
            return this.isManagedCommand(value);
        }

        if (Array.isArray(value)) {
            return value.some((entry) => this.isManagedClaudeHookValue(entry));
        }

        if (typeof value === 'object') {
            if (value._managedBy === MANAGED_KEY) {
                return true;
            }

            if (this.isManagedCommand(value.command)) {
                return true;
            }

            if (Array.isArray(value.hooks)) {
                return value.hooks.some((hook) => this.isManagedClaudeHookValue(hook));
            }
        }

        return false;
    }

    static isManagedCommand(command) {
        return typeof command === 'string'
            && (command.includes('ThatIsOk') || command.includes('hook-bridge.js'));
    }

    static isManagedHookValue(value) {
        if (!value) {
            return false;
        }

        if (typeof value === 'string') {
            return this.isManagedCommand(value);
        }

        if (Array.isArray(value)) {
            return value.some((entry) => this.isManagedHookValue(entry));
        }

        if (typeof value === 'object') {
            if (value._managedBy === MANAGED_KEY) {
                return true;
            }

            if (this.isManagedCommand(value.command)) {
                return true;
            }

            if (Array.isArray(value.hooks)) {
                return value.hooks.some((hook) => this.isManagedHookValue(hook));
            }
        }

        return false;
    }

    static uninjectCodex() {
        const hooksPath = path.join(os.homedir(), '.codex', 'hooks.json');

        if (!fs.existsSync(hooksPath)) {
            return;
        }

        try {
            const config = JSON.parse(fs.readFileSync(hooksPath, 'utf8'));
            if (!config.hooks) {
                return;
            }

            for (const eventName of CODEX_HOOK_EVENTS) {
                if (Array.isArray(config.hooks[eventName])) {
                    config.hooks[eventName] = config.hooks[eventName].filter(
                        (entry) => entry && entry._managedBy !== 'ThatIsOk'
                    );
                    if (config.hooks[eventName].length === 0) {
                        delete config.hooks[eventName];
                    }
                }
            }

            fs.writeFileSync(hooksPath, JSON.stringify(config, null, 2));
            console.log('Removed ThatIsOk hooks from Codex hooks.');
        } catch (err) {
            console.error('Failed to uninject Codex hooks:', err);
        }
    }

    static uninjectGemini() {
        const configPath = path.join(os.homedir(), '.gemini', 'settings.json');

        if (!fs.existsSync(configPath)) {
            return;
        }

        try {
            const config = JSON.parse(fs.readFileSync(configPath, 'utf8'));
            if (!config.hooks) {
                return;
            }

            for (const eventName of GEMINI_HOOK_EVENTS) {
                if (Array.isArray(config.hooks[eventName])) {
                    config.hooks[eventName] = config.hooks[eventName].filter(
                        (entry) => entry && entry._managedBy !== 'ThatIsOk'
                    );
                    if (config.hooks[eventName].length === 0) {
                        delete config.hooks[eventName];
                    }
                }
            }

            fs.writeFileSync(configPath, JSON.stringify(config, null, 2));
            console.log('Removed ThatIsOk hooks from Gemini settings.');
        } catch (err) {
            console.error('Failed to uninject Gemini settings:', err);
        }
    }

    static getClaudeStatus() {
        const configPath = path.join(os.homedir(), '.claude', 'settings.json');
        if (!fs.existsSync(configPath)) {
            return { installed: false, reason: 'settings.json not found' };
        }

        try {
            const config = JSON.parse(fs.readFileSync(configPath, 'utf8'));
            const hooks = config.hooks || {};
            const installed = CLAUDE_HOOK_EVENTS.some(
                (event) => hooks[event] && hooks[event].some(
                    (entry) => entry && entry._managedBy === 'ThatIsOk'
                )
            );
            return { installed };
        } catch (err) {
            return { installed: false, reason: err.message };
        }
    }

    static getCodexStatus() {
        const hooksPath = path.join(os.homedir(), '.codex', 'hooks.json');
        if (!fs.existsSync(hooksPath)) {
            return { installed: false, reason: 'hooks.json not found' };
        }

        try {
            const config = JSON.parse(fs.readFileSync(hooksPath, 'utf8'));
            const hooks = config.hooks || {};
            const installed = CODEX_HOOK_EVENTS.some(
                (event) => hooks[event] && hooks[event].some(
                    (entry) => entry && entry._managedBy === 'ThatIsOk'
                )
            );
            return { installed };
        } catch (err) {
            return { installed: false, reason: err.message };
        }
    }

    static getGeminiStatus() {
        const configPath = path.join(os.homedir(), '.gemini', 'settings.json');
        if (!fs.existsSync(configPath)) {
            return { installed: false, reason: 'settings.json not found' };
        }

        try {
            const config = JSON.parse(fs.readFileSync(configPath, 'utf8'));
            const hooks = config.hooks || {};
            const installed = GEMINI_HOOK_EVENTS.some(
                (event) => hooks[event] && hooks[event].some(
                    (entry) => entry && entry._managedBy === 'ThatIsOk'
                )
            );
            return { installed };
        } catch (err) {
            return { installed: false, reason: err.message };
        }
    }

    static buildBridgeCommand(source, eventName) {
        const bridgePath = this.bridgePath || path.join(this.appPath, 'bridge', 'hook-bridge.js');
        if (process.platform === 'win32') {
            const escaped = bridgePath.replace(/"/g, '\\"');
            return `node "${escaped}" --source ${source} --event ${eventName}`;
        }

        return `"/usr/bin/env" node "${bridgePath}" --source ${source} --event ${eventName}`;
    }
}

module.exports = ConfigInjector;
