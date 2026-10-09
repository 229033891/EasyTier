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
  it('commitChipTokens splits on comma and skips duplicates', () => {
    const values = ['10.0.0.1']
    expect(commitChipTokens(values, '10.0.0.2, 10.0.0.1，10.0.0.3')).toBe(true)
    expect(values).toEqual(['10.0.0.1', '10.0.0.2', '10.0.0.3'])
  })

  it('onChipsKeydown commits on Enter and clears the input', () => {
    const values: string[] = []
    const { event, input } = makeInputEvent('keydown', '10.1.2.3', 'Enter')
    const prevent = vi.spyOn(event, 'preventDefault')
    onChipsKeydown(event as KeyboardEvent, values)
    expect(values).toEqual(['10.1.2.3'])
    expect(input.value).toBe('')
    expect(prevent).toHaveBeenCalled()
  })

  it('onChipsFocusOut commits pending typed text', () => {
    const values: string[] = []
    const { event, input } = makeInputEvent('focusout', '8.8.8.8')
    onChipsFocusOut(event as FocusEvent, values)
    expect(values).toEqual(['8.8.8.8'])
    expect(input.value).toBe('')
  })

  it('onChipsFocusOut clears whitespace-only drafts', () => {
    const values: string[] = []
    const { event, input } = makeInputEvent('focusout', '   ')
    onChipsFocusOut(event as FocusEvent, values)
    expect(values).toEqual([])
    expect(input.value).toBe('')
  })

  it('onChipsKeydown clears whitespace-only drafts on Enter', () => {
    const values: string[] = []
    const { event, input } = makeInputEvent('keydown', '  ', 'Enter')
    onChipsKeydown(event as KeyboardEvent, values)
    expect(values).toEqual([])
    expect(input.value).toBe('')
  })

  it('ignores undefined values arrays', () => {
    const { event } = makeInputEvent('keydown', '1.1.1.1', 'Enter')
    expect(() => onChipsKeydown(event as KeyboardEvent, undefined)).not.toThrow()
  })
})
