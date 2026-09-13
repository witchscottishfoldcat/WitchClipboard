import { useEffect, useState, type KeyboardEvent } from 'react'
import { AnimatePresence, motion } from 'motion/react'
import {
  ClipboardPaste,
  Copy,
  Pin,
  Trash2,
  Plus,
  X,
  Hash,
  FolderOpen,
  Link2,
  ChevronRight,
  Keyboard,
} from 'lucide-react'
import type { ClipItem, Group, PasteTransform } from '@shared/types'
import { badgeOf, colorValue } from '@/lib/kinds'
import { fileInfo } from '@/lib/files'
import { absoluteTime, formatBytes } from '@/lib/format'
import { api } from '@/lib/api'

const TRANSFORMS: ReadonlyArray<readonly [PasteTransform, string, boolean]> = [
  ['plainText', '纯文本', false],
  ['upper', '大写', true],
  ['lower', '小写', true],
  ['capitalize', '首字母', true],
  ['sentence', '句首', true],
  ['camel', 'camel', true],
  ['trim', '去空白', false],
]

/** 扁平分组列表转带缩进层级的下拉选项 */
function groupOptions(groups: Group[]): Array<Group & { depth: number }> {
  const depthOf = (group: Group, guard = 0): number => {
    if (!group.parentId || guard > 8) return 0
    const parent = groups.find((candidate) => candidate.id === group.parentId)
    return parent ? depthOf(parent, guard + 1) + 1 : 0
  }
  return groups.map((group) => ({ ...group, depth: depthOf(group) }))
}

/** 条目级持久热键的录制与展示；Backspace 清除、Escape 取消 */
function HotkeySetter({
  current,
  onCommit,
}: {
  current: string | null
  onCommit: (value: string | null) => void
}) {
  const [recording, setRecording] = useState(false)

  const capture = (e: KeyboardEvent<HTMLInputElement>): void => {
    e.preventDefault()
    e.stopPropagation()
    if (e.key === 'Escape') {
      setRecording(false)
      return
    }
    if (e.key === 'Backspace') {
      if (current) onCommit(null)
      setRecording(false)
      return
    }
    if (['Control', 'Alt', 'Shift', 'Meta'].includes(e.key)) return
    const parts: string[] = []
    if (e.ctrlKey) parts.push('Ctrl')
    if (e.altKey) parts.push('Alt')
    if (e.shiftKey) parts.push('Shift')
    if (e.metaKey) parts.push('Super')
    if (parts.length === 0) return // 全局热键必须带修饰键
    parts.push(e.key.length === 1 ? e.key.toUpperCase() : e.key)
    onCommit(parts.join('+'))
    setRecording(false)
  }

  if (recording) {
    return (
      <input
        autoFocus
        readOnly
        value="按下组合键…"
        onKeyDown={capture}
        onBlur={() => setRecording(false)}
        className="h-6.5 w-24 shrink-0 rounded-md border border-brand-500/60 bg-brand-500/8 px-1.5 text-center text-[10.5px] text-brand-600 outline-none dark:text-brand-400"
      />
    )
  }
  return (
    <button
      onClick={() => setRecording(true)}
      title={current ? '点击重设 · 录制中按 Backspace 清除' : '设置全局粘贴热键'}
      className="inline-flex h-6.5 shrink-0 items-center gap-1 rounded-md border border-black/10 px-1.5 text-[10.5px] text-black/55 transition hover:border-brand-500/50 hover:text-brand-600 dark:border-white/12 dark:text-white/55 dark:hover:text-brand-400"
    >
      <Keyboard className="size-3" />
      {current ?? '热键'}
    </button>
  )
}

/** 预览面板显示原图；列表里用的是缩略图 */
function useFullImage(item: ClipItem | null): string | null {
  const [url, setUrl] = useState<string | null>(null)
  const id = item?.kind === 'image' ? item.id : null

  useEffect(() => {
    if (id === null) {
      setUrl(null)
      return
    }
    let alive = true
    void api.imageDataUrl(id).then((u) => {
      if (alive) setUrl(u)
    })
    return () => {
      alive = false
    }
  }, [id])

  return url
}

function useRelatedItems(item: ClipItem | null): ClipItem[] {
  const [related, setRelated] = useState<ClipItem[]>([])
  const id = item?.id ?? null

  useEffect(() => {
    if (id === null) {
      setRelated([])
      return
    }
    let alive = true
    const load = (): void => {
      void api.relatedItems(id, 5).then((items) => {
        if (alive) setRelated(items)
      })
    }
    load()
    const unsubscribe = api.onChanged(load)
    return () => {
      alive = false
      unsubscribe()
    }
  }, [id])

  return related
}

