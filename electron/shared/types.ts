/** 主进程 / 预加载 / 渲染进程 三方共用的类型契约 */

export type ItemKind = 'text' | 'image' | 'files'

/** 自动识别出的文本子类型，用于着色和快速筛选 */
export type AutoKind =
  | 'plain'
  | 'url'
  | 'key'
  | 'model'
  | 'code'
  | 'color'
  | 'path'
  | 'email'
  | 'number'

/** 顶部快速筛选栏可显示的内置分类 */
export type FilterId =
  | 'all'
  | 'text'
  | 'image'
  | 'files'
  | 'url'
  | 'key'
  | 'model'
  | 'code'
  | 'color'
  | 'path'
  | 'email'
  | 'number'

export type AccentPalette = 'violet' | 'blue' | 'cyan' | 'teal' | 'green' | 'amber' | 'rose'

export interface ClipItem {
  id: number
  kind: ItemKind
  /** 完整文本；image 类型为 null */
  text: string | null
  /** 富文本来源的 HTML；普通文本、图片和文件通常为 null。 */
  html?: string | null
  /** 列表用摘要（首行、已折叠空白） */
  preview: string
  /** 内容 sha256，去重用 */
  hash: string
  /** 图片缩略图，data URL */
  thumb: string | null
  width: number | null
  height: number | null
  bytes: number
  sourceApp: string | null
  autoKind: AutoKind
  tags: string[]
  pinned: boolean
  useCount: number
  createdAt: number
  lastUsedAt: number
  /** 自由备注；可搜索 */
  note?: string | null
  /** 条目级持久热键，例如 "Ctrl+Alt+0"；未绑定为 null */
  hotkey?: string | null
  /** 所属分组；0 不是合法值（保留给「未分组」过滤） */
  groupId?: number | null
}

/** 分组树节点；count 只统计直接挂在该节点上的条目 */
export interface Group {
  id: number
  parentId: number | null
  name: string
  count: number
}

/** 粘贴变换；大小写类只作用于字母文本 */
export type PasteTransform =
  | 'plainText'
  | 'upper'
  | 'lower'
  | 'capitalize'
  | 'sentence'
  | 'camel'
  | 'trim'

export interface ListQuery {
  /** 搜索关键词，空串表示不过滤 */
  q?: string
  /** 只看某个 kind */
  kind?: ItemKind | null
  /** 只看某个自动识别的文本子类型 */
  autoKind?: AutoKind | null
  /** 必须包含的标签 */
  tag?: string | null
  /** 只看置顶 */
  pinnedOnly?: boolean
  /** 分组过滤；0 表示未分组，undefined 表示不过滤 */
  groupId?: number | null
  limit?: number
  offset?: number
}

export interface ListResult {
  items: ClipItem[]
  total: number
}

export interface Stats {
  total: number
  pinned: number
  images: number
  bytes: number
}

export interface Settings {
  /** 面板全局热键 */
  hotkey: string
  /** 全局快粘的修饰键；数字键固定为 1…9 */
  quickPasteModifiers: string
  /** 最多保留条数，0 = 不限 */
  maxItems: number
  /** 最多保留天数，0 = 不限 */
  maxDays: number
  /** 跳过带「不要记录」标记的剪贴板，以及来自 sensitiveApps 的复制 */
  skipSensitive: boolean
  /** 敏感来源进程名片段，命中即不入库 */
  sensitiveApps: string[]
  /** 粘贴后自动隐藏面板 */
  hideAfterPaste: boolean
  /** 单击托盘弹迷你预览面板（false = 直接开完整面板） */
  trayOpensMini: boolean
  /** 顶部导航栏显示的快速筛选标签；all 始终显示 */
  visibleFilters: FilterId[]
  /** 开机自启（静默启动到托盘） */
  autoLaunch: boolean
  theme: 'system' | 'light' | 'dark'
  /** 主界面按钮、选中态和强调色使用的统一调色盘 */
  accent: AccentPalette
  /** 主面板与迷你面板的背景不透明度（20–100） */
  opacity: number
  /** 用户选了「暂不更新」的版本号，启动时不再提示它 */
  skippedVersion: string | null
}

export type UpdateState =
  | 'idle'
  | 'checking'
  | 'available'
  | 'none'
  | 'downloading'
  | 'ready'
  | 'error'
  /** 开发模式等不支持自动更新的场景 */
  | 'unsupported'

export interface UpdateStatus {
  state: UpdateState
  currentVersion: string
  /** 可更新到的版本号 */
  version?: string
  /** 发布说明（已去标签、截断） */
  notes?: string
  /** 下载进度 0~100 */
  percent?: number
  error?: string
}

/** 自动粘贴的结果；失败时界面提示「已复制，请手动 Ctrl+V」 */
export interface PasteOutcome {
  ok: boolean
  reason?:
    | 'no-native'
    | 'no-target'
    | 'focus-failed'
    | 'target-elevated'
    | 'send-failed'
    | 'not-found'
}

export interface SecurityInfo {
  /** 主密钥是否由操作系统保护（Windows DPAPI） */
  osProtected: boolean
  /** 数据库是否加密 */
  dbEncrypted: boolean
  /** 原生能力（剪贴板序列号、自动粘贴）是否可用 */
  nativeAvailable: boolean
  /** 是否降级到内存存储（重启会丢） */
  memoryFallback: boolean
  dataDir: string
}

export interface CrossDeviceStatus {
  running: boolean
  /** 手机扫码访问的局域网地址 */
  url: string | null
  /** 配对地址的短码，仅用于让用户核对当前会话 */
  pairCode: string | null
  /** 最近 5 秒内是否有手机保持连接 */
  connected: boolean
  lastSeenAt: number | null
  lastSentAt: number | null
  lastSentPreview: string | null
  pendingDevice?: CrossDeviceDevice | null
  approvedDevice?: CrossDeviceDevice | null
  transfers?: CrossDeviceTransfer[]
}

