import { mount, type VueWrapper } from '@vue/test-utils'
import { describe, expect, it, vi } from 'vitest'
import { defineComponent, h, nextTick, reactive } from 'vue'
import Config from '../src/components/Config.vue'
import {
  DEFAULT_NETWORK_CONFIG,
  toBackendNetworkConfig,
  type NetworkConfig,
} from '../src/types/network'

vi.mock('primevue', async () => {
  const actual = await vi.importActual<typeof import('primevue')>('primevue')
  return {
    ...actual,
    useToast: () => ({ add: vi.fn() }),
    useConfirm: () => ({ require: vi.fn() }),
  }
})

const CONFIG_FLAG_FIELDS = [
  'latency_first',
  'use_smoltcp',
  'ipv6_public_addr_auto',
  'enable_kcp_proxy',
  'disable_kcp_input',
  'enable_quic_proxy',
  'disable_quic_input',
  'disable_p2p',
  'p2p_only',
  'lazy_p2p',
  'bind_device',
  'no_tun',
  'enable_exit_node',
  'allow_peer_default_without_exit',
  'relay_all_peer_rpc',
  'need_p2p',
  'multi_thread',
  'proxy_forward_by_system',
  'disable_tcp_hole_punching',
  'disable_udp_hole_punching',
  'enable_udp_broadcast_relay',
  'disable_upnp',
  'disable_sym_hole_punching',
  'enable_magic_dns',
  'enable_private_mode',
] as const satisfies readonly (keyof NetworkConfig)[]

const CONFIG_CHECKBOX_FIELDS = [
  ['dhcp', '#virtual_ip_auto'],
  ...CONFIG_FLAG_FIELDS.map((field) => [field, `#${field}`] as const),
] as const satisfies readonly (readonly [keyof NetworkConfig, string])[]

/**
 * 「高级设置」面板中三个布尔开关区（网络白名单 / 自定义路由 / SOCKS5）的渲染顺序。
 * VPN Portal 当前是面板内第一个 ToggleButton，IPv6 公网地址提供者排在这三个开关之后。
 */
const CONFIG_TOGGLE_FIELDS = [
  'enable_relay_network_whitelist',
  'enable_manual_routes',
  'enable_socks5',
] as const satisfies readonly (keyof NetworkConfig)[]

/** 非 VPN ToggleButton 在序列中的起始下标 */
const CONFIG_TOGGLE_START_INDEX = 1

/** 「VPN Portal」开关在 ToggleButton 序列中的下标 */
const VPN_PORTAL_TOGGLE_INDEX = 0

const CONFIG_UI_BOOLEAN_FIELDS = [
  ...CONFIG_CHECKBOX_FIELDS.map(([field]) => field),
  ...CONFIG_TOGGLE_FIELDS,
] as const satisfies readonly (keyof NetworkConfig)[]

vi.mock('vue-i18n', () => ({
  useI18n: () => ({
    t: (key: string, values?: unknown[]) => values ? `${key}:${values.join(',')}` : key,
  }),
}))

const PassThrough = defineComponent({
  name: 'PassThrough',
  setup(_, { slots }) {
    return () => h('div', slots.default?.())
  },
})

const PanelStub = defineComponent({
  name: 'Panel',
  props: {
    header: String,
  },
  setup(props, { slots }) {
    return () => h('section', { 'data-stub': 'panel', 'data-header': props.header }, slots.default?.())
  },
})

const DividerStub = defineComponent({
  name: 'Divider',
  setup() {
    return () => h('hr', { 'data-stub': 'divider' })
  },
})

function splitList(value: string): string[] {
  return value.split(',').map((item) => item.trim()).filter((item) => item.length > 0)
}

const InputTextStub = defineComponent({
  name: 'InputText',
  props: {
    modelValue: [String, Number],
    id: String,
    disabled: Boolean,
  },
  emits: ['update:modelValue'],
  setup(props, { attrs, emit }) {
    return () => h('input', {
      ...attrs,
      id: props.id,
      disabled: props.disabled,
      value: props.modelValue ?? '',
      'data-stub': 'input-text',
      onInput: (event: Event) => emit('update:modelValue', (event.target as HTMLInputElement).value),
    })
  },
})

