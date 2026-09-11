import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { getVersion } from '@tauri-apps/api/app'
import { relaunch } from '@tauri-apps/plugin-process'
import { check, type DownloadEvent, type Update } from '@tauri-apps/plugin-updater'
import type {
  ClipboardApi,
  ListQuery,
  ListResult,
  PasteOutcome,
  Stats,
  UpdateStatus,
} from '@shared/types'

export const isTauriRuntime = '__TAURI_INTERNALS__' in window

/** Tauri 的 WebView 会拦截 target="_blank" 外链，必须经 Rust 侧 ShellExecuteW 打开 */
export async function openExternal(url: string): Promise<void> {
  await invoke('open_external', { url })
}

function subscribe(event: string, cb: () => void): () => void {
  let disposed = false
  let unlisten: UnlistenFn | null = null

  void listen(event, cb).then((stop) => {
    if (disposed) stop()
    else unlisten = stop
  })

  return () => {
    disposed = true
    unlisten?.()
  }
}

let pendingUpdate: Update | null = null
let updateStatus: UpdateStatus = { state: 'idle', currentVersion: '0.0.0' }
const updateSubscribers = new Set<(status: UpdateStatus) => void>()
let startupCheckScheduled = false

async function currentVersion(): Promise<string> {
  if (updateStatus.currentVersion === '0.0.0') {
    updateStatus = { ...updateStatus, currentVersion: await getVersion() }
  }
  return updateStatus.currentVersion
}

function publishUpdate(patch: Partial<UpdateStatus>): UpdateStatus {
  updateStatus = { ...updateStatus, ...patch }
  for (const subscriber of updateSubscribers) subscriber(updateStatus)
  return updateStatus
}

async function checkUpdate(): Promise<UpdateStatus> {
  const version = await currentVersion()
  publishUpdate({ state: 'checking', error: undefined, percent: undefined })
  try {
    await pendingUpdate?.close()
    pendingUpdate = await check({ timeout: 15_000 })
    if (!pendingUpdate) {
      return publishUpdate({ state: 'none', currentVersion: version, version: undefined, notes: undefined })
    }
    return publishUpdate({
      state: 'available',
      currentVersion: version,
      version: pendingUpdate.version,
      notes: pendingUpdate.body?.replace(/<[^>]+>/g, '').trim().slice(0, 600),
    })
  } catch (error) {
    return publishUpdate({ state: 'error', error: error instanceof Error ? error.message : String(error) })
  }
}

async function downloadUpdate(): Promise<UpdateStatus> {
  if (!pendingUpdate) return checkUpdate()
  let received = 0
  let total = 0
  publishUpdate({ state: 'downloading', percent: 0, error: undefined })
  try {
    await pendingUpdate.download((event: DownloadEvent) => {
      if (event.event === 'Started') total = event.data.contentLength ?? 0
      if (event.event === 'Progress') received += event.data.chunkLength
      if (event.event === 'Progress' && total > 0) {
        publishUpdate({ state: 'downloading', percent: Math.min(99, Math.round((received / total) * 100)) })
      }
      if (event.event === 'Finished') publishUpdate({ state: 'ready', percent: 100 })
    })
    return publishUpdate({ state: 'ready', percent: 100 })
  } catch (error) {
    return publishUpdate({ state: 'error', error: error instanceof Error ? error.message : String(error) })
  }
}

/** Native Tauri adapter. Every desktop capability is implemented here; no Electron fallback is used. */
export function createTauriApi(): ClipboardApi {
  if (!startupCheckScheduled && !new URLSearchParams(window.location.search).has('mode')) {
    startupCheckScheduled = true
    window.setTimeout(() => {
      void checkUpdate().then(async (result) => {
        if (result.state === 'available') {
          const settings = await invoke<{ skippedVersion: string | null }>('get_settings')
          if (settings.skippedVersion === result.version) publishUpdate({ state: 'idle' })
        } else if (result.state === 'error') {
          publishUpdate({ state: 'idle', error: undefined })
        }
      })
    }, 12_000)
  }
  return {
    list: (query: ListQuery): Promise<ListResult> => invoke('clipboard_list', { query }),
    stats: (): Promise<Stats> => invoke('clipboard_stats'),
    tags: (): Promise<string[]> => invoke('clipboard_tags'),
    setTags: (id, tags) => invoke('clipboard_set_tags', { id, tags }),
    togglePin: (id) => invoke('toggle_pin', { id }),
    remove: (id) => invoke('remove_item', { id }),
    clearAll: () => invoke('clear_all'),
    copy: (id) => invoke('copy_item', { id }),
    paste: (id): Promise<PasteOutcome> => invoke('paste_item', { id }),
    imageDataUrl: (id) => invoke('clipboard_image', { id }),
    relatedItems: (id, limit) => invoke('clipboard_related', { id, limit }),
    hidePanel: () => invoke('hide_panel'),
    expandPanel: () => invoke('expand_panel'),
    revealFile: (id) => invoke('reveal_file', { id }),
    getSettings: () => invoke('get_settings'),
    saveSettings: (patch) => invoke('save_settings', { patch }),
    security: () => invoke('security_info'),
    openDataDir: () => invoke('open_data_dir'),
    startCrossDevice: () => invoke('cross_device_start'),
    stopCrossDevice: () => invoke('cross_device_stop'),
    crossDeviceStatus: () => invoke('cross_device_status'),
    sendCrossDeviceItem: (id) => invoke('cross_device_send', { id }),
    approveCrossDevice: (deviceId) => invoke('cross_device_approve', { deviceId }),
    rejectCrossDevice: (deviceId) => invoke('cross_device_reject', { deviceId }),
    cancelCrossDeviceTransfer: (transferId) => invoke('cross_device_cancel_transfer', { transferId }),
    retryCrossDeviceTransfer: (transferId) => invoke('cross_device_retry_transfer', { transferId }),
    webDavConfig: () => invoke('webdav_config'),
    saveWebDavConfig: (patch) => invoke('webdav_save_config', { patch }),
    copyWebDavSyncKey: () => invoke('webdav_copy_sync_key'),
    webDavStatus: () => invoke('webdav_status'),
    syncWebDavNow: () => invoke('webdav_sync_now'),
    checkUpdate,
    downloadUpdate,
    installUpdate: async () => {
      if (!pendingUpdate || updateStatus.state !== 'ready') return
      await pendingUpdate.install()
      await relaunch()
    },
    skipUpdate: async (version) => {
      const target = version ?? updateStatus.version
      if (target) await invoke('save_settings', { patch: { skippedVersion: target } })
      await pendingUpdate?.close()
      pendingUpdate = null
      return publishUpdate({ state: 'idle' })
    },
    updateStatus: async () => {
      await currentVersion()
      return updateStatus
    },
    onUpdateStatus: (cb) => {
      updateSubscribers.add(cb)
      return () => updateSubscribers.delete(cb)
    },
    onChanged: (cb) => subscribe('witchcat://changed', cb),
    onPanelShown: (cb) => subscribe('witchcat://panel-shown', cb),
  }
}
