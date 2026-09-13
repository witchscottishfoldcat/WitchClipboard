import { useEffect, useRef, useState } from 'react'
import { Download, Upload, FileJson } from 'lucide-react'
import { listen } from '@tauri-apps/api/event'
import { api, isDesktop } from '@/lib/api'

/**
 * 历史导入导出。导出文件是明文 JSON（含剪贴板内容本身），落盘即脱离加密保护，
 * 所以导出必须显式确认；导入通过把文件拖到导入区完成（Tauri 拦截系统拖放并
 * 以 tauri://drag-drop 事件提供路径，避免为文件对话框引入新依赖）。
 */
export function ImportExportSection({
  onToast,
  onCleared,
}: {
  onToast: (text: string, tone?: 'ok' | 'warn') => void
  onCleared: () => void
}) {
  const [confirmExport, setConfirmExport] = useState(false)
  const [droppedPath, setDroppedPath] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const dropRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    if (!isDesktop) return
    const unsubscribe = listen<{ paths: string[] }>('tauri://drag-drop', (event) => {
      const path = event.payload.paths?.[0]
      if (!path || !path.toLowerCase().endsWith('.json')) return
      // 只认落在导入区上的拖放
      if (dropRef.current && !dropRef.current.matches(':hover')) return
      setDroppedPath(path)
    })
    return () => void unsubscribe.then((stop) => stop())
  }, [])

  const doExport = (): void => {
    setBusy(true)
    void api
      .exportItems()
      .then((path) => {
        setConfirmExport(false)
        onToast(`已导出到 ${path}`)
      })
      .catch(() => onToast('导出失败', 'warn'))
      .finally(() => setBusy(false))
  }

  const doImport = (): void => {
    if (!droppedPath) return
    setBusy(true)
    void api
      .importItems(droppedPath)
      .then((summary) => {
        setDroppedPath(null)
        onToast(summary)
        onCleared()
      })
      .catch((error: unknown) => {
        const reason = typeof error === 'string' ? error : String(error)
        onToast(reason.includes('format') ? '不是有效的导出文件' : '导入失败', 'warn')
      })
      .finally(() => setBusy(false))
  }

  return (
    <div className="space-y-1.5">
      <button
        disabled={busy}
        onClick={() => {
          if (!confirmExport) {
            setConfirmExport(true)
            return
          }
          doExport()
        }}
        className={`flex h-8 w-full items-center justify-center gap-1.5 rounded-lg text-[11.5px] transition ${
          confirmExport
            ? 'bg-brand-500 text-white hover:bg-brand-600'
            : 'bg-black/5 text-black/60 hover:bg-black/10 dark:bg-white/8 dark:text-white/60 dark:hover:bg-white/14'
        }`}
      >
        <Download className="size-3.5" />
        {confirmExport ? '导出为明文 JSON，确认？' : '导出全部历史'}
      </button>

      <div
        ref={dropRef}
        className={`flex h-14 w-full flex-col items-center justify-center gap-0.5 rounded-lg border border-dashed text-[10.5px] transition ${
          droppedPath
            ? 'border-brand-500/60 bg-brand-500/8 text-brand-600 dark:text-brand-400'
            : 'border-black/15 text-black/40 dark:border-white/15 dark:text-white/40'
        }`}
      >
        {droppedPath ? (
          <>
            <span className="flex items-center gap-1 font-medium">
              <FileJson className="size-3" />
              {droppedPath.split(/[\\/]/).pop()}
            </span>
            <span className="flex gap-2">
              <button disabled={busy} onClick={doImport} className="underline hover:text-brand-600">
                导入
              </button>
              <button
                disabled={busy}
                onClick={() => setDroppedPath(null)}
                className="underline opacity-60"
              >
                取消
              </button>
            </span>
          </>
        ) : (
          <>
            <Upload className="size-3.5" />
            把导出的 .json 文件拖到这里导入
          </>
        )}
      </div>

      <div className="text-[10px] leading-4 text-black/30 dark:text-white/30">
        导出文件包含明文剪贴板内容（含图片），请自行妥善保管；重复内容按哈希去重跳过。
      </div>
    </div>
  )
}