const PasswordStub = defineComponent({
  name: 'Password',
  props: {
    modelValue: [String, Number],
    id: String,
    disabled: Boolean,
  },
  emits: ['update:modelValue'],
  setup(props, { attrs, emit }) {
    return () => h('input', {
      ...attrs,
      id: props.id,
      disabled: props.disabled,
      type: 'password',
      value: props.modelValue ?? '',
      'data-stub': 'password',
      onInput: (event: Event) => emit('update:modelValue', (event.target as HTMLInputElement).value),
    })
  },
})

const InputNumberStub = defineComponent({
  name: 'InputNumber',
  props: {
    modelValue: Number,
    id: String,
    inputId: String,
    disabled: Boolean,
  },
  emits: ['update:modelValue'],
  setup(props, { attrs, emit }) {
    return () => h('input', {
      ...attrs,
      id: props.id ?? props.inputId,
      disabled: props.disabled,
      type: 'number',
      value: props.modelValue ?? '',
      'data-stub': 'input-number',
      onInput: (event: Event) => {
        const value = (event.target as HTMLInputElement).value
        emit('update:modelValue', value === '' ? null : Number(value))
      },
    })
  },
})

const CheckboxStub = defineComponent({
  name: 'Checkbox',
  props: {
    modelValue: Boolean,
    inputId: String,
    disabled: Boolean,
  },
  emits: ['update:modelValue'],
  setup(props, { attrs, emit }) {
    return () => h('input', {
      ...attrs,
      id: props.inputId,
      checked: props.modelValue,
      disabled: props.disabled,
      type: 'checkbox',
      'data-stub': 'checkbox',
      onChange: (event: Event) => {
        if (props.disabled) return
        emit('update:modelValue', (event.target as HTMLInputElement).checked)
      },
    })
  },
})

const ToggleButtonStub = defineComponent({
  name: 'ToggleButton',
  props: {
    modelValue: Boolean,
    onIcon: String,
    offIcon: String,
    onLabel: String,
    offLabel: String,
    disabled: Boolean,
  },
  emits: ['update:modelValue'],
  setup(props, { emit }) {
    return () => h('button', {
      type: 'button',
      disabled: props.disabled,
      'aria-pressed': String(Boolean(props.modelValue)),
      'data-stub': 'toggle-button',
      onClick: () => {
        if (props.disabled) return
        emit('update:modelValue', !props.modelValue)
      },
    }, props.modelValue ? props.onLabel : props.offLabel)
  },
})

const AutoCompleteStub = defineComponent({
  name: 'AutoComplete',
  props: {
    modelValue: Array,
    id: String,
    multiple: Boolean,
  },
  emits: ['update:modelValue', 'complete'],
  setup(props, { attrs, emit }) {
    return () => h('input', {
      ...attrs,
      id: props.id,
      value: (props.modelValue ?? []).join(','),
      'data-stub': 'auto-complete',
      onInput: (event: Event) => emit('update:modelValue', splitList((event.target as HTMLInputElement).value)),
    })
  },
})

const MultiSelectStub = defineComponent({
  name: 'MultiSelect',
  props: {
    modelValue: Array,
    inputId: String,
    appendTo: String,
  },
  emits: ['update:modelValue'],
  setup(props, { attrs, emit }) {
    return () => h('input', {
      ...attrs,
      id: props.inputId,
      'data-append-to': props.appendTo,
      value: (props.modelValue ?? []).join(','),
      'data-stub': 'multi-select',
      onInput: (event: Event) => emit('update:modelValue', splitList((event.target as HTMLInputElement).value)),
    })
  },
})

const UrlListInputStub = defineComponent({
  name: 'UrlListInput',
  props: {
    modelValue: Array,
    id: String,
    addLabel: String,
  },
  emits: ['update:modelValue'],
  setup(props, { attrs, emit }) {
    return () => h('input', {
      ...attrs,
      id: props.id,
      value: (props.modelValue ?? []).join(','),
      'data-stub': 'url-list-input',
      'data-add-label': props.addLabel,
      onInput: (event: Event) => emit('update:modelValue', splitList((event.target as HTMLInputElement).value)),
    })
  },
})

