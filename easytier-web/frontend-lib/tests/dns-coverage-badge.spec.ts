import { mount } from '@vue/test-utils'
import { describe, expect, it, vi } from 'vitest'
import DnsCoverageBadge from '../src/components/dns/DnsCoverageBadge.vue'

vi.mock('vue-i18n', () => ({
  useI18n: () => ({
    t: (key: string) => key,
  }),
}))

vi.mock('floating-vue', () => ({
  VTooltip: {},
}))

describe('DnsCoverageBadge', () => {
  it('applies covered modifier class from state', () => {
    const wrapper = mount(DnsCoverageBadge, {
      props: { state: 'covered' },
      global: {
        directives: {
          tooltip: () => {},
        },
      },
    })
    expect(wrapper.classes()).toContain('dns-coverage-badge--covered')
    expect(wrapper.text()).toContain('dns.coverage.covered')
    wrapper.unmount()
  })

  it('links manual_required badge to docs url', () => {
    const wrapper = mount(DnsCoverageBadge, {
      props: { state: 'manual_required' },
      global: {
        directives: {
          tooltip: () => {},
        },
      },
    })
    expect(wrapper.element.tagName.toLowerCase()).toBe('a')
    expect(wrapper.classes()).toContain('dns-coverage-badge--manual_required')
    wrapper.unmount()
  })
})
