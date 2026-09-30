<template>
  <div class="network-chart-card">
    <div class="flex items-center justify-center mb-3">
      <div class="flex gap-2 text-sm">
        <span class="flex items-center gap-1 w-32">
          <div class="w-2 h-2 rounded-full bg-emerald-500"></div>
          <span class="truncate text-emerald-600 dark:text-emerald-400">{{ t('upload') }}: {{ currentUpload }}/s</span>
        </span>
        <span class="flex items-center gap-1 w-32">
          <div class="w-2 h-2 rounded-full" style="background: var(--primary-color, #0ea5e9)"></div>
          <span class="truncate" style="color: var(--primary-color, #0ea5e9)">{{ t('download') }}: {{ currentDownload }}/s</span>
        </span>
      </div>
    </div>
    <div class="h-32">
      <canvas ref="chartCanvas"></canvas>
    </div>
  </div>
</template>

<style scoped>
.network-chart-card {
  border: 1px solid color-mix(in srgb, var(--primary-color, #0ea5e9) 22%, var(--et-border-color, #e2e8f0));
  border-radius: var(--et-radius, 0.75rem);
  padding: 1rem;
  background: linear-gradient(
    145deg,
    color-mix(in srgb, var(--primary-color, #0ea5e9) 8%, #ffffff) 0%,
    color-mix(in srgb, var(--primary-color, #0ea5e9) 14%, #f8fafc) 100%
  );
  box-shadow: none;
  transition: box-shadow 0.2s ease, border-color 0.2s ease;
}

@media (hover: hover) {
  .network-chart-card:hover {
    box-shadow: 0 8px 20px rgba(15, 23, 42, 0.06);
  }
}

@media (prefers-color-scheme: dark) {
  .network-chart-card {
    background: linear-gradient(
      145deg,
      color-mix(in srgb, var(--primary-color, #0ea5e9) 16%, #0f172a) 0%,
      color-mix(in srgb, var(--primary-color, #0ea5e9) 10%, #1e293b) 100%
    );
  }
}
</style>

<script setup lang="ts">
import { ref, onMounted, onUnmounted, watch, nextTick } from 'vue'
import {
  Chart as ChartJS,
  CategoryScale,
  LinearScale,
  PointElement,
  LineElement,
  LineController,
  Title,
  Tooltip,
  Legend,
  Filler
} from 'chart.js'
import { useI18n } from 'vue-i18n';

const { t } = useI18n()

// 注册Chart.js组件
ChartJS.register(
  CategoryScale,
  LinearScale,
  PointElement,
  LineElement,
  LineController,
  Title,
  Tooltip,
  Legend,
  Filler
)

interface Props {
  uploadRate: string
  downloadRate: string
}

const props = defineProps<Props>()

const chartCanvas = ref<HTMLCanvasElement>()
let chart: ChartJS | null = null
let updateTimer: number | null = null

// 存储历史数据，最多保存30个数据点（1分钟历史）
const maxDataPoints = 120
const uploadHistory: number[] = []
const downloadHistory: number[] = []
const timeLabels: string[] = []

const currentUpload = ref('0')
const currentDownload = ref('0')

// 将带单位的速率字符串转换为字节数
function parseRateToBytes(rateStr: string): number {
  if (!rateStr || rateStr === '0') return 0

  const match = rateStr.match(/([0-9.]+)\s*([KMGT]?i?B)/i)
  if (!match) return 0

  const value = parseFloat(match[1])
  const unit = match[2].toUpperCase()

  const multipliers: { [key: string]: number } = {
    'B': 1,
    'KB': 1000,
    'KIB': 1024,
    'MB': 1000000,
    'MIB': 1024 * 1024,
    'GB': 1000000000,
    'GIB': 1024 * 1024 * 1024,
    'TB': 1000000000000,
    'TIB': 1024 * 1024 * 1024 * 1024
  }

  return value * (multipliers[unit] || 1)
}

// 格式化字节为可读格式
function formatBytes(bytes: number): string {
  if (bytes < 1) return bytes.toFixed(1) + ' B'

  const k = 1024
  const sizes = ['B', 'KB', 'MB', 'GB', 'TB']
  const i = Math.floor(Math.log(bytes) / Math.log(k))

  return parseFloat((bytes / Math.pow(k, i)).toFixed(1)) + ' ' + sizes[i]
}

// 更新数据
function updateData() {
  const uploadBytes = parseRateToBytes(props.uploadRate)
  const downloadBytes = parseRateToBytes(props.downloadRate)

  currentUpload.value = formatBytes(uploadBytes)
  currentDownload.value = formatBytes(downloadBytes)

  // 添加新数据点
  uploadHistory.push(uploadBytes)
  downloadHistory.push(downloadBytes)

  // 生成时间标签
  const now = new Date()
  const timeStr = now.toLocaleTimeString('zh-CN', {
    hour12: false,
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit'
  })
  timeLabels.push(timeStr)

  // 保持数据点数量不超过最大值
  if (uploadHistory.length > maxDataPoints) {
    uploadHistory.shift()
    downloadHistory.shift()
    timeLabels.shift()
  }

  // 更新图表
  if (chart) {
    chart.data.labels = timeLabels
    chart.data.datasets[0].data = uploadHistory
    chart.data.datasets[1].data = downloadHistory
    chart.update('none')
  }
}

// 初始化图表
function initChart() {
  if (!chartCanvas.value) return

  const ctx = chartCanvas.value.getContext('2d')
  if (!ctx) return

  chart = new ChartJS(ctx, {
    type: 'line',
    data: {
      labels: timeLabels,
      datasets: [
        {
          label: t('upload'),
          data: uploadHistory,
          borderColor: 'rgb(34, 197, 94)',
          backgroundColor: 'rgba(34, 197, 94, 0.1)',
          borderWidth: 2,
          fill: true,
          tension: 0.4,
          pointRadius: 0,
          pointHoverRadius: 4
        },
        {
          label: t('download'),
          data: downloadHistory,
          borderColor: 'rgb(14, 165, 233)',
          backgroundColor: 'rgba(14, 165, 233, 0.1)',
          borderWidth: 2,
          fill: true,
          tension: 0.4,
          pointRadius: 0,
          pointHoverRadius: 4
        }
      ]
    },
    options: {
      responsive: true,
      maintainAspectRatio: false,
      interaction: {
        intersect: false,
        mode: 'index'
      },
      plugins: {
        legend: {
          display: false
        },
        tooltip: {
          callbacks: {
            label: function (context: any) {
              const value = context.parsed.y
              return `${context.dataset.label}: ${formatBytes(value)}/s`
            }
          }
        }
      },
      scales: {
        x: {
          display: true,
          grid: {
            display: false
          },
          ticks: {
            maxTicksLimit: 3,
            font: {
              size: 8
            }
          }
        },
        y: {
          display: true,
          beginAtZero: true,
          min: 0,
          grid: {
            color: 'rgba(0, 0, 0, 0.1)'
          },
          ticks: {
            callback: function (value: any) {
              return formatBytes(value as number)
            },
            font: {
              size: 8
            },
          },
        }
      },
      animation: {
        duration: 10
      }
    }
  })
}

// 监听props变化
watch([() => props.uploadRate, () => props.downloadRate], () => {
  updateData()
}, { immediate: true })

onMounted(async () => {
  // add initial point
  const now = new Date();
  for (let i = 0; i < maxDataPoints; i++) {
    let date = new Date(now.getTime() - (maxDataPoints - i) * 2000)
    const timeStr = date.toLocaleTimeString(navigator.language, {
      hour12: false,
      hour: '2-digit',
      minute: '2-digit',
      second: '2-digit'
    })
    uploadHistory.push(0)
    downloadHistory.push(0)
    timeLabels.push(timeStr)
  }

  await nextTick()
  initChart()
  updateData()

  // 启动定时器，每2秒更新一次图表
  updateTimer = window.setInterval(() => {
    updateData()
  }, 2000)
})

onUnmounted(() => {
  if (chart) {
    chart.destroy()
  }
  if (updateTimer) {
    clearInterval(updateTimer)
  }
})
</script>