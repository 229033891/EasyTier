<script setup lang="ts">
import { v4 as uuidv4 } from 'uuid'
import { AutoComplete, Button, Checkbox, Dialog, InputNumber, InputText, MultiSelect, Panel, Password, Select, ToggleButton, useConfirm, useToast } from 'primevue'
import { TOAST_LIFE } from '../modules/toast'
import { isPanelHeaderInteractiveTarget } from '../modules/panel'
import InputGroup from 'primevue/inputgroup'
import InputGroupAddon from 'primevue/inputgroupaddon'
import {
  addRow,
  applyConnectionPathTier,
  CompressionAlgoPb,
  ConnectionPathTier,
  DEFAULT_NETWORK_CONFIG,
  emptyDnsConfig,
  NetworkConfig,
  normalizeNetworkConfig,
  removeRow,
  type DnsConfig,
  type VpnPortalClientConfig,
  type VpnPortalConfig,
} from '../types/network'
import { computed, reactive, ref, onMounted, onUnmounted, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import {
  advancedFlagConflictHelpKey,
  collectConfigConflictWarnings,
  isAdvancedFlagDisabled,
  isAdvancedFlagHidden,
} from '../modules/configConflicts'
import AclManager from './acl/AclManager.vue'
import DnsHostsEditor from './dns/DnsHostsEditor.vue'
import DnsForwardersEditor from './dns/DnsForwardersEditor.vue'
import DnsUpstreamEditor from './dns/DnsUpstreamEditor.vue'
import UrlListInput from './UrlListInput.vue'

const props = defineProps<{
  actionLabel?: string
  configInvalid?: boolean
  hostname?: string
  /** 由父级统一渲染底部操作区时隐藏内置「运行网络」按钮 */
  hideRunButton?: boolean
}>()

defineEmits(['runNetwork'])

const curNetwork = defineModel('curNetwork', {
  type: Object as () => NetworkConfig,
  default: DEFAULT_NETWORK_CONFIG,
})

const { t } = useI18n()
const toast = useToast()
const confirm = useConfirm()

const NETWORK_SECRET_ALPHABET =
  'ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz23456789!@#$%^&*-_=+'

/** 均匀取样，避免 `byte % alphabet.length` 的模偏差 */
function randomSecret(length = 24): string {
  const alphabet = NETWORK_SECRET_ALPHABET
  const maxUnbiased = Math.floor(256 / alphabet.length) * alphabet.length
  const out: string[] = []
  while (out.length < length) {
    const bytes = new Uint8Array(length - out.length)
    crypto.getRandomValues(bytes)
    for (const b of bytes) {
      if (b >= maxUnbiased) continue
      out.push(alphabet[b % alphabet.length])
      if (out.length >= length) break
    }
  }
  return out.join('')
}

async function applyGeneratedNetworkSecret() {
  const secret = randomSecret()
  curNetwork.value.network_secret = secret
  try {
    if (!navigator.clipboard?.writeText) {
      throw new Error('clipboard unavailable')
    }
    await navigator.clipboard.writeText(secret)
    toast.add({
      severity: 'success',
      summary: t('network_secret_generated'),
      life: TOAST_LIFE.success,
    })
  } catch {
    toast.add({
      severity: 'warn',
      summary: t('network_secret_copy_failed'),
      life: TOAST_LIFE.warn,
    })
  }
}

/** 生成高强度网络密码并复制；若已有密码则先确认覆盖 */
function generateAndCopyNetworkSecret() {
  if (curNetwork.value.network_secret?.trim()) {
    confirm.require({
      message: t('network_secret_overwrite_confirm'),
      header: t('network_secret_overwrite_header'),
      icon: 'pi pi-exclamation-triangle',
      rejectProps: {
        label: t('web.common.cancel'),
        severity: 'secondary',
        outlined: true,
      },
      acceptProps: {
        label: t('network_secret_generate'),
        severity: 'warning',
      },
      accept: () => {
        void applyGeneratedNetworkSecret()
      },
    })
    return
  }
  void applyGeneratedNetworkSecret()
}

/** 可折叠 Panel：默认 Basic 展开，其余收起；整块标题栏可点（触摸友好） */
const panelCollapsed = reactive({
  basic: false,
  advanced: true,
  portForwards: true,
  dns: true,
  acl: true,
})

function ensureDnsConfig(): DnsConfig {
  if (!curNetwork.value.dns_config) {
    curNetwork.value.dns_config = emptyDnsConfig()
  }
  curNetwork.value.dns_config.hosts ??= []
  curNetwork.value.dns_config.forwarders ??= []
  curNetwork.value.dns_config.upstream_dns ??= []
  return curNetwork.value.dns_config
}

function initDnsHosts() {
  ensureDnsConfig()
}

function onToggleablePanelHeaderClick(
  key: keyof typeof panelCollapsed,
  event: Event,
) {
  const target = event.target as HTMLElement | null
  // +/- 按钮会自己切换；若再冒泡到标题栏会翻两次，表现为「一点开又关上」
  // 帮助图标同样跳过，避免点 tip 时面板跟着折叠
  if (
    isPanelHeaderInteractiveTarget(target, event, [
      '.config-help-tip',
      '.dns-mixed-version-tip',
      '[role="img"]',
    ])
  ) {
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
      // 阻止加减号区域冒泡到标题栏，避免双重 toggle
      onClick: (event: Event) => event.stopPropagation(),
    },
  }
}

const protos: { [proto: string]: number } = {
  tcp: 11010,
  udp: 11010,
  wg: 11011,
  ws: 11011,
  wss: 11012,
  quic: 11012,
  faketcp: 11013,
  http: 80,
  https: 443,
  txt: 0,
  srv: 0,
}

const listenerExcludedProtos = new Set(['http', 'https', 'txt', 'srv'])
const listenerProtos: { [proto: string]: number } = Object.fromEntries(
  Object.entries(protos).filter(([proto]) => !listenerExcludedProtos.has(proto))
)

const inetSuggestions = ref([''])

function searchInetSuggestions(e: { query: string }) {
  if (e.query.search('/') >= 0) {
    inetSuggestions.value = [e.query]
  } else {
    const ret = []
    for (let i = 0; i < 32; i++) {
      ret.push(`${e.query}/${i}`)
    }
    inetSuggestions.value = ret
  }
}

const exitNodesSuggestions = ref([''])

function searchExitNodesSuggestions(e: { query: string }) {
  const ret = []
  ret.push(e.query)
  exitNodesSuggestions.value = ret
}

const whitelistSuggestions = ref([''])

function searchWhitelistSuggestions(e: { query: string }) {
  const ret = []
  ret.push(e.query)
  whitelistSuggestions.value = ret
}

type AdvancedFlagGroup = 'connectivity' | 'transport' | 'system' | 'security'

interface BoolFlag {
  field: keyof NetworkConfig
  help: string
  group: AdvancedFlagGroup
}

