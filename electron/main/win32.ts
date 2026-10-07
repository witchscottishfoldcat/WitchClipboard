/**
 * Win32 原生调用（koffi，无需编译）。
 * 全部包在 try 里：拿不到原生能力时降级，不让应用崩。
 */
import koffi from 'koffi'
import { basename } from 'node:path'

const VK = {
  SHIFT: 0x10,
  CONTROL: 0x11,
  MENU: 0x12, // Alt
  LWIN: 0x5b,
  RWIN: 0x5c,
  V: 0x56,
} as const

const KEYEVENTF_KEYUP = 0x0002
const PROCESS_QUERY_LIMITED_INFORMATION = 0x1000

/** 资源管理器复制文件时放进剪贴板的格式 */
const CF_HDROP = 15
const GMEM_MOVEABLE = 0x0002
/** DragQueryFileW 传这个 index 表示「告诉我有几个文件」 */
const DRAG_QUERY_COUNT = 0xffffffff

/** 密码管理器用来标记「别记录我」的剪贴板格式 */
const SENSITIVE_FORMATS = [
  'ExcludeClipboardContentFromMonitorProcessing',
  'CanIncludeInClipboardHistory',
  'Clipboard Viewer Ignore',
]

interface Native {
  GetClipboardSequenceNumber: () => number
  GetForegroundWindow: () => unknown
  SetForegroundWindow: (hwnd: unknown) => boolean
  /** 同一个导出，参数按整数句柄传，方便直接用 Electron 给的 HWND 数值 */
  SetForegroundWindowByHandle: (hwnd: number) => boolean
  GetForegroundWindowPid: (hwnd: number, pid: number[]) => number
  GetWindowThreadProcessId: (hwnd: unknown, pid: number[]) => number
  RegisterClipboardFormatW: (name: string) => number
  IsClipboardFormatAvailable: (format: number) => boolean
  keybd_event: (vk: number, scan: number, flags: number, extra: number) => void
  GetAsyncKeyState: (vk: number) => number
  OpenProcess: (access: number, inherit: boolean, pid: number) => unknown
  QueryFullProcessImageNameW: (
    handle: unknown,
    flags: number,
    buf: Uint16Array,
    size: number[],
  ) => boolean
  CloseHandle: (handle: unknown) => boolean
  // 剪贴板文件列表（CF_HDROP）
  OpenClipboard: (hwnd: unknown) => boolean
  CloseClipboard: () => boolean
  EmptyClipboard: () => boolean
  GetClipboardData: (format: number) => unknown
  SetClipboardData: (format: number, handle: unknown) => unknown
  DragQueryFileCount: (hDrop: unknown, index: number, buf: null, cch: number) => number
  DragQueryFilePath: (hDrop: unknown, index: number, buf: Uint16Array, cch: number) => number
  GlobalAlloc: (flags: number, bytes: number) => unknown
  GlobalLock: (handle: unknown) => unknown
  GlobalUnlock: (handle: unknown) => boolean
  GlobalFree: (handle: unknown) => unknown
  CopyMemory: (dst: unknown, src: Buffer, len: number) => void
}

const native: Native | null = load()

