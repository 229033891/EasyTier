import { mount } from '@vue/test-utils'
import PrimeVue from 'primevue/config'
import { describe, expect, it, vi } from 'vitest'
import { defineComponent, h, nextTick, ref } from 'vue'

vi.mock('vue-i18n', () => ({
  useI18n: () => ({ t: (key: string) => key }),
}))

import PolicyRoutesEditor from '../src/components/policy/PolicyRoutesEditor.vue'

/** Host wrapper: simulates Config.vue's v-model:routes binding (defineModel('routes')). */
const Host = defineComponent({
  setup() {
    const routes = ref<any[]>([{ cidr: '1.1.1.1/32', domains: [], peers: [] }])
    const routesRef = routes
    return () => h(PolicyRoutesEditor, {
      'routes': routesRef.value as any,
      'onUpdate:routes': (v: any[]) => { routesRef.value = v },
    })
  },
})

function mountEditor() {
  return mount(Host, {
    global: {
      plugins: [PrimeVue],
      directives: {
        tooltip: { mounted: () => {}, updated: () => {} },
      },
    },
  })
}

function peerRoutes(wrapper: any) {
  return wrapper.findComponent(PolicyRoutesEditor).props('routes') as any[]
}

describe('PolicyRoutesEditor peers chips (real AutoComplete mount)', () => {
  it('enter commits a chip into row.peers AND renders it in the DOM', async () => {
    const wrapper = mountEditor()
    const input = wrapper.find('#policy_route_peers_0 input')
    expect(input.exists(), 'peers AutoComplete inner input should exist').toBe(true)

    await input.setValue('10.10.10.50')
    await input.trigger('keydown', { key: 'Enter' })
    await nextTick()
    await nextTick()

    // Model: row.peers must contain the committed value.
    expect(peerRoutes(wrapper)[0].peers).toEqual(['10.10.10.50'])

    // DOM: the chip must actually render (PrimeVue renders chips from its
    // d_value snapshot, refreshed by a shallow modelValue watch — reference
    // replacement is what makes this work).
    expect(wrapper.text()).toContain('10.10.10.50')
    expect((input.element as HTMLInputElement).value).toBe('')
  })

  it('fullwidth comma splits tokens on focusout', async () => {
    const wrapper = mountEditor()
    const input = wrapper.find('#policy_route_peers_0 input')
    expect(input.exists()).toBe(true)

    await input.setValue('10.10.10.50，10.10.10.51')
    await input.trigger('focusout')
    await nextTick()
    await nextTick()

    expect(peerRoutes(wrapper)[0].peers).toEqual(['10.10.10.50', '10.10.10.51'])
    expect(wrapper.text()).toContain('10.10.10.50')
  })

  it('domains: enter commits a chip and renders it (same PrimeVue shallow-watch bug)', async () => {
    const wrapper = mountEditor()
    const input = wrapper.find('#policy_route_domains_0 input')
    expect(input.exists(), 'domains AutoComplete inner input should exist').toBe(true)

    await input.setValue('*.atlassian.net')
    await input.trigger('keydown', { key: 'Enter' })
    await nextTick()
    await nextTick()

    expect(peerRoutes(wrapper)[0].domains).toEqual(['*.atlassian.net'])
    expect(wrapper.text()).toContain('*.atlassian.net')
    expect((input.element as HTMLInputElement).value).toBe('')
  })
})
