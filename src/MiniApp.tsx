import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { Clipboard, Maximize2, Search, X } from 'lucide-react'
import { Logo } from '@/components/Logo'
import type { AutoKind, FilterId, ItemKind, ListQuery, PasteOutcome } from '@shared/types'
import { api } from '@/lib/api'
import { useItems } from '@/hooks/useItems'
import { useTheme } from '@/hooks/useTheme'
import { MiniRow } from '@/components/MiniRow'
import { Toast, type ToastMessage } from '@/components/Toast'
import { DEFAULT_VISIBLE_FILTERS, visibleKindFilters } from '@/lib/kinds'
import { applyAccent } from '@/lib/accent'
import { applyPanelBackgroundOpacity } from '@/lib/opacity'

const PASTE_FAILURE_TEXT: Record<NonNullable<PasteOutcome['reason']>, string> = {
  'no-native': '已复制，请手动 Ctrl+V',
  'no-target': '已复制，请手动 Ctrl+V',
  'focus-failed': '已复制，但切不回原窗口',
  'send-failed': '已复制，模拟按键失败',
  'not-found': '这条记录不存在了',
}

/** 迷你面板只看最近这些条，够预览就行 */
const LIMIT = 40

export default function MiniApp() {
  useTheme()

  const [q, setQ] = useState('')
  const [kind, setKind] = useState<ItemKind | null>(null)
  const [autoKind, setAutoKind] = useState<AutoKind | null>(null)
  const [selectedId, setSelectedId] = useState<number | null>(null)
  const [toast, setToast] = useState<ToastMessage | null>(null)
  const [searching, setSearching] = useState(false)
  const [quickPasteModifiers, setQuickPasteModifiers] = useState('Ctrl+Alt')
  const [visibleFilters, setVisibleFilters] = useState<FilterId[]>(DEFAULT_VISIBLE_FILTERS)

  const inputRef = useRef<HTMLInputElement>(null)
  const toastTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const toastSeq = useRef(0)

  const query = useMemo<ListQuery>(
    () => ({ q, kind, autoKind, limit: LIMIT }),
    [q, kind, autoKind],
  )
  const { items } = useItems(query)

  const index = items.findIndex((it) => it.id === selectedId)

  const showToast = useCallback((text: string, tone: 'ok' | 'warn' = 'ok') => {
    if (toastTimer.current) clearTimeout(toastTimer.current)
    setToast({ id: ++toastSeq.current, text, tone })
    toastTimer.current = setTimeout(() => setToast(null), tone === 'warn' ? 3000 : 1500)
  }, [])

  const loadPreferences = useCallback(() => {
    void api.getSettings().then((settings) => {
      setQuickPasteModifiers(settings.quickPasteModifiers)
      setVisibleFilters(settings.visibleFilters)
      applyAccent(settings.accent)
      applyPanelBackgroundOpacity(settings.opacity)
    })
  }, [])

  useEffect(() => {
    if (items.length === 0) setSelectedId(null)
    else if (!items.some((it) => it.id === selectedId)) setSelectedId(items[0].id)
  }, [items, selectedId])

  useEffect(() => loadPreferences(), [loadPreferences])

  // 每次弹出都回到干净状态
  useEffect(
    () =>
      api.onPanelShown(() => {
        setQ('')
        setKind(null)
        setAutoKind(null)
        setSearching(false)
        loadPreferences()
      }),
    [loadPreferences],
  )

  const paste = useCallback(
    (id: number) => {
      void api.paste(id).then((r) => {
        if (!r.ok) showToast(PASTE_FAILURE_TEXT[r.reason ?? 'send-failed'], 'warn')
      })
    },
    [showToast],
  )

  const selectItem = useCallback((id: number) => {
    setSelectedId(id)
    void api.crossDeviceStatus().then((status) => {
      if (!status.connected) return
      void api.sendCrossDeviceItem(id)
    })
  }, [])

  const move = useCallback(
    (delta: number) => {
      if (items.length === 0) return
      const from = index < 0 ? 0 : index
      selectItem(items[Math.min(Math.max(from + delta, 0), items.length - 1)].id)
    },
    [index, items, selectItem],
  )

  useEffect(() => {
    const onKey = (e: KeyboardEvent): void => {
      if (e.key === 'Escape') {
        if (searching) {
          setSearching(false)
          setQ('')
        } else void api.hidePanel()
        return
      }
      switch (e.key) {
        case 'ArrowDown':
          e.preventDefault()
          move(1)
          return
        case 'ArrowUp':
          e.preventDefault()
          move(-1)
          return
        case 'Enter':
          if (selectedId !== null) {
            e.preventDefault()
            paste(selectedId)
          }
          return
      }
      // 直接打字就进搜索
      if (!searching && !e.ctrlKey && !e.altKey && e.key.length === 1) {
        setSearching(true)
        setQ(e.key)
        requestAnimationFrame(() => inputRef.current?.focus())
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [items, move, paste, searching, selectedId])

  return (
    <div className="panel-surface relative flex h-full flex-col overflow-hidden text-black dark:text-white">
      {/* 顶栏：可拖动，右侧是展开和关闭 */}
      <div className="drag-region flex items-center gap-2 px-2.5 pt-2.5 pb-1.5">
        <Logo className="no-drag size-6 shrink-0 overflow-hidden rounded-[5px] bg-white p-[1.5px] shadow-sm shadow-black/12 ring-1 ring-black/5 dark:shadow-black/25 dark:ring-white/15" />

        {searching ? (
          <div className="no-drag relative flex-1">
            <Search className="pointer-events-none absolute top-1/2 left-2 size-3 -translate-y-1/2 text-black/35 dark:text-white/35" />
            <input
              ref={inputRef}
              autoFocus
              value={q}
              onChange={(e) => setQ(e.target.value)}
              placeholder="搜索内容或来源…"
              spellCheck={false}
              className="h-6.5 w-full rounded-md border border-brand-500/40 bg-white/70 pr-2 pl-7 text-[11.5px] outline-none dark:bg-white/8 dark:text-white/90"
            />
          </div>
        ) : (
          <>
            <span className="flex-1 truncate text-[11.5px] font-medium text-black/65 dark:text-white/70">
              最近剪贴
            </span>
            <button
              onClick={() => setSearching(true)}
              title="搜索（直接打字也行）"
              className="no-drag flex size-6 items-center justify-center rounded-md text-black/40 transition hover:bg-black/8 hover:text-black/70 dark:text-white/40 dark:hover:bg-white/10"
            >
              <Search className="size-3.5" />
            </button>
          </>
        )}

        <button
          onClick={() => void api.expandPanel()}
          title="展开完整面板"
          className="no-drag flex size-6 shrink-0 items-center justify-center rounded-md text-black/40 transition hover:bg-black/8 hover:text-black/70 dark:text-white/40 dark:hover:bg-white/10"
        >
          <Maximize2 className="size-3.5" />
        </button>
        <button
          onClick={() => void api.hidePanel()}
          title="收起 (Esc)"
          className="no-drag flex size-6 shrink-0 items-center justify-center rounded-md text-black/40 transition hover:bg-red-500/85 hover:text-white dark:text-white/40"
        >
          <X className="size-3.5" />
        </button>
      </div>

      {/* 类型筛选 */}
      <div className="flex items-center gap-1 overflow-x-auto px-2.5 pb-1.5">
        {visibleKindFilters(visibleFilters).map((f) => (
          <button
            key={f.id}
            onClick={() => {
              setKind(f.kind)
              setAutoKind(f.autoKind)
            }}
            className={`h-5.5 rounded-full px-2 text-[10.5px] font-medium transition ${
              kind === f.kind && autoKind === f.autoKind
                ? 'bg-brand-500 text-white'
                : 'bg-black/5 text-black/50 hover:bg-black/10 dark:bg-white/7 dark:text-white/50 dark:hover:bg-white/12'
            }`}
          >
            {f.label}
          </button>
        ))}
      </div>

      {/* 列表 */}
      <div className="min-h-0 flex-1 space-y-0.5 overflow-y-auto border-t border-black/6 px-1.5 py-1.5 dark:border-white/8">
        {items.length === 0 ? (
          <div className="flex h-full flex-col items-center justify-center gap-1.5 px-4 text-center">
            <Clipboard className="size-5 text-black/20 dark:text-white/20" />
            <div className="text-[11.5px] text-black/40 dark:text-white/40">
              {q ? '没有匹配的记录' : '还没有记录'}
            </div>
            {!q && (
              <div className="text-[10px] leading-4 text-black/28 dark:text-white/28">
                复制任何文字、图片或文件，
                <br />
                Witch Clipboard 会自动收进来
              </div>
            )}
          </div>
        ) : (
          items.map((item, i) => (
            <MiniRow
              key={item.id}
              item={item}
              selected={item.id === selectedId}
              hotIndex={i < 9 ? i + 1 : null}
              onSelect={() => selectItem(item.id)}
              onPaste={() => paste(item.id)}
              onReveal={() => void api.revealFile(item.id)}
            />
          ))
        )}
      </div>

      {/* 底栏 */}
      <div className="flex items-center gap-2 border-t border-black/6 px-2.5 py-1.5 text-[9.5px] text-black/35 dark:border-white/8 dark:text-white/35">
        <span>双击或 Enter 粘贴</span>
        <span className="text-black/15 dark:text-white/15">·</span>
        <span>{quickPasteModifiers}+1…9 快贴</span>
        <span className="ml-auto tabular-nums">{items.length}</span>
      </div>

      <Toast message={toast} />
    </div>
  )
}