const SelectButtonStub = defineComponent({
  name: 'SelectButton',
  props: {
    modelValue: String,
    options: Array,
  },
  emits: ['update:modelValue'],
  setup(props, { emit }) {
    return () => h('select', {
      value: props.modelValue,
      'data-stub': 'select-button',
      onChange: (event: Event) => emit('update:modelValue', (event.target as HTMLSelectElement).value),
    }, (props.options ?? []).map((option) => h('option', { value: option as string }, option as string)))
  },
})

const SelectStub = defineComponent({
  name: 'Select',
  props: {
    modelValue: [String, Number],
    id: String,
    inputId: String,
    disabled: Boolean,
    options: Array,
  },
  emits: ['update:modelValue'],
  setup(props, { attrs, emit }) {
    return () => h('select', {
      ...attrs,
      id: props.id ?? props.inputId,
      disabled: props.disabled,
      value: props.modelValue ?? '',
      'data-stub': 'select',
      onChange: (event: Event) => {
        const raw = (event.target as HTMLSelectElement).value
        const numeric = Number(raw)
        emit('update:modelValue', Number.isFinite(numeric) && String(numeric) === raw ? numeric : raw)
      },
    }, (props.options ?? []).map((option) => {
      if (typeof option === 'string' || typeof option === 'number') {
        return h('option', { value: String(option) }, String(option))
      }
      const { label, value } = (option ?? {}) as { label?: unknown, value?: unknown }
      const optionValue = value ?? label ?? ''
      return h('option', { value: String(optionValue) }, String(label ?? optionValue))
    }))
  },
})

const ButtonStub = defineComponent({
  name: 'Button',
  props: {
    label: String,
    icon: String,
    disabled: Boolean,
  },
  emits: ['click'],
  setup(props, { slots, emit }) {
    return () => h('button', {
      type: 'button',
      disabled: props.disabled,
      'data-label': props.label ?? props.icon,
      onClick: (event: MouseEvent) => emit('click', event),
    }, slots.default?.() ?? props.label ?? props.icon)
  },
})

const DialogStub = defineComponent({
  name: 'Dialog',
  props: {
    visible: Boolean,
  },
  setup(props, { slots }) {
    return () => h('div', { hidden: !props.visible, 'data-stub': 'dialog' }, [
      slots.default?.(),
      slots.footer?.(),
    ])
  },
})

const AclManagerStub = defineComponent({
  name: 'AclManager',
  props: {
    modelValue: Object,
  },
  emits: ['update:modelValue'],
  setup(props) {
    return () => h('pre', { 'data-stub': 'acl-manager' }, JSON.stringify(props.modelValue))
  },
})

function makeConfig(): NetworkConfig {
  const config = DEFAULT_NETWORK_CONFIG()

  return {
    ...config,
    dhcp: false,
    virtual_ipv4: '10.1.2.3',
    network_length: 24,
    network_name: 'mesh-a',
    network_secret: 'secret-a',
    peer_urls: ['tcp://peer-a:11010', 'udp://peer-b:11010'],
    latency_first: true,
    use_smoltcp: true,
    disable_ipv6: true,
    no_tun: true,
    hostname: 'host-a',
    proxy_cidrs: ['10.10.0.0/16', '172.16.1.0/24'],
    vpn_portal_config: {
      wireguard_listen: '0.0.0.0:22023',
      wireguard_private_key: 'portal-private-key',
      clients: [{
        name: 'phone-a',
        virtual_ip: '10.1.2.10/24',
        groups: ['ops'],
      }],
    },
    listener_urls: ['tcp://0.0.0.0:12010'],
    dev_name: 'tun-test',
    mtu: 1280,
    instance_recv_bps_limit: '9007199254740993',
    enable_relay_network_whitelist: true,
    relay_network_whitelist: ['network-a'],
    enable_manual_routes: true,
    routes: ['192.168.0.0/16'],
    enable_socks5: true,
    socks5_port: 1086,
    exit_nodes: ['exit-a'],
    mapped_listeners: ['tcp://127.0.0.1:22000'],
    port_forwards: [{
      proto: 'udp',
      bind_ip: '0.0.0.0',
      bind_port: 18080,
      dst_ip: '10.0.0.2',
      dst_port: 8080,
    }],
  }
}

