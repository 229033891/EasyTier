<script setup lang="ts">
import { NetworkInstance, VpnPortalClientState, type TunnelInfo, type NodeInfo, type PeerInfo, type PeerRoutePair, type VpnPortalClientInfo, type VpnPortalInfo } from '../types/network'
import type { RemoteClient } from '../modules/api'
import { useI18n } from 'vue-i18n';
import { computed, onMounted, onUnmounted, reactive, ref, watch } from 'vue';
import { ipv4InetToString, ipv4ToString, ipv6ToString, formatEventTime } from '../modules/utils';
import { isPanelHeaderInteractiveTarget } from '../modules/panel';
import { isMeaningfulProxyCidrRouteSync, jitterMs, latencyMs, lossRate, numericValue, pathQualityCell, pathQualityTip, peerConns, resolvePeerRemoteAddr, resolveRoutePath, type RoutePeerLabel } from '../modules/statusDisplay';
import { Badge, DataTable, Column, Tag, Button, ScrollPanel, Timeline, Card, Panel, } from 'primevue';
import NetworkChart from './NetworkChart.vue';
import PeerConnHistoryChart from './PeerConnHistoryChart.vue';

const props = defineProps<{
  curNetworkInst: NetworkInstance | null,
  api: RemoteClient,
}>()

const { t } = useI18n()

const peerRouteInfos = computed(() => {
  if (props.curNetworkInst) {
    const my_node_info = props.curNetworkInst.detail?.my_node_info
    return [{
      route: {
        ipv4_addr: my_node_info?.virtual_ipv4,
        hostname: my_node_info?.hostname,
        version: my_node_info?.version,
        stun_info: my_node_info?.stun_info
      },
    }, ...(props.curNetworkInst.detail?.peer_route_pairs || [])]
  }

  return []
})

/** peer_id → 带直连隧道的 PeerInfo，供中转下一跳查隧道地址 */
const peersById = computed(() => {
  const map = new Map<number, PeerInfo>()
  for (const pair of props.curNetworkInst?.detail?.peer_route_pairs || []) {
    const peer = pair.peer
    if (peer?.peer_id && peer.conns?.length)
      map.set(peer.peer_id, peer)
  }
  return map
})

/** peer_id → 主机名/虚拟 IP，供中转路径展示 */
const peersMetaById = computed(() => {
  const map = new Map<number, RoutePeerLabel>()
  for (const pair of props.curNetworkInst?.detail?.peer_route_pairs || []) {
    const peerId = pair.route?.peer_id
    if (!peerId)
      continue
    const ipv4 = pair.route?.ipv4_addr
    map.set(peerId, {
      peerId,
      hostname: pair.route?.hostname,
      ipv4: typeof ipv4 === 'string' ? ipv4 : (ipv4 ? ipv4InetToString(ipv4) : undefined),
    })
  }
  return map
})

function peerAddrDisplay(info: PeerRoutePair) {
  const resolved = resolvePeerRemoteAddr(info, peersById.value)
  if (!resolved.text)
    return { text: '', tip: undefined as string | undefined }
  if (resolved.viaNextHop) {
    return {
      text: t('status.peer_addr_via', { addr: resolved.text }),
      tip: `${t('status.peer_addr_via_tip')}\n${resolved.text}`,
    }
  }
  return { text: resolved.text, tip: resolved.text }
}

function routeCostDisplay(info: PeerRoutePair) {
  const me = props.curNetworkInst?.detail?.my_node_info
  const path = resolveRoutePath(info, {
    localLabel: me?.hostname || t('status.local'),
    localText: t('status.local'),
    p2pLabel: t('status.p2p'),
    peersMetaById: peersMetaById.value,
    viaPath: (p) => t('status.route_path', { path: p }),
    ellipsisLabel: (count) => t('status.route_path_ellipsis', { count }),
    incompleteTip: t('status.route_path_incomplete_tip'),
  })
  return path
}

function resolveObjPath(path: string, obj: any = globalThis, separator = '.') {
  const properties = path.split(separator)
  return properties.reduce((prev, curr) => prev?.[curr], obj)
}

function statsCommon(info: any, field: string): number | undefined {
  if (!info.peer)
    return undefined

  let sum = 0
  let hasValue = false
  for (const conn of peerConns(info)) {
    const value = numericValue(resolveObjPath(field, conn))
    if (value === undefined)
      continue

    sum += value
    hasValue = true
  }
  return hasValue ? sum : undefined
}

function humanFileSize(bytes: number, si = false, dp = 1) {
  const thresh = si ? 1000 : 1024

  if (Math.abs(bytes) < thresh)
    return `${bytes} B`

  const units = si
    ? ['kB', 'MB', 'GB', 'TB', 'PB', 'EB', 'ZB', 'YB']
    : ['KiB', 'MiB', 'GiB', 'TiB', 'PiB', 'EiB', 'ZiB', 'YiB']
  let u = -1
  const r = 10 ** dp

  do {
    bytes /= thresh
    ++u
  } while (Math.round(Math.abs(bytes) * r) / r >= thresh && u < units.length - 1)

  return `${bytes.toFixed(dp)} ${units[u]}`
}

