<template>
  <div class="flex flex-col gap-3">
    <div class="flex flex-wrap items-center gap-2">
      <MultiSelect v-if="seriesOptions.length" v-model="selectedKeys" :options="seriesOptions"
        option-label="label" option-value="key" :placeholder="t('flow_history_select_flow')" display="chip"
        :max-selected-labels="3" class="flow-history-select" />

      <SelectButton v-model="hours" :options="rangeOptions" option-label="label" option-value="value"
        :allow-empty="false" class="flow-history-range" />

      <Button size="small" severity="secondary" text rounded icon="pi pi-refresh" class="et-icon-action-btn"
        :loading="loading" :aria-label="t('web.common.refresh')" v-tooltip.top="t('web.common.refresh')"
        @click="load()" />

      <span v-if="data?.truncated" class="text-xs text-surface-500">
        {{ t('flow_history_truncated', { n: data.flows.length }) }}
      </span>
    </div>

    <div v-if="loading && !data" class="py-6 text-center text-sm text-surface-500">
      {{ t('history_loading') }}
    </div>

    <div v-else-if="loadFailed" class="py-6 text-center text-sm text-red-500">
      {{ t('web.common.error') }}
    </div>

    <div v-else-if="!seriesOptions.length" class="py-6 text-center text-sm text-surface-500">
      {{ t('flow_history_no_data') }}
    </div>

    <template v-else>
      <div>
        <div class="flow-history-chart-title flex items-center gap-1">
          <span>{{ t('flow_history_rate') }}</span>
          <i class="pi pi-question-circle config-help-tip" tabindex="0"
            v-tooltip.top="t('flow_history_rate_help')" :aria-label="t('flow_history_rate_help')"
            role="img" />
        </div>
        <div class="h-56">
          <canvas ref="rateCanvas"></canvas>
        </div>
      </div>
    </template>
  </div>
</template>

<script setup lang="ts">
import { Button, MultiSelect, SelectButton } from 'primevue'
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
import type { FlowHistoryResponse, FlowHistorySeries, RemoteClient } from '../modules/api'
import {
  alignFlowRates,
  buildFlowTimeline,
  flowSeriesColorForKey,
  flowSeriesKey,
  flowSeriesLabel,
  formatFlowRate,
} from '../modules/flowHistory'

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
const data = ref<FlowHistoryResponse>()
const loading = ref(false)
const loadFailed = ref(false)
const selectedKeys = ref<string[]>([])

/** 该实现不支持历史查询（GUI 直连内核）时不渲染任何东西 */
const supported = computed(() => typeof props.api?.get_flow_history === 'function')

/** 后端已按窗口内总流量降序返回，这里只负责补标签 */
const seriesOptions = computed(() =>
  (data.value?.flows ?? []).map((item: FlowHistorySeries) => {
    const base = flowSeriesLabel(item)
    // 采样放大（§4.5）必须可见：任一点是估算值就在图例里标注。
    const label = item.points.some(point => point.sampled)
      ? `${base} · ${t('status.flow_estimated')}`
      : base
    return {
      ...item,
      key: flowSeriesKey(item),
      label,
    }
  }),
)

const selectedSeries = computed(() =>
  seriesOptions.value.filter(item => selectedKeys.value.includes(item.key)),
)

function formatTime(unixSeconds: number): string {
  const d = new Date(unixSeconds * 1000)
  const pad = (n: number) => String(n).padStart(2, '0')
  const hm = `${pad(d.getHours())}:${pad(d.getMinutes())}`
  // 跨度达到一天时补上日期（24h 会跨两个自然日），否则看不出是哪天
  return hours.value >= 24 ? `${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${hm}` : hm
}

const timeline = computed(() => buildFlowTimeline(selectedSeries.value))
const labels = computed(() => timeline.value.map(formatTime))

const rateCanvas = ref<HTMLCanvasElement>()
let rateChart: ChartJS | null = null

const axisFont = { size: 10 }