function load(): Native | null {
  if (process.platform !== 'win32') return null
  try {
    const user32 = koffi.load('user32.dll')
    const kernel32 = koffi.load('kernel32.dll')
    const shell32 = koffi.load('shell32.dll')
    return {
      GetClipboardSequenceNumber: user32.func('uint32 __stdcall GetClipboardSequenceNumber()'),
      GetForegroundWindow: user32.func('void* __stdcall GetForegroundWindow()'),
      SetForegroundWindow: user32.func('bool __stdcall SetForegroundWindow(void* hWnd)'),
      SetForegroundWindowByHandle: user32.func('bool __stdcall SetForegroundWindow(uintptr hWnd)'),
      GetForegroundWindowPid: user32.func(
        'uint32 __stdcall GetWindowThreadProcessId(uintptr hWnd, _Out_ uint32* lpdwProcessId)',
      ),
      GetWindowThreadProcessId: user32.func(
        'uint32 __stdcall GetWindowThreadProcessId(void* hWnd, _Out_ uint32* lpdwProcessId)',
      ),
      RegisterClipboardFormatW: user32.func(
        'uint32 __stdcall RegisterClipboardFormatW(str16 lpszFormat)',
      ),
      IsClipboardFormatAvailable: user32.func(
        'bool __stdcall IsClipboardFormatAvailable(uint32 format)',
      ),
      keybd_event: user32.func(
        'void __stdcall keybd_event(uint8 bVk, uint8 bScan, uint32 dwFlags, uintptr dwExtraInfo)',
      ),
      GetAsyncKeyState: user32.func('int16 __stdcall GetAsyncKeyState(int vKey)'),
      OpenProcess: kernel32.func(
        'void* __stdcall OpenProcess(uint32 dwDesiredAccess, bool bInheritHandle, uint32 dwProcessId)',
      ),
      QueryFullProcessImageNameW: kernel32.func(
        'bool __stdcall QueryFullProcessImageNameW(void* hProcess, uint32 dwFlags, _Out_ uint16* lpExeName, _Inout_ uint32* lpdwSize)',
      ),
      CloseHandle: kernel32.func('bool __stdcall CloseHandle(void* handle)'),

      OpenClipboard: user32.func('bool __stdcall OpenClipboard(void* hWndNewOwner)'),
      CloseClipboard: user32.func('bool __stdcall CloseClipboard()'),
      EmptyClipboard: user32.func('bool __stdcall EmptyClipboard()'),
      GetClipboardData: user32.func('void* __stdcall GetClipboardData(uint32 uFormat)'),
      SetClipboardData: user32.func('void* __stdcall SetClipboardData(uint32 uFormat, void* hMem)'),
      // 同一个导出绑两次：查数量必须传 NULL，读路径要传缓冲区
      DragQueryFileCount: shell32.func(
        'uint32 __stdcall DragQueryFileW(void* hDrop, uint32 iFile, void* lpszFile, uint32 cch)',
      ),
      DragQueryFilePath: shell32.func(
        'uint32 __stdcall DragQueryFileW(void* hDrop, uint32 iFile, _Out_ uint16* lpszFile, uint32 cch)',
      ),
      GlobalAlloc: kernel32.func('void* __stdcall GlobalAlloc(uint32 uFlags, size_t dwBytes)'),
      GlobalLock: kernel32.func('void* __stdcall GlobalLock(void* hMem)'),
      GlobalUnlock: kernel32.func('bool __stdcall GlobalUnlock(void* hMem)'),
      GlobalFree: kernel32.func('void* __stdcall GlobalFree(void* hMem)'),
      CopyMemory: kernel32.func('void __stdcall RtlMoveMemory(void* dst, void* src, size_t len)'),
    }
  } catch (err) {
    console.error('[win32] 原生能力加载失败，将降级运行：', (err as Error).message)
    return null
  }
}

export const hasNative = (): boolean => native !== null

/**
 * 剪贴板序列号：每次剪贴板内容变化就自增。
 * 一次极便宜的系统调用，省掉了轮询里反复解码剪贴板内容的开销。
 */
export function clipboardSequence(): number | null {
  try {
    return native ? native.GetClipboardSequenceNumber() : null
  } catch {
    return null
  }
}

/** 剪贴板里是否带「不要记录」标记（密码管理器会写） */
export function hasSensitiveMarker(): boolean {
  if (!native) return false
  try {
    for (const name of SENSITIVE_FORMATS) {
      const id = native.RegisterClipboardFormatW(name)
      if (id !== 0 && native.IsClipboardFormatAvailable(id)) return true
    }
  } catch {
    /* 拿不到就当没有标记 */
  }
  return false
}

export interface ForegroundInfo {
  hwnd: unknown
  /** 进程可执行文件名，如 chrome.exe */
  exe: string | null
}

export function foregroundWindow(): ForegroundInfo | null {
  if (!native) return null
  try {
    const hwnd = native.GetForegroundWindow()
    if (!hwnd) return null
    return { hwnd, exe: exeOfWindow(hwnd) }
  } catch {
    return null
  }
}

function exeOfWindow(hwnd: unknown): string | null {
  if (!native) return null
  let handle: unknown = null
  try {
    const pid: number[] = [0]
    native.GetWindowThreadProcessId(hwnd, pid)
    if (!pid[0]) return null

    handle = native.OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid[0])
    if (!handle) return null

    const buf = new Uint16Array(520)
    const size: number[] = [buf.length]
    if (!native.QueryFullProcessImageNameW(handle, 0, buf, size)) return null

    const full = Buffer.from(buf.buffer, 0, size[0] * 2).toString('utf16le')
    return full ? basename(full) : null
  } catch {
    return null
  } finally {
    if (handle && native) {
      try {
        native.CloseHandle(handle)
      } catch {
        /* ignore */
      }
    }
  }
}

/**
 * 读剪贴板里的文件列表。
 * Electron 的 clipboard API 拿不到 CF_HDROP，只能自己开剪贴板去读。
 * 返回 null 表示剪贴板里没有文件（不是出错）。
 */
export function readClipboardFiles(): string[] | null {
  if (!native) return null
  try {
    if (!native.IsClipboardFormatAvailable(CF_HDROP)) return null
    if (!native.OpenClipboard(null)) return null
    try {
      const hDrop = native.GetClipboardData(CF_HDROP)
      if (!hDrop) return null

      const count = native.DragQueryFileCount(hDrop, DRAG_QUERY_COUNT, null, 0)
      if (!count) return null

      const paths: string[] = []
      const buf = new Uint16Array(4096) // 单条路径上限远小于 8KB
      for (let i = 0; i < count; i++) {
        const len = native.DragQueryFilePath(hDrop, i, buf, buf.length)
        if (len > 0) paths.push(Buffer.from(buf.buffer, 0, len * 2).toString('utf16le'))
      }
      return paths.length > 0 ? paths : null
    } finally {
      native.CloseClipboard()
    }
  } catch (err) {
    console.error('[win32] 读剪贴板文件列表失败', err)
    return null
  }
}

