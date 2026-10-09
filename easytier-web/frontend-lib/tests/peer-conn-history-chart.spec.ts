import { flushPromises, mount } from '@vue/test-utils'
import { describe, expect, it, vi } from 'vitest'
import { nextTick } from 'vue'
import PeerConnHistoryChart from '../src/components/PeerConnHistoryChart.vue'
import type { PeerConnHistoryResponse } from '../src/modules/api'

const chartStats = vi.hoisted(() => ({
  created: 0,
  destroyed: 0,
}))

vi.mock('vue-i18n', () => ({
  useI18n: () => ({
    t: (key: string) => key,
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

  const SelectStub = dc({
    name: 'Select',
    props: {
      modelValue: [String, Number, Object],
      options: Array,
    },
    emits: ['update:modelValue'],
    setup() {
      return () => hh('div', { 'data-stub': 'select' })
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
    Select: SelectStub,
    SelectButton: SelectButtonStub,
  }
})

vi.mock('chart.js', () => {
  class ChartMock {
    static register = vi.fn()
    data: { labels: unknown[], datasets: Array<{ data: unknown }> }
    constructor(
      _canvas: unknown,
      config?: { data?: { labels?: unknown[], datasets?: Array<{ data?: unknown }> } },
    ) {
      chartStats.created += 1
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

function historyResp(peerId: number, hostname: string): PeerConnHistoryResponse {
  return {
    peers: [{
      peer_id: peerId,
      hostname,
      remote_addr: '1.2.3.4:11010',
      tunnel_type: 'udp',
      points: [
        { t: 1_700_000_000, latency_us: 5000, loss_rate: 0.01, jitter_us: 200, rx_bytes: 100, tx_bytes: 200 },
        { t: 1_700_000_060, latency_us: 6000, loss_rate: 0.02, jitter_us: 300, rx_bytes: 150, tx_bytes: 250 },
      ],
    }],
  }
}

describe('PeerConnHistoryChart remount', () => {
  it('recreates Chart instances after instanceId switch unmounts canvas', async () => {
    chartStats.created = 0
    chartStats.destroyed = 0

    const getHistory = vi.fn(async (instanceId: string) => {
      if (instanceId === 'A')
        return historyResp(1, 'peer-a')
      return historyResp(2, 'peer-b')
    })

    const TooltipDirective = {
      mounted() {},
      updated() {},
    }

    const wrapper = mount(PeerConnHistoryChart, {
      props: {
        api: { get_peer_conn_history: getHistory } as any,
        instanceId: 'A',
        visible: true,
      },
      global: {
        directives: { tooltip: TooltipDirective },
        stubs: {
          // Keep canvas in DOM for Chart constructor paths.
        },
      },
    })

    try {
      await flushPromises()
      await nextTick()
      expect(wrapper.findAll('canvas')).toHaveLength(4)
      expect(chartStats.created).toBe(4)

      await wrapper.setProps({ instanceId: 'B' })
      await flushPromises()
      await nextTick()
      await flushPromises()

      expect(wrapper.findAll('canvas')).toHaveLength(4)
      // Without destroyCharts on clear, created stays 4 and new canvases stay blank.
      expect(chartStats.destroyed).toBeGreaterThanOrEqual(4)
      expect(chartStats.created).toBe(8)
      expect(getHistory).toHaveBeenCalledWith('B', expect.any(Number))
    }
    finally {
      wrapper.unmount()
    }
  })
})
