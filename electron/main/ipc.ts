import { ipcMain, shell, app, BrowserWindow, clipboard } from 'electron'
import type {
  CrossDeviceSendResult,
  ListQuery,
  PasteOutcome,
  SecurityInfo,
  Settings,
} from '@shared/types'
import type { ItemStore } from './store'
import { MemoryStore } from './store'
import { getSettings, saveSettings } from './settings'
import { hidePanel, showPanel } from './window'
import { getMini, hideMini, showMini } from './mini'
import { registerHotkey, registerQuickPaste } from './shortcuts'
import { pasteToPreviousWindow } from './paste'
import { hasNative } from './win32'
import { writeItemToClipboard } from './item-clipboard'
import { isOsProtected } from '../data/crypto'
import { sweep } from '../data/retention'
import {
  checkForUpdate,
  currentStatus,
  downloadUpdate,
  installUpdate,
  skipVersion,
} from './updater'
import type { WatcherHandle } from './watcher'
import type { CrossDeviceService } from './cross-device'

export interface IpcDeps {
  store: ItemStore
  watcher: WatcherHandle | null
  /** 数据库不可用、降级到内存时为 true */
  memoryFallback: boolean
  crossDevice: CrossDeviceService
}

function broadcast(channel: string): void {
  // 隐藏窗口重新显示时会主动刷新；后台不做无意义的列表/统计查询。
  for (const win of BrowserWindow.getAllWindows()) {
    if (win.isVisible()) win.webContents.send(channel)
  }
}

export function registerIpc(deps: IpcDeps): void {
  const { store } = deps

  ipcMain.handle('items:list', (_e, query: ListQuery) => store.list(query ?? {}))
  ipcMain.handle('items:stats', () => store.stats())
  ipcMain.handle('items:tags', () => store.tags())

  ipcMain.handle('items:setTags', (_e, id: number, tags: string[]) => {
    store.setTags(id, tags)
    broadcast('items:changed')
  })

  ipcMain.handle('items:togglePin', (_e, id: number) => {
    store.togglePin(id)
    broadcast('items:changed')
  })

  ipcMain.handle('items:remove', (_e, id: number) => {
    store.remove(id)
    broadcast('items:changed')
  })

  ipcMain.handle('items:clear', () => {
    store.clearAll()
    broadcast('items:changed')
  })

  ipcMain.handle('items:copy', (_e, id: number) => {
    writeItemToClipboard(deps, id)
    broadcast('items:changed')
  })

  ipcMain.handle('items:paste', async (event, id: number): Promise<PasteOutcome> => {
    if (!writeItemToClipboard(deps, id)) return { ok: false, reason: 'not-found' }

    const fromMini = BrowserWindow.fromWebContents(event.sender) === getMini()
    const hidden = getSettings().hideAfterPaste
    if (hidden) {
      if (fromMini) hideMini()
      else hidePanel()
    }

    const result = await pasteToPreviousWindow()
    // 自动粘贴失败时把窗口重新亮出来，否则用户看不到「请手动 Ctrl+V」的提示
    if (!result.ok && hidden) {
      if (fromMini) showMini()
      else showPanel()
    }

    broadcast('items:changed')
    return result
  })

  ipcMain.handle('items:image', (_e, id: number): string | null => {
    const png = store.imagePng(id)
    return png ? `data:image/png;base64,${png.toString('base64')}` : null
  })
  ipcMain.handle('items:related', (_e, id: number, limit?: number) =>
    store.related(id, Math.min(Math.max(limit ?? 5, 1), 5)),
  )

  ipcMain.handle('panel:hide', (event) => {
    // 迷你面板和完整面板共用这条通道，谁发的就收谁
    const win = BrowserWindow.fromWebContents(event.sender)
    if (win && win === getMini()) hideMini()
    else hidePanel()
  })

  ipcMain.handle('panel:expand', () => {
    hideMini()
    showPanel()
  })

  ipcMain.handle('items:reveal', (_e, id: number) => {
    const item = store.get(id)
    if (item?.kind !== 'files') return
    const first = (item.text ?? '').split('\n').filter(Boolean)[0]
    if (first) shell.showItemInFolder(first)
  })

  ipcMain.handle('settings:get', () => getSettings())
  ipcMain.handle('settings:save', (_e, patch: Partial<Settings>) => {
    const before = getSettings()
    const requested = { ...before, ...patch }

    if (requested.hotkey !== before.hotkey && !registerHotkey(requested.hotkey)) {
      return before
    }
    if (
      requested.quickPasteModifiers !== before.quickPasteModifiers &&
      !registerQuickPaste(requested.quickPasteModifiers)
    ) {
      // 同一次更新若还改了唤出热键，也恢复它，保证设置是原子的。
      if (requested.hotkey !== before.hotkey) registerHotkey(before.hotkey)
      return before
    }

    const next = saveSettings(patch)

    if (next.autoLaunch !== before.autoLaunch) {
      // --hidden：开机自启时只驻托盘，不弹面板
      app.setLoginItemSettings({ openAtLogin: next.autoLaunch, args: ['--hidden'] })
    }

    if (next.maxItems !== before.maxItems || next.maxDays !== before.maxDays) {
      sweep(store)
    }
    broadcast('items:changed')
    return next
  })

  ipcMain.handle(
    'app:security',
    (): SecurityInfo => ({
      osProtected: deps.memoryFallback ? false : isOsProtected(),
      dbEncrypted: !deps.memoryFallback,
      nativeAvailable: hasNative(),
      memoryFallback: deps.memoryFallback || store instanceof MemoryStore,
      dataDir: app.getPath('userData'),
    }),
  )

  ipcMain.handle('app:openDataDir', () => shell.openPath(app.getPath('userData')))

  ipcMain.handle('cross-device:start', async () => {
    await deps.crossDevice.start()
    // 手机刚连接时不该看到空白：把电脑当前剪贴板作为第一条立即发布。
    const currentText = clipboard.readText()
    if (currentText.trim()) {
      deps.crossDevice.publishText(currentText)
    } else if (clipboard.availableFormats().some((format) => format.startsWith('image/'))) {
      const image = clipboard.readImage()
      if (!image.isEmpty()) deps.crossDevice.publishImage(image.toPNG(), '当前剪贴板图片')
    }
    return deps.crossDevice.status()
  })
  ipcMain.handle('cross-device:stop', () => deps.crossDevice.stop())
  ipcMain.handle('cross-device:status', () => deps.crossDevice.status())
  ipcMain.handle(
    'cross-device:send-item',
    (_e, id: number): CrossDeviceSendResult => {
      const item = store.get(id)
      if (!item) return { ok: false, reason: 'not-found' }
      if (item.kind === 'text' && item.text) return deps.crossDevice.publishText(item.text)
      if (item.kind === 'image') {
        const png = store.imagePng(id)
        return png
          ? deps.crossDevice.publishImage(png, item.preview)
          : { ok: false, reason: 'not-found' }
      }
      return { ok: false, reason: 'unsupported' }
    },
  )

  ipcMain.handle('update:check', () => checkForUpdate(false))
  ipcMain.handle('update:download', () => downloadUpdate())
  ipcMain.handle('update:install', () => installUpdate())
  ipcMain.handle('update:skip', (_e, version?: string) => skipVersion(version))
  ipcMain.handle('update:status', () => currentStatus())
}
