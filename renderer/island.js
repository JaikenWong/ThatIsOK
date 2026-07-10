const ipcRenderer = createIpcRendererAdapter();
const appPlatform = getAppPlatform();

const island = document.getElementById('island');
const pillContent = document.getElementById('pillContent');
const expandedContent = document.getElementById('expandedContent');
const primaryLabel = document.getElementById('primaryLabel');
const primaryLabelText = document.getElementById('primaryLabelText');
const primaryValue = document.getElementById('primaryValue');
const pillProgress = document.getElementById('pillProgress');
const sessionsList = document.getElementById('sessionsList');
const accountsList = document.getElementById('accountsList');
const actionBar = document.querySelector('.actions');
const syncButton = document.getElementById('syncButton');
const syncIntervalDown = document.getElementById('syncIntervalDown');
const syncIntervalUp = document.getElementById('syncIntervalUp');
const syncIntervalValue = document.getElementById('syncIntervalValue');
const settingsMeta = document.getElementById('settingsMeta');
const interventionPanel = document.getElementById('interventionPanel');
const interventionSource = document.getElementById('interventionSource');
const interventionRisk = document.getElementById('interventionRisk');
const interventionTool = document.getElementById('interventionTool');
const interventionTitle = document.getElementById('interventionTitle');
const interventionExplanation = document.getElementById('interventionExplanation');
const interventionDetail = document.getElementById('interventionDetail');
const interventionThinking = document.getElementById('interventionThinking');
const interventionCommand = document.getElementById('interventionCommand');
const interventionMeta = document.getElementById('interventionMeta');
const approveButton = document.getElementById('approveButton');
const approveAlwaysButton = document.getElementById('approveAlwaysButton');
const denyButton = document.getElementById('denyButton');
const providerToggles = document.getElementById('providerToggles');
const runtimeWarning = document.getElementById('runtimeWarning');
const approveShortcut = document.getElementById('approveShortcut');
const alwaysShortcut = document.getElementById('alwaysShortcut');
const denyShortcut = document.getElementById('denyShortcut');
const jumpToTerminalButton = document.getElementById('jumpToTerminalButton');
const interventionAskOptions = document.getElementById('interventionAskOptions');
const viewTabs = document.getElementById('viewTabs');
const setupHealth = document.getElementById('setupHealth');
const rulesPanel = document.getElementById('rulesPanel');
const rulesSearch = document.getElementById('rulesSearch');
const rulesFilters = document.getElementById('rulesFilters');
const rulesList = document.getElementById('rulesList');
const rulesUndo = document.getElementById('rulesUndo');
const rulesUndoText = document.getElementById('rulesUndoText');
const rulesUndoButton = document.getElementById('rulesUndoButton');

const PROVIDER_SETUP_TIPS = {
    codex: 'Requires Codex login. Run codex login, restart Codex, then Sync.',
    claude: 'Requires Claude Code login and hooks. Restart Claude Code after enabling.',
    cursor: 'Requires Cursor local login before usage can be synced.',
    minimax: 'Requires MINIMAX_API_KEY or MINIMAX_CN_API_KEY in environment, then Sync.',
    gemini: 'Requires Antigravity login before usage can be synced.',
    deepseek: 'Requires DEEPSEEK_API_KEY in project .env or environment, then restart.',
    opencode: 'Install AgentIsOK plugin: copy agentisok-opencode.js to ~/.config/opencode/plugins/, update config.json.',
    kiro: 'Requires Kiro (Amazon Q) installed and signed in. Open Kiro dashboard once to populate usage data.'
};

let currentData = null;
let pendingIntervention = null;
let respondingInterventionId = null;
let providerVisibility = {};
let dragging = false;
let suppressClickUntil = 0;
let dragPointerDown = null;
let dragStarted = false;
let heightSyncFrame = null;
let lastExpandedHeight = 0;
let lastPillWidth = 0;
let heightSyncDebounce = null;
let pendingDragMove = null;
let dragMoveFrame = null;
let windowState = { mode: 'pill' };
let runtimeWarningTimer = null;
let settingsState = { syncIntervalMinutes: 10 };
let updateReadyToRestart = false;
let activeView = 'home';
let approvalRules = [];
let selectedAgentId = null;
let rulesSearchQuery = '';
let rulesSourceFilter = 'all';
let expandedRuleIndex = null;
let pendingRuleUndo = null;
const DRAG_THRESHOLD = 8;
const SYNC_INTERVAL_STEPS = [5, 10, 15, 30, 60];
const knownProviderOrder = ['claude', 'codex', 'cursor', 'deepseek', 'gemini', 'kiro', 'minimax', 'opencode'];

function expandIsland() {
    island.classList.remove('pill');
    island.classList.add('expanded');
    expandedContent.classList.remove('hidden');
    ipcRenderer.send('island:set-mode', 'expanded');
    scheduleExpandedHeightSync();
    setTimeout(scheduleExpandedHeightSync, 80);

    const agentStatusIcons = document.getElementById('agentStatusIcons');
    if (agentStatusIcons) {
        agentStatusIcons.remove();
    }
}

function collapseIsland() {
    if (pendingIntervention) {
        return;
    }

    lastExpandedHeight = 0;
    island.classList.remove('expanded');
    island.classList.add('pill');
    expandedContent.classList.add('hidden');
    ipcRenderer.send('island:set-mode', 'pill');

    // Re-render pill bars from latest data so collapsed view
    // always reflects the most recent sync.
    if (currentData) {
        refreshPillContent(currentData);
    }
}

/**
 * Update only the pill header without touching the expanded account list.
 */
function refreshPillContent(data) {
    const visibleAccounts = getPrioritizedVisibleAccounts(data.accounts || []);
    const progressAccounts = visibleAccounts.filter(hasPillIndicator).slice(0, 5);
    const primaryAccount = visibleAccounts[0];

    if (progressAccounts.length) {
        primaryLabelText.innerText = progressAccounts.length > 1 ? '' : (primaryAccount ? primaryAccount.label : 'Provider');
        primaryValue.innerText = primaryAccount ? (primaryAccount.plan || 'Live') : 'Live';
        renderPillProgress(progressAccounts);
    } else if (primaryAccount) {
        primaryLabelText.innerText = primaryAccount.label || 'Provider';
        primaryValue.innerText = renderAccountHeadline(primaryAccount);
        hidePillProgress();
    } else {
        primaryLabelText.innerText = 'AgentIsOK';
        primaryValue.innerText = 'Live';
        hidePillProgress();
    }

    renderAgentStatusIcons(data.sessions || []);
    island.classList.remove('tone-neutral', 'tone-good', 'tone-warn', 'tone-danger');
    island.classList.add(getTone(data));
}

function createIpcRendererAdapter() {
    if (typeof require === 'function') {
        try {
            const electron = require('electron');
            if (electron && electron.ipcRenderer) {
                return electron.ipcRenderer;
            }
        } catch (_err) {
            // Fall through to Tauri adapter.
        }
    }

    const tauri = window.__TAURI__;
    if (!tauri || !tauri.core || !tauri.event) {
        throw new Error('No supported IPC runtime found');
    }

    const invokeMap = {
        'island:get-data': 'island_get_data',
        'island:get-intervention': 'island_get_intervention',
        'island:sync-now': 'island_sync_now',
        'intervention:respond': 'intervention_respond',
        'providers:get-visibility': 'providers_get_visibility',
        'settings:get': 'settings_get',
        'settings:set-sync-interval': 'settings_set_sync_interval',
        'approval-rules:list': 'approval_rules_list',
        'approval-rules:delete': 'approval_rules_delete',
        'approval-rules:restore': 'approval_rules_restore',
        'island:jump-to-terminal': 'jump_to_terminal'
    };

    const sendMap = {
        'island:set-mode': (mode) => tauri.core.invoke('island_set_mode', { mode }),
        'island:set-expanded-height': (height) => tauri.core.invoke('island_set_expanded_height', { height }),
        'island:drag-start': (mouse) => tauri.core.invoke('island_drag_start', { mouse }),
        'island:drag-move': (mouse) => tauri.core.invoke('island_drag_move', { mouse }),
        'island:drag-end': () => tauri.core.invoke('island_drag_end'),
        'intervention:respond': (decision, answer = '') => tauri.core.invoke('intervention_respond', { decision, answer }),
        'providers:set-visibility': (provider, visible) => tauri.core.invoke('providers_set_visibility', { provider, visible }),
        'island:set-pill-width': (width) => tauri.core.invoke('island_set_pill_width', { width }),
        'app:restart': () => tauri.core.invoke('app_restart')
    };

    return {
        invoke(channel, ...args) {
            const command = invokeMap[channel];
            if (!command) {
                return Promise.reject(new Error(`Unsupported invoke channel: ${channel}`));
            }
            return tauri.core.invoke(command, ...args);
        },
        send(channel, ...args) {
            const sender = sendMap[channel];
            if (!sender) {
                console.warn(`Unsupported send channel: ${channel}`);
                return;
            }
            sender(...args).catch((err) => console.error(`IPC send failed: ${channel}`, err));
        },
        on(channel, handler) {
            tauri.event.listen(channel, (event) => handler({}, event.payload))
                .catch((err) => console.error(`IPC listen failed: ${channel}`, err));
        }
    };
}

function getAppPlatform() {
    if (typeof process !== 'undefined' && process.platform) {
        return process.platform;
    }
    return navigator.platform.toLowerCase().includes('mac') ? 'darwin' : 'win32';
}

island.addEventListener('click', (event) => {
    if (dragging || Date.now() < suppressClickUntil) {
        return;
    }

    if (event.target.tagName === 'BUTTON') {
        return;
    }

    if (event.target.closest('.providerToggle')) {
        return;
    }

    const isPill = island.classList.contains('pill');
    const clickedHeader = Boolean(event.target.closest('#pillContent'));

    if (isPill) {
        expandIsland();
    } else if (clickedHeader) {
        collapseIsland();
    }
});

document.getElementById('pillContent').addEventListener('mousedown', (event) => {
    dragPointerDown = { x: event.screenX, y: event.screenY };
    dragStarted = false;
});

document.getElementById('island').addEventListener('mousedown', (event) => {
    if (!island.classList.contains('expanded')) return;
    if (event.target.tagName === 'BUTTON') return;
    if (event.target.closest('.providerToggle')) return;
    if (event.target.closest('.tipBadge')) return;
    if (event.target.closest('.settingsStepper')) return;
    dragPointerDown = { x: event.screenX, y: event.screenY };
    dragStarted = false;
});

window.addEventListener('mousemove', (event) => {
    if (!dragPointerDown) {
        return;
    }

    const dx = event.screenX - dragPointerDown.x;
    const dy = event.screenY - dragPointerDown.y;
    const distance = Math.hypot(dx, dy);

    if (!dragStarted && distance >= DRAG_THRESHOLD) {
        dragStarted = true;
        dragging = true;
        ipcRenderer.send('island:drag-start', dragPointerDown);
    }

    if (!dragStarted) {
        return;
    }

    pendingDragMove = { x: event.screenX, y: event.screenY };
    if (!dragMoveFrame) {
        dragMoveFrame = requestAnimationFrame(() => {
            dragMoveFrame = null;
            if (pendingDragMove) {
                const nextMove = pendingDragMove;
                pendingDragMove = null;
                ipcRenderer.send('island:drag-move', nextMove);
            }
        });
    }
});

