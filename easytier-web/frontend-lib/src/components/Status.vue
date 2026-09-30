<script setup lang="ts">
import { useTimeAgo } from '@vueuse/core'
import { NetworkInstance, VpnPortalClientState, type TunnelInfo, type NodeInfo, type PeerRoutePair, type VpnPortalClientInfo, type VpnPortalInfo } from '../types/network'
import type { RemoteClient } from '../modules/api'
import { useI18n } from 'vue-i18n';
import { computed, onMounted, onUnmounted, reactive, ref, watch } from 'vue';
import { ipv4InetToString, ipv4ToString, ipv6ToString } from '../modules/utils';
import { latencyMs, lossRate, numericValue, peerConns } from '../modules/statusDisplay';
import { Badge, DataTable, Column, Tag, Chip, Button, ScrollPanel, Timeline, Card, Panel, } from 'primevue';
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

function routeCost(info: any) {
  if (info.route) {
    const cost = info.route.cost
    return cost ? cost === 1 ? 'p2p' : `relay(${cost})` : t('status.local')
  }

  return '?'
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

interface Chip {
  label: string
  icon: string
}

interface ChipGroup {
  key: string
  titleKey: string
  chips: Chip[]
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

/** 按类型分区；顺序：Peer ID → Virtual IP → UDP NAT → Local IP → Public IP → Listener */
const myNodeInfoGroups = computed(() => {
  const groups: ChipGroup[] = []
  if (!props.curNetworkInst)
    return groups

  const my_node_info = props.curNetworkInst.detail?.my_node_info
  if (!my_node_info)
    return groups

  const chip = (label: string): Chip => ({ label, icon: '' })

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

  groups.push({
    key: 'virtual_ip',
    titleKey: 'node_info_group_virtual_ip',
    chips: [chip(ipv4InetToString(my_node_info.virtual_ipv4))],
  })

  const udpNatType: NatType | undefined = my_node_info.stun_info?.udp_nat_type
  if (udpNatType !== undefined) {
    groups.push({
      key: 'udp_nat',
      titleKey: 'node_info_group_udp_nat_type',
      chips: [chip(udpNatTypeStrMap[udpNatType] ?? String(udpNatType))],
    })
  }

  const localChips: Chip[] = []
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

  const publicChips: Chip[] = []
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

  const listenerChips: Chip[] = []
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

async function copyGroupChips(group: { titleKey: string; chips: Chip[] }) {
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
  if (target?.closest('button, a, input, textarea, select, [role="button"]')) {
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
  <div class="frontend-lib">
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

    <template v-else>
      <div class="status-panels flex flex-col gap-2">
        <Panel v-model:collapsed="panelCollapsed.myNode" :header="t('my_node_info')" toggleable
          :pt="panelHeaderPt('myNode')">
          <div class="w-full">
            <NetworkChart :upload-rate="txRate" :download-rate="rxRate" />
          </div>
        </Panel>

        <!-- 节点详情：与「节点信息」同级的一级面板，不再嵌在「当前节点信息」内 -->
        <Panel v-if="myNodeInfo" v-model:collapsed="panelCollapsed.nodeDetails" :header="t('node_info_details')"
          toggleable :pt="panelHeaderPt('nodeDetails')">
          <div class="node-detail-groups flex flex-col gap-1.5 max-h-72 overflow-auto">
            <div v-for="group in myNodeInfoGroups" :key="group.key" class="node-info-group">
              <span class="node-info-group-title truncate">
                {{ t(group.titleKey) }}
                <span v-if="group.chips.length > 1" class="normal-case font-normal">
                  ({{ group.chips.length }})
                </span>
              </span>
              <!-- 内容多时自动换行（不横向滚动），分组高度随之增加 -->
              <div class="node-info-group-chips">
                <Chip v-for="(chip, i) in group.chips" :key="i" :label="chip.label" :icon="chip.icon"
                  class="node-info-chip" v-tooltip.top="chip.label" />
              </div>
              <Button v-if="group.chips.length" size="small" text rounded icon="pi pi-copy"
                :aria-label="t('node_info_copy_group')" v-tooltip.top="t('node_info_copy_group')"
                @click="copyGroupChips(group)" />
            </div>
          </div>
        </Panel>

        <Panel v-model:collapsed="panelCollapsed.peer" toggleable :pt="panelHeaderPt('peer')">
          <template #header>
            <div class="flex items-center gap-2">
              <span>{{ t('peer_info') }}</span>
              <Badge :value="peerCount" severity="info"
                class="text-xs font-semibold px-2 py-0.5 rounded-full" />
            </div>
          </template>
          <DataTable :value="peerRouteInfos" column-resize-mode="fit" table-class="w-full">
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
            <Column :field="routeCost" :header="t('route_cost')" />
            <Column :field="tunnelProto" :header="t('tunnel_proto')" />
            <Column :field="latencyMs" :header="t('latency')" />
            <Column :field="txBytes" :header="t('upload_bytes')" />
            <Column :field="rxBytes" :header="t('download_bytes')" />
            <Column :field="lossRate" :header="t('loss_rate')" />
            <Column :field="natType" :header="t('nat_type')" />
            <Column :header="t('status.version')">
              <template #body="slotProps">
                <span>{{ version(slotProps.data) }}</span>
              </template>
            </Column>
          </DataTable>
        </Panel>

        <Panel v-if="api.get_peer_conn_history" v-model:collapsed="panelCollapsed.peerHistory"
          :header="t('peer_conn_history')" toggleable :pt="panelHeaderPt('peerHistory')">
          <PeerConnHistoryChart :api="api" :instance-id="curNetworkInst?.instance_id ?? ''"
            :visible="!panelCollapsed.peerHistory" />
        </Panel>

        <Panel v-if="myNodeInfo" v-model:collapsed="panelCollapsed.vpnPortal" :header="t('vpn_portal_config')"
          toggleable :pt="panelHeaderPt('vpnPortal')">
          <ScrollPanel class="max-h-[50vh] pr-3">
            <div v-if="vpnPortalLoading" class="py-8 text-center text-surface-500">
              {{ t('web.device_management.loading_network_status') }}
            </div>
            <div v-else-if="vpnPortalError" class="py-4 text-red-500">
              {{ vpnPortalError }}
            </div>
            <div
              v-else-if="!vpnPortalInfo || ((!vpnPortalInfo.vpn_type || vpnPortalInfo.vpn_type === 'null') && vpnPortalClients.length === 0)"
              class="py-4 text-surface-500">
              {{ t('vpn_portal_not_configured') }}
            </div>
            <div v-else class="flex flex-col gap-4">
              <div class="flex flex-wrap gap-x-6 gap-y-2 text-sm">
                <span v-if="vpnPortalInfo.vpn_type"><strong>{{ t('vpn_portal_type') }}:</strong>
                  {{ vpnPortalInfo.vpn_type }}</span>
                <span v-if="vpnPortalInfo.listener"><strong>{{ t('vpn_portal_listener') }}:</strong>
                  {{ vpnPortalInfo.listener }}</span>
              </div>

              <div v-for="client in vpnPortalClients" :key="client.name"
                class="rounded border border-surface-200 dark:border-surface-700 p-4">
                <div class="mb-3 flex flex-wrap items-center justify-between gap-2">
                  <div class="font-semibold">{{ client.name }} · {{ client.virtual_ip }}</div>
                  <Tag :severity="vpnPortalStateSeverity(client.state)" :value="t(vpnPortalStateKey(client.state))" />
                </div>
                <div class="mb-3 grid gap-x-6 gap-y-1 text-sm sm:grid-cols-2">
                  <span v-if="client.groups.length"><strong>{{ t('vpn_portal_client_groups') }}:</strong>
                    {{ client.groups.join(', ') }}</span>
                  <span v-if="client.peer_id !== undefined"><strong>{{ t('vpn_portal_peer_id') }}:</strong>
                    {{ client.peer_id }}</span>
                  <span v-if="client.endpoint"><strong>{{ t('vpn_portal_endpoint') }}:</strong>
                    {{ client.endpoint }}</span>
                  <span v-if="client.tunnel_ip"><strong>{{ t('vpn_portal_tunnel_ip') }}:</strong>
                    {{ client.tunnel_ip }}</span>
                  <span v-if="client.error" class="text-red-500 sm:col-span-2">{{ client.error }}</span>
                </div>
                <div class="mb-2 flex items-center justify-between gap-3">
                  <label class="font-medium">{{ t('vpn_portal_client_config') }}</label>
                  <Button size="small" severity="secondary" icon="pi pi-copy"
                    :label="copiedVpnPortalClient === client.name ? t('config_copied') : t('vpn_portal_copy_client_config')"
                    @click="copyVpnPortalClientConfig(client)" />
                </div>
                <pre
                  class="max-w-full overflow-x-auto whitespace-pre-wrap break-all rounded bg-surface-100 p-3 text-xs dark:bg-surface-800">{{ client.client_config }}</pre>
              </div>
            </div>
          </ScrollPanel>
        </Panel>

        <Panel v-if="myNodeInfo" v-model:collapsed="panelCollapsed.eventLog" :header="t('event_log')" toggleable
          :pt="panelHeaderPt('eventLog')">
          <Timeline v-if="eventLogContent.length" :value="eventLogContent">
            <template #opposite="slotProps">
              <small class="text-surface-500 dark:text-surface-400">{{ useTimeAgo(Date.parse(slotProps.item.time))
              }}</small>
            </template>
            <template #content="slotProps">
              <HumanEvent :event="slotProps.item.event" />
            </template>
          </Timeline>
          <div v-else class="py-4 text-surface-500 text-sm">—</div>
        </Panel>
      </div>
    </template>
  </div>
</template>

<style lang="postcss" scoped>
/* 面板列表：整体收紧面板间距 */
.status-panels {
  gap: 0.25rem;
}

/* 面板标题栏：折叠态更矮（覆盖 PrimeVue 默认与全局触摸目标高度） */
.status-panels :deep(.p-panel .p-panel-header) {
  padding: 0.25rem 0.65rem !important;
  min-height: 1.9rem !important;
  font-size: 0.875rem;
  font-weight: 600;
  line-height: 1.2;
}

.status-panels :deep(.p-panel .p-panel-header.cursor-pointer) {
  min-height: 1.9rem !important;
}

.status-panels :deep(.p-panel .p-panel-content) {
  padding: 0.5rem 0.7rem !important;
}

.status-panels :deep(.p-panel .p-panel-header .p-panel-title),
.status-panels :deep(.p-panel .p-panel-header span) {
  font-size: 0.875rem;
  font-weight: 600;
  line-height: 1.2;
}

/* 节点详情分组卡片 */
.node-detail-groups {
  padding-right: 0.1rem;
}

.node-info-group {
  border: 1px solid var(--surface-border, #e5e7eb);
  border-radius: 0.375rem;
  background: var(--surface-50, #f8fafc);
  padding: 0.3rem 0.5rem 0.4rem;
}

/* 一组：标题定宽（顶部对齐）+ chip 区自动换行（不横向滚动）+ 复制按钮 */
.node-info-group {
  display: flex;
  align-items: flex-start;
  gap: 0.5rem;
  min-height: 1.6rem;
}

.node-info-group-title {
  flex: 0 0 6.5rem;
  min-width: 0;
  padding-top: 0.15rem; /* 与换行的 chip 首行基线对齐 */
  font-size: 0.6875rem;
  font-weight: 600;
  letter-spacing: 0.02em;
  text-transform: uppercase;
  color: var(--text-color-secondary, #64748b);
}

.node-info-group-chips {
  flex: 1 1 auto;
  min-width: 0;
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 0.3rem;
}

.status-panels :deep(.node-info-chip.p-chip) {
  flex: 0 1 auto;
  min-width: 0;
  padding: 0.05rem 0.4rem;
  min-height: 1.25rem;
  height: auto;
  font-size: 0.75rem;
  line-height: 1.2;
  /* 允许文字在极窄屏换行，避免单个超长 chip 撑出横向滚动条 */
  white-space: normal;
  overflow-wrap: anywhere;
}

/* 窄屏把标题压窄一点，给 chip 区留出可见宽度 */
@media (max-width: 640px) {
  .node-info-group-title {
    flex: 0 0 5rem;
  }
}

/* 节点信息表格：行高紧凑（仅收紧内边距，不改字号） */
.status-panels :deep(.p-datatable .p-datatable-thead > tr > th) {
  padding: 0.4rem 0.6rem !important;
}

.status-panels :deep(.p-datatable .p-datatable-tbody > tr > td) {
  padding: 0.35rem 0.6rem !important;
}

.p-timeline :deep(.p-timeline-event-opposite) {
  @apply flex-none;
}

:deep(.p-datatable .p-datatable-column-title) {
  white-space: nowrap;
}
</style>