function relatedPreview(item: ClipItem): string {
  if (item.autoKind !== 'key') return item.preview
  const value = item.preview.trim()
  if (value.length <= 10) return '••••••••'
  return `${value.slice(0, 5)}••••${value.slice(-4)}`
}

interface Props {
  item: ClipItem | null
  groups: Group[]
  onPaste: (id: number) => void
  onCopy: (id: number) => void
  onTogglePin: (id: number) => void
  onRemove: (id: number) => void
  onSetTags: (id: number, tags: string[]) => void
  onReveal: (id: number) => void
  onSetNote: (id: number, note: string | null) => void
  onSetGroup: (id: number, groupId: number | null) => void
  onSetHotkey: (id: number, hotkey: string | null) => void
  onPasteTransformed: (id: number, transform: PasteTransform) => void
}

export function PreviewPane({
  item,
  groups,
  onPaste,
  onCopy,
  onTogglePin,
  onRemove,
  onSetTags,
  onReveal,
  onSetNote,
  onSetGroup,
  onSetHotkey,
  onPasteTransformed,
}: Props) {
  const [draft, setDraft] = useState('')
  const [adding, setAdding] = useState(false)
  const [relatedCollapsed, setRelatedCollapsed] = useState(false)
  const fullImage = useFullImage(item)
  const related = useRelatedItems(item)

  if (!item) {
    return (
      <div className="flex w-[300px] shrink-0 items-center justify-center border-l border-black/6 text-[12px] text-black/30 dark:border-white/8 dark:text-white/30">
        选中一条记录查看详情
      </div>
    )
  }

  const badge = badgeOf(item)
  const color = colorValue(item)
  const files = fileInfo(item)
  const isCode = item.kind === 'text' && item.autoKind === 'code'

  const commitTag = (e: KeyboardEvent<HTMLInputElement>): void => {
    if (e.key === 'Enter' && draft.trim()) {
      onSetTags(item.id, [...item.tags, draft.trim()])
      setDraft('')
      setAdding(false)
    } else if (e.key === 'Escape') {
      setDraft('')
      setAdding(false)
      e.stopPropagation()
    }
  }

  return (
    <div className="flex w-[300px] shrink-0 flex-col border-l border-black/6 dark:border-white/8">
      <AnimatePresence mode="wait">
        <motion.div
          key={item.id}
          initial={{ opacity: 0, y: 6 }}
          animate={{ opacity: 1, y: 0 }}
          exit={{ opacity: 0, y: -4 }}
          transition={{ duration: 0.16, ease: 'easeOut' }}
          className="flex min-h-0 flex-1 flex-col"
        >
          {/* 元信息 */}
          <div className="flex items-center gap-1.5 px-3.5 pt-3 pb-2">
            <span
              className={`inline-flex h-5 items-center rounded px-1.5 text-[10px] font-semibold ${badge.chip}`}
            >
              {badge.label}
            </span>
            {item.pinned && (
              <Pin className="size-3 fill-brand-500 text-brand-500" strokeWidth={2} />
            )}
            <span className="ml-auto text-[10.5px] text-black/35 tabular-nums dark:text-white/35">
              {formatBytes(item.bytes)}
            </span>
          </div>

          {/* 内容 */}
          <div className="min-h-0 flex-1 overflow-y-auto px-3.5">
            {item.kind === 'image' ? (
              <div className="space-y-1.5">
                <div className="overflow-hidden rounded-xl border border-black/8 bg-[repeating-conic-gradient(#0000_0_25%,#8881_0_50%)] bg-[length:16px_16px] dark:border-white/10">
                  <img
                    src={fullImage ?? item.thumb ?? ''}
                    alt=""
                    draggable={false}
                    className="w-full object-contain"
                  />
                </div>
                <div className="text-[10.5px] text-black/40 tabular-nums dark:text-white/40">
                  {item.width}×{item.height} px{fullImage ? '' : ' · 缩略图'}
                </div>
              </div>
            ) : files ? (
              <div className="space-y-1.5">
                {files.paths.map((p, i) => (
                  <div
                    key={p}
                    className="group flex items-start gap-1.5 rounded-lg bg-black/4 px-2 py-1.5 dark:bg-white/6"
                  >
                    <FolderOpen className="mt-0.5 size-3.5 shrink-0 text-black/35 dark:text-white/35" />
                    <div className="min-w-0 flex-1">
                      <div className="truncate text-[11.5px] text-black/80 dark:text-white/80">
                        {p.split(/[\\/]/).pop()}
                      </div>
                      {/* 完整路径：找视频/大文件时要的就是这个 */}
                      <div className="selectable break-all text-[10px] leading-4 text-black/40 dark:text-white/40">
                        {p}
                      </div>
                    </div>
                    {i === 0 && (
                      <button
                        onClick={() => onReveal(item.id)}
                        title="在资源管理器中定位"
                        className="shrink-0 rounded-md p-1 text-black/35 opacity-0 transition group-hover:opacity-100 hover:bg-black/8 hover:text-black/70 dark:text-white/35 dark:hover:bg-white/12"
                      >
                        <FolderOpen className="size-3" />
                      </button>
                    )}
                  </div>
                ))}
                <div className="text-[10.5px] text-black/40 dark:text-white/40">
                  {files.paths.length} 个文件 · 共 {formatBytes(item.bytes)} · 只记录路径，不复制文件内容
                </div>
              </div>
            ) : color ? (
              <div className="space-y-2">
                <div
                  className="h-24 w-full rounded-xl border border-black/10 dark:border-white/15"
                  style={{ background: color }}
                />
                <div className="selectable font-mono text-[12px] text-black/70 dark:text-white/70">
                  {item.text}
                </div>
              </div>
            ) : (
              <pre
                className={`selectable m-0 whitespace-pre-wrap break-words text-[12px] leading-[1.55] text-black/78 dark:text-white/78 ${
                  isCode ? 'font-mono' : 'font-sans'
                }`}
              >
                {item.text}
              </pre>
            )}
          </div>

          {/* 自动关联：不依赖内容分类，5 秒内复制的条目视为同一组 */}
          {related.length > 0 && (
            <div className="px-3.5 pt-2.5">
              <button
                type="button"
                onClick={() => setRelatedCollapsed((collapsed) => !collapsed)}
                aria-expanded={!relatedCollapsed}
                title={relatedCollapsed ? '展开关联配置' : '收起关联配置'}
                className={`flex w-full items-center gap-1 rounded-md py-0.5 text-[10.5px] font-medium text-black/45 transition hover:text-black/65 dark:text-white/45 dark:hover:text-white/70 ${
                  relatedCollapsed ? '' : 'mb-1.5'
                }`}
              >
                <Link2 className="size-3" />
                关联配置
                <span className="ml-auto text-[9px] font-normal text-black/28 dark:text-white/28">
                  5 秒内 · 最多 5 条
                </span>
                <ChevronRight
                  className={`size-3 transition-transform ${relatedCollapsed ? '' : 'rotate-90'}`}
                />
              </button>
              {!relatedCollapsed && (
                <div className="space-y-1">
                  {related.map((relatedItem) => {
                    const relatedBadge = badgeOf(relatedItem)
                    return (
                      <button
                        key={relatedItem.id}
                        type="button"
                        onClick={() => onCopy(relatedItem.id)}
                        title="复制这条关联内容"
                        className="flex h-8 w-full items-center gap-1.5 rounded-lg bg-black/[0.035] px-2 text-left transition hover:bg-brand-500/10 dark:bg-white/[0.055] dark:hover:bg-brand-500/14"
                      >
                        <span
                          className={`shrink-0 rounded px-1 py-0.5 text-[8.5px] font-semibold ${relatedBadge.chip}`}
                        >
                          {relatedBadge.label}
                        </span>
                        <span className="min-w-0 flex-1 truncate text-[10.5px] text-black/58 dark:text-white/60">
                          {relatedPreview(relatedItem)}
                        </span>
                        <Copy className="size-3 shrink-0 text-black/28 dark:text-white/28" />
                      </button>
                    )
                  })}
                </div>
              )}
            </div>
          )}

          {/* 标签 */}
          <div className="flex flex-wrap items-center gap-1.5 px-3.5 pt-2.5">
            {item.tags.map((t) => (
              <span
                key={t}
                className="group inline-flex h-6 items-center gap-1 rounded-full bg-black/6 pr-1 pl-2 text-[11px] text-black/60 dark:bg-white/10 dark:text-white/60"
              >
                <Hash className="size-2.5 opacity-60" />
                {t}
                <button
                  onClick={() => onSetTags(item.id, item.tags.filter((x) => x !== t))}
                  className="flex size-4 items-center justify-center rounded-full text-black/35 transition hover:bg-black/10 hover:text-red-500 dark:text-white/35 dark:hover:bg-white/15"
                >
                  <X className="size-2.5" />
                </button>
              </span>
            ))}

            {adding ? (
              <input
                autoFocus
                value={draft}
                onChange={(e) => setDraft(e.target.value)}
                onKeyDown={commitTag}
                onBlur={() => {
                  setDraft('')
                  setAdding(false)
                }}
                placeholder="标签名，回车确认"
                className="h-6 w-28 rounded-full border border-brand-500/50 bg-white/70 px-2 text-[11px] outline-none dark:bg-white/10 dark:text-white/85"
              />
            ) : (
              <button
                onClick={() => setAdding(true)}
                className="inline-flex h-6 items-center gap-0.5 rounded-full border border-dashed border-black/15 px-2 text-[11px] text-black/40 transition hover:border-brand-500/50 hover:text-brand-600 dark:border-white/15 dark:text-white/40 dark:hover:text-brand-400"
              >
                <Plus className="size-3" />
                标签
              </button>
            )}
          </div>

          {/* 备注：自由文本，纳入全文搜索 */}
          <div className="px-3.5 pt-2.5">
            <textarea
              key={item.id}
              defaultValue={item.note ?? ''}
              placeholder="备注（可搜索）…"
              rows={2}
              onKeyDown={(e) => {
                // 备注>里打字不应触发列表导航；Escape 保持原有「收起」语义
                if (e.key !== 'Escape') e.stopPropagation()
              }}
              onBlur={(e) => {
                const value = e.target.value.trim()
                if (value !== (item.note ?? '')) onSetNote(item.id, value || null)
              }}
              className="w-full resize-none rounded-lg border border-black/8 bg-black/[0.025] px-2 py-1.5 text-[11.5px] leading-4 text-black/70 outline-none transition focus:border-brand-500/50 dark:border-white/10 dark:bg-white/[0.045] dark:text-white/72"
            />
          </div>

          {/* 分组与条目热键 */}
          <div className="flex items-center gap-1.5 px-3.5 pt-2.5">
            <select
              value={item.groupId ?? ''}
              onChange={(e) =>
                onSetGroup(item.id, e.target.value === '' ? null : Number(e.target.value))
              }
              className="h-6.5 min-w-0 flex-1 rounded-md border border-black/10 bg-black/[0.03] px-1.5 text-[11px] text-black/60 outline-none dark:border-white/12 dark:bg-white/8 dark:text-white/62"
            >
              <option value="">未分组</option>
              {groupOptions(groups).map((group) => (
                <option key={group.id} value={group.id}>
                  {'　'.repeat(group.depth)}
                  {group.name}
                </option>
              ))}
            </select>
            <HotkeySetter current={item.hotkey ?? null} onCommit={(value) => onSetHotkey(item.id, value)} />
          </div>

          {/* 粘贴变换：只对文本条目有意义；无字母文本禁用大小写类 */}
          {item.kind === 'text' && (
            <div className="flex flex-wrap gap-1 px-3.5 pt-2.5">
              {TRANSFORMS.map(([transform, label, needsLetters]) => {
                const disabled = needsLetters && !/[a-zA-Z]/.test(item.text ?? '')
                return (
                  <button
                    key={transform}
                    disabled={disabled}
                    onClick={() => onPasteTransformed(item.id, transform)}
                    title={`粘贴为${label}`}
                    className="rounded-full border border-black/10 px-2 py-0.5 text-[10.5px] text-black/55 transition hover:border-brand-500/50 hover:text-brand-600 disabled:opacity-35 disabled:hover:border-black/10 disabled:hover:text-black/55 dark:border-white/12 dark:text-white/55 dark:hover:text-brand-400"
                  >
                    {label}
                  </button>
                )
              })}
            </div>
          )}

          {/* 来源与时间 */}
          <div className="px-3.5 pt-2.5 text-[10.5px] leading-4 text-black/35 dark:text-white/35">
            <div>{absoluteTime(item.createdAt)} 复制</div>
            {item.sourceApp && <div>来自 {item.sourceApp}</div>}
            {item.useCount > 0 && <div>已使用 {item.useCount} 次</div>}
          </div>
        </motion.div>
      </AnimatePresence>

      {/* 操作区 */}
      <div className="flex items-center gap-1.5 p-3">
        <button
          onClick={() => onPaste(item.id)}
          className="flex h-8 flex-1 items-center justify-center gap-1.5 rounded-lg bg-brand-500 text-[12px] font-medium text-white shadow-sm shadow-brand-500/30 transition hover:bg-brand-600 active:scale-[0.98]"
        >
          <ClipboardPaste className="size-3.5" />
          粘贴
        </button>
        {(
          [
            [Copy, '复制到剪贴板 (Ctrl+C)', () => onCopy(item.id), ''],
            [Pin, item.pinned ? '取消置顶 (Ctrl+P)' : '置顶 (Ctrl+P)', () => onTogglePin(item.id), ''],
            [Trash2, '删除 (Del)', () => onRemove(item.id), 'hover:bg-red-500/90 hover:text-white'],
          ] as const
        ).map(([Icon, title, action, extra], i) => (
          <button
            key={i}
            title={title}
            onClick={action}
            className={`flex size-8 items-center justify-center rounded-lg bg-black/5 text-black/55 transition hover:bg-black/10 hover:text-black/80 dark:bg-white/8 dark:text-white/55 dark:hover:bg-white/14 dark:hover:text-white/90 ${extra}`}
          >
            <Icon className="size-3.5" />
          </button>
        ))}
      </div>
    </div>
  )
}