function mountConfig(config: NetworkConfig = makeConfig()) {
  const curNetwork = reactive(config) as NetworkConfig
  const wrapper = mount(Config, {
    props: {
      curNetwork,
      hostname: 'host-from-prop',
    },
    global: {
      directives: {
        tooltip: () => {},
      },
      stubs: {
        AclManager: AclManagerStub,
        AutoComplete: AutoCompleteStub,
        Button: ButtonStub,
        Checkbox: CheckboxStub,
        Dialog: DialogStub,
        Divider: DividerStub,
        InputGroup: PassThrough,
        InputGroupAddon: PassThrough,
        InputNumber: InputNumberStub,
        InputText: InputTextStub,
        MultiSelect: MultiSelectStub,
        Panel: PanelStub,
        Password: PasswordStub,
        Select: SelectStub,
        SelectButton: SelectButtonStub,
        ToggleButton: ToggleButtonStub,
        UrlListInput: UrlListInputStub,
      },
    },
  })

  return { curNetwork, wrapper }
}

function input(wrapper: VueWrapper, selector: string): HTMLInputElement {
  return wrapper.find(selector).element as HTMLInputElement
}

async function setInput(wrapper: VueWrapper, selector: string, value: string) {
  await wrapper.find(selector).setValue(value)
  await nextTick()
}

