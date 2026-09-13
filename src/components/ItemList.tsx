import { useEffect, useRef, type MouseEvent } from 'react'
import { useVirtualizer } from '@tanstack/react-virtual'
import { ClipboardList } from 'lucide-react'
import type { ClipItem } from '@shared/types'
import { ItemRow } from './ItemRow'

interface Props {
  items: ClipItem[]
  selectedId: number | null
  multiSelectedIds: Set<number>
  onSelect: (id: number, e: MouseEvent) => void
  onPaste: (id: number) => void
  onTogglePin: (id: number) => void
  loading: boolean
  /** 库本身是空的（而不是被筛选条件筛空了） */
  libraryEmpty: boolean
  hotkey: string
}

const ROW = 72 // 68px 行高 + 4px 间隔
/** 给首行圆角和选中 ring 留空间，避免贴着滚动容器顶边被裁掉 */
const LIST_TOP_GAP = 5

export function ItemList({
  items,
  selectedId,
  multiSelectedIds,
  onSelect,
  onPaste,
  onTogglePin,
  loading,
  libraryEmpty,
  hotkey,
}: Props) {
  const scrollRef = useRef<HTMLDivElement>(null)

  const virt = useVirtualizer({
    count: items.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ROW,
    overscan: 6,
    scrollPaddingStart: LIST_TOP_GAP,
  })

  // 键盘移动选中项时保证可见。
  // 只在选中项真的变了才滚动——虚拟列表每次滚动都会重渲染，
  // 无条件调用会把用户用滚轮浏览的位置强行拽回选中项。
  const index = items.findIndex((it) => it.id === selectedId)
  const lastIndex = useRef(-1)
  useEffect(() => {
    if (index >= 0 && index !== lastIndex.current) {
      lastIndex.current = index
      virt.scrollToIndex(index, { align: 'auto' })
    }
  }, [index, virt])

  if (!loading && items.length === 0) {
    return (
      <div className="flex flex-1 flex-col items-center justify-center gap-3 px-6 text-center">
        <div className="flex size-14 items-center justify-center rounded-2xl bg-black/4 dark:bg-white/6">
          <ClipboardList className="size-6 text-black/25 dark:text-white/25" />
        </div>
        {libraryEmpty ? (
          <>
            <div className="text-[13px] text-black/45 dark:text-white/45">还没有记录</div>
            <div className="text-[11px] leading-5 text-black/30 dark:text-white/30">
              复制任何文字、图片或文件，Witch Clipboard 会自动收进来。
              <br />
              之后按{' '}
              <kbd className="rounded border border-black/10 bg-black/4 px-1 font-sans dark:border-white/12 dark:bg-white/8">
                {hotkey}
              </kbd>{' '}
              随时唤出这个面板。
            </div>
          </>
        ) : (
          <>
            <div className="text-[13px] text-black/45 dark:text-white/45">没有匹配的记录</div>
            <div className="text-[11px] text-black/30 dark:text-white/30">
              换个关键词，或清掉上面的筛选条件
            </div>
          </>
        )}
      </div>
    )
  }

  return (
    <div ref={scrollRef} className="flex-1 overflow-y-auto px-2 pb-2">
      <div
        className="relative w-full"
        style={{ height: virt.getTotalSize() + LIST_TOP_GAP }}
      >
        {virt.getVirtualItems().map((row) => {
          const item = items[row.index]
          return (
            <div
              key={item.id}
              className="absolute top-0 left-0 w-full pb-1"
              style={{
                height: ROW,
                transform: `translateY(${row.start + LIST_TOP_GAP}px)`,
              }}
            >
              <ItemRow
                item={item}
                selected={item.id === selectedId}
                multiSelected={multiSelectedIds.has(item.id)}
                hotIndex={row.index < 9 ? row.index + 1 : null}
                onSelect={(e) => onSelect(item.id, e)}
                onPaste={() => onPaste(item.id)}
                onTogglePin={() => onTogglePin(item.id)}
              />
            </div>
          )
        })}
      </div>
    </div>
  )
}
