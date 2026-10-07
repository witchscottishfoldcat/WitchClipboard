import { join } from 'node:path'
import { app, Menu, Tray, nativeImage } from 'electron'
import { showPanel, hidePanel, toggleFromTray, markQuitting, type AnchorRect } from './window'
import { showMini, hideMini, toggleMiniFromTray } from './mini'
import { currentHotkey } from './shortcuts'
import { getSettings } from './settings'
import packageJson from '../../package.json'

let tray: Tray | null = null

export function createTray(): Tray {
  const iconCandidates = [
    join(app.getAppPath(), 'resources', 'tray.png'),
    join(process.cwd(), 'resources', 'tray.png'),
  ]
  const iconPath = iconCandidates.find((path) => !nativeImage.createFromPath(path).isEmpty())
  const image = iconPath ? nativeImage.createFromPath(iconPath) : nativeImage.createEmpty()
  if (image.isEmpty()) console.error(`[tray] 图标加载失败：${iconPath}`)
  tray = new Tray(image.isEmpty() ? nativeImage.createEmpty() : image)

  tray.setToolTip(
    `Witch Clipboard · 单击预览 · 双击完整面板 · ${currentHotkey() ?? '热键未注册'}`,
  )

  // 单击默认弹迷你预览面板；设置里可以改成直接开完整面板。
  // toggle 函数内部自带「刚被失焦收起」的冷却判断
  tray.on('click', (_event, bounds) => {
    const anchor = pickAnchor(bounds)
    const settings = getSettings()
    if (!settings.hotkeyOpensMini && settings.trayOpensMini) toggleMiniFromTray(anchor)
    else toggleFromTray(anchor)
  })

  // 双击托盘：直接开完整面板
  tray.on('double-click', (_event, bounds) => {
    hideMini()
    showPanel(pickAnchor(bounds))
  })

  const menu = Menu.buildFromTemplate([
    { label: '打开剪贴板', click: () => showPanel(trayBounds()) },
    { label: '迷你预览面板', click: () => showMini(trayBounds()) },
    { label: `快捷键：${currentHotkey() ?? '未注册'}`, enabled: false },
    {
      label: '全部收起',
      click: () => {
        hideMini()
        hidePanel()
      },
    },
    { type: 'separator' },
    { label: `Witch Clipboard v${packageJson.version}`, enabled: false },
    { type: 'separator' },
    {
      label: '退出',
      click: () => {
        markQuitting()
        app.quit()
      },
    },
  ])
  tray.setContextMenu(menu)

  // Win11 会把新出现的托盘图标收进「溢出」区域，这时 bounds 是 0，用户也看不见图标
  const bounds = tray.getBounds()
  if (bounds.width === 0) {
    console.warn('[tray] 图标可能被收进托盘溢出区，用户需要在任务栏设置里把它固定出来')
  } else if (!app.isPackaged) {
    console.log('[tray] bounds =', JSON.stringify(bounds))
  }

  return tray
}

export function updateTrayTooltip(text: string): void {
  tray?.setToolTip(text)
}

/** 托盘图标矩形；被 Win11 收进溢出区时宽高为 0，这时返回 undefined 让面板退回光标定位 */
function trayBounds(): AnchorRect | undefined {
  return pickAnchor(tray?.getBounds())
}

function pickAnchor(bounds?: AnchorRect): AnchorRect | undefined {
  if (bounds && bounds.width > 0) return bounds
  const own = tray?.getBounds()
  return own && own.width > 0 ? own : undefined
}
