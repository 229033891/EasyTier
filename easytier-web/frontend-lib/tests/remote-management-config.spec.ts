import { flushPromises, mount } from '@vue/test-utils'
import { describe, expect, it, vi } from 'vitest'
import { nextTick } from 'vue'
import RemoteManagement from '../src/components/RemoteManagement.vue'
import {
  DEFAULT_NETWORK_CONFIG,
  type NetworkConfig,
} from '../src/types/network'

const BOOLEAN_CONFIG_FIELDS = [
  'dhcp',
  'advanced_settings',
  'latency_first',
  'use_smoltcp',
  'disable_ipv6',
  'enable_kcp_proxy',
  'disable_kcp_input',
  'disable_p2p',
  'bind_device',
  'no_tun',
  'enable_exit_node',
  'allow_peer_default_without_exit',
  'relay_all_peer_rpc',
  'multi_thread',
  'enable_relay_network_whitelist',
  'enable_manual_routes',
  'proxy_forward_by_system',
  'disable_encryption',
  'enable_socks5',
  'disable_udp_hole_punching',
  'enable_magic_dns',
  'enable_private_mode',
  'enable_quic_proxy',
  'disable_quic_input',
  'disable_sym_hole_punching',
  'p2p_only',
  'lazy_p2p',
  'need_p2p',
  'disable_upnp',
  'ipv6_public_addr_provider',
  'ipv6_public_addr_auto',
  'disable_relay_data',
  'enable_udp_broadcast_relay',
  'disable_tcp_hole_punching',
] as const satisfies readonly (keyof NetworkConfig)[]

const confirmRequire = vi.fn()

vi.mock('vue-i18n', () => ({
  useI18n: () => ({
    t: (key: string) => key,
    te: () => true,
  }),
}))

