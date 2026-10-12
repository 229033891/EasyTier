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
  /** Proto ConfigSource int; default Web (2). */
  source?: number
}) {
  const { config, disabled = true, running = false, secondConfig, source = 2 } = options
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
        source,
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
          template:
            '<div data-stub="config" :data-readonly="String(!!$attrs.readOnly || !!$attrs[\'read-only\'])"><div data-slot="config-toolbar" v-if="$slots[\'config-toolbar\']"><slot name="config-toolbar" /></div></div>',
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

async function enterCombinedConfig(wrapper: Awaited<ReturnType<typeof mountRemote>>) {
  const editBtn = wrapper.find('button[data-label="web.device_management.switch_to_config"]')
  expect(editBtn.exists()).toBe(true)
  await editBtn.trigger('click')
  await settleRemoteManagement()
}

describe('RemoteManagement config vs run pages', () => {
  it('stopped run page shows read-only config + start + node-config, not edit toolbar/run_network', async () => {
    const config = makeFlagConfig()
    const api = makeApi({ config, disabled: true })
    const wrapper = await mountRemote(api)

    try {
      const configStub = wrapper.find('[data-stub="config"]')
      expect(configStub.exists()).toBe(true)
      expect(configStub.attributes('data-readonly')).toBe('true')
      expect(wrapper.find('[data-stub="config"] [data-slot="config-toolbar"]').exists()).toBe(false)
      expect(wrapper.find('button[data-label="web.device_management.start_network"]').exists()).toBe(true)
      expect(wrapper.find('button[data-label="web.device_management.switch_to_config"]').exists()).toBe(true)
      expect(wrapper.find('button[data-label="run_network"]').exists()).toBe(false)
      expect(wrapper.find('button[data-label="web.device_management.save_config"]').exists()).toBe(false)
    } finally {
      wrapper.unmount()
    }
  })

  it('config page has no start/stop/run primary; dirty shows save+discard pair', async () => {
    const config = makeFlagConfig()
    const expectedFlags = snapshotBooleanConfigFields(config)
    const api = makeApi({ config, disabled: true })
    const wrapper = await mountRemote(api)

    try {
      await enterCombinedConfig(wrapper)
      expect(wrapper.find('[data-stub="config"]').exists()).toBe(true)
      expect(wrapper.find('button[data-label="web.device_management.start_network"]').exists()).toBe(false)
      expect(wrapper.find('button[data-label="run_network"]').exists()).toBe(false)
      expect(wrapper.find('button[data-label="web.device_management.disable_network"]').exists()).toBe(false)

      // Clean config: return-to-run, no save pair.
      expect(wrapper.find('button[data-label="web.device_management.switch_to_status"]').exists()).toBe(true)
      expect(wrapper.find('button[data-label="web.device_management.save_config"]').exists()).toBe(false)

      networkConfigRef(wrapper).network_name = 'changed-name'
      await nextTick()
      await flushPromises()

      expect(wrapper.find('button[data-label="web.device_management.switch_to_status"]').exists()).toBe(false)
      const saveButton = wrapper.find('button[data-label="web.device_management.save_config"]')
      const discard = wrapper.find('button[data-label="web.device_management.discard_changes"]')
      expect(saveButton.exists()).toBe(true)
      expect(discard.exists()).toBe(true)

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
      await enterCombinedConfig(wrapper)
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
      expect(wrapper.find('button[data-label="web.device_management.save_config"]').exists()).toBe(false)
      expect(wrapper.find('button[data-label="web.device_management.discard_changes"]').exists()).toBe(false)
    } finally {
      wrapper.unmount()
    }
  })

  it('running page shows stop + node-config; clean edit shows return without save', async () => {
    const config = makeFlagConfig()
    const api = makeApi({ config, disabled: false, running: true })
    const wrapper = await mountRemote(api)

    try {
      expect(wrapper.find('button[data-label="web.device_management.disable_network"]').exists()).toBe(true)
      expect(wrapper.find('button[data-label="web.device_management.switch_to_config"]').exists()).toBe(true)
      expect(wrapper.find('button[data-label="run_network"]').exists()).toBe(false)

      await enterCombinedConfig(wrapper)

      expect(wrapper.find('button[data-label="web.device_management.disable_network"]').exists()).toBe(false)
      expect(wrapper.find('button[data-label="web.device_management.switch_to_status"]').exists()).toBe(true)
      expect(wrapper.find('button[data-label="web.device_management.save_config"]').exists()).toBe(false)

      networkConfigRef(wrapper).network_name = 'dirty-while-running'
      await nextTick()
      await flushPromises()

      expect(wrapper.find('button[data-label="web.device_management.switch_to_status"]').exists()).toBe(false)
      expect(wrapper.find('button[data-label="web.device_management.save_config"]').exists()).toBe(true)
      expect(wrapper.find('button[data-label="web.device_management.discard_changes"]').exists()).toBe(true)
    } finally {
      wrapper.unmount()
    }
  })

  it('web config mode can edit a stopped network (not stuck on read-only/loading)', async () => {
    const config = makeFlagConfig()
    const api = makeApi({ config, disabled: true })
    const wrapper = await mountRemote(api, { mode: 'config' })

    try {
      const configStub = wrapper.find('[data-stub="config"]')
      expect(configStub.exists()).toBe(true)
      expect(configStub.attributes('data-readonly')).toBe('false')
      expect(wrapper.find('[data-slot="config-toolbar"]').exists()).toBe(true)
      expect(wrapper.find('button[data-label="web.device_management.start_network"]').exists()).toBe(false)
      expect(wrapper.find('button[data-label="web.device_management.disable_network"]').exists()).toBe(false)
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
      await enterCombinedConfig(wrapper)
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

  it('P2: Save pending+disabled while running auto-Starts to align', async () => {
    const config = makeFlagConfig()
    const api = makeApi({ config, disabled: false, running: true, source: 2 })
    api.save_config = vi.fn(async () => ({ action: 'pending', reason: 'disabled' }))
    const wrapper = await mountRemote(api)

    try {
      await enterCombinedConfig(wrapper)
      networkConfigRef(wrapper).network_name = 'changed-name'
      await nextTick()
      await flushPromises()

      const saveButton = wrapper.find('button[data-label="web.device_management.save_config"]')
      expect(saveButton.exists()).toBe(true)
      await saveButton.trigger('click')
      await settleRemoteManagement()

      expect(api.save_config).toHaveBeenCalledOnce()
      expect(api.update_network_instance_state).toHaveBeenCalledWith(INSTANCE_ID, false)
    } finally {
      wrapper.unmount()
    }
  })

  it('P2: Save pending+disabled when stopped does not Start', async () => {
    const config = makeFlagConfig()
    const api = makeApi({ config, disabled: true, running: false, source: 2 })
    api.save_config = vi.fn(async () => ({ action: 'pending', reason: 'disabled' }))
    const wrapper = await mountRemote(api)

    try {
      await enterCombinedConfig(wrapper)
      networkConfigRef(wrapper).network_name = 'changed-name'
      await nextTick()
      await flushPromises()

      await wrapper.find('button[data-label="web.device_management.save_config"]').trigger('click')
      await settleRemoteManagement()

      expect(api.save_config).toHaveBeenCalledOnce()
      expect(api.update_network_instance_state).not.toHaveBeenCalled()
    } finally {
      wrapper.unmount()
    }
  })
})

describe('RemoteManagement delete authority', () => {
  it('grays out delete for Web source when deleteAuthority is console', async () => {
    const config = makeFlagConfig()
    const api = makeApi({ config, source: 2 })
    const wrapper = await mountRemote(api, { deleteAuthority: 'console' })
    try {
      await enterCombinedConfig(wrapper)
      const btn = wrapper.find('button[data-label="web.device_management.delete_network"]')
      expect(btn.exists()).toBe(true)
      expect(btn.attributes('disabled')).toBeDefined()
    } finally {
      wrapper.unmount()
    }
  })

  it('keeps delete enabled for User source when deleteAuthority is console', async () => {
    const config = makeFlagConfig()
    const api = makeApi({ config, source: 1 })
    const wrapper = await mountRemote(api, { deleteAuthority: 'console' })
    try {
      await enterCombinedConfig(wrapper)
      const btn = wrapper.find('button[data-label="web.device_management.delete_network"]')
      expect(btn.exists()).toBe(true)
      expect(btn.attributes('disabled')).toBeUndefined()
    } finally {
      wrapper.unmount()
    }
  })

  it('web-console host confirms User-source delete even with deleteAuthority local', async () => {
    confirmRequire.mockClear()
    const config = makeFlagConfig()
    const api = makeApi({ config, source: 1 })
    const wrapper = await mountRemote(api, {
      hostSurface: 'web-console',
      deleteAuthority: 'local',
    })
    try {
      await enterCombinedConfig(wrapper)
      const btn = wrapper.find('button[data-label="web.device_management.delete_network"]')
      expect(btn.exists()).toBe(true)
      await btn.trigger('click')
      await flushPromises()
      expect(confirmRequire).toHaveBeenCalled()
      const arg = confirmRequire.mock.calls[0][0] as { message: string }
      expect(arg.message).toBe('web.device_management.confirm_delete_user_source')
    } finally {
      wrapper.unmount()
    }
  })

  it('gui-client host does not use User-source confirm', async () => {
    confirmRequire.mockClear()
    const config = makeFlagConfig()
    const api = makeApi({ config, source: 1 })
    const wrapper = await mountRemote(api, {
      hostSurface: 'gui-client',
      deleteAuthority: 'local',
    })
    try {
      await enterCombinedConfig(wrapper)
      const btn = wrapper.find('button[data-label="web.device_management.delete_network"]')
      await btn.trigger('click')
      await flushPromises()
      expect(confirmRequire).toHaveBeenCalled()
      const arg = confirmRequire.mock.calls[0][0] as { message: string }
      expect(arg.message).toBe('web.device_management.confirm_delete_network')
    } finally {
      wrapper.unmount()
    }
  })

  it('P0-2: delete rpc_timeout offers retry confirm that re-invokes delete', async () => {
    confirmRequire.mockClear()
    const config = makeFlagConfig()
    const api = makeApi({ config, source: 2 })
    api.delete_network = vi.fn()
      .mockRejectedValueOnce({
        response: { data: { message: 'Timeout(Elapsed(()))', code: 'rpc_timeout' } },
      })
      .mockResolvedValueOnce(undefined)
    const wrapper = await mountRemote(api, { hostSurface: 'web-console' })
    try {
      await enterCombinedConfig(wrapper)
      const btn = wrapper.find('button[data-label="web.device_management.delete_network"]')
      await btn.trigger('click')
      await flushPromises()

      // First confirm: delete
      expect(confirmRequire).toHaveBeenCalled()
      const first = confirmRequire.mock.calls[0][0] as { accept: () => void, header: string }
      expect(first.header).toBe('web.device_management.confirm_delete_network_header')
      first.accept()
      await settleRemoteManagement()

      expect(api.delete_network).toHaveBeenCalledOnce()
      // Second confirm: retry after failure
      expect(confirmRequire.mock.calls.length).toBeGreaterThanOrEqual(2)
      const retry = confirmRequire.mock.calls[confirmRequire.mock.calls.length - 1][0] as {
        header: string
        accept: () => void
      }
      expect(retry.header).toBe('web.device_management.confirm_delete_retry_header')
      retry.accept()
      await settleRemoteManagement()
      expect(api.delete_network).toHaveBeenCalledTimes(2)
    } finally {
      wrapper.unmount()
    }
  })
})
