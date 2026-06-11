const { app, BrowserWindow, ipcMain, nativeImage, screen, globalShortcut } = require('electron');
const path = require('path');

const { loadEnvFile } = require('./env-loader');
const Watcher = require('./watcher');
const SessionStore = require('./session-store');
const UsageStore = require('./storage/usage-store');
const { IPCServer } = require('./ipc-server');
const ConfigInjector = require('./config-injector');
const createTray = require('./tray');
const SyncService = require('./services/sync-service');
const InterventionManager = require('./intervention-manager');
const { LocalAPI } = require('./local-api');

loadEnvFile();

// Windows taskbar icon requires AUMI set before app is ready
if (process.platform === 'win32') {
  app.setAppUserModelId('com.thatisok.app');
}

let islandWindow;
let trayController;
let sessionStore;
let usageStore;
let watcher;
let syncService;
let ipcServer;
let syncTimer;
let interventionManager;
let localAPI;
let islandState = {
  mode: 'pill',
  expandedHeight: 404,
  dragStartBounds: null,
  dragStartMouse: null,
  lastDragBounds: null
};

const WINDOW_SIZES = {
  pill: { width: 248, height: 56 },
  expanded: { width: 356, height: 404 }
};

const WINDOW_MARGIN = 12;
const APP_ICON_PATH = path.join(__dirname, '..', 'assets', 'icon.png');
const TRAY_ICON_PATH = path.join(__dirname, '..', 'assets', 'icon-16.png');

function getTopCenterBounds(size) {
  const display = screen.getPrimaryDisplay();
  const area = display.workArea;
  return {
    width: size.width,
    height: size.height,
    x: Math.round(area.x + (area.width - size.width) / 2),
    y: Math.round(area.y + 12)
  };
}

function getWorkArea() {
  return screen.getPrimaryDisplay().workArea;
}

function getWindowSize(mode = islandState.mode) {
  if (mode === 'expanded') {
    return {
      width: WINDOW_SIZES.expanded.width,
      height: islandState.expandedHeight || WINDOW_SIZES.expanded.height
    };
  }

  return WINDOW_SIZES.pill;
}

function clampBounds(bounds, options = {}) {
  const area = options.workArea || getWorkArea();
  const allowOffscreenX = Boolean(options.allowOffscreenX);
  const allowOffscreenY = Boolean(options.allowOffscreenY);
  return {
    width: bounds.width,
    height: bounds.height,
    x: allowOffscreenX
      ? bounds.x
      : Math.min(Math.max(bounds.x, area.x + WINDOW_MARGIN), area.x + area.width - bounds.width - WINDOW_MARGIN),
    y: allowOffscreenY
      ? bounds.y
      : Math.min(Math.max(bounds.y, area.y + WINDOW_MARGIN), area.y + area.height - bounds.height - WINDOW_MARGIN)
  };
}

function applyIslandBounds(bounds) {
  if (!islandWindow || islandWindow.isDestroyed()) {
    return;
  }

  islandWindow.setBounds(bounds, false);
  broadcastWindowState();
}

function applyDragPosition(bounds) {
  if (!islandWindow || islandWindow.isDestroyed()) {
    return;
  }

  islandWindow.setPosition(Math.round(bounds.x), Math.round(bounds.y), false);
}

function broadcastWindowState() {
  if (!islandWindow || islandWindow.isDestroyed()) {
    return;
  }

  islandWindow.webContents.send('island-window-state', {
    mode: islandState.mode
  });
}

function createIslandWindow() {
  const bounds = getTopCenterBounds(WINDOW_SIZES.pill);
  islandState.mode = 'pill';

  islandWindow = new BrowserWindow({
    ...bounds,
    show: false,
    frame: false,
    transparent: true,
    hasShadow: false,
    resizable: false,
    movable: false,
    fullscreenable: false,
    minimizable: false,
    maximizable: false,
    closable: false,
    skipTaskbar: true,
    alwaysOnTop: true,
    icon: APP_ICON_PATH,
    backgroundColor: '#00000000',
    webPreferences: {
      nodeIntegration: true,
      contextIsolation: false
    }
  });

  if (process.platform === 'darwin') {
    islandWindow.setVisibleOnAllWorkspaces(true, { visibleOnFullScreen: true });
    islandWindow.setAlwaysOnTop(true, 'screen-saver');
  } else {
    islandWindow.setAlwaysOnTop(true, 'floating');
  }
  islandWindow.loadFile(path.join(__dirname, '..', 'renderer', 'index.html'));
  // islandWindow.webContents.openDevTools({ mode: 'detach' });
  islandWindow.once('ready-to-show', () => {
    islandWindow.showInactive();
    broadcastWindowState();
  });
}

function broadcastSummary() {
  if (!usageStore) {
    return;
  }

  const summary = usageStore.getDashboardData();
  summary.sessions = sessionStore ? sessionStore.getActiveSessions() : [];

  if (trayController) {
    trayController.updateSummary(summary);
  }

  if (islandWindow && !islandWindow.isDestroyed()) {
    islandWindow.webContents.send('island-data', summary);
    islandWindow.webContents.send('intervention-state', interventionManager ? interventionManager.getPending() : null);
    broadcastWindowState();
  }
}

