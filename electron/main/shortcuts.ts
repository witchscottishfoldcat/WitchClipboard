import { globalShortcut } from 'electron'
import { togglePanel } from './window'
import { toggleMini } from './mini'
import { getSettings } from './settings'

let current: string | null = null
let currentQuickPasteModifiers: string | null = null
let currentQuickPasteAccelerators: string[] = []
let quickPasteHandler: ((index: number) => void) | null = null

/**
 * 注册面板热键。先尝试新组合，成功后才释放旧组合；
 * 新组合被占用时，原来可用的热键不会丢。
 */
export function registerHotkey(accelerator = getSettings().hotkey): boolean {
  if (current === accelerator && globalShortcut.isRegistered(accelerator)) return true

  try {
    const ok = globalShortcut.register(accelerator, () => {
      if (getSettings().hotkeyOpensMini) toggleMini()
      else togglePanel()
    })
    if (!ok) {
      console.error(`[hotkey] 注册失败，可能已被占用: ${accelerator}`)
      return false
    }

    const previous = current
    current = accelerator
    if (previous && previous !== accelerator) globalShortcut.unregister(previous)
    return true
  } catch (err) {
    console.error('[hotkey] 注册异常', err)
    return false
  }
}

/** 主进程启动时注入实际的快粘动作。 */
export function setQuickPasteHandler(handler: (index: number) => void): void {
  quickPasteHandler = handler
}

export function quickPasteAccelerators(modifiers: string): string[] {
  return Array.from({ length: 9 }, (_, index) => `${modifiers}+${index + 1}`)
}

/**
 * 原子地注册 1…9 九个全局快粘键。
 * 新组合有任意一个被占用时撤销本次注册，保留原组合。
 */
export function registerQuickPaste(modifiers = getSettings().quickPasteModifiers): boolean {
  if (!quickPasteHandler || modifiers.trim().length === 0) return false

  const next = quickPasteAccelerators(modifiers)
  if (
    currentQuickPasteModifiers === modifiers &&
    next.every((accelerator) => globalShortcut.isRegistered(accelerator))
  ) {
    return true
  }

  const previousSet = new Set(currentQuickPasteAccelerators)
  const newlyRegistered: string[] = []

  try {
    for (const [index, accelerator] of next.entries()) {
      if (previousSet.has(accelerator)) continue
      const ok = globalShortcut.register(accelerator, () => quickPasteHandler?.(index))
      if (!ok) {
        for (const registered of newlyRegistered) globalShortcut.unregister(registered)
        console.error(`[quick-paste] 注册失败，可能已被占用: ${accelerator}`)
        return false
      }
      newlyRegistered.push(accelerator)
    }

    const nextSet = new Set(next)
    for (const accelerator of currentQuickPasteAccelerators) {
      if (!nextSet.has(accelerator)) globalShortcut.unregister(accelerator)
    }
    currentQuickPasteModifiers = modifiers
    currentQuickPasteAccelerators = next
    return true
  } catch (err) {
    for (const registered of newlyRegistered) globalShortcut.unregister(registered)
    console.error('[quick-paste] 注册异常', err)
    return false
  }
}

export function unregisterHotkey(): void {
  if (current) {
    globalShortcut.unregister(current)
    current = null
  }
}

export function unregisterQuickPaste(): void {
  for (const accelerator of currentQuickPasteAccelerators) {
    globalShortcut.unregister(accelerator)
  }
  currentQuickPasteModifiers = null
  currentQuickPasteAccelerators = []
}

export function unregisterAllShortcuts(): void {
  unregisterHotkey()
  unregisterQuickPaste()
}

export function currentHotkey(): string | null {
  return current
}
