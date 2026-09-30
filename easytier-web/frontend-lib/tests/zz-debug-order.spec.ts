import { mount } from '@vue/test-utils'
import PrimeVue from 'primevue/config'
import { describe, expect, it, vi } from 'vitest'
import { reactive } from 'vue'
import Config from '../src/components/Config.vue'
import { DEFAULT_NETWORK_CONFIG, type NetworkConfig } from '../src/types/network'

vi.mock('vue-i18n', () => ({
  useI18n: () => ({ t: (key: string) => key }),
}))

describe('debug toggle order', () => {
  it('dumps toggle buttons in DOM order', async () => {
    const cfg = reactive({
      ...DEFAULT_NETWORK_CONFIG(),
      enable_relay_network_whitelist: true,
      enable_manual_routes: false,
      enable_socks5: true,
      vpn_portal_config: { wireguard_listen: 'a', clients: [] },
    }) as NetworkConfig

    const wrapper = mount(Config, {
      props: { curNetwork: cfg, hostname: 'h' },
      global: {
        plugins: [PrimeVue],
        directives: { tooltip: () => {} },
      },
    })
    await new Promise((resolve) => setTimeout(resolve, 20))
    await wrapper.vm.$nextTick()

    const toggles = wrapper.findAll('[data-pc-name="togglebutton"], button[data-stub="toggle-button"]')
    const info = toggles.map((b, index) => {
      const pressed = b.attributes('aria-pressed')
      const owner = b.element.closest('.flex.flex-col')?.querySelector('label')?.textContent?.trim()
      return `${index}:${pressed}|${owner ?? '?'}`
    })
    console.log('TOGGLES>>', info.join('  '))
    expect(toggles.length).toBeGreaterThan(0)
  })
})
