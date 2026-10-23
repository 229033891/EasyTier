import { flushPromises, mount } from '@vue/test-utils'
import { describe, expect, it, vi } from 'vitest'
import { nextTick } from 'vue'
import FlowHistoryChart from '../src/components/FlowHistoryChart.vue'
import type { FlowHistoryResponse } from '../src/modules/api'

const chartStats = vi.hoisted(() => ({
  created: 0,
  destroyed: 0,
}))

vi.mock('vue-i18n', () => ({
  useI18n: () => ({
    t: (key: string, values?: Record<string, unknown>) => (values ? `${key}:${JSON.stringify(values)}` : key),
    te: () => true,
  }),
}))

vi.mock('primevue', async () => {
  const { defineComponent: dc, h: hh } = await import('vue')

  const ButtonStub = dc({
    name: 'Button',
    emits: ['click'],
    setup(_props, { emit }) {
      return () => hh('button', {
        type: 'button',
        onClick: (event: MouseEvent) => emit('click', event),
      })
    },
  })

  const MultiSelectStub = dc({
    name: 'MultiSelect',
    props: {
      modelValue: [Array],
      options: Array,
    },
    emits: ['update:modelValue'],
    setup() {
      return () => hh('div', { 'data-stub': 'multi-select' })
    },
  })

  const SelectButtonStub = dc({
    name: 'SelectButton',
    props: {
      modelValue: [String, Number],
      options: Array,
    },
    emits: ['update:modelValue'],
    setup() {
      return () => hh('div', { 'data-stub': 'select-button' })
    },
  })

  return {
    Button: ButtonStub,
    MultiSelect: MultiSelectStub,
    SelectButton: SelectButtonStub,
  }
})

vi.mock('chart.js', () => {
  class ChartMock {
    static register = vi.fn()
    data: { labels: unknown[], datasets: Array<{ data: unknown, label?: string }> }
    /**
     * Last config handed to the constructor. Captured so tests can assert the
     * *rendering* contract (e.g. `spanGaps`) and not just the data.
     */
    static lastConfig: { data?: { labels?: unknown[], datasets?: any[] }, options?: any } | undefined
    config: { data?: { labels?: unknown[], datasets?: any[] }, options?: any }
    constructor(
      _canvas: unknown,
      config?: { data?: { labels?: unknown[], datasets?: any[] }, options?: any },
    ) {
      chartStats.created += 1
      this.config = config ?? {}
      ChartMock.lastConfig = this.config
      this.data = {
        labels: config?.data?.labels ?? [],
        datasets: (config?.data?.datasets ?? [{ data: [] }]).map(d => ({
          ...d,
          data: d.data ?? [],
        })),
      }
    }
    update = vi.fn()
    destroy = () => {
      chartStats.destroyed += 1
    }
  }
  return {
    Chart: ChartMock,
    CategoryScale: {},
    LinearScale: {},
    PointElement: {},
    LineElement: {},
    LineController: {},
    Tooltip: {},
    Legend: {},
    Filler: {},
  }
})

function flowResp(connId: string, ip: string): FlowHistoryResponse {
  return {
    bucket_seconds: 60,
    from: 1_700_000_000,
    to: 1_700_000_120,
    truncated: false,
    flows: [{
      conn_id: connId,
      peer_id: 7,
      peer_hostname: 'peer-b',
      ip,
      prefix_len: 24,
      direction: 1,
      last_seen: 1_700_000_060,
      points: [
        { t: 1_700_000_000, rate_bytes_per_sec: 100, bytes: 6_000, flow_count: 3, sampled: false, samples: 1 },
        { t: 1_700_000_060, rate_bytes_per_sec: 200, bytes: 18_000, flow_count: 4, sampled: false, samples: 1 },
      ],
    }],
  }
}

