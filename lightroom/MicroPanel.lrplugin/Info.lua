return {
  LrSdkVersion = 10.0,
  LrSdkMinimumVersion = 10.0,
  LrToolkitIdentifier = 'org.micropanel.lightroom',
  LrPluginName = 'Micro Color Panel',
  LrInitPlugin = 'MicroPanelInit.lua',
  LrForceInitPlugin = true,
  LrShutdownPlugin = 'MicroPanelShutdown.lua',
  LrExportMenuItems = {
    { title = 'Micro Color Panel status', file = 'MicroPanelStatus.lua' },
  },
  VERSION = { major = 0, minor = 3, revision = 5 },
}