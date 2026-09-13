import type { ClipboardApi, Settings } from '@shared/types'
import { createTauriApi, isTauriRuntime } from './tauri-api'

declare global {
  interface Window {
    witchcat?: ClipboardApi
  }
}

export const DEFAULT_SETTINGS: Settings = {
  hotkey: 'Alt+V',
  quickPasteModifiers: 'Ctrl+Alt',
  maxItems: 2000,
  maxDays: 30,
  skipSensitive: true,
  sensitiveApps: [],
  hideAfterPaste: true,
  trayOpensMini: true,
  visibleFilters: ['all', 'text', 'image', 'files', 'url', 'key'],
  autoLaunch: false,
  theme: 'system',
  accent: 'violet',
  opacity: 90,
  skippedVersion: null,
}

/** 纯浏览器里打开时（没有 preload）用的空实现，避免整页崩掉 */
export const fallbackApi: ClipboardApi = {
  list: async () => ({ items: [], total: 0 }),
  stats: async () => ({ total: 0, pinned: 0, images: 0, bytes: 0 }),
  tags: async () => [],
  setTags: async () => {},
  togglePin: async () => {},
  remove: async () => {},
  clearAll: async () => {},
  copy: async () => {},
  paste: async () => ({ ok: false, reason: 'no-native' }),
  imageDataUrl: async () => null,
  relatedItems: async () => [],
  hidePanel: async () => {},
  expandPanel: async () => {},
  revealFile: async () => {},
  getSettings: async () => DEFAULT_SETTINGS,
  saveSettings: async (patch) => ({ ...DEFAULT_SETTINGS, ...patch }),
  security: async () => ({
    osProtected: false,
    dbEncrypted: false,
    nativeAvailable: false,
    memoryFallback: true,
    dataDir: '',
  }),
  startCrossDevice: async () => ({
    running: false,
    url: null,
    pairCode: null,
    connected: false,
    lastSeenAt: null,
    lastSentAt: null,
    lastSentPreview: null,
  }),
  stopCrossDevice: async () => ({
    running: false,
    url: null,
    pairCode: null,
    connected: false,
    lastSeenAt: null,
    lastSentAt: null,
    lastSentPreview: null,
  }),
  crossDeviceStatus: async () => ({
    running: false,
    url: null,
    pairCode: null,
    connected: false,
    lastSeenAt: null,
    lastSentAt: null,
    lastSentPreview: null,
  }),
  sendCrossDeviceItem: async () => ({ ok: false, reason: 'not-running' }),
  approveCrossDevice: async () => fallbackApi.crossDeviceStatus(),
  rejectCrossDevice: async () => fallbackApi.crossDeviceStatus(),
  cancelCrossDeviceTransfer: async () => fallbackApi.crossDeviceStatus(),
  retryCrossDeviceTransfer: async () => fallbackApi.crossDeviceStatus(),
  webDavConfig: async () => ({
    enabled: false,
    url: '',
    username: '',
    hasPassword: false,
    hasSyncKey: false,
    keyFingerprint: null,
  }),
  saveWebDavConfig: async (patch) => ({
    enabled: patch.enabled,
    url: patch.url,
    username: patch.username,
    hasPassword: Boolean(patch.password),
    hasSyncKey: Boolean(patch.syncKey),
    keyFingerprint: null,
  }),
  copyWebDavSyncKey: async () => { throw new Error('当前运行环境不支持 WebDAV 同步') },
  webDavStatus: async () => ({ state: 'idle', lastSyncAt: null, uploaded: 0, downloaded: 0, deleted: 0, error: null }),
  syncWebDavNow: async () => { throw new Error('当前运行环境不支持 WebDAV 同步') },
  openDataDir: async () => {},
  checkUpdate: async () => ({ state: 'unsupported', currentVersion: '0.0.0' }),
  downloadUpdate: async () => ({ state: 'unsupported', currentVersion: '0.0.0' }),
  installUpdate: async () => {},
  skipUpdate: async () => ({ state: 'idle', currentVersion: '0.0.0' }),
  updateStatus: async () => ({ state: 'idle', currentVersion: '0.0.0' }),
  onUpdateStatus: () => () => {},
  pasteItems: async () => ({ ok: false, reason: 'send-failed' as const }),
  pasteTransformed: async () => ({ ok: false, reason: 'send-failed' as const }),
  setItemNote: async () => {},
  setItemHotkey: async () => { throw new Error('当前运行环境不支持设置热键') },
  groups: async () => [],
  groupCreate: async () => { throw new Error('当前运行环境不支持分组') },
  groupRename: async () => { throw new Error('当前运行环境不支持分组') },
  groupDelete: async () => { throw new Error('当前运行环境不支持分组') },
  itemSetGroup: async () => {},
  exportItems: async () => { throw new Error('当前运行环境不支持导出') },
  importItems: async () => { throw new Error('当前运行环境不支持导入') },
  onChanged: () => () => {},
  onPanelShown: () => () => {},
  onPasteFailed: () => () => {},
}

export const api: ClipboardApi = isTauriRuntime
  ? createTauriApi()
  : (window.witchcat ?? fallbackApi)
export const isDesktop = isTauriRuntime || Boolean(window.witchcat)