const bool_flags: BoolFlag[] = [
  { field: 'latency_first', help: 'latency_first_help', group: 'connectivity' },
  // disable_p2p / prefer_peer_relay: projected from connection_path_tier (P0.4 UI SoT)
  { field: 'p2p_only', help: 'p2p_only_help', group: 'connectivity' },
  { field: 'lazy_p2p', help: 'lazy_p2p_help', group: 'connectivity' },
  { field: 'need_p2p', help: 'need_p2p_help', group: 'connectivity' },
  { field: 'enable_exit_node', help: 'enable_exit_node_help', group: 'connectivity' },
  { field: 'allow_peer_default_without_exit', help: 'allow_peer_default_without_exit_help', group: 'connectivity' },
  { field: 'relay_all_peer_rpc', help: 'relay_all_peer_rpc_help', group: 'connectivity' },
  { field: 'disable_relay_data', help: 'disable_relay_data_help', group: 'connectivity' },
  { field: 'use_smoltcp', help: 'use_smoltcp_help', group: 'transport' },
  { field: 'enable_kcp_proxy', help: 'enable_kcp_proxy_help', group: 'transport' },
  { field: 'disable_kcp_input', help: 'disable_kcp_input_help', group: 'transport' },
  { field: 'enable_quic_proxy', help: 'enable_quic_proxy_help', group: 'transport' },
  { field: 'disable_quic_input', help: 'disable_quic_input_help', group: 'transport' },
  { field: 'disable_tcp_hole_punching', help: 'disable_tcp_hole_punching_help', group: 'transport' },
  { field: 'disable_udp_hole_punching', help: 'disable_udp_hole_punching_help', group: 'transport' },
  { field: 'disable_sym_hole_punching', help: 'disable_sym_hole_punching_help', group: 'transport' },
  { field: 'disable_upnp', help: 'disable_upnp_help', group: 'transport' },
  { field: 'enable_udp_broadcast_relay', help: 'enable_udp_broadcast_relay_help', group: 'transport' },
  { field: 'disable_ipv6', help: 'disable_ipv6_help', group: 'system' },
  { field: 'ipv6_public_addr_auto', help: 'ipv6_public_addr_auto_help', group: 'system' },
  { field: 'bind_device', help: 'bind_device_help', group: 'system' },
  { field: 'no_tun', help: 'no_tun_help', group: 'system' },
  { field: 'multi_thread', help: 'multi_thread_help', group: 'system' },
  { field: 'proxy_forward_by_system', help: 'proxy_forward_by_system_help', group: 'system' },
  { field: 'enable_private_mode', help: 'enable_private_mode_help', group: 'security' },
  { field: 'disable_encryption', help: 'disable_encryption_help', group: 'security' },
]

const advancedFlagGroups = computed(() => {
  const groupTitles: Array<{ key: AdvancedFlagGroup; titleKey: string; icon: string }> = [
    { key: 'connectivity', titleKey: 'advanced_group_connectivity', icon: 'pi pi-share-alt' },
    { key: 'transport', titleKey: 'advanced_group_transport', icon: 'pi pi-send' },
    { key: 'system', titleKey: 'advanced_group_system', icon: 'pi pi-sliders-h' },
    { key: 'security', titleKey: 'advanced_group_security', icon: 'pi pi-shield' },
  ]
  const cfg = curNetwork.value
  return groupTitles.map((group) => ({
    ...group,
    flags: bool_flags
      .filter((flag) => flag.group === group.key && !isAdvancedFlagHidden(cfg, flag.field))
      .map((flag) => {
        const conflictHelp = advancedFlagConflictHelpKey(cfg, flag.field)
        return {
          ...flag,
          disabled: isAdvancedFlagDisabled(cfg, flag.field),
          helpKey: conflictHelp ?? flag.help,
        }
      }),
  }))
})

const conflictWarnings = computed(() => collectConfigConflictWarnings(curNetwork.value))

const relayControlsDisabled = computed(() => !!curNetwork.value.p2p_only)
const tunControlsDisabled = computed(() => !!curNetwork.value.no_tun)
const ipv6ControlsHidden = computed(() => !!curNetwork.value.disable_ipv6)
const encryptionAlgoHidden = computed(() => !!curNetwork.value.disable_encryption)

/**
 * 加密算法选项对齐 `EncryptionAlgorithm::from_str`（easytier-core/src/config/encryption.rs）：除下列名称外，后端还接受
 * openssl-* 前缀别名（openssl-aes-gcm / openssl-aes-256-gcm / openssl-chacha20）；这里只列会原样回写的 canonical 名称。
 */
const encryptionAlgoOptions = [
  { value: 'aes-gcm', label: 'aes-gcm' },
  { value: 'aes-256-gcm', label: 'aes-256-gcm' },
  { value: 'chacha20', label: 'chacha20' },
  { value: 'xor', label: 'xor' },
]

/** Direct-connect scheme preference (`flags.default_protocol`); UDP is also sorted ahead of other non-default schemes. */
const defaultProtocolOptions = computed(() => {
  const options = [
    { value: 'tcp', label: 'TCP' },
    { value: 'udp', label: 'UDP' },
  ]
  const current = curNetwork.value.default_protocol
  if (current && !options.some((o) => o.value === current)) {
    options.unshift({ value: current, label: current })
  }
  return options
})

/** Path policy SoT; projects to disable_p2p / prefer_peer_relay. */
const connectionPathTierOptions = computed(() => [
  { value: ConnectionPathTier.DIRECT_FIRST, label: t('connection_path_tier_direct_first') },
  { value: ConnectionPathTier.PREFER_RELAY, label: t('connection_path_tier_prefer_relay') },
  { value: ConnectionPathTier.RELAY_ONLY, label: t('connection_path_tier_relay_only') },
])

function applyPathTier(tier: ConnectionPathTier) {
  const next = applyConnectionPathTier(curNetwork.value, tier)
  curNetwork.value.connection_path_tier = next.connection_path_tier
  curNetwork.value.disable_p2p = next.disable_p2p
  curNetwork.value.prefer_peer_relay = next.prefer_peer_relay
  curNetwork.value.p2p_only = next.p2p_only
}

function onConnectionPathTierChange(tier: ConnectionPathTier | number | null | undefined) {
  if (tier == null || tier === ConnectionPathTier.UNSPECIFIED) {
    return
  }
  const target = tier as ConnectionPathTier
  // Switching to a relay tier clears p2p_only (incompatible with "no relay").
  // Never do that silently — ask first so legacy p2p_only isn't lost by a misclick.
  if (
    curNetwork.value.p2p_only &&
    (target === ConnectionPathTier.PREFER_RELAY || target === ConnectionPathTier.RELAY_ONLY)
  ) {
    confirm.require({
      message: t('p2p_only_relay_conflict_help'),
      header: t('connection_path_tier'),
      icon: 'pi pi-exclamation-triangle',
      rejectProps: {
        label: t('web.common.cancel'),
        severity: 'secondary',
        outlined: true,
      },
      acceptProps: {
        label: t('web.common.confirm'),
      },
      accept: () => applyPathTier(target),
    })
    return
  }
  applyPathTier(target)
}

/** CompressionAlgoPb 取值来自 proto：None = 1、Zstd = 2、Invalid = 0（前端不展示 Invalid，未设置时按 None 处理） */
const dataCompressAlgoOptions = [
  { value: CompressionAlgoPb.None, label: 'none' },
  { value: CompressionAlgoPb.Zstd, label: 'zstd' },
]

const portForwardProtocolOptions = ['tcp', 'udp'] as const

const editingPortForward = ref(false);
const editingPortForwardIndex = ref(-1);
const editingPortForwardData = ref();

function openPortForwardEditor(index: number) {
  editingPortForwardIndex.value = index;
  // deep copy
  editingPortForwardData.value = JSON.parse(JSON.stringify(curNetwork.value.port_forwards[index]));
  editingPortForward.value = true;
}

function addPortForward() {
  addRow(curNetwork.value.port_forwards)
  if (isCompact.value) {
    openPortForwardEditor(curNetwork.value.port_forwards.length - 1)
  }
}

function savePortForward() {
  curNetwork.value.port_forwards[editingPortForwardIndex.value] = editingPortForwardData.value;
  editingPortForward.value = false;
}

