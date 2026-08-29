import { useEffect, useState } from 'react'
import type { Settings } from '@shared/types'
import { api } from '@/lib/api'
import { applyAccent } from '@/lib/accent'

/** 把 settings.theme 落到 <html data-theme>，system 时跟随系统 */
export function useTheme(): 'light' | 'dark' {
  const [pref, setPref] = useState<Settings['theme']>('system')
  const [resolved, setResolved] = useState<'light' | 'dark'>('dark')

  useEffect(() => {
    let disposed = false
    const sync = (): void => {
      void api.getSettings().then((s) => {
        if (disposed) return
        setPref(s.theme)
        applyAccent(s.accent)
      })
    }

    sync()
    const unsubscribe = api.onChanged(sync)
    return () => {
      disposed = true
      unsubscribe()
    }
  }, [])

  useEffect(() => {
    const mq = window.matchMedia('(prefers-color-scheme: dark)')
    const apply = (): void => {
      const next = pref === 'system' ? (mq.matches ? 'dark' : 'light') : pref
      document.documentElement.dataset['theme'] = next
      setResolved(next)
    }
    apply()
    mq.addEventListener('change', apply)
    return () => mq.removeEventListener('change', apply)
  }, [pref])

  return resolved
}
