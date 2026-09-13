import { contextBridge, ipcRenderer } from 'electron'
import type { ClipboardApi, ListQuery, Settings, UpdateStatus } from '@shared/types'

/** 渲染进程只能看到这份白名单，没有 node / 没有裸 ipcRenderer */
const api: ClipboardApi = {
  list: (query: ListQuery) => ipcRenderer.invoke('items:list', query),
  stats: () => ipcRenderer.invoke('items:stats'),
  tags: () => ipcRenderer.invoke('items:tags'),
  setTags: (id, tags) => ipcRenderer.invoke('items:setTags', id, tags),
  togglePin: (id) => ipcRenderer.invoke('items:togglePin', id),
  remove: (id) => ipcRenderer.invoke('items:remove', id),
  clearAll: () => ipcRenderer.invoke('items:clear'),
  copy: (id) => ipcRenderer.invoke('items:copy', id),
  paste: (id) => ipcRenderer.invoke('items:paste', id),
  // 旧 Electron 回滚版不实现新能力，调用即明确报错（与 WebDAV 桩同一策略）
  pasteItems: async () => { throw new Error('旧 Electron 回滚版不支持多选粘贴') },
  pasteTransformed: async () => { throw new Error('旧 Electron 回滚版不支持粘贴变换') },
  setItemNote: async () => { throw new Error('旧 Electron 回滚版不支持备注') },
  setItemHotkey: async () => { throw new Error('旧 Electron 回滚版不支持条目热键') },
  groups: async () => [],
  groupCreate: async () => { throw new Error('旧 Electron 回滚版不支持分组') },
  groupRename: async () => { throw new Error('旧 Electron 回滚版不支持分组') },
  groupDelete: async () => { throw new Error('旧 Electron 回滚版不支持分组') },
  itemSetGroup: async () => { throw new Error('旧 Electron 回滚版不支持分组') },
  exportItems: async () => { throw new Error('旧 Electron 回滚版不支持导出') },
  importItems: async () => { throw new Error('旧 Electron 回滚版不支持导入') },
  imageDataUrl: (id) => ipcRenderer.invoke('items:image', id),
  relatedItems: (id, limit) => ipcRenderer.invoke('items:related', id, limit),
  hidePanel: () => ipcRenderer.invoke('panel:hide'),
  expandPanel: () => ipcRenderer.invoke('panel:expand'),
  revealFile: (id) => ipcRenderer.invoke('items:reveal', id),
  getSettings: () => ipcRenderer.invoke('settings:get'),
  saveSettings: (patch: Partial<Settings>) => ipcRenderer.invoke('settings:save', patch),
  security: () => ipcRenderer.invoke('app:security'),

  startCrossDevice: () => ipcRenderer.invoke('cross-device:start'),
  stopCrossDevice: () => ipcRenderer.invoke('cross-device:stop'),
  crossDeviceStatus: () => ipcRenderer.invoke('cross-device:status'),
  sendCrossDeviceItem: (id) => ipcRenderer.invoke('cross-device:send-item', id),
  approveCrossDevice: async () => ipcRenderer.invoke('cross-device:status'),
  rejectCrossDevice: async () => ipcRenderer.invoke('cross-device:status'),
  cancelCrossDeviceTransfer: async () => ipcRenderer.invoke('cross-device:status'),
  retryCrossDeviceTransfer: async () => ipcRenderer.invoke('cross-device:status'),
  webDavConfig: async () => ({ enabled: false, url: '', username: '', hasPassword: false, hasSyncKey: false, keyFingerprint: null }),
  saveWebDavConfig: async (patch) => ({ enabled: patch.enabled, url: patch.url, username: patch.username, hasPassword: Boolean(patch.password), hasSyncKey: Boolean(patch.syncKey), keyFingerprint: null }),
  copyWebDavSyncKey: async () => { throw new Error('旧 Electron 回滚版不支持 WebDAV 同步') },
  webDavStatus: async () => ({ state: 'idle' as const, lastSyncAt: null, uploaded: 0, downloaded: 0, deleted: 0, error: null }),
  syncWebDavNow: async () => { throw new Error('旧 Electron 回滚版不支持 WebDAV 同步') },

  checkUpdate: () => ipcRenderer.invoke('update:check'),
  downloadUpdate: () => ipcRenderer.invoke('update:download'),
  installUpdate: () => ipcRenderer.invoke('update:install'),
  skipUpdate: (version) => ipcRenderer.invoke('update:skip', version),
  updateStatus: () => ipcRenderer.invoke('update:status'),
  onUpdateStatus: (cb) => {
    const handler = (_e: unknown, status: UpdateStatus): void => cb(status)
    ipcRenderer.on('update:status', handler)
    return () => ipcRenderer.off('update:status', handler)
  },
  openDataDir: () => ipcRenderer.invoke('app:openDataDir'),

  onChanged: (cb) => {
    const handler = (): void => cb()
    ipcRenderer.on('items:changed', handler)
    return () => ipcRenderer.off('items:changed', handler)
  },
  onPanelShown: (cb) => {
    const handler = (): void => cb()
    ipcRenderer.on('panel:shown', handler)
    return () => ipcRenderer.off('panel:shown', handler)
  },
  onPasteFailed: () => () => {},
}

contextBridge.exposeInMainWorld('witchcat', api)
