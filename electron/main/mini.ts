/**
 * 迷你预览面板：单击托盘时贴着图标弹出的小窗。
 * 和完整面板共用同一份渲染层代码，靠 ?mode=mini 切布局。
 */
import { join } from 'node:path'
import { BrowserWindow, app, screen, shell } from 'electron'
import { rememberForegroundWindow } from './paste'
import { autoHideDisabled, claimForeground, watchOutsideClick } from './dismiss'
import { isQuitting, type AnchorRect } from './window'

const MINI_W = 280
const MINI_H = 390
const IDLE_RELEASE_MS = 60_000
const isDev = !app.isPackaged

let mini: BrowserWindow | null = null
let hiddenAt = 0
let releaseTimer: NodeJS.Timeout | null = null
let destroyingMini: BrowserWindow | null = null
let beforeShow: (() => void) | null = null

export function getMini(): BrowserWindow | null {
  return mini
}

export function createMini(): BrowserWindow {
  if (mini && !mini.isDestroyed()) return mini

  const win = new BrowserWindow({
    width: MINI_W,
    height: MINI_H,
    show: false,
    frame: true,
    titleBarStyle: 'hidden',
    hasShadow: false,
    backgroundColor: '#00000000',
    backgroundMaterial: 'acrylic',
    skipTaskbar: true,
    alwaysOnTop: true,
    resizable: false,
    minimizable: false,
    maximizable: false,
    fullscreenable: false,
    autoHideMenuBar: true,
    icon: join(app.getAppPath(), 'resources', 'icon.png'),
    webPreferences: {
      preload: join(__dirname, '../preload/index.js'),
      contextIsolation: true,
      nodeIntegration: false,
      spellcheck: false,
    },
  })

  mini = win
  win.setAlwaysOnTop(true, 'pop-up-menu')
  win.setMenuBarVisibility(false)

  // 和完整面板一样：平时点 × 只隐藏，但退出时必须放行。
  // 少了这个判断，app.quit() 会被这个窗口一直否决，应用永远退不掉。
  win.on('close', (e) => {
    if (isQuitting() || destroyingMini === win) return
    e.preventDefault()
    hideMini()
  })

  win.on('closed', () => {
    if (mini === win) {
      mini = null
      cancelRelease()
      stopWatch?.()
      stopWatch = null
    }
    if (destroyingMini === win) destroyingMini = null
  })

  win.on('blur', () => {
    if (autoHideDisabled()) return
    if (win.webContents.isDevToolsFocused()) return
    hideMini()
  })

  win.on('show', () => notifyShown(win))

  if (isDev) {
    // 开发时把渲染进程的日志转到终端，省得为了看一行 log 去开 DevTools
    win.webContents.on('console-message', (event) => {
      console.log('[mini renderer]', event.message)
    })
  }

  win.webContents.setWindowOpenHandler(({ url }) => {
    void shell.openExternal(url)
    return { action: 'deny' }
  })

  const devUrl = process.env['ELECTRON_RENDERER_URL']
  if (devUrl) {
    void win.loadURL(`${devUrl}?mode=mini`)
  } else {
    void win.loadFile(join(__dirname, '../renderer/index.html'), { search: 'mode=mini' })
  }

  return win
}

/** 由启动模块注入，避免 mini.ts 与 window.ts 增加新的循环依赖。 */
export function setMiniBeforeShow(callback: () => void): void {
  beforeShow = callback
}

/** 立即释放迷你面板及其 renderer；切换到完整面板时调用。 */
export function releaseMini(): void {
  cancelRelease()
  stopWatch?.()
  stopWatch = null
  const win = mini
  if (!win || win.isDestroyed()) {
    mini = null
    return
  }
  destroyingMini = win
  mini = null
  win.destroy()
}

/** 贴着托盘图标弹出，右缘对齐图标右缘、压在任务栏上方 */
function position(win: BrowserWindow, anchor?: AnchorRect): void {
  const [w, h] = win.getSize()
  const near = anchor ? { x: anchor.x, y: anchor.y } : screen.getCursorScreenPoint()
  const { workArea } = screen.getDisplayNearestPoint(near)

  const rawX = anchor ? anchor.x + anchor.width - w : near.x - w / 2
  const rawY = anchor ? anchor.y - h - 8 : near.y - 40

  win.setPosition(
    Math.min(Math.max(Math.round(rawX), workArea.x + 8), workArea.x + workArea.width - w - 8),
    Math.min(Math.max(Math.round(rawY), workArea.y + 8), workArea.y + workArea.height - h - 8),
    false,
  )
}

let stopWatch: (() => void) | null = null

export function showMini(anchor?: AnchorRect): void {
  beforeShow?.()
  cancelRelease()
  const win = mini ?? createMini()
  // 抢焦点之前记下原来的前台窗口，粘贴时要还给它
  rememberForegroundWindow()
  position(win, anchor)
  claimForeground(win)

  stopWatch?.()
  stopWatch = watchOutsideClick(win, hideMini)
}

export function hideMini(): void {
  stopWatch?.()
  stopWatch = null
  if (mini?.isVisible()) hiddenAt = Date.now()
  mini?.hide()
  scheduleRelease()
}

function cancelRelease(): void {
  if (releaseTimer) clearTimeout(releaseTimer)
  releaseTimer = null
}

function scheduleRelease(): void {
  cancelRelease()
  if (!mini || mini.isDestroyed()) return
  releaseTimer = setTimeout(() => {
    releaseTimer = null
    if (mini && !mini.isVisible()) releaseMini()
  }, IDLE_RELEASE_MS)
  releaseTimer.unref()
}

function notifyShown(win: BrowserWindow): void {
  const notify = (): void => {
    if (win.isDestroyed() || !win.isVisible()) return
    win.webContents.send('panel:shown')
    win.webContents.send('items:changed')
  }
  if (win.webContents.isLoadingMainFrame()) win.webContents.once('did-finish-load', notify)
  else notify()
}

export function miniHiddenRecently(within = 400): boolean {
  return Date.now() - hiddenAt < within
}

/**
 * 托盘单击语义，和完整面板一致：
 * 点托盘时窗口会先因失焦收起，紧接着才收到 click，少了冷却判断就永远收不起来。
 */
export function toggleMiniFromTray(anchor?: AnchorRect): void {
  if (miniHiddenRecently()) return
  if (mini?.isVisible()) hideMini()
  else showMini(anchor)
}