function txBytes(info: PeerRoutePair) {
  const tx = statsCommon(info, 'stats.tx_bytes')
  return tx == null ? '' : humanFileSize(tx)
}

function rxBytes(info: PeerRoutePair) {
  const rx = statsCommon(info, 'stats.rx_bytes')
  return rx == null ? '' : humanFileSize(rx)
}

function version(info: PeerRoutePair) {
  return info.route.version === '' ? 'unknown' : info.route.version
}

function ipFormat(info: PeerRoutePair) {
  const ip = info.route.ipv4_addr
  if (typeof ip === 'string')
    return ip
  return ip ? ipv4InetToString(ip) : ''
}

function oneTunnelProto(tunnel?: TunnelInfo): string {
  if (!tunnel)
    return ''

  const local_addr = tunnel.local_addr
  let isIPv6 = false;
  if (local_addr?.url) {
    try {
      const urlObj = new URL(local_addr.url, 'http://dummy');
      // IPv6 addresses in URLs are enclosed in brackets and contain ':'
      isIPv6 = /^\[.*:.*\]$/.test(urlObj.hostname);
    } catch (e) {
      // fallback to original check if URL parsing fails
      isIPv6 = local_addr.url.indexOf('[') >= 0;
    }
  }
  if (isIPv6)
    return `${tunnel.tunnel_type}6`
  else
    return tunnel.tunnel_type
}

function tunnelProto(info: PeerRoutePair) {
  return [...new Set(peerConns(info).map(c => oneTunnelProto(c.tunnel)))].join(',')
}

const myNodeInfo = computed(() => {
  if (!props.curNetworkInst)
    return {} as NodeInfo

  return props.curNetworkInst.detail?.my_node_info
})

interface InfoChip {
  label: string
}

interface ChipGroup {
  key: string
  titleKey: string
  chips: InfoChip[]
}

// udp nat type
enum NatType {
  // has NAT; but own a single public IP, port is not changed
  Unknown = 0,
  OpenInternet = 1,
  NoPAT = 2,
  FullCone = 3,
  Restricted = 4,
  PortRestricted = 5,
  Symmetric = 6,
  SymUdpFirewall = 7,
  SymmetricEasyInc = 8,
  SymmetricEasyDec = 9,
};

const udpNatTypeStrMap = {
  [NatType.Unknown]: 'Unknown',
  [NatType.OpenInternet]: 'Open Internet',
  [NatType.NoPAT]: 'No PAT',
  [NatType.FullCone]: 'Full Cone',
  [NatType.Restricted]: 'Restricted',
  [NatType.PortRestricted]: 'Port Restricted',
  [NatType.Symmetric]: 'Symmetric',
  [NatType.SymUdpFirewall]: 'Symmetric UDP Firewall',
  [NatType.SymmetricEasyInc]: 'Symmetric Easy Inc',
  [NatType.SymmetricEasyDec]: 'Symmetric Easy Dec',
}