const portForwardContainer = ref<HTMLElement | null>(null);
const isCompact = ref(false);
let portForwardResizeObserver: ResizeObserver | undefined;

const UINT64_MAX = (1n << 64n) - 1n

onMounted(() => {
  if (!portForwardContainer.value || typeof ResizeObserver === 'undefined') {
    return
  }

  portForwardResizeObserver = new ResizeObserver(entries => {
    for (const entry of entries) {
      isCompact.value = entry.contentRect.width < 540;
    }
  });
  portForwardResizeObserver.observe(portForwardContainer.value);
});

onUnmounted(() => {
  portForwardResizeObserver?.disconnect();
  portForwardResizeObserver = undefined;
});

function syncNormalizedNetwork(network: NetworkConfig | undefined): void {
  if (!network) {
    return
  }

  Object.assign(network, normalizeNetworkConfig(network))
}

watch(() => curNetwork.value, syncNormalizedNetwork, { immediate: true, deep: false })

function parseInstanceRecvBpsLimitInput(value: string): number | string | null | undefined {
  const trimmed = value.trim()
  if (trimmed.length === 0) {
    return null
  }
  if (!/^\d+$/.test(trimmed)) {
    return undefined
  }

  const limit = BigInt(trimmed)
  if (limit === 0n) {
    return null
  }
  if (limit > UINT64_MAX) {
    return undefined
  }

  return limit <= BigInt(Number.MAX_SAFE_INTEGER) ? Number(limit) : limit.toString()
}

const instanceRecvBpsLimitInput = computed<string>({
  get: () => {
    const limit = curNetwork.value.instance_recv_bps_limit
    return limit == null ? '' : String(limit)
  },
  set: (value) => {
    const limit = parseInstanceRecvBpsLimitInput(value)
    if (limit !== undefined) {
      curNetwork.value.instance_recv_bps_limit = limit
    }
  },
})

function defaultVpnPortalConfig(): VpnPortalConfig {
  return {
    wireguard_listen: '0.0.0.0:22022',
    clients: [],
  }
}

const vpnPortalEnabled = computed({
  get: () => curNetwork.value.vpn_portal_config !== undefined,
  set: (enabled: boolean) => {
    curNetwork.value.vpn_portal_config = enabled ? defaultVpnPortalConfig() : undefined
  },
})

const vpnPortalConfig = computed(() => curNetwork.value.vpn_portal_config ?? defaultVpnPortalConfig())

const vpnPortalPrivateKey = computed({
  get: () => vpnPortalConfig.value.wireguard_private_key ?? '',
  set: (value: string | null | undefined) => {
    vpnPortalConfig.value.wireguard_private_key = value && value.length > 0 ? value : undefined
  },
})

const vpnPortalGroupOptions = computed(() => (
  curNetwork.value.acl?.acl_v1?.group?.declares ?? []
).map((group) => group.group_name))

const vpnPortalClientViewKeys = new WeakMap<VpnPortalClientConfig, string>()

function vpnPortalClientViewKey(client: VpnPortalClientConfig): string {
  let key = vpnPortalClientViewKeys.get(client)
  if (!key) {
    key = uuidv4()
    vpnPortalClientViewKeys.set(client, key)
  }
  return key
}

function addVpnPortalClient() {
  vpnPortalConfig.value.clients.push({ name: '', virtual_ip: '', groups: [] })
}

function removeVpnPortalClient(index: number) {
  vpnPortalConfig.value.clients.splice(index, 1)
}
</script>

