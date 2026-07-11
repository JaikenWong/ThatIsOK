const { invoke } = window.__TAURI__.core;
const MENU_ITEMS = [
    ['open-activity','Open Home'],['open-agents','Running Agents'],['open-usage','Usage & Providers'],
    ['open-rules','Approval Rules'],['open-settings','Settings'],['sync','Sync Now'],['update','Version & Updates']
];
const SHORTCUTS = [['toggle','Toggle Island'],['approve','Approve'],['approveAlways','Approve Always'],['deny','Deny']];
let state = { trayMenu:[], shortcuts:{}, shortcutDisplay:{}, syncIntervalMinutes:10 };
let listening = null;

document.querySelectorAll('.navItem').forEach(button => button.addEventListener('click', () => {
    document.querySelectorAll('.navItem').forEach(item => item.classList.toggle('active', item === button));
    document.querySelectorAll('.page').forEach(page => page.classList.toggle('active', page.id === `${button.dataset.page}Page`));
    document.getElementById('pageTitle').textContent = button.textContent;
    document.getElementById('resetButton').hidden = ['general','integrations'].includes(button.dataset.page);
}));

function renderMenu() {
    const enabled = state.trayMenu.filter(id => !id.startsWith('sep-'));
    const order = [...enabled, ...MENU_ITEMS.map(([id]) => id).filter(id => !enabled.includes(id))];
    document.getElementById('menuList').innerHTML = order.map(id => {
        const label = MENU_ITEMS.find(item => item[0] === id)[1];
        return `<div class="menuRow" draggable="true" data-id="${id}"><span class="dragHandle">⠿</span><span class="menuLabel">${label}</span><small>${enabled.includes(id) ? 'Shown' : 'Hidden'}</small><label class="switch"><input type="checkbox" ${enabled.includes(id) ? 'checked' : ''}><i></i></label></div>`;
    }).join('');
    document.querySelectorAll('.menuRow input').forEach(input => input.addEventListener('change', () => {
        const id = input.closest('.menuRow').dataset.id;
        const next = [...enabled];
        if (input.checked) next.push(id); else next.splice(next.indexOf(id),1);
        saveMenu(next);
    }));
    bindDrag(enabled);
}

function bindDrag(enabled) {
    let source = null;
    document.querySelectorAll('.menuRow').forEach(row => {
        row.addEventListener('dragstart', () => { source = row.dataset.id; row.classList.add('dragging'); });
        row.addEventListener('dragend', () => row.classList.remove('dragging'));
        row.addEventListener('dragover', event => event.preventDefault());
        row.addEventListener('drop', event => {
            event.preventDefault(); const target = row.dataset.id;
            if (!enabled.includes(source) || !enabled.includes(target) || source === target) return;
            const next = enabled.filter(id => id !== source); next.splice(next.indexOf(target),0,source); saveMenu(next);
        });
    });
}

async function saveMenu(items) { state = {...state, ...await invoke('settings_set_tray_menu',{items})}; renderMenu(); }

function renderShortcuts() {
    document.getElementById('shortcutList').innerHTML = SHORTCUTS.map(([id,label]) => `<div class="shortcutRow"><span>${label}</span><button class="keyButton${listening === id ? ' listening' : ''}" data-id="${id}" type="button">${listening === id ? 'Press keys…' : state.shortcutDisplay[id]}</button></div>`).join('');
    document.querySelectorAll('.keyButton').forEach(button => button.addEventListener('click', () => { listening = button.dataset.id; renderShortcuts(); }));
}

window.addEventListener('keydown', async event => {
    if (!listening) return; event.preventDefault();
    if (event.key === 'Escape') { listening = null; renderShortcuts(); return; }
    if (['Meta','Control','Alt','Shift'].includes(event.key)) return;
    const parts=[]; if(event.metaKey)parts.push('Cmd'); if(event.ctrlKey)parts.push('Ctrl'); if(event.altKey)parts.push('Alt'); if(event.shiftKey)parts.push('Shift');
    let key=event.key; if(key===' ')key='Space'; else if(key.startsWith('Arrow'))key=key.slice(5); else if(key.length===1)key=key.toUpperCase();
    if (!parts.length) return;
    parts.push(key); const shortcuts={...state.shortcuts,[listening]:parts.join('+')};
    state={...state,...await invoke('settings_set_shortcuts',{shortcuts})}; listening=null; renderShortcuts();
});

document.getElementById('resetButton').addEventListener('click', () => {
    const page=document.querySelector('.navItem.active').dataset.page;
    if(page==='menu') saveMenu(MENU_ITEMS.map(item=>item[0]));
    if(page==='shortcuts') {
        const shortcuts={toggle:'Space',approve:'A',approveAlways:'L',deny:'D'};
        invoke('settings_set_shortcuts',{shortcuts}).then(result => { state={...state,...result}; renderShortcuts(); });
    }
});
document.getElementById('syncInterval').addEventListener('change', async event => { state={...state,...await invoke('settings_set_sync_interval',{minutes:Number(event.target.value)})}; });

function renderIntegration(status) {
    const badge=document.getElementById('integrationStatus');
    badge.textContent=status.enabled ? (status.healthy ? 'Connected' : 'Repairing') : 'Disabled';
    badge.className=`statusBadge ${status.enabled ? (status.healthy ? 'connected' : 'warning') : ''}`;
    document.getElementById('enableIntegration').disabled=status.enabled && status.healthy;
    document.getElementById('disableIntegration').disabled=!status.enabled;
}
async function updateIntegration(command, message) {
    const output=document.getElementById('integrationMessage'); output.textContent='Working…';
    try { renderIntegration(await invoke(command)); output.textContent=message; }
    catch(error) { output.textContent=`Could not update integration: ${error}`; }
}
document.getElementById('enableIntegration').addEventListener('click',()=>updateIntegration('integrations_enable','Agent integration enabled.'));
document.getElementById('disableIntegration').addEventListener('click',()=>updateIntegration('integrations_disable','Agent integration disabled.'));

(async function init(){ state={...state,...await invoke('settings_get')}; document.getElementById('syncInterval').value=state.syncIntervalMinutes; renderMenu(); renderShortcuts(); renderIntegration(await invoke('integrations_get')); })();
