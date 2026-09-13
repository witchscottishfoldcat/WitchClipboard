import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { AnimatePresence } from 'motion/react'
import type {
  AutoKind,
  FilterId,
  ItemKind,
  ListQuery,
  PasteOutcome,
  UpdateStatus,

  PasteTransform,
  Group,} from '@shared/types'
import { ClipboardPaste } from 'lucide-react'
import { api } from '@/lib/api'
import { useItems, useStats, useTags } from '@/hooks/useItems'
import { useTheme } from '@/hooks/useTheme'
import { Header } from '@/components/Header'
import { FilterBar } from '@/components/FilterBar'
import { ItemList } from '@/components/ItemList'
import { PreviewPane } from '@/components/PreviewPane'
import { Footer } from '@/components/Footer'
import { SettingsSheet } from '@/components/SettingsSheet'
import { Toast, type ToastMessage } from '@/components/Toast'
import { UpdateBanner } from '@/components/UpdateBanner'
import { CrossDeviceSheet } from '@/components/CrossDeviceSheet'
import { DEFAULT_VISIBLE_FILTERS } from '@/lib/kinds'
import { applyAccent } from '@/lib/accent'
import { applyPanelBackgroundOpacity } from '@/lib/opacity'

const PASTE_FAILURE_TEXT: Record<NonNullable<PasteOutcome['reason']>, string> = {
  'no-native': '已复制到剪贴板，请手动 Ctrl+V（原生能力不可用）',
  'no-target': '已复制，但没记录到目标窗口，请手动 Ctrl+V',
  'focus-failed': '已复制，但切不回原窗口，请手动 Ctrl+V',
  'target-elevated': '目标窗口以管理员运行，无法自动粘贴；已复制，请手动 Ctrl+V',
  'send-failed': '已复制，模拟按键失败，请手动 Ctrl+V',
  'not-found': '这条记录已经不存在了',
}