<template>
  <div class="flex flex-col h-full">
      <div class="config-panels w-full self-center">
          <Panel v-model:collapsed="panelCollapsed.basic" :header="t('basic_settings')" toggleable
            :pt="panelHeaderPt('basic')">
            <div class="flex flex-col gap-y-2">
              <div class="flex flex-row gap-x-9 flex-wrap">
                <div class="flex flex-col gap-2 basis-5/12 grow">
                  <div class="flex items-center" for="virtual_ip">
                    <label class="mr-2"> {{ t('virtual_ipv4') }} </label>
                    <Checkbox v-model="curNetwork.dhcp" input-id="virtual_ip_auto" :binary="true" />

                    <label for="virtual_ip_auto" class="ml-2">
                      {{ t('virtual_ipv4_dhcp') }}
                    </label>
                  </div>
                  <InputGroup>
                    <InputText id="virtual_ip" v-model="curNetwork.virtual_ipv4" :disabled="curNetwork.dhcp"
                      aria-describedby="virtual_ipv4-help" />
                    <InputGroupAddon>
                      <span>/</span>
                    </InputGroupAddon>
                    <InputNumber v-model="curNetwork.network_length" :disabled="curNetwork.dhcp"
                      inputId="horizontal-buttons" showButtons :step="1" mode="decimal" :min="1" :max="32" fluid
                      class="max-w-20" />
                  </InputGroup>
                </div>
              </div>

              <div class="flex flex-row gap-x-9 flex-wrap">
                <div class="flex flex-col gap-2 basis-5/12 grow">
                  <label for="network_name">{{ t('network_name') }}</label>
                  <InputText id="network_name" v-model="curNetwork.network_name" aria-describedby="network_name-help" />
                </div>
                <div class="flex flex-col gap-2 basis-5/12 grow">
                  <label for="network_secret">{{ t('network_secret') }}</label>
                  <InputGroup class="network-secret-group">
                    <Password id="network_secret" v-model="curNetwork.network_secret"
                      aria-describedby="network_secret-help" toggleMask :feedback="false" fluid />
                    <InputGroupAddon>
                      <Button type="button" icon="pi pi-key" severity="secondary" text
                        class="network-secret-generate-btn"
                        :aria-label="t('network_secret_generate')"
                        v-tooltip.top="t('network_secret_generate')"
                        @click="generateAndCopyNetworkSecret" />
                    </InputGroupAddon>
                  </InputGroup>
                </div>
              </div>

              <div class="flex flex-row gap-x-9 flex-wrap">
                <div class="flex flex-col gap-2 basis-5/12 grow">
                  <div class="flex items-center">
                    <label for="initial_nodes">{{ t('initial_nodes') }}</label>
                    <i class="pi pi-question-circle config-help-tip ml-2" tabindex="0" v-tooltip.top="{ value: t('initial_nodes_help'), escape: false }" role="img"></i>
                  </div>
                  <div class="items-center flex flex-col p-fluid gap-y-2">
                    <UrlListInput id="initial_nodes" v-model="curNetwork.peer_urls" :protos="protos"
                      defaultUrl="tcp://:11010" :add-label="t('add_initial_node')"
                      :placeholder="t('initial_node_placeholder')" />
                  </div>
                </div>
              </div>

              <div class="flex flex-row gap-x-9 flex-wrap">
                <div class="flex flex-col gap-2 basis-5/12 grow">
                  <div class="flex items-center">
                    <label for="connection_path_tier">{{ t('connection_path_tier') }}</label>
                    <i class="pi pi-question-circle config-help-tip ml-2" tabindex="0"
                      v-tooltip.top="{ value: t('connection_path_tier_help'), escape: false }" role="img"></i>
                  </div>
                  <Select
                    id="connection_path_tier"
                    :model-value="curNetwork.connection_path_tier ?? ConnectionPathTier.DIRECT_FIRST"
                    :options="connectionPathTierOptions"
                    option-label="label"
                    option-value="value"
                    fluid
                    class="et-select"
                    @update:model-value="onConnectionPathTierChange"
                  />
                </div>
              </div>
            </div>
          </Panel>

          <Panel v-model:collapsed="panelCollapsed.advanced" :header="t('advanced_settings')" toggleable
            :pt="panelHeaderPt('advanced')">
            <div class="flex flex-col gap-y-2">

              <div class="advanced-flags-section">
                <div class="advanced-flags-heading">{{ t('flags_switch') }}</div>
                <div
                  v-if="conflictWarnings.length"
                  class="config-conflict-banners flex flex-col gap-2"
                  role="status"
                >
                  <p
                    v-for="warning in conflictWarnings"
                    :key="warning.code"
                    class="config-conflict-banner m-0"
                    :class="warning.severity === 'warn' ? 'config-conflict-banner--warn' : 'config-conflict-banner--info'"
                  >
                    {{ t(warning.i18nKey) }}
                  </p>
                </div>
                <div class="advanced-flag-groups">
                  <section v-for="group in advancedFlagGroups" :key="group.key" class="advanced-flag-group">
                    <h3 class="advanced-group-title">
                      <i :class="group.icon" aria-hidden="true"></i>
                      {{ t(group.titleKey) }}
                    </h3>
                    <div class="advanced-flags-grid">
                      <div
                        v-for="flag in group.flags"
                        :key="flag.field"
                        class="advanced-flag-item"
                        :class="{ 'advanced-flag-item--disabled': flag.disabled }"
                      >
                        <Checkbox
                          v-model="curNetwork[flag.field]"
                          :input-id="flag.field"
                          :binary="true"
                          :disabled="flag.disabled"
                        />
                        <label :for="flag.field">{{ t(flag.field) }}</label>
                        <i class="pi pi-question-circle config-help-tip" tabindex="0"
                          v-tooltip.top="{ value: t(flag.helpKey), escape: false }"
                          :aria-label="t(flag.helpKey)" role="img"></i>
                      </div>
                    </div>
                  </section>
                </div>
              </div>

              <div class="config-compact-grid">
                <div class="config-compact-field">
                  <div class="config-compact-label flex items-center gap-1">
                    <label for="hostname">{{ t('hostname') }}</label>
                    <i class="pi pi-question-circle config-help-tip" tabindex="0"
                      v-tooltip.top="{ value: t('hostname_help'), escape: false }" role="img"></i>
                  </div>
                  <InputText id="hostname" v-model="curNetwork.hostname" aria-describedby="hostname-help"
                    :format="true" :placeholder="t('hostname_placeholder', [props.hostname])" fluid />
                </div>

                <div class="config-compact-field">
                  <label for="dev_name" class="config-compact-label">{{ t('dev_name') }}</label>
                  <InputText id="dev_name" v-model="curNetwork.dev_name" aria-describedby="dev_name-help"
                    :format="true" :placeholder="t('dev_name_placeholder')" fluid
                    :disabled="tunControlsDisabled" />
                  <p v-if="tunControlsDisabled" class="config-field-hint m-0">{{ t('no_tun_dev_mtu_hint') }}</p>
                </div>

                <div class="config-compact-field">
                  <div class="config-compact-label flex items-center gap-1">
                    <label for="mtu">{{ t('mtu') }}</label>
                    <i class="pi pi-question-circle config-help-tip" tabindex="0"
                      v-tooltip.top="{ value: t('mtu_help'), escape: false }" role="img"></i>
                  </div>
                  <InputNumber id="mtu" v-model="curNetwork.mtu" aria-describedby="mtu-help" :format="false"
                    :placeholder="t('mtu_placeholder')" :min="400" :max="1380" fluid
                    :disabled="tunControlsDisabled" />
                </div>

                <div class="config-compact-field">
                  <div class="config-compact-label flex items-center gap-1">
                    <label for="instance_recv_bps_limit">{{ t('instance_recv_bps_limit') }}</label>
                    <i class="pi pi-question-circle config-help-tip" tabindex="0"
                      v-tooltip.top="{ value: t('instance_recv_bps_limit_help'), escape: false }" role="img"></i>
                  </div>
                  <InputText id="instance_recv_bps_limit" v-model="instanceRecvBpsLimitInput"
                    aria-describedby="instance_recv_bps_limit-help" inputmode="numeric" pattern="[0-9]*"
                    :placeholder="t('instance_recv_bps_limit_placeholder')" fluid />
                </div>
              </div>

              <div class="flex flex-col gap-2">
                <div class="config-inline-field">
                  <div class="config-inline-label flex items-center gap-1">
                    <label for="subnet-proxy">{{ t('proxy_cidrs') }}</label>
                    <i class="pi pi-question-circle config-help-tip" tabindex="0"
                      v-tooltip.top="{ value: t('proxy_cidrs_help'), escape: false }" role="img"></i>
                  </div>
                  <div class="config-inline-control">
                    <AutoComplete id="subnet-proxy" v-model="curNetwork.proxy_cidrs"
                      :placeholder="t('chips_placeholder', ['10.0.0.0/24'])" multiple fluid
                      :suggestions="inetSuggestions" @complete="searchInetSuggestions" />
                  </div>
                </div>

                <div class="config-inline-field">
                  <div class="config-inline-label flex items-center gap-1">
                    <label for="exit_nodes">{{ t('exit_nodes') }}</label>
                    <i class="pi pi-question-circle config-help-tip" tabindex="0"
                      v-tooltip.top="{ value: t('exit_nodes_help'), escape: false }" role="img"></i>
                  </div>
                  <div class="config-inline-control">
                    <AutoComplete id="exit_nodes" v-model="curNetwork.exit_nodes"
                      :placeholder="t('chips_placeholder', ['192.168.8.8'])" multiple fluid
                      :suggestions="exitNodesSuggestions" @complete="searchExitNodesSuggestions" />
                    <p v-if="tunControlsDisabled" class="config-field-hint m-0 mt-1">{{ t('no_tun_exit_nodes_hint') }}</p>
                  </div>
                </div>
              </div>

              <div class="flex flex-col gap-2">
                <div class="config-inline-field config-inline-field--top">
                  <div class="config-inline-label flex items-center gap-1">
                    <label for="listener_urls">{{ t('listener_urls') }}</label>
                    <i class="pi pi-question-circle config-help-tip" tabindex="0"
                      v-tooltip.top="{ value: t('listener_urls_help'), escape: false }" role="img"></i>
                  </div>
                  <div class="config-inline-control">
                    <UrlListInput id="listener_urls" v-model="curNetwork.listener_urls" :protos="listenerProtos"
                      :add-label="t('add_listener_url')" placeholder="0.0.0.0" />
                  </div>
                </div>

                <div class="config-inline-field config-inline-field--top">
                  <div class="config-inline-label flex items-center gap-1">
                    <label for="mapped_listeners">{{ t('mapped_listeners') }}</label>
                    <i class="pi pi-question-circle config-help-tip" tabindex="0"
                      v-tooltip.top="{ value: t('mapped_listeners_help'), escape: false }" role="img"></i>
                  </div>
                  <div class="config-inline-control">
                    <UrlListInput id="mapped_listeners" v-model="curNetwork.mapped_listeners" :protos="protos"
                      :add-label="t('add_mapped_listener')" />
                  </div>
                </div>
              </div>

              <div class="flex flex-col gap-2">
                <div class="config-inline-field">
                  <label class="config-inline-label">VPN Portal</label>
                  <div class="config-inline-control">
                    <ToggleButton v-model="vpnPortalEnabled" on-icon="pi pi-check" off-icon="pi pi-times"
                      :on-label="t('off_text')" :off-label="t('on_text')" class="w-48" />
                  </div>
                </div>
                <div v-if="vpnPortalEnabled" class="config-inline-expand config-inline-expand--flush flex flex-col gap-3">
                  <div class="flex flex-col gap-3 md:flex-row md:gap-4">
                    <div class="flex flex-col gap-1 flex-1 min-w-0">
                      <label for="vpn_portal_wireguard_listen">{{ t('vpn_portal_wireguard_listen') }}</label>
                      <InputText id="vpn_portal_wireguard_listen" v-model="vpnPortalConfig.wireguard_listen"
                        :placeholder="t('vpn_portal_wireguard_listen_placeholder')" fluid />
                    </div>
                    <div class="flex flex-col gap-1 flex-1 min-w-0">
                      <label for="vpn_portal_wireguard_private_key">{{ t('vpn_portal_wireguard_private_key') }}</label>
                      <Password id="vpn_portal_wireguard_private_key" v-model="vpnPortalPrivateKey"
                        :placeholder="t('vpn_portal_wireguard_private_key_placeholder')"
                        toggleMask :feedback="false" fluid />
                    </div>
                  </div>

                  <div class="flex items-center justify-between gap-3">
                    <label>{{ t('vpn_portal_clients') }}</label>
                    <Button icon="pi pi-plus" :label="t('vpn_portal_add_client')" severity="secondary" size="small"
                      outlined :disabled="vpnPortalConfig.clients.length >= 64"
                      @click="addVpnPortalClient" />
                  </div>

                  <div v-if="vpnPortalConfig.clients.length === 0"
                    class="text-sm text-surface-500 dark:text-surface-400">
                    {{ t('vpn_portal_no_clients') }}
                  </div>
                  <div v-for="(client, index) in vpnPortalConfig.clients" :key="vpnPortalClientViewKey(client)"
                    class="flex flex-row gap-3 flex-wrap items-end rounded border border-surface-200 dark:border-surface-700 p-3">
                    <div class="flex flex-col gap-2 grow basis-3/12">
                      <label :for="`vpn_portal_client_name_${index}`">{{ t('vpn_portal_client_name') }}</label>
                      <InputText :id="`vpn_portal_client_name_${index}`" v-model="client.name"
                        :placeholder="t('vpn_portal_client_name_placeholder')" />
                    </div>
                    <div class="flex flex-col gap-2 grow basis-3/12">
                      <label :for="`vpn_portal_client_virtual_ip_${index}`">{{ t('vpn_portal_client_virtual_ip') }}</label>
                      <InputText :id="`vpn_portal_client_virtual_ip_${index}`" v-model="client.virtual_ip"
                        :placeholder="t('vpn_portal_client_virtual_ip_placeholder')" />
                    </div>
                    <div class="flex flex-col gap-2 grow basis-4/12">
                      <label :for="`vpn_portal_client_groups_${index}`">{{ t('vpn_portal_client_groups') }}</label>
                      <MultiSelect :input-id="`vpn_portal_client_groups_${index}`" v-model="client.groups"
                        :options="vpnPortalGroupOptions" appendTo="self" filter fluid
                        :placeholder="t('vpn_portal_client_groups_placeholder')" />
                    </div>
                    <Button icon="pi pi-trash" severity="danger" text rounded class="et-icon-action-btn"
                      :aria-label="t('vpn_portal_remove_client')" @click="removeVpnPortalClient(index)" />
                  </div>
                </div>

                <div class="config-inline-field">
                  <div class="config-inline-label flex items-center gap-1">
                    <label for="relay_network_whitelist">{{ t('relay_network_whitelist') }}</label>
                    <i class="pi pi-question-circle config-help-tip" tabindex="0"
                      v-tooltip.top="{ value: t(relayControlsDisabled ? 'p2p_only_blocks_relay_help' : 'relay_network_whitelist_help'), escape: false }" role="img"></i>
                  </div>
                  <div class="config-inline-control">
                    <ToggleButton v-model="curNetwork.enable_relay_network_whitelist" on-icon="pi pi-check"
                      off-icon="pi pi-times" :on-label="t('off_text')" :off-label="t('on_text')" class="w-48"
                      :disabled="relayControlsDisabled && !curNetwork.enable_relay_network_whitelist" />
                  </div>
                </div>
                <div v-if="curNetwork.enable_relay_network_whitelist" class="config-inline-expand">
                  <AutoComplete id="relay_network_whitelist" v-model="curNetwork.relay_network_whitelist"
                    :placeholder="t('relay_network_whitelist')" multiple fluid
                    :disabled="relayControlsDisabled"
                    :suggestions="whitelistSuggestions" @complete="searchWhitelistSuggestions" />
                </div>

                <div class="config-inline-field">
                  <div class="config-inline-label flex items-center gap-1">
                    <label for="routes">{{ t('manual_routes') }}</label>
                    <i class="pi pi-question-circle config-help-tip" tabindex="0"
                      v-tooltip.top="{ value: t('manual_routes_help'), escape: false }" role="img"></i>
                  </div>
                  <div class="config-inline-control">
                    <ToggleButton v-model="curNetwork.enable_manual_routes" on-icon="pi pi-check" off-icon="pi pi-times"
                      :on-label="t('off_text')" :off-label="t('on_text')" class="w-48" />
                  </div>
                </div>
                <div v-if="curNetwork.enable_manual_routes" class="config-inline-expand">
                  <AutoComplete id="routes" v-model="curNetwork.routes"
                    :placeholder="t('chips_placeholder', ['192.168.0.0/16'])" multiple fluid
                    :suggestions="inetSuggestions" @complete="searchInetSuggestions" />
                </div>

                <div class="config-inline-field">
                  <div class="config-inline-label flex items-center gap-1">
                    <label for="socks5_port">{{ t('socks5') }}</label>
                    <i class="pi pi-question-circle config-help-tip" tabindex="0"
                      v-tooltip.top="{ value: t('socks5_help'), escape: false }" role="img"></i>
                  </div>
                  <div class="config-inline-control">
                    <ToggleButton v-model="curNetwork.enable_socks5" on-icon="pi pi-check" off-icon="pi pi-times"
                      :on-label="t('off_text')" :off-label="t('on_text')" class="w-48" />
                  </div>
                </div>
                <div v-if="curNetwork.enable_socks5" class="config-inline-expand">
                  <InputNumber id="socks5_port" v-model="curNetwork.socks5_port" aria-describedby="rpc_port-help"
                    :format="false" :allow-empty="false" :min="0" :max="65535" fluid />
                </div>

                <div v-if="!ipv6ControlsHidden" class="config-inline-field">
                  <div class="config-inline-label flex items-center gap-1">
                    <label for="ipv6_public_addr_provider">{{ t('ipv6_public_addr_provider') }}</label>
                    <i class="pi pi-question-circle config-help-tip" tabindex="0"
                      v-tooltip.top="{ value: t('ipv6_public_addr_provider_help'), escape: false }" role="img"></i>
                  </div>
                  <div class="config-inline-control">
                    <ToggleButton v-model="curNetwork.ipv6_public_addr_provider" on-icon="pi pi-check"
                      off-icon="pi pi-times" :on-label="t('off_text')" :off-label="t('on_text')" class="w-48" />
                  </div>
                </div>
                <div v-if="!ipv6ControlsHidden && curNetwork.ipv6_public_addr_provider" class="config-inline-expand">
                  <InputText id="ipv6_public_addr_prefix" v-model="curNetwork.ipv6_public_addr_prefix"
                    :placeholder="t('ipv6_public_addr_prefix_placeholder')" fluid
                    aria-describedby="ipv6_public_addr_prefix-help" />
                </div>

                <div class="config-compact-grid">
                  <div class="config-compact-field">
                    <div class="config-compact-label flex items-center gap-1">
                      <label for="default_protocol">{{ t('default_protocol') }}</label>
                      <i class="pi pi-question-circle config-help-tip" tabindex="0"
                        v-tooltip.top="{ value: t('default_protocol_help'), escape: false }" role="img"></i>
                    </div>
                    <Select id="default_protocol" v-model="curNetwork.default_protocol"
                      :options="defaultProtocolOptions" option-label="label" option-value="value" fluid
                      class="et-select" />
                  </div>

                  <div class="config-compact-field">
                    <div class="config-compact-label flex items-center gap-1">
                      <label for="encryption_algorithm">{{ t('encryption_algorithm') }}</label>
                      <i class="pi pi-question-circle config-help-tip" tabindex="0"
                        v-tooltip.top="{ value: t(encryptionAlgoHidden ? 'disable_encryption_algo_conflict_help' : 'encryption_algorithm_help'), escape: false }" role="img"></i>
                    </div>
                    <Select
                      v-if="!encryptionAlgoHidden"
                      id="encryption_algorithm"
                      v-model="curNetwork.encryption_algorithm"
                      :options="encryptionAlgoOptions"
                      option-label="label"
                      option-value="value"
                      fluid
                      class="et-select"
                      :placeholder="t('encryption_algorithm_placeholder')"
                    />
                    <p v-else class="config-field-hint m-0">{{ t('disable_encryption_algo_hint') }}</p>
                  </div>

                  <div class="config-compact-field">
                    <div class="config-compact-label flex items-center gap-1">
                      <label for="data_compress_algo">{{ t('data_compress_algo') }}</label>
                      <i class="pi pi-question-circle config-help-tip" tabindex="0"
                        v-tooltip.top="{ value: t('data_compress_algo_help'), escape: false }" role="img"></i>
                    </div>
                    <Select id="data_compress_algo" v-model="curNetwork.data_compress_algo"
                      :options="dataCompressAlgoOptions" option-label="label" option-value="value" fluid
                      class="et-select" />
                  </div>

                  <div class="config-compact-field">
                    <div class="config-compact-label flex items-center gap-1">
                      <label for="socket_mark">{{ t('socket_mark') }}</label>
                      <i class="pi pi-question-circle config-help-tip" tabindex="0"
                        v-tooltip.top="{ value: t('socket_mark_help'), escape: false }" role="img"></i>
                    </div>
                    <InputNumber id="socket_mark" v-model="curNetwork.socket_mark" :format="false" fluid
                      :allow-empty="true" :min="0" :max="4294967295" :placeholder="t('socket_mark_placeholder')" />
                  </div>
                </div>
              </div>

            </div>
          </Panel>

          <Panel v-model:collapsed="panelCollapsed.portForwards" :header="t('port_forwards')" toggleable
            :pt="panelHeaderPt('portForwards')">
            <div ref="portForwardContainer" class="flex flex-col gap-y-2">
              <div class="flex flex-row gap-x-9 flex-wrap w-full">
                <div class="flex flex-col gap-2 grow p-fluid">
                  <div class="flex items-center gap-1">
                    <label for="port_forwards">{{ t('port_forwards') }}</label>
                    <i
                      class="pi pi-question-circle config-help-tip" tabindex="0"
                      v-tooltip.top="t('port_forwards_help')"
                      :aria-label="t('port_forwards_help')"
                      role="img"
                    />
                  </div>
                  <div v-for="(row, index) in curNetwork.port_forwards" :key="index" class="form-row">
                    <!-- Wide screen view -->
                    <div v-if="!isCompact" class="flex gap-2 items-center">
                      <div style="flex-grow: 5;">
                        <InputGroup>
                          <Select :input-id="`port_forward_proto_${index}`" v-model="row.proto"
                            :options="[...portForwardProtocolOptions]" class="et-proto-select" />
                          <InputText v-model="row.bind_ip" :placeholder="t('port_forwards_bind_addr')" />
                          <InputGroupAddon>
                            <span style="font-weight: bold">:</span>
                          </InputGroupAddon>
                          <InputNumber v-model="row.bind_port" :format="false" inputId="horizontal-buttons" :step="1"
                            mode="decimal" :min="1" :max="65535" fluid class="max-w-20" />
                        </InputGroup>
                      </div>
                      <div style="flex-grow: 4;">
                        <InputGroup>
                          <InputText v-model="row.dst_ip" :placeholder="t('port_forwards_dst_addr')" />
                          <InputGroupAddon>
                            <span style="font-weight: bold">:</span>
                          </InputGroupAddon>
                          <InputNumber v-model="row.dst_port" :format="false" inputId="horizontal-buttons" :step="1"
                            mode="decimal" :min="1" :max="65535" fluid class="max-w-20" />
                        </InputGroup>
                      </div>
                      <div class="flex gap-1 items-center" style="flex-grow: 1;">
                        <Button icon="pi pi-pencil" severity="secondary" text rounded class="et-icon-action-btn"
                          @click="openPortForwardEditor(index)" />
                        <Button icon="pi pi-trash" severity="danger" text rounded class="et-icon-action-btn"
                          @click="removeRow(index, curNetwork.port_forwards)" />
                      </div>
                    </div>
                    <!-- Small screen view -->
                    <div v-else class="flex justify-between items-center p-2 border-b border-surface">
                      <span>{{ row.proto }}://{{ row.bind_ip }}:{{ row.bind_port }}/{{ row.dst_ip }}:{{
                        row.dst_port }}</span>
                      <div class="flex gap-1">
                        <Button icon="pi pi-pencil" severity="secondary" text rounded class="et-icon-action-btn"
                          @click="openPortForwardEditor(index)" />
                        <Button icon="pi pi-trash" severity="danger" text rounded class="et-icon-action-btn"
                          @click="removeRow(index, curNetwork.port_forwards)" />
                      </div>
                    </div>
                  </div>

                  <div class="flex justify-start mt-4">
                    <Button class="et-panel-action-btn" icon="pi pi-plus" :label="t('port_forwards_add_btn')"
                      severity="success" v-tooltip.top="t('port_forwards_add_tip')"
                      @click="addPortForward" />
                  </div>

                  <Dialog v-model:visible="editingPortForward" modal :header="t('edit_port_forward')"
                    class="et-dialog"
                    :style="{ width: '90vw', maxWidth: '600px' }">
                    <div v-if="editingPortForwardData" class="flex flex-col gap-4">
                      <InputGroup>
                        <Select id="edit_port_forward_proto" v-model="editingPortForwardData.proto"
                          :options="[...portForwardProtocolOptions]" class="et-proto-select" />
                        <InputText v-model="editingPortForwardData.bind_ip"
                          :placeholder="t('port_forwards_bind_addr')" />
                        <InputGroupAddon>
                          <span style="font-weight: bold">:</span>
                        </InputGroupAddon>
                        <InputNumber v-model="editingPortForwardData.bind_port" :format="false" :step="1" mode="decimal"
                          :min="1" :max="65535" class="max-w-20" />
                      </InputGroup>
                      <InputGroup>
                        <InputText v-model="editingPortForwardData.dst_ip" :placeholder="t('port_forwards_dst_addr')" />
                        <InputGroupAddon>
                          <span style="font-weight: bold">:</span>
                        </InputGroupAddon>
                        <InputNumber v-model="editingPortForwardData.dst_port" :format="false" :step="1" mode="decimal"
                          :min="1" :max="65535" class="max-w-20" />
                      </InputGroup>
                    </div>
                    <template #footer>
                      <Button :label="t('web.common.cancel')" icon="pi pi-times" severity="secondary" outlined
                        @click="editingPortForward = false" />
                      <Button :label="t('web.common.save')" icon="pi pi-save" @click="savePortForward" />
                    </template>
                  </Dialog>
                </div>
              </div>
            </div>
          </Panel>

          <Panel v-model:collapsed="panelCollapsed.dns" toggleable
            :pt="panelHeaderPt('dns')">
            <template #header>
              <div class="flex items-center gap-2 min-w-0">
                <span>{{ t('dns.title') }}</span>
                <i
                  v-if="!curNetwork.enable_magic_dns"
                  class="pi pi-question-circle config-help-tip" tabindex="0"
                  v-tooltip.top="t('dns.hosts.magic_dns_off_hint')"
                  :aria-label="t('dns.hosts.magic_dns_off_hint')"
                  role="img"
                />
              </div>
            </template>
            <div class="flex flex-col gap-y-4">
              <div class="dns-section">
                <div class="dns-section__title dns-section__title--control">
                  <Checkbox v-model="curNetwork.enable_magic_dns" input-id="enable_magic_dns" :binary="true" />
                  <label for="enable_magic_dns">{{ t('enable_magic_dns') }}</label>
                  <i
                    class="pi pi-question-circle config-help-tip"
                    tabindex="0"
                    v-tooltip.top="{ value: t('enable_magic_dns_help'), escape: false }"
                    :aria-label="t('enable_magic_dns_help')"
                    role="img"
                  />
                </div>
                <p v-if="tunControlsDisabled" class="config-field-hint m-0">{{ t('no_tun_magic_dns_hint') }}</p>
              </div>

              <p
                v-if="curNetwork.dns_config && (curNetwork.dns_config.hosts?.length || curNetwork.dns_config.forwarders?.length || curNetwork.dns_config.upstream_dns?.length)"
                class="dns-mixed-version-hint et-meta m-0"
                role="status"
              >
                {{ t('dns.mixed_version_warning') }}
              </p>

              <template v-if="curNetwork.dns_config">
                <div class="dns-section">
                  <div class="dns-section__title">
                    <span>{{ t('dns.hosts.title') }}</span>
                    <i
                      class="pi pi-question-circle config-help-tip" tabindex="0"
                      v-tooltip.top="t('dns.hosts.editor_help')"
                      :aria-label="t('dns.hosts.editor_help')"
                      role="img"
                    />
                  </div>
                  <DnsHostsEditor v-model:hosts="curNetwork.dns_config.hosts" />
                </div>

                <div class="dns-section">
                  <div class="dns-section__title">
                    <span>{{ t('dns.forwarders.title') }}</span>
                    <i
                      class="pi pi-question-circle config-help-tip" tabindex="0"
                      v-tooltip.top="t('dns.forwarders.editor_help')"
                      :aria-label="t('dns.forwarders.editor_help')"
                      role="img"
                    />
                  </div>
                  <DnsForwardersEditor v-model:forwarders="curNetwork.dns_config.forwarders" />
                </div>

                <div class="dns-section">
                  <div class="dns-section__title">
                    <span>{{ t('dns.upstream.title') }}</span>
                    <i
                      class="pi pi-question-circle config-help-tip" tabindex="0"
                      v-tooltip.top="{ value: t('dns.upstream.help'), escape: false }"
                      :aria-label="t('dns.upstream.help')"
                      role="img"
                    />
                  </div>
                  <DnsUpstreamEditor v-model:servers="curNetwork.dns_config.upstream_dns" />
                </div>
              </template>
              <div v-else class="flex justify-start">
                <Button class="et-panel-action-btn" icon="pi pi-plus" :label="t('dns.hosts.enable')"
                  severity="success" v-tooltip.top="t('dns.hosts.enable_tip')" @click="initDnsHosts" />
              </div>
            </div>
          </Panel>

          <Panel v-model:collapsed="panelCollapsed.acl" :header="t('acl.title')" toggleable
            :pt="panelHeaderPt('acl')">
            <div v-if="curNetwork.acl" class="flex flex-col gap-y-2">
              <AclManager v-model="curNetwork.acl" />
            </div>
            <div v-else class="flex justify-start">
              <Button class="et-panel-action-btn" :label="t('acl.enabled')" severity="success"
                v-tooltip.top="t('acl.enabled_tip')"
                @click="curNetwork.acl = { acl_v1: { chains: [], group: { declares: [], members: [] } } }" />
            </div>
          </Panel>

          <div v-if="!hideRunButton" class="config-run-actions flex justify-center gap-3 pt-3">
            <Button class="network-footer-btn" :label="actionLabel || t('run_network')" icon="pi pi-arrow-right"
              icon-pos="right" :disabled="configInvalid" @click="$emit('runNetwork', curNetwork)" />
          </div>
      </div>
  </div>
