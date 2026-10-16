import { describe, expect, it, vi } from 'vitest'
import { commitChipTokens, onChipsFocusOut, onChipsKeydown } from '../src/modules/chipsInput'

function makeInputEvent(
  type: 'keydown' | 'focusout',
  value: string,
  key?: string,
): { event: KeyboardEvent | FocusEvent; input: HTMLInputElement } {
  const input = document.createElement('input')
  input.value = value
  const event = new Event(type === 'keydown' ? 'keydown' : 'focusout', {
    bubbles: true,
    cancelable: true,
  }) as KeyboardEvent & FocusEvent
  Object.defineProperty(event, 'target', { value: input })
  if (type === 'keydown' && key !== undefined) {
    Object.defineProperty(event, 'key', { value: key })
    Object.defineProperty(event, 'isComposing', { value: false })
  }
  return { event, input }
}

describe('chipsInput', () => {
  it('commitChipTokens dedupe-append tokens and returns a NEW array', () => {
    const values = ['10.0.0.1']
    const next = commitChipTokens(values, '10.0.0.2, 10.0.0.1，10.0.0.3')
    expect(next).toEqual(['10.0.0.1', '10.0.0.2', '10.0.0.3'])
    // Pure: the input array is untouched; callers assign the new array back.
    expect(values).toEqual(['10.0.0.1'])
  })

  it('onChipsKeydown assigns a fresh array so PrimeVue modelValue watch fires', () => {
    const initial = [] as string[]
    const row = { peers: initial }
    const { event, input } = makeInputEvent('keydown', '10.1.2.3', 'Enter')
    const prevent = vi.spyOn(event, 'preventDefault')
    onChipsKeydown(event as KeyboardEvent, row, 'peers')
    expect(row.peers).toEqual(['10.1.2.3'])
    // Reference MUST change: PrimeVue re-renders chips only on modelValue
    // identity change (shallow watch), not on in-place splice.
    expect(row.peers).not.toBe(initial)
    expect(input.value).toBe('')
    expect(prevent).toHaveBeenCalled()
  })

  it('onChipsFocusOut assigns a fresh array on blur', () => {
    const row = { peers: [] as string[] }
    const { event, input } = makeInputEvent('focusout', '8.8.8.8')
    onChipsFocusOut(event as FocusEvent, row, 'peers')
    expect(row.peers).toEqual(['8.8.8.8'])
    expect(input.value).toBe('')
  })

  it('onChipsFocusOut clears whitespace-only drafts without creating non-array chips', () => {
    const row: Record<string, unknown> = { peers: [] as string[] }
    const { event, input } = makeInputEvent('focusout', '   ')
    onChipsFocusOut(event as FocusEvent, row, 'peers')
    expect(row.peers).toEqual([])
    expect(input.value).toBe('')
  })

  it('onChipsKeydown clears whitespace-only drafts on Enter', () => {
    const row: Record<string, unknown> = { peers: [] }
    const { event, input } = makeInputEvent('keydown', '  ', 'Enter')
    onChipsKeydown(event as KeyboardEvent, row, 'peers')
    expect(row.peers).toEqual([])
    expect(input.value).toBe('')
    expect(event.defaultPrevented).toBe(true)
  })

  it('initializes missing backing arrays to a real Array (never undefined)', () => {
    const row: Record<string, unknown> = {}
    const { event, input } = makeInputEvent('focusout', '10.0.0.9')
    onChipsFocusOut(event as FocusEvent, row, 'peers')
    expect(Array.isArray(row.peers)).toBe(true)
    expect(row.peers).toEqual(['10.0.0.9'])
    expect(input.value).toBe('')
  })
})
