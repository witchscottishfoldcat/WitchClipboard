import { useState } from 'react'
import { Pin, Tag as TagIcon, FolderPlus, X, FolderInput } from 'lucide-react'
import type { AutoKind, FilterId, Group, ItemKind } from '@shared/types'
import { visibleKindFilters } from '@/lib/kinds'

interface Props {
  kind: ItemKind | null
  onKind: (k: ItemKind | null) => void
  autoKind: AutoKind | null
  onAutoKind: (k: AutoKind | null) => void
  tags: string[]
  activeTag: string | null
  onTag: (t: string | null) => void
  pinnedOnly: boolean
  onPinnedOnly: (v: boolean) => void
  visibleFilters: FilterId[]
  groups: Group[]
  /** null = 不过滤；0 = 未分组；其他 = 该分组（含子分组） */
  activeGroupId: number | null
  onGroup: (id: number | null) => void
  onCreateGroup: (name: string, parentId: number | null) => void
  onDeleteGroup: (id: number) => void
}

const chipBase =
  'inline-flex h-6.5 shrink-0 items-center gap-1 rounded-full px-2.5 text-[11.5px] font-medium transition select-none'
const chipOff =
  'bg-black/5 text-black/55 hover:bg-black/10 hover:text-black/80 dark:bg-white/7 dark:text-white/55 dark:hover:bg-white/12 dark:hover:text-white/85'
const chipOn = 'bg-brand-500 text-white shadow-sm shadow-brand-500/30'

export function FilterBar({
  kind,
  onKind,
  autoKind,
  onAutoKind,
  tags,
  activeTag,
  onTag,
  pinnedOnly,
  onPinnedOnly,
  visibleFilters,
  groups,
  activeGroupId,
  onGroup,
  onCreateGroup,
  onDeleteGroup,
}: Props) {
  const [creating, setCreating] = useState(false)
  const [draft, setDraft] = useState('')
  const depthOf = (group: Group, guard = 0): number => {
    if (!group.parentId || guard > 8) return 0
    const parent = groups.find((candidate) => candidate.id === group.parentId)
    return parent ? depthOf(parent, guard + 1) + 1 : 0
  }
  const commitCreate = (): void => {
    const name = draft.trim()
    if (name) onCreateGroup(name, activeGroupId && activeGroupId > 0 ? activeGroupId : null)
    setDraft('')
    setCreating(false)
  }

  return (
    <div className="flex items-center gap-1.5 overflow-x-auto px-3.5 pb-2.5 [scrollbar-width:none] [&::-webkit-scrollbar]:hidden">
      {visibleKindFilters(visibleFilters).map((f) => (
        <button
          key={f.id}
          onClick={() => {
            onKind(f.kind)
            onAutoKind(f.autoKind)
          }}
          className={`${chipBase} ${
            kind === f.kind && autoKind === f.autoKind ? chipOn : chipOff
          }`}
        >
          {f.label}
        </button>
      ))}

      <button
        onClick={() => onPinnedOnly(!pinnedOnly)}
        className={`${chipBase} ${pinnedOnly ? chipOn : chipOff}`}
      >
        <Pin className="size-3" strokeWidth={2.5} />
        置顶
      </button>

      {/* 分组：点选过滤（含子分组），长按 X 删除；选中分组时新建的是它的子分组 */}
      {groups.length > 0 && <span className="mx-0.5 h-4 w-px shrink-0 bg-black/10 dark:bg-white/12" />}

      {groups.map((group) => (
        <span
          key={group.id}
          className={`group/chip inline-flex h-6.5 shrink-0 items-center gap-1 rounded-full px-2.5 text-[11.5px] font-medium transition select-none ${
            activeGroupId === group.id ? chipOn : chipOff
          }`}
        >
          <button type="button" onClick={() => onGroup(activeGroupId === group.id ? null : group.id)}>
            {'\u00A0'.repeat(depthOf(group) * 2)}
            {group.name}
            <span className="ml-1 opacity-60 tabular-nums">{group.count}</span>
          </button>
          <button
            type="button"
            title="删除分组（条目回到未分组）"
            onClick={() => onDeleteGroup(group.id)}
            className="opacity-0 transition group-hover/chip:opacity-70 hover:!opacity-100"
          >
            <X className="size-2.5" />
          </button>
        </span>
      ))}

      <button
        onClick={() => onGroup(activeGroupId === 0 ? null : 0)}
        title="只看未分组的条目"
        className={`${chipBase} ${activeGroupId === 0 ? chipOn : chipOff}`}
      >
        <FolderInput className="size-3" strokeWidth={2.5} />
        未分组
      </button>

      {creating ? (
        <input
          autoFocus
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onBlur={commitCreate}
          onKeyDown={(e) => {
            e.stopPropagation()
            if (e.key === 'Enter') commitCreate()
            else if (e.key === 'Escape') {
              setDraft('')
              setCreating(false)
            }
          }}
          placeholder="分组名，回车确认"
          className="h-6.5 w-28 shrink-0 rounded-full border border-brand-500/50 bg-white/70 px-2.5 text-[11px] outline-none dark:bg-white/10 dark:text-white/85"
        />
      ) : (
        <button
          onClick={() => setCreating(true)}
          title={activeGroupId && activeGroupId > 0 ? '在当前分组下新建子分组' : '新建分组'}
          className={`${chipBase} ${chipOff} !px-2`}
        >
          <FolderPlus className="size-3" strokeWidth={2.5} />
        </button>
      )}

      {tags.length > 0 && <span className="mx-0.5 h-4 w-px shrink-0 bg-black/10 dark:bg-white/12" />}

      {tags.map((t) => (
        <button
          key={t}
          onClick={() => onTag(activeTag === t ? null : t)}
          className={`${chipBase} ${activeTag === t ? chipOn : chipOff}`}
        >
          <TagIcon className="size-3" strokeWidth={2.5} />
          {t}
        </button>
      ))}
    </div>
  )
}