window.addEventListener('mouseup', () => {
    if (!dragPointerDown) {
        return;
    }

    if (dragStarted) {
        dragging = false;
        suppressClickUntil = Date.now() + 180;
        if (dragMoveFrame) {
            cancelAnimationFrame(dragMoveFrame);
            dragMoveFrame = null;
        }
        if (pendingDragMove) {
            ipcRenderer.send('island:drag-move', pendingDragMove);
            pendingDragMove = null;
        }
        ipcRenderer.send('island:drag-end');
    }

    dragPointerDown = null;
    dragStarted = false;
});

function formatUsd(value) {
    if (typeof value !== 'number' || Number.isNaN(value)) {
        return '--';
    }

    return `$${value.toFixed(1)}`;
}

function getAccountPriority(account) {
    const provider = account?.provider || '';
    const index = knownProviderOrder.indexOf(provider);

    if (index === -1) return 100;

    if (account?.status === 'stale') return index + 50;

    return index;
}

function getTone(data) {
    if (pendingIntervention) {
        return 'tone-danger';
    }

    if (!data) {
        return 'tone-neutral';
    }

    const accounts = data.accounts || [];

    for (const account of accounts) {
        if (account.status === 'setup' || account.status === 'error' || account.status === 'stale') continue;

        if (typeof account.usage?.remainingPercent === 'number') {
            if (account.usage.remainingPercent <= 0) return 'tone-danger';
            if (account.usage.remainingPercent <= 30) return 'tone-warn';
        }

        if (Array.isArray(account.lines)) {
            for (const line of account.lines) {
                if (!line || line.type !== 'progress') continue;
                const used = Number(line.used || 0);
                const limit = Number(line.limit || 0);
                if (limit <= 0) continue;
                const pct = (used / limit) * 100;
                const remainingPct = line.format?.mode === 'remaining' ? pct : (100 - pct);
                if (remainingPct <= 0) return 'tone-danger';
                if (remainingPct <= 30) return 'tone-warn';
            }
        }
    }

    if (Number.isFinite(data.overview.totalBalanceUsd) && (data.overview.totalBalanceUsd <= 20 || data.overview.runwayDays <= 3)) {
        return 'tone-danger';
    }

    if (Number.isFinite(data.overview.totalBalanceUsd) && (data.overview.totalBalanceUsd <= 50 || data.overview.runwayDays <= 7)) {
        return 'tone-warn';
    }

    return 'tone-good';
}
const welcomeGuide = document.getElementById('welcomeGuide');
const guideClose = document.getElementById('guideClose');

guideClose.addEventListener('click', () => {
    welcomeGuide.classList.add('hidden');
    scheduleExpandedHeightSync();
});

viewTabs.querySelectorAll('.viewTab').forEach(tab => {
    tab.addEventListener('click', (event) => {
        event.stopPropagation();
        setActiveView(tab.dataset.view || 'home');
    });
});

rulesSearch.addEventListener('input', (event) => {
    rulesSearchQuery = event.target.value || '';
    renderRulesList();
    scheduleExpandedHeightSync();
});

rulesFilters.addEventListener('click', (event) => {
    const button = event.target.closest('.rulesFilter');
    if (!button) return;
    rulesSourceFilter = button.dataset.source || 'all';
    expandedRuleIndex = null;
    renderRulesList();
    scheduleExpandedHeightSync();
});

rulesUndoButton.addEventListener('click', async (event) => {
    event.stopPropagation();
    if (!pendingRuleUndo) return;

    const undo = pendingRuleUndo;
    clearPendingRuleUndo();
    try {
        approvalRules = await ipcRenderer.invoke('approval-rules:restore', {
            index: undo.index,
            rule: undo.rule
        });
        renderRulesList();
        scheduleExpandedHeightSync();
    } catch (err) {
        console.error('Restore approval rule failed', err);
        renderRuntimeWarning('Could not restore approval rule.');
    }
});

function renderSummary(data) {
    currentData = data;
    const accounts = data.accounts || [];
    const visibleAccounts = getPrioritizedVisibleAccounts(accounts);

    if (accounts.length === 0 && !pendingIntervention) {
        welcomeGuide.classList.remove('hidden');
    } else {
        welcomeGuide.classList.add('hidden');
    }

    const progressAccounts = visibleAccounts.filter(hasPillIndicator).slice(0, 5);
    const primaryAccount = visibleAccounts[0];

    if (progressAccounts.length) {
        primaryLabelText.innerText = progressAccounts.length > 1 ? '' : (primaryAccount ? primaryAccount.label : 'Provider');
        primaryValue.innerText = primaryAccount ? (primaryAccount.plan || 'Live') : 'Live';
        renderPillProgress(progressAccounts);
    } else if (primaryAccount) {
        primaryLabelText.innerText = primaryAccount.label || 'Provider';
        primaryValue.innerText = renderAccountHeadline(primaryAccount);
        hidePillProgress();
    } else {
        primaryLabelText.innerText = 'AgentIsOK';
        primaryValue.innerText = 'Live';
        hidePillProgress();
    }
    renderAgentStatusIcons(data.sessions || []);
    renderAccounts(data.accounts || [], false);
    renderSetupHealth(data);
    renderActiveView();
    renderSyncedAt(data.syncedAt);
    updateCompactVisibility();

    island.classList.remove('tone-neutral', 'tone-good', 'tone-warn', 'tone-danger');
    island.classList.add(getTone(data));
    
    if (island.classList.contains('expanded')) {
        scheduleExpandedHeightSync();
    }
}

function renderPillProgress(accounts) {
    if (!accounts.length) {
        hidePillProgress();
        return;
    }

    const displayAccounts = accounts.slice(0, 5);
    const compact = displayAccounts.length >= 5;

    pillProgress.classList.remove('hidden');
    pillContent.classList.add('pillContent-rings');
    if (compact) pillContent.classList.add('pillContent-rings-tight');
    primaryValue.classList.add('pillTextHidden');
    pillProgress.innerHTML = displayAccounts.map((account) => {
        if (account.provider === 'deepseek' && getPillMeterText(account)) {
            return renderPillStat(account);
        }

        const progressLine = getProgressLine(account);
        if (!progressLine) {
            return renderPillMeter(account, displayAccounts.length);
        }

        const used = Number(progressLine.used || 0);
        const limit = Number(progressLine.limit || 0);
        const percent = Math.max(0, Math.min(100, limit > 0 ? (used / limit) * 100 : 0));
        const displayPercent = progressLine.format?.mode === 'remaining' ? percent : Math.max(0, 100 - percent);
        const radius = displayAccounts.length > 1 ? (compact ? 10 : 12) : 17;
        const size = displayAccounts.length > 1 ? (compact ? 24 : 28) : 36;
        const center = size / 2;
        const circumference = 2 * Math.PI * radius;
        const offset = circumference * (1 - displayPercent / 100);
        const label = progressLine.format?.ringText || getProviderShortLabel(account);

        return `
            <div class="pillRing" style="width:${size}px;height:${size}px;flex:0 0 ${size}px">
                <svg class="pillRingSvg" viewBox="0 0 ${size} ${size}" aria-hidden="true">
                    <circle class="pillProgressTrack" cx="${center}" cy="${center}" r="${radius}"></circle>
                    <circle class="pillProgressFill" cx="${center}" cy="${center}" r="${radius}"
                        style="stroke-dasharray:${circumference};stroke-dashoffset:${offset};"></circle>
                </svg>
                <span class="pillRingText">${escapeHtml(label)}</span>
            </div>
        `;
    }).join('');

    syncPillWidth();
}

function renderPillStat(account) {
    const value = getPillStatValue(account);
    const provider = getProviderShortLabel(account);
    const tone = getPillStatTone(account);
    const compactClass = value.length >= 5 ? ' pillStat-compact' : '';

    return `
        <div class="pillStat ${tone}${compactClass}">
            <span class="pillStatProvider">${escapeHtml(provider)}</span>
            <span class="pillStatValue">${escapeHtml(value)}</span>
        </div>
    `;
}

function getPillStatValue(account) {
    const progressLine = getProgressLine(account);
    if (progressLine) {
        const limit = Number(progressLine.limit || 0);
        const used = Number(progressLine.used || 0);
        const isRemaining = (progressLine.format || {}).mode === 'remaining';
        const percent = limit > 0 ? Math.max(0, Math.min(100, (used / limit) * 100)) : 0;
        const displayPercent = isRemaining ? percent : percent;
        return `${Math.round(displayPercent)}%`;
    }

    const meter = getPillMeterText(account);
    return compactPillText(meter || '--');
}

function getPillStatTone(account) {
    const progressLine = getProgressLine(account);
    if (!progressLine) {
        return 'pillStat-balance';
    }

    const limit = Number(progressLine.limit || 0);
    const used = Number(progressLine.used || 0);
    const percent = limit > 0 ? Math.max(0, Math.min(100, (used / limit) * 100)) : 0;
    const score = progressLine.format?.mode === 'remaining' ? percent : Math.max(0, 100 - percent);

    if (score <= 20) return 'pillStat-danger';
    if (score <= 45) return 'pillStat-warn';
    return 'pillStat-good';
}

function renderPillMeter(account, count) {
    const text = getPillMeterText(account);
    const compact = compactPillText(text);
    const fontSize = getPillMeterFontSize(compact, count);

    return `
        <div class="pillMeter" style="font-size:${fontSize}px">
            <span class="pillMeterProvider">${escapeHtml(getProviderShortLabel(account))}</span>
            <span class="pillMeterValue">${escapeHtml(compact)}</span>
        </div>
    `;
}

function hidePillProgress() {
    pillProgress.classList.add('hidden');
    pillContent.classList.remove('pillContent-rings', 'pillContent-rings-tight', 'pillContent-rings-right', 'pillContent-rings-hidden', 'pillContent-stats');
    primaryLabel.classList.remove('pillTextHidden');
    primaryValue.classList.remove('pillTextHidden');
    pillProgress.innerHTML = '';
}

function hasProgressRing(account) {
    return Boolean(getProgressLine(account));
}

function hasPillIndicator(account) {
    return hasProgressRing(account) || Boolean(getPillMeterText(account));
}

function getProgressLine(account) {
    return Array.isArray(account.lines)
        ? account.lines.find((line) => line && line.type === 'progress' && Number(line.limit) > 0)
        : null;
}

function getPillMeterText(account) {
    if (account?.usage && typeof account.usage.totalBalance === 'number') {
        return formatMoney(account.usage.totalBalance, account.usage.currency);
    }
    if (typeof account?.balanceUsd === 'number' && !Number.isNaN(account.balanceUsd)) {
        return formatUsd(account.balanceUsd);
    }
    return null;
}

