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
const syncButton = document.getElementById('syncButton');
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
const approveShortcut = document.getElementById('approveShortcut');
const alwaysShortcut = document.getElementById('alwaysShortcut');
const denyShortcut = document.getElementById('denyShortcut');

const PROVIDER_SETUP_TIPS = {
    codex: 'Requires Codex login. Run codex login, restart Codex, then Sync.',
    claude: 'Requires Claude Code login and hooks. Restart Claude Code after enabling.',
    cursor: 'Requires Cursor local login before usage can be synced.',
    minimax: 'Requires MiniMax local login before plan usage can be synced.',
    gemini: 'Requires Gemini local login before usage can be synced.',
    deepseek: 'Requires DEEPSEEK_API_KEY in project .env or environment, then restart.'
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
let heightSyncDebounce = null;
let pendingDragMove = null;
let dragMoveFrame = null;
let windowState = { mode: 'pill' };
const DRAG_THRESHOLD = 8;

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
        'providers:get-visibility': 'providers_get_visibility'
    };

    const sendMap = {
        'island:set-mode': (mode) => tauri.core.invoke('island_set_mode', { mode }),
        'island:set-expanded-height': (height) => tauri.core.invoke('island_set_expanded_height', { height }),
        'island:drag-start': (mouse) => tauri.core.invoke('island_drag_start', { mouse }),
        'island:drag-move': (mouse) => tauri.core.invoke('island_drag_move', { mouse }),
        'island:drag-end': () => tauri.core.invoke('island_drag_end'),
        'intervention:respond': (decision) => tauri.core.invoke('intervention_respond', { decision }),
        'providers:set-visibility': (provider, visible) => tauri.core.invoke('providers_set_visibility', { provider, visible })
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

    if (island.classList.contains('pill')) {
        expandIsland();
    } else {
        collapseIsland();
    }
});

document.getElementById('pillContent').addEventListener('mousedown', (event) => {
    if (!island.classList.contains('pill')) {
        return;
    }

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
    const provider = account?.provider;
    const order = ['codex', 'claude', 'cursor', 'minimax', 'gemini', 'deepseek'];
    const index = order.indexOf(provider);
    
    // If not in our known list, put at the end
    if (index === -1) return 100;
    
    // Secondary priority: stale accounts go after non-stale in the same group
    // but the user mostly cares about the provider order.
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
        primaryLabelText.innerText = progressAccounts.length > 1 ? 'Providers' : (primaryAccount ? primaryAccount.label : 'Provider');
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

    pillProgress.classList.remove('hidden');
    pillContent.classList.add('pillContent-rings');
    primaryValue.classList.add('pillTextHidden');
    pillProgress.innerHTML = displayAccounts.map((account) => {
        const progressLine = getProgressLine(account);
        if (!progressLine) {
            return renderPillMeter(account, displayAccounts.length);
        }

        const used = Number(progressLine.used || 0);
        const limit = Number(progressLine.limit || 0);
        const percent = Math.max(0, Math.min(100, limit > 0 ? (used / limit) * 100 : 0));
        const displayPercent = progressLine.format?.mode === 'remaining' ? percent : Math.max(0, 100 - percent);
        const radius = displayAccounts.length > 1 ? 12 : 17;
        const size = displayAccounts.length > 1 ? 28 : 36;
        const center = size / 2;
        const circumference = 2 * Math.PI * radius;
        const offset = circumference * (1 - displayPercent / 100);
        const label = progressLine.format?.ringText || getProviderShortLabel(account);

        return `
            <div class="pillRing">
                <svg class="pillRingSvg" viewBox="0 0 ${size} ${size}" aria-hidden="true">
                    <circle class="pillProgressTrack" cx="${center}" cy="${center}" r="${radius}"></circle>
                    <circle class="pillProgressFill" cx="${center}" cy="${center}" r="${radius}"
                        style="stroke-dasharray:${circumference};stroke-dashoffset:${offset};"></circle>
                </svg>
                <span class="pillRingText">${escapeHtml(label)}</span>
            </div>
        `;
    }).join('');
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
    pillContent.classList.remove('pillContent-rings', 'pillContent-rings-right', 'pillContent-rings-hidden');
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
        deepseek: 'D'
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
        deepseek: 'D'
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
    const plan = account.plan ? `<span class="accountPlan">${escapeHtml(account.plan)}</span>` : '';
    const lines = Array.isArray(account.lines) ? account.lines.slice(0, 2).map((line) => renderAccountLine(line)).join('') : '';
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

    if (account.provider === 'claude' && account.plan) {
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
    if (!sessions.length) {
        sessionsList.classList.add('hidden');
        sessionsList.innerHTML = '';
        return;
    }

    sessionsList.classList.remove('hidden');
    sessionsList.innerHTML = sessions.slice(0, 2).map((session) => `
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
    interventionCommand.textContent = compactText(text, 132);
}

function renderDetailText(intervention) {
    if (!intervention) {
        return '--';
    }

    if (intervention.reason) {
        return compactText(intervention.reason, 120);
    }

    if (intervention.command && intervention.detail === intervention.command) {
        return `${formatSource(intervention.source)} requested action`;
    }

    return compactText(intervention.detail || '--', 64);
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

        const desiredHeight = Math.ceil(topPadding + headerHeight + gap + contentHeight + bottomPadding);

        // Cap: never exceed 80% of available work area
        const maxH = Math.round(window.screen.availHeight * 0.8);
        const clampedHeight = Math.min(desiredHeight, maxH);

        if (Math.abs(clampedHeight - lastExpandedHeight) < 8) {
            return;
        }

        lastExpandedHeight = clampedHeight;
        ipcRenderer.send('island:set-expanded-height', clampedHeight);
    });
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

ipcRenderer.on('island-window-state', (_event, nextState) => {
    windowState = nextState || windowState;
});

syncButton.addEventListener('click', async () => {
    syncButton.disabled = true;
    try {
        const data = await ipcRenderer.invoke('island:sync-now');
        renderSummary(data);
    } finally {
        syncButton.disabled = false;
    }
});

approveButton.addEventListener('click', () => {
    sendInterventionDecision('approve');
});

approveAlwaysButton.addEventListener('click', () => {
    sendInterventionDecision('approve_always');
});

denyButton.addEventListener('click', () => {
    sendInterventionDecision('deny');
});

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

function sendInterventionDecision(decision) {
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
    ipcRenderer.send('intervention:respond', decision);
}

function renderShortcuts() {
    const prefix = appPlatform === 'darwin' ? 'Cmd Opt' : 'Ctrl Alt';
    approveShortcut.innerText = `${prefix} A`;
    alwaysShortcut.innerText = `${prefix} L`;
    denyShortcut.innerText = `${prefix} D`;
}

function renderProviderToggles() {
    const providers = Object.entries(providerVisibility);
    if (!providers.length) {
        providerToggles.innerHTML = '';
        return;
    }

    providerToggles.innerHTML = providers.map(([key, info]) => {
        const activeClass = info.visible ? ' active' : '';
        const tip = getProviderToggleTip(key);
        return `
            <div class="providerToggle" data-provider="${escapeHtml(key)}">
                <span class="providerToggleLabel">
                    ${renderProviderBadge(key, info.label)}
                    ${escapeHtml(info.label)}
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
        return account.message || getProviderSetupTip(provider);
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
    loadProviderVisibility()
]).then(([data, intervention]) => {
    renderShortcuts();
    renderSummary(data);
    renderIntervention(intervention);
});