export interface CrossDeviceDevice {
  id: string
  name: string
}

export interface CrossDeviceTransfer {
  id: string
  name: string
  direction: 'upload' | 'download'
  state: 'pending' | 'transferring' | 'completed' | 'failed' | 'cancelled'
  bytesTransferred: number
  totalBytes: number
  error: string | null
}

export interface CrossDeviceSendResult {
  ok: boolean
  reason?: 'not-running' | 'not-approved' | 'not-found' | 'unsupported' | 'sensitive' | 'too-large'
}

export interface WebDavConfig {
  enabled: boolean
  url: string
  username: string
  hasPassword: boolean
  hasSyncKey: boolean
  keyFingerprint: string | null
}

export interface WebDavConfigPatch {
  enabled: boolean
  url: string
  username: string
  password?: string
  syncKey?: string
}

export interface WebDavSyncStatus {
  state: 'idle' | 'syncing' | 'error'
  lastSyncAt: number | null
  uploaded: number
  downloaded: number
  deleted: number
  error: string | null
}

/** contextBridge 暴露给渲染进程的全部能力 */
export interface ClipboardApi {
  list(query: ListQuery): Promise<ListResult>
  stats(): Promise<Stats>
  tags(): Promise<string[]>
  setTags(id: number, tags: string[]): Promise<void>
  togglePin(id: number): Promise<void>
  remove(id: number): Promise<void>
  clearAll(): Promise<void>
  /** 写入系统剪贴板，不改变焦点 */
  copy(id: number): Promise<void>
  /** 写入剪贴板 → 还原前台窗口 → 模拟 Ctrl+V */
  paste(id: number): Promise<PasteOutcome>
  /** 多选批量粘贴：按传入顺序逐条写入并粘贴 */
  pasteItems(ids: number[]): Promise<PasteOutcome>
  /** 后台热键粘贴失败时推送原因（面板可见时用于提示） */
  onPasteFailed(cb: (reason: string) => void): () => void
  /** 粘贴变换：对文本条目做大小写/去空白等变换后粘贴 */
  pasteTransformed(id: number, transform: PasteTransform): Promise<PasteOutcome>
  /** 保存条目自由备注（纳入搜索） */
  setItemNote(id: number, note: string | null): Promise<void>
  /** 绑定/解绑条目级持久热键；冲突返回错误码 */
  setItemHotkey(id: number, hotkey: string | null): Promise<void>
  groups(): Promise<Group[]>
  groupCreate(name: string, parentId: number | null): Promise<number>
  groupRename(id: number, name: string): Promise<void>
  groupDelete(id: number): Promise<void>
  itemSetGroup(id: number, groupId: number | null): Promise<void>
  /** 导出为明文 JSON（数据目录 exports/ 下），返回文件路径 */
  exportItems(ids?: number[]): Promise<string>
  importItems(path: string): Promise<string>
  /** 取全尺寸原图（data URL），非图片条目返回 null */
  imageDataUrl(id: number): Promise<string | null>
  /** 查找同一复制会话中关联的 Key、URL 和模型名称 */
  relatedItems(id: number, limit?: number): Promise<ClipItem[]>
  hidePanel(): Promise<void>
  /** 从迷你面板切到完整面板 */
  expandPanel(): Promise<void>
  /** 在资源管理器里定位文件条目 */
  revealFile(id: number): Promise<void>
  getSettings(): Promise<Settings>
  saveSettings(patch: Partial<Settings>): Promise<Settings>
  security(): Promise<SecurityInfo>

  /** 启动同一局域网内的手机配对服务 */
  startCrossDevice(): Promise<CrossDeviceStatus>
  stopCrossDevice(): Promise<CrossDeviceStatus>
  crossDeviceStatus(): Promise<CrossDeviceStatus>
  /** 手动把指定历史条目发送到已配对手机 */
  sendCrossDeviceItem(id: number): Promise<CrossDeviceSendResult>
  approveCrossDevice(deviceId: string): Promise<CrossDeviceStatus>
  rejectCrossDevice(deviceId: string): Promise<CrossDeviceStatus>
  cancelCrossDeviceTransfer(transferId: string): Promise<CrossDeviceStatus>
  retryCrossDeviceTransfer(transferId: string): Promise<CrossDeviceStatus>

  webDavConfig(): Promise<WebDavConfig>
  saveWebDavConfig(patch: WebDavConfigPatch): Promise<WebDavConfig>
  copyWebDavSyncKey(): Promise<void>
  webDavStatus(): Promise<WebDavSyncStatus>
  syncWebDavNow(): Promise<WebDavSyncStatus>

  /** 手动检查更新 */
  checkUpdate(): Promise<UpdateStatus>
  /** 用户同意后才下载 */
  downloadUpdate(): Promise<UpdateStatus>
  /** 重启并安装已下载的更新 */
  installUpdate(): Promise<void>
  /** 暂不更新：记住这个版本，下次启动不再提示 */
  skipUpdate(version?: string): Promise<UpdateStatus>
  updateStatus(): Promise<UpdateStatus>
  /** 更新状态变化（检查中、有更新、下载进度、下载完成…） */
  onUpdateStatus(cb: (status: UpdateStatus) => void): () => void
  /** 在资源管理器里打开数据目录 */
  openDataDir(): Promise<void>
  /** 库有变化时触发，返回取消订阅函数 */
  onChanged(cb: () => void): () => void
  /** 面板显示时触发（用于重置选中项 / 聚焦搜索框） */
  onPanelShown(cb: () => void): () => void
}
