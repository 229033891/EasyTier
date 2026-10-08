import { flushPromises, mount } from '@vue/test-utils'
import { describe, expect, it, vi } from 'vitest'
import { defineComponent, h } from 'vue'
import Status from '../src/components/Status.vue'
import { VpnPortalClientState, type NetworkInstance } from '../src/types/network'

vi.mock('vue-i18n', () => ({
  useI18n: () => ({ t: (key: string) => key }),
}))

vi.mock('../src/components/NetworkChart.vue', () => ({
  default: defineComponent({ render: () => h('div') }),
}))

vi.mock('primevue', () => {
  const PassThrough = defineComponent({
    setup(_, { slots }) {
      return () => h('div', slots.default?.())
    },
  })
  const PanelStub = defineComponent({
    props: {
      header: String,
      collapsed: Boolean,
    },
    emits: ['update:collapsed'],
    setup(props, { emit, slots }) {
      return () => h('section', [
        h('button', {
          type: 'button',
          'data-label': props.header,
          onClick: () => emit('update:collapsed', false),
        }, props.header),
        props.collapsed ? null : slots.default?.(),
      ])
    },
  })
  const CardStub = defineComponent({
    setup(_, { slots }) {
      return () => h('div', [slots.title?.(), slots.content?.()])
    },
  })
  const ButtonStub = defineComponent({
    props: { label: String },
    emits: ['click'],
    setup(props, { emit }) {
      return () => h('button', {
        'data-label': props.label,
        onClick: (event: MouseEvent) => emit('click', event),
      }, props.label)
    },
  })

  return {
    Badge: PassThrough,
    Button: ButtonStub,
    Card: CardStub,
    Chip: PassThrough,
    Column: PassThrough,
  DataTable: PassThrough,
  Dialog: PassThrough,
  Divider: PassThrough,
  Panel: PanelStub,
    ScrollPanel: PassThrough,
    Tag: PassThrough,
  }
})

function runningInstance(): NetworkInstance {
  return {
    instance_id: '12345678-9abc-def0-fedc-ba9876543210',
    running: true,
    error_msg: '',
    detail: {
      dev_name: 'tun0',
      running: true,
      events: [],
      routes: [],
      peers: [],
      peer_route_pairs: [],
      my_node_info: {
        virtual_ipv4: { address: { addr: 0x0a000001 }, network_length: 24 },
        hostname: 'portal-node',
        version: 'test',
        ips: {
          public_ipv4: { addr: 0 },
          interface_ipv4s: [],
          public_ipv6: { part1: 0, part2: 0, part3: 0, part4: 0 },
          interface_ipv6s: [],
          listeners: [],
        },
        stun_info: { udp_nat_type: 0, tcp_nat_type: 0, last_update_time: 0 },
        listeners: [],
        peer_id: 1,
      },
    },
  }
}