function compactPillText(text) {
    const raw = String(text || '');
    const match = raw.match(/^([^0-9.-]*)(-?\d+(?:\.\d+)?)/);
    if (!match) {
        return raw.length > 7 ? raw.slice(0, 7) : raw;
    }

    const prefix = match[1] || '';
    const value = Number(match[2]);
    if (!Number.isFinite(value)) {
        return raw;
    }
    if (Math.abs(value) >= 1000000000) {
        return `${prefix}${(value / 1000000000).toFixed(1)}b`;
    }
    if (Math.abs(value) >= 1000000) {
        return `${prefix}${(value / 1000000).toFixed(1)}m`;
    }
    if (Math.abs(value) >= 10000) {
        return `${prefix}${Math.round(value / 1000)}k`;
    }
    if (Math.abs(value) >= 1000) {
        return `${prefix}${(value / 1000).toFixed(1)}k`;
    }
    if (Math.abs(value) >= 100) {
        return `${prefix}${Math.round(value)}`;
    }
    return `${prefix}${value.toFixed(1)}`;
}

function getPillMeterFontSize(text, count) {
    const length = String(text || '').length;
    if (count > 3 || length > 6) return 10;
    if (length > 4) return 11;
    return 12;
}

function getProviderShortLabel(account) {
    const map = {
        codex: 'C',
        claude: 'A',
        gemini: 'A',
        minimax: 'M',
        cursor: 'R',
        deepseek: 'D',
        opencode: 'O',
        kiro: 'K'
    };
    return map[account.provider] || String(account.label || '?').slice(0, 1).toUpperCase();
}

function getProviderShortLabelByKey(provider, fallbackLabel = '?') {
    const map = {
        codex: 'C',
        claude: 'A',
        gemini: 'A',
        minimax: 'M',
        cursor: 'R',
        deepseek: 'D',
        opencode: 'O',
        kiro: 'K'
    };
    return map[provider] || String(fallbackLabel || '?').slice(0, 1).toUpperCase();
}

function renderProviderBadge(provider, fallbackLabel) {
    const shortLabel = getProviderShortLabelByKey(provider, fallbackLabel);
    return `<span class="providerBadge provider-${escapeHtml(provider || 'unknown')}">${escapeHtml(shortLabel)}</span>`;
}

function getVisibleAccounts(accounts) {
    const visibleProviders = Object.entries(providerVisibility)
        .filter(([, info]) => info.visible)
        .map(([key]) => key);

    if (!visibleProviders.length) {
        return accounts;
    }

    return accounts.filter((account) => visibleProviders.includes(account.provider));
}

function getPrioritizedVisibleAccounts(accounts) {
    return getVisibleAccounts(accounts)
        .slice()
        .sort((left, right) => getAccountPriority(left) - getAccountPriority(right));
}

function renderAccountCard(account) {
    const plan = account.plan ? `<div class="accountPlanRow"><span class="accountPlan">${escapeHtml(account.plan)}</span></div>` : '';
    const lines = Array.isArray(account.lines) ? account.lines.slice(0, 3).map((line) => renderAccountLine(line)).join('') : '';
    const statusClass = account.status ? ` account-${escapeHtml(account.status)}` : '';
    const tip = getAccountTip(account);
    const tipBadge = tip ? renderTipBadge(tip) : '';
    const providerBadge = renderProviderBadge(account.provider, account.label);

    return `
        <div class="accountCard${statusClass}">
            <div class="accountHead">
                <span class="accountNameWrap">
                    ${providerBadge}
                    <span class="accountName">${escapeHtml(account.label || account.provider || 'Account')}</span>
                    ${tipBadge}
                </span>
                <span class="accountValue">${renderAccountHeadline(account)}</span>
            </div>
            ${plan}
            <div class="accountLines">${lines}</div>
        </div>
    `;
}

function renderTipBadge(tip) {
    return `<span class="tipBadge" data-tip="${escapeHtml(tip)}">?</span>`;
}

function getAccountTip(account) {
    if (!account) {
        return null;
    }

    if (account.status === 'error') {
        return account.message || getProviderSetupTip(account.provider);
    }

    if (account.status === 'stale') {
        return account.message || 'Login is stale. Refresh the provider login, then Sync.';
    }

    if (!Array.isArray(account.lines) || account.lines.length === 0) {
        return getProviderSetupTip(account.provider);
    }

    if (account.message) {
        return account.message;
    }

    return null;
}

function renderAccountHeadline(account) {
    if (account.status === 'stale') {
        return 'Stale';
    }

    if (account.meta && account.meta.manualPlan) {
        return account.meta.manualPlan;
    }

    if (typeof account.balanceUsd === 'number' && !Number.isNaN(account.balanceUsd)) {
        return formatUsd(account.balanceUsd);
    }

    if (account.usage && typeof account.usage.totalBalance === 'number') {
        return formatMoney(account.usage.totalBalance, account.usage.currency);
    }

    if (account.usage && typeof account.usage.remainingPercent === 'number') {
        return `${Math.round(account.usage.remainingPercent)}% left`;
    }

    if (account.provider === 'codex' && account.meta && account.meta.planType) {
        return account.plan || 'ChatGPT login';
    }

    if (account.plan) {
        return account.plan;
    }

    return '--';
}

function renderAccountLine(line) {
    if (!line) {
        return '';
    }

    if (line.type === 'progress') {
        return renderProgressLine(line);
    }

    return `
        <div class="accountLine accountTextLine">
            <span class="lineLabel">${escapeHtml(line.label || '')}</span>
            <span class="lineValue">${escapeHtml(line.value || '--')}</span>
        </div>
        ${line.subtitle ? `<div class="lineSub">${escapeHtml(line.subtitle)}</div>` : ''}
    `;
}

function renderProgressLine(line) {
    const limit = Number(line.limit || 0);
    const used = Number(line.used || 0);
    const isRemaining = (line.format || {}).mode === 'remaining';
    const percent = limit > 0 ? Math.max(0, Math.min(100, (used / limit) * 100)) : 0;
    const resetText = line.resetsAt ? ` · ${formatResetDate(line.resetsAt)}` : '';
    
    let valueLabel = '';
    const format = line.format || { kind: 'currency', currency: 'USD' };
    
    if (format.kind === 'percent') {
        valueLabel = `${Math.round(isRemaining ? percent : used)}%`;
    } else if (format.kind === 'count') {
        valueLabel = `${used}${format.suffix ? ` ${format.suffix}` : ''}`;
    } else {
        valueLabel = formatUsd(used);
    }

    let limitLabel = '';
    if (format.kind === 'percent') {
        limitLabel = '100%';
    } else if (format.kind === 'count') {
        limitLabel = `${limit}${format.suffix ? ` ${format.suffix}` : ''}`;
    } else {
        limitLabel = formatUsd(limit);
    }

    const subtitle = `${line.subtitle || `${valueLabel} / ${limitLabel}`}${resetText}`;

    return `
        <div class="accountLine accountProgressLine">
            <span class="lineLabel">${escapeHtml(line.label || 'Usage')}</span>
            <span class="lineValue">${format.kind === 'percent' ? valueLabel : `${Math.round(percent)}%`}</span>
        </div>
        <div class="progressTrack"><div class="progressFill" style="width:${percent}%"></div></div>
        <div class="lineSub">${escapeHtml(subtitle)}</div>
    `;
}

function renderAgentStatusIcons(sessions) {
    const meaningful = sessions
        .filter(s => s && s.source)
        .filter(s => String(s.status || '').toLowerCase() !== 'done')
        .slice(0, 5);

    const existing = document.getElementById('agentStatusIcons');
    if (existing) {
        existing.remove();
    }

    // Add/remove indicator dot on logo instead of separate icons
    const logoRingDot = primaryLabel.querySelector('.logoRingDot');
    if (logoRingDot) {
        if (meaningful.length > 0) {
            logoRingDot.style.fill = 'var(--c-ok)';
            logoRingDot.style.filter = 'drop-shadow(0 0 3px var(--c-ok))';
        } else {
            logoRingDot.style.fill = '';
            logoRingDot.style.filter = '';
        }
    }

    if (!meaningful.length) {
        return;
    }

    const container = document.createElement('div');
    container.id = 'agentStatusIcons';
    container.className = 'agentStatusIcons';

    meaningful.forEach(session => {
        const status = getAgentStatus(session);
        const chip = document.createElement('div');
        chip.className = `agentStatusChip ${status} source-${session.source || 'agent'}`;
        chip.title = `${formatSource(session.source)}: ${status}`;
        chip.innerHTML = '<span class="agentBar"></span><span class="agentBar"></span><span class="agentBar"></span>';
        container.appendChild(chip);
    });

    primaryLabel.appendChild(container);

    // Re-sync pill width to account for icons
    syncPillWidth();
}

function getAgentStatus(session) {
    const status = String(session.status || '').toLowerCase();
    const lastEvent = String(session.lastEvent || '').toLowerCase();

    if (status === 'waiting' || lastEvent.includes('permission') || lastEvent.includes('ask')) {
        return 'waiting';
    }
    if (status === 'active' || status === 'running' || status === 'working') {
        return 'running';
    }
    return 'idle';
}

function getAgentStatusIcon(status) {
    switch (status) {
        case 'running':
            return '▶';
        case 'waiting':
            return '⏸';
        case 'idle':
            return '○';
        default:
            return '○';
    }
}

function renderSessions(sessions) {
    const meaningful = sessions
        .filter(s => s && s.source)
        .filter(s => String(s.status || '').toLowerCase() !== 'done')
        .slice(0, 5);
    if (!meaningful.length) {
        sessionsList.classList.remove('hidden');
        sessionsList.innerHTML = `
            <div class="agentsShell">
                <div class="emptyState">No active agents</div>
            </div>
        `;
        return;
    }

    if (!selectedAgentId || !meaningful.some(s => s.id === selectedAgentId)) {
        selectedAgentId = meaningful[0].id;
    }
    const selected = meaningful.find(s => s.id === selectedAgentId) || meaningful[0];
    sessionsList.classList.remove('hidden');
    sessionsList.innerHTML = `
        <div class="agentsShell">
            <div class="agentList">
                <div class="sessionsLabel">Running Agents</div>
                ${meaningful.map((session) => `
                <button class="agentListItem source-${session.source || 'agent'}${session.id === selectedAgentId ? ' active' : ''}" data-agent-id="${escapeHtml(session.id)}" type="button">
                    <span class="agentDot"></span>
                    <span class="agentListMain">
                        <span class="sessionName">${formatSource(session.source)}</span>
                        <span class="sessionActivity">${escapeHtml(session.activity || session.summary || session.status || 'Working')}</span>
                    </span>
                </button>
                `).join('')}
            </div>
            <div class="agentDetail source-${selected.source || 'agent'}">
                <div class="agentDetailTop">
                    <div>
                        <div class="agentTitle">${formatSource(selected.source)}</div>
                        <div class="sessionTime">${escapeHtml(formatSessionMeta(selected))}</div>
                    </div>
                    <span class="sessionStatus">${escapeHtml(selected.status || 'Active')}</span>
                </div>
                <div class="agentSummary">${escapeHtml(selected.activity || selected.summary || 'Working')}</div>
                <pre class="agentActivityFull">${escapeHtml(selected.activityDetail || selected.activity || selected.summary || 'No detail yet')}</pre>
                <div class="agentDetailGrid">
                    ${renderAgentField('Tool', selected.toolName || '--')}
                    ${renderAgentField('Event', selected.lastEvent || '--')}
                    ${renderAgentField('Session', compactText(selected.id || '--', 34))}
                    ${renderAgentField('File', selected.filePath || '--', true)}
                    ${renderAgentField('Command', selected.command || '--', true)}
                    ${renderAgentField('Terminal', selected.jumpTarget?.terminalApp || '--')}
                    ${renderAgentField('TTY', selected.jumpTarget?.terminalTTY || '--')}
                    ${renderAgentField('Working dir', selected.jumpTarget?.workingDirectory || '--', true)}
                </div>
                ${renderAgentTimeline(selected.events || [])}
                ${selected.jumpTarget ? `<button class="sessionJump agentJump" data-target='${escapeHtml(JSON.stringify(selected.jumpTarget))}' type="button">Jump to Terminal</button>` : ''}
            </div>
        </div>
    `;

    sessionsList.querySelectorAll('.agentListItem').forEach(btn => {
        btn.addEventListener('click', (e) => {
            e.stopPropagation();
            selectedAgentId = btn.dataset.agentId;
            renderSessions(currentData?.sessions || []);
            scheduleExpandedHeightSync();
        });
    });
    sessionsList.querySelectorAll('.sessionJump').forEach(btn => {
        btn.addEventListener('click', async (e) => {
            e.stopPropagation();
            try {
                const target = JSON.parse(btn.dataset.target);
                await ipcRenderer.invoke('island:jump-to-terminal', { target });
            } catch (err) {
                console.error('Session jump failed', err);
            }
        });
    });
}