describe('FlowHistoryChart', () => {
  it('recreates the Chart instance after instanceId switch unmounts the canvas', async () => {
    chartStats.created = 0
    chartStats.destroyed = 0

    const getFlowHistory = vi.fn(async (instanceId: string) => {
      if (instanceId === 'A')
        return flowResp('42', '10.0.0.0')
      return flowResp('7', '10.0.9.0')
    })

    const TooltipDirective = {
      mounted() {},
      updated() {},
    }

    const wrapper = mount(FlowHistoryChart, {
      props: {
        api: { get_flow_history: getFlowHistory } as any,
        instanceId: 'A',
        visible: true,
      },
      global: {
        directives: { tooltip: TooltipDirective },
      },
    })

    try {
      await flushPromises()
      await nextTick()
      expect(wrapper.findAll('canvas')).toHaveLength(1)
      expect(chartStats.created).toBe(1)

      await wrapper.setProps({ instanceId: 'B' })
      await flushPromises()
      await nextTick()
      await flushPromises()

      expect(wrapper.findAll('canvas')).toHaveLength(1)
      // 清空数据会卸载 canvas；没有 destroyCharts 的话 created 会停在 1、新画布空白
      expect(chartStats.destroyed).toBeGreaterThanOrEqual(1)
      expect(chartStats.created).toBe(2)
      expect(getFlowHistory).toHaveBeenCalledWith('B', expect.any(Number))
    }
    finally {
      wrapper.unmount()
    }
  })

  it('shows the empty state when the node has no flows', async () => {
    const getFlowHistory = vi.fn(async () => ({
      bucket_seconds: 60,
      from: 0,
      to: 1,
      truncated: false,
      flows: [],
    }))

    const wrapper = mount(FlowHistoryChart, {
      props: {
        api: { get_flow_history: getFlowHistory } as any,
        instanceId: 'A',
        visible: true,
      },
      global: {
        directives: { tooltip: { mounted() {}, updated() {} } },
      },
    })

    try {
      await flushPromises()
      await nextTick()
      expect(wrapper.findAll('canvas')).toHaveLength(0)
      expect(wrapper.text()).toContain('flow_history_no_data')
    }
    finally {
      wrapper.unmount()
    }
  })

  it('does not fetch while the panel is collapsed', async () => {
    const getFlowHistory = vi.fn(async () => flowResp('42', '10.0.0.0'))
    const wrapper = mount(FlowHistoryChart, {
      props: {
        api: { get_flow_history: getFlowHistory } as any,
        instanceId: 'A',
        visible: false,
      },
      global: {
        directives: { tooltip: { mounted() {}, updated() {} } },
      },
    })

    try {
      await flushPromises()
      expect(getFlowHistory).not.toHaveBeenCalled()
    }
    finally {
      wrapper.unmount()
    }
  })

  /**
   * `alignFlowRates` 用 null 表示「这个桶没有观测到流量」，靠 chart.js 的
   * `spanGaps: false` 断线。若有人把它改回 true（或漏写），null 会被插值连起来，
   * 一条因 TTL 淘汰而中断的流会被画成持续有流量 —— 与 §17.3「补 null 不补 0」
   * 的设计意图正好相反，且 UI 上完全看不出来。故在渲染层锁死这个契约。
   */
  it('renders gaps as breaks, not interpolated lines (spanGaps: false)', async () => {
    const { Chart } = await import('chart.js')
    ;(Chart as any).lastConfig = undefined

    const getFlowHistory = vi.fn(async () => flowResp('42', '10.0.0.0'))
    const wrapper = mount(FlowHistoryChart, {
      props: {
        api: { get_flow_history: getFlowHistory } as any,
        instanceId: 'A',
        visible: true,
      },
      global: {
        directives: { tooltip: { mounted() {}, updated() {} } },
      },
    })

    try {
      await flushPromises()
      await nextTick()

      const config = (Chart as any).lastConfig
      expect(config, 'the chart was never constructed').toBeTruthy()

      const datasets = config.data?.datasets ?? []
      expect(datasets.length).toBeGreaterThan(0)
      for (const dataset of datasets) {
        expect(
          dataset.spanGaps,
          'spanGaps must be false so null buckets break the line instead of being interpolated',
        ).toBe(false)
      }
    }
    finally {
      wrapper.unmount()
    }
  })
})
