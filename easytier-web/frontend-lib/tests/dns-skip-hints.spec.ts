import { mount, type VueWrapper } from '@vue/test-utils'
import { describe, expect, it, vi } from 'vitest'
import { defineComponent, h, nextTick } from 'vue'
import DnsForwardersEditor from '../src/components/dns/DnsForwardersEditor.vue'
import DnsHostsEditor from '../src/components/dns/DnsHostsEditor.vue'
import DnsUpstreamEditor from '../src/components/dns/DnsUpstreamEditor.vue'
import {
  asciiLower,
  isValidForwarderDomain,
  isValidHostIpv4,
  isValidIpv6,
  isValidUpstreamDnsEntry,
  staticHostSkipReason,
} from '../src/modules/dnsHostValidate'
import type { DnsForwarder, DnsHostEntry } from '../src/types/network'

vi.mock('vue-i18n', () => ({
  useI18n: () => ({
    t: (key: string, values?: unknown[]) =>
      values ? `${key}:${values.join(',')}` : key,
  }),
}))

const InputStub = (name: string) =>
  defineComponent({
    name,
    props: { modelValue: [String, Number, Array], id: String },
    emits: ['update:modelValue'],
    setup(props) {
      return () =>
        h('input', {
          id: props.id,
          value: Array.isArray(props.modelValue) ? '' : (props.modelValue ?? ''),
          'data-stub': name,
        })
    },
  })

const ButtonStub = defineComponent({
  name: 'Button',
  emits: ['click'],
  setup(_, { slots }) {
    return () => h('button', { 'data-stub': 'button' }, slots.default?.())
  },
})

const mountGlobals = {
  directives: { tooltip: () => {} },
  stubs: {
    AutoComplete: InputStub('AutoComplete'),
    Button: ButtonStub,
    InputNumber: InputStub('InputNumber'),
    InputText: InputStub('InputText'),
  },
}

function hostRow(overrides: Partial<DnsHostEntry> = {}): DnsHostEntry {
  return { name: '', ips: [], ttl_secs: 300, ...overrides }
}

function hints(wrapper: VueWrapper): string[] {
  return wrapper.findAll('.dns-skip-hint').map((w) => w.text())
}

describe('dnsHostValidate mirror (O2)', () => {
  it('staticHostSkipReason mirrors classify_host_name + reject_reason', () => {
    // Cases from easytier-core config/dns.rs invalid_hosts_reports_reasons.
    expect(staticHostSkipReason('app.internal.')).toBeNull()
    expect(staticHostSkipReason('*.com')).toBe('single_label_zone')
    expect(staticHostSkipReason('a.*.b')).toBe('invalid_star')

    expect(staticHostSkipReason('')).toBe('empty')
    expect(staticHostSkipReason('   ')).toBe('empty')
    expect(staticHostSkipReason('*.corp.example')).toBeNull()
    // '*.' loses its dot to trailing-dot stripping first, so both the Rust
    // parser and this mirror report invalid_star (not wildcard_needs_parent).
    expect(staticHostSkipReason('*.')).toBe('invalid_star')
    expect(staticHostSkipReason('*.*.example')).toBe('multi_wildcard')
    expect(staticHostSkipReason('*..example')).toBe('invalid_wildcard_parent')
    expect(staticHostSkipReason('a*b')).toBe('invalid_star')
    expect(staticHostSkipReason('com')).toBe('single_label_zone')
    expect(staticHostSkipReason('app.internal')).toBeNull()
  })

  it('asciiLower folds ASCII only, mirroring Rust to_ascii_lowercase', () => {
    // ASCII A-Z folds.
    expect(asciiLower('Corp.Example')).toBe('corp.example')
    expect(asciiLower('ABCXYZ')).toBe('abcxyz')
    // Non-ASCII is left byte-identical. `toLowerCase()` would fold these
    // (`Σ`→`σ`, `Λ`→`λ`, `İ`→`i`+U+0307), which is the divergence this guards.
    // (Use non-ASCII letters only — an ASCII `K` here would legitimately fold.)
    expect(asciiLower('İΣΛ')).toBe('İΣΛ')
    expect(asciiLower('İΣΛ')).not.toBe('İΣΛ'.toLowerCase())
    expect(asciiLower('İstanbul.com')).not.toBe('İstanbul.com'.toLowerCase())
    // Verdicts stay identical to the Rust path for such a name (both accept).
    expect(staticHostSkipReason('İstanbul.com')).toBeNull()
  })

  it('isValidHostIpv4 mirrors Ipv4Addr::from_str on trimmed input', () => {
    expect(isValidHostIpv4('10.0.0.1')).toBe(true)
    expect(isValidHostIpv4('  10.0.0.1  ')).toBe(true)
    expect(isValidHostIpv4('256.1.1.1')).toBe(false)
    expect(isValidHostIpv4('1.2.3')).toBe(false)
    expect(isValidHostIpv4('1.2.3.4.5')).toBe(false)
    expect(isValidHostIpv4('')).toBe(false)
    expect(isValidHostIpv4('abc')).toBe(false)
  })

  it('isValidIpv6 accepts literals Rust IpAddr::from_str accepts', () => {
    for (const ok of [
      'fd00::1',
      '::1',
      '::',
      '2001:db8::1',
      '1:2:3:4:5:6:7:8',
      '::ffff:1.2.3.4',
      '1:2:3:4:5:6:1.2.3.4',
    ]) {
      expect(isValidIpv6(ok)).toBe(true)
    }
    for (const bad of [
      '',
      ':53',
      '1:::2',
      '12345::',
      '1:2:3:4:5:6:7:8:9',
      '1.2.3.4',
      'gggg::1',
      'fe80::1%eth0',
    ]) {
      expect(isValidIpv6(bad)).toBe(false)
    }
  })

  it('isValidUpstreamDnsEntry mirrors parse_upstream_nameserver', () => {
    // Cases from server.rs parse_upstream_accepts_ip_port_and_url_forms.
    for (const ok of [
      '1.1.1.1',
      '8.8.8.8:5353',
      'udp://9.9.9.9:53',
      'tcp://9.9.9.9',
      'fd00::1',
      '[fd00::1]:5353',
    ]) {
      expect(isValidUpstreamDnsEntry(ok)).toBe(true)
    }
    expect(isValidUpstreamDnsEntry('https://1.1.1.1/dns-query')).toBe(false)
    expect(isValidUpstreamDnsEntry('')).toBe(false)
    expect(isValidUpstreamDnsEntry('notaip')).toBe(false)
    expect(isValidUpstreamDnsEntry('1.1.1.1:')).toBe(false)
    expect(isValidUpstreamDnsEntry('1.1.1.1:99999')).toBe(false)
    expect(isValidUpstreamDnsEntry(':53')).toBe(false)
    // u16 port and bracketed IPv4 are accepted by the Rust parser too.
    expect(isValidUpstreamDnsEntry('1.1.1.1:0')).toBe(true)
    expect(isValidUpstreamDnsEntry('[1.2.3.4]')).toBe(true)
  })

  it('isValidForwarderDomain mirrors normalize_forward_zone', () => {
    expect(isValidForwarderDomain('corp.example')).toBe(true)
    expect(isValidForwarderDomain('Corp.Example.')).toBe(true)
    expect(isValidForwarderDomain('')).toBe(false)
    expect(isValidForwarderDomain('   ')).toBe(false)
    expect(isValidForwarderDomain('...')).toBe(false)
  })
})