function renderAgentTimeline(events) {
    if (!events.length) {
        return '<div class="emptyState compactEmpty">No timeline yet</div>';
    }
    return `
        <div class="agentTimeline">
            <div class="sessionsLabel">Timeline</div>
            ${events.slice(0, 6).map(event => `
                <div class="timelineItem">
                    <span class="timelineDot"></span>
                    <div class="timelineBody">
                        <div class="timelineTop">
                            <span>${escapeHtml(String(event.event || '--').replace(/_/g, ' '))}</span>
                            <em>${escapeHtml(formatTimelineTime(event.createdAt))}</em>
                        </div>
                        <div class="timelineSummary">${escapeHtml(event.summary || event.detail || '--')}</div>
                    </div>
                </div>
            `).join('')}
        </div>
    `;
}

function formatTimelineTime(value) {
    const time = Number(value || 0);
    if (time <= 0) return '';
    const seconds = Math.max(0, Math.floor((Date.now() - time) / 1000));
    if (seconds < 60) return `${seconds}s`;
    return `${Math.floor(seconds / 60)}m`;
}

function renderAgentField(label, value, wide = false) {
    return `
        <div class="agentField${wide ? ' wide' : ''}">
            <span>${escapeHtml(label)}</span>
            <strong>${escapeHtml(value || '--')}</strong>
        </div>
    `;
}

function formatSessionMeta(session) {
    const parts = [];
    if (session.lastEvent) {
        parts.push(String(session.lastEvent).replace(/_/g, ' '));
    }
    const time = Number(session.updatedAt || 0);
    if (time > 0) {
        const seconds = Math.max(0, Math.floor((Date.now() - time) / 1000));
        if (seconds < 60) {
            parts.push(`${seconds}s ago`);
        } else {
            parts.push(`${Math.floor(seconds / 60)}m ago`);
        }
    }
    return parts.join(' · ');
}

function renderHomeAgentSection(sessions) {
    const selected = sessions.find(s => s.id === selectedAgentId) || sessions[0];
    return `
        <div class="homeAgentsSection">
            <div class="homeSectionLabel">Active Agents</div>
            <div class="homeAgentList">
                ${sessions.map(session => {
                    const status = getAgentStatus(session);
                    return `
                        <button class="homeAgentRow source-${session.source || 'agent'}${session.id === selectedAgentId ? ' active' : ''}" data-agent-id="${escapeHtml(session.id)}" data-jump-target='${session.jumpTarget ? escapeHtml(JSON.stringify(session.jumpTarget)) : ''}' type="button">
                            <span class="agentDot agentDot-${status}"></span>
                            <span class="homeAgentInfo">
                                <span class="homeAgentName">${formatSource(session.source)}</span>
                                <span class="homeAgentActivity">${escapeHtml(session.activity || session.summary || session.status || 'Working')}</span>
                            </span>
                            <span class="homeAgentTime">${escapeHtml(formatTimelineTime(session.updatedAt))}</span>
                        </button>
                    `;
                }).join('')}
            </div>
            ${selected ? renderHomeAgentDetail(selected) : ''}
        </div>
    `;
}

function renderHomeAgentDetail(session) {
    if (!session) return '';
    const jumpInfo = session.jumpTarget || {};
    const hasFile = session.filePath && session.filePath !== '--';
    const hasCommand = session.command && session.command !== '--';
    const hasTool = session.toolName && session.toolName !== '--';
    const hasEvent = session.lastEvent;
    const hasActivityDetail = session.activityDetail && session.activityDetail !== session.activity;
    const events = session.events || [];

    const fields = [];
    if (hasTool) fields.push(renderAgentField('Tool', session.toolName));
    if (hasEvent) fields.push(renderAgentField('Event', session.lastEvent));
    fields.push(renderAgentField('Session', compactText(session.id || '--', 34)));
    if (hasFile) fields.push(renderAgentField('File', session.filePath, true));
    if (hasCommand) fields.push(renderAgentField('Command', session.command, true));
    if (jumpInfo.terminalApp) fields.push(renderAgentField('Terminal', jumpInfo.terminalApp));
    if (jumpInfo.terminalTTY) fields.push(renderAgentField('TTY', jumpInfo.terminalTTY));
    if (jumpInfo.workingDirectory) fields.push(renderAgentField('Working dir', jumpInfo.workingDirectory, true));

    const gridHtml = fields.length
        ? `<div class="agentDetailGrid">${fields.join('')}</div>`
        : '';

    const detailHtml = hasActivityDetail
        ? `<pre class="agentActivityFull">${escapeHtml(session.activityDetail)}</pre>`
        : '';

    const timelineHtml = events.length ? renderAgentTimeline(events) : '';

    return `
        <div class="homeAgentDetail">
            <div class="homeAgentDetailTop">
                <div>
                    <div class="homeAgentDetailTitle">${formatSource(session.source)}</div>
                    <div class="homeAgentDetailMeta">${escapeHtml(formatSessionMeta(session))}</div>
                </div>
                <span class="homeAgentDetailStatus">${escapeHtml(session.status || 'Active')}</span>
            </div>
            <div class="homeAgentDetailSummary">${escapeHtml(session.activity || session.summary || 'Working')}</div>
            ${detailHtml}
            ${gridHtml}
            ${timelineHtml}
            ${session.jumpTarget ? `<button class="sessionJump agentJump" data-target='${escapeHtml(JSON.stringify(session.jumpTarget))}' type="button">Jump to Terminal</button>` : ''}
        </div>
    `;
}

function renderSetupHealth(data) {
    const accounts = data.accounts || [];
    const sessions = data.sessions || [];
    const visibleAccounts = getPrioritizedVisibleAccounts(accounts);
    const activeProviders = Object.entries(providerVisibility)
        .filter(([, info]) => info.visible)
        .map(([provider]) => provider);

    // Active agents with detail — merged from old Agents tab
    const activeSessions = sessions
        .filter(s => s && s.source)
        .filter(s => String(s.status || '').toLowerCase() !== 'done')
        .slice(0, 5);

    const agentSection = activeSessions.length
        ? renderHomeAgentSection(activeSessions)
        : `<div class="homeAgentsSection">
            <div class="homeSectionLabel">Active Agents</div>
            <div class="emptyState">No agents running</div>
        </div>`;

    // Token stats — compact, only if data exists
    const tokenSection = renderTodayTokenStatsCompact(visibleAccounts);

    // Provider health — collapsed to bottom, minimal
    const providerItems = activeProviders.slice(0, 8).map(provider => {
        const account = visibleAccounts.find(a => a.provider === provider);
        const label = providerVisibility[provider]?.label || account?.label || provider;
        const status = account?.status || 'setup';
        const tone = status === 'error' || status === 'stale' ? 'bad' : status === 'setup' ? 'setup' : 'ok';
        const text = tone === 'ok' ? 'OK' : status === 'stale' ? 'Stale' : status === 'error' ? 'Err' : '—';
        return `
            <div class="healthItem health-${tone}" title="${escapeHtml(label)}: ${text}">
                ${renderProviderBadge(provider, label)}
                <span class="healthState">${text}</span>
            </div>
        `;
    }).join('');

    setupHealth.innerHTML = `
        ${agentSection}
        ${tokenSection}
        <div class="healthGrid">${providerItems || '<div class="emptyState">No visible providers</div>'}</div>
    `;

    // Wire up agent click → re-render with selected agent detail
    setupHealth.querySelectorAll('.homeAgentRow').forEach(btn => {
        let clickTimer = null;
        let clickCount = 0;

        btn.addEventListener('click', (e) => {
            e.stopPropagation();
            clickCount++;

            if (clickCount === 1) {
                clickTimer = setTimeout(() => {
                    if (clickCount === 1) {
                        // Single click: select agent
                        selectedAgentId = btn.dataset.agentId;
                        renderSetupHealth(currentData);
                        scheduleExpandedHeightSync();
                    }
                    clickCount = 0;
                }, 250);
            } else if (clickCount === 2) {
                // Double click: jump to terminal
                clearTimeout(clickTimer);
                clickCount = 0;
                const jumpTarget = btn.dataset.jumpTarget;
                if (!jumpTarget) return;
                try {
                    const target = JSON.parse(jumpTarget);
                    ipcRenderer.invoke('island:jump-to-terminal', { target }).catch(err => {
                        console.error('Session jump failed', err);
                    });
                } catch (err) {
                    console.error('Invalid jump target', err);
                }
            }
        });
    });

    // Wire up jump button
    setupHealth.querySelectorAll('.sessionJump').forEach(btn => {
        btn.addEventListener('click', async (e) => {
            e.stopPropagation();
            try {
                const target = JSON.parse(btn.dataset.target);
                await ipcRenderer.invoke('island:jump-to-terminal', { target });
            } catch (err) {
                console.error('Session jump failed', err);
            }
        });
    });
}

function renderTodayTokenStatsCompact(accounts) {
    const stats = accounts
        .map(readAccountTokenStats)
        .filter(item => item.total > 0);
    const total = stats.reduce((sum, item) => sum + item.total, 0);

    if (!stats.length) {
        return '';
    }

    const topProviders = stats.slice(0, 3).map(item => `
        <div class="homeTokenChip">
            <span class="homeTokenChipLabel">${escapeHtml(item.label)}</span>
            <strong class="homeTokenChipValue">${escapeHtml(formatCompactNumber(item.total))}</strong>
        </div>
    `).join('');

    return `
        <div class="homeTokensCompact">
            <div class="homeTokensTotal">
                <span>Today</span>
                <strong>${escapeHtml(formatCompactNumber(total))}</strong>
                <span class="homeTokensUnit">tokens</span>
            </div>
            <div class="homeTokensChips">${topProviders}</div>
        </div>
    `;
}

