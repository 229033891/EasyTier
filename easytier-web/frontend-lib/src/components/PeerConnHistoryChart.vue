<template>
  <div class="flex flex-col gap-3">
    <div class="flex flex-wrap items-center gap-2">
      <Select v-if="peers.length" v-model="selectedPeerId" :options="peers" option-label="label"
        option-value="peer_id" class="peer-history-peer-select" :placeholder="t('history_select_peer')"
        :pt="{ root: { class: 'w-full sm:w-auto' } }" />

      <SelectButton v-model="hours" :options="rangeOptions" option-label="label" option-value="value"
        :allow-empty="false" class="peer-history-range" />

      <Button size="small" severity="secondary" text rounded icon="pi pi-refresh" class="et-icon-action-btn"
        :loading="loading" :aria-label="t('web.common.refresh')" v-tooltip.top="t('web.common.refresh')"
        @click="load()" />

      <span v-if="selectedPeer" class="text-xs text-surface-500 truncate">
        {{ selectedPeer.remote_addr }}<template v-if="selectedPeer.tunnel_type"> ·
          {{ selectedPeer.tunnel_type }}</template>
      </span>
    </div>

    <div v-if="loading && !data" class="py-6 text-center text-sm text-surface-500">
      {{ t('history_loading') }}
    </div>

    <div v-else-if="loadFailed" class="py-6 text-center text-sm text-red-500">
      {{ t('web.common.error') }}
    </div>

    <div v-else-if="!peers.length" class="py-6 text-center text-sm text-surface-500">
      {{ t('history_no_data') }}
    </div>

    <template v-else>
      <div>
        <div class="peer-history-chart-title flex items-center gap-1">
          <span>{{ t('history_latency') }}</span>
          <i
            class="pi pi-question-circle config-help-tip" tabindex="0"
            v-tooltip.top="t('history_sampling_hint')"
            :aria-label="t('history_sampling_hint')"
            role="img"
          />
        </div>
        <div class="h-40">
          <canvas ref="latencyCanvas"></canvas>
        </div>
      </div>
      <div>
        <div class="peer-history-chart-title">{{ t('history_loss') }}</div>
        <div class="h-40">
          <canvas ref="lossCanvas"></canvas>
        </div>
      </div>
      <div>
        <div class="peer-history-chart-title">{{ t('history_jitter') }}</div>
        <div class="h-40">
          <canvas ref="jitterCanvas"></canvas>
        </div>
      </div>
      <div>
        <div class="peer-history-chart-title">{{ t('history_traffic') }}</div>
        <div class="h-40">
          <canvas ref="trafficCanvas"></canvas>
        </div>
      </div>
    </template>
  </div>
</template>

<script setup lang="ts">
import { Button, Select, SelectButton } from 'primevue'
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import {
  Chart as ChartJS,
  CategoryScale,
  LinearScale,
  PointElement,
  LineElement,
  LineController,
  Tooltip,
  Legend,
  Filler,
} from 'chart.js'
import type { PeerConnHistoryResponse, PeerConnHistorySeries, RemoteClient } from '../modules/api'
import { jitterMsSeries, latencyMsSeries, lossPctSeries, rateSeries } from '../modules/peerHistory'

ChartJS.register(
  CategoryScale,
  LinearScale,
  PointElement,
  LineElement,
  LineController,
  Tooltip,
  Legend,
  Filler,
)

const props = defineProps<{
  api: RemoteClient
  instanceId: string
  /** 面板展开后才拉数据，避免折叠状态下空跑请求 */
  visible?: boolean
}>()

const { t } = useI18n()

const rangeOptions = computed(() => [
  { label: t('history_range_1h'), value: 1 },
  { label: t('history_range_6h'), value: 6 },
  { label: t('history_range_24h'), value: 24 },
  { label: t('history_range_7d'), value: 24 * 7 },
])

const hours = ref(24)
const data = ref<PeerConnHistoryResponse>()
const loading = ref(false)
const loadFailed = ref(false)
const selectedPeerId = ref<number>()

/** 该实现不支持历史查询（GUI 直连内核）时不渲染任何东西 */
const supported = computed(() => typeof props.api?.get_peer_conn_history === 'function')

const peers = computed(() =>
  (data.value?.peers ?? []).map((p: PeerConnHistorySeries) => ({
    ...p,
    label: p.hostname
      ? `${p.hostname}${p.remote_addr ? ` · ${p.remote_addr}` : ''}`
      : `${t('node_info_group_peer_id')} ${p.peer_id}`,
  })),
)

const selectedPeer = computed(() => peers.value.find(p => p.peer_id === selectedPeerId.value))