describe('DNS editors skip hints (O2)', () => {
  it('hosts editor warns on invalid name, stays quiet on valid/pristine rows', async () => {
    const wrapper = mount(DnsHostsEditor, {
      props: {
        hosts: [
          hostRow({ name: '*.com' }),
          hostRow({ name: 'app.internal', ips: ['10.0.0.1'] }),
          hostRow(),
        ],
      },
      global: mountGlobals,
    })
    await nextTick()
    const text = hints(wrapper)
    expect(text.some((h) => h.includes('dns.hosts.skip.single_label_zone'))).toBe(true)
    expect(wrapper.findAll('.dns-skip-hint')).toHaveLength(1)
  })

  it('hosts editor warns on empty name only when the row carries IPs', async () => {
    const wrapper = mount(DnsHostsEditor, {
      props: { hosts: [hostRow({ name: '', ips: ['10.0.0.1'] })] },
      global: mountGlobals,
    })
    await nextTick()
    expect(hints(wrapper).some((h) => h.includes('dns.hosts.skip.empty'))).toBe(true)
  })

  it('hosts editor lists invalid IPs and flags all-invalid rows', async () => {
    const wrapper = mount(DnsHostsEditor, {
      props: {
        hosts: [
          hostRow({ name: 'app.internal', ips: ['10.0.0.1', '999.1.1.1'] }),
          hostRow({ name: 'db.internal', ips: ['999.1.1.1'] }),
        ],
      },
      global: mountGlobals,
    })
    await nextTick()
    const text = hints(wrapper).join('\n')
    expect(text).toContain('dns.hosts.invalid_ips:999.1.1.1')
    expect(text).toContain('dns.hosts.invalid_ips_all_drop')
    // First row keeps one usable IP → no all-drop for it (exactly one all-drop hint).
    expect(hints(wrapper).filter((h) => h.includes('invalid_ips_all_drop'))).toHaveLength(1)
  })

  it('upstream editor warns only on non-blank invalid entries', async () => {
    const wrapper = mount(DnsUpstreamEditor, {
      props: { servers: ['1.1.1.1', 'https://1.1.1.1/dns-query', ''] },
      global: mountGlobals,
    })
    await nextTick()
    expect(wrapper.findAll('.dns-skip-hint')).toHaveLength(1)
    expect(hints(wrapper)[0]).toContain('dns.upstream.invalid_entry')
  })

  it('forwarders editor warns on invalid servers/domains and skipped rows', async () => {
    const wrapper = mount(DnsForwardersEditor, {
      props: {
        forwarders: [
          { domains: ['corp.example'], servers: ['notaip'] } as DnsForwarder,
          { domains: ['corp.example'], servers: [] } as DnsForwarder,
          { domains: [], servers: [] } as DnsForwarder,
          { domains: ['corp.example'], servers: ['1.1.1.1'] } as DnsForwarder,
        ],
      },
      global: mountGlobals,
    })
    await nextTick()
    const text = hints(wrapper).join('\n')
    expect(text).toContain('dns.forwarders.invalid_servers:notaip')
    // Row 0 (no usable server) and row 1 (no servers) are skipped; blank and
    // healthy rows stay quiet.
    expect(hints(wrapper).filter((h) => h.includes('row_skipped'))).toHaveLength(2)
  })

  it('forwarders editor warns on blank domains', async () => {
    const wrapper = mount(DnsForwardersEditor, {
      props: {
        forwarders: [{ domains: ['...'], servers: ['1.1.1.1'] } as DnsForwarder],
      },
      global: mountGlobals,
    })
    await nextTick()
    const text = hints(wrapper).join('\n')
    expect(text).toContain('dns.forwarders.invalid_domains:...')
    expect(text).toContain('dns.forwarders.row_skipped')
  })
})
