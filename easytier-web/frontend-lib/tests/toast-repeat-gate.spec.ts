import { describe, expect, it, vi } from 'vitest'
import {
  TOAST_REPEAT_COOLDOWN_MS,
  addThrottledToast,
  createToastRepeatGate,
} from '../src/modules/toast'

describe('toast repeat gate', () => {
  it('allows the first report and suppresses repeats inside the cooldown', () => {
    const gate = createToastRepeatGate(1_000)
    expect(gate.allow('poll:devices', 0)).toBe(true)
    expect(gate.allow('poll:devices', 999)).toBe(false)
    // Exactly at the boundary the window has elapsed again.
    expect(gate.allow('poll:devices', 1_000)).toBe(true)
  })

  it('keeps independent windows per key', () => {
    const gate = createToastRepeatGate(1_000)
    expect(gate.allow('poll:devices', 0)).toBe(true)
    // A different failure is not suppressed by the first one.
    expect(gate.allow('poll:archive', 10)).toBe(true)
    expect(gate.allow('poll:devices', 10)).toBe(false)
  })

  it('defaults to the shared 30s window', () => {
    const gate = createToastRepeatGate()
    expect(gate.allow('k', 0)).toBe(true)
    expect(gate.allow('k', TOAST_REPEAT_COOLDOWN_MS - 1)).toBe(false)
    expect(gate.allow('k', TOAST_REPEAT_COOLDOWN_MS)).toBe(true)
  })

  it('reset() clears recorded timestamps', () => {
    const gate = createToastRepeatGate(60_000)
    expect(gate.allow('k', 0)).toBe(true)
    expect(gate.allow('k', 1)).toBe(false)
    gate.reset()
    expect(gate.allow('k', 1)).toBe(true)
  })

  it('addThrottledToast reports whether the toast was added', () => {
    const toast = { add: vi.fn() }
    const gate = createToastRepeatGate(1_000)
    const message = { severity: 'error', detail: 'boom' }

    expect(addThrottledToast(toast, gate, 'poll:devices', message, 0)).toBe(true)
    expect(addThrottledToast(toast, gate, 'poll:devices', message, 500)).toBe(false)
    expect(addThrottledToast(toast, gate, 'poll:devices', message, 1_500)).toBe(true)

    expect(toast.add).toHaveBeenCalledTimes(2)
    expect(toast.add).toHaveBeenNthCalledWith(1, message)
  })
})