export default function App() {
  useTheme()

  const [q, setQ] = useState('')
  const [kind, setKind] = useState<ItemKind | null>(null)
  const [autoKind, setAutoKind] = useState<AutoKind | null>(null)
  const [tag, setTag] = useState<string | null>(null)
  const [pinnedOnly, setPinnedOnly] = useState(false)
  const [groupId, setGroupId] = useState<number | null>(null)
  const [groups, setGroups] = useState<Group[]>([])
  const [multiSelected, setMultiSelected] = useState<Set<number>>(new Set())
  const anchorIdRef = useRef<number | null>(null)
  const [selectedId, setSelectedId] = useState<number | null>(null)
  const [settingsOpen, setSettingsOpen] = useState(false)
  const [crossDeviceOpen, setCrossDeviceOpen] = useState(false)
  const [hotkey, setHotkey] = useState('Alt+V')
  const [quickPasteModifiers, setQuickPasteModifiers] = useState('Ctrl+Alt')
  const [visibleFilters, setVisibleFilters] = useState<FilterId[]>(DEFAULT_VISIBLE_FILTERS)
  const [toast, setToast] = useState<ToastMessage | null>(null)
  const [update, setUpdate] = useState<UpdateStatus | null>(null)

  const inputRef = useRef<HTMLInputElement>(null)
  const toastTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const toastSeq = useRef(0)

  const query = useMemo<ListQuery>(
    () => ({ q, kind, autoKind, tag, pinnedOnly, groupId }),
    [q, kind, autoKind, tag, pinnedOnly, groupId],
  )
  const { items, total, loading } = useItems(query)
  const stats = useStats()
  const tags = useTags()

  const selected = items.find((it) => it.id === selectedId) ?? null
  const index = items.findIndex((it) => it.id === selectedId)
  const filtered = Boolean(q || kind || autoKind || tag || pinnedOnly || groupId !== null)

  const showToast = useCallback((text: string, tone: 'ok' | 'warn' = 'ok') => {
    if (toastTimer.current) clearTimeout(toastTimer.current)
    setToast({ id: ++toastSeq.current, text, tone })
    toastTimer.current = setTimeout(() => setToast(null), tone === 'warn' ? 3600 : 1800)
  }, [])

  const loadPreferences = useCallback(() => {
    void api.getSettings().then((s) => {
      setHotkey(s.hotkey)
      setQuickPasteModifiers(s.quickPasteModifiers)
      setVisibleFilters(s.visibleFilters)
      applyAccent(s.accent)
      applyPanelBackgroundOpacity(s.opacity)
    })
  }, [])

  useEffect(() => loadPreferences(), [loadPreferences])

  const reloadGroups = useCallback(() => {
    void api.groups().then(setGroups)
  }, [])
  useEffect(() => {
    reloadGroups()
    return api.onChanged(reloadGroups)
  }, [reloadGroups])

  // 后台热键粘贴失败时给可见提示（面板隐藏时事件无害）
  useEffect(
    () =>
      api.onPasteFailed((reason) => {
        showToast(PASTE_FAILURE_TEXT[reason as NonNullable<PasteOutcome['reason']>] ?? '后台粘贴失败', 'warn')
      }),
    [showToast],
  )

  // 更新状态：主进程启动后会自动查一次，有结果就推过来
  useEffect(() => {
    void api.updateStatus().then(setUpdate)
    return api.onUpdateStatus(setUpdate)
  }, [])

  // 结果变化后保证有选中项
  useEffect(() => {
    if (items.length === 0) {
      setSelectedId(null)
    } else if (!items.some((it) => it.id === selectedId)) {
      setSelectedId(items[0].id)
    }
  }, [items, selectedId])

  // 面板每次弹出：清空筛选、回到顶部、聚焦搜索框
  useEffect(
    () =>
      api.onPanelShown(() => {
        setQ('')
        setKind(null)
        setAutoKind(null)
        setTag(null)
        setPinnedOnly(false)
        setGroupId(null)
        setMultiSelected(new Set())
        anchorIdRef.current = null
        setSettingsOpen(false)
        setCrossDeviceOpen(false)
        inputRef.current?.focus()
        inputRef.current?.select()
      }),
    [],
  )

  useEffect(() => {
    inputRef.current?.focus()
  }, [])

  const paste = useCallback(
    (id: number) => {
      void api.paste(id).then((r) => {
        if (!r.ok) showToast(PASTE_FAILURE_TEXT[r.reason ?? 'send-failed'], 'warn')
      })
    },
    [showToast],
  )

  const copy = useCallback(
    (id: number) => {
      void api.copy(id).then(() => showToast('已复制到剪贴板'))
    },
    [showToast],
  )

  const togglePin = useCallback((id: number) => void api.togglePin(id), [])
  const remove = useCallback((id: number) => void api.remove(id), [])
  const setItemTags = useCallback((id: number, t: string[]) => void api.setTags(id, t), [])

  const pasteMulti = useCallback(() => {
    // 按当前列表顺序粘贴，而不是点击顺序
    const ids = items.filter((it) => multiSelected.has(it.id)).map((it) => it.id)
    if (ids.length === 0) return
    setMultiSelected(new Set())
    void api.pasteItems(ids).then((r) => {
      if (!r.ok) showToast(PASTE_FAILURE_TEXT[r.reason ?? 'send-failed'], 'warn')
    })
  }, [items, multiSelected, showToast])

  const setItemNote = useCallback((id: number, note: string | null) => {
    void api.setItemNote(id, note)
  }, [])

  const setItemGroup = useCallback(
    (id: number, target: number | null) => {
      void api.itemSetGroup(id, target).then(reloadGroups)
    },
    [reloadGroups],
  )

  const setItemHotkey = useCallback(
    (id: number, hotkey: string | null) => {
      void api.setItemHotkey(id, hotkey).catch((error: unknown) => {
        const reason = typeof error === 'string' ? error : String(error)
        const text = reason.includes('reserved')
          ? '这个热键已留给面板功能，请换一个组合'
          : reason.includes('conflict')
            ? '热键冲突：已被其他条目或程序占用'
            : reason.includes('invalid')
              ? '无法识别这个组合键'
              : '热键设置失败'
        showToast(text, 'warn')
      })
    },
    [showToast],
  )

  const pasteTransformed = useCallback(
    (id: number, transform: PasteTransform) => {
      void api.pasteTransformed(id, transform).then((r) => {
        if (!r.ok) showToast(PASTE_FAILURE_TEXT[r.reason ?? 'send-failed'], 'warn')
      })
    },
    [showToast],
  )

  /** 手机在线时，用户在历史列表中选中哪条就立即发送哪条。 */
  const selectItem = useCallback((id: number) => {
    setSelectedId(id)
    void api.crossDeviceStatus().then((status) => {
      if (!status.connected) return
      void api.sendCrossDeviceItem(id)
    })
  }, [])

  /** Ctrl 点选加入/移出多选集合，Shift 从锚点连选，普通点击清空多选并单选。 */
  const handleRowSelect = useCallback(
    (id: number, e: { ctrlKey: boolean; shiftKey: boolean; metaKey: boolean; preventDefault: () => void }) => {
      const toggle = e.ctrlKey || e.metaKey
      if (toggle) {
        e.preventDefault()
        setMultiSelected((current) => {
          const next = new Set(current)
          if (next.has(id)) next.delete(id)
          else next.add(id)
          return next
        })
        anchorIdRef.current = id
        setSelectedId(id)
        return
      }
      if (e.shiftKey && anchorIdRef.current !== null) {
        e.preventDefault()
        const from = items.findIndex((it) => it.id === anchorIdRef.current)
        const to = items.findIndex((it) => it.id === id)
        if (from >= 0 && to >= 0) {
          const [lo, hi] = from < to ? [from, to] : [to, from]
          setMultiSelected(new Set(items.slice(lo, hi + 1).map((it) => it.id)))
          setSelectedId(id)
        }
        return
      }
      setMultiSelected(new Set())
      anchorIdRef.current = id
      selectItem(id)
    },
    [items, selectItem],
  )

  const move = useCallback(
    (delta: number) => {
      if (items.length === 0) return
      const from = index < 0 ? 0 : index
      const next = Math.min(Math.max(from + delta, 0), items.length - 1)
      selectItem(items[next].id)
    },
    [index, items, selectItem],
  )

  useEffect(() => {
    const onKey = (e: KeyboardEvent): void => {
      if (e.key === 'Escape') {
        if (crossDeviceOpen) setCrossDeviceOpen(false)
        else if (settingsOpen) setSettingsOpen(false)
        else if (q) setQ('')
        else void api.hidePanel()
        return
      }
      if (settingsOpen || crossDeviceOpen) return

      switch (e.key) {
        case 'ArrowDown':
          e.preventDefault()
          move(1)
          return
        case 'ArrowUp':
          e.preventDefault()
          move(-1)
          return
        case 'PageDown':
          e.preventDefault()
          move(6)
          return
        case 'PageUp':
          e.preventDefault()
          move(-6)
          return
        case 'Home':
          if (!q) {
            e.preventDefault()
            move(-items.length)
          }
          return
        case 'End':
          if (!q) {
            e.preventDefault()
            move(items.length)
          }
          return
        case 'Enter':
          if (multiSelected.size > 0) {
            e.preventDefault()
            pasteMulti()
          } else if (selectedId !== null) {
            e.preventDefault()
            paste(selectedId)
          }
          return
        case 'Delete':
          if (selectedId !== null) {
            e.preventDefault()
            remove(selectedId)
          }
          return
      }

      if (e.ctrlKey && (e.key === 'c' || e.key === 'C')) {
        // 预览区有选中文本时让浏览器默认复制行为生效
        if ((window.getSelection()?.toString() ?? '').length > 0) return
        if (selectedId !== null) {
          e.preventDefault()
          copy(selectedId)
        }
        return
      }
      if (e.ctrlKey && (e.key === 'p' || e.key === 'P')) {
        if (selectedId !== null) {
          e.preventDefault()
          togglePin(selectedId)
        }
        return
      }
      // 用 e.code：中文输入法激活时 e.key 拿不到逗号
      if (e.ctrlKey && (e.code === 'Comma' || e.key === ',')) {
        e.preventDefault()
        setSettingsOpen(true)
      }
    }

    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [
    copy,
    crossDeviceOpen,
    items,
    move,
    multiSelected,
    paste,
    pasteMulti,
    q,
    remove,
    selectedId,
    settingsOpen,
    togglePin,
  ])

  return (
    <div className="panel-surface relative flex h-full flex-col overflow-hidden text-black dark:text-white">
      <Header
        value={q}
        onChange={setQ}
        inputRef={inputRef}
        stats={stats}
        onClose={() => void api.hidePanel()}
        onOpenSettings={() => {
          setCrossDeviceOpen(false)
          setSettingsOpen(true)
        }}
        onOpenCrossDevice={() => {
          setSettingsOpen(false)
          setCrossDeviceOpen(true)
        }}
      />

      <AnimatePresence>
        {(update?.state === 'available' || update?.state === 'ready') && (
          <UpdateBanner
            status={update}
            onOpen={() => setSettingsOpen(true)}
            onSkip={() => void api.skipUpdate(update.version).then(setUpdate)}
          />
        )}
      </AnimatePresence>

      <FilterBar
        kind={kind}
        onKind={setKind}
        autoKind={autoKind}
        onAutoKind={setAutoKind}
        tags={tags}
        activeTag={tag}
        onTag={setTag}
        pinnedOnly={pinnedOnly}
        onPinnedOnly={setPinnedOnly}
        visibleFilters={visibleFilters}
        groups={groups}
        activeGroupId={groupId}
        onGroup={setGroupId}
        onCreateGroup={(name, parentId) => {
          void api.groupCreate(name, parentId).then(() => reloadGroups())
        }}
        onDeleteGroup={(id) => {
          void api.groupDelete(id).then(() => {
            if (groupId === id) setGroupId(null)
            reloadGroups()
          })
        }}
      />

      <div className="flex min-h-0 flex-1 border-t border-black/6 dark:border-white/8">
        {multiSelected.size > 0 && (
          <div className="absolute inset-x-0 bottom-10 z-10 flex justify-center">
            <div className="flex items-center gap-2 rounded-full bg-black/78 px-3 py-1.5 text-[12px] text-white shadow-lg backdrop-blur dark:bg-white/85 dark:text-black">
              已选 {multiSelected.size} 条
              <button
                onClick={pasteMulti}
                className="inline-flex h-6 items-center gap-1 rounded-full bg-brand-500 px-2.5 font-medium text-white transition hover:bg-brand-600"
              >
                <ClipboardPaste className="size-3" />
                按顺序粘贴
              </button>
              <button
                onClick={() => setMultiSelected(new Set())}
                className="h-6 rounded-full px-2 text-white/70 transition hover:bg-white/12 dark:text-black/60 dark:hover:bg-black/8"
              >
                取消
              </button>
            </div>
          </div>
        )}
        <ItemList
          items={items}
          selectedId={selectedId}
          multiSelectedIds={multiSelected}
          onSelect={handleRowSelect}
          onPaste={paste}
          onTogglePin={togglePin}
          loading={loading}
          libraryEmpty={!filtered && (stats?.total ?? 0) === 0}
          hotkey={hotkey}
        />
        <PreviewPane
          item={selected}
          groups={groups}
          onPaste={paste}
          onCopy={copy}
          onTogglePin={togglePin}
          onRemove={remove}
          onSetTags={setItemTags}
          onReveal={(id) => void api.revealFile(id)}
          onSetNote={setItemNote}
          onSetGroup={setItemGroup}
          onSetHotkey={setItemHotkey}
          onPasteTransformed={pasteTransformed}
        />
      </div>

      <Footer
        count={items.length}
        total={stats?.total ?? total}
        quickPasteModifiers={quickPasteModifiers}
      />

      <Toast message={toast} />

      <AnimatePresence>
        {settingsOpen && (
          <SettingsSheet
            onClose={() => {
              setSettingsOpen(false)
              setKind(null)
              setAutoKind(null)
              loadPreferences()
            }}
            onCleared={() => setSettingsOpen(false)}
            onToast={showToast}
          />
        )}
      </AnimatePresence>

      <AnimatePresence>
        {crossDeviceOpen && (
          <CrossDeviceSheet
            item={selected}
            onClose={() => setCrossDeviceOpen(false)}
            onToast={showToast}
          />
        )}
      </AnimatePresence>
    </div>
  )
}
