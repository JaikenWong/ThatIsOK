const { Menu, Tray } = require('electron');

module.exports = function createTray({ icon, onOpenDashboard, onRefresh, onQuit }) {
  const tray = new Tray(icon);
  tray.setToolTip('ThatIsOk');

  function updateSummary() {
    const contextMenu = Menu.buildFromTemplate([
      {
        label: 'Open',
        click: onOpenDashboard
      },
      {
        label: 'Refresh',
        click: onRefresh
      },
      { type: 'separator' },
      {
        label: 'Quit',
        click: onQuit
      }
    ]);

    tray.setContextMenu(contextMenu);
  }

  tray.on('click', onOpenDashboard);

  updateSummary();

  return {
    tray,
    updateSummary
  };
};
