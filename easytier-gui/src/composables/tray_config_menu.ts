/**
 * Tray「网络配置」子菜单的内容模型。
 *
 * 纯函数、无 Tauri / i18n 依赖：托盘菜单的可测部分全在这里，`tray.ts` 只负责把
 * 结果翻译成 muda 菜单项。这样「排序 / 重名消歧 / 截断 / 运行中优先」都能直接
 * 单测，而不用起真机。
 *
 * 语义约定（与托盘图标 `tray_run_state.ts` 保持一致）：
 * - `running` 来自设备实例表 `running_inst_ids`（内核实例表全量，含启动失败后仍留在
 *   表里的实例）。所以「启动失败」在这里仍显示为 ●，点一下是停止 —— 这是有意为之：
 *   失败实例占着实例表，先停掉才能重新启动，两步操作就是恢复路径。
 * - 名字可能为空（meta 拿不到 / 老后端），统一回落到调用方给的占位文案。
 */

/**
 * 子菜单里最多列多少条。
 *
 * Windows 的托盘菜单是原生 Win32 弹出菜单，条目过多会超出屏幕高度，Windows 只能
 * 分列/滚动，几乎不可用；同时托盘刷新是 30s 级别的，长列表也不值得每次重建。
 */
export const TRAY_CONFIG_MENU_MAX = 12

/** 单条标签的最大字符数（超出截断 + `…`）。托盘菜单没有 tooltip，只能靠截断。 */
export const TRAY_CONFIG_LABEL_MAX = 32

/** 一条待展示的配置。 */
export interface TrayConfigMenuSource {
  instanceId: string
  /** 设备 meta 的 `network_name`，拿不到时回落到本地缓存里的名字。 */
  name?: string
  /** true = 在设备实例表里（点击 = 停止）；false/缺省 = 可启动（点击 = 启动）。 */
  running?: boolean
}

export interface TrayConfigMenuEntry {
  instanceId: string
  /** 已消歧、已截断的展示名，非空。 */
  label: string
  running: boolean
}

export interface TrayConfigMenuModel {
  /** 实际要渲染的条目（≤ `max`，已排序）。 */
  entries: TrayConfigMenuEntry[]
  /** 去重后的配置总数（含被截断的）。 */
  total: number
  /** 被 `max` 截掉的条数；> 0 时渲染一行「还有 N 个未列出」。 */
  hidden: number
  /** 运行中的数量（子菜单标题用）。 */
  runningCount: number
}

export interface TrayConfigMenuInput {
  entries: TrayConfigMenuSource[]
  /** 空名字的占位文案（i18n 由调用方注入）。 */
  untitledLabel: string
  /** ≤ 0 表示不限制（测试 / 单配置场景用）。 */
  max?: number
  maxLabelLength?: number
}

function shortId(instanceId: string): string {
  const compact = instanceId.replace(/-/g, '')
  return (compact || instanceId).slice(0, 4)
}

function truncate(text: string, maxChars: number): string {
  if (maxChars <= 0 || text.length <= maxChars)
    return text
  return `${text.slice(0, Math.max(1, maxChars - 1))}…`
}

/**
 * 把设备实例表 + 名字拼成菜单模型。
 *
 * 排序规则：运行中的排前面（托盘最常见的动作是「停掉正在跑的那个」），同组内按
 * 名字排序。截断在排序之后做，所以优先保住运行中的条目。
 */
export function buildTrayConfigMenu(input: TrayConfigMenuInput): TrayConfigMenuModel {
  const max = input.max ?? TRAY_CONFIG_MENU_MAX
  const maxLabelLength = input.maxLabelLength ?? TRAY_CONFIG_LABEL_MAX

  // 同一个 id 可能同时出现在 running / disabled 两个列表里（防御性去重）。
  const seen = new Map<string, { instanceId: string, name: string, running: boolean }>()
  for (const src of input.entries) {
    const instanceId = typeof src?.instanceId === 'string' ? src.instanceId.trim() : ''
    if (!instanceId)
      continue
    const name = (src.name ?? '').trim() || input.untitledLabel
    const running = src.running === true
    const prev = seen.get(instanceId)
    // running 取「或」：只要任一处说在跑就按在跑算（宁可按在跑展示，也不给一条点了
    // 会失败的「启动」）。
    seen.set(instanceId, { instanceId, name, running: running || (prev?.running ?? false) })
  }

  const rows = [...seen.values()]

  // ET 允许两个配置同名，而托盘菜单没有第二列信息，只能追加短 id 区分。
  const nameCount = new Map<string, number>()
  for (const row of rows)
    nameCount.set(row.name, (nameCount.get(row.name) ?? 0) + 1)

  const entries: TrayConfigMenuEntry[] = rows.map((row) => {
    const duplicated = (nameCount.get(row.name) ?? 0) > 1
    const base = duplicated ? `${row.name} · ${shortId(row.instanceId)}` : row.name
    return {
      instanceId: row.instanceId,
      label: truncate(base, maxLabelLength),
      running: row.running,
    }
  })

  entries.sort((a, b) => {
    if (a.running !== b.running)
      return a.running ? -1 : 1
    return a.label.localeCompare(b.label, undefined, { numeric: true })
  })

  const runningCount = entries.filter(e => e.running).length
  const limited = max > 0 ? entries.slice(0, max) : entries

  return {
    entries: limited,
    total: entries.length,
    hidden: entries.length - limited.length,
    runningCount,
  }
}

/**
 * 「这个托盘动作是不是已经无需执行」。
 *
 * 菜单行是**快照**：用户点下去之前，配置可能已经在窗口里 / 控制台侧被启动或停止了。
 * 而真正的「启动」走 `run_network_instance(overwrite = true)`
 * （`easytier-core/src/management/full/remote_client.rs`），对已经在跑的实例等于
 * **先停后起** —— 拆掉 TUN / WinTun 再重建，隧道重连（见
 * `docs/roadmap/connection-stability-todo.md` P-WIN.1）。所以「点了也白点」必须在
 * 发 RPC 之前挡掉，不能指望服务端幂等。
 *
 * 判据（`inInstanceTable` 来自 `running_inst_ids`，它是**内核实例表全量**）：
 * - `stop`：不在表里 = 已经是停的。
 * - `start`：不在表里 = 该起；在表里还得看是否真的在跑 —— 启动失败 / 被取消的实例
 *   仍留在表里，此时「启动」正是恢复路径，不能当空操作跳过。
 * - `reallyRunning` 为 `undefined`（探针失败 / 表里没有）：按**在跑**算。理由是不对称：
 *   重复启动的代价（拆网卡 + 重连）远高于漏一次启动（用户再点一下）。
 */
export function trayConfigActionIsNoop(
  action: 'start' | 'stop',
  inInstanceTable: boolean,
  reallyRunning: boolean | undefined = undefined,
): boolean {
  if (action === 'stop')
    return !inInstanceTable
  if (!inInstanceTable)
    return false
  return reallyRunning !== false
}

/**
 * 菜单内容指纹。菜单重建有成本（muda 每次新建一棵树），30s 轮询不该触发；只有
 * 「配置集合 / 运行态 / 名字」真的变了（窗口里启停、改名、控制台下发）才重建。
 *
 * 不含 i18n 文案：切语言已经由 `index.vue` 的 locale watch 单独重建。
 */
export function trayConfigMenuKey(entries: TrayConfigMenuSource[]): string {
  return entries
    .map(e => `${e.instanceId}:${e.running === true ? 1 : 0}:${e.name ?? ''}`)
    .sort()
    .join('\u0000')
}
