import { mount } from '@vue/test-utils'
import { describe, expect, it, vi } from 'vitest'
import { defineComponent, h, nextTick, reactive } from 'vue'
import Config from '../src/components/Config.vue'
import { DEFAULT_NETWORK_CONFIG, type NetworkConfig } from '../src/types/network'

vi.mock('primevue', async () => {
  const actual = await vi.importActual<typeof import('primevue')>('primevue')
  return {
    ...actual,
    useToast: () => ({ add: vi.fn() }),
    useConfirm: () => ({ require: vi.fn() }),
  }
})

vi.mock('vue-i18n', () => ({
  useI18n: () => ({ t: (k: string) => k }),
}))

const PassThrough = defineComponent({
  name: 'PassThrough',
  setup(_, { slots }) {
    return () => h('div', slots.default?.())
  },
})

const genericStub = (name: string) =>
  defineComponent({
    name,
    props: { modelValue: { type: [String, Number, Boolean, Array, Object], default: undefined } },
    emits: ['update:modelValue'],
    setup(props, { slots }) {
      return () => h('div', { 'data-stub': name, 'data-value': String(props.modelValue ?? '') }, slots.default?.())
    },
  })

const AclManagerStub = defineComponent({
  name: 'AclManager',
  setup: (_, { slots }) => () => h('div', slots.default?.()),
})

function mountConfig(readOnly: boolean, hideRunButton?: boolean) {
  const curNetwork = reactive({
    ...DEFAULT_NETWORK_CONFIG(),
    instance_id: '00000000-0000-0000-0000-000000000001',
    network_name: 'ro-mesh',
  }) as NetworkConfig

  const wrapper = mount(Config, {
    props: { curNetwork, readOnly, hideRunButton },
    global: {
      directives: { tooltip: () => {} },
      stubs: {
        AclManager: AclManagerStub,
        AutoComplete: genericStub('AutoComplete'),
        Button: genericStub('Button'),
        Checkbox: genericStub('Checkbox'),
        Dialog: genericStub('Dialog'),
        Divider: genericStub('Divider'),
        InputGroup: PassThrough,
        InputGroupAddon: PassThrough,
        InputNumber: genericStub('InputNumber'),
        InputText: genericStub('InputText'),
        MultiSelect: genericStub('MultiSelect'),
        Password: genericStub('Password'),
        Select: genericStub('Select'),
        SelectButton: genericStub('SelectButton'),
        ToggleSwitch: genericStub('ToggleSwitch'),
        UrlListInput: genericStub('UrlListInput'),
      },
    },
  })
  return { curNetwork, wrapper }
}

/** 只读模式下打开的折叠面板数（readOnly 初始会把全部面板展开）。 */
function expandedToggleStates(wrapper: ReturnType<typeof mount>['wrapper']): boolean[] {
  return wrapper
    .findAll('.p-panel-toggle-button')
    .map((el) => el.attributes('aria-expanded') === 'true')
}

describe('Config read-only panel collapse', () => {
  it('read-only: locks form content but leaves the root subtree interactive', async () => {
    const { wrapper } = mountConfig(true)
    await nextTick()

    // 根节点不再 inert —— 否则整棵子树（含标题栏）都无法点击
    expect(wrapper.find('.config--readonly').attributes('inert')).toBeUndefined()

    // 每个面板的内容区都被锁定
    const contents = wrapper.findAll('[data-pc-section="content"]')
    expect(contents.length).toBeGreaterThan(0)
    for (const content of contents) {
      expect(content.attributes('inert')).toBeDefined()
    }

    // 标题栏没有被锁
    for (const header of wrapper.findAll('[data-pc-section="header"]')) {
      expect(header.attributes('inert')).toBeUndefined()
    }
  })

  it('read-only: header click still collapses and expands a panel', async () => {
    const { wrapper } = mountConfig(true)
    await nextTick()

    // 初始全部展开
    expect(expandedToggleStates(wrapper).every(Boolean)).toBe(true)

    const header = wrapper.findAll('[data-pc-section="header"]')[0]
    await header.trigger('click')
    await nextTick()
    expect(expandedToggleStates(wrapper)[0]).toBe(false)

    await header.trigger('click')
    await nextTick()
    expect(expandedToggleStates(wrapper)[0]).toBe(true)
  })

  it('editable mode: nothing is inert', async () => {
    const { wrapper } = mountConfig(false)
    await nextTick()

    expect(wrapper.find('.config--readonly').exists()).toBe(false)
    const contents = wrapper.findAll('[data-pc-section="content"]')
    expect(contents.length).toBeGreaterThan(0)
    for (const content of contents) {
      expect(content.attributes('inert')).toBeUndefined()
    }
  })

  it('read-only: hides the out-of-panel run button even when hideRunButton is false', async () => {
    const { wrapper } = mountConfig(true, false)
    await nextTick()
    expect(wrapper.find('.config-run-actions').exists()).toBe(false)
  })

  it('editable mode with hideRunButton=false still renders the run button', async () => {
    const { wrapper } = mountConfig(false, false)
    await nextTick()
    expect(wrapper.find('.config-run-actions').exists()).toBe(true)
  })

  it('read-only: config model is not mutated by toggling panels', async () => {
    const { curNetwork, wrapper } = mountConfig(true)
    await nextTick()
    const before = JSON.stringify(curNetwork)

    const header = wrapper.findAll('[data-pc-section="header"]')[0]
    await header.trigger('click')
    await nextTick()
    await header.trigger('click')
    await nextTick()

    expect(JSON.stringify(curNetwork)).toBe(before)
  })
})