describe('Config.vue network config projection', () => {
  it('projects config values into the visible form controls', async () => {
    const { curNetwork, wrapper } = mountConfig()
    await nextTick()

    expect(input(wrapper, '#network_name').value).toBe('mesh-a')
    expect(input(wrapper, '#network_secret').value).toBe('secret-a')
    expect(input(wrapper, '#virtual_ip').value).toBe('10.1.2.3')
    expect(input(wrapper, '#initial_nodes').value).toBe('tcp://peer-a:11010,udp://peer-b:11010')
    expect(input(wrapper, '#virtual_ip_auto').checked).toBe(false)
    expect(input(wrapper, '#latency_first').checked).toBe(true)
    expect(input(wrapper, '#use_smoltcp').checked).toBe(true)
    expect(input(wrapper, '#allow_ipv6').checked).toBe(false)
    expect(input(wrapper, '#no_tun').checked).toBe(true)

    expect(input(wrapper, '#hostname').value).toBe('host-a')
    expect(input(wrapper, '#subnet-proxy').value).toBe('10.10.0.0/16,172.16.1.0/24')
    expect(input(wrapper, '#vpn_portal_wireguard_listen').value).toBe('0.0.0.0:22023')
    expect(input(wrapper, '#vpn_portal_wireguard_private_key').value).toBe('portal-private-key')
    expect(input(wrapper, '#vpn_portal_client_name_0').value).toBe('phone-a')
    expect(input(wrapper, '#vpn_portal_client_virtual_ip_0').value).toBe('10.1.2.10/24')
    expect(input(wrapper, '#vpn_portal_client_groups_0').value).toBe('ops')
    expect(input(wrapper, '#dev_name').value).toBe('tun-test')
    expect(input(wrapper, '#mtu').value).toBe('1280')
    expect(input(wrapper, '#instance_recv_bps_limit').value).toBe('9007199254740993')
    expect(input(wrapper, '#relay_network_whitelist').value).toBe('network-a')
    expect(input(wrapper, '#routes').value).toBe('192.168.0.0/16')
    expect(input(wrapper, '#socks5_port').value).toBe('1086')
    expect(input(wrapper, '#exit_nodes').value).toBe('exit-a')
    expect(input(wrapper, 'input[data-add-label="add_listener_url"]').value).toBe('tcp://0.0.0.0:12010')
    expect(input(wrapper, 'input[data-add-label="add_mapped_listener"]').value).toBe('tcp://127.0.0.1:22000')

    expect(wrapper.find<HTMLSelectElement>('#port_forward_proto_0').element.value).toBe('udp')
    expect(input(wrapper, 'input[placeholder="port_forwards_bind_addr"]').value).toBe('0.0.0.0')
    expect(input(wrapper, 'input[placeholder="port_forwards_dst_addr"]').value).toBe('10.0.0.2')
    expect(wrapper.findComponent(AclManagerStub).props('modelValue')).toStrictEqual(curNetwork.acl)
  })

  it('projects form edits back into config and backend JSON', async () => {
    const { curNetwork, wrapper } = mountConfig()
    await nextTick()

    await wrapper.find('#virtual_ip_auto').setValue(false)
    await setInput(wrapper, '#network_name', 'mesh-edited')
    await setInput(wrapper, '#network_secret', 'secret-edited')
    await setInput(wrapper, '#virtual_ip', '10.7.7.7')
    await setInput(wrapper, '#initial_nodes', ' tcp://peer-x:11010, , udp://peer-y:11010 ')
    await wrapper.find('#no_tun').setValue(false)
    await wrapper.find('#allow_ipv6').setValue(true)
    await setInput(wrapper, '#hostname', 'host-edited')
    await setInput(wrapper, '#subnet-proxy', '10.7.0.0/16,172.17.0.0/16')
    await setInput(wrapper, '#vpn_portal_wireguard_listen', '[::]:23000')
    await setInput(wrapper, '#vpn_portal_wireguard_private_key', 'edited-private-key')
    await setInput(wrapper, '#vpn_portal_client_name_0', 'laptop-a')
    await setInput(wrapper, '#vpn_portal_client_virtual_ip_0', '10.1.2.20/24')
    await setInput(wrapper, '#vpn_portal_client_groups_0', 'ops,admin')
    await setInput(wrapper, 'input[data-add-label="add_listener_url"]', 'tcp://0.0.0.0:13010')
    await setInput(wrapper, '#dev_name', 'tun-edited')
    await setInput(wrapper, '#mtu', '1260')
    await setInput(wrapper, '#instance_recv_bps_limit', '9007199254740993')
    await setInput(wrapper, '#relay_network_whitelist', 'network-edited')
    await setInput(wrapper, '#routes', '192.168.10.0/24')
    await setInput(wrapper, '#socks5_port', '1089')
    await setInput(wrapper, '#exit_nodes', 'exit-edited')
    await setInput(wrapper, 'input[data-add-label="add_mapped_listener"]', 'tcp://127.0.0.1:23000')
    await wrapper.find('#port_forward_proto_0').setValue('tcp')
    await setInput(wrapper, 'input[placeholder="port_forwards_bind_addr"]', '127.0.0.1')
    await setInput(wrapper, 'input[placeholder="port_forwards_dst_addr"]', '10.9.0.2')

    const portNumbers = wrapper.findAll<HTMLInputElement>('input#horizontal-buttons')
    await portNumbers[1].setValue('19090')
    await portNumbers[2].setValue('9090')

    expect(curNetwork).toMatchObject({
      dhcp: false,
      virtual_ipv4: '10.7.7.7',
      network_name: 'mesh-edited',
      network_secret: 'secret-edited',
      peer_urls: ['tcp://peer-x:11010', 'udp://peer-y:11010'],
      no_tun: false,
      disable_ipv6: false,
      hostname: 'host-edited',
      proxy_cidrs: ['10.7.0.0/16', '172.17.0.0/16'],
      vpn_portal_config: {
        wireguard_listen: '[::]:23000',
        wireguard_private_key: 'edited-private-key',
        clients: [{
          name: 'laptop-a',
          virtual_ip: '10.1.2.20/24',
          groups: ['ops', 'admin'],
        }],
      },
      listener_urls: ['tcp://0.0.0.0:13010'],
      dev_name: 'tun-edited',
      mtu: 1260,
      instance_recv_bps_limit: '9007199254740993',
      relay_network_whitelist: ['network-edited'],
      routes: ['192.168.10.0/24'],
      socks5_port: 1089,
      exit_nodes: ['exit-edited'],
      mapped_listeners: ['tcp://127.0.0.1:23000'],
      port_forwards: [{
        proto: 'tcp',
        bind_ip: '127.0.0.1',
        bind_port: 19090,
        dst_ip: '10.9.0.2',
        dst_port: 9090,
      }],
    })

    const backend = toBackendNetworkConfig(curNetwork)
    expect(backend).toMatchObject({
      virtual_ipv4: '10.7.7.7',
      network_name: 'mesh-edited',
      network_secret: 'secret-edited',
      peer_urls: ['tcp://peer-x:11010', 'udp://peer-y:11010'],
      listener_urls: ['tcp://0.0.0.0:13010'],
      mtu: 1260,
      instance_recv_bps_limit: '9007199254740993',
      vpn_portal_config: {
        wireguard_listen: '[::]:23000',
        wireguard_private_key: 'edited-private-key',
        clients: [{
          name: 'laptop-a',
          virtual_ip: '10.1.2.20/24',
          groups: ['ops', 'admin'],
        }],
      },
      port_forwards: [{
        proto: 'tcp',
        bind_ip: '127.0.0.1',
        bind_port: 19090,
        dst_ip: '10.9.0.2',
        dst_port: 9090,
      }],
    })
  })

  it('loads legacy single default_protocol and stores ordered CSV from MultiSelect', async () => {
    const { curNetwork, wrapper } = mountConfig({
      ...makeConfig(),
      default_protocol: 'udp',
    })
    await nextTick()

    expect(curNetwork.default_protocol).toBe('udp')
    const multi = wrapper.find('#default_protocol')
    expect(multi.exists()).toBe(true)

    curNetwork.default_protocol = 'wss,tcp,udp'
    await nextTick()
    expect(curNetwork.default_protocol).toBe('wss,tcp,udp')
    const backend = toBackendNetworkConfig(curNetwork) as Record<string, unknown>
    expect(backend.default_protocol).toBe('wss,tcp,udp')
  })

  it('projects bond count controls and omits them from backend JSON when blank', async () => {
    const { curNetwork, wrapper } = mountConfig()
    await nextTick()

    // Blank (null) means unset: backend falls back to bond_count=1 / replica_fill_max=5.
    expect(input(wrapper, '#peer_link_bond_count').value).toBe('')
    expect(input(wrapper, '#peer_link_replica_fill_max').value).toBe('')
    const blankBackend = toBackendNetworkConfig(curNetwork) as Record<string, unknown>
    expect('peer_link_bond_count' in blankBackend).toBe(false)
    expect('peer_link_replica_fill_max' in blankBackend).toBe(false)

    await setInput(wrapper, '#peer_link_bond_count', '3')
    await setInput(wrapper, '#peer_link_replica_fill_max', '4')
    expect(curNetwork.peer_link_bond_count).toBe(3)
    expect(curNetwork.peer_link_replica_fill_max).toBe(4)
    const backend = toBackendNetworkConfig(curNetwork) as Record<string, unknown>
    expect(backend.peer_link_bond_count).toBe(3)
    expect(backend.peer_link_replica_fill_max).toBe(4)
  })

  it('round-trips every visible boolean config control into backend JSON', async () => {
    const config = makeConfig()
    const originalFlagValues = new Map(
      CONFIG_UI_BOOLEAN_FIELDS.map((field, index) => {
        const value = index % 2 === 0
        config[field] = value
        return [field, value]
      }),
    )

    const { curNetwork, wrapper } = mountConfig(config)
    await nextTick()

    const toggledFields = new Set<keyof NetworkConfig>()

    for (const [field, selector] of CONFIG_CHECKBOX_FIELDS) {
      const checkbox = wrapper.find(selector)
      // Hidden by dependency rules (e.g. ipv6_public_addr_auto under disable_ipv6).
      if (!checkbox.exists()) {
        continue
      }
      const el = input(wrapper, selector)
      // Disabled by mutex rules — skip toggle; covered by config-conflicts.spec.ts.
      if (el.disabled) {
        expect(el.checked, `${field} should still project while disabled`).toBe(originalFlagValues.get(field))
        continue
      }
      const value = originalFlagValues.get(field)
      expect(el.checked, `${field} should project into UI`).toBe(value)
      await checkbox.setValue(!value)
      await nextTick()
      toggledFields.add(field)
    }

    const toggleButtons = wrapper.findAll('button[data-stub="toggle-button"]')
    const expectedToggleCount =
      CONFIG_TOGGLE_FIELDS.length +
      CONFIG_TOGGLE_START_INDEX +
      (curNetwork.disable_ipv6 ? 0 : 1)
    expect(toggleButtons).toHaveLength(expectedToggleCount)
    for (const [index, field] of CONFIG_TOGGLE_FIELDS.entries()) {
      const value = originalFlagValues.get(field)
      const toggle = toggleButtons[index + CONFIG_TOGGLE_START_INDEX]
      expect(toggle.attributes('aria-pressed'), `${field} should project into UI`)
        .toBe(String(value))
      if (toggle.attributes('disabled') !== undefined) {
        continue
      }
      await toggle.trigger('click')
      await nextTick()
      toggledFields.add(field)
    }

    const backend = toBackendNetworkConfig(curNetwork) as Record<string, unknown>
    for (const field of toggledFields) {
      const value = originalFlagValues.get(field)
      const expectedValue = !value
      expect(curNetwork[field], `${field} should update config`).toBe(expectedValue)
      expect(backend[field], `${field} should be preserved in backend JSON`).toBe(expectedValue)
    }
  })

  it('shows conflict banners for legacy disable_p2p + p2p_only without rewriting values', async () => {
    const config = makeConfig()
    config.disable_p2p = true
    config.p2p_only = true
    config.lazy_p2p = false
    config.need_p2p = false
    const { curNetwork, wrapper } = mountConfig(config)
    await nextTick()

    expect(wrapper.text()).toContain('disable_p2p_conflict_help')
    expect(curNetwork.disable_p2p).toBe(true)
    expect(curNetwork.p2p_only).toBe(true)
  })

  it('renders disable_p2p as an advanced flag while prefer_peer_relay stays hidden', async () => {
    const config = makeConfig()
    const { wrapper } = mountConfig(config)
    await nextTick()

    // disable_p2p 是硬约束，保留在高级设置；prefer_peer_relay UI 隐藏（TOML 可选 OSPF 投影）
    expect(wrapper.find('#disable_p2p').exists()).toBe(true)
    expect(wrapper.find('#prefer_peer_relay').exists()).toBe(false)
  })

  it('drives relay permission from the basic allow-relay switch', async () => {
    const config = makeConfig()
    const { curNetwork, wrapper } = mountConfig(config)
    await nextTick()

    // 默认允许中转（disable_relay_data: false）
    expect(input(wrapper, '#allow_relay').checked).toBe(true)

    await wrapper.find('#allow_relay').setValue(false)
    await nextTick()
    expect(curNetwork.disable_relay_data).toBe(true)
    expect(toBackendNetworkConfig(curNetwork).disable_relay_data).toBe(true)

    await wrapper.find('#allow_relay').setValue(true)
    await nextTick()
    expect(curNetwork.disable_relay_data).toBe(false)
  })

  it('blocks re-checking allow-relay under p2p_only but allows unchecking', async () => {
    const config = makeConfig()
    config.p2p_only = true
    config.disable_relay_data = false
    const { wrapper } = mountConfig(config)
    await nextTick()

    // 已勾选 + p2p_only：可取消（checkbox 启用，点击后 disallow）
    expect(input(wrapper, '#allow_relay').disabled).toBe(false)
    await wrapper.find('#allow_relay').setValue(false)
    await nextTick()

    // 未勾选 + p2p_only：禁止重新勾选
    expect(input(wrapper, '#allow_relay').disabled).toBe(true)
  })

  it('binds inverted switches positively (checked = enabled)', async () => {
    const config = makeConfig()
    config.disable_ipv6 = true
    config.disable_encryption = true
    const { curNetwork, wrapper } = mountConfig(config)
    await nextTick()

    expect(input(wrapper, '#allow_ipv6').checked).toBe(false)
    expect(input(wrapper, '#allow_encryption').checked).toBe(false)

    await wrapper.find('#allow_ipv6').setValue(true)
    await wrapper.find('#allow_encryption').setValue(true)
    await nextTick()

    expect(curNetwork.disable_ipv6).toBe(false)
    expect(curNetwork.disable_encryption).toBe(false)
    expect(toBackendNetworkConfig(curNetwork).disable_ipv6).toBe(false)
    expect(toBackendNetworkConfig(curNetwork).disable_encryption).toBe(false)

    await wrapper.find('#allow_ipv6').setValue(false)
    await nextTick()
    expect(curNetwork.disable_ipv6).toBe(true)
  })

  it('disables TUN name/MTU when no_tun is on and hides encryption algo when encryption is off', async () => {
    const config = makeConfig()
    config.no_tun = true
    config.disable_encryption = true
    const { wrapper } = mountConfig(config)
    await nextTick()

    expect(input(wrapper, '#dev_name').disabled).toBe(true)
    expect(input(wrapper, '#mtu').disabled).toBe(true)
    expect(wrapper.find('#encryption_algorithm').exists()).toBe(false)
    expect(wrapper.text()).toContain('disable_encryption_algo_hint')
  })

  it('uses VPN Portal config presence as the enable switch', async () => {
    const config = DEFAULT_NETWORK_CONFIG()
    const { curNetwork, wrapper } = mountConfig(config)
    await nextTick()

    const portalToggle = wrapper.findAll('button[data-stub="toggle-button"]')[VPN_PORTAL_TOGGLE_INDEX]
    expect(portalToggle.attributes('aria-pressed')).toBe('false')

    await portalToggle.trigger('click')
    await nextTick()
    expect(curNetwork.vpn_portal_config).toEqual({
      wireguard_listen: '0.0.0.0:22022',
      clients: [],
    })

    await portalToggle.trigger('click')
    await nextTick()
    expect(curNetwork.vpn_portal_config).toBeUndefined()
  })

  it('keeps each VPN Portal client row bound to the same client when reordered', async () => {
    const config = makeConfig()
    config.vpn_portal_config!.clients.push({
      name: 'phone-b',
      virtual_ip: '10.1.2.11',
      groups: ['guests'],
    })
    const { curNetwork, wrapper } = mountConfig(config)
    await nextTick()

    const firstClient = curNetwork.vpn_portal_config!.clients[0]
    const secondClient = curNetwork.vpn_portal_config!.clients[1]
    const firstClientInput = input(wrapper, '#vpn_portal_client_name_0')
    curNetwork.vpn_portal_config!.clients = [secondClient, firstClient]
    await nextTick()

    expect(input(wrapper, '#vpn_portal_client_name_1')).toBe(firstClientInput)
    await setInput(wrapper, '#vpn_portal_client_name_1', 'phone-a-edited')
    expect(firstClient.name).toBe('phone-a-edited')
    expect(secondClient.name).toBe('phone-b')
  })

  it('keeps VPN Portal ACL group menus inside the management drawer', async () => {
    const { wrapper } = mountConfig()
    await nextTick()

    expect(wrapper.find('#vpn_portal_client_groups_0').attributes('data-append-to')).toBe('self')
  })

  it('keeps uint64 input editable without losing large values', async () => {
    const { curNetwork, wrapper } = mountConfig()
    await nextTick()

    await setInput(wrapper, '#instance_recv_bps_limit', '1234')
    expect(curNetwork.instance_recv_bps_limit).toBe(1234)

    await setInput(wrapper, '#instance_recv_bps_limit', 'not-a-number')
    expect(curNetwork.instance_recv_bps_limit).toBe(1234)

    await setInput(wrapper, '#instance_recv_bps_limit', '0')
    expect(curNetwork.instance_recv_bps_limit).toBeNull()
    expect(input(wrapper, '#instance_recv_bps_limit').value).toBe('')

    await setInput(wrapper, '#instance_recv_bps_limit', '9007199254740993')
    expect(curNetwork.instance_recv_bps_limit).toBe('9007199254740993')

    await setInput(wrapper, '#instance_recv_bps_limit', '18446744073709551616')
    expect(curNetwork.instance_recv_bps_limit).toBe('9007199254740993')

    await setInput(wrapper, '#instance_recv_bps_limit', '')
    expect(curNetwork.instance_recv_bps_limit).toBeNull()
  })

  it('emits runNetwork with the current projected config', async () => {
    const { curNetwork, wrapper } = mountConfig()
    await nextTick()

    await setInput(wrapper, '#network_name', 'mesh-running')
    await wrapper.find('button[data-label="run_network"]').trigger('click')

    expect(wrapper.emitted('runNetwork')?.[0]).toEqual([curNetwork])
    expect((wrapper.emitted('runNetwork')?.[0][0] as NetworkConfig).network_name).toBe('mesh-running')
  })
})