function formatBytes(bytes: number): string {
  if (!bytes) return '0 B'
  const k = 1024
  const sizes = ['B', 'KB', 'MB', 'GB', 'TB']
  const i = Math.min(Math.floor(Math.log(bytes) / Math.log(k)), sizes.length - 1)
  return `${parseFloat((bytes / Math.pow(k, i)).toFixed(1))} ${sizes[i]}`
}

function formatTime(unixSeconds: number): string {
  const d = new Date(unixSeconds * 1000)
  const pad = (n: number) => String(n).padStart(2, '0')
  const hm = `${pad(d.getHours())}:${pad(d.getMinutes())}`
  // 跨度超过一天时补上日期，否则看不出是哪天
  return hours.value > 24 ? `${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${hm}` : hm
}

const labels = computed(() => selectedPeer.value?.points.map(p => formatTime(p.t)) ?? [])

const latencyData = computed(() => latencyMsSeries(selectedPeer.value?.points ?? []))
const lossData = computed(() => lossPctSeries(selectedPeer.value?.points ?? []))
const jitterData = computed(() => jitterMsSeries(selectedPeer.value?.points ?? []))

const rxRate = computed(() => rateSeries(selectedPeer.value?.points ?? [], 'rx_bytes'))
const txRate = computed(() => rateSeries(selectedPeer.value?.points ?? [], 'tx_bytes'))

const latencyCanvas = ref<HTMLCanvasElement>()
const lossCanvas = ref<HTMLCanvasElement>()
const jitterCanvas = ref<HTMLCanvasElement>()
const trafficCanvas = ref<HTMLCanvasElement>()
let latencyChart: ChartJS | null = null
let lossChart: ChartJS | null = null
let jitterChart: ChartJS | null = null
let trafficChart: ChartJS | null = null

const axisFont = { size: 10 }

function baseOptions(
  yTickFormatter: (value: number) => string,
  yTooltipFormatter: (value: number) => string,
  showLegend = true,
) {
  return {
    responsive: true,
    maintainAspectRatio: false,
    animation: { duration: 200 },
    interaction: { intersect: false, mode: 'index' as const },
    plugins: {
      legend: { display: showLegend, labels: { boxWidth: 10, font: axisFont } },
      tooltip: {
        callbacks: {
          // 不加单位的话悬停只显示裸数字（延迟的 ms、流量的 B/s 都看不出来）
          label: (ctx: { parsed: { y: number | null }, dataset: { label?: string } }) => {
            const label = ctx.dataset.label ?? ''
            const value = ctx.parsed?.y
            if (value == null) return `${label}: —`
            return `${label}: ${yTooltipFormatter(value)}`
          },
        },
      },
    },
    scales: {
      x: {
        grid: { display: false },
        ticks: { maxTicksLimit: 6, font: axisFont, autoSkip: true },
      },
      y: {
        beginAtZero: true,
        grid: { color: 'rgba(0, 0, 0, 0.08)' },
        ticks: { font: axisFont, callback: (value: unknown) => yTickFormatter(value as number) },
      },
    },
  }
}

function initCharts() {
  if (latencyCanvas.value && !latencyChart) {
    latencyChart = new ChartJS(latencyCanvas.value.getContext('2d')!, {
      type: 'line',
      data: {
        labels: labels.value,
        datasets: [{
          label: t('history_latency'),
          data: latencyData.value,
          borderColor: 'rgb(249, 115, 22)',
          backgroundColor: 'rgba(249, 115, 22, 0.1)',
          borderWidth: 2,
          fill: true,
          tension: 0.35,
          pointRadius: 0,
          pointHoverRadius: 4,
          spanGaps: true,
        }],
      },
      // 只有一条线，图例和上面的「延迟」标题重复，隐藏掉
      options: baseOptions(v => `${v} ms`, v => `${v} ms`, false),
    })
  }

  if (lossCanvas.value && !lossChart) {
    lossChart = new ChartJS(lossCanvas.value.getContext('2d')!, {
      type: 'line',
      data: {
        labels: labels.value,
        datasets: [{
          label: t('history_loss'),
          data: lossData.value,
          borderColor: 'rgb(239, 68, 68)',
          backgroundColor: 'rgba(239, 68, 68, 0.1)',
          borderWidth: 2,
          fill: true,
          tension: 0.35,
          pointRadius: 0,
          pointHoverRadius: 4,
          spanGaps: true,
        }],
      },
      options: baseOptions(v => `${v}%`, v => `${v}%`, false),
    })
  }

  if (jitterCanvas.value && !jitterChart) {
    jitterChart = new ChartJS(jitterCanvas.value.getContext('2d')!, {
      type: 'line',
      data: {
        labels: labels.value,
        datasets: [{
          label: t('history_jitter'),
          data: jitterData.value,
          borderColor: 'rgb(168, 85, 247)',
          backgroundColor: 'rgba(168, 85, 247, 0.1)',
          borderWidth: 2,
          fill: true,
          tension: 0.35,
          pointRadius: 0,
          pointHoverRadius: 4,
          spanGaps: true,
        }],
      },
      options: baseOptions(v => `${v} ms`, v => `${v} ms`, false),
    })
  }

  if (trafficCanvas.value && !trafficChart) {
    trafficChart = new ChartJS(trafficCanvas.value.getContext('2d')!, {
      type: 'line',
      data: {
        labels: labels.value,
        datasets: [
          {
            label: t('upload'),
            data: txRate.value,
            borderColor: 'rgb(34, 197, 94)',
            backgroundColor: 'rgba(34, 197, 94, 0.1)',
            borderWidth: 2,
            fill: true,
            tension: 0.35,
            pointRadius: 0,
            pointHoverRadius: 4,
            spanGaps: true,
          },
          {
            label: t('download'),
            data: rxRate.value,
            borderColor: 'rgb(14, 165, 233)',
            backgroundColor: 'rgba(14, 165, 233, 0.1)',
            borderWidth: 2,
            fill: true,
            tension: 0.35,
            pointRadius: 0,
            pointHoverRadius: 4,
            spanGaps: true,
          },
        ],
      },
      // 轴刻度用 formatBytes，tooltip 补上 /s 表明这是速率
      options: baseOptions(v => formatBytes(v), v => `${formatBytes(v)}/s`),
    })
  }
}