/** 按类型分区；顺序：Peer ID → … → Listener */
const myNodeInfoGroups = computed(() => {
  const groups: ChipGroup[] = []
  if (!props.curNetworkInst)
    return groups

  const my_node_info = props.curNetworkInst.detail?.my_node_info
  if (!my_node_info)
    return groups

  const chip = (label: string): InfoChip => ({ label })

  groups.push({
    key: 'peer_id',
    titleKey: 'node_info_group_peer_id',
    chips: [chip(String(my_node_info.peer_id))],
  })

  const dev_name = props.curNetworkInst.detail?.dev_name
  if (dev_name) {
    groups.push({
      key: 'tun',
      titleKey: 'node_info_group_tun_device',
      chips: [chip(dev_name)],
    })
  }

  // Prefer the L2 / platform sync summary over a separate Route chip list —
  // both surfaces show the same installed CIDRs; keep one canonical view.
  const routeSync = props.curNetworkInst.detail?.proxy_cidr_route_sync
  if (isMeaningfulProxyCidrRouteSync(routeSync)) {
    groups.push({
      key: 'proxy_cidr_route_sync',
      titleKey: 'node_info_group_proxy_cidr_route_sync',
      chips: [chip(routeSync!)],
    })
  }

  if (my_node_info.virtual_ipv4?.address !== undefined) {
  groups.push({
    key: 'virtual_ip',
    titleKey: 'node_info_group_virtual_ip',
    chips: [chip(ipv4InetToString(my_node_info.virtual_ipv4))],
  })
  }

  const udpNatType: NatType | undefined = my_node_info.stun_info?.udp_nat_type
  if (udpNatType !== undefined) {
    groups.push({
      key: 'udp_nat',
      titleKey: 'node_info_group_udp_nat_type',
      chips: [chip(udpNatTypeStrMap[udpNatType] ?? String(udpNatType))],
    })
  }

  const localChips: InfoChip[] = []
  for (const [idx, ip] of my_node_info.ips?.interface_ipv4s?.entries() ?? []) {
    localChips.push(chip(`IPv4 ${idx}: ${ipv4ToString(ip)}`))
  }
  for (const [idx, ip] of my_node_info.ips?.interface_ipv6s?.entries() ?? []) {
    localChips.push(chip(`IPv6 ${idx}: ${ipv6ToString(ip)}`))
  }
  if (localChips.length) {
    groups.push({
      key: 'local_ip',
      titleKey: 'node_info_group_local_ip',
      chips: localChips,
    })
  }

  const publicChips: InfoChip[] = []
  // 公网 IP 附带侦听端口，便于直接作为对端接入地址使用；优先取 UDP 侦听端口
  const listenerUrls = my_node_info.listeners ?? []
  const publicPort = (() => {
    const pick = listenerUrls.find(l => l.url?.startsWith('udp:')) ?? listenerUrls[0]
    const m = pick?.url?.match(/:(\d+)(?:[/?#]|$)/)
    return m?.[1] ?? ''
  })()
  const withPort = (ip: string) => (publicPort ? `${ip}:${publicPort}` : ip)
  if (my_node_info.ips?.public_ipv4) {
    publicChips.push(chip(`IPv4: ${withPort(ipv4ToString(my_node_info.ips.public_ipv4))}`))
  }
  if (my_node_info.ips?.public_ipv6) {
    publicChips.push(chip(`IPv6: ${withPort(ipv6ToString(my_node_info.ips.public_ipv6))}`))
  }
  if (publicChips.length) {
    groups.push({
      key: 'public_ip',
      titleKey: 'node_info_group_public_ip',
      chips: publicChips,
    })
  }

  const listenerChips: InfoChip[] = []
  for (const [idx, listener] of my_node_info.listeners?.entries() ?? []) {
    listenerChips.push(chip(`${idx}: ${listener.url}`))
  }
  if (listenerChips.length) {
    groups.push({
      key: 'listener',
      titleKey: 'node_info_group_listener',
      chips: listenerChips,
    })
  }

  return groups
})

function globalSumCommon(field: string) {
  let sum = 0
  if (!peerRouteInfos.value)
    return sum

  for (const info of peerRouteInfos.value) {
    const tx = statsCommon(info, field)
    if (tx)
      sum += tx
  }
  return sum
}

function txGlobalSum() {
  return globalSumCommon('stats.tx_bytes')
}

function rxGlobalSum() {
  return globalSumCommon('stats.rx_bytes')
}

function natType(info: PeerRoutePair): string {
  const udpNatType = info.route?.stun_info?.udp_nat_type;
  if (udpNatType !== undefined)
    return udpNatTypeStrMap[udpNatType as NatType]

  return ''
}

function isPublicServerRoute(info: PeerRoutePair): boolean {
  return info.route?.feature_flag?.is_public_server ?? false
}

function shouldAvoidRelayData(info: PeerRoutePair): boolean {
  return info.route?.feature_flag?.avoid_relay_data ?? false
}

const peerCount = computed(() => {
  if (!peerRouteInfos.value)
    return 0

  return peerRouteInfos.value.length
})

// calculate tx/rx rate every 2 seconds
let rateIntervalId = 0
const rateInterval = 2000
let prevTxSum: number | undefined
let prevRxSum: number | undefined
const txRate = ref('0 B')
const rxRate = ref('0 B')

/** 可折叠 Panel：节点默认展开（仅图表）；详情/历史/Peer/VPN/事件默认收起 */
const panelCollapsed = reactive({
  myNode: false,
  nodeDetails: true,
  peer: true,
  peerHistory: true,
  vpnPortal: true,
  eventLog: true,
})

/** 节点详情不再分组折叠：所有条目一次铺开，长内容允许换行，避免撑出横向滚动条。 */

async function copyGroupChips(group: { titleKey: string; chips: InfoChip[] }) {
  const text = group.chips.map(c => c.label).join('\n')
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(text)
    } else {
      const textarea = document.createElement('textarea')
      textarea.value = text
      textarea.style.position = 'fixed'
      textarea.style.opacity = '0'
      document.body.appendChild(textarea)
      textarea.select()
      document.execCommand('copy')
      textarea.remove()
    }
  } catch (e) {
    console.error('Failed to copy node info group', e)
  }
}

function onToggleablePanelHeaderClick(
  key: keyof typeof panelCollapsed,
  event: Event,
) {
  const target = event.target as HTMLElement | null
  if (isPanelHeaderInteractiveTarget(target, event)) {
    return
  }
  panelCollapsed[key] = !panelCollapsed[key]
}

function panelHeaderPt(key: keyof typeof panelCollapsed) {
  return {
    header: {
      class: 'cursor-pointer select-none touch-manipulation',
      onClick: (event: Event) => onToggleablePanelHeaderClick(key, event),
    },
    headerActions: {
      onClick: (event: Event) => event.stopPropagation(),
    },
  }
}

onMounted(() => {
  rateIntervalId = window.setInterval(() => {
    const curTxSum = txGlobalSum()
    if (prevTxSum === undefined || curTxSum < prevTxSum) {
      prevTxSum = curTxSum
      txRate.value = '0 B'
    } else {
      txRate.value = humanFileSize((curTxSum - prevTxSum) / (rateInterval / 1000))
      prevTxSum = curTxSum
    }

    const curRxSum = rxGlobalSum()
    if (prevRxSum === undefined || curRxSum < prevRxSum) {
      prevRxSum = curRxSum
      rxRate.value = '0 B'
    } else {
      rxRate.value = humanFileSize((curRxSum - prevRxSum) / (rateInterval / 1000))
      prevRxSum = curRxSum
    }
  }, rateInterval)
})

onUnmounted(() => {
  clearInterval(rateIntervalId)
})

const vpnPortalInfo = ref<VpnPortalInfo>()
const vpnPortalClients = computed(() => vpnPortalInfo.value?.clients ?? [])
const vpnPortalLoading = ref(false)
const vpnPortalError = ref('')
const copiedVpnPortalClient = ref('')

let vpnPortalLoadToken = 0

async function loadVpnPortalConfig() {
  const instanceId = props.curNetworkInst?.instance_id
  if (!instanceId)
    return

  const loadToken = ++vpnPortalLoadToken
  vpnPortalInfo.value = undefined
  vpnPortalError.value = ''
  copiedVpnPortalClient.value = ''
  vpnPortalLoading.value = true
  try {
    const info = await props.api.get_vpn_portal_info(instanceId)
    // 实例切换后，旧请求不能覆盖当前实例的状态。
    if (loadToken === vpnPortalLoadToken && props.curNetworkInst?.instance_id === instanceId) {
      vpnPortalInfo.value = info
    }
  } catch (error) {
    console.error('Failed to load VPN Portal information', error)
    if (loadToken === vpnPortalLoadToken && props.curNetworkInst?.instance_id === instanceId) {
      vpnPortalError.value = t('vpn_portal_load_failed')
    }
  } finally {
    if (loadToken === vpnPortalLoadToken) {
      vpnPortalLoading.value = false
    }
  }
}

watch(
  [() => panelCollapsed.vpnPortal, () => props.curNetworkInst?.instance_id],
  ([collapsed]) => {
    if (!collapsed)
      void loadVpnPortalConfig()
  },
)

function vpnPortalStateKey(state: VpnPortalClientState | string): string {
  const normalized = typeof state === 'string'
    ? state.toLowerCase().replace('vpn_portal_client_state_', '')
    : VpnPortalClientState[state]?.toLowerCase()
  return `vpn_portal_state_${normalized ?? 'unspecified'}`
}

function vpnPortalStateSeverity(state: VpnPortalClientState | string): 'success' | 'warn' | 'danger' | 'secondary' {
  const key = vpnPortalStateKey(state)
  if (key.endsWith('online')) return 'success'
  if (key.endsWith('connecting')) return 'warn'
  if (key.endsWith('error')) return 'danger'
  return 'secondary'
}

async function copyVpnPortalClientConfig(client: VpnPortalClientInfo) {
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(client.client_config)
    } else {
      const textarea = document.createElement('textarea')
      textarea.value = client.client_config
      textarea.style.position = 'fixed'
      textarea.style.opacity = '0'
      document.body.appendChild(textarea)
      textarea.select()
      document.execCommand('copy')
      textarea.remove()
    }
    copiedVpnPortalClient.value = client.name
  } catch (error) {
    console.error('Failed to copy VPN Portal client config', error)
  }
}

