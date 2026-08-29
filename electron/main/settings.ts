import { app } from 'electron'
import { readFileSync, writeFileSync, mkdirSync } from 'node:fs'
import { join, dirname } from 'node:path'
import type { FilterId, Settings } from '@shared/types'

const DEFAULTS: Settings = {
  hotkey: 'Alt+V',
  quickPasteModifiers: 'Ctrl+Alt',
  maxItems: 2000,
  maxDays: 30,
  skipSensitive: true,
  // 常见密码管理器：命中进程名就不入库
  sensitiveApps: [
    'keepass',
    '1password',
    'bitwarden',
    'lastpass',
    'enpass',
    'keeweb',
    'dashlane',
    'nordpass',
  ],
  hideAfterPaste: true,
  trayOpensMini: true,
  visibleFilters: ['all', 'text', 'image', 'files', 'url', 'key'],
  autoLaunch: false,
  theme: 'system',
  accent: 'violet',
  opacity: 90,
  skippedVersion: null,
}
const ACCENTS: Settings['accent'][] = ['violet', 'blue', 'cyan', 'teal', 'green', 'amber', 'rose']
const MAX_VISIBLE_FILTERS = 5

function normalizeOpacity(value: unknown): number {
  return typeof value === 'number' && Number.isFinite(value)
    ? Math.min(100, Math.max(20, Math.round(value)))
    : DEFAULTS.opacity
}

function normalizeVisibleFilters(value: unknown): Settings['visibleFilters'] {
  const ids = Array.isArray(value)
    ? value.filter((id): id is FilterId => typeof id === 'string')
    : []
  return ['all', ...ids.filter((id) => id !== 'all').slice(0, MAX_VISIBLE_FILTERS)]
}

let cache: Settings | null = null

function file(): string {
  return join(app.getPath('userData'), 'settings.json')
}

export function getSettings(): Settings {
  if (cache) return cache
  try {
    // 手工编辑过的文件可能带 UTF-8 BOM，JSON.parse 会直接抛
    const text = readFileSync(file(), 'utf8').replace(/^﻿/, '')
    const raw = JSON.parse(text) as Partial<Settings>
    cache = {
      ...DEFAULTS,
      ...raw,
      visibleFilters: normalizeVisibleFilters(
        Array.isArray(raw.visibleFilters) && raw.visibleFilters.length > 0
          ? raw.visibleFilters
          : DEFAULTS.visibleFilters,
      ),
      accent: raw.accent && ACCENTS.includes(raw.accent) ? raw.accent : DEFAULTS.accent,
      opacity: normalizeOpacity(raw.opacity),
    }
  } catch {
    cache = { ...DEFAULTS }
  }
  return cache
}

export function saveSettings(patch: Partial<Settings>): Settings {
  const current = getSettings()
  const next = {
    ...current,
    ...patch,
    opacity: normalizeOpacity(patch.opacity ?? current.opacity),
  }
  cache = next
  try {
    mkdirSync(dirname(file()), { recursive: true })
    writeFileSync(file(), JSON.stringify(next, null, 2), 'utf8')
  } catch (err) {
    console.error('[settings] 写入失败', err)
  }
  return next
}