function chartOptions() {
  return {
    responsive: true,
    maintainAspectRatio: false,
    animation: { duration: 200 },
    interaction: { intersect: false, mode: 'index' as const },
    plugins: {
      legend: { display: true, labels: { boxWidth: 10, font: axisFont } },
      tooltip: {
        callbacks: {
          label: (ctx: { parsed: { y: number | null }, dataset: { label?: string } }) => {
            const label = ctx.dataset.label ?? ''
            const value = ctx.parsed?.y
            if (value == null) return `${label}: —`
            return `${label}: ${formatFlowRate(value)}`
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
        ticks: {
          font: axisFont,
          callback: (value: unknown) => formatFlowRate(value as number),
        },
      },
    },
  }
}

function buildDatasets() {
  return selectedSeries.value.map(item => ({
    label: item.label,
    data: alignFlowRates(item.points, timeline.value),
    borderColor: flowSeriesColorForKey(item.key),
    backgroundColor: 'rgba(14, 165, 233, 0.08)',
    borderWidth: 2,
    fill: false,
    tension: 0.35,
    pointRadius: 0,
    pointHoverRadius: 4,
    spanGaps: false,
  }))
}

function initChart() {
  if (!rateCanvas.value || rateChart) return
  rateChart = new ChartJS(rateCanvas.value.getContext('2d')!, {
    type: 'line',
    data: { labels: labels.value, datasets: buildDatasets() },
    options: chartOptions(),
  })
}

function destroyCharts() {
  rateChart?.destroy()
  rateChart = null
}

function syncCharts() {
  if (!rateChart) return
  rateChart.data.labels = labels.value
  rateChart.data.datasets = buildDatasets()
  rateChart.update('none')
}

/** 默认选中流量最大的前几条曲线，避免一上来就把 40 条线全画出来 */
function defaultSelection(flows: FlowHistorySeries[]): string[] {
  return flows.slice(0, 5).map(flowSeriesKey)
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
    const resp = await props.api.get_flow_history!(instanceId, requestHours)
    if (sequence !== loadSequence || props.instanceId !== instanceId || hours.value !== requestHours) return
    data.value = resp
    // 保留仍然存在的选择；全都消失了才回退到默认的前几条
    const available = new Set((resp?.flows ?? []).map(flowSeriesKey))
    const kept = selectedKeys.value.filter(key => available.has(key))
    selectedKeys.value = kept.length ? kept : defaultSelection(resp?.flows ?? [])
  } catch (e) {
    if (sequence !== loadSequence) return
    console.error('Failed to load flow history', e)
    loadFailed.value = true
    data.value = undefined
    // 数据清空 → canvas 卸载；必须置空句柄，否则下次挂载 initChart 会以为实例还在
    destroyCharts()
  } finally {
    // 只有最新的请求负责复位 loading；被更新请求取代时由新请求负责。
    // visible=false 会递增 sequence 但不发起新请求，由该 watcher 显式复位。
    if (sequence === loadSequence) {
      loading.value = false
    }
    await nextTick()
    if (sequence !== loadSequence) return
    if (!(data.value?.flows?.length)) {
      destroyCharts()
      return
    }
    initChart()
    syncCharts()
  }
}

function maybeLoad() {
  if (props.visible === false) return
  void load()
}

watch(() => props.visible, v => {
  if (v) {
    void load()
  } else {
    // 折叠时取代在途请求，并立即复位 loading（不会再有新请求来复位它）。
    loadSequence++
    loading.value = false
  }
})

watch([() => props.instanceId, hours], () => {
  data.value = undefined
  selectedKeys.value = []
  // 清空数据会卸载 canvas，这里必须同步销毁实例（详见 load() 里的说明）
  destroyCharts()
  maybeLoad()
})

watch(selectedKeys, async () => {
  await nextTick()
  if (!selectedSeries.value.length) {
    destroyCharts()
    return
  }
  initChart()
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
.flow-history-select {
  min-width: 16rem;
  max-width: 26rem;
}

.flow-history-range :deep(.p-selectbutton) {
  display: flex;
  flex-wrap: wrap;
}

.flow-history-chart-title {
  margin-bottom: 0.25rem;
  font-size: 0.75rem;
  font-weight: 600;
  color: var(--text-color-secondary, #334155);
}
</style>