function renderHealthUsageInline(account) {
    if (!account) {
        return '<div class="healthQuota muted">not synced</div>';
    }
    const tokenStats = readAccountTokenStats(account);
    if (tokenStats.total > 0) {
        return `
            <div class="healthQuota">
                <span>Today</span>
                <strong>${escapeHtml(formatCompactNumber(tokenStats.total))}</strong>
            </div>
        `;
    }

    const messageCount = readFirstNumber([account?.meta?.todayMessages, account?.usage?.todayMessages]);
    const sessionCount = readFirstNumber([account?.meta?.todaySessions, account?.usage?.todaySessions]);
    if (messageCount > 0 || sessionCount > 0) {
        return `
            <div class="healthQuota">
                <span>${escapeHtml(formatCompactNumber(messageCount))} msg</span>
                <strong>${escapeHtml(formatCompactNumber(sessionCount))} sess</strong>
            </div>
        `;
    }

    const firstLine = Array.isArray(account.lines) ? account.lines[0] : null;
    if (firstLine?.subtitle) {
        return `<div class="healthQuota muted">${escapeHtml(compactText(firstLine.subtitle, 24))}</div>`;
    }

    return '<div class="healthQuota muted">ready</div>';
}

function renderTodayTokenStats(accounts) {
    const stats = accounts
        .map(readAccountTokenStats)
        .filter(item => item.total > 0);
    const total = stats.reduce((sum, item) => sum + item.total, 0);
    const input = stats.reduce((sum, item) => sum + item.input, 0);
    const output = stats.reduce((sum, item) => sum + item.output, 0);
    const exactTotal = stats.reduce((sum, item) => sum + item.exactTotal, 0);
    const estimatedTotal = stats.reduce((sum, item) => sum + item.estimatedTotal, 0);
    const sources = stats.map(item => item.label).join(' · ');

    return `
        <div class="todayTokens">
            <div class="todayTokensMain">
                <span>Today Tokens</span>
                <strong>${escapeHtml(stats.length ? formatCompactNumber(total) : '--')}</strong>
            </div>
            <div class="todayTokensSplit">
                <span>In ${escapeHtml(stats.length ? formatCompactNumber(input) : '--')}</span>
                <span>Out ${escapeHtml(stats.length ? formatCompactNumber(output) : '--')}</span>
                <span>${escapeHtml(stats.length ? `Exact ${formatCompactNumber(exactTotal)} · Est ${formatCompactNumber(estimatedTotal)}` : 'No token data')}</span>
                <span>${escapeHtml(stats.length ? sources : 'Sync local agents')}</span>
            </div>
        </div>
    `;
}

function readAccountTokenStats(account) {
    const tokenUsage = account?.tokenUsage || {};
    const exactInput = readFirstNumber([
        tokenUsage.exactInput,
        account?.meta?.tokensInput,
        account?.usage?.tokens?.input,
        account?.usage?.tokensInput,
        account?.usage?.inputTokens,
    ]);
    const exactOutput = readFirstNumber([
        tokenUsage.exactOutput,
        account?.meta?.tokensOutput,
        account?.usage?.tokens?.output,
        account?.usage?.tokensOutput,
        account?.usage?.outputTokens,
    ]);
    const exactReasoning = readFirstNumber([tokenUsage.exactReasoning]);
    const exactTotal = readFirstNumber([tokenUsage.exactTotal]) || exactInput + exactOutput + exactReasoning;
    const estimatedInput = readFirstNumber([tokenUsage.estimatedInput]);
    const estimatedOutput = readFirstNumber([tokenUsage.estimatedOutput]);
    const estimatedTotal = readFirstNumber([tokenUsage.estimatedTotal]) || estimatedInput + estimatedOutput;
    return {
        input: exactInput + estimatedInput,
        output: exactOutput + exactReasoning + estimatedOutput,
        exactInput,
        exactOutput: exactOutput + exactReasoning,
        exactTotal,
        estimatedInput,
        estimatedOutput,
        estimatedTotal,
        total: exactTotal + estimatedTotal,
        label: account?.label || formatSource(account?.provider),
    };
}

function readFirstNumber(values) {
    for (const value of values) {
        const numeric = Number(value);
        if (Number.isFinite(numeric) && numeric > 0) {
            return numeric;
        }
    }
    return 0;
}

function renderHomeUsageFloor(accounts) {
    const tokenStats = accounts
        .map(readAccountTokenStats)
        .filter(item => item.total > 0)
        .sort((a, b) => b.total - a.total);
    const activity = accounts.reduce((acc, account) => {
        acc.messages += readFirstNumber([
            account?.meta?.todayMessages,
            account?.usage?.todayMessages,
        ]);
        acc.sessions += readFirstNumber([
            account?.meta?.todaySessions,
            account?.usage?.todaySessions,
        ]);
        acc.tools += readFirstNumber([
            account?.meta?.todayTools,
            account?.usage?.todayTools,
        ]);
        return acc;
    }, { messages: 0, sessions: 0, tools: 0 });
    const maxTokens = Math.max(...tokenStats.map(item => item.total), 1);
    const rows = tokenStats.length
        ? tokenStats.slice(0, 4).map(item => {
            const pct = Math.max(6, Math.round(item.total / maxTokens * 100));
            return `
                <div class="homeTokenRow">
                    <span>${escapeHtml(item.label)}</span>
                    <div><i style="width:${pct}%"></i></div>
                    <strong>${escapeHtml(formatCompactNumber(item.total))}</strong>
                </div>
            `;
        }).join('')
        : '<div class="homeTokenEmpty">Token data appears after local providers sync.</div>';
    const hasActivity = activity.messages > 0 || activity.sessions > 0 || activity.tools > 0;

    return `
        <div class="homeUsageFloor">
            <div class="homeActivityMini">
                <div><strong>${escapeHtml(hasActivity ? formatCompactNumber(activity.messages) : '--')}</strong><span>messages</span></div>
                <div><strong>${escapeHtml(hasActivity ? formatCompactNumber(activity.sessions) : '--')}</strong><span>sessions</span></div>
                <div><strong>${escapeHtml(hasActivity ? formatCompactNumber(activity.tools) : '--')}</strong><span>tools</span></div>
            </div>
            <div class="homeTokenBars">${rows}</div>
        </div>
    `;
}

function renderActiveView() {
    const compact = Boolean(pendingIntervention);
    island.classList.toggle('approvalView', compact);
    island.classList.toggle('usageView', !compact && activeView === 'usage');
    island.classList.toggle('panelView', !compact && ['home', 'usage', 'rules'].includes(activeView));
    viewTabs.classList.toggle('hidden', compact);
    setupHealth.classList.toggle('hidden', compact || activeView !== 'home');
    sessionsList.classList.toggle('viewHidden', compact || activeView !== 'home');
    document.querySelector('.topStack')?.classList.toggle('hidden', compact || activeView !== 'usage');
    accountsList.classList.toggle('hidden', compact || activeView !== 'usage');
    rulesPanel.classList.toggle('hidden', compact || activeView !== 'rules');
    actionBar.classList.toggle('viewHidden', compact || activeView !== 'usage');
    renderRulesList();
    scheduleExpandedHeightSync();
}

function setActiveView(view) {
    if (view === 'activity' || view === 'agents') {
        view = 'home';
    }
    activeView = view;
    viewTabs.querySelectorAll('.viewTab').forEach(tab => {
        tab.classList.toggle('active', tab.dataset.view === view);
    });
    if (view === 'rules') {
        loadApprovalRules();
    }
    renderActiveView();
}

function renderRulesList() {
    if (activeView !== 'rules') {
        return;
    }
    renderRulesFilters();
    if (!approvalRules.length) {
        rulesList.innerHTML = '<div class="emptyState">No always-allow rules yet</div>';
        return;
    }
    const query = rulesSearchQuery.trim().toLowerCase();
    const indexedRules = approvalRules
        .map((rule, index) => ({ rule, index }))
        .filter(({ rule }) => rulesSourceFilter === 'all' || normalizeRuleSource(rule.source) === rulesSourceFilter)
        .filter(({ rule }) => !query || ruleSearchText(rule).includes(query));

    if (!indexedRules.length) {
        rulesList.innerHTML = '<div class="emptyState">No matching rules</div>';
        return;
    }

    rulesList.innerHTML = indexedRules.map(({ rule, index }) => {
        const subject = rule.prefixRule || rule.command || rule.filePath || rule.toolName || 'permission';
        const scope = renderRuleScope(rule);
        const source = formatSource(rule.source);
        const fullSubject = String(subject || 'permission');
        const expanded = expandedRuleIndex === index;
        return `
            <div class="ruleRow${expanded ? ' expanded' : ''}" data-index="${index}" title="${escapeHtml(fullSubject)}">
                <div class="ruleMain">
                    <span class="ruleSource">${escapeHtml(source)}</span>
                    <span class="ruleText">${escapeHtml(compactText(subject, expanded ? 260 : 78))}</span>
                    <span class="ruleMeta">${escapeHtml(scope)} · ${escapeHtml(rule.toolName || 'permission')} · ${formatRuleDate(rule.createdAt)}</span>
                </div>
                <button class="ruleDelete" data-index="${index}" type="button" aria-label="Remove rule" title="Remove">
                    <svg viewBox="0 0 24 24" aria-hidden="true">
                        <path d="M9 3h6l1 2h4v2H4V5h4l1-2Zm1 7h2v8h-2v-8Zm4 0h2v8h-2v-8ZM6 8h12l-1 13H7L6 8Z"></path>
                    </svg>
                </button>
            </div>
        `;
    }).join('');
    rulesList.querySelectorAll('.ruleRow').forEach(row => {
        row.addEventListener('click', (event) => {
            if (event.target.closest('.ruleDelete')) return;
            const index = Number(row.dataset.index);
            expandedRuleIndex = expandedRuleIndex === index ? null : index;
            renderRulesList();
            scheduleExpandedHeightSync();
        });
    });
    rulesList.querySelectorAll('.ruleDelete').forEach(button => {
        button.addEventListener('click', async (event) => {
            event.stopPropagation();
            const index = Number(button.dataset.index);
            const deletedRule = approvalRules[index];
            try {
                approvalRules = await ipcRenderer.invoke('approval-rules:delete', { index });
                if (expandedRuleIndex === index) {
                    expandedRuleIndex = null;
                } else if (expandedRuleIndex > index) {
                    expandedRuleIndex -= 1;
                }
                showRuleUndo(deletedRule, index);
                renderRulesList();
                scheduleExpandedHeightSync();
            } catch (err) {
                console.error('Delete approval rule failed', err);
                renderRuntimeWarning('Could not remove approval rule.');
            }
        });
    });
}