function destroyCharts() {
  latencyChart?.destroy()
  lossChart?.destroy()
  jitterChart?.destroy()
  trafficChart?.destroy()
  latencyChart = null
  lossChart = null
  jitterChart = null
  trafficChart = null
}

function syncCharts() {
  if (latencyChart) {
    latencyChart.data.labels = labels.value
    latencyChart.data.datasets[0].data = latencyData.value
    latencyChart.update('none')
  }
  if (lossChart) {
    lossChart.data.labels = labels.value
    lossChart.data.datasets[0].data = lossData.value
    lossChart.update('none')
  }
  if (jitterChart) {
    jitterChart.data.labels = labels.value
    jitterChart.data.datasets[0].data = jitterData.value
    jitterChart.update('none')
  }
  if (trafficChart) {
    trafficChart.data.labels = labels.value
    trafficChart.data.datasets[0].data = txRate.value
    trafficChart.data.datasets[1].data = rxRate.value
    trafficChart.update('none')
  }
}

let loadSequence = 0

async function load() {
  if (!supported.value || !props.instanceId) return

  const sequence = ++loadSequence
  const instanceId = props.instanceId
  const requestHours = hours.value
  loading.value = true
  loadFailed.value = false
  try {
    const resp = await props.api.get_peer_conn_history!(instanceId, requestHours)
    if (sequence !== loadSequence || props.instanceId !== instanceId || hours.value !== requestHours) return
    data.value = resp
    // 保留当前选择；peer 消失了才回退到第一个
    if (!resp?.peers.some(p => p.peer_id === selectedPeerId.value)) {
      selectedPeerId.value = resp?.peers[0]?.peer_id
    }
  } catch (e) {
    if (sequence !== loadSequence) return
    console.error('Failed to load peer connection history', e)
    loadFailed.value = true
    data.value = undefined
    // Peers empty → canvas unmounts; clear Chart handles so a later remount can re-init.
    destroyCharts()
  } finally {
    if (sequence !== loadSequence) return
    loading.value = false
    await nextTick()
    if (sequence !== loadSequence) return
    // Canvas unmounts when peers empty; drop Chart.js instances so remount can re-init.
    if (!(data.value?.peers?.length)) {
      destroyCharts()
      return
    }
    initCharts()
    syncCharts()
  }
}

function maybeLoad() {
  if (props.visible === false) return
  void load()
}

watch(() => props.visible, v => {
  if (v) void load()
  else loadSequence++
})

watch([() => props.instanceId, hours], () => {
  data.value = undefined
  selectedPeerId.value = undefined
  // Clearing data unmounts <canvas>; must null Chart handles here (not on every load),
  // otherwise initCharts sees stale non-null chart vars and skips recreate → blank plots.
  // Do not destroy at load() start: refresh keeps peers mounted and would flicker.
  destroyCharts()
  maybeLoad()
})

watch(selectedPeerId, async () => {
  await nextTick()
  syncCharts()
})

onMounted(() => {
  maybeLoad()
})

onUnmounted(() => {
  loadSequence++
  destroyCharts()
})
</script>

<style scoped>
.peer-history-peer-select {
  min-width: 14rem;
}

.peer-history-range :deep(.p-selectbutton) {
  display: flex;
  flex-wrap: wrap;
}

.peer-history-chart-title {
  margin-bottom: 0.25rem;
  font-size: 0.75rem;
  font-weight: 600;
  color: var(--text-color-secondary, #64748b);
}
</style>