/**
 * 把文件列表写回剪贴板（真正的 CF_HDROP，能直接粘贴成文件而不是文本）。
 * 结构是 DROPFILES 头 + 双 null 结尾的宽字符路径串。
 */
export function writeClipboardFiles(paths: string[]): boolean {
  if (!native || paths.length === 0) return false

  const HEADER = 20 // DROPFILES: DWORD pFiles + POINT pt(8) + BOOL fNC + BOOL fWide
  const list = Buffer.concat([
    ...paths.map((p) => Buffer.from(p + '\0', 'utf16le')),
    Buffer.from('\0', 'utf16le'),
  ])
  const payload = Buffer.alloc(HEADER + list.length)
  payload.writeUInt32LE(HEADER, 0) // pFiles：路径串相对结构起始的偏移
  payload.writeUInt32LE(1, 16) // fWide = TRUE（宽字符）
  list.copy(payload, HEADER)

  let hMem: unknown = null
  try {
    hMem = native.GlobalAlloc(GMEM_MOVEABLE, payload.length)
    if (!hMem) return false
    const locked = native.GlobalLock(hMem)
    if (!locked) {
      native.GlobalFree(hMem)
      return false
    }
    native.CopyMemory(locked, payload, payload.length)
    native.GlobalUnlock(hMem)

    if (!native.OpenClipboard(null)) {
      native.GlobalFree(hMem)
      return false
    }
    native.EmptyClipboard()
    const ok = native.SetClipboardData(CF_HDROP, hMem)
    native.CloseClipboard()

    // SetClipboardData 成功后内存所有权归系统，不能再 free
    if (!ok) {
      native.GlobalFree(hMem)
      return false
    }
    return true
  } catch (err) {
    console.error('[win32] 写剪贴板文件列表失败', err)
    try {
      if (hMem) native.GlobalFree(hMem)
    } catch {
      /* ignore */
    }
    return false
  }
}

/**
 * 当前前台窗口属于哪个进程。
 * 用它来判断「用户是不是点到别处去了」——比 blur 事件可靠：
 * 从托盘唤出时 Windows 的前台抢占限制可能让窗口显示了却没拿到焦点，
 * 那种情况下永远不会有 blur，只靠事件就永远收不起来。
 * 返回 null 表示拿不到（原生不可用），调用方不要据此做收起动作。
 */
export function foregroundPid(): number | null {
  if (!native) return null
  try {
    const hwnd = native.GetForegroundWindow()
    if (!hwnd) return null
    const pid: number[] = [0]
    native.GetWindowThreadProcessId(hwnd, pid)
    return pid[0] || null
  } catch {
    return null
  }
}

export function leftMouseButtonDown(): boolean {
  try {
    return native !== null && (native.GetAsyncKeyState(0x01) & 0x8000) !== 0
  } catch {
    return false
  }
}

/** 用 Electron 给的原生句柄强行抢前台，配合 win.focus() 提高成功率 */
export function forceForeground(handle: Buffer): boolean {
  if (!native) return false
  try {
    const hwnd = handle.length >= 8 ? Number(handle.readBigUInt64LE(0)) : handle.readUInt32LE(0)
    if (!hwnd) return false
    return native.SetForegroundWindowByHandle(hwnd)
  } catch (err) {
    console.error('[win32] 抢前台失败', err)
    return false
  }
}

export function focusWindow(hwnd: unknown): boolean {
  if (!native || !hwnd) return false
  try {
    return native.SetForegroundWindow(hwnd)
  } catch {
    return false
  }
}

/**
 * 模拟 Ctrl+V。
 * 先把可能还按着的修饰键放开——热键 Alt+V 唤出面板时 Alt 往往仍处于按下状态，
 * 直接发 Ctrl+V 会变成 Ctrl+Alt+V，目标程序收不到粘贴。
 */
export function sendCtrlV(): boolean {
  if (!native) return false
  try {
    for (const vk of [VK.MENU, VK.SHIFT, VK.LWIN, VK.RWIN, VK.CONTROL]) {
      if ((native.GetAsyncKeyState(vk) & 0x8000) !== 0) {
        native.keybd_event(vk, 0, KEYEVENTF_KEYUP, 0)
      }
    }
    native.keybd_event(VK.CONTROL, 0, 0, 0)
    native.keybd_event(VK.V, 0, 0, 0)
    native.keybd_event(VK.V, 0, KEYEVENTF_KEYUP, 0)
    native.keybd_event(VK.CONTROL, 0, KEYEVENTF_KEYUP, 0)
    return true
  } catch (err) {
    console.error('[win32] 模拟粘贴失败', err)
    return false
  }
}