async function syncNow() {
  if (!syncService) {
    return;
  }

  try {
    await syncService.syncAllAccounts();
  } catch (error) {
    console.error('Sync failed:', error);
  }

  broadcastSummary();
}

function setIslandMode(mode = 'pill') {
  if (!islandWindow || islandWindow.isDestroyed()) {
    return;
  }

  islandState.mode = mode;
  const size = getWindowSize(mode);
  const current = islandWindow.getBounds();
  const nextBounds = clampBounds({
    width: size.width,
    height: size.height,
    x: current.x,
    y: current.y
  });

  applyIslandBounds(nextBounds);
}

function startIslandDrag(mouse) {
  if (!islandWindow || islandWindow.isDestroyed()) {
    return;
  }

  islandState.dragStartBounds = islandWindow.getBounds();
  islandState.dragStartMouse = mouse;
}

function moveIslandDrag(mouse) {
  if (!islandWindow || islandWindow.isDestroyed() || !islandState.dragStartBounds || !islandState.dragStartMouse) {
    return;
  }

  const dx = mouse.x - islandState.dragStartMouse.x;
  const dy = mouse.y - islandState.dragStartMouse.y;
  
  const display = screen.getDisplayNearestPoint(mouse);
  const nextBounds = clampBounds({
    ...islandState.dragStartBounds,
    x: islandState.dragStartBounds.x + dx,
    y: islandState.dragStartBounds.y + dy
  }, {
    workArea: display.workArea
  });

  const roundedX = Math.round(nextBounds.x);
  const roundedY = Math.round(nextBounds.y);
  const previous = islandState.lastDragBounds;

  if (previous && previous.x === roundedX && previous.y === roundedY) {
    return;
  }

  islandState.lastDragBounds = { x: roundedX, y: roundedY };
  islandWindow.setPosition(roundedX, roundedY, false);
}

function endIslandDrag() {
  if (!islandWindow || islandWindow.isDestroyed()) {
    return;
  }

  const current = islandWindow.getBounds();
  const display = screen.getDisplayNearestPoint(screen.getCursorScreenPoint());

  islandState.dragStartBounds = null;
  islandState.dragStartMouse = null;
  islandState.lastDragBounds = null;

  applyIslandBounds(clampBounds(current, { workArea: display.workArea }));
}

function setupIPC() {
  ipcMain.handle('island:get-data', () => usageStore.getDashboardData());
  ipcMain.handle('island:get-intervention', () => interventionManager ? interventionManager.getPending() : null);
  ipcMain.handle('island:sync-now', async () => {
    await syncNow();
    return usageStore.getDashboardData();
  });
  ipcMain.on('island:set-mode', (_event, mode) => {
    setIslandMode(mode);
  });
  ipcMain.on('island:set-expanded-height', (_event, height) => {
    const nextHeight = Number(height);
    if (!Number.isFinite(nextHeight)) {
      return;
    }

    const area = getWorkArea();
    const MIN_EXPANDED_HEIGHT = 80;
    const maxHeight = Math.max(MIN_EXPANDED_HEIGHT, area.height - WINDOW_MARGIN * 2);
    const clampedHeight = Math.min(Math.max(Math.round(nextHeight), MIN_EXPANDED_HEIGHT), maxHeight);
    if (clampedHeight === islandState.expandedHeight) {
      return;
    }

    islandState.expandedHeight = clampedHeight;
    if (islandState.mode === 'expanded') {
      setIslandMode('expanded');
    }
  });
  ipcMain.on('island:drag-start', (_event, mouse) => {
    startIslandDrag(mouse);
  });
  ipcMain.on('island:drag-move', (_event, mouse) => {
    moveIslandDrag(mouse);
  });
  ipcMain.on('island:drag-end', () => {
    endIslandDrag();
  });
  ipcMain.on('intervention:respond', (_event, decision) => {
    respondToIntervention(decision);
  });

  ipcMain.handle('hooks:get-status', () => {
    return {
      claude: ConfigInjector.getClaudeStatus(),
      codex: ConfigInjector.getCodexStatus(),
      gemini: ConfigInjector.getGeminiStatus()
    };
  });

  ipcMain.handle('hooks:install', (_event, target) => {
    if (target === 'claude') {
      ConfigInjector.injectClaude();
    } else if (target === 'codex') {
      ConfigInjector.injectCodex();
    } else if (target === 'gemini') {
      ConfigInjector.injectGemini();
    }
    return {
      claude: ConfigInjector.getClaudeStatus(),
      codex: ConfigInjector.getCodexStatus(),
      gemini: ConfigInjector.getGeminiStatus()
    };
  });

  ipcMain.handle('hooks:uninstall', (_event, target) => {
    if (target === 'claude') {
      ConfigInjector.uninjectClaude();
    } else if (target === 'codex') {
      ConfigInjector.uninjectCodex();
    } else if (target === 'gemini') {
      ConfigInjector.uninjectGemini();
    }
    return {
      claude: ConfigInjector.getClaudeStatus(),
      codex: ConfigInjector.getCodexStatus(),
      gemini: ConfigInjector.getGeminiStatus()
    };
  });

  ipcMain.handle('providers:get-visibility', () => {
    return syncService.registry.getProviderVisibility();
  });

  ipcMain.on('providers:set-visibility', (_event, provider, visible) => {
    syncService.registry.setProviderVisibility(provider, visible);
    if (visible) {
      syncNow().catch((err) => console.error('Visibility-triggered sync failed:', err));
    } else {
      broadcastSummary();
    }
  });
}