const eventLogContent = computed(() => {
  const detail = props.curNetworkInst?.detail
  if (!detail?.events)
    return []
  const items: any[] = []
  for (const event of detail.events) {
    try {
      items.push(JSON.parse(event))
    } catch {
      // 单条损坏时跳过，避免整页白屏
    }
  }
  return items
})
</script>

<template>
  <Card v-if="curNetworkInst?.error_msg">
      <template #title>
        Run Network Error
      </template>
      <template #content>
        <div class="flex flex-col gap-y-5">
          <div class="text-red-500">
            {{ curNetworkInst.error_msg }}
          </div>
        </div>
      </template>
    </Card>

  <div v-else class="status-panels flex flex-col gap-2">
        <Panel v-model:collapsed="panelCollapsed.myNode" :header="t('my_node_info')" toggleable
          class="status-panel"
          :class="{ 'status-panel--expanded': !panelCollapsed.myNode }"
          :pt="panelHeaderPt('myNode')">
          <div class="w-full status-panel-body">
            <NetworkChart :upload-rate="txRate" :download-rate="rxRate" />
          </div>
        </Panel>

        <!-- 节点详情：与「节点信息」同级的一级面板，不再嵌在「当前节点信息」内 -->
        <Panel v-if="myNodeInfo" v-model:collapsed="panelCollapsed.nodeDetails" :header="t('node_info_details')"
          class="status-panel"
          :class="{ 'status-panel--expanded': !panelCollapsed.nodeDetails }"
          toggleable :pt="panelHeaderPt('nodeDetails')">
          <div class="node-detail-groups status-panel-body">
            <div v-for="group in myNodeInfoGroups" :key="group.key" class="node-info-group">
              <span class="node-info-group-title">
                {{ t(group.titleKey) }}
                <span v-if="group.chips.length > 1" class="node-info-group-count">
                  ({{ group.chips.length }})
                </span>
              </span>
              <div class="node-info-group-chips">
                <span
                  v-for="(chip, i) in group.chips"
                  :key="i"
                  class="node-info-chip"
                >{{ chip.label }}</span>
              </div>
              <Button v-if="group.chips.length" size="small" severity="secondary" text rounded icon="pi pi-copy"
                class="et-icon-action-btn node-info-group-copy" :aria-label="t('node_info_copy_group')"
                v-tooltip.top="t('node_info_copy_group')" @click="copyGroupChips(group)" />
            </div>
          </div>
        </Panel>

        <Panel v-model:collapsed="panelCollapsed.peer" toggleable
          class="status-panel"
          :class="{ 'status-panel--expanded': !panelCollapsed.peer }"
          :pt="panelHeaderPt('peer')">
          <template #header>
            <div class="flex items-center gap-2">
              <span>{{ t('peer_info') }}</span>
              <Badge :value="peerCount" severity="info"
                class="text-xs font-semibold px-2 py-0.5 rounded-full" />
            </div>
          </template>
          <div class="peer-table-scroll status-panel-body">
          <DataTable :value="peerRouteInfos" column-resize-mode="expand" table-class="peer-route-table">
            <Column :field="ipFormat" :header="t('virtual_ipv4')" />
            <Column :header="t('hostname')">
              <template #body="slotProps">
                <div v-if="!slotProps.data.route.cost || !isPublicServerRoute(slotProps.data)"
                  v-tooltip.top="slotProps.data.route.hostname">
                  {{
                    slotProps.data.route.hostname }}
                </div>
                <div v-else v-tooltip.top="slotProps.data.route.hostname" class="space-x-1">
                  <Tag v-if="isPublicServerRoute(slotProps.data)" severity="info" value="Info">
                    {{ t('status.server') }}
                  </Tag>
                  <Tag v-if="shouldAvoidRelayData(slotProps.data)" severity="warn" value="Warn">
                    {{ t('status.relay') }}
                  </Tag>
                </div>
              </template>
            </Column>
            <Column :header="t('route_cost')">
              <template #body="slotProps">
                <span class="route-cost-cell" v-tooltip.top="routeCostDisplay(slotProps.data).tip">
                  {{ routeCostDisplay(slotProps.data).text }}
                </span>
              </template>
            </Column>
            <Column :field="tunnelProto" :header="t('tunnel_proto')"
              header-class="peer-col-secondary" body-class="peer-col-secondary" />
            <Column :header="t('peer_addr')">
              <template #body="slotProps">
                <span class="peer-addr-cell" v-tooltip.top="peerAddrDisplay(slotProps.data).tip">
                  {{ peerAddrDisplay(slotProps.data).text }}
                </span>
              </template>
            </Column>
            <Column :field="latencyMs" :header="t('latency')" />
            <Column :field="jitterMs" :header="t('jitter')"
              header-class="peer-col-secondary" body-class="peer-col-secondary" />
            <Column :field="lossRate" :header="t('loss_rate')"
              header-class="peer-col-secondary" body-class="peer-col-secondary" />
            <Column :header="t('path_quality')">
              <template #body="slotProps">
                <span
                  class="path-quality-cell"
                  v-tooltip.top="{ value: pathQualityTip(slotProps.data) || undefined, escape: true }"
                >
                  {{ pathQualityCell(slotProps.data) }}
                </span>
              </template>
            </Column>
            <Column :field="txBytes" :header="t('upload_bytes')"
              header-class="peer-col-secondary" body-class="peer-col-secondary" />
            <Column :field="rxBytes" :header="t('download_bytes')"
              header-class="peer-col-secondary" body-class="peer-col-secondary" />
            <Column :field="natType" :header="t('nat_type')"
              header-class="peer-col-secondary" body-class="peer-col-secondary" />
            <Column :header="t('status.version')"
              header-class="peer-col-secondary" body-class="peer-col-secondary">
              <template #body="slotProps">
                <span>{{ version(slotProps.data) }}</span>
              </template>
            </Column>
          </DataTable>
          </div>
        </Panel>

        <Panel v-if="api.get_peer_conn_history" v-model:collapsed="panelCollapsed.peerHistory"
          :header="t('peer_conn_history')" toggleable
          class="status-panel"
          :class="{ 'status-panel--expanded': !panelCollapsed.peerHistory }"
          :pt="panelHeaderPt('peerHistory')">
          <div class="status-panel-body">
            <PeerConnHistoryChart :api="api" :instance-id="curNetworkInst?.instance_id ?? ''"
              :visible="!panelCollapsed.peerHistory" />
          </div>
        </Panel>

        <Panel v-if="myNodeInfo" v-model:collapsed="panelCollapsed.vpnPortal" :header="t('vpn_portal_config')"
          class="status-panel"
          :class="{ 'status-panel--expanded': !panelCollapsed.vpnPortal }"
          toggleable :pt="panelHeaderPt('vpnPortal')">
          <ScrollPanel class="vpn-portal-scroll status-panel-body pr-3">
            <div v-if="vpnPortalLoading" class="status-empty status-empty--center">
              {{ t('web.device_management.loading_network_status') }}
            </div>
            <div v-else-if="vpnPortalError" class="status-empty status-empty--error">
              {{ vpnPortalError }}
            </div>
            <div
              v-else-if="!vpnPortalInfo || ((!vpnPortalInfo.vpn_type || vpnPortalInfo.vpn_type === 'null') && vpnPortalClients.length === 0)"
              class="status-empty">
              {{ t('vpn_portal_not_configured') }}
            </div>
            <div v-else class="flex flex-col gap-3">
              <div class="status-meta-row">
                <span v-if="vpnPortalInfo.vpn_type">
                  <span class="status-meta-label">{{ t('vpn_portal_type') }}</span>
                  {{ vpnPortalInfo.vpn_type }}
                </span>
                <span v-if="vpnPortalInfo.listener">
                  <span class="status-meta-label">{{ t('vpn_portal_listener') }}</span>
                  {{ vpnPortalInfo.listener }}
                </span>
              </div>

              <div v-for="client in vpnPortalClients" :key="client.name" class="status-subcard">
                <div class="mb-2 flex flex-wrap items-center justify-between gap-2">
                  <div class="status-subcard-title">{{ client.name }} · {{ client.virtual_ip }}</div>
                  <Tag :severity="vpnPortalStateSeverity(client.state)" :value="t(vpnPortalStateKey(client.state))" />
                </div>
                <div class="status-meta-grid mb-2">
                  <span v-if="client.groups.length">
                    <span class="status-meta-label">{{ t('vpn_portal_client_groups') }}</span>
                    {{ client.groups.join(', ') }}
                  </span>
                  <span v-if="client.peer_id !== undefined">
                    <span class="status-meta-label">{{ t('vpn_portal_peer_id') }}</span>
                    {{ client.peer_id }}
                  </span>
                  <span v-if="client.endpoint">
                    <span class="status-meta-label">{{ t('vpn_portal_endpoint') }}</span>
                    {{ client.endpoint }}
                  </span>
                  <span v-if="client.tunnel_ip">
                    <span class="status-meta-label">{{ t('vpn_portal_tunnel_ip') }}</span>
                    {{ client.tunnel_ip }}
                  </span>
                  <span v-if="client.error" class="status-meta-error sm:col-span-2">{{ client.error }}</span>
                </div>
                <div class="mb-2 flex items-center justify-between gap-3">
                  <span class="status-meta-label">{{ t('vpn_portal_client_config') }}</span>
                  <Button size="small" severity="secondary" text rounded icon="pi pi-copy"
                    class="et-icon-action-btn"
                    :aria-label="t('vpn_portal_copy_client_config')"
                    v-tooltip.top="copiedVpnPortalClient === client.name ? t('config_copied') : t('vpn_portal_copy_client_config')"
                    @click="copyVpnPortalClientConfig(client)" />
                </div>
                <pre class="status-code-block">{{ client.client_config }}</pre>
              </div>
            </div>
          </ScrollPanel>
        </Panel>

        <Panel v-if="myNodeInfo" v-model:collapsed="panelCollapsed.eventLog" :header="t('event_log')" toggleable
          class="status-panel"
          :class="{ 'status-panel--expanded': !panelCollapsed.eventLog }"
          :pt="panelHeaderPt('eventLog')">
          <div class="status-panel-body">
            <Timeline v-if="eventLogContent.length" :value="eventLogContent">
              <template #opposite="slotProps">
                <small class="status-event-time">{{ formatEventTime(slotProps.item.time) }}</small>
              </template>
              <template #content="slotProps">
                <HumanEvent :event="slotProps.item.event" />
              </template>
            </Timeline>
            <div v-else class="status-empty">—</div>
          </div>
        </Panel>
  </div>