function renderRulesFilters() {
    const sources = Array.from(new Set(approvalRules.map(rule => normalizeRuleSource(rule.source)).filter(Boolean)));
    sources.sort((a, b) => {
        const ai = knownProviderOrder.indexOf(a);
        const bi = knownProviderOrder.indexOf(b);
        if (ai !== -1 && bi !== -1) return ai - bi;
        if (ai !== -1) return -1;
        if (bi !== -1) return 1;
        return a.localeCompare(b);
    });

    if (rulesSourceFilter !== 'all' && !sources.includes(rulesSourceFilter)) {
        rulesSourceFilter = 'all';
    }

    const filters = ['all', ...sources];
    rulesFilters.innerHTML = filters.map(source => {
        const label = source === 'all' ? 'All' : formatSource(source);
        const active = source === rulesSourceFilter ? ' active' : '';
        return `<button class="rulesFilter${active}" data-source="${escapeHtml(source)}" type="button">${escapeHtml(label)}</button>`;
    }).join('');
}

function normalizeRuleSource(source) {
    return String(source || 'agent').toLowerCase();
}

function showRuleUndo(rule, index) {
    if (!rule) return;
    clearPendingRuleUndo();
    pendingRuleUndo = {
        rule,
        index,
        timer: setTimeout(clearPendingRuleUndo, 3000)
    };
    rulesUndoText.innerText = `${formatSource(rule.source)} rule removed`;
    rulesUndo.classList.remove('hidden');
}

function clearPendingRuleUndo() {
    if (pendingRuleUndo?.timer) {
        clearTimeout(pendingRuleUndo.timer);
    }
    pendingRuleUndo = null;
    rulesUndo.classList.add('hidden');
}

function ruleSearchText(rule) {
    return [
        formatSource(rule.source),
        rule.source,
        rule.toolName,
        rule.command,
        rule.filePath,
        rule.prefixRule,
        renderRuleScope(rule),
    ].filter(Boolean).join(' ').toLowerCase();
}

function renderRuleScope(rule) {
    if (rule.prefixRule) {
        return 'prefix / sandbox rule';
    }
    if (rule.command) {
        return 'exact command';
    }
    if (rule.filePath) {
        return 'file path';
    }
    return 'tool scope';
}

async function loadApprovalRules() {
    try {
        approvalRules = await ipcRenderer.invoke('approval-rules:list');
        renderRulesList();
        scheduleExpandedHeightSync();
    } catch (err) {
        console.error('Load approval rules failed', err);
        approvalRules = [];
    }
}

function formatRuleDate(value) {
    const date = new Date(Number(value || 0));
    if (Number.isNaN(date.getTime())) {
        return 'unknown';
    }
    const mm = String(date.getMonth() + 1).padStart(2, '0');
    const dd = String(date.getDate()).padStart(2, '0');
    return `${mm}/${dd}`;
}

function renderIntervention(intervention) {
    const hadPending = Boolean(pendingIntervention);
    pendingIntervention = intervention;

    if (!intervention) {
        interventionPanel.classList.add('hidden');
        interventionAskOptions.classList.add('hidden');
        jumpToTerminalButton.classList.add('hidden');
        updateCompactVisibility();
        scheduleExpandedHeightSync();
        if (currentData) {
            island.classList.remove('tone-neutral', 'tone-good', 'tone-warn', 'tone-danger');
            island.classList.add(getTone(currentData));
        }
        if (hadPending) {
            collapseIsland();
        }
        return;
    }

    interventionPanel.classList.remove('hidden');
    interventionSource.innerText = formatSource(intervention.source);
    interventionTool.innerText = formatTool(intervention.toolName);
    renderInterventionRisk(intervention);
    interventionTitle.innerText = intervention.title || 'Approval required';
    interventionExplanation.innerText = intervention.explanation || '';
    interventionExplanation.classList.toggle('hidden', !intervention.explanation || intervention.explanation === intervention.detail);
    interventionThinking.textContent = intervention.thinking || '';
    interventionThinking.classList.toggle('hidden', !intervention.thinking);
    renderInterventionCommand(intervention.command || intervention.filePath);
    interventionDetail.innerText = renderDetailText(intervention);
    renderInterventionMeta(intervention);

    if (intervention.event === 'QuestionAsked' || intervention.toolName === 'AskUserQuestion') {
        interventionAskOptions.classList.remove('hidden');
        const options = extractAskOptions(intervention);
        interventionAskOptions.innerHTML = options.length > 0
            ? options.map((opt, i) => `<button class="btn askOption" data-index="${i}">${escapeHtml(opt)}</button>`).join('')
            : '';
        interventionAskOptions.querySelectorAll('.askOption').forEach(btn => {
            btn.addEventListener('click', () => {
                const idx = parseInt(btn.dataset.index, 10);
                sendInterventionDecision('approve', options[idx] || '');
            });
        });
        approveButton.innerText = 'Answer';
        approveAlwaysButton.classList.add('hidden');
    } else {
        interventionAskOptions.classList.add('hidden');
        approveButton.innerText = 'Approve';
        approveAlwaysButton.innerText = 'Allow Rule';
        approveAlwaysButton.classList.remove('hidden');
    }

    if (intervention.jumpTarget) {
        jumpToTerminalButton.classList.remove('hidden');
    } else {
        jumpToTerminalButton.classList.add('hidden');
    }

    updateCompactVisibility();
    applySourceTheme(intervention.source);
    island.classList.remove('tone-neutral', 'tone-good', 'tone-warn', 'tone-danger');
    island.classList.add('tone-danger');
    expandIsland();
    scheduleExpandedHeightSync();
}

function extractAskOptions(intervention) {
    const raw = intervention.raw || '';
    try {
        const payload = JSON.parse(raw);
        const question = payload.tool_input?.questions?.[0] || payload.toolInput?.questions?.[0] || payload.questions?.[0];
        const options = question?.options || payload.options || payload.choices || [];
        if (Array.isArray(options)) {
            return options.map(o => typeof o === 'string' ? o : (o.label || o.text || String(o)));
        }
    } catch (_) {}
    return [];
}

function formatSource(source) {
    if (source === 'claude') {
        return 'Claude';
    }
    if (source === 'codex') {
        return 'Codex';
    }
    if (source === 'gemini') {
        return 'Antigravity';
    }
    if (source === 'minimax') {
        return 'MiniMax';
    }
    if (source === 'opencode') {
        return 'OpenCode';
    }
    return 'Agent';
}

function formatTool(toolName) {
    if (!toolName) {
        return 'permission';
    }
    return String(toolName).replace(/_/g, ' ');
}

function renderInterventionMeta(intervention) {
    const items = [];

    if (intervention.prefixRule) {
        items.push(`rule ${compactText(intervention.prefixRule, 28)}`);
    }

    if (intervention.sandbox) {
        items.push(`perm ${compactText(intervention.sandbox, 18)}`);
    }

    if (!items.length) {
        interventionMeta.classList.add('hidden');
        interventionMeta.innerText = '';
        return;
    }

    interventionMeta.classList.remove('hidden');
    interventionMeta.innerText = items.join(' · ');
}

function renderInterventionCommand(command) {
    if (!command) {
        interventionCommand.classList.add('hidden');
        interventionCommand.textContent = '';
        return;
    }

    const text = String(command);
    interventionCommand.classList.remove('hidden');
    interventionCommand.textContent = compactText(text, 260);
}

function renderDetailText(intervention) {
    if (!intervention) {
        return '--';
    }

    if (intervention.explanation && intervention.detail && intervention.explanation !== intervention.detail) {
        return compactText(intervention.detail, 200);
    }

    if (intervention.command && intervention.detail === intervention.command) {
        return `${formatSource(intervention.source)} requested action`;
    }

    return compactText(intervention.detail || '--', 200);
}

function renderInterventionRisk(intervention) {
    const risk = inferInterventionRisk(intervention);
    interventionRisk.innerText = risk.label;
    interventionRisk.className = `riskBadge risk-${risk.level}`;
}

function inferInterventionRisk(intervention) {
    const text = [
        intervention.command,
        intervention.filePath,
        intervention.detail,
        intervention.toolName
    ].filter(Boolean).join(' ').toLowerCase();

    if (/(rm\s+-rf|sudo\s+|chmod\s+|chown\s+|dd\s+if=|mkfs|diskutil|format\s+|git\s+push|curl[^|;&]*\|\s*(sh|bash)|wget[^|;&]*\|\s*(sh|bash))/.test(text)) {
        return { level: 'danger', label: 'Danger' };
    }
    if (/(\.env|id_rsa|private[_-]?key|secret|token|credential|delete|remove|write|edit|patch|network|fetch|http)/.test(text)) {
        return { level: 'caution', label: 'Caution' };
    }
    if (/(\bls\b|\brg\b|\bgrep\b|\bcat\b|\bsed\b|\bgit status\b|\bgit diff\b|\bpwd\b)/.test(text)) {
        return { level: 'safe', label: 'Safe' };
    }
    return { level: 'caution', label: 'Caution' };
}

function compactText(value, maxLength) {
    const text = String(value || '').replace(/\s+/g, ' ').trim();
    if (!text) {
        return '--';
    }
    return text.length > maxLength ? `${text.slice(0, maxLength)}…` : text;
}

