/**
 * 「点到别处就收起」的实现。
 *
 * 只监听 blur 事件是不够的：从托盘唤出时，Windows 的前台窗口抢占限制可能让窗口
 * 显示出来却拿不到焦点，这种情况下永远不会触发 blur，面板就怎么点都不消失。
 * 所以再加一个看门狗，直接问系统「现在前台窗口是谁家的」。
 */
import type { BrowserWindow } from 'electron'
import { screen } from 'electron'
import { foregroundPid, forceForeground, leftMouseButtonDown } from './win32'
import { getSettings } from './settings'

const POLL_MS = 200
/** 刚显示的这段时间不判定，等焦点交接完成 */
const GRACE_MS = 500

/** 设 WCC_NO_AUTOHIDE=1 可以关掉自动收起，开发时截图方便 */
const disabled = process.env['WCC_NO_AUTOHIDE'] === '1'

export function autoHideDisabled(): boolean {
  return disabled
}

/** 显示窗口后调用：尽量把它抢到前台，blur 才会可靠 */
export function claimForeground(win: BrowserWindow): void {
  if (getSettings().preserveFocusOnShow) {
    win.showInactive()
    return
  }
  win.show()
  win.focus()
  if (disabled) return
  try {
    forceForeground(win.getNativeWindowHandle())
  } catch {
    /* 拿不到原生能力就算了，win.focus() 已经尽力 */
  }
}

/** 看门狗只需要窗口的这两个状态，抽出来便于自检里注入假对象 */
export interface DismissTarget {
  isDestroyed(): boolean
  isVisible(): boolean
  getBounds?(): { x: number; y: number; width: number; height: number }
}

export interface WatchOptions {
  pollMs?: number
  graceMs?: number
  /** 取当前前台窗口所属进程；返回 null 表示拿不到 */
  getPid?: () => number | null
  selfPid?: number
  /** 忽略 WCC_NO_AUTOHIDE，自检要测真实逻辑 */
  force?: boolean
}

/**
 * 盯着前台窗口，用户切到别的进程（点桌面、点别的程序）就收起。
 * 返回停止函数。
 */
export function watchOutsideClick(
  win: DismissTarget,
  hide: () => void,
  options: WatchOptions = {},
): () => void {
  if (disabled && !options.force) return () => {}

  const preserveFocus = getSettings().preserveFocusOnShow
  const pollMs = options.pollMs ?? (preserveFocus ? 25 : POLL_MS)
  const graceMs = options.graceMs ?? GRACE_MS
  const getPid = options.getPid ?? foregroundPid
  const selfPid = options.selfPid ?? process.pid

  const startedAt = Date.now()
  /**
   * 必须先确认我们真的拿到过前台才允许收起。
   * 否则「一直没抢到焦点」会被误判成「用户点了别处」，面板会自己莫名消失。
   */
  let sawSelfForeground = false
  let wasMouseDown = leftMouseButtonDown()

  const timer = setInterval(() => {
    if (win.isDestroyed() || !win.isVisible()) {
      clearInterval(timer)
      return
    }

    const mouseDown = leftMouseButtonDown()
    const clicked = mouseDown && !wasMouseDown
    wasMouseDown = mouseDown
    if (preserveFocus && clicked && win.getBounds) {
      const bounds = win.getBounds()
      const cursor = screen.getCursorScreenPoint()
      if (cursor.x < bounds.x || cursor.x >= bounds.x + bounds.width
        || cursor.y < bounds.y || cursor.y >= bounds.y + bounds.height) {
        clearInterval(timer)
        hide()
        return
      }
    }

    const pid = getPid()
    if (pid === null) return // 原生不可用，退回只靠 blur

    if (pid === selfPid) {
      sawSelfForeground = true
      return
    }

    if (Date.now() - startedAt < graceMs) return
    if (!sawSelfForeground) return

    clearInterval(timer)
    hide()
  }, pollMs)

  return () => clearInterval(timer)
}
