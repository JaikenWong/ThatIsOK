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
const interventionTool = document.getElementById('interventionTool');
const interventionTitle = document.getElementById('interventionTitle');
const interventionCommand = document.getElementById('interventionCommand');
const interventionDetail = document.getElementById('interventionDetail');
const interventionMeta = document.getElementById('interventionMeta');
const approveButton = document.getElementById('approveButton');
const approveAlwaysButton = document.getElementById('approveAlwaysButton');
const denyButton = document.getElementById('denyButton');
const providerToggles = document.getElementById('providerToggles');
const runtimeWarning = document.getElementById('runtimeWarning');
const approveShortcut = document.getElementById('approveShortcut');
const alwaysShortcut = document.getElementById('alwaysShortcut');
const denyShortcut = document.getElementById('denyShortcut');

const PROVIDER_SETUP_TIPS = {
    codex: 'Requires Codex login. Run codex login, restart Codex, then Sync.',
    claude: 'Requires Claude Code login and hooks. Restart Claude Code after enabling.',
    cursor: 'Requires Cursor local login before usage can be synced.',
    minimax: 'Requires MINIMAX_API_KEY or MINIMAX_CN_API_KEY in environment, then Sync.',
    gemini: 'Requires Gemini local login before usage can be synced.',
    deepseek: 'Requires DEEPSEEK_API_KEY in project .env or environment, then restart.',
    opencode: 'Install ThatIsOK plugin: copy thatisok-opencode.js to ~/.config/opencode/plugins/, update config.json.',
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
        primaryLabelText.innerText = 'Status';
        primaryValue.innerText = 'Live';
        hidePillProgress();
    }

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
        'settings:set-sync-interval': 'settings_set_sync_interval'
    };

    const sendMap = {
        'island:set-mode': (mode) => tauri.core.invoke('island_set_mode', { mode }),
        'island:set-expanded-height': (height) => tauri.core.invoke('island_set_expanded_height', { height }),
        'island:drag-start': (mouse) => tauri.core.invoke('island_drag_start', { mouse }),
        'island:drag-move': (mouse) => tauri.core.invoke('island_drag_move', { mouse }),
        'island:drag-end': () => tauri.core.invoke('island_drag_end'),
        'intervention:respond': (decision) => tauri.core.invoke('intervention_respond', { decision }),
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

    if (island.classList.contains('pill')) {
        expandIsland();
    } else {
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

function renderSummary(data) {
    currentData = data;
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
        primaryLabelText.innerText = 'Status';
        primaryValue.innerText = 'Live';
        hidePillProgress();
    }
    renderSessions(data.sessions || []);
    renderAccounts(data.accounts || [], false);
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
        const percent = limit > 0 ? Math.max(0, Math.min(100, (used / limit) * 100)) : 0;
        const displayPercent = progressLine.format?.mode === 'remaining'
            ? percent
            : Math.max(0, Math.min(100, used));
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
        gemini: 'G',
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
        gemini: 'G',
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
    const percent = limit > 0 ? Math.max(0, Math.min(100, (used / limit) * 100)) : 0;
    const resetText = line.resetsAt ? ` · reset ${formatResetDate(line.resetsAt)}` : '';
    
    let valueLabel = '';
    const format = line.format || { kind: 'currency', currency: 'USD' };
    
    if (format.kind === 'percent') {
        valueLabel = `${Math.round(format.mode === 'remaining' ? percent : used)}%`;
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

function renderSessions(sessions) {
    const meaningful = sessions.filter(s => s.status);
    if (!meaningful.length) {
        sessionsList.classList.add('hidden');
        sessionsList.innerHTML = '';
        return;
    }

    sessionsList.classList.remove('hidden');
    sessionsList.innerHTML = meaningful.slice(0, 2).map((session) => `
        <div class="sessionRow">
            <span class="sessionName">${formatSource(session.source)}</span>
            <span class="sessionValue">${session.status || '--'}</span>
        </div>
    `).join('');
}

function renderIntervention(intervention) {
    const hadPending = Boolean(pendingIntervention);
    pendingIntervention = intervention;

    if (!intervention) {
        interventionPanel.classList.add('hidden');
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
    interventionTitle.innerText = intervention.title || 'Approval required';
    renderInterventionCommand(intervention.command || intervention.filePath);
    interventionDetail.innerText = renderDetailText(intervention);
    renderInterventionMeta(intervention);
    updateCompactVisibility();
    applySourceTheme(intervention.source);
    island.classList.remove('tone-neutral', 'tone-good', 'tone-warn', 'tone-danger');
    island.classList.add('tone-danger');
    expandIsland();
    scheduleExpandedHeightSync();
}

function formatSource(source) {
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

    if (intervention.reason) {
        return compactText(intervention.reason, 180);
    }

    if (intervention.command && intervention.detail === intervention.command) {
        return `${formatSource(intervention.source)} requested action`;
    }

    return compactText(intervention.detail || '--', 180);
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

function formatResetDate(value) {
    const date = new Date(value);
    if (Number.isNaN(date.getTime())) {
        return '--';
    }

    return `${date.getMonth() + 1}/${date.getDate()}`;
}

function updateCompactVisibility() {
    const compact = Boolean(pendingIntervention);
    sessionsList.classList.toggle('hidden', compact || !sessionsList.innerHTML);
    accountsList.classList.toggle('compactHidden', compact);
    syncButton.classList.toggle('compactHidden', compact);
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
        const actionBarHeight = actionBar && !actionBar.classList.contains('compactHidden') ? (actionBar.offsetHeight || 0) : 0;

        // Measure all children of expandedContent
        let contentHeight = 0;
        const children = Array.from(expandedContent.children);
        children.forEach(child => {
            if (child.classList.contains('hidden')) return;

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
        const visibleChildrenCount = children.filter(c => !c.classList.contains('hidden')).length;
        if (visibleChildrenCount > 1) {
            contentHeight += (visibleChildrenCount - 1) * gap;
        }

        const desiredHeight = Math.ceil(topPadding + headerHeight + gap + contentHeight + bottomPadding + actionBarHeight);

        // Cap: never exceed 92% of available work area
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

    const logoWidth = primaryLabel.offsetWidth || 0;
    const ringsWidth = pillProgress.offsetWidth || 0;
    const gap = 14;
    const islandStyle = window.getComputedStyle(island);
    const padLeft = Number.parseFloat(islandStyle.paddingLeft) || 0;
    const padRight = Number.parseFloat(islandStyle.paddingRight) || 0;
    const needed = Math.ceil(logoWidth + gap + ringsWidth + padLeft + padRight + 4);
    const minW = 140;
    const clamped = Math.max(minW, needed);

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

ipcRenderer.on('intervention-state', (_event, intervention) => {
    if (!intervention) {
        respondingInterventionId = null;
    }
    renderIntervention(intervention);
});

ipcRenderer.on('runtime-warning', (_event, warning) => {
    renderRuntimeWarning(warning);
});

ipcRenderer.on('island-window-state', (_event, nextState) => {
    windowState = nextState || windowState;
});

syncButton.addEventListener('click', async () => {
    syncButton.disabled = true;
    try {
        const data = await ipcRenderer.invoke('island:sync-now');
        renderSummary(data);
    } catch (error) {
        console.error('Sync failed', error);
        renderRuntimeWarning('Sync failed. Try again in a moment.');
    } finally {
        syncButton.disabled = false;
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

function bindInterventionButton(button, decision) {
    let pointerHandledAt = 0;
    const handler = (event) => {
        event.preventDefault();
        event.stopPropagation();
        if (event.type === 'click' && Date.now() - pointerHandledAt < 350) {
            return;
        }
        if (event.type === 'pointerup') {
            pointerHandledAt = Date.now();
        }
        sendInterventionDecision(decision);
    };
    button.addEventListener('pointerup', handler);
    button.addEventListener('click', handler);
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

async function sendInterventionDecision(decision) {
    if (!pendingIntervention) {
        return;
    }

    const requestId = pendingIntervention.id || `${pendingIntervention.source}:${pendingIntervention.createdAt}`;
    if (respondingInterventionId === requestId) {
        return;
    }

    respondingInterventionId = requestId;
    approveButton.disabled = true;
    approveAlwaysButton.disabled = true;
    denyButton.disabled = true;
    try {
        const ok = await ipcRenderer.invoke('intervention:respond', decision);
        if (!ok) {
            respondingInterventionId = null;
            approveButton.disabled = false;
            approveAlwaysButton.disabled = false;
            denyButton.disabled = false;
        }
    } catch (err) {
        console.error('Intervention decision failed', err);
        respondingInterventionId = null;
        approveButton.disabled = false;
        approveAlwaysButton.disabled = false;
        denyButton.disabled = false;
    }
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

    accountsList.innerHTML = visibleAccounts
        .map((account) => renderAccountCard(account))
        .join('');

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
    loadSettings()
]).then(([data, intervention]) => {
    renderShortcuts();
    renderSummary(data);
    renderIntervention(intervention);
    loadVersion();
});

function showUpdateBanner(status, message) {
    const el = document.getElementById('settingsMeta');
    if (!el) return;
    switch (status) {
        case 'checking':
            el.innerHTML = 'Checking for updates...';
            el.classList.add('update-banner');
            break;
        case 'available':
            el.innerHTML = `v${escapeHtml(message)} available. Click ThatIsOK v1.0.0 in tray to update.`;
            el.classList.add('update-banner');
            break;
        case 'downloading':
            el.innerHTML = `Downloading v${escapeHtml(message)}...`;
            el.classList.add('update-banner');
            break;
        case 'installed':
            el.innerHTML = 'Update ready. <button id="restartButton" class="btn sync">Restart</button>';
            el.classList.add('update-banner');
            document.getElementById('restartButton')?.addEventListener('click', () => {
                ipcRenderer.send('app:restart');
            });
            break;
        case 'up-to-date':
            el.innerHTML = 'ThatIsOK is up to date.';
            el.classList.add('update-banner');
            setTimeout(() => { el.innerHTML = ''; el.classList.remove('update-banner'); }, 4000);
            break;
        case 'error':
            el.innerHTML = `Update failed: ${escapeHtml(message)}`;
            el.classList.add('update-banner');
            break;
        default: break;
    }
}

ipcRenderer.on('update-status', (_, payload) => {
    showUpdateBanner(payload.status, payload.version || payload.message || '');
});

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