vi.mock('primevue', async () => {
  const { defineComponent, h } = await import('vue')

  const PassThrough = defineComponent({
    name: 'PassThrough',
    props: {
      label: String,
      value: String,
    },
    setup(props, { slots }) {
      return () => h('div', {
        'data-label': props.label,
        'data-value': props.value,
        'data-stub': 'pass-through',
      }, slots.default?.())
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

  const SelectStub = defineComponent({
    name: 'Select',
    props: {
      modelValue: Object,
      options: Array,
    },
    emits: ['update:modelValue'],
    setup(props, { slots, emit }) {
      return () => h('div', {
        'data-stub': 'select',
        onClick: () => {
          const opts = (props.options ?? []) as Array<{ uuid: string }>
          const other = opts.find((o) => o.uuid !== (props.modelValue as { uuid?: string } | undefined)?.uuid)
          if (other) {
            emit('update:modelValue', other)
          }
        },
      }, [
        slots.value?.({ value: props.modelValue, placeholder: '' }),
      ])
    },
  })

  const MenuStub = defineComponent({
    name: 'Menu',
    setup(_, { expose }) {
      expose({ toggle: vi.fn() })
      return () => h('div', { 'data-stub': 'menu' })
    },
  })

  return {
    Button: ButtonStub,
    ConfirmDialog: PassThrough,
    ConfirmPopup: PassThrough,
    Divider: PassThrough,
    IftaLabel: PassThrough,
    Menu: MenuStub,
    Message: PassThrough,
    Select: SelectStub,
    Tag: PassThrough,
    useConfirm: () => ({ require: confirmRequire }),
    useToast: () => ({ add: vi.fn() }),
  }
})

const INSTANCE_ID = '00000000-0000-0000-0000-000000000001'
const INSTANCE_ID_B = '00000000-0000-0000-0000-000000000002'
const INSTANCE_UUID = {
  part1: 0,
  part2: 0,
  part3: 0,
  part4: 1,
}
const INSTANCE_UUID_B = {
  part1: 0,
  part2: 0,
  part3: 0,
  part4: 2,
}

function makeFlagConfig(instanceId = INSTANCE_ID): NetworkConfig {
  const config = {
    ...DEFAULT_NETWORK_CONFIG(),
    instance_id: instanceId,
    network_name: 'mesh-save',
  }

  BOOLEAN_CONFIG_FIELDS.forEach((field, index) => {
    config[field] = index % 2 === 0
  })

  return config
}

function cloneConfig(config: NetworkConfig): NetworkConfig {
  return JSON.parse(JSON.stringify(config)) as NetworkConfig
}

function snapshotBooleanConfigFields(config: NetworkConfig): Record<string, unknown> {
  return Object.fromEntries(
    BOOLEAN_CONFIG_FIELDS.map((field) => [field, config[field]]),
  )
}

async function settleRemoteManagement() {
  for (let i = 0; i < 3; i++) {
    await new Promise((resolve) => setTimeout(resolve, 0))
    await flushPromises()
    await nextTick()
  }
}

function makeApi(options: {
  config: NetworkConfig
  disabled?: boolean
  running?: boolean
  secondConfig?: NetworkConfig
}) {
  const { config, disabled = true, running = false, secondConfig } = options
  const configs: Record<string, NetworkConfig> = {
    [config.instance_id]: cloneConfig(config),
  }
  if (secondConfig) {
    configs[secondConfig.instance_id] = cloneConfig(secondConfig)
  }

  return {
    delete_network: vi.fn(),
    generate_config: vi.fn(),
    get_network_config: vi.fn(async (id: string) => cloneConfig(configs[id] ?? config)),
    get_network_info: vi.fn(),
    get_vpn_portal_info: vi.fn(),
    get_network_metas: vi.fn(async (instanceIds: string[]) => ({
      metas: Object.fromEntries(instanceIds.map((id) => [id, {
        config_permission: 0,
        inst_id: id === INSTANCE_ID_B ? INSTANCE_UUID_B : INSTANCE_UUID,
        instance_name: 'mesh-save',
        network_name: 'mesh-save',
        source: 2,
      }])),
    })),
    list_network_instance_ids: vi.fn(async () => ({
      disabled_inst_ids: disabled
        ? [INSTANCE_UUID, ...(secondConfig ? [INSTANCE_UUID_B] : [])]
        : (secondConfig && !running ? [INSTANCE_UUID_B] : []),
      running_inst_ids: running
        ? [INSTANCE_UUID, ...(secondConfig && running ? [] : [])]
        : [],
    })),
    parse_config: vi.fn(),
    run_network: vi.fn(async () => undefined),
    save_config: vi.fn(async () => undefined),
    update_network_instance_state: vi.fn(),
    validate_config: vi.fn(),
  }
}

async function mountRemote(api: ReturnType<typeof makeApi>, props: Record<string, unknown> = {}) {
  const wrapper = mount(RemoteManagement, {
    props: {
      api,
      instanceId: INSTANCE_ID,
      ...props,
    },
    global: {
      directives: {
        tooltip: () => {},
      },
      stubs: {
        Config: {
          template: '<div data-stub="config"><slot name="config-toolbar" /></div>',
        },
        ConfigEditDialog: true,
        Status: true,
      },
    },
  })
  await settleRemoteManagement()
  return wrapper
}

function setupState(wrapper: Awaited<ReturnType<typeof mountRemote>>) {
  return (wrapper.vm as unknown as { $: { setupState: Record<string, any> } }).$.setupState
}

function networkConfigRef(wrapper: Awaited<ReturnType<typeof mountRemote>>): NetworkConfig {
  const cfg = setupState(wrapper).currentNetworkConfig
  return (cfg && typeof cfg === 'object' && 'value' in cfg ? cfg.value : cfg) as NetworkConfig
}

function readDirty(wrapper: Awaited<ReturnType<typeof mountRemote>>): boolean {
  const dirty = setupState(wrapper).isConfigDirty
  return !!(dirty && typeof dirty === 'object' && 'value' in dirty ? dirty.value : dirty)
}

describe('RemoteManagement config save', () => {
  it('saves from the sticky footer without dropping boolean fields', async () => {
    const config = makeFlagConfig()
    const expectedFlags = snapshotBooleanConfigFields(config)
    const api = makeApi({ config })

    const wrapper = await mountRemote(api)

    try {
      const toolbarSave = wrapper.find('.config-toolbar button[data-label="web.device_management.save_config"]')
      expect(toolbarSave.exists()).toBe(false)

      // Clean draft: footer save stays hidden until the first edit.
      expect(wrapper.find('button[data-label="web.device_management.save_config"]').exists()).toBe(false)

      networkConfigRef(wrapper).network_name = 'changed-name'
      await nextTick()
      await flushPromises()

      const saveButton = wrapper.find('button[data-label="web.device_management.save_config"]')
      expect(saveButton.exists()).toBe(true)
      expect(saveButton.attributes('disabled')).toBeUndefined()

      await saveButton.trigger('click')
      await flushPromises()

      expect(api.save_config).toHaveBeenCalledOnce()
      const savedConfig = api.save_config.mock.calls[0][0] as NetworkConfig

      for (const field of BOOLEAN_CONFIG_FIELDS) {
        expect(savedConfig[field], `${field} should be saved`).toBe(expectedFlags[field])
      }
    } finally {
      wrapper.unmount()
    }
  })

  it('marks dirty on edit, clears after discard reload', async () => {
    const config = makeFlagConfig()
    const api = makeApi({ config })
    const wrapper = await mountRemote(api)

    try {
      expect(wrapper.find('[data-value="web.device_management.unsaved_changes"]').exists()).toBe(false)

      networkConfigRef(wrapper).network_name = 'changed-name'
      await nextTick()
      await flushPromises()

      expect(readDirty(wrapper)).toBe(true)
      expect(wrapper.find('[data-value="web.device_management.unsaved_changes"]').exists()).toBe(true)

      const discard = wrapper.find('button[data-label="web.device_management.discard_changes"]')
      expect(discard.exists()).toBe(true)
      await discard.trigger('click')
      await settleRemoteManagement()

      expect(api.get_network_config.mock.calls.length).toBeGreaterThan(1)
      expect(readDirty(wrapper)).toBe(false)
      expect(networkConfigRef(wrapper).network_name).toBe('mesh-save')
    } finally {
      wrapper.unmount()
    }
  })

  it('confirms before switching network when dirty', async () => {
    confirmRequire.mockClear()
    const config = makeFlagConfig()
    const second = makeFlagConfig(INSTANCE_ID_B)
    second.network_name = 'other'
    const api = makeApi({ config, secondConfig: second })
    const wrapper = await mountRemote(api)

    try {
      networkConfigRef(wrapper).network_name = 'dirty'
      await nextTick()
      expect(readDirty(wrapper)).toBe(true)

      await wrapper.find('[data-stub="select"]').trigger('click')
      await flushPromises()

      expect(confirmRequire).toHaveBeenCalled()
      const arg = confirmRequire.mock.calls[0][0] as { message: string }
      expect(arg.message).toBe('web.device_management.confirm_discard_on_switch')
    } finally {
      wrapper.unmount()
    }
  })

  it('confirms before re-running a live network', async () => {
    confirmRequire.mockClear()
    const config = makeFlagConfig()
    const api = makeApi({ config, disabled: false, running: true })
    // Combined mode: enter edit on running instance
    const wrapper = await mountRemote(api)

    try {
      const editBtn = wrapper.find('button[data-label="web.device_management.switch_to_config"]')
      expect(editBtn.exists()).toBe(true)
      await editBtn.trigger('click')
      await settleRemoteManagement()

      const runBtn = wrapper.find('button[data-label="run_network"]')
      expect(runBtn.exists()).toBe(true)
      // Simulate a real click (MouseEvent) — must NOT treat the event as NetworkConfig.
      await runBtn.trigger('click')
      await flushPromises()

      expect(confirmRequire).toHaveBeenCalled()
      const arg = confirmRequire.mock.calls.at(-1)![0] as {
        message: string
        accept?: () => void
      }
      expect(arg.message).toBe('web.device_management.confirm_rerun_network')
      expect(api.run_network).not.toHaveBeenCalled()

      arg.accept?.()
      await settleRemoteManagement()

      expect(api.run_network).toHaveBeenCalledOnce()
      const [runCfg, remoteSave] = api.run_network.mock.calls[0] as [NetworkConfig, boolean]
      expect(runCfg.instance_id).toBe(INSTANCE_ID)
      expect(runCfg.network_name).toBe('mesh-save')
      expect(typeof remoteSave).toBe('boolean')
    } finally {
      wrapper.unmount()
    }
  })
})