function escapeHtml(value) {
    return String(value || '')
        .replace(/&/g, '&amp;')
        .replace(/</g, '&lt;')
        .replace(/>/g, '&gt;')
        .replace(/"/g, '&quot;')
        .replace(/'/g, '&#39;');
}

function formatMoney(value, currency) {
    const numeric = Number(value);
    if (!Number.isFinite(numeric)) {
        return '--';
    }
    const symbol = currency === 'USD' ? '$' : currency === 'CNY' ? '¥' : `${currency || ''} `;
    return `${symbol}${numeric.toFixed(2)}`;
}

function formatCompactNumber(value) {
    const numeric = Number(value);
    if (!Number.isFinite(numeric) || numeric <= 0) {
        return '0';
    }
    if (numeric >= 1_000_000) {
        return `${(numeric / 1_000_000).toFixed(1)}M`;
    }
    if (numeric >= 1000) {
        return `${(numeric / 1000).toFixed(1)}k`;
    }
    return `${Math.round(numeric)}`;
}

function formatResetDate(value) {
    const date = new Date(value);
    if (Number.isNaN(date.getTime())) {
        return '--';
    }
    const diffMs = date.getTime() - Date.now();
    if (diffMs <= 0) {
        return 'now';
    }
    const totalMin = Math.floor(diffMs / 60000);
    const h = Math.floor(totalMin / 60);
    const m = totalMin % 60;
    if (h > 0) {
        return m > 0 ? `${h}h ${m}m` : `${h}h`;
    }
    if (m > 0) {
        return `${m}m`;
    }
    const sec = Math.floor(diffMs / 1000);
    return `${sec}s`;
}

function updateCompactVisibility() {
    renderActiveView();
}

function scheduleExpandedHeightSync() {
    if (!island.classList.contains('expanded')) {
        return;
    }

    if (heightSyncFrame) {
        cancelAnimationFrame(heightSyncFrame);
    }

    heightSyncFrame = requestAnimationFrame(() => {
        heightSyncFrame = null;
        const islandStyle = window.getComputedStyle(island);
        const topPadding = Number.parseFloat(islandStyle.paddingTop) || 0;
        const bottomPadding = Number.parseFloat(islandStyle.paddingBottom) || 0;
        const expandedStyle = window.getComputedStyle(expandedContent);
        const gap = Number.parseFloat(expandedStyle.gap) || 0;
        const headerHeight = pillContent.offsetHeight || 0;
        const actionBarVisible = actionBar
            && !actionBar.classList.contains('compactHidden')
            && !actionBar.classList.contains('viewHidden');
        const actionBarHeight = actionBarVisible ? (actionBar.offsetHeight || 0) : 0;

        // Measure all children of expandedContent
        let contentHeight = 0;
        const children = Array.from(expandedContent.children);
        children.forEach(child => {
            if (child.classList.contains('hidden') || child.classList.contains('viewHidden')) return;

            if (child.id === 'accountsList') {
                // Sum child card heights directly to avoid flex-inflated scrollHeight.
                // flex:1 1 auto makes scrollHeight grow with window size — measuring
                // children gives us the true natural content height.
                const cards = Array.from(child.children);
                let listHeight = 0;
                cards.forEach((card, i) => {
                    listHeight += card.offsetHeight;
                    if (i < cards.length - 1) listHeight += gap;
                });
                contentHeight += listHeight;
            } else {
                contentHeight += child.offsetHeight;
                const style = window.getComputedStyle(child);
                const marginBottom = Number.parseFloat(style.marginBottom) || 0;
                contentHeight += marginBottom;
            }
        });

        // Add the container gap between visible children
        const visibleChildrenCount = children.filter(c => !c.classList.contains('hidden') && !c.classList.contains('viewHidden')).length;
        if (visibleChildrenCount > 1) {
            contentHeight += (visibleChildrenCount - 1) * gap;
        }

        const compactApproval = Boolean(pendingIntervention);
        const fixedPanelViews = ['home', 'agents', 'usage', 'rules'];
        const viewMinimum = compactApproval ? 245 : (fixedPanelViews.includes(activeView) ? 640 : 0);
        const naturalHeight = Math.ceil(topPadding + headerHeight + gap + contentHeight + bottomPadding + actionBarHeight);
        const desiredHeight = Math.max(viewMinimum, naturalHeight);

        // Cap expanded panels close to work area while keeping approvals compact.
        const maxH = Math.round(window.screen.availHeight * 0.92);
        const clampedHeight = Math.min(desiredHeight, maxH);

        if (Math.abs(clampedHeight - lastExpandedHeight) < 8) {
            return;
        }

        lastExpandedHeight = clampedHeight;
        ipcRenderer.send('island:set-expanded-height', clampedHeight);
    });
}

function syncPillWidth() {
    if (island.classList.contains('expanded')) {
        return;
    }

    // Dynamically adjust padding-left in rings mode to fit logo + agent icons
    if (pillContent.classList.contains('pillContent-rings')) {
        const labelW = primaryLabel.scrollWidth || primaryLabel.offsetWidth || 0;
        const dynamicPad = Math.max(41, labelW + 8);
        pillContent.style.paddingLeft = dynamicPad + 'px';
    } else {
        pillContent.style.paddingLeft = '';
    }

    const logoWidth = primaryLabel.scrollWidth || primaryLabel.offsetWidth || 0;
    const ringsWidth = pillProgress.offsetWidth || 0;
    const gap = 14;
    const islandStyle = window.getComputedStyle(island);
    const padLeft = Number.parseFloat(islandStyle.paddingLeft) || 0;
    const padRight = Number.parseFloat(islandStyle.paddingRight) || 0;
    const needed = Math.ceil(logoWidth + gap + ringsWidth + padLeft + padRight + 4);
    const baseW = 200;
    const clamped = Math.max(baseW, needed);

    if (Math.abs(clamped - lastPillWidth) < 6) {
        return;
    }
    lastPillWidth = clamped;
    ipcRenderer.send('island:set-pill-width', clamped);
}

function applySourceTheme(source) {
    interventionPanel.classList.remove('source-claude', 'source-codex', 'source-agent');

    if (source === 'claude') {
        interventionPanel.classList.add('source-claude');
        return;
    }

    if (source === 'codex') {
        interventionPanel.classList.add('source-codex');
        return;
    }

    interventionPanel.classList.add('source-agent');
}

ipcRenderer.on('island-data', (_event, data) => {
    renderSummary(data);
});

ipcRenderer.on('island-force-expand', () => {
    expandIsland();
});

ipcRenderer.on('island-open-view', (_event, view) => {
    setActiveView(view || 'home');
    expandIsland();
});

ipcRenderer.on('intervention-state', (_event, intervention) => {
    if (!intervention) {
        respondingInterventionId = null;
    }
    renderIntervention(intervention);
});

ipcRenderer.on('runtime-warning', (_event, warning) => {
    renderRuntimeWarning(warning);
});

ipcRenderer.on('hook-event', (_event, payload) => {
    if (currentData) {
        const sessions = currentData.sessions || [];
        const existingIdx = sessions.findIndex(s => s.id === payload.sessionID);
        const nextSession = {
            ...(existingIdx >= 0 ? sessions[existingIdx] : {}),
            id: payload.sessionID,
            source: payload.source,
            status: payload.status || (existingIdx >= 0 ? sessions[existingIdx].status : 'Active'),
            activity: payload.summary || (existingIdx >= 0 ? sessions[existingIdx].activity : ''),
            activityDetail: payload.activityDetail || (existingIdx >= 0 ? sessions[existingIdx].activityDetail : ''),
            toolName: payload.toolName || (existingIdx >= 0 ? sessions[existingIdx].toolName : ''),
            command: payload.command || (existingIdx >= 0 ? sessions[existingIdx].command : ''),
            filePath: payload.filePath || (existingIdx >= 0 ? sessions[existingIdx].filePath : ''),
            lastEvent: payload.event,
            updatedAt: Date.now(),
            jumpTarget: payload.jumpTarget || (existingIdx >= 0 ? sessions[existingIdx].jumpTarget : null),
        };
        const existingEvents = existingIdx >= 0 && Array.isArray(sessions[existingIdx].events)
            ? sessions[existingIdx].events
            : [];
        nextSession.events = payload.timelineEvent
            ? [payload.timelineEvent, ...existingEvents].slice(0, 8)
            : existingEvents;
        if (existingIdx >= 0) {
            sessions[existingIdx] = nextSession;
        } else {
            sessions.unshift(nextSession);
        }
        currentData.sessions = sessions.slice(0, 5);
        renderAgentStatusIcons(currentData.sessions || []);
        renderSetupHealth(currentData);
    }
});

ipcRenderer.on('island-window-state', (_event, nextState) => {
    windowState = nextState || windowState;
});

const checkUpdateButton = document.getElementById('checkUpdateButton');

checkUpdateButton.addEventListener('click', async () => {
    if (updateReadyToRestart) {
        ipcRenderer.send('app:restart');
        return;
    }
    checkUpdateButton.disabled = true;
    checkUpdateButton.innerText = 'Checking...';
    try {
        await checkForUpdates();
    } catch (error) {
        console.error('Update check failed', error);
        const message = error && (error.message || String(error));
        renderRuntimeWarning(message ? `Update check failed: ${message}` : 'Update check failed');
        showUpdateBanner('error', message || 'Update check failed');
        checkUpdateButton.disabled = false;
        checkUpdateButton.innerText = 'Check Updates';
    }
});

syncButton.addEventListener('click', async () => {
    syncButton.disabled = true;
    syncButton.classList.add('loading');
    const originalLabel = syncButton.getAttribute('aria-label') || 'Sync now';
    syncButton.setAttribute('aria-label', 'Syncing');
    syncButton.title = 'Syncing';
    try {
        const data = await ipcRenderer.invoke('island:sync-now');
        renderSummary(data);
    } catch (error) {
        console.error('Sync failed', error);
        renderRuntimeWarning('Sync failed. Try again in a moment.');
    } finally {
        syncButton.disabled = false;
        syncButton.classList.remove('loading');
        syncButton.setAttribute('aria-label', originalLabel);
        syncButton.title = 'Sync';
    }
});

syncIntervalDown.addEventListener('click', async () => {
    await nudgeSyncInterval(-1);
});

syncIntervalUp.addEventListener('click', async () => {
    await nudgeSyncInterval(1);
});

bindInterventionButton(approveButton, 'approve');
bindInterventionButton(approveAlwaysButton, 'approve_always');
bindInterventionButton(denyButton, 'deny');

jumpToTerminalButton.addEventListener('click', async (event) => {
    event.preventDefault();
    event.stopPropagation();
    if (!pendingIntervention || !pendingIntervention.jumpTarget) return;
    try {
        await ipcRenderer.invoke('island:jump-to-terminal', { target: pendingIntervention.jumpTarget });
    } catch (err) {
        console.error('Jump failed', err);
    }
});

function bindInterventionButton(button, decision) {
    button.dataset.decision = decision;
    button.addEventListener('click', (event) => {
        event.preventDefault();
        event.stopPropagation();
        const pointerHandledAt = Number(button.dataset.pointerHandledAt || 0);
        if (Date.now() - pointerHandledAt < 350) {
            return;
        }
        sendInterventionDecision(decision);
    });
    button.addEventListener('pointerup', (event) => {
        event.preventDefault();
        event.stopPropagation();
        button.dataset.pointerHandledAt = String(Date.now());
        sendInterventionDecision(decision);
    });
}

window.addEventListener('keydown', (event) => {
    if (!pendingIntervention) {
        return;
    }

    const modifierPressed = event.ctrlKey || event.metaKey;
    const actionModifierPressed = event.altKey;
    if (!modifierPressed || !actionModifierPressed) {
        return;
    }

    const key = event.key.toLowerCase();
    if (key === 'a') {
        sendInterventionDecision('approve');
        event.preventDefault();
    } else if (key === 'l') {
        sendInterventionDecision('approve_always');
        event.preventDefault();
    } else if (key === 'd') {
        sendInterventionDecision('deny');
        event.preventDefault();
    }
});

async function sendInterventionDecision(decision, answer = '') {
    if (!pendingIntervention) {
        renderRuntimeWarning('No pending approval.');
        return;
    }

    const requestId = pendingIntervention.id || `${pendingIntervention.source}:${pendingIntervention.createdAt}`;
    if (respondingInterventionId === requestId) {
        return;
    }

    respondingInterventionId = requestId;
    const activeButton = getInterventionDecisionButton(decision);
    const originalText = activeButton ? activeButton.innerText : '';
    approveButton.disabled = true;
    approveAlwaysButton.disabled = true;
    denyButton.disabled = true;
    if (activeButton) {
        activeButton.innerText = getInterventionPendingLabel(decision);
    }
    try {
        const ok = await ipcRenderer.invoke('intervention:respond', { decision, answer });
        if (ok && decision === 'approve_always') {
            loadApprovalRules();
        }
        if (ok) {
            renderIntervention(null);
            return;
        }
        if (!ok) {
            renderRuntimeWarning('Approval already cleared or expired.');
            respondingInterventionId = null;
            approveButton.disabled = false;
            approveAlwaysButton.disabled = false;
            denyButton.disabled = false;
            if (activeButton) {
                activeButton.innerText = originalText;
            }
        }
    } catch (err) {
        console.error('Intervention decision failed', err);
        const message = err && (err.message || String(err));
        renderRuntimeWarning(message ? `Approval failed: ${message}` : 'Approval failed.');
        respondingInterventionId = null;
        approveButton.disabled = false;
        approveAlwaysButton.disabled = false;
        denyButton.disabled = false;
        if (activeButton) {
            activeButton.innerText = originalText;
        }
    }
}

function getInterventionDecisionButton(decision) {
    if (decision === 'approve') return approveButton;
    if (decision === 'approve_always') return approveAlwaysButton;
    if (decision === 'deny') return denyButton;
    return null;
}

function getInterventionPendingLabel(decision) {
    if (decision === 'approve') return 'Approving...';
    if (decision === 'approve_always') return 'Saving...';
    if (decision === 'deny') return 'Denying...';
    return 'Working...';
}

function renderShortcuts() {
    const prefix = appPlatform === 'darwin' ? 'Cmd Opt' : 'Ctrl Alt';
    approveShortcut.innerText = `${prefix} A`;
    alwaysShortcut.innerText = `${prefix} L`;
    denyShortcut.innerText = `${prefix} D`;
}

function renderSettings() {
    const minutes = Number(settingsState.syncIntervalMinutes) || 10;
    syncIntervalValue.innerText = `${minutes}m`;
    const index = SYNC_INTERVAL_STEPS.indexOf(minutes);
    syncIntervalDown.disabled = index <= 0;
    syncIntervalUp.disabled = index === -1 || index >= SYNC_INTERVAL_STEPS.length - 1;
}

async function loadSettings() {
    const next = await ipcRenderer.invoke('settings:get');
    settingsState = {
        ...settingsState,
        ...(next || {})
    };
    renderSettings();
}

async function nudgeSyncInterval(direction) {
    const current = Number(settingsState.syncIntervalMinutes) || 10;
    const currentIndex = Math.max(0, SYNC_INTERVAL_STEPS.indexOf(current));
    const nextIndex = Math.min(
        SYNC_INTERVAL_STEPS.length - 1,
        Math.max(0, currentIndex + direction)
    );
    const nextMinutes = SYNC_INTERVAL_STEPS[nextIndex];
    if (nextMinutes === current) {
        return;
    }

    syncIntervalDown.disabled = true;
    syncIntervalUp.disabled = true;
    try {
        const saved = await ipcRenderer.invoke('settings:set-sync-interval', { minutes: nextMinutes });
        settingsState = {
            ...settingsState,
            ...(saved || {}),
        };
        renderSettings();
        settingsMeta.innerText = 'Saved. New cadence is active now.';
    } catch (error) {
        console.error('Failed to save sync interval', error);
        renderRuntimeWarning('Could not save sync cadence.');
    } finally {
        renderSettings();
    }
}

function renderRuntimeWarning(warning) {
    if (runtimeWarningTimer) {
        clearTimeout(runtimeWarningTimer);
        runtimeWarningTimer = null;
    }

    const message = typeof warning === 'string'
        ? warning
        : warning && typeof warning.message === 'string'
            ? warning.message
            : '';

    if (!message) {
        runtimeWarning.innerText = '';
        runtimeWarning.classList.add('hidden');
        scheduleExpandedHeightSync();
        return;
    }

    runtimeWarning.innerText = message;
    runtimeWarning.classList.remove('hidden');
    scheduleExpandedHeightSync();
    runtimeWarningTimer = setTimeout(() => {
        runtimeWarning.innerText = '';
        runtimeWarning.classList.add('hidden');
        scheduleExpandedHeightSync();
        runtimeWarningTimer = null;
    }, 9000);
}

function renderProviderToggles() {
    const providers = Object.entries(providerVisibility);
    if (!providers.length) {
        providerToggles.innerHTML = '';
        return;
    }

    providers.sort(([a], [b]) => {
        const ai = knownProviderOrder.indexOf(a);
        const bi = knownProviderOrder.indexOf(b);
        if (ai !== -1 && bi !== -1) return ai - bi;
        if (ai !== -1) return -1;
        if (bi !== -1) return 1;
        return a.localeCompare(b);
    });

    providerToggles.innerHTML = providers.map(([key, info]) => {
        const activeClass = info.visible ? ' active' : '';
        const tip = getProviderToggleTip(key);
        return `
            <div class="providerToggle" data-provider="${escapeHtml(key)}">
                <span class="providerToggleLabel">
                    ${renderProviderBadge(key, info.label)}
                    <span class="providerToggleName">${escapeHtml(info.label)}</span>
                    ${renderTipBadge(tip)}
                </span>
                <div class="toggleSwitch${activeClass}"></div>
            </div>
        `;
    }).join('');

    providerToggles.querySelectorAll('.providerToggle').forEach((el) => {
        el.addEventListener('click', (event) => {
            if (event.target.closest('.tipBadge')) {
                return;
            }
            const provider = el.dataset.provider;
            const current = providerVisibility[provider];
            if (current) {
                const newVisible = !current.visible;
                providerVisibility[provider].visible = newVisible;
                ipcRenderer.send('providers:set-visibility', provider, newVisible);
                renderProviderToggles();
                if (currentData) {
                    renderSummary(currentData);
                } else {
                    renderAccounts([], false);
                }
                scheduleExpandedHeightSync();
            }
        });
    });
}

function getProviderToggleTip(provider) {
    const account = currentData && Array.isArray(currentData.accounts)
        ? currentData.accounts.find((item) => item.provider === provider)
        : null;

    if (!account) {
        return getProviderSetupTip(provider);
    }

    if (account.status === 'error' || account.status === 'stale') {
        return account.message || null;
    }

    if (!Array.isArray(account.lines) || account.lines.length === 0) {
        return getProviderSetupTip(provider);
    }

    return account.message || getProviderSetupTip(provider);
}

function getProviderSetupTip(provider) {
    return PROVIDER_SETUP_TIPS[provider] || 'Enable this provider, complete its login or key setup, then Sync.';
}

function renderAccounts(accounts, syncHeight = true) {
    if (!accounts.length) {
        accountsList.innerHTML = '';
        return;
    }

    const visibleAccounts = getPrioritizedVisibleAccounts(accounts);

    if (!visibleAccounts.length) {
        accountsList.innerHTML = '<div style="font-size:10px;opacity:0.4;text-align:center;padding:4px;">All providers hidden</div>';
        return;
    }

    const liveAccounts = visibleAccounts.filter(account => !['setup', 'error', 'stale'].includes(account.status));
    const setupAccounts = visibleAccounts.filter(account => ['setup', 'error', 'stale'].includes(account.status));
    const sections = [];
    if (liveAccounts.length) {
        sections.push(liveAccounts.map((account) => renderAccountCard(account)).join(''));
    }
    if (setupAccounts.length) {
        sections.push(`
            <div class="sectionLabel">Needs setup</div>
            ${setupAccounts.map((account) => renderAccountCard(account)).join('')}
        `);
    }
    accountsList.innerHTML = sections.join('');

    if (syncHeight) {
        scheduleExpandedHeightSync();
    }
}

async function loadProviderVisibility() {
    providerVisibility = await ipcRenderer.invoke('providers:get-visibility');
    renderProviderToggles();
    if (currentData) {
        renderSummary(currentData);
    }
}

Promise.all([
    ipcRenderer.invoke('island:get-data'),
    ipcRenderer.invoke('island:get-intervention'),
    loadProviderVisibility(),
    loadSettings(),
    loadApprovalRules()
]).then(([data, intervention]) => {
    renderShortcuts();
    renderSummary(data);
    renderIntervention(intervention);
    loadVersion();
});

function showUpdateBanner(status, message) {
    const el = document.getElementById('settingsMeta');
    if (!el) return;
    el.classList.remove('hidden');
    switch (status) {
        case 'checking':
            el.textContent = 'Checking for updates...';
            break;
        case 'available':
            el.textContent = `v${escapeHtml(message)} available.`;
            break;
        case 'downloading':
            el.textContent = `Downloading v${escapeHtml(message)}...`;
            break;
        case 'installed':
            el.innerHTML = 'Update ready. <button id="restartButton" class="btn sync">Restart</button>';
            document.getElementById('restartButton')?.addEventListener('click', () => {
                ipcRenderer.send('app:restart');
            });
            break;
        case 'up-to-date':
            el.textContent = 'AgentIsOK is up to date.';
            setTimeout(() => { el.textContent = ''; el.classList.add('hidden'); }, 4000);
            break;
        case 'error':
            el.textContent = `Update failed: ${escapeHtml(message)}`;
            break;
        default: break;
    }
}

ipcRenderer.on('update-status', (_, payload) => {
    showUpdateBanner(payload.status, payload.version || payload.message || '');
    if (payload.status === 'available') {
        checkUpdateButton.innerText = 'Download';
        checkUpdateButton.disabled = false;
    } else if (payload.status === 'up-to-date' || payload.status === 'error') {
        checkUpdateButton.innerText = 'Check Updates';
        checkUpdateButton.disabled = false;
    } else if (payload.status === 'downloading') {
        updateReadyToRestart = false;
        checkUpdateButton.innerText = 'Downloading...';
        checkUpdateButton.disabled = true;
    } else if (payload.status === 'installed') {
        updateReadyToRestart = true;
        checkUpdateButton.innerText = 'Restart';
        checkUpdateButton.disabled = false;
    }
});

async function checkForUpdates() {
    showUpdateBanner('checking', '');
    const tauri = window.__TAURI__;
    if (!tauri || !tauri.updater || !tauri.updater.check) {
        throw new Error('Updater API unavailable');
    }

    const update = await tauri.updater.check();
    if (!update) {
        updateReadyToRestart = false;
        showUpdateBanner('up-to-date', '');
        checkUpdateButton.innerText = 'Check Updates';
        checkUpdateButton.disabled = false;
        return;
    }

    const version = update.version || '';
    showUpdateBanner('downloading', version);
    checkUpdateButton.innerText = 'Downloading...';
    checkUpdateButton.disabled = true;
    await update.downloadAndInstall();
    updateReadyToRestart = true;
    showUpdateBanner('installed', version);
    checkUpdateButton.innerText = 'Restart';
    checkUpdateButton.disabled = false;
}

function loadVersion() {
    try {
        const tauri = window.__TAURI__;
        if (tauri && tauri.app && tauri.app.getVersion) {
            tauri.app.getVersion().then(v => {
                document.getElementById('appVersion').textContent = `v${v}`;
            }).catch(() => {});
        }
    } catch (_) {}
}

function renderSyncedAt(syncedAt) {
    const el = document.getElementById('syncedAt');
    if (!el) return;
    if (!syncedAt || syncedAt === 0) {
        el.textContent = '';
        return;
    }
    const d = new Date(syncedAt);
    const hh = String(d.getHours()).padStart(2, '0');
    const mm = String(d.getMinutes()).padStart(2, '0');
    const ss = String(d.getSeconds()).padStart(2, '0');
    el.textContent = `Synced ${hh}:${mm}:${ss}`;
}