function setupBridgeServer() {
  ipcServer = new IPCServer({
    'hook-event': async (data, callback) => {
      if (sessionStore) {
        sessionStore.upsertSession(data);
      }
      const result = await watcher.handleHookEvent(data, interventionManager);
      broadcastSummary();
      callback(result || { ok: true });
    }
  });

  ipcServer.start();
}

async function createApp() {
  sessionStore = new SessionStore();
  usageStore = new UsageStore();
  watcher = new Watcher(usageStore);
  syncService = new SyncService(usageStore);
  interventionManager = new InterventionManager();
  interventionManager.setOnChange((pending) => {
    if (!islandWindow || islandWindow.isDestroyed()) {
      return;
    }

    islandWindow.webContents.send('intervention-state', pending);
    if (pending) {
      setIslandMode('expanded');
      islandWindow.showInactive();
      islandWindow.webContents.send('island-force-expand');
    }
  });

  createIslandWindow();
  trayController = createTray({
    icon: nativeImage.createFromPath(TRAY_ICON_PATH),
    onOpenDashboard: async () => {
      if (islandWindow) {
        islandWindow.showInactive();
        setIslandMode('expanded');
        islandWindow.webContents.send('island-force-expand');
        broadcastSummary();
      }
    },
    onRefresh: syncNow,
    onQuit: () => {
      app.isQuiting = true;
      app.exit(0);
    }
  });

  setupIPC();
  setupBridgeServer();
  ConfigInjector.setAppPath(app.getAppPath());
  ConfigInjector.injectClaude();
  ConfigInjector.injectCodex();
  ConfigInjector.injectGemini();

  localAPI = new LocalAPI(usageStore, interventionManager);
  localAPI.start();

  registerGlobalShortcuts();

  await syncNow();

  const intervalMinutes = syncService.defaultsConfig.syncIntervalMinutes || 10;
  syncTimer = setInterval(syncNow, intervalMinutes * 60 * 1000);
}

const gotSingleInstanceLock = app.requestSingleInstanceLock();
if (!gotSingleInstanceLock) {
  app.quit();
} else {
  app.on('second-instance', () => {
    if (islandWindow && !islandWindow.isDestroyed()) {
      islandWindow.showInactive();
      setIslandMode('expanded');
      broadcastSummary();
    }
  });

  app.whenReady().then(createApp);
}

app.on('activate', () => {
  if (islandWindow) {
    islandWindow.showInactive();
    broadcastSummary();
  }
});

app.on('window-all-closed', () => {});

function registerGlobalShortcuts() {
  registerShortcut('CommandOrControl+Shift+Space', () => {
    if (islandWindow && !islandWindow.isDestroyed()) {
      if (islandWindow.isVisible()) {
        setIslandMode('pill');
      } else {
        islandWindow.showInactive();
        setIslandMode('expanded');
        broadcastSummary();
      }
    }
  });

  registerDecisionShortcut('CommandOrControl+Alt+A', 'approve');
  registerDecisionShortcut('CommandOrControl+Shift+A', 'approve');
  registerDecisionShortcut('CommandOrControl+Alt+L', 'approve_always');
  registerDecisionShortcut('CommandOrControl+Shift+L', 'approve_always');
  registerDecisionShortcut('CommandOrControl+Alt+D', 'deny');
  registerDecisionShortcut('CommandOrControl+Shift+D', 'deny');
}

function registerShortcut(accelerator, callback) {
  const registered = globalShortcut.register(accelerator, callback);
  if (!registered) {
    console.warn(`Shortcut registration failed: ${accelerator}`);
  }
}

function registerDecisionShortcut(accelerator, decision) {
  registerShortcut(accelerator, () => respondToIntervention(decision));
}

function respondToIntervention(decision) {
  if (!interventionManager || !interventionManager.getPending()) {
    return false;
  }

  interventionManager.respond(decision);
  if (!interventionManager.getPending()) {
    setIslandMode('pill');
  }
  broadcastSummary();
  return true;
}

app.on('before-quit', () => {
  app.isQuiting = true;
  globalShortcut.unregisterAll();
  if (syncTimer) {
    clearInterval(syncTimer);
  }
  if (localAPI) {
    localAPI.stop();
  }
});
