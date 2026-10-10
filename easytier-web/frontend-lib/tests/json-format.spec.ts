import { describe, expect, it } from 'vitest'
import { formatCompactJson, MAX_INLINE_WIDTH } from '../src/modules/jsonFormat'

/** 截图里的真实载荷：PeerConnAdded 事件（u64 走 protobuf JSON 是字符串）。 */
const PEER_CONN_ADDED = {
  conn_id: '1f93d902-389c-4131-b384-b7259bd115fb',
  my_peer_id: 3431683332,
  peer_id: 1665716567,
  features: ['liveness-echo-v1'],
  tunnel: {
    tunnel_type: 'tcp',
    local_addr: { url: 'tcp://0.0.0.0:11010' },
    remote_addr: { url: 'tcp://180.173.152.19:4496' },
    resolved_remote_addr: { url: 'tcp://180.173.152.19:4496' },
  },
  stats: { rx_bytes: '90', tx_bytes: '106', rx_packets: '0', tx_packets: '1' },
  network_name: 'et',
}

function lineCount(text: string): number {
  return text.split('\n').length
}

function maxLineWidth(text: string): number {
  return Math.max(...text.split('\n').map(l => l.length))
}

describe('formatCompactJson', () => {
  it('collapses all-scalar wrappers instead of spending 3 lines each', () => {
    expect(formatCompactJson({ url: 'tcp://0.0.0.0:11010' }))
      .toBe('{"url":"tcp://0.0.0.0:11010"}')
  })

  it('keeps structure identical to JSON.stringify (only re-flows)', () => {
    const compact = formatCompactJson(PEER_CONN_ADDED)
    expect(JSON.parse(compact)).toEqual(JSON.parse(JSON.stringify(PEER_CONN_ADDED)))
  })

  it('collapses scalar arrays and scalar-only objects onto one line', () => {
    expect(formatCompactJson({ features: ['a', 'b'] })).toBe('{"features":["a","b"]}')
    expect(formatCompactJson({ stats: { rx_bytes: '90', tx_bytes: '106' } }))
      .toBe('{"stats":{"rx_bytes":"90","tx_bytes":"106"}}')
  })

  it('keeps a block expanded when collapsing it would overflow the width budget', () => {
    // tunnel 整块拼起来 ~170 字符，远超预算 → 保持展开；
    // 但里面三个单键 url 包裹各自很短 → 各自折叠。
    const out = formatCompactJson(PEER_CONN_ADDED)
    expect(out).toContain('"tunnel":{\n')
    expect(out).toContain(' "local_addr":{"url":"tcp://0.0.0.0:11010"},')
    expect(out).toContain(' "remote_addr":{"url":"tcp://180.173.152.19:4496"},')
  })

  it('is materially shorter than the 2-space default on a real payload', () => {
    const before = JSON.stringify(PEER_CONN_ADDED, null, 2)
    const after = formatCompactJson(PEER_CONN_ADDED)

    // 折叠掉 3 个单键 url 包裹（各省 2 行）+ features 数组（省 2 行）+ stats 对象（省 4 行）
    expect(lineCount(before)).toBe(27)
    expect(lineCount(after)).toBe(14)
    expect(lineCount(after)).toBeLessThan(lineCount(before) * 0.6)

    // 折叠后 key 和值同处一行，所以最长行反而变长（原先分散在多行）。
    // 正因为如此必须同时把 <pre> 撑满宽度，否则会在窄盒里折行。
    expect(maxLineWidth(after)).toBeGreaterThan(maxLineWidth(before))
    expect(maxLineWidth(after)).toBeLessThanOrEqual(MAX_INLINE_WIDTH + 8)
  })

  it('handles scalars and empty containers', () => {
    expect(formatCompactJson('hello')).toBe('"hello"')
    expect(formatCompactJson(42)).toBe('42')
    expect(formatCompactJson(true)).toBe('true')
    expect(formatCompactJson(null)).toBe('null')
    expect(formatCompactJson(undefined)).toBe('null')
    expect(formatCompactJson({})).toBe('{}')
    expect(formatCompactJson([])).toBe('[]')
    // 空数组 / 空对象只占一个 token，不该让父对象多展开
    expect(formatCompactJson({ a: [], b: {} })).toBe('{"a":[],"b":{}}')
    expect(formatCompactJson([1, 'two', null])).toBe('[1,"two",null]')
    expect(formatCompactJson({ items: [1, 2] })).toBe('{"items":[1,2]}')
  })

  it('collapses fully-scalar nesting instead of indenting every level', () => {
    expect(formatCompactJson({ a: { b: { c: 1 } } })).toBe('{"a":{"b":{"c":1}}}')
  })

  it('indents relative to its own depth when a nested block must expand', () => {
    const wide = 'x'.repeat(60)
    const out = formatCompactJson({ outer: { wide, inner: 1 } })
    expect(out).toBe(`{\n "outer":{\n  "wide":"${wide}",\n  "inner":1\n }\n}`)
  })

  it('escapes strings so the output stays valid JSON', () => {
    expect(formatCompactJson({ note: 'say "hi", ok' }))
      .toBe('{"note":"say \\"hi\\", ok"}')
    expect(() => JSON.parse(formatCompactJson({ note: 'say "hi", ok' }))).not.toThrow()
  })

  it('survives circular references instead of throwing', () => {
    const node: Record<string, unknown> = { name: 'root' }
    node.self = node
    const out = formatCompactJson(node)
    expect(out).toContain('[Circular]')
    expect(() => JSON.parse(out.replace('"[Circular]"', 'null'))).not.toThrow()
  })

  it('does not blow up on a throwing getter', () => {
    const bad = {
      get boom() { throw new Error('nope') },
    }
    expect(() => formatCompactJson(bad)).not.toThrow()
  })

  it('handles long scalar arrays without dropping entries', () => {
    const ports = Array.from({ length: 40 }, (_, i) => i)
    const out = formatCompactJson({ ports })
    expect(JSON.parse(out)).toEqual({ ports })
  })
})