describe('Status VPN Portal details', () => {
  it('fetches portal details only when the user opens the panel', async () => {
    const getVpnPortalInfo = vi.fn(async () => ({
      vpn_type: 'wireguard',
      client_config: '',
      connected_clients: [],
      listener: '0.0.0.0:22022',
      clients: [{
        name: 'phone-a',
        virtual_ip: '10.0.0.10',
        groups: ['ops'],
        state: VpnPortalClientState.ONLINE,
        peer_id: 42,
        endpoint: '203.0.113.5:51820',
        tunnel_ip: '192.0.2.1',
        client_config: '[Interface]\nPrivateKey = secret',
      }],
    }))
    const wrapper = mount(Status, {
      props: {
        curNetworkInst: runningInstance(),
        api: { get_vpn_portal_info: getVpnPortalInfo } as any,
      },
      global: {
        directives: { tooltip: () => {} },
        stubs: { HumanEvent: true },
      },
    })

    try {
      expect(getVpnPortalInfo).not.toHaveBeenCalled()

      await wrapper.find('button[data-label="vpn_portal_config"]').trigger('click')
      await flushPromises()

      expect(getVpnPortalInfo).toHaveBeenCalledOnce()
      expect(getVpnPortalInfo).toHaveBeenCalledWith('12345678-9abc-def0-fedc-ba9876543210')
      expect(wrapper.text()).toContain('phone-a · 10.0.0.10')
      expect(wrapper.text()).toContain('203.0.113.5:51820')
      expect(wrapper.text()).toContain('PrivateKey = secret')
    } finally {
      wrapper.unmount()
    }
  })

  it('renders the unconfigured portal sentinel as an empty state', async () => {
    const getVpnPortalInfo = vi.fn(async () => ({
      vpn_type: 'null',
      client_config: '',
      connected_clients: [],
      clients: [],
    }))
    const wrapper = mount(Status, {
      props: {
        curNetworkInst: runningInstance(),
        api: { get_vpn_portal_info: getVpnPortalInfo } as any,
      },
      global: {
        directives: { tooltip: () => {} },
        stubs: { HumanEvent: true },
      },
    })

    try {
      await wrapper.find('button[data-label="vpn_portal_config"]').trigger('click')
      await flushPromises()

      expect(wrapper.text()).toContain('vpn_portal_not_configured')
      expect(wrapper.text()).not.toContain('vpn_portal_type: null')
    } finally {
      wrapper.unmount()
    }
  })
})

describe('Status proxy CIDR route sync row', () => {
  function instanceWithRouteSync(summary?: string): NetworkInstance {
    const inst = runningInstance()
    inst.detail!.proxy_cidr_route_sync = summary
    return inst
  }

  function mountStatus(inst: NetworkInstance) {
    return mount(Status, {
      props: {
        curNetworkInst: inst,
        api: { get_vpn_portal_info: vi.fn() } as any,
      },
      global: {
        directives: { tooltip: () => {} },
        stubs: { HumanEvent: true },
      },
    })
  }

  /** 「节点详情」面板默认折叠，分组内容要展开后才渲染。 */
  async function mountStatusWithDetailsOpen(inst: NetworkInstance) {
    const wrapper = mountStatus(inst)
    await wrapper.find('button[data-label="node_info_details"]').trigger('click')
    await flushPromises()
    return wrapper
  }

  // 空摘要不等于「没数据」，而是「确实一条代理路由都没装」：必须显示，别靠整行消失去推断。
  it('renders the row even when the summary is the empty placeholder', async () => {
    const wrapper = await mountStatusWithDetailsOpen(
      instanceWithRouteSync('desired=[-] installed=[-] exit=false'),
    )
    try {
      expect(wrapper.text()).toContain('node_info_group_proxy_cidr_route_sync')
      expect(wrapper.text()).toContain('desired=[-] installed=[-] exit=false')
    } finally {
      wrapper.unmount()
    }
  })

  it('renders installed proxy CIDRs when the summary reports them', async () => {
    const wrapper = await mountStatusWithDetailsOpen(
      instanceWithRouteSync('desired=[10.0.0.0/24] installed=[10.0.0.0/24] exit=false'),
    )
    try {
      expect(wrapper.text()).toContain('node_info_group_proxy_cidr_route_sync')
      expect(wrapper.text()).toContain('desired=[10.0.0.0/24] installed=[10.0.0.0/24] exit=false')
    } finally {
      wrapper.unmount()
    }
  })

  // 字段缺失（老核心 / 非桌面平台）时仍然不占位。
  it('omits the row when the field is absent', async () => {
    const wrapper = await mountStatusWithDetailsOpen(instanceWithRouteSync(undefined))
    try {
      expect(wrapper.text()).not.toContain('node_info_group_proxy_cidr_route_sync')
    } finally {
      wrapper.unmount()
    }
  })

  it('omits the row for a blank summary', async () => {
    const wrapper = await mountStatusWithDetailsOpen(instanceWithRouteSync('   '))
    try {
      expect(wrapper.text()).not.toContain('node_info_group_proxy_cidr_route_sync')
    } finally {
      wrapper.unmount()
    }
  })
})