</template>

<style lang="postcss" scoped>
/* 面板列表：整体收紧面板间距；桌面端占满父级以便展开项吃剩余高度 */
.status-panels {
  gap: 0.5rem;
  flex: 1 1 auto;
  min-height: 0;
  height: 100%;
}

.status-panels :deep(.p-panel) {
  border: 1px solid var(--et-border-color, #e2e8f0);
  border-radius: var(--et-radius, 0.75rem);
  background: var(--surface-card, #ffffff);
  box-shadow: none;
  overflow: hidden;
  flex: 0 0 auto;
  display: flex;
  flex-direction: column;
  min-height: 0;
}

/* 面板标题栏：折叠态更矮（覆盖 PrimeVue 默认与全局触摸目标高度） */
.status-panels :deep(.p-panel .p-panel-header) {
  padding: 0.35rem 0.75rem !important;
  min-height: 2rem !important;
  font-size: var(--et-fs-body, 0.875rem);
  font-weight: 600;
  line-height: 1.2;
  color: var(--text-color, #1e293b);
  background: transparent;
  border: none;
  flex-shrink: 0;
}

.status-panels :deep(.p-panel .p-panel-header.cursor-pointer) {
  min-height: 2rem !important;
}

.status-panels :deep(.p-panel .p-panel-content) {
  padding: 0.55rem 0.75rem !important;
  border: none;
  background: transparent;
  font-size: var(--et-fs-body, 0.875rem);
  color: var(--text-color, #1e293b);
}

.status-panels :deep(.p-panel .p-panel-header .p-panel-title),
.status-panels :deep(.p-panel .p-panel-header span:not(.p-badge)) {
  font-size: var(--et-fs-body, 0.875rem);
  font-weight: 600;
  line-height: 1.2;
  color: var(--text-color, #1e293b);
}

/* 桌面：展开的面板吃掉剩余高度，内容区内滚动 */
@media (min-width: 641px) {
  .status-panels :deep(.p-panel.status-panel--expanded) {
    flex: 1 1 0;
    min-height: 8rem;
  }

  .status-panels :deep(.status-panel--expanded .p-toggleable-content),
  .status-panels :deep(.status-panel--expanded .p-panel-content-container),
  .status-panels :deep(.status-panel--expanded [data-pc-section="contentcontainer"]),
  .status-panels :deep(.status-panel--expanded [data-pc-section="toggleablecontent"]) {
    flex: 1 1 0;
    min-height: 0;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .status-panels :deep(.status-panel--expanded .p-panel-content),
  .status-panels :deep(.status-panel--expanded [data-pc-section="content"]) {
    flex: 1 1 0;
    min-height: 0;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .status-panels :deep(.status-panel--expanded .status-panel-body) {
    flex: 1 1 0;
    min-height: 0;
    overflow: auto;
    overscroll-behavior: contain;
  }

  .status-panels :deep(.status-panel--expanded .node-detail-groups) {
    max-height: none;
  }

  .status-panels :deep(.status-panel--expanded .vpn-portal-scroll) {
    max-height: none !important;
    height: 100%;
  }

  .status-panels :deep(.status-panel--expanded .vpn-portal-scroll.p-scrollpanel),
  .status-panels :deep(.status-panel--expanded .p-scrollpanel.vpn-portal-scroll) {
    flex: 1 1 0;
    min-height: 0;
  }

  .status-panels :deep(.status-panel--expanded .peer-table-scroll) {
    overflow: auto;
  }
}

/* 节点详情分组：行高随 chip 内容自适应（1 行或多行） */
.node-detail-groups {
  display: flex;
  flex-direction: column;
  gap: 0.375rem;
  padding-right: 0.1rem;
  /* Desktop/GUI: keep a compact scroll area. */
  max-height: 18rem;
  overflow: auto;
  overscroll-behavior: contain;
}

.node-info-group {
  display: grid;
  grid-template-columns: 6.5rem minmax(0, 1fr) auto;
  align-items: start;
  column-gap: 0.5rem;
  row-gap: 0.35rem;
  border: 1px solid var(--et-border-color, #e2e8f0);
  border-radius: calc(var(--et-radius, 0.75rem) - 0.25rem);
  background: var(--surface-50, #f8fafc);
  padding: 0.35rem 0.5rem 0.4rem;
  /* Never shrink inside the flex column — Android Chrome/WebView otherwise
     crushes rows so chips and copy buttons overlap. */
  height: auto;
  min-height: auto;
  flex: 0 0 auto;
}

.node-info-group-title {
  grid-column: 1;
  min-width: 0;
  padding-top: 0.15rem;
  font-size: var(--et-fs-meta, 0.75rem);
  font-weight: 600;
  letter-spacing: 0.02em;
  text-transform: uppercase;
  line-height: 1.25;
  color: var(--text-color-secondary, #64748b);
}

.node-info-group-count {
  font-weight: 400;
  text-transform: none;
  letter-spacing: 0;
}

.node-info-group-chips {
  grid-column: 2;
  min-width: 0;
  display: flex;
  flex-wrap: wrap;
  align-items: flex-start;
  align-content: flex-start;
  gap: 0.3rem;
  height: auto;
}

.node-info-group-copy {
  grid-column: 3;
  align-self: start;
}

/*
 * 标签用次要色、数值用正文色（回退色保证 Android WebView 可见）。
 * 禁止 flex-shrink，避免单行被压成 0 宽；过长 chip 可在容器内断行。
 */
.node-info-chip {
  display: inline-block;
  flex: 0 0 auto;
  width: auto;
  max-width: 100%;
  padding: 0.1rem 0.45rem;
  min-height: 1.3rem;
  box-sizing: border-box;
  font-size: var(--et-fs-body, 0.875rem);
  font-weight: 500;
  line-height: 1.35;
  color: var(--text-color, #1e293b);
  background: var(--surface-100, #f1f5f9);
  border: 1px solid var(--et-border-color, #e2e8f0);
  border-radius: 0.375rem;
  white-space: normal;
  overflow-wrap: anywhere;
  word-break: break-word;
}

.status-subcard {
  border: 1px solid var(--et-border-color, #e2e8f0);
  border-radius: calc(var(--et-radius, 0.75rem) - 0.25rem);
  background: var(--surface-50, #f8fafc);
  padding: 0.75rem 0.85rem;
}

.status-subcard-title {
  font-size: var(--et-fs-body, 0.875rem);
  font-weight: 600;
  color: var(--text-color, #1e293b);
  line-height: 1.35;
}

.status-meta-row,
.status-meta-grid {
  font-size: var(--et-fs-body, 0.875rem);
  color: var(--text-color, #1e293b);
}

.status-meta-row {
  display: flex;
  flex-wrap: wrap;
  gap: 0.35rem 1.25rem;
}

.status-meta-grid {
  display: grid;
  gap: 0.25rem 1.5rem;
}

@media (min-width: 640px) {
  .status-meta-grid {
    grid-template-columns: repeat(2, minmax(0, 1fr));
  }
}

.status-meta-label {
  font-size: var(--et-fs-meta, 0.75rem);
  font-weight: 600;
  color: var(--text-color-secondary, #64748b);
  margin-right: 0.25rem;
}

.status-meta-error {
  color: var(--et-danger, #ef4444);
}

.status-code-block {
  max-width: 100%;
  margin: 0;
  overflow-x: auto;
  white-space: pre-wrap;
  overflow-wrap: anywhere;
  word-break: break-word;
  padding: 0.65rem 0.75rem;
  border: 1px solid var(--et-border-color, #e2e8f0);
  border-radius: calc(var(--et-radius, 0.75rem) - 0.35rem);
  background: var(--surface-100, #f1f5f9);
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: var(--et-fs-meta, 0.75rem);
  line-height: 1.45;
  color: var(--text-color, #1e293b);
}

.status-empty {
  padding: 1rem 0;
  font-size: var(--et-fs-body, 0.875rem);
  color: var(--text-color-secondary, #64748b);
}

.status-empty--center {
  text-align: center;
  padding: 2rem 0;
}

.status-empty--error {
  color: var(--et-danger, #ef4444);
}

.status-event-time {
  font-size: var(--et-fs-meta, 0.75rem);
  color: var(--text-color-secondary, #64748b);
}

@media (max-width: 640px) {
  /* Mobile web console / Android: avoid nested scroll; let the page scroll. */
  .status-panels {
    height: auto;
    flex: 0 0 auto;
  }

  .node-detail-groups {
    max-height: none;
    overflow: visible;
  }

  .node-info-group {
    /* Narrow: title + copy on row 1, chips full-width on row 2. */
    grid-template-columns: minmax(0, 1fr) auto;
    padding: 0.45rem 0.55rem;
  }

  .node-info-group-title {
    grid-column: 1;
    padding-top: 0;
    font-size: var(--et-fs-meta, 0.75rem);
    text-transform: none;
    letter-spacing: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .node-info-group-copy {
    grid-column: 2;
  }

  .node-info-group-chips {
    grid-column: 1 / -1;
    width: 100%;
  }
}

@media (prefers-color-scheme: dark) {
  .node-info-group,
  .status-subcard {
    border-color: var(--surface-border, #334155);
    background: var(--surface-card, #1e293b);
  }

  .node-info-group-title,
  .status-meta-label,
  .status-empty,
  .status-event-time {
    color: var(--text-color-secondary, #94a3b8);
  }

  .node-info-chip,
  .status-code-block {
    color: var(--text-color, #f1f5f9);
    background: var(--surface-100, #334155);
    border-color: var(--surface-border, #475569);
  }

  .status-subcard-title,
  .status-meta-row,
  .status-meta-grid {
    color: var(--text-color, #f1f5f9);
  }

  .status-empty--error,
  .status-meta-error {
    color: var(--et-danger, #f87171);
  }
}

/* 节点信息表格：行高紧凑，字号与面板正文一致 */
.status-panels :deep(.p-datatable .p-datatable-thead > tr > th) {
  padding: 0.4rem 0.6rem !important;
  font-size: var(--et-fs-meta, 0.75rem);
  font-weight: 600;
  color: var(--text-color-secondary, #64748b);
}

.status-panels :deep(.p-datatable .p-datatable-tbody > tr > td) {
  padding: 0.35rem 0.6rem !important;
  font-size: var(--et-fs-body, 0.875rem);
  color: var(--text-color, #1e293b);
}

.peer-addr-cell,
.route-cost-cell {
  display: inline-block;
  max-width: 16rem;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  vertical-align: bottom;
  font-variant-numeric: tabular-nums;
}

.peer-table-scroll {
  width: 100%;
  overflow-x: auto;
  -webkit-overflow-scrolling: touch;
}

.peer-table-scroll :deep(.peer-route-table) {
  min-width: 36rem;
}

@media (max-width: 640px) {
  .peer-table-scroll :deep(.peer-col-secondary) {
    display: none !important;
  }

  .peer-table-scroll :deep(.peer-route-table) {
    min-width: 22rem;
  }

  .peer-addr-cell,
  .route-cost-cell {
    max-width: 8rem;
  }

  /* 事件时间线：窄屏改为时间在内容上方，避免对侧栏挤压正文 */
  .status-panels :deep(.p-timeline-event) {
    flex-direction: column;
    align-items: flex-start;
    gap: 0.15rem;
  }

  .status-panels :deep(.p-timeline-event-opposite) {
    flex: 0 0 auto !important;
    padding: 0 !important;
    text-align: left;
  }

  .status-panels :deep(.p-timeline-event-separator) {
    display: none;
  }
}

.p-timeline :deep(.p-timeline-event-opposite) {
  @apply flex-none;
}

:deep(.p-datatable .p-datatable-column-title) {
  white-space: nowrap;
}
</style>
