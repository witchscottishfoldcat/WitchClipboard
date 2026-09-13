import { useCallback, useEffect, useRef, useState } from 'react'
import type { ClipItem, ListQuery, Stats } from '@shared/types'
import { api } from '@/lib/api'

interface ItemsState {
  items: ClipItem[]
  total: number
  loading: boolean
}

/**
 * 订阅库变更事件并防抖刷新：跨设备批量同步、连续复制会在短时间内触发一串
 * witchcat://changed，逐个响应等于每个事件做全量重查，合并成一次即可。
 */
function useDebouncedChanged(reload: () => void): void {
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null)

  useEffect(() => {
    const schedule = (): void => {
      if (timer.current) clearTimeout(timer.current)
      timer.current = setTimeout(reload, 80)
    }
    const stop = api.onChanged(schedule)
    return () => {
      stop()
      if (timer.current) clearTimeout(timer.current)
    }
  }, [reload])
}

/** 拉取列表：query 变化时防抖重查，库变更时自动刷新 */
export function useItems(query: ListQuery): ItemsState & { refetch: () => void } {
  const [state, setState] = useState<ItemsState>({ items: [], total: 0, loading: true })
  const key = JSON.stringify(query)
  const seq = useRef(0)

  const fetch = useCallback(async () => {
    const my = ++seq.current
    const res = await api.list(JSON.parse(key) as ListQuery)
    // 丢弃过期响应，避免快速输入时结果乱序
    if (my === seq.current) setState({ items: res.items, total: res.total, loading: false })
  }, [key])

  useEffect(() => {
    const t = setTimeout(() => void fetch(), 90)
    return () => clearTimeout(t)
  }, [fetch])

  useDebouncedChanged(() => void fetch())

  return { ...state, refetch: () => void fetch() }
}

export function useStats(): Stats | null {
  const [stats, setStats] = useState<Stats | null>(null)

  const load = useCallback(async () => setStats(await api.stats()), [])

  useEffect(() => {
    void load()
  }, [load])
  useDebouncedChanged(() => void load())

  return stats
}

export function useTags(): string[] {
  const [tags, setTags] = useState<string[]>([])

  const load = useCallback(async () => setTags(await api.tags()), [])

  useEffect(() => {
    void load()
  }, [load])
  useDebouncedChanged(() => void load())

  return tags
}
