import { memo, type MouseEvent } from 'react'
import { Pin, Image as ImageIcon, CheckCircle2 } from 'lucide-react'
import type { ClipItem } from '@shared/types'
import { badgeOf, colorValue } from '@/lib/kinds'
import { relativeTime } from '@/lib/format'

interface Props {
  item: ClipItem
  selected: boolean
  /** 处于多选集合中（Ctrl/Shift 选择） */
  multiSelected: boolean
  /** 1..9，用于全局数字快粘；超出范围为 null */
  hotIndex: number | null
  /** 携带鼠标事件，供调用方判断 Ctrl/Shift 修饰键 */
  onSelect: (e: MouseEvent) => void
  onPaste: () => void
  onTogglePin: () => void
}

export const ItemRow = memo(function ItemRow({
  item,
  selected,
  multiSelected,
  hotIndex,
  onSelect,
  onPaste,
  onTogglePin,
}: Props) {
  const badge = badgeOf(item)
  const color = colorValue(item)
  const isCode = item.kind === 'text' && item.autoKind === 'code'

  return (
    <div
      onMouseDown={onSelect}
      onDoubleClick={onPaste}
      className={[
        'group relative flex h-[68px] cursor-default items-center gap-3 overflow-hidden rounded-xl px-3 transition-colors',
        selected
          ? 'bg-brand-500/12 ring-1 ring-brand-500/35 dark:bg-brand-500/18'
          : 'hover:bg-black/4 dark:hover:bg-white/6',
        multiSelected ? 'ring-1 ring-brand-500/55 bg-brand-500/8 dark:bg-brand-500/14' : '',
      ].join(' ')}
    >
      {/* 左侧类型色条 */}
      <span
        className={`absolute top-1/2 left-0 h-8 w-[3px] -translate-y-1/2 rounded-r-full ${badge.bar} ${
          selected ? 'opacity-100' : 'opacity-0 group-hover:opacity-60'
        } transition-opacity`}
      />

      {/* 缩略图 / 颜色块 */}
      {item.kind === 'image' ? (
        item.thumb ? (
          <img
            src={item.thumb}
            alt=""
            draggable={false}
            className="size-11 shrink-0 rounded-lg border border-black/8 object-cover dark:border-white/10"
          />
        ) : (
          <div className="flex size-11 shrink-0 items-center justify-center rounded-lg bg-violet-500/12 text-violet-500">
            <ImageIcon className="size-5" />
          </div>
        )
      ) : color ? (
        <div
          className="size-11 shrink-0 rounded-lg border border-black/10 shadow-inner dark:border-white/15"
          style={{ background: color }}
        />
      ) : null}

      {/* 正文 */}
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-1.5">
          <span
            className={`inline-flex h-4.5 items-center rounded px-1.5 text-[10px] font-semibold tracking-wide ${badge.chip}`}
          >
            {badge.label}
          </span>
          {item.pinned && (
            <Pin className="size-3 shrink-0 fill-brand-500 text-brand-500" strokeWidth={2} />
          )}
          {item.tags.map((t) => (
            <span
              key={t}
              className="truncate rounded bg-black/6 px-1.5 text-[10px] text-black/50 dark:bg-white/10 dark:text-white/50"
            >
              {t}
            </span>
          ))}
          <span className="ml-auto shrink-0 pl-2 text-[10.5px] text-black/35 tabular-nums dark:text-white/35">
            {relativeTime(item.lastUsedAt)}
          </span>
        </div>

        <div
          className={[
            'mt-1 truncate text-[13px] leading-5',
            isCode ? 'font-mono text-[12px]' : '',
            selected ? 'text-black/90 dark:text-white/92' : 'text-black/70 dark:text-white/72',
          ].join(' ')}
        >
          {item.preview || <span className="italic opacity-50">（空白内容）</span>}
        </div>

        {item.sourceApp && (
          <div className="truncate text-[10.5px] text-black/32 dark:text-white/32">
            {item.sourceApp}
          </div>
        )}
      </div>

      {/* 右侧操作：hover / 选中时出现 */}
      <div
        className={`flex shrink-0 items-center gap-1 transition-opacity ${
          selected ? 'opacity-100' : 'opacity-0 group-hover:opacity-100'
        }`}
      >
        <button
          onMouseDown={(e) => {
            e.stopPropagation()
            onTogglePin()
          }}
          title={item.pinned ? '取消置顶 (Ctrl+P)' : '置顶 (Ctrl+P)'}
          className="flex size-6.5 items-center justify-center rounded-md text-black/40 transition hover:bg-black/8 hover:text-brand-600 dark:text-white/40 dark:hover:bg-white/12 dark:hover:text-brand-400"
        >
          <Pin className={`size-3.5 ${item.pinned ? 'fill-current' : ''}`} />
        </button>
      </div>

      {multiSelected && (
        <CheckCircle2 className="size-4 shrink-0 fill-brand-500 text-white" strokeWidth={2} />
      )}

      {/* 全局数字快粘角标 */}
      {hotIndex !== null && (
        <span
          className={`flex size-5 shrink-0 items-center justify-center rounded-md border text-[10px] font-semibold tabular-nums transition ${
            selected
              ? 'border-brand-500/40 bg-brand-500/20 text-brand-600 dark:text-brand-400'
              : 'border-black/8 bg-black/4 text-black/35 dark:border-white/10 dark:bg-white/6 dark:text-white/35'
          }`}
        >
          {hotIndex}
        </span>
      )}
    </div>
  )
})