</template>

<style scoped>
.dns-mixed-version-hint {
  color: var(--et-warning, #b45309);
  background: color-mix(in srgb, var(--et-warning, #f59e0b) 12%, transparent);
  border-radius: 0.375rem;
  padding: 0.5rem 0.75rem;
}

.dns-section {
  display: flex;
  flex-direction: column;
  gap: 0.5rem;
  /* Nested under the DNS Strategy panel header — one visual step down. */
  padding-left: 0.25rem;
}

.dns-section__title {
  display: inline-flex;
  align-items: center;
  gap: 0.35rem;
  /* Below panel title (0.875rem / primary); peer titles stay identical. */
  font-size: 0.8125rem;
  font-weight: 600;
  line-height: 1.2;
  color: var(--text-color-secondary, #64748b);
}

.dns-section__title label,
.dns-section__title span {
  margin: 0;
  font: inherit;
  color: inherit;
}

.dns-section__title--control {
  gap: 0.5rem;
}

.config-panels {
  display: flex;
  flex-direction: column;
  gap: 0.5rem;
}

.config-panels :deep(.p-panel) {
  border: 1px solid var(--et-border-color, #e2e8f0);
  border-radius: var(--et-radius, 0.75rem);
  background: var(--surface-card, #ffffff);
  box-shadow: none;
  overflow: hidden;
}

/* 面板标题栏紧凑化（与 Status.vue 保持一致） */
.config-panels :deep(.p-panel .p-panel-header) {
  padding: 0.35rem 0.75rem !important;
  min-height: 2rem !important;
  font-size: var(--et-fs-body, 0.875rem);
  font-weight: 600;
  line-height: 1.2;
  color: var(--text-color, #1e293b);
  background: transparent;
  border: none;
}

.config-panels :deep(.p-panel .p-panel-header.cursor-pointer) {
  min-height: 2rem !important;
}

.config-panels :deep(.p-panel .p-panel-content) {
  padding: 0.55rem 0.75rem !important;
  border: none;
  background: transparent;
  font-size: var(--et-fs-body, 0.875rem);
  color: var(--text-color, #1e293b);
}

.config-panels :deep(.p-panel .p-panel-header .p-panel-title),
.config-panels :deep(.p-panel .p-panel-header span:not(.p-badge)) {
  font-size: var(--et-fs-body, 0.875rem);
  font-weight: 600;
  line-height: 1.2;
  color: var(--text-color, #1e293b);
}

.network-footer-btn {
  width: var(--et-btn-w, 10rem);
  min-width: var(--et-btn-w, 10rem);
  height: var(--et-btn, 2.5rem) !important;
  padding: 0 0.9rem !important;
  font-size: var(--et-fs-body, 0.875rem) !important;
  font-weight: 600 !important;
  justify-content: center;
  box-sizing: border-box;
}

/* 高级开关按使用场景分组，避免 28 个选项堆成一片 */
.advanced-flags-section {
  display: flex;
  flex-direction: column;
  gap: 0.75rem;
}

.advanced-flags-heading {
  color: var(--text-color-secondary, #64748b);
  font-size: 0.75rem;
  font-weight: 700;
  letter-spacing: 0.04em;
  text-transform: uppercase;
}

.advanced-flag-groups {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 0.75rem;
}

.advanced-flag-group {
  min-width: 0;
  padding: 0.75rem;
  border: 1px solid var(--et-border-color, #e2e8f0);
  border-radius: calc(var(--et-radius, 0.75rem) - 0.125rem);
  background: var(--surface-50, #f8fafc);
}

.advanced-group-title {
  display: flex;
  align-items: center;
  gap: 0.45rem;
  margin: 0 0 0.625rem;
  color: var(--text-color, #1e293b);
  font-size: 0.8125rem;
  font-weight: 700;
}

.advanced-group-title i {
  color: var(--primary-color, var(--et-primary, #0ea5e9));
}

.advanced-flags-grid {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(min(100%, 14rem), 1fr));
  gap: 0.4rem 0.75rem;
}

.advanced-flag-item {
  display: flex;
  align-items: center;
  min-width: 0;
  min-height: 2.15rem;
  padding: 0.3rem 0.5rem;
  border: 1px solid transparent;
  border-radius: 0.5rem;
  background: var(--surface-0, #ffffff);
  transition: background-color 0.15s ease, border-color 0.15s ease;
}

.advanced-flag-item:has(.p-checkbox-checked),
.advanced-flag-item:has(.p-highlight) {
  border-color: color-mix(in srgb, var(--primary-color, #0ea5e9) 28%, var(--et-border-color, #e2e8f0));
  background: color-mix(in srgb, var(--primary-color, #0ea5e9) 7%, #ffffff);
}

@media (hover: hover) {
  .advanced-flag-item:hover {
    background: var(--surface-hover, #eef2f7);
    border-color: var(--et-border-color, #e2e8f0);
  }

  .advanced-flag-item:has(.p-checkbox-checked):hover,
  .advanced-flag-item:has(.p-highlight):hover {
    background: color-mix(in srgb, var(--primary-color, #0ea5e9) 11%, #ffffff);
  }
}

.advanced-flag-item label {
  min-width: 0;
  margin-left: 0.55rem;
  line-height: 1.3;
  cursor: pointer;
  color: var(--text-color, #1e293b);
  font-size: 0.8125rem;
}

.advanced-flag-item--disabled {
  opacity: 0.55;
}

.advanced-flag-item--disabled label {
  cursor: not-allowed;
}

.config-conflict-banner {
  font-size: 0.8125rem;
  line-height: 1.4;
  border-radius: 0.375rem;
  padding: 0.5rem 0.75rem;
}

.config-conflict-banner--warn {
  color: var(--et-warning, #b45309);
  background: color-mix(in srgb, var(--et-warning, #f59e0b) 12%, transparent);
}

.config-conflict-banner--info {
  color: var(--text-color-secondary, #64748b);
  background: color-mix(in srgb, var(--surface-200, #e2e8f0) 60%, transparent);
}

.config-compact-grid {
  display: grid;
  grid-template-columns: repeat(4, minmax(0, 1fr));
  gap: 0.75rem 1rem;
  min-width: 0;
}

.config-compact-field {
  display: flex;
  flex-direction: column;
  gap: 0.35rem;
  min-width: 0;
}

.config-compact-label {
  min-height: 1.25rem;
  color: var(--text-color, #1e293b);
  font-size: 0.8125rem;
  font-weight: 500;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.config-field-hint {
  color: var(--text-color-secondary, #64748b);
  font-size: 0.75rem;
  line-height: 1.35;
}

.config-panels :deep(label) {
  color: var(--text-color, #1e293b);
  font-size: 0.8125rem;
  font-weight: 500;
}

.config-panels :deep(.p-inputtext),
.config-panels :deep(.p-password),
.config-panels :deep(.p-inputnumber),
.config-panels :deep(.p-select),
.config-panels :deep(.p-autocomplete) {
  min-height: 2.35rem;
}

.config-panels :deep(.p-inputtext::placeholder),
.config-panels :deep(.p-inputnumber-input::placeholder),
.config-panels :deep(.p-password-input::placeholder),
.config-panels :deep(.p-autocomplete-input::placeholder),
.config-panels :deep(.p-autocomplete .p-inputtext::placeholder),
.config-panels :deep(.p-autocomplete-input-multiple input::placeholder) {
  color: var(--et-placeholder-color, #a8b5c5) !important;
  -webkit-text-fill-color: var(--et-placeholder-color, #a8b5c5) !important;
  opacity: 1 !important;
}

.config-panels :deep(.p-placeholder),
.config-panels :deep(.p-select .p-placeholder),
.config-panels :deep(.p-multiselect .p-placeholder),
.config-panels :deep(.p-autocomplete .p-placeholder) {
  color: var(--et-placeholder-color, #a8b5c5) !important;
}

.advanced-flag-item .config-help-tip {
  margin-left: 0.4rem;
}

.network-secret-group :deep(.p-password),
.network-secret-group :deep(.p-password-input) {
  width: 100%;
}

.network-secret-generate-btn {
  width: 2.35rem !important;
  min-width: 2.35rem !important;
  height: 2.35rem !important;
  padding: 0 !important;
}

@media (max-width: 760px) {
  .advanced-flag-groups {
    grid-template-columns: 1fr;
  }

  /* 窄屏：开关项双列，避免单列过长 */
  .advanced-flags-grid {
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 0.35rem 0.5rem;
  }

  .advanced-flag-item {
    min-height: 2rem;
    padding: 0.25rem 0.4rem;
  }

  .advanced-flag-item label {
    margin-left: 0.35rem;
    font-size: 0.75rem;
  }

  .advanced-flag-item .config-help-tip {
    margin-left: 0.2rem;
    width: 0.95rem;
    height: 0.95rem;
    font-size: 0.85rem;
  }

  .config-compact-grid {
    grid-template-columns: 1fr;
  }
}

@media (prefers-color-scheme: dark) {
  .advanced-flag-group {
    background: var(--surface-800, #1e293b);
  }

  @media (hover: hover) {
    .advanced-flag-item:hover {
      background: var(--surface-hover, rgba(255, 255, 255, 0.06));
    }
  }
}

/* 问号提示图标：内联对齐 + 固定尺寸，避免撑高所在行 */
.config-help-tip {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
  width: 1.1rem;
  height: 1.1rem;
  font-size: 0.95rem;
  line-height: 1;
  color: var(--text-color-secondary, #64748b);
  cursor: help;
}

/* 标签与输入同一行样式已提升到 style.css（子组件 DNS 编辑器也要用） */
.config-inline-control :deep(.p-inputtext),
.config-inline-control :deep(.p-autocomplete),
.config-inline-control :deep(.p-inputnumber),
.config-inline-control :deep(.p-select),
.config-inline-control :deep(.p-password),
.config-inline-control :deep(.p-inputwrapper),
.config-inline-expand :deep(.p-inputtext),
.config-inline-expand :deep(.p-autocomplete),
.config-inline-expand :deep(.p-inputnumber),
.config-inline-expand :deep(.p-password),
.config-inline-expand :deep(.p-inputwrapper) {
  width: 100%;
}
</style